#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod app;
mod downloader;
mod epg;
mod hdhr;
mod epg_search;
mod error;
mod favorites;
mod log;
mod model;
mod cache;
mod parser;
mod player;
mod provider;
mod recorder;
mod search;
mod series;
mod timezone;
mod theme;
mod ui;
mod xtream;

use app::{App, AppMode, GuideViewMode, SeriesRow};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use epg::Epg;
use provider::Provider;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use xtream::XtreamProvider;

/// Read current process RSS from /proc/self/status (Linux only). Returns MB.
fn get_rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
        })
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

/// Log Arc strong counts and memory info for debugging.
fn log_memory_diagnostics(app: &App) {
    let rss = get_rss_mb();
    let playlist_refs = Arc::strong_count(&app.playlist);
    let epg_refs = app.epg.as_ref().map(|e| Arc::strong_count(e)).unwrap_or(0);
    let playlist_channels = app.playlist.channels.len();
    let epg_programmes = app.epg.as_ref().map(|e| e.programme_count()).unwrap_or(0);

    app.log.info("memory", format!(
        "RSS: {:.0} MB | Playlist Arc refs: {} ({} channels) | EPG Arc refs: {} ({} programmes) | Favorites: {} | Downloads: {}",
        rss,
        playlist_refs,
        playlist_channels,
        epg_refs,
        epg_programmes,
        app.favorites.len(),
        app.download_manager.pending_count(),
    ));
}

struct CliArgs {
    provider: Provider,
    epg_source: Option<String>,
    hdhr_port: Option<u16>,
    hdhr_bind: String,
    buffer_mode: hdhr::BufferMode,
    buffer_secs: f64,
    refresh_hours: f64,
    player: player::Player,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cli = parse_args(&args)?;

    // Try loading from cache first for instant startup
    let mut used_cache = false;
    let start = Instant::now();
    let playlist = if let Some(cached) = cli.provider.load_cached() {
        used_cache = true;
        eprintln!(
            "Loaded {} from cache in {:.1}s (will refresh in background)",
            cached.stats(),
            start.elapsed().as_secs_f64()
        );
        cached
    } else {
        let playlist = cli.provider.load()?;
        eprintln!(
            "Loaded {} in {:.1}s",
            playlist.stats(),
            start.elapsed().as_secs_f64()
        );
        playlist
    };

    // Load EPG: try cache first, then fetch
    let epg = if let Some(ref source) = cli.epg_source {
        let epg_start = Instant::now();
        let is_url = source.starts_with("http://") || source.starts_with("https://");

        // Try cache first for URL sources (skip if URL changed)
        if is_url && !cache::epg_source_changed(source) {
            if let Some(cached_epg) = Epg::load_cached() {
                if !used_cache {
                    // Playlist was fresh but EPG was cached — that's fine
                }
                used_cache = true;
                eprintln!(
                    "Loaded EPG from cache: {} programmes for {} channels in {:.1}s",
                    cached_epg.programme_count(),
                    cached_epg.channel_count(),
                    epg_start.elapsed().as_secs_f64()
                );
                Some(cached_epg)
            } else {
                let epg = Epg::parse_xmltv_url(source)?;
                eprintln!(
                    "Loaded EPG: {} programmes for {} channels in {:.1}s",
                    epg.programme_count(),
                    epg.channel_count(),
                    epg_start.elapsed().as_secs_f64()
                );
                Some(epg)
            }
        } else if is_url {
            let epg = Epg::parse_xmltv_url(source)?;
            eprintln!(
                "Loaded EPG: {} programmes for {} channels in {:.1}s",
                epg.programme_count(),
                epg.channel_count(),
                epg_start.elapsed().as_secs_f64()
            );
            Some(epg)
        } else {
            let epg = Epg::parse_xmltv_file(std::path::Path::new(source))?;
            eprintln!(
                "Loaded EPG: {} programmes for {} channels in {:.1}s",
                epg.programme_count(),
                epg.channel_count(),
                epg_start.elapsed().as_secs_f64()
            );
            Some(epg)
        }
    } else {
        None
    };

