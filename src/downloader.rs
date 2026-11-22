use crate::model::ContentType;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub name: String,
    pub url: String,
    pub content_type: ContentType,
    #[allow(dead_code)]
    pub group: String,
}

#[derive(Debug, Clone)]
pub enum DownloadStatus {
    Queued,
    Downloading {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
        bytes_per_sec: Option<u64>,
    },
    Complete {
        path: PathBuf,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub item: DownloadItem,
    pub status: DownloadStatus,
}

/// Shared state between the UI thread and the download worker.
pub struct DownloadManager {
    /// Send new items to the worker thread.
    item_tx: mpsc::Sender<DownloadItem>,
    /// Shared queue state (readable from UI, writable from worker).
    pub queue: Arc<Mutex<Vec<QueueEntry>>>,
    /// Base directory for downloads.
    #[allow(dead_code)]
    pub download_dir: PathBuf,
}

impl DownloadManager {
    pub fn new(download_dir: PathBuf) -> Self {
        let queue: Arc<Mutex<Vec<QueueEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let (item_tx, item_rx) = mpsc::channel::<DownloadItem>();

        let worker_queue = Arc::clone(&queue);
        let worker_dir = download_dir.clone();

        thread::Builder::new()
            .name("downloader".into())
            .spawn(move || {
                worker_loop(item_rx, worker_queue, worker_dir);
            })
            .expect("failed to spawn download worker");

        Self {
            item_tx,
            queue,
            download_dir,
        }
    }

    /// Add an item to the download queue.
    pub fn enqueue(&self, item: DownloadItem) {
        {
            let mut q = self.queue.lock().unwrap();
            // Don't add duplicates (same URL)
            if q.iter().any(|e| e.item.url == item.url) {
                return;
            }
            q.push(QueueEntry {
                item: item.clone(),
                status: DownloadStatus::Queued,
            });
        }
        let _ = self.item_tx.send(item);
    }

    /// Remove a completed or failed entry from the queue by index.
    pub fn remove(&self, index: usize) {
        let mut q = self.queue.lock().unwrap();
        if index < q.len() {
            match q[index].status {
                DownloadStatus::Complete { .. } | DownloadStatus::Failed { .. } => {
                    q.remove(index);
                }
                _ => {} // Can't remove active/queued items
            }
        }
    }

    /// Get a snapshot of the current queue for rendering.
    pub fn snapshot(&self) -> Vec<QueueEntry> {
        self.queue.lock().unwrap().clone()
    }

    /// Count of active + queued items.
    pub fn pending_count(&self) -> usize {
        let q = self.queue.lock().unwrap();
        q.iter()
            .filter(|e| {
                matches!(
                    e.status,
                    DownloadStatus::Queued | DownloadStatus::Downloading { .. }
                )
            })
            .count()
    }
}

fn worker_loop(
    rx: mpsc::Receiver<DownloadItem>,
    queue: Arc<Mutex<Vec<QueueEntry>>>,
    download_dir: PathBuf,
) {
    while let Ok(item) = rx.recv() {
        // Mark as downloading
        update_status(
            &queue,
            &item.url,
            DownloadStatus::Downloading {
                bytes_downloaded: 0,
                total_bytes: None,
                bytes_per_sec: None,
            },
        );

        match download_file(&item, &download_dir, &queue) {
            Ok(path) => {
                update_status(&queue, &item.url, DownloadStatus::Complete { path });
            }
            Err(e) => {
                update_status(
                    &queue,
                    &item.url,
                    DownloadStatus::Failed {
                        error: e.to_string(),
                    },
                );
            }
        }
    }
}

fn update_status(queue: &Arc<Mutex<Vec<QueueEntry>>>, url: &str, status: DownloadStatus) {
    let mut q = queue.lock().unwrap();
    if let Some(entry) = q.iter_mut().find(|e| e.item.url == url) {
        entry.status = status;
    }
}

fn download_file(
    item: &DownloadItem,
    download_dir: &Path,
    queue: &Arc<Mutex<Vec<QueueEntry>>>,
) -> anyhow::Result<PathBuf> {
    // Build output path based on content type
    let output_path = build_output_path(item, download_dir);

    // Create parent directories
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Determine file extension from URL if not in the built path
    let response = ureq::get(&item.url).call()?;

    let total_bytes: Option<u64> = response
        .header("Content-Length")
        .and_then(|s| s.parse().ok());

    // Update with total size
    update_status(
        queue,
        &item.url,
        DownloadStatus::Downloading {
            bytes_downloaded: 0,
            total_bytes,
            bytes_per_sec: None,
        },
    );

    let mut reader = response.into_reader();
    let mut file = fs::File::create(&output_path)?;

    let mut buf = [0u8; 65536]; // 64KB buffer
    let mut downloaded: u64 = 0;
    let mut last_update: u64 = 0;
    let mut speed_bytes: u64 = 0;
    let mut speed_timer = Instant::now();
    let mut last_speed: Option<u64> = None;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        speed_bytes += n as u64;

        // Update progress every 512KB to avoid lock contention
        if downloaded - last_update >= 524_288 {
            let elapsed = speed_timer.elapsed().as_secs_f64();
            if elapsed > 0.1 {
                last_speed = Some((speed_bytes as f64 / elapsed) as u64);
                speed_bytes = 0;
                speed_timer = Instant::now();
            }
            update_status(
                queue,
                &item.url,
                DownloadStatus::Downloading {
                    bytes_downloaded: downloaded,
                    total_bytes,
                    bytes_per_sec: last_speed,
                },
            );
            last_update = downloaded;
        }
    }

