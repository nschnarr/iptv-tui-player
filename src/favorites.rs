use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;

fn favorites_path() -> PathBuf {
    let config_dir = dirs_fallback();
    config_dir.join("favorites.json")
}

/// Legacy path (URL-based favorites from older versions)
fn legacy_favorites_path() -> PathBuf {
    let config_dir = dirs_fallback();
    config_dir.join("favorites_legacy.json")
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("iptv")
    } else {
        PathBuf::from(".config").join("iptv")
    }
}

/// Load favorites. These are stored as "name\tgroup" keys for stability
/// across playlist reloads (URLs often contain expiring tokens).
pub fn load_favorites() -> HashSet<String> {
    let path = favorites_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => HashSet::new(),
    }
}

pub fn save_favorites(favorites: &HashSet<String>) -> Result<()> {
    let path = favorites_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(favorites)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Generate a stable favorite key for a channel (name + group).
pub fn favorite_key(name: &str, group: &str) -> String {
    format!("{}\t{}", name, group)
}

/// Migrate old URL-based favorites to name-based.
/// Call once at startup with the current playlist.
pub fn migrate_if_needed(
    current_favorites: &HashSet<String>,
    channels: &[(String, String, String)], // (name, group, url)
) -> Option<HashSet<String>> {
    if current_favorites.is_empty() {
        return None;
    }

    // Check if any current favorites look like URLs (old format)
    let has_urls = current_favorites.iter().any(|f| f.starts_with("http"));
    if !has_urls {
        return None; // Already migrated
    }

    // Build URL → (name, group) lookup
    let mut migrated = HashSet::new();
    for (name, group, url) in channels {
        if current_favorites.contains(url) {
            migrated.insert(favorite_key(name, group));
        }
    }

    // Back up old favorites
    let legacy_path = legacy_favorites_path();
    if let Ok(json) = serde_json::to_string_pretty(current_favorites) {
        let _ = std::fs::write(legacy_path, json);
    }

    Some(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_favorite_key_format() {
        assert_eq!(favorite_key("BBC One", "UK"), "BBC One\tUK");
        assert_eq!(favorite_key("", ""), "\t");
        assert_eq!(favorite_key("CNN", "News"), "CNN\tNews");
    }

    #[test]
    fn test_favorite_key_tab_separator() {
        let key = favorite_key("Channel Name", "Group Name");
        assert!(key.contains('\t'));
        let parts: Vec<&str> = key.split('\t').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "Channel Name");
        assert_eq!(parts[1], "Group Name");
    }

    #[test]
    fn test_migrate_empty_favorites() {
        let empty: HashSet<String> = HashSet::new();
        let channels = vec![("ch1".into(), "g1".into(), "http://a".into())];
        assert!(migrate_if_needed(&empty, &channels).is_none());
    }

    #[test]
    fn test_migrate_already_migrated() {
        let mut favs = HashSet::new();
        favs.insert("BBC One\tUK".to_string());
        let channels = vec![("BBC One".into(), "UK".into(), "http://bbc".into())];
        // No URLs in favorites = already migrated
        assert!(migrate_if_needed(&favs, &channels).is_none());
    }

    #[test]
    fn test_migrate_url_favorites() {
        let mut favs = HashSet::new();
        favs.insert("http://example.com/stream1".to_string());
        favs.insert("http://example.com/stream2".to_string());

        let channels = vec![
            (
                "BBC One".to_string(),
                "UK".to_string(),
                "http://example.com/stream1".to_string(),
            ),
            (
                "CNN".to_string(),
                "News".to_string(),
                "http://example.com/stream2".to_string(),
            ),
            (
                "ESPN".to_string(),
                "Sports".to_string(),
                "http://example.com/stream3".to_string(),
            ),
        ];

        let result = migrate_if_needed(&favs, &channels);
        assert!(result.is_some());
        let migrated = result.unwrap();
        assert_eq!(migrated.len(), 2);
        assert!(migrated.contains(&favorite_key("BBC One", "UK")));
        assert!(migrated.contains(&favorite_key("CNN", "News")));
        assert!(!migrated.contains(&favorite_key("ESPN", "Sports")));
    }

    #[test]
    fn test_migrate_unmatched_urls_dropped() {
        let mut favs = HashSet::new();
        favs.insert("http://dead-link.com/gone".to_string());
        let channels = vec![("Ch1".into(), "G1".into(), "http://other.com".into())];
        let result = migrate_if_needed(&favs, &channels);
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("favorites.json");
        let mut favs = HashSet::new();
        favs.insert("BBC One\tUK".to_string());
        favs.insert("CNN\tNews".to_string());

        let json = serde_json::to_string_pretty(&favs).unwrap();
        std::fs::write(&path, &json).unwrap();

        let loaded: HashSet<String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(favs, loaded);
    }
}
