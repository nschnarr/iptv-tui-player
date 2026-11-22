//! Core IPTV logic: playlist/EPG loading, favourites, caching and the
//! HDHomeRun tuner emulation. Shared by the `iptv` TUI and `iptv-server`.

pub mod cache;
pub mod channel_numbers;
pub mod epg;
pub mod epg_search;
pub mod error;
pub mod favorites;
pub mod hdhr;
pub mod hdhr_discovery;
pub mod model;
pub mod parser;
pub mod paths;
pub mod provider;
pub mod redact;
pub mod search;
pub mod xtream;