    file.flush()?;
    Ok(output_path)
}

fn build_output_path(item: &DownloadItem, download_dir: &Path) -> PathBuf {
    let ext = url_extension(&item.url).unwrap_or_else(|| "mp4".to_string());

    match &item.content_type {
        ContentType::Series {
            series_name,
            season,
            episode,
        } => {
            let safe_series = sanitize_filename(series_name);
            let safe_name = sanitize_filename(&item.name);
            download_dir.join(&safe_series).join(format!(
                "S{:02}E{:02} - {}.{}",
                season, episode, safe_name, ext
            ))
        }
        ContentType::Movie => {
            let safe_name = sanitize_filename(&item.name);
            download_dir
                .join("Movies")
                .join(format!("{}.{}", safe_name, ext))
        }
        ContentType::Live => {
            // Shouldn't normally download live streams, but handle gracefully
            let safe_name = sanitize_filename(&item.name);
            download_dir.join(format!("{}.{}", safe_name, ext))
        }
    }
}

fn url_extension(url: &str) -> Option<String> {
    // Strip query params
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next()?;
    let ext = filename.rsplit('.').next()?;
    let ext = ext.to_lowercase();
    // Only return known media extensions
    match ext.as_str() {
        "mp4" | "mkv" | "avi" | "ts" | "m3u8" | "flv" | "mov" | "wmv" | "mpg" | "mpeg" => Some(ext),
        _ => None,
    }
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

    // --- url_extension ---

    #[test]
    fn test_url_extension_mp4() {
        assert_eq!(
            url_extension("http://example.com/video.mp4"),
            Some("mp4".to_string())
        );
    }

    #[test]
    fn test_url_extension_with_query_params() {
        assert_eq!(
            url_extension("http://example.com/video.mkv?token=abc"),
            Some("mkv".to_string())
        );
    }

    #[test]
    fn test_url_extension_ts() {
        assert_eq!(
            url_extension("http://example.com/stream.ts"),
            Some("ts".to_string())
        );
    }

    #[test]
    fn test_url_extension_m3u8() {
        assert_eq!(
            url_extension("http://example.com/live.m3u8"),
            Some("m3u8".to_string())
        );
    }

    #[test]
    fn test_url_extension_unknown() {
        assert_eq!(url_extension("http://example.com/file.xyz"), None);
    }

    #[test]
    fn test_url_extension_no_extension() {
        assert_eq!(url_extension("http://example.com/stream"), None);
    }

    #[test]
    fn test_url_extension_case_insensitive() {
        assert_eq!(
            url_extension("http://example.com/VIDEO.MP4"),
            Some("mp4".to_string())
        );
    }

    // --- sanitize_filename ---

    #[test]
    fn test_sanitize_normal() {
        assert_eq!(sanitize_filename("My Movie 2024"), "My Movie 2024");
    }

    #[test]
    fn test_sanitize_path_separators() {
        assert_eq!(sanitize_filename("path/to\\file"), "path_to_file");
    }

    #[test]
    fn test_sanitize_windows_forbidden() {
        assert_eq!(
            sanitize_filename("file:name*with?chars"),
            "file_name_with_chars"
        );
    }

    // --- build_output_path ---

    #[test]
    fn test_output_path_live() {
        let item = DownloadItem {
            name: "CNN Live".to_string(),
            url: "http://example.com/cnn.ts".to_string(),
            content_type: ContentType::Live,
            group: "News".to_string(),
        };
        let path = build_output_path(&item, Path::new("/downloads"));
        assert_eq!(path, PathBuf::from("/downloads/CNN Live.ts"));
    }

    #[test]
    fn test_output_path_movie() {
        let item = DownloadItem {
            name: "The Matrix".to_string(),
            url: "http://example.com/matrix.mkv".to_string(),
            content_type: ContentType::Movie,
            group: "Movies".to_string(),
        };
        let path = build_output_path(&item, Path::new("/downloads"));
        assert_eq!(path, PathBuf::from("/downloads/Movies/The Matrix.mkv"));
    }

    #[test]
    fn test_output_path_series() {
        let item = DownloadItem {
            name: "Pilot".to_string(),
            url: "http://example.com/pilot.mp4".to_string(),
            content_type: ContentType::Series {
                series_name: Arc::from("Breaking Bad"),
                season: 1,
                episode: 1,
            },
            group: "Series".to_string(),
        };
        let path = build_output_path(&item, Path::new("/downloads"));
        assert_eq!(
            path,
            PathBuf::from("/downloads/Breaking Bad/S01E01 - Pilot.mp4")
        );
    }

    #[test]
    fn test_output_path_series_high_numbers() {
        let item = DownloadItem {
            name: "Episode 99".to_string(),
            url: "http://example.com/ep.mp4".to_string(),
            content_type: ContentType::Series {
                series_name: Arc::from("Long Show"),
                season: 12,
                episode: 99,
            },
            group: "Series".to_string(),
        };
        let path = build_output_path(&item, Path::new("/dl"));
        assert_eq!(path, PathBuf::from("/dl/Long Show/S12E99 - Episode 99.mp4"));
    }

    #[test]
    fn test_output_path_no_extension_defaults_mp4() {
        let item = DownloadItem {
            name: "Stream".to_string(),
            url: "http://example.com/stream".to_string(),
            content_type: ContentType::Movie,
            group: "Movies".to_string(),
        };
        let path = build_output_path(&item, Path::new("/dl"));
        assert_eq!(path, PathBuf::from("/dl/Movies/Stream.mp4"));
    }

    #[test]
    fn test_output_path_sanitizes_names() {
        let item = DownloadItem {
            name: "Bad: Name*Here".to_string(),
            url: "http://example.com/file.mp4".to_string(),
            content_type: ContentType::Series {
                series_name: Arc::from("Show/With\\Slashes"),
                season: 1,
                episode: 1,
            },
            group: "Series".to_string(),
        };
        let path = build_output_path(&item, Path::new("/dl"));
        assert_eq!(
            path,
            PathBuf::from("/dl/Show_With_Slashes/S01E01 - Bad_ Name_Here.mp4")
        );
    }

    // --- DownloadManager queue ---

    #[test]
    fn test_download_manager_enqueue_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DownloadManager::new(dir.path().to_path_buf());

        let item = DownloadItem {
            name: "Test".to_string(),
            url: "http://example.com/test.mp4".to_string(),
            content_type: ContentType::Movie,
            group: "Movies".to_string(),
        };

        mgr.enqueue(item.clone());
        mgr.enqueue(item.clone()); // duplicate — should be ignored

        let snapshot = mgr.snapshot();
        assert_eq!(snapshot.len(), 1);
    }

    #[test]
    fn test_download_manager_pending_count() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DownloadManager::new(dir.path().to_path_buf());

        assert_eq!(mgr.pending_count(), 0);

        mgr.enqueue(DownloadItem {
            name: "A".to_string(),
            url: "http://a.mp4".to_string(),
            content_type: ContentType::Live,
            group: "G".to_string(),
        });

        // Item should be queued (pending)
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn test_download_manager_snapshot_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        assert!(mgr.snapshot().is_empty());
    }
}