    run_tui(playlist, epg, cli.hdhr_port, cli.hdhr_bind, cli.buffer_mode, cli.buffer_secs, cli.refresh_hours, cli.player, cli.provider, cli.epg_source, used_cache)?;
    Ok(())
}

fn parse_args(args: &[String]) -> anyhow::Result<CliArgs> {
    let mut playlist_path: Option<PathBuf> = None;
    let mut playlist_url: Option<String> = None;
    let mut xtream_server: Option<String> = None;
    let mut xtream_user: Option<String> = None;
    let mut xtream_pass: Option<String> = None;
    let mut epg_source: Option<String> = None;
    let mut hdhr_port: Option<u16> = None;
    let mut hdhr_bind = "127.0.0.1".to_string();
    let mut buffer_mode = hdhr::BufferMode::None;
    let mut buffer_secs: f64 = 0.0;
    let mut refresh_hours: f64 = 0.0;
    let mut player = player::Player::Vlc;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--playlist" | "-p" => {
                i += 1;
                if i < args.len() {
                    playlist_path = Some(PathBuf::from(&args[i]));
                } else {
                    anyhow::bail!("--playlist requires a path argument");
                }
            }
            "--url" | "-u" => {
                i += 1;
                if i < args.len() {
                    playlist_url = Some(args[i].clone());
                } else {
                    anyhow::bail!("--url requires a URL argument");
                }
            }
            "--xtream-server" => {
                i += 1;
                if i < args.len() {
                    xtream_server = Some(args[i].clone());
                } else {
                    anyhow::bail!("--xtream-server requires a URL argument");
                }
            }
            "--xtream-user" => {
                i += 1;
                if i < args.len() {
                    xtream_user = Some(args[i].clone());
                } else {
                    anyhow::bail!("--xtream-user requires a username argument");
                }
            }
            "--xtream-pass" => {
                i += 1;
                if i < args.len() {
                    xtream_pass = Some(args[i].clone());
                } else {
                    anyhow::bail!("--xtream-pass requires a password argument");
                }
            }
            "--epg" | "-e" => {
                i += 1;
                if i < args.len() {
                    epg_source = Some(args[i].clone());
                } else {
                    anyhow::bail!("--epg requires a path or URL argument");
                }
            }
            "--hdhr-port" => {
                i += 1;
                if i < args.len() {
                    hdhr_port = Some(args[i].parse().map_err(|_| {
                        anyhow::anyhow!("--hdhr-port requires a valid port number")
                    })?);
                } else {
                    anyhow::bail!("--hdhr-port requires a port number");
                }
            }
            "--hdhr-bind" => {
                i += 1;
                if i < args.len() {
                    hdhr_bind = args[i].clone();
                } else {
                    anyhow::bail!("--hdhr-bind requires an address (e.g. 127.0.0.1 or 0.0.0.0)");
                }
            }
            "--buffer" => {
                i += 1;
                if i < args.len() {
                    buffer_mode = hdhr::BufferMode::from_str(&args[i])?;
                } else {
                    anyhow::bail!("--buffer requires a mode: none or ffmpeg");
                }
            }
            "--hdhr-buffer-secs" => {
                i += 1;
                if i < args.len() {
                    buffer_secs = args[i].parse().map_err(|_| {
                        anyhow::anyhow!("--hdhr-buffer-secs requires a number (e.g. 5)")
                    })?;
                } else {
                    anyhow::bail!("--hdhr-buffer-secs requires a number (e.g. 5)");
                }
            }
            "--refresh-hours" => {
                i += 1;
                if i < args.len() {
                    refresh_hours = args[i].parse().map_err(|_| {
                        anyhow::anyhow!("--refresh-hours requires a number (e.g. 24)")
                    })?;
                } else {
                    anyhow::bail!("--refresh-hours requires a number (e.g. 24)");
                }
            }
            "--player" => {
                i += 1;
                if i < args.len() {
                    player = player::Player::from_str(&args[i])?;
                } else {
                    anyhow::bail!("--player requires a player: vlc or mpv");
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                // Positional argument: treat as playlist path
                playlist_path = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }

    // If Xtream flags are present, use Xtream provider
    if xtream_server.is_some() || xtream_user.is_some() || xtream_pass.is_some() {
        let server =
            xtream_server.ok_or_else(|| anyhow::anyhow!("--xtream-server is required"))?;
        let username =
            xtream_user.ok_or_else(|| anyhow::anyhow!("--xtream-user is required"))?;
        let password =
            xtream_pass.ok_or_else(|| anyhow::anyhow!("--xtream-pass is required"))?;
        return Ok(CliArgs { epg_source, hdhr_port, hdhr_bind, buffer_mode, buffer_secs, refresh_hours, player, provider: Provider::Xtream(XtreamProvider {
            server,
            username,
            password,
        })});
    }

    // Fall back to M3U
    if playlist_path.is_some() || playlist_url.is_some() {
        return Ok(CliArgs {
            epg_source,
            hdhr_port,
            hdhr_bind,
            buffer_mode,
            buffer_secs,
            refresh_hours,
            player,
            provider: Provider::M3u {
                path: playlist_path,
                url: playlist_url,
            },
        });
    }

    anyhow::bail!(
        "No source specified. Use --playlist <path>, --url <url>, or --xtream-server with --xtream-user and --xtream-pass."
    )
}

