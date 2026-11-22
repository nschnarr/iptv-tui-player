use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
struct CacheMeta {
    #[serde(default)]
    playlist_url: Option<String>,
    #[serde(default)]
    epg_url: Option<String>,
}

fn config_dir() -> PathBuf {
    crate::paths::data_dir()
}

fn cache_dir() -> PathBuf {
    let dir = config_dir().join("cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn meta_path() -> PathBuf {
    cache_dir().join("cache_meta.json")
}

fn load_meta() -> CacheMeta {
    let path = meta_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_meta(meta: &CacheMeta) {
    let path = meta_path();
    if let Ok(data) = serde_json::to_string(meta) {
        let _ = fs::write(&path, data);
    }
}

fn playlist_cache_path() -> PathBuf {
    cache_dir().join("playlist.m3u")
}

fn epg_cache_path() -> PathBuf {
    cache_dir().join("epg.xml")
}

/// Save raw M3U data to cache, recording the source URL.
pub fn save_playlist(data: &[u8], url: Option<&str>) {
    let path = playlist_cache_path();
    if let Err(e) = fs::write(&path, data) {
        eprintln!("Failed to cache playlist: {}", e);
        return;
    }
    if url.is_some() {
        let mut meta = load_meta();
        meta.playlist_url = url.map(|s| s.to_string());
        save_meta(&meta);
    }
}

/// Save raw EPG/XMLTV data to cache, recording the source URL.
pub fn save_epg(data: &[u8], url: Option<&str>) {
    let path = epg_cache_path();
    if let Err(e) = fs::write(&path, data) {
        eprintln!("Failed to cache EPG: {}", e);
        return;
    }
    if url.is_some() {
        let mut meta = load_meta();
        meta.epg_url = url.map(|s| s.to_string());
        save_meta(&meta);
    }
}

/// Check if the cached playlist was fetched from a different URL.
pub fn playlist_source_changed(current_url: &str) -> bool {
    let meta = load_meta();
    match meta.playlist_url {
        Some(ref cached_url) => cached_url != current_url,
        None => false, // No recorded URL — don't invalidate
    }
}

/// Check if the cached EPG was fetched from a different URL.
pub fn epg_source_changed(current_url: &str) -> bool {
    let meta = load_meta();
    match meta.epg_url {
        Some(ref cached_url) => cached_url != current_url,
        None => false,
    }
}

/// Load cached playlist if it exists. Returns the path.
pub fn load_playlist_path() -> Option<PathBuf> {
    let path = playlist_cache_path();
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Load cached EPG if it exists. Returns the path.
pub fn load_epg_path() -> Option<PathBuf> {
    let path = epg_cache_path();
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Get the modification time of the cached playlist as a human-readable string.
pub fn playlist_cache_age() -> Option<String> {
    age_string(&playlist_cache_path())
}

/// Get the modification time of the cached EPG as a human-readable string.
pub fn epg_cache_age() -> Option<String> {
    age_string(&epg_cache_path())
}

/// Check if the playlist cache is stale (older than max_age_hours).
pub fn playlist_cache_stale(max_age_hours: f64) -> bool {
    cache_stale(&playlist_cache_path(), max_age_hours)
}

/// Check if the EPG cache is stale (older than max_age_hours).
pub fn epg_cache_stale(max_age_hours: f64) -> bool {
    cache_stale(&epg_cache_path(), max_age_hours)
}

fn cache_stale(path: &PathBuf, max_age_hours: f64) -> bool {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true, // No cache = stale
    };
    let modified = match meta.modified() {
        Ok(m) => m,
        Err(_) => return true,
    };
    let elapsed = match modified.elapsed() {
        Ok(e) => e,
        Err(_) => return true,
    };
    elapsed.as_secs_f64() > max_age_hours * 3600.0
}

fn age_string(path: &PathBuf) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    let secs = elapsed.as_secs();
    if secs < 60 {
        Some("just now".to_string())
    } else if secs < 3600 {
        Some(format!("{}m ago", secs / 60))
    } else if secs < 86400 {
        Some(format!("{}h {}m ago", secs / 3600, (secs % 3600) / 60))
    } else {
        Some(format!("{}d ago", secs / 86400))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_cache_stale_missing_file() {
        let _guard = crate::paths::test_data_dir();
        let p = PathBuf::from("/tmp/nonexistent_iptv_cache_test_file");
        assert!(cache_stale(&p, 24.0));
    }

    #[test]
    fn test_cache_stale_fresh_file() {
        let _guard = crate::paths::test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fresh.m3u");
        std::fs::write(&p, b"#EXTM3U\n").unwrap();
        // Just written — should not be stale for 24h
        assert!(!cache_stale(&p.clone(), 24.0));
    }

    #[test]
    fn test_cache_stale_zero_hours() {
        let _guard = crate::paths::test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("zero.m3u");
        std::fs::write(&p, b"data").unwrap();
        // 0 hours means anything is stale (elapsed > 0)
        // Might be flaky if filesystem is instant, but practically always true
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(cache_stale(&p.clone(), 0.0));
    }

    #[test]
    fn test_age_string_missing_file() {
        let _guard = crate::paths::test_data_dir();
        let p = PathBuf::from("/tmp/nonexistent_iptv_age_test");
        assert!(age_string(&p).is_none());
    }

    #[test]
    fn test_age_string_just_created() {
        let _guard = crate::paths::test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("age.txt");
        std::fs::write(&p, b"x").unwrap();
        let s = age_string(&p.clone()).unwrap();
        // Should be "just now" or "Xm ago" (if slow CI)
        assert!(s == "just now" || s.ends_with("m ago"), "got: {}", s);
    }

    #[test]
    fn test_save_and_load_playlist() {
        let _guard = crate::paths::test_data_dir();
        // Uses real config dir — just verify the functions don't panic
        let data = b"#EXTM3U\n#EXTINF:-1,Test\nhttp://example.com/stream\n";
        save_playlist(data, Some("http://test"));
        let path = load_playlist_path();
        assert!(path.is_some());
    }

    #[test]
    fn test_save_and_load_epg() {
        let _guard = crate::paths::test_data_dir();
        let data = b"<?xml version=\"1.0\"?><tv></tv>";
        save_epg(data, Some("http://test"));
        let path = load_epg_path();
        assert!(path.is_some());
    }

    #[test]
    fn test_playlist_cache_age_exists() {
        let _guard = crate::paths::test_data_dir();
        save_playlist(b"test", None);
        let age = playlist_cache_age();
        assert!(age.is_some());
    }

    #[test]
    fn test_epg_cache_stale_large_window() {
        let _guard = crate::paths::test_data_dir();
        save_epg(b"test", None);
        // Just saved, should not be stale at 9999 hours
        assert!(!epg_cache_stale(9999.0));
    }

    #[test]
    fn test_source_change_detection() {
        let _guard = crate::paths::test_data_dir();
        // Single test to avoid parallel race on shared cache_meta.json

        // Playlist: same URL → not changed
        save_playlist(b"#EXTM3U\n", Some("http://example.com/list.m3u"));
        assert!(!playlist_source_changed("http://example.com/list.m3u"));

        // Playlist: different URL → changed
        assert!(playlist_source_changed("http://example.com/new.m3u"));

        // Playlist: save with no URL → don't invalidate
        save_playlist(b"#EXTM3U\n", None);
        // meta still has old URL from above, but save with None doesn't update it
        // so it still reports changed for a different URL
        assert!(playlist_source_changed("http://something-else.com"));

        // EPG: same URL → not changed
        save_epg(b"<tv/>", Some("http://epg.example.com/guide.xml"));
        assert!(!epg_source_changed("http://epg.example.com/guide.xml"));

        // EPG: different URL → changed
        assert!(epg_source_changed("http://different.com/guide.xml"));
    }
}
