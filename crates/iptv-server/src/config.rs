//! Server configuration, read from environment variables (container-friendly).

use anyhow::{bail, Context, Result};
use iptv_core::hdhr::{BufferMode, DeviceInfo};
use iptv_core::provider::Provider;
use iptv_core::xtream::XtreamProvider;
use std::path::PathBuf;
use std::time::Duration;

pub const HELP: &str = "\
iptv-server — headless IPTV tuner with a web UI and Plex DVR sync

Usage: iptv-server             run the tuner, web UI and Plex sync
       iptv-server discovery   run only the HDHomeRun UDP discovery responder

All configuration is via environment variables:

Playlist source (one of):
  IPTV_M3U_URL              Remote M3U playlist URL
  IPTV_M3U_PATH             Local M3U file path
  IPTV_XTREAM_SERVER        Xtream Codes server URL (with IPTV_XTREAM_USERNAME
  IPTV_XTREAM_USERNAME        and IPTV_XTREAM_PASSWORD)
  IPTV_XTREAM_PASSWORD

Guide:
  IPTV_EPG                  XMLTV URL or file path (optional)

Storage:
  IPTV_DATA_DIR             Favourites, channel numbers and cache (default: ~/.config/iptv)

Network:
  IPTV_BIND                 Bind address for both ports (default: 0.0.0.0)
  IPTV_UI_PORT              Web UI / API port (default: 8080)
  IPTV_HDHR_PORT            HDHomeRun tuner port, point Plex here (default: 5004)

Tuner:
  IPTV_TUNER_COUNT          Concurrent streams advertised to Plex (default: 2)
  IPTV_DEVICE_ID            Tuner device ID Plex tracks (default: 1F7E5E07)
  IPTV_FRIENDLY_NAME        Tuner name shown in Plex (default: IPTV Server)
  IPTV_BUFFER               none | ffmpeg (default: none)
  IPTV_BUFFER_SECS          Pre-roll seconds before streaming (default: 0)
  IPTV_REFRESH_HOURS        Re-fetch playlist + EPG every N hours, 0 = off (default: 12)

Discovery responder (`iptv-server discovery`, run on the host network):
  IPTV_TUNER_URL            Where Plex should reach the tuner, e.g. http://192.168.1.50:5004
  IPTV_DEVICE_ID            Must match the main server's (default: 1F7E5E07)
  IPTV_TUNER_COUNT          Must match the main server's (default: 2)
  IPTV_DISCOVERY_BIND       UDP bind address (default: 0.0.0.0)

Plex (optional; enables channel sync):
  PLEX_URL                  e.g. http://plex.media.svc:32400
  PLEX_TOKEN                X-Plex-Token
  PLEX_DVR_ID               DVR key to manage (default: auto-detect by device ID)
  PLEX_AUTO_SYNC            Push lineup changes to Plex automatically (default: true)
  PLEX_SYNC_DELAY_SECS      Wait this long after the last change before syncing (default: 10)
";

pub struct Config {
    pub provider: Provider,
    pub epg_source: Option<String>,
    pub bind: String,
    pub ui_port: u16,
    pub hdhr_port: u16,
    pub device: DeviceInfo,
    pub buffer_mode: BufferMode,
    pub buffer_secs: f64,
    pub refresh_interval: Option<Duration>,
    pub plex: Option<PlexConfig>,
}

#[derive(Clone)]
pub struct PlexConfig {
    pub url: String,
    pub token: String,
    pub dvr_id: Option<String>,
    pub auto_sync: bool,
    pub sync_delay: Duration,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parsed<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match var(name) {
        Some(v) => v
            .parse()
            .ok()
            .with_context(|| format!("{name} has an invalid value: {v:?}")),
        None => Ok(default),
    }
}

fn flag(name: &str, default: bool) -> Result<bool> {
    match var(name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(default),
        Some(v) if ["1", "true", "yes", "on"].contains(&v.as_str()) => Ok(true),
        Some(v) if ["0", "false", "no", "off"].contains(&v.as_str()) => Ok(false),
        Some(v) => bail!("{name} must be true or false, got {v:?}"),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let provider = match (
            var("IPTV_M3U_URL"),
            var("IPTV_M3U_PATH"),
            var("IPTV_XTREAM_SERVER"),
        ) {
            (_, _, Some(server)) => Provider::Xtream(XtreamProvider {
                server,
                username: var("IPTV_XTREAM_USERNAME")
                    .context("IPTV_XTREAM_USERNAME is required with IPTV_XTREAM_SERVER")?,
                password: var("IPTV_XTREAM_PASSWORD")
                    .context("IPTV_XTREAM_PASSWORD is required with IPTV_XTREAM_SERVER")?,
            }),
            (Some(url), _, None) => Provider::M3u {
                path: None,
                url: Some(url),
            },
            (None, Some(path), None) => Provider::M3u {
                path: Some(PathBuf::from(path)),
                url: None,
            },
            (None, None, None) => bail!(
                "No playlist configured: set IPTV_M3U_URL, IPTV_M3U_PATH or IPTV_XTREAM_SERVER \
                 (run with --help for all options)"
            ),
        };

        let refresh_hours: f64 = parsed("IPTV_REFRESH_HOURS", 12.0)?;
        let defaults = DeviceInfo::default();

        let plex = match (var("PLEX_URL"), var("PLEX_TOKEN")) {
            (Some(url), Some(token)) => Some(PlexConfig {
                url: url.trim_end_matches('/').to_string(),
                token,
                dvr_id: var("PLEX_DVR_ID"),
                auto_sync: flag("PLEX_AUTO_SYNC", true)?,
                sync_delay: Duration::from_secs(parsed("PLEX_SYNC_DELAY_SECS", 10)?),
            }),
            (None, None) => None,
            _ => bail!("PLEX_URL and PLEX_TOKEN must be set together"),
        };

        Ok(Self {
            provider,
            epg_source: var("IPTV_EPG"),
            bind: var("IPTV_BIND").unwrap_or_else(|| "0.0.0.0".to_string()),
            ui_port: parsed("IPTV_UI_PORT", 8080)?,
            hdhr_port: parsed("IPTV_HDHR_PORT", 5004)?,
            device: DeviceInfo {
                device_id: var("IPTV_DEVICE_ID").unwrap_or_else(|| "1F7E5E07".to_string()),
                friendly_name: var("IPTV_FRIENDLY_NAME")
                    .unwrap_or_else(|| "IPTV Server".to_string()),
                tuner_count: parsed("IPTV_TUNER_COUNT", defaults.tuner_count)?,
            },
            buffer_mode: BufferMode::from_str(
                &var("IPTV_BUFFER").unwrap_or_else(|| "none".to_string()),
            )?,
            buffer_secs: parsed("IPTV_BUFFER_SECS", 0.0)?,
            refresh_interval: (refresh_hours > 0.0)
                .then(|| Duration::from_secs_f64(refresh_hours * 3600.0)),
            plex,
        })
    }
}
