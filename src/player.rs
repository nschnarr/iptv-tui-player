use crate::log::AppLog;
use anyhow::{Context, Result};
use std::fmt;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Player {
    Vlc,
    Mpv,
}

impl Player {
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "vlc" => Ok(Self::Vlc),
            "mpv" => Ok(Self::Mpv),
            _ => anyhow::bail!("Unknown player: {}. Use 'vlc' or 'mpv'", s),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Player::Vlc => Player::Mpv,
            Player::Mpv => Player::Vlc,
        }
    }
}

impl fmt::Display for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Player::Vlc => write!(f, "VLC"),
            Player::Mpv => write!(f, "mpv"),
        }
    }
}

pub fn launch(player: Player, url: &str, log: &AppLog) -> Result<()> {
    let mut cmd = match player {
        Player::Vlc => {
            let mut c = std::process::Command::new("vlc");
            c.arg(url).arg("--no-video-title-show");
            c
        }
        Player::Mpv => {
            let mut c = std::process::Command::new("mpv");
            c.arg(url);
            c
        }
    };

    let player_name = player.to_string();
    log.info("player", format!("Launching {} with: {}", player_name, url));

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(format!("Failed to launch {}. Is it installed?", player_name))?;

    // Spawn threads to read stdout/stderr and pipe to app log
    let log_out = log.clone();
    let name_out = player_name.clone();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    log_out.player(&name_out, &line);
                }
            }
        });
    }

    let log_err = log.clone();
    let name_err = player_name;
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    log_err.player(&name_err, &line);
                }
            }
        });
    }

    // Don't wait for child — let it run in background
    // Spawn a thread to reap it so we don't get zombies
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}
