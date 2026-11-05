use crate::model::{ContentType, Playlist};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

/// A single episode in the series tree.
#[derive(Debug, Clone)]
pub struct Episode {
    /// Index into playlist.channels
    pub channel_idx: usize,
    pub season: u32,
    pub episode: u32,
    pub name: Arc<str>,
    /// Whether this is a duplicate (same season+episode number as an earlier entry).
    pub is_duplicate: bool,
}

/// A season containing episodes.
#[derive(Debug, Clone)]
pub struct Season {
    #[allow(dead_code)]
    pub number: u32,
    pub episodes: Vec<Episode>,
}

/// A series containing seasons.
#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub seasons: BTreeMap<u32, Season>,
}

impl Series {
    pub fn episode_count(&self) -> usize {
        self.seasons.values().map(|s| s.episodes.len()).sum()
    }

    /// Count of unique (non-duplicate) episodes.
    pub fn unique_episode_count(&self) -> usize {
        self.seasons.values().map(|s| s.episodes.iter().filter(|e| !e.is_duplicate).count()).sum()
    }

    /// Count of duplicate episodes.
    pub fn duplicate_count(&self) -> usize {
        self.episode_count() - self.unique_episode_count()
    }
}

impl Season {
    pub fn unique_episode_count(&self) -> usize {
        self.episodes.iter().filter(|e| !e.is_duplicate).count()
    }
}