fn print_help() {
    println!("Usage: iptv [OPTIONS]");
    println!();
    println!("M3U source:");
    println!("  -p, --playlist <path>        Path to M3U playlist file");
    println!("  -u, --url <url>              URL to M3U playlist");
    println!();
    println!("Xtream Codes source:");
    println!("  --xtream-server <url>        Xtream server URL (e.g. http://provider.com:8080)");
    println!("  --xtream-user <username>     Xtream username");
    println!("  --xtream-pass <password>     Xtream password");
    println!();
    println!("EPG:");
    println!("  -e, --epg <path_or_url>      XMLTV EPG file path or URL");
    println!();
    println!("HDHR Tuner:");
    println!("  --hdhr-port <port>           Start HDHomeRun tuner server on port (serves favorites)");
    println!("  --hdhr-bind <addr>           Bind address for HDHR server (default: 127.0.0.1)");
    println!("                               Use 0.0.0.0 to allow LAN access (e.g. for Plex on another machine)");
    println!("  --buffer <none|ffmpeg>       Stream buffer mode (default: none)");
    println!("                               ffmpeg: remux + buffer via FFmpeg (recommended)");
    println!("                               none: raw HTTP pipe (no buffering)");
    println!("  --hdhr-buffer-secs <secs>    Pre-roll buffer seconds (default: 0)");
    println!("                               Accumulates N seconds of data before playback");
    println!("                               starts. Higher = slower channel change, fewer pauses");
    println!();
    println!("Auto-refresh:");
    println!("  --refresh-hours <hours>      Re-fetch playlist every N hours (default: 0 = off)");
    println!("                               Keeps stream URLs fresh without restarting");
    println!();
    println!("Playback:");
    println!("  --player <vlc|mpv>           Default media player (default: vlc)");
    println!("                               Can still be switched at runtime with P");
    println!();
    println!("Other:");
    println!("  -h, --help                   Show this help");
}

