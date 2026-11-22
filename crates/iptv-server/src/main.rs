#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod config;
mod loader;
mod plex;
mod state;
mod web;

use config::Config;
use iptv_core::hdhr::HdhrServer;
use iptv_core::model::Playlist;
use state::{AppState, LoadStatus, PlexStatus};
use std::sync::{mpsc, Arc, Mutex};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        print!("{}", config::HELP);
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("discovery") {
        return run_discovery();
    }
    let cfg = Config::from_env()?;
    if !iptv_core::hdhr_discovery::parse_device_id(&cfg.device.device_id)
        .is_some_and(iptv_core::hdhr_discovery::device_id_is_valid)
    {
        eprintln!(
            "Warning: IPTV_DEVICE_ID {} is not a valid HDHomeRun ID (8 hex digits with checksum); \
             UDP discovery clients may ignore it",
            cfg.device.device_id
        );
    }
    eprintln!("Data dir: {}", iptv_core::paths::data_dir().display());

    // The HDHR server starts with an empty playlist; the loader fills it in.
    let favorites = iptv_core::favorites::load_favorites();
    eprintln!("Loaded {} favourite(s)", favorites.len());
    let server = HdhrServer::new(
        cfg.hdhr_port,
        cfg.bind.clone(),
        Arc::new(Playlist::default()),
        favorites,
        None,
        cfg.buffer_mode.clone(),
        cfg.buffer_secs,
    )
    .with_device(cfg.device.clone());
    let hdhr = server.state();
    let hdhr_port = server.start()?;
    eprintln!(
        "HDHomeRun tuner '{}' (ID {}) on {}:{}; add http://<this-host>:{} in Plex, \
         guide: http://<this-host>:{}/xmltv.xml",
        cfg.device.friendly_name, cfg.device.device_id, cfg.bind, hdhr_port, hdhr_port, hdhr_port
    );

    let (refresh_tx, refresh_rx) = mpsc::channel();
    let (plex_tx, plex_rx) = match cfg.plex {
        Some(_) => {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            (Some(tx), Some(rx))
        }
        None => (None, None),
    };
    let state = Arc::new(AppState {
        hdhr,
        load: Mutex::new(LoadStatus {
            loading: true,
            ..Default::default()
        }),
        plex: Mutex::new(PlexStatus {
            configured: cfg.plex.is_some(),
            auto_sync: cfg.plex.as_ref().is_some_and(|p| p.auto_sync),
            ..Default::default()
        }),
        refresh_tx,
        plex_tx,
        hdhr_port,
    });

    if let (Some(plex_cfg), Some(rx)) = (cfg.plex.clone(), plex_rx) {
        eprintln!(
            "Plex sync enabled for {} (auto-sync: {})",
            plex_cfg.url, plex_cfg.auto_sync
        );
        let delay = plex_cfg.sync_delay;
        let client = Arc::new(plex::PlexClient::new(plex_cfg));
        tokio::spawn(state::plex_worker(
            Arc::clone(&state),
            client,
            cfg.device.device_id.clone(),
            delay,
            rx,
        ));
    }

    loader::spawn(
        Arc::clone(&state),
        cfg.provider,
        cfg.epg_source,
        cfg.refresh_interval,
        refresh_rx,
    );

    let listener = tokio::net::TcpListener::bind((cfg.bind.as_str(), cfg.ui_port)).await?;
    eprintln!("Web UI on http://{}:{}", cfg.bind, cfg.ui_port);
    axum::serve(listener, web::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// `iptv-server discovery`: answer HDHomeRun UDP broadcasts (port 65001) with
/// a pointer to a tuner running elsewhere. Needs host networking in k8s, since
/// broadcasts don't reach pod networks.
fn run_discovery() -> anyhow::Result<()> {
    use anyhow::Context;
    use iptv_core::hdhr_discovery::{self, Advertisement};
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());

    let base_url = var("IPTV_TUNER_URL")
        .context("IPTV_TUNER_URL is required for discovery, e.g. http://192.168.1.50:5004")?
        .trim_end_matches('/')
        .to_string();
    let id_str = var("IPTV_DEVICE_ID").unwrap_or_else(|| "1F7E5E07".to_string());
    let device_id = hdhr_discovery::parse_device_id(&id_str)
        .with_context(|| format!("IPTV_DEVICE_ID must be 8 hex digits, got {id_str:?}"))?;
    if !hdhr_discovery::device_id_is_valid(device_id) {
        eprintln!(
            "Warning: IPTV_DEVICE_ID {id_str} fails the HDHomeRun checksum; Plex may ignore it"
        );
    }
    let tuner_count: u8 = match var("IPTV_TUNER_COUNT") {
        Some(v) => v.parse().context("IPTV_TUNER_COUNT must be a number")?,
        None => 2,
    };
    let bind = var("IPTV_DISCOVERY_BIND").unwrap_or_else(|| "0.0.0.0".to_string());

    let ad = Advertisement {
        device_id,
        tuner_count,
        base_url,
    };
    eprintln!(
        "HDHomeRun discovery on {bind}:{}: advertising {:08X} at {}",
        hdhr_discovery::DISCOVERY_PORT,
        ad.device_id,
        ad.base_url
    );
    hdhr_discovery::serve(&bind, &ad).context("discovery responder failed")?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
    eprintln!("Shutting down");
}
