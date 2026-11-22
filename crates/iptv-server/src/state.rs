use crate::plex::{PlexClient, SyncReport};
use iptv_core::hdhr::HdhrState;
use serde::Serialize;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct LoadStatus {
    pub loading: bool,
    /// "cache" or "provider"
    pub source: Option<&'static str>,
    pub loaded_at: Option<u64>,
    pub epg_programmes: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct PlexStatus {
    pub configured: bool,
    pub auto_sync: bool,
    pub running: bool,
    /// A sync is queued and waiting for changes to settle.
    pub pending: bool,
    pub last_sync_at: Option<u64>,
    pub last_report: Option<SyncReport>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncRequest {
    /// Lineup changed; sync once changes settle.
    Debounced,
    /// User asked for it; sync right away.
    Now,
}

pub struct AppState {
    pub hdhr: Arc<Mutex<HdhrState>>,
    pub load: Mutex<LoadStatus>,
    pub plex: Mutex<PlexStatus>,
    pub refresh_tx: mpsc::Sender<()>,
    pub plex_tx: Option<UnboundedSender<SyncRequest>>,
    pub hdhr_port: u16,
}

impl AppState {
    /// Called whenever the lineup may have changed.
    pub fn lineup_changed(&self) {
        if self.plex.lock().unwrap().auto_sync {
            self.request_sync(SyncRequest::Debounced);
        }
    }

    pub fn request_sync(&self, req: SyncRequest) -> bool {
        match &self.plex_tx {
            Some(tx) => {
                self.plex.lock().unwrap().pending = true;
                tx.send(req).is_ok()
            }
            None => false,
        }
    }

    pub fn lineup_numbers(&self) -> Vec<u32> {
        self.hdhr
            .lock()
            .unwrap()
            .lineup()
            .iter()
            .map(|c| c.number)
            .collect()
    }
}

/// Background task: waits for sync requests, debounces bursts of favourite
/// changes into one Plex update, and records the outcome.
pub async fn plex_worker(
    state: Arc<AppState>,
    client: Arc<PlexClient>,
    device_id: String,
    delay: Duration,
    mut rx: UnboundedReceiver<SyncRequest>,
) {
    while let Some(req) = rx.recv().await {
        if req == SyncRequest::Debounced {
            // Keep waiting while changes keep arriving; an explicit "Now" ends the wait.
            loop {
                match tokio::time::timeout(delay, rx.recv()).await {
                    Ok(Some(SyncRequest::Debounced)) => continue,
                    Ok(Some(SyncRequest::Now)) | Err(_) => break,
                    Ok(None) => return,
                }
            }
        }
        // Drain anything queued meanwhile; this sync covers it.
        while rx.try_recv().is_ok() {}

        {
            let mut p = state.plex.lock().unwrap();
            p.pending = false;
            p.running = true;
        }
        let numbers = state.lineup_numbers();
        let client = Arc::clone(&client);
        let id = device_id.clone();
        let result = tokio::task::spawn_blocking(move || client.sync(&id, &numbers)).await;

        let mut p = state.plex.lock().unwrap();
        p.running = false;
        p.last_sync_at = Some(now_secs());
        match result {
            Ok(Ok(report)) => {
                eprintln!(
                    "[plex] Synced {} channel(s) to DVR {}",
                    report.channels_enabled, report.dvr_id
                );
                p.last_report = Some(report);
                p.last_error = None;
            }
            Ok(Err(e)) => {
                eprintln!("[plex] Sync failed: {e:#}");
                p.last_error = Some(format!("{e:#}"));
            }
            Err(e) => p.last_error = Some(format!("sync task panicked: {e}")),
        }
    }
}