#[allow(clippy::too_many_arguments)]
fn run_tui(playlist: model::Playlist, epg: Option<Epg>, hdhr_port: Option<u16>, hdhr_bind: String, buffer_mode: hdhr::BufferMode, buffer_secs: f64, refresh_hours: f64, player: player::Player, provider: Provider, epg_source: Option<String>, used_cache: bool) -> anyhow::Result<()> {
    // Start HDHR server before entering raw mode so we can print the port
    let hdhr_state = if let Some(port) = hdhr_port {
        let server = hdhr::HdhrServer::new(port, hdhr_bind.clone(), Arc::new(playlist.clone()), crate::favorites::load_favorites(), epg.as_ref().map(|e| Arc::new(e.clone())), buffer_mode.clone(), buffer_secs);
        let hdhr_state = server.state();
        let actual_port = server.start()?;
        let mode_str = match &buffer_mode {
            hdhr::BufferMode::FFmpeg => "ffmpeg (remux + buffer)",
            hdhr::BufferMode::None => "none (raw pipe)",
        };
        eprintln!("HDHR tuner server running on http://{}:{}", hdhr_bind, actual_port);
        eprintln!("  Plex tuner URL:  http://<your-ip>:{}", actual_port);
        eprintln!("  XMLTV guide URL: http://<your-ip>:{}/xmltv.xml", actual_port);
        eprintln!("  Buffer mode:     {}", mode_str);
        if buffer_secs > 0.0 {
            eprintln!("  Pre-roll buffer: {:.1}s", buffer_secs);
        }
        Some(hdhr_state)
    } else {
        None
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let has_epg = epg.is_some();
    let mut app = App::new(playlist, epg);
    app.hdhr_state = hdhr_state;
    app.sync_hdhr_favorites(); // Ensure HDHR has migrated favorites
    if has_epg {
        app.epg_last_refresh = Some(chrono::Utc::now());
    }
    app.epg_source = epg_source;
    app.selected_player = player;

    // Log startup info
    app.log.info("startup", format!("Loaded {} channels across {} groups", app.playlist.channels.len(), app.playlist.groups.len()));
    if has_epg {
        app.log.info("startup", "EPG data loaded");
    }
    if app.hdhr_state.is_some() {
        app.log.info("startup", "HDHomeRun tuner server active");
    }
    if used_cache {
        app.log.info("startup", "Loaded from cache (will refresh in background)");
    }
    app.log.info("startup", format!("Player: {}", app.selected_player));

    // Start background playlist+EPG refresh thread.
    // Handles both timed auto-refresh and manual trigger.
    // Fetches playlist first, then EPG sequentially so both stay in sync.
    {
        let (refresh_tx, refresh_rx) = std::sync::mpsc::channel();
        let (trigger_tx, trigger_rx) = std::sync::mpsc::channel::<()>();
        let interval_ms = if refresh_hours > 0.0 {
            (refresh_hours * 3_600_000.0) as u64
        } else {
            0
        };
        let epg_url_for_refresh = app.epg_source.clone();

        std::thread::Builder::new()
            .name("playlist-refresh".into())
            .spawn(move || {
                loop {
                    // Wait for either the interval or a manual trigger
                    if interval_ms > 0 {
                        // Use trigger_rx with timeout for timed refresh
                        match trigger_rx.recv_timeout(std::time::Duration::from_millis(interval_ms)) {
                            Ok(()) => {} // Manual trigger
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {} // Auto refresh
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        // No auto-refresh, just wait for manual triggers
                        if trigger_rx.recv().is_err() {
                            break;
                        }
                    }
                    eprintln!("[refresh] Starting playlist refresh...");
                    let refresh_start = std::time::Instant::now();
                    match provider.load() {
                        Ok(new_playlist) => {
                            eprintln!("[refresh] Playlist loaded: {} channels ({:.1}s)",
                                new_playlist.channels.len(),
                                refresh_start.elapsed().as_secs_f64());
                            // Fetch EPG sequentially after playlist
                            let fresh_epg = epg_url_for_refresh.as_deref().and_then(|src| {
                                if src.starts_with("http://") || src.starts_with("https://") {
                                    Epg::parse_xmltv_url(src).ok()
                                } else {
                                    Epg::parse_xmltv_file(std::path::Path::new(src)).ok()
                                }
                            });
                            eprintln!("[refresh] Total refresh took {:.1}s (EPG: {})",
                                refresh_start.elapsed().as_secs_f64(),
                                if fresh_epg.is_some() { "loaded" } else { "none" });
                            if refresh_tx.send((new_playlist, fresh_epg)).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("Playlist refresh failed: {}", e);
                        }
                    }
                }
            })?;
        app.refresh_rx = Some(refresh_rx);
        app.refresh_trigger = Some(trigger_tx.clone());
        if refresh_hours > 0.0 {
            eprintln!("Playlist auto-refresh: every {:.1}h", refresh_hours);
        }
        // If we loaded from cache, only background refresh if cache is stale (>24h)
        if used_cache {
            let playlist_stale = cache::playlist_cache_stale(24.0);
            let epg_stale = cache::epg_cache_stale(24.0);

            if playlist_stale {
                app.refresh_in_progress = true;
                let _ = trigger_tx.send(());
                app.status_message = Some("Refreshing playlist in background...".to_string());
            }

            if epg_stale {
                if let Some(ref source) = app.epg_source {
                    if source.starts_with("http://") || source.starts_with("https://") {
                        let source = source.clone();
                        let (epg_tx, epg_rx) = std::sync::mpsc::channel();
                        std::thread::Builder::new()
                            .name("epg-bg-refresh".into())
                            .spawn(move || {
                                if let Ok(epg) = Epg::parse_xmltv_url(&source) {
                                    let _ = epg_tx.send(epg);
                                }
                            })?;
                        app.epg_refresh_rx = Some(epg_rx);
                        if !playlist_stale {
                            app.status_message = Some("Refreshing EPG in background...".to_string());
                        } else {
                            app.status_message = Some("Refreshing playlist + EPG in background...".to_string());
                        }
                    }
                }
            }
        }
    }

    let result = run_event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    let mut last_memory_log = Instant::now();
    let memory_log_interval = Duration::from_secs(300); // every 5 minutes

    // Log initial memory state
    log_memory_diagnostics(&app);

    loop {
        app.poll_search_results();

        // Periodic memory diagnostics
        if last_memory_log.elapsed() >= memory_log_interval {
            log_memory_diagnostics(&app);
            last_memory_log = Instant::now();
        }

        // Check for playlist+EPG refresh
        if let Some(ref rx) = app.refresh_rx {
            if let Ok((new_playlist, fresh_epg)) = rx.try_recv() {
                app.log.info("refresh", format!("Refresh received — playlist: {} channels, EPG: {}",
                    new_playlist.channels.len(),
                    if fresh_epg.is_some() { "yes" } else { "no" }
                ));
                log_memory_diagnostics(&app);
                app.log.info("refresh-mem", "--- BEGIN refresh_playlist ---");
                app.refresh_playlist(new_playlist);
                app.log.info("refresh-mem", "--- END refresh_playlist ---");
                log_memory_diagnostics(&app);
                if let Some(new_epg) = fresh_epg {
                    let rss_pre_epg = get_rss_mb();
                    let old_epg_refs = app.epg.as_ref().map(|e| Arc::strong_count(e)).unwrap_or(0);
                    let old_epg_progs = app.epg.as_ref().map(|e| e.programme_count()).unwrap_or(0);
                    let new_epg_progs = new_epg.programme_count();
                    app.log.info("refresh-mem", format!(
                        "EPG STEP 0: RSS={:.0}MB | old EPG refs={} ({} progs) | new EPG has {} progs",
                        rss_pre_epg, old_epg_refs, old_epg_progs, new_epg_progs
                    ));

                    app.epg_last_refresh = Some(chrono::Utc::now());
                    let epg_arc = Arc::new(new_epg);
                    if let Some(ref hdhr_state) = app.hdhr_state {
                        let mut state = hdhr_state.lock().unwrap();
                        state.epg = Some(Arc::clone(&epg_arc));
                    }

                    // Drop old epg_search_engine before rebuilding (release old EPG Arc)
                    app.epg_search_engine = None;
                    let rss_after_old_drop = get_rss_mb();
                    app.log.info("refresh-mem", format!(
                        "EPG STEP 1 (drop old epg_search): RSS={:.0}MB (delta={:+.0}) | old EPG refs={}",
                        rss_after_old_drop, rss_after_old_drop - rss_pre_epg,
                        app.epg.as_ref().map(|e| Arc::strong_count(e)).unwrap_or(0)
                    ));

                    app.epg_search_engine = Some(epg_search::EpgSearchEngine::new(
                        Arc::clone(&app.playlist),
                        Some(Arc::clone(&epg_arc)),
                    ));

                    let prog_count = epg_arc.programme_count();
                    let ch_count = epg_arc.channel_count();

                    // Swap EPG — old Arc dropped
                    let old_epg_strong = app.epg.as_ref().map(|e| Arc::strong_count(e)).unwrap_or(0);
                    app.epg = Some(epg_arc);
                    let rss_after_epg_swap = get_rss_mb();
                    app.log.info("refresh-mem", format!(
                        "EPG STEP 2 (swap epg): RSS={:.0}MB (delta={:+.0}) | old EPG had {} refs | new EPG refs={}",
                        rss_after_epg_swap, rss_after_epg_swap - rss_pre_epg,
                        old_epg_strong,
                        app.epg.as_ref().map(|e| Arc::strong_count(e)).unwrap_or(0)
                    ));

                    app.guide_state.channels_dirty = true;
                    let msg = format!(
                        "Playlist + EPG refreshed: {} programmes for {} channels",
                        prog_count, ch_count
                    );
                    app.log.info("refresh", &msg);
                    app.status_message = Some(msg);
                }
                app.log.info("refresh-mem", "--- FINAL STATE ---");
                log_memory_diagnostics(&app);
            }
        }

        // Check for EPG-only refresh (stale cache on startup)
        if let Some(ref rx) = app.epg_refresh_rx {
            if let Ok(new_epg) = rx.try_recv() {
                app.epg_last_refresh = Some(chrono::Utc::now());
                let epg_arc = Arc::new(new_epg);
                if let Some(ref hdhr_state) = app.hdhr_state {
                    let mut state = hdhr_state.lock().unwrap();
                    state.epg = Some(Arc::clone(&epg_arc));
                }
                app.epg_search_engine = Some(epg_search::EpgSearchEngine::new(
                    Arc::clone(&app.playlist),
                    Some(Arc::clone(&epg_arc)),
                ));
                let prog_count = epg_arc.programme_count();
                let ch_count = epg_arc.channel_count();
                app.epg = Some(epg_arc);
                app.guide_state.channels_dirty = true;
                let msg = format!(
                    "EPG refreshed: {} programmes for {} channels",
                    prog_count, ch_count
                );
                app.log.info("refresh", &msg);
                app.status_message = Some(msg);
            }
        }

        // Rebuild caches if needed (before drawing)
        if app.mode == AppMode::Guide {
            app.ensure_guide_channels();
        }
        if app.show_favorites_only {
            app.ensure_favorite_cache();
        }

        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                // Clear status message on any keypress
                app.status_message = None;

                // Global Ctrl shortcuts (work in all modes)
                let handled = if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('c') => {
                            app.should_quit = true;
                            true
                        }
                        KeyCode::Char('d') => {
                            // Download selected item (context-aware)
                            match &app.mode {
                                AppMode::Search | AppMode::Normal => app.download_selected(),
                                AppMode::Guide => app.guide_download_selected(),
                                AppMode::Series => app.series_download_selected(),
                                _ => {}
                            }
                            true
                        }
                        _ => false,
                    }
                } else {
                    false
                };

                if !handled {
                    match &app.mode {
                        AppMode::Search => handle_search_key(app, key.code),
                        AppMode::Guide => handle_guide_key(app, key.code),
                        AppMode::Downloads => handle_downloads_key(app, key.code),
                        AppMode::Series => handle_series_key(app, key.code),
                        AppMode::Recordings => handle_recordings_key(app, key.code),
                        AppMode::SourceInfo => handle_source_info_key(app, key.code),
                        AppMode::Logs => handle_logs_key(app, key.code),
                        AppMode::Timezone => handle_timezone_key(app, key.code),
                        AppMode::Help => {
                            if matches!(
                                key.code,
                                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                            ) {
                                app.mode = AppMode::Normal;
                            }
                        }
                        AppMode::Normal => handle_normal_key(app, key.code),
                    }
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_normal_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            // Clear locked search results
            if !app.search_query.is_empty() {
                app.search_query.clear();
                app.search_results.clear();
                app.selected_channel = 0;
            }
        }
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.move_down(),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(),
        KeyCode::Char('h') | KeyCode::Left => {
            app.focus = app::Focus::Groups;
            // Clear locked search when switching to groups
            app.search_query.clear();
            app.search_results.clear();
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.focus = app::Focus::Channels;
        }
        KeyCode::Char('g') => app.jump_top(),
        KeyCode::Char('G') => app.enter_guide(),
        KeyCode::End => app.jump_bottom(),
        KeyCode::Char('/') => app.enter_search(),
        KeyCode::Char('s') | KeyCode::Char(' ') => app.toggle_favorite(),
        KeyCode::Char('f') => app.toggle_favorites_filter(),
        KeyCode::Char('d') => app.download_selected(),
        KeyCode::Char('D') => app.enter_downloads(),
        KeyCode::Char('S') => app.enter_series(),
        KeyCode::Char('r') => app.record_selected_channel(),
        KeyCode::Char('R') => app.enter_recordings(),
        KeyCode::Char('i') => app.enter_source_info(),
        KeyCode::Char('P') => app.cycle_player(),
        KeyCode::Char('L') => app.enter_logs(),
        KeyCode::Char('t') => {
            app.theme = theme::next_theme(app.theme.name);
            app.status_message = Some(format!("Theme: {}", app.theme.name));
        }
        KeyCode::Char('T') => app.enter_timezone(),
        KeyCode::Char('?') => app.mode = AppMode::Help,
        KeyCode::Enter => app.launch_selected(),
        KeyCode::Tab => app.toggle_focus(),
        _ => {}
    }
}

