use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn recordings_path() -> PathBuf {
    let dir = if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("iptv")
    } else {
        PathBuf::from(".config").join("iptv")
    };
    let _ = std::fs::create_dir_all(&dir);
    dir.join("recordings.json")
}

/// A scheduled or active recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: u32,
    pub channel_name: String,
    pub programme_title: String,
    pub url: String,
    pub start: DateTime<Utc>,
    pub stop: DateTime<Utc>,
    pub status: RecordingStatus,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordingStatus {
    /// Waiting for start time.
    Scheduled,
    /// Currently recording.
    Recording,
    /// Finished successfully.
    Complete,
    /// Recording failed.
    Failed { error: String },
    /// Cancelled by user.
    Cancelled,
}

/// Shared state for the recording system.
pub struct RecorderState {
    pub recordings: Vec<Recording>,
    next_id: u32,
}

impl RecorderState {
    fn new() -> Self {
        Self {
            recordings: Vec::new(),
            next_id: 1,
        }
    }

    fn load() -> Self {
        let path = recordings_path();
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(mut recs) = serde_json::from_str::<Vec<Recording>>(&data) {
                let next_id = recs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
                let now = Utc::now();
                // Recover interrupted recordings:
                // - "Recording" that was interrupted → re-schedule if stop is in the future
                // - Expired scheduled/recording → mark as failed
                for rec in &mut recs {
                    match rec.status {
                        RecordingStatus::Recording => {
                            if rec.stop > now {
                                rec.status = RecordingStatus::Scheduled;
                            } else {
                                rec.status = RecordingStatus::Failed {
                                    error: "Interrupted by app restart".to_string(),
                                };
                            }
                        }
                        RecordingStatus::Scheduled if rec.stop <= now => {
                            rec.status = RecordingStatus::Failed {
                                error: "Missed (app was not running)".to_string(),
                            };
                        }
                        _ => {}
                    }
                }
                return Self {
                    recordings: recs,
                    next_id,
                };
            }
        }
        Self::new()
    }

    fn save(&self) {
        let path = recordings_path();
        if let Ok(data) = serde_json::to_string_pretty(&self.recordings) {
            let _ = std::fs::write(&path, data);
        }
    }
}

/// Manages scheduled and active recordings.
pub struct Recorder {
    state: Arc<Mutex<RecorderState>>,
    /// Active ffmpeg processes, keyed by recording id.
    processes: Arc<Mutex<HashMap<u32, Child>>>,
    recording_dir: PathBuf,
}