/// Build a series tree from the playlist.
/// Returns series sorted by name. Each series contains seasons sorted by number,
/// each season contains episodes sorted by episode number.
pub fn build_series_tree(playlist: &Playlist) -> Vec<Series> {
    let mut map: BTreeMap<String, BTreeMap<u32, Vec<Episode>>> = BTreeMap::new();

    for (idx, channel) in playlist.channels.iter().enumerate() {
        if let ContentType::Series {
            ref series_name,
            season,
            episode,
        } = channel.content_type
        {
            let key = series_name.to_string();
            map.entry(key)
                .or_default()
                .entry(season)
                .or_default()
                .push(Episode {
                    channel_idx: idx,
                    season,
                    episode,
                    name: channel.name.clone(),
                    is_duplicate: false, // marked below
                });
        }
    }

    let mut result: Vec<Series> = map
        .into_iter()
        .map(|(name, seasons_map)| {
            let seasons: BTreeMap<u32, Season> = seasons_map
                .into_iter()
                .map(|(num, mut eps)| {
                    eps.sort_by_key(|e| e.episode);
                    // Mark duplicates: first occurrence of each episode number is primary
                    let mut seen = HashSet::new();
                    for ep in &mut eps {
                        if !seen.insert(ep.episode) {
                            ep.is_duplicate = true;
                        }
                    }
                    (num, Season { number: num, episodes: eps })
                })
                .collect();
            Series { name, seasons }
        })
        .collect();

    result.sort_by_key(|a| a.name.to_lowercase());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, ContentType, Playlist};
    use std::collections::HashMap;

    fn make_channel(name: &str, group: &str, ct: ContentType, idx: usize) -> Channel {
        Channel {
            name: Arc::from(name),
            url: Arc::from("http://test"),
            group: Arc::from(group),
            logo_url: None,
            tvg_id: None,
            tvg_name: None,
            tvg_language: None,
            tvg_country: None,
            content_type: ct,
            index: idx,
        }
    }

    fn make_playlist(channels: Vec<Channel>) -> Playlist {
        let mut groups: Vec<Arc<str>> = Vec::new();
        let mut group_indices: HashMap<Arc<str>, Vec<usize>> = HashMap::new();
        for (i, ch) in channels.iter().enumerate() {
            if !groups.iter().any(|g| g.as_ref() == ch.group.as_ref()) {
                groups.push(ch.group.clone());
            }
            group_indices.entry(ch.group.clone()).or_default().push(i);
        }
        Playlist { channels, groups, group_indices }
    }

    #[test]
    fn test_empty_playlist() {
        let pl = make_playlist(vec![]);
        let tree = build_series_tree(&pl);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_no_series_content() {
        let pl = make_playlist(vec![
            make_channel("BBC One", "UK", ContentType::Live, 0),
            make_channel("Die Hard", "Movies", ContentType::Movie, 1),
        ]);
        let tree = build_series_tree(&pl);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_single_series_single_episode() {
        let pl = make_playlist(vec![
            make_channel("Breaking Bad S01E01", "Series", ContentType::Series {
                series_name: Arc::from("Breaking Bad"),
                season: 1,
                episode: 1,
            }, 0),
        ]);
        let tree = build_series_tree(&pl);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "Breaking Bad");
        assert_eq!(tree[0].episode_count(), 1);
        assert_eq!(tree[0].seasons.len(), 1);
    }

    #[test]
    fn test_multiple_seasons() {
        let pl = make_playlist(vec![
            make_channel("BB S01E01", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 1, episode: 1,
            }, 0),
            make_channel("BB S01E02", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 1, episode: 2,
            }, 1),
            make_channel("BB S02E01", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 2, episode: 1,
            }, 2),
        ]);
        let tree = build_series_tree(&pl);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].seasons.len(), 2);
        assert_eq!(tree[0].seasons[&1].episodes.len(), 2);
        assert_eq!(tree[0].seasons[&2].episodes.len(), 1);
        assert_eq!(tree[0].episode_count(), 3);
    }

    #[test]
    fn test_duplicate_episodes_marked() {
        let pl = make_playlist(vec![
            make_channel("BB S01E01 HD", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 1, episode: 1,
            }, 0),
            make_channel("BB S01E01 SD", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 1, episode: 1,
            }, 1),
            make_channel("BB S01E02", "S", ContentType::Series {
                series_name: Arc::from("BB"), season: 1, episode: 2,
            }, 2),
        ]);
        let tree = build_series_tree(&pl);
        let s1 = &tree[0].seasons[&1];
        assert_eq!(s1.episodes.len(), 3);
        assert!(!s1.episodes[0].is_duplicate); // first E01
        assert!(s1.episodes[1].is_duplicate);  // second E01
        assert!(!s1.episodes[2].is_duplicate); // E02
        assert_eq!(tree[0].unique_episode_count(), 2);
        assert_eq!(tree[0].duplicate_count(), 1);
        assert_eq!(s1.unique_episode_count(), 2);
    }

    #[test]
    fn test_multiple_series_sorted() {
        let pl = make_playlist(vec![
            make_channel("Z Show S01E01", "S", ContentType::Series {
                series_name: Arc::from("Z Show"), season: 1, episode: 1,
            }, 0),
            make_channel("A Show S01E01", "S", ContentType::Series {
                series_name: Arc::from("A Show"), season: 1, episode: 1,
            }, 1),
            make_channel("M Show S01E01", "S", ContentType::Series {
                series_name: Arc::from("M Show"), season: 1, episode: 1,
            }, 2),
        ]);
        let tree = build_series_tree(&pl);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].name, "A Show");
        assert_eq!(tree[1].name, "M Show");
        assert_eq!(tree[2].name, "Z Show");
    }

    #[test]
    fn test_episodes_sorted_by_number() {
        let pl = make_playlist(vec![
            make_channel("S01E05", "S", ContentType::Series {
                series_name: Arc::from("Show"), season: 1, episode: 5,
            }, 0),
            make_channel("S01E01", "S", ContentType::Series {
                series_name: Arc::from("Show"), season: 1, episode: 1,
            }, 1),
            make_channel("S01E03", "S", ContentType::Series {
                series_name: Arc::from("Show"), season: 1, episode: 3,
            }, 2),
        ]);
        let tree = build_series_tree(&pl);
        let eps = &tree[0].seasons[&1].episodes;
        assert_eq!(eps[0].episode, 1);
        assert_eq!(eps[1].episode, 3);
        assert_eq!(eps[2].episode, 5);
    }

    #[test]
    fn test_mixed_content_types() {
        let pl = make_playlist(vec![
            make_channel("Live CH", "L", ContentType::Live, 0),
            make_channel("Movie", "M", ContentType::Movie, 1),
            make_channel("Series S01E01", "S", ContentType::Series {
                series_name: Arc::from("Series"), season: 1, episode: 1,
            }, 2),
        ]);
        let tree = build_series_tree(&pl);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "Series");
    }
}
