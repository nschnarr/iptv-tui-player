use crate::epg::Epg;
use crate::model::Playlist;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::{mpsc, Arc};
use std::thread;

pub struct EpgSearchEngine {
    query_tx: mpsc::SyncSender<EpgSearchQuery>,
    result_rx: mpsc::Receiver<EpgSearchResult>,
}

struct EpgSearchQuery {
    query: String,
    time_start: DateTime<Utc>,
    time_end: DateTime<Utc>,
}

pub struct EpgSearchResult {
    pub query: String,
    pub channel_indices: HashSet<usize>,
}

impl EpgSearchEngine {
    pub fn new(playlist: Arc<Playlist>, epg: Option<Arc<Epg>>) -> Self {
        let (query_tx, query_rx) = mpsc::sync_channel::<EpgSearchQuery>(4);
        let (result_tx, result_rx) = mpsc::channel::<EpgSearchResult>();

        thread::Builder::new()
            .name("epg-search".into())
            .spawn(move || {
                while let Ok(mut query_obj) = query_rx.recv() {
                    // Drain to latest query
                    while let Ok(newer) = query_rx.try_recv() {
                        query_obj = newer;
                    }

                    if query_obj.query.is_empty() {
                        let _ = result_tx.send(EpgSearchResult {
                            query: query_obj.query,
                            channel_indices: HashSet::new(),
                        });
                        continue;
                    }

                    let query_lower = query_obj.query.to_lowercase();
                    let mut matches = HashSet::new();

                    if let Some(ref epg) = epg {
                        for (i, channel) in playlist.channels.iter().enumerate() {
                            if let Some(tvg_id) = channel.tvg_id.as_deref() {
                                let progs = epg.programmes_in_range(
                                    tvg_id,
                                    query_obj.time_start,
                                    query_obj.time_end,
                                );
                                for prog in &progs {
                                    if prog.title.to_lowercase().contains(&query_lower) {
                                        matches.insert(i);
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    let _ = result_tx.send(EpgSearchResult {
                        query: query_obj.query,
                        channel_indices: matches,
                    });
                }
            })
            .expect("failed to spawn EPG search thread");

        Self {
            query_tx,
            result_rx,
        }
    }

    pub fn send_query(&self, query: &str, time_start: DateTime<Utc>, time_end: DateTime<Utc>) {
        let _ = self.query_tx.try_send(EpgSearchQuery {
            query: query.to_string(),
            time_start,
            time_end,
        });
    }

    pub fn try_recv(&self) -> Option<EpgSearchResult> {
        self.result_rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epg::{Epg, Programme};
    use crate::model::{Channel, ContentType, Playlist};
    use std::collections::HashMap;

    fn make_test_channel(name: &str, tvg_id: Option<&str>) -> Channel {
        Channel {
            name: Arc::from(name),
            url: Arc::from("http://test"),
            group: Arc::from("Test"),
            logo_url: None,
            tvg_id: tvg_id.map(Arc::from),
            tvg_name: None,
            tvg_language: None,
            tvg_country: None,
            content_type: ContentType::Live,
            index: 0,
        }
    }

    fn make_test_epg() -> Epg {
        let now = Utc::now();
        let mut data = HashMap::new();
        data.insert(
            "CNN.us".to_string(),
            vec![Programme {
                start: now - chrono::Duration::hours(1),
                stop: now + chrono::Duration::hours(1),
                title: "Breaking News Live".to_string(),
                description: Some("Latest news coverage".to_string()),
            }],
        );
        data.insert(
            "ESPN.us".to_string(),
            vec![Programme {
                start: now - chrono::Duration::minutes(30),
                stop: now + chrono::Duration::minutes(90),
                title: "Monday Night Football".to_string(),
                description: None,
            }],
        );
        Epg { data }
    }

    #[test]
    fn test_epg_search_finds_match() {
        let playlist = Arc::new(Playlist {
            channels: vec![
                make_test_channel("CNN", Some("CNN.us")),
                make_test_channel("ESPN", Some("ESPN.us")),
                make_test_channel("No EPG", None),
            ],
            groups: vec![Arc::from("Test")],
            group_indices: HashMap::new(),
        });

        let epg = Some(Arc::new(make_test_epg()));
        let engine = EpgSearchEngine::new(playlist, epg);

        let now = Utc::now();
        engine.send_query(
            "football",
            now - chrono::Duration::hours(2),
            now + chrono::Duration::hours(2),
        );

        // Wait for result
        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = engine.try_recv();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.query, "football");
        assert!(result.channel_indices.contains(&1)); // ESPN
        assert!(!result.channel_indices.contains(&0)); // not CNN
    }

    #[test]
    fn test_epg_search_empty_query() {
        let playlist = Arc::new(Playlist {
            channels: vec![make_test_channel("CNN", Some("CNN.us"))],
            groups: vec![Arc::from("Test")],
            group_indices: HashMap::new(),
        });

        let epg = Some(Arc::new(make_test_epg()));
        let engine = EpgSearchEngine::new(playlist, epg);

        let now = Utc::now();
        engine.send_query("", now, now + chrono::Duration::hours(1));

        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = engine.try_recv().unwrap();
        assert!(result.channel_indices.is_empty());
    }

    #[test]
    fn test_epg_search_no_epg() {
        let playlist = Arc::new(Playlist {
            channels: vec![make_test_channel("CNN", Some("CNN.us"))],
            groups: vec![Arc::from("Test")],
            group_indices: HashMap::new(),
        });

        let engine = EpgSearchEngine::new(playlist, None);

        let now = Utc::now();
        engine.send_query("news", now, now + chrono::Duration::hours(1));

        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = engine.try_recv().unwrap();
        assert!(result.channel_indices.is_empty());
    }

    #[test]
    fn test_epg_search_case_insensitive() {
        let playlist = Arc::new(Playlist {
            channels: vec![make_test_channel("CNN", Some("CNN.us"))],
            groups: vec![Arc::from("Test")],
            group_indices: HashMap::new(),
        });

        let epg = Some(Arc::new(make_test_epg()));
        let engine = EpgSearchEngine::new(playlist, epg);

        let now = Utc::now();
        engine.send_query(
            "BREAKING NEWS",
            now - chrono::Duration::hours(2),
            now + chrono::Duration::hours(2),
        );

        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = engine.try_recv().unwrap();
        assert!(result.channel_indices.contains(&0));
    }
}