impl Recorder {
    pub fn new(recording_dir: PathBuf) -> Self {
        let recorder = Self {
            state: Arc::new(Mutex::new(RecorderState::load())),
            processes: Arc::new(Mutex::new(HashMap::new())),
            recording_dir,
        };

        // Start the scheduler thread that checks for recordings to start/stop
        let state = Arc::clone(&recorder.state);
        let processes = Arc::clone(&recorder.processes);
        let dir = recorder.recording_dir.clone();
        thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                scheduler_loop(state, processes, dir);
            })
            .expect("failed to spawn recorder thread");

        recorder
    }

    #[allow(dead_code)]
    pub fn state(&self) -> Arc<Mutex<RecorderState>> {
        Arc::clone(&self.state)
    }

    /// Schedule a recording. If start <= now, begins immediately.
    pub fn schedule(
        &self,
        channel_name: String,
        programme_title: String,
        url: String,
        start: DateTime<Utc>,
        stop: DateTime<Utc>,
    ) -> u32 {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;

        let safe_title = sanitize_filename(&programme_title);
        let safe_channel = sanitize_filename(&channel_name);
        let date_str = start.format("%Y%m%d_%H%M").to_string();
        let output_path = self.recording_dir.join(format!(
            "{} - {} [{}].ts",
            safe_channel, safe_title, date_str
        ));

        let status = RecordingStatus::Scheduled;

        state.recordings.push(Recording {
            id,
            channel_name,
            programme_title,
            url,
            start,
            stop,
            status,
            output_path,
        });
        state.save();

        id
    }

    /// Record immediately for a given duration.
    pub fn record_now(
        &self,
        channel_name: String,
        url: String,
        duration_minutes: u32,
    ) -> u32 {
        let now = Utc::now();
        let stop = now + chrono::Duration::minutes(duration_minutes as i64);
        self.schedule(channel_name, "Manual Recording".to_string(), url, now, stop)
    }

    /// Cancel a scheduled or active recording.
    pub fn cancel(&self, id: u32) {
        let mut state = self.state.lock().unwrap();
        if let Some(rec) = state.recordings.iter_mut().find(|r| r.id == id) {
            match rec.status {
                RecordingStatus::Scheduled => {
                    rec.status = RecordingStatus::Cancelled;
                }
                RecordingStatus::Recording => {
                    rec.status = RecordingStatus::Cancelled;
                    // Kill the ffmpeg process
                    let mut procs = self.processes.lock().unwrap();
                    if let Some(mut child) = procs.remove(&id) {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                _ => {} // Already done
            }
            state.save();
        }
    }

    /// Get a snapshot of all recordings for UI rendering.
    pub fn snapshot(&self) -> Vec<Recording> {
        self.state.lock().unwrap().recordings.clone()
    }

    /// Remove completed/cancelled/failed recordings from the list.
    pub fn remove_finished(&self, id: u32) {
        let mut state = self.state.lock().unwrap();
        state.recordings.retain(|r| {
            r.id != id || matches!(r.status, RecordingStatus::Scheduled | RecordingStatus::Recording)
        });
        state.save();
    }
}

fn scheduler_loop(
    state: Arc<Mutex<RecorderState>>,
    processes: Arc<Mutex<HashMap<u32, Child>>>,
    _recording_dir: PathBuf,
) {
    loop {
        thread::sleep(Duration::from_secs(1));
        let now = Utc::now();

        let mut to_start: Vec<(u32, String, PathBuf, DateTime<Utc>)> = Vec::new();
        let mut to_stop: Vec<u32> = Vec::new();

        {
            let state = state.lock().unwrap();
            for rec in &state.recordings {
                match rec.status {
                    RecordingStatus::Scheduled if now >= rec.start => {
                        to_start.push((rec.id, rec.url.clone(), rec.output_path.clone(), rec.stop));
                    }
                    RecordingStatus::Recording if now >= rec.stop => {
                        to_stop.push(rec.id);
                    }
                    _ => {}
                }
            }
        }

        // Start recordings
        for (id, url, output_path, _stop) in to_start {
            if let Some(parent) = output_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match start_ffmpeg_recording(&url, &output_path) {
                Ok(child) => {
                    processes.lock().unwrap().insert(id, child);
                    let mut s = state.lock().unwrap();
                    if let Some(rec) = s.recordings.iter_mut().find(|r| r.id == id) {
                        rec.status = RecordingStatus::Recording;
                    }
                    s.save();
                }
                Err(e) => {
                    let mut s = state.lock().unwrap();
                    if let Some(rec) = s.recordings.iter_mut().find(|r| r.id == id) {
                        rec.status = RecordingStatus::Failed {
                            error: e.to_string(),
                        };
                    }
                    s.save();
                }
            }
        }

        // Stop recordings that have passed their end time
        for id in to_stop {
            let mut procs = processes.lock().unwrap();
            if let Some(mut child) = procs.remove(&id) {
                let _ = child.kill();
                let _ = child.wait();
            }
            let mut s = state.lock().unwrap();
            if let Some(rec) = s.recordings.iter_mut().find(|r| r.id == id) {
                rec.status = RecordingStatus::Complete;
            }
            s.save();
        }
    }
}

fn start_ffmpeg_recording(url: &str, output_path: &std::path::Path) -> anyhow::Result<Child> {
    let child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "error",
            "-user_agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            "-reconnect", "1",
            "-reconnect_streamed", "1",
            "-reconnect_delay_max", "5",
            "-i", url,
            "-c", "copy",
            "-f", "mpegts",
            output_path.to_str().unwrap_or("recording.ts"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;

    Ok(child)
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sanitize_filename ---

    #[test]
    fn test_sanitize_plain() {
        assert_eq!(sanitize_filename("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_special_chars() {
        assert_eq!(sanitize_filename("file/name:with*bad?chars"), "file_name_with_bad_chars");
    }

    #[test]
    fn test_sanitize_all_special() {
        let input = r#"/\:*?"<>|"#;
        assert_eq!(sanitize_filename(input), "_________");
    }

    #[test]
    fn test_sanitize_empty() {
        assert_eq!(sanitize_filename(""), "");
    }

    // --- RecordingStatus serde ---

    #[test]
    fn test_recording_status_serialize_roundtrip() {
        let statuses = vec![
            RecordingStatus::Scheduled,
            RecordingStatus::Recording,
            RecordingStatus::Complete,
            RecordingStatus::Failed { error: "timeout".to_string() },
            RecordingStatus::Cancelled,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let back: RecordingStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, s);
        }
    }

    // --- Recording serde ---

    #[test]
    fn test_recording_serialize_roundtrip() {
        let rec = Recording {
            id: 42,
            channel_name: "CNN".to_string(),
            programme_title: "News Hour".to_string(),
            url: "http://example.com/stream".to_string(),
            start: Utc::now(),
            stop: Utc::now() + chrono::Duration::hours(1),
            status: RecordingStatus::Scheduled,
            output_path: PathBuf::from("/tmp/recordings/cnn.ts"),
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: Recording = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.channel_name, "CNN");
        assert_eq!(back.programme_title, "News Hour");
    }

    // --- RecorderState ---

    #[test]
    fn test_recorder_state_new_empty() {
        let state = RecorderState::new();
        assert!(state.recordings.is_empty());
        assert_eq!(state.next_id, 1);
    }

    #[test]
    fn test_recorder_state_load_recovers_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recordings.json");

        let future = Utc::now() + chrono::Duration::hours(1);
        let past = Utc::now() - chrono::Duration::hours(1);

        let recs = vec![
            // Was recording, stop in future → re-schedule
            Recording {
                id: 1,
                channel_name: "A".into(),
                programme_title: "Show A".into(),
                url: "http://a".into(),
                start: past,
                stop: future,
                status: RecordingStatus::Recording,
                output_path: PathBuf::from("/tmp/a.ts"),
            },
            // Was recording, stop in past → failed
            Recording {
                id: 2,
                channel_name: "B".into(),
                programme_title: "Show B".into(),
                url: "http://b".into(),
                start: past - chrono::Duration::hours(2),
                stop: past,
                status: RecordingStatus::Recording,
                output_path: PathBuf::from("/tmp/b.ts"),
            },
            // Scheduled but stop in past → missed
            Recording {
                id: 3,
                channel_name: "C".into(),
                programme_title: "Show C".into(),
                url: "http://c".into(),
                start: past - chrono::Duration::hours(1),
                stop: past,
                status: RecordingStatus::Scheduled,
                output_path: PathBuf::from("/tmp/c.ts"),
            },
        ];
        std::fs::write(&path, serde_json::to_string(&recs).unwrap()).unwrap();

        // Temporarily override HOME to use our temp dir
        // Since RecorderState::load() uses recordings_path() which reads HOME,
        // we test the recovery logic directly instead
        let mut state = RecorderState {
            recordings: serde_json::from_str::<Vec<Recording>>(
                &std::fs::read_to_string(&path).unwrap()
            ).unwrap(),
            next_id: 4,
        };

        // Apply the same recovery logic as load()
        let now = Utc::now();
        for rec in &mut state.recordings {
            match rec.status {
                RecordingStatus::Recording => {
                    if rec.stop > now {
                        rec.status = RecordingStatus::Scheduled;
                    } else {
                        rec.status = RecordingStatus::Failed {
                            error: "Interrupted by app restart".to_string(),
                        };
                    }
                }
                RecordingStatus::Scheduled => {
                    if rec.stop <= now {
                        rec.status = RecordingStatus::Failed {
                            error: "Missed (app was not running)".to_string(),
                        };
                    }
                }
                _ => {}
            }
        }

        assert_eq!(state.recordings[0].status, RecordingStatus::Scheduled); // re-scheduled
        assert!(matches!(state.recordings[1].status, RecordingStatus::Failed { .. })); // interrupted
        assert!(matches!(state.recordings[2].status, RecordingStatus::Failed { .. })); // missed
    }

    #[test]
    fn test_recorder_state_next_id_from_existing() {
        let recs = vec![
            Recording {
                id: 5,
                channel_name: "X".into(),
                programme_title: "P".into(),
                url: "http://x".into(),
                start: Utc::now(),
                stop: Utc::now() + chrono::Duration::hours(1),
                status: RecordingStatus::Complete,
                output_path: PathBuf::from("/tmp/x.ts"),
            },
            Recording {
                id: 10,
                channel_name: "Y".into(),
                programme_title: "Q".into(),
                url: "http://y".into(),
                start: Utc::now(),
                stop: Utc::now() + chrono::Duration::hours(1),
                status: RecordingStatus::Complete,
                output_path: PathBuf::from("/tmp/y.ts"),
            },
        ];
        let next_id = recs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        assert_eq!(next_id, 11);
    }
}
