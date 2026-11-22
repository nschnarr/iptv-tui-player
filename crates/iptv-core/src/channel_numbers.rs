//! Stable HDHomeRun guide numbers for favourite channels.
//!
//! Plex keys its DVR channel map and guide on the tuner's guide numbers, so a
//! favourite must keep its number when other favourites are added or removed.
//! Numbers are persisted per favourite key and are never reused for a different
//! channel, so un-favouriting and re-favouriting a channel gives it back the
//! same number.

use std::collections::HashMap;
use std::path::PathBuf;

pub struct ChannelNumbers {
    map: HashMap<String, u32>,
    path: Option<PathBuf>,
}

impl ChannelNumbers {
    /// Load from `<data_dir>/channel_numbers.json`, starting empty if missing.
    pub fn load() -> Self {
        let path = crate::paths::data_dir().join("channel_numbers.json");
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            map,
            path: Some(path),
        }
    }

    /// Non-persistent store (tests, or callers that don't want disk state).
    pub fn in_memory() -> Self {
        Self {
            map: HashMap::new(),
            path: None,
        }
    }

    pub fn get(&self, key: &str) -> Option<u32> {
        self.map.get(key).copied()
    }

    /// Assign numbers to any keys that don't have one yet, in the order given,
    /// then persist if anything changed. New numbers start after the highest
    /// number ever assigned.
    pub fn assign_all<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) {
        let mut next = self.map.values().max().copied().unwrap_or(0) + 1;
        let mut changed = false;
        for key in keys {
            if !self.map.contains_key(key) {
                self.map.insert(key.to_string(), next);
                next += 1;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&self.map) {
            Ok(json) => {
                // Write-then-rename so a crash can't leave a truncated file.
                let tmp = path.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path))
                {
                    eprintln!("Failed to save channel numbers: {e}");
                }
            }
            Err(e) => eprintln!("Failed to serialise channel numbers: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_sequentially_and_keeps_existing() {
        let mut n = ChannelNumbers::in_memory();
        n.assign_all(["a", "b"]);
        assert_eq!(n.get("a"), Some(1));
        assert_eq!(n.get("b"), Some(2));
        // Adding a key before existing ones must not renumber them.
        n.assign_all(["c", "a", "b"]);
        assert_eq!(n.get("a"), Some(1));
        assert_eq!(n.get("b"), Some(2));
        assert_eq!(n.get("c"), Some(3));
    }

    #[test]
    fn numbers_are_not_reused() {
        let mut n = ChannelNumbers::in_memory();
        n.assign_all(["a", "b"]);
        // "a" unfavourited (still remembered); a new key gets a fresh number.
        n.assign_all(["b", "d"]);
        assert_eq!(n.get("d"), Some(3));
        assert_eq!(n.get("a"), Some(1));
    }
}
