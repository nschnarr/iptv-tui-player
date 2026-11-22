#![allow(dead_code)]

use thiserror::Error;

#[derive(Error, Debug)]
pub enum IptvError {
    #[error("Failed to parse M3U file: {0}")]
    ParseError(String),

    #[error("No playlist file specified")]
    NoPlaylist,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Player error: {0}")]
    PlayerError(String),
}