fn handle_guide_key(app: &mut App, code: KeyCode) {
    if app.guide_state.group_selector_active {
        match code {
            KeyCode::Esc => app.guide_group_selector_cancel(),
            KeyCode::Enter => app.guide_group_selector_select(),
            KeyCode::Up | KeyCode::Char('k') => app.guide_group_selector_up(),
            KeyCode::Down | KeyCode::Char('j') => app.guide_group_selector_down(),
            _ => {}
        }
    } else if app.guide_state.search_active {
        match code {
            KeyCode::Esc => {
                app.guide_state.search_query.clear();
                app.guide_state.search_results_set.clear();
                app.guide_state.epg_search_results.clear();
                app.guide_state.search_active = false;
                app.guide_state.selected_channel_idx = 0;
                app.guide_state.channels_dirty = true;
            }
            KeyCode::Enter => {
                app.guide_state.search_active = false;
                // Keep results filtered
            }
            KeyCode::Backspace => {
                app.guide_state.search_query.pop();
                app.guide_update_search();
            }
            KeyCode::Char(c) => {
                app.guide_state.search_query.push(c);
                app.guide_update_search();
            }
            KeyCode::Down => app.guide_move_down(),
            KeyCode::Up => app.guide_move_up(),
            _ => {}
        }
    } else {
        match code {
            KeyCode::Char('g') => app.guide_open_group_selector(),
            KeyCode::Char('f') => app.guide_switch_view(GuideViewMode::Favorites),
            KeyCode::Char('a') => app.guide_switch_view(GuideViewMode::All),
            KeyCode::Char('/') => {
                app.guide_state.search_active = true;
                app.guide_state.search_query.clear();
                app.guide_state.search_results_set.clear();
                app.guide_state.epg_search_results.clear();
                app.guide_state.channels_dirty = true;
            }
            KeyCode::Char('[') | KeyCode::Char('<') | KeyCode::PageUp => app.guide_shift_time_left(),
            KeyCode::Char(']') | KeyCode::Char('>') | KeyCode::PageDown => app.guide_shift_time_right(),
            KeyCode::Up | KeyCode::Char('k') => app.guide_move_up(),
            KeyCode::Down | KeyCode::Char('j') => app.guide_move_down(),
            KeyCode::Left | KeyCode::Char('h') => app.guide_move_left(),
            KeyCode::Right | KeyCode::Char('l') => app.guide_move_right(),
            KeyCode::Enter => app.guide_launch_selected(),
            KeyCode::Char('d') => app.guide_download_selected(),
            KeyCode::Char('D') => app.enter_downloads(),
            KeyCode::Char('r') => app.guide_record_selected(),
            KeyCode::Char('R') => app.enter_recordings(),
            KeyCode::Char('s') | KeyCode::Char(' ') => app.guide_toggle_favorite(),
            KeyCode::Esc => app.exit_guide(),
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        }
    }
}

