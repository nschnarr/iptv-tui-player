//! Background playlist + EPG loading. Runs on its own thread because the core
//! loaders are blocking and a slow provider can take minutes.

use crate::state::{now_secs, AppState};
use iptv_core::{cache, epg::Epg, provider::Provider};
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub fn spawn(
    state: Arc<AppState>,
    provider: Provider,
    epg_source: Option<String>,
    interval: Option<Duration>,
    refresh_rx: mpsc::Receiver<()>,
) {
    std::thread::Builder::new()
        .name("playlist-loader".into())
        .spawn(move || run(state, provider, epg_source, interval, refresh_rx))
        .expect("failed to spawn loader thread");
}

fn run(
    state: Arc<AppState>,
    provider: Provider,
    epg_source: Option<String>,
    interval: Option<Duration>,
    refresh_rx: mpsc::Receiver<()>,
) {
    // Serve the cached copy immediately so the tuner works while the
    // provider (which may be throttling us) is re-fetched.
    let mut fetch_now = true;
    if let Some(playlist) = provider.load_cached() {
        eprintln!("[loader] Loaded {} from cache", playlist.stats());
        apply_playlist(&state, playlist, "cache");
        if let Some(epg) = epg_source.as_deref().and_then(load_cached_epg) {
            apply_epg(&state, epg);
        }
        // Don't hammer the provider on every pod restart if the cache is recent.
        let max_age_h = interval
            .map(|d| d.as_secs_f64() / 3600.0)
            .unwrap_or(f64::MAX);
        fetch_now = cache::playlist_cache_stale(max_age_h);
        if !fetch_now {
            eprintln!("[loader] Cache is fresh; next provider fetch on schedule");
        }
    }

    loop {
        if fetch_now {
            refresh(&state, &provider, epg_source.as_deref());
        }
        fetch_now = true;
        let woke = match interval {
            Some(d) => match refresh_rx.recv_timeout(d) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => true,
                Err(mpsc::RecvTimeoutError::Disconnected) => false,
            },
            None => refresh_rx.recv().is_ok(),
        };
        if !woke {
            return;
        }
        while refresh_rx.try_recv().is_ok() {} // coalesce repeated clicks
    }
}

fn refresh(state: &AppState, provider: &Provider, epg_source: Option<&str>) {
    state.load.lock().unwrap().loading = true;
    let start = Instant::now();
    match provider.load() {
        Ok(playlist) => {
            eprintln!(
                "[loader] Loaded {} from provider in {:.1}s",
                playlist.stats(),
                start.elapsed().as_secs_f64()
            );
            apply_playlist(state, playlist, "provider");
            state.load.lock().unwrap().last_error = None;
        }
        Err(e) => {
            eprintln!("[loader] Playlist fetch failed: {e:#}");
            state.load.lock().unwrap().last_error = Some(format!("Playlist: {e:#}"));
        }
    }
    if let Some(src) = epg_source {
        match fetch_epg(src) {
            Ok(epg) => apply_epg(state, epg),
            Err(e) => {
                eprintln!("[loader] EPG fetch failed: {e:#}");
                state.load.lock().unwrap().last_error = Some(format!("EPG: {e:#}"));
            }
        }
    }
    state.load.lock().unwrap().loading = false;
}

fn apply_playlist(state: &AppState, playlist: iptv_core::model::Playlist, source: &'static str) {
    let before = state.lineup_numbers();
    state.hdhr.lock().unwrap().set_playlist(Arc::new(playlist));
    {
        let mut s = state.load.lock().unwrap();
        s.source = Some(source);
        s.loaded_at = Some(now_secs());
    }
    if state.lineup_numbers() != before {
        state.lineup_changed();
    }
}

fn apply_epg(state: &AppState, epg: Epg) {
    eprintln!(
        "[loader] EPG: {} programmes for {} channels",
        epg.programme_count(),
        epg.channel_count()
    );
    state.load.lock().unwrap().epg_programmes = epg.programme_count();
    state.hdhr.lock().unwrap().epg = Some(Arc::new(epg));
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn load_cached_epg(src: &str) -> Option<Epg> {
    if is_url(src) && !cache::epg_source_changed(src) {
        Epg::load_cached()
    } else {
        None
    }
}

fn fetch_epg(src: &str) -> anyhow::Result<Epg> {
    if is_url(src) {
        Epg::parse_xmltv_url(src)
    } else {
        Epg::parse_xmltv_file(Path::new(src))
    }
}