fn handle_search_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.exit_search(),
        KeyCode::Enter => {
            // Lock search results and switch to Normal mode
            // (keeps filtered list, normal keybindings now work: s, d, Enter to play, etc.)
            app.mode = AppMode::Normal;
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            app.update_search();
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
            app.update_search();
        }
        KeyCode::Down | KeyCode::Tab => app.move_down(),
        KeyCode::Up => app.move_up(),
        _ => {}
    }
}

fn handle_source_info_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => app.exit_source_info(),
        KeyCode::Char('p') => app.trigger_playlist_refresh(),
        KeyCode::Char('e') => app.trigger_epg_refresh(),
        _ => {}
    }
}

fn handle_logs_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('L') => app.exit_logs(),
        KeyCode::Up | KeyCode::Char('k') => app.log_scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.log_scroll_down(),
        KeyCode::PageUp => app.log_scroll_page_up(),
        KeyCode::PageDown => app.log_scroll_page_down(),
        KeyCode::Home | KeyCode::Char('g') => {
            // Scroll to top (oldest)
            let total = app.log.entry_count();
            app.log_scroll = total.saturating_sub(1);
        }
        KeyCode::End | KeyCode::Char('G') => {
            // Scroll to bottom (newest)
            app.log_scroll = 0;
        }
        _ => {}
    }
}

fn handle_recordings_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.exit_recordings(),
        KeyCode::Up | KeyCode::Char('k') => app.recordings_move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.recordings_move_down(),
        KeyCode::Char('x') | KeyCode::Delete => app.recordings_cancel_selected(),
        _ => {}
    }
}

fn handle_downloads_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.exit_downloads(),
        KeyCode::Up | KeyCode::Char('k') => app.downloads_move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.downloads_move_down(),
        KeyCode::Char('x') | KeyCode::Delete => app.downloads_remove_selected(),
        _ => {}
    }
}

fn handle_series_key(app: &mut App, code: KeyCode) {
    let search_active = app.series_state.as_ref().is_some_and(|s| s.search_active);
    
    if search_active {
        match code {
            KeyCode::Esc => {
                if let Some(ref mut state) = app.series_state {
                    state.search_query.clear();
                    state.filtered_series = None;
                    state.search_active = false;
                    state.selected = 0;
                    state.rebuild_rows();
                }
            }
            KeyCode::Enter => {
                if let Some(ref mut state) = app.series_state {
                    state.search_active = false;
                    // Keep filter active
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut state) = app.series_state {
                    state.search_query.pop();
                    state.update_filter();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut state) = app.series_state {
                    state.search_query.push(c);
                    state.update_filter();
                }
            }
            KeyCode::Down => app.series_move_down(),
            KeyCode::Up => app.series_move_up(),
            _ => {}
        }
    } else {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.exit_series(),
            KeyCode::Char('/') => {
                if let Some(ref mut state) = app.series_state {
                    state.search_active = true;
                    state.search_query.clear();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => app.series_move_up(),
            KeyCode::Down | KeyCode::Char('j') => app.series_move_down(),
            KeyCode::Enter => {
                let is_episode = app.series_state.as_ref().is_some_and(|s| {
                    s.selected < s.visible_rows.len()
                        && matches!(s.visible_rows[s.selected], SeriesRow::Episode { .. })
                });
                if is_episode {
                    app.series_launch_selected();
                } else {
                    app.series_toggle_expand();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => app.series_toggle_expand(),
            KeyCode::Left | KeyCode::Char('h') => app.series_collapse(),
            KeyCode::Char('d') => app.series_download_selected(),
            KeyCode::Char('D') => app.enter_downloads(),
            KeyCode::Char('u') => app.series_toggle_duplicates(),
            _ => {}
        }
    }
}

fn handle_timezone_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.exit_timezone(),
        KeyCode::Enter => app.tz_select(),
        KeyCode::Up | KeyCode::Char('k') => app.tz_move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.tz_move_down(),
        KeyCode::Backspace => {
            if !app.tz_search.is_empty() {
                app.tz_search.pop();
                app.tz_update_search(app.tz_search.clone());
            }
        }
        KeyCode::Char(c) => {
            app.tz_search.push(c);
            app.tz_update_search(app.tz_search.clone());
        }
        _ => {}
    }
}
