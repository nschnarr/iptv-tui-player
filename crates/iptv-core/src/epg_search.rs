use crate::epg::{Epg, Programme};
use crate::model::Playlist;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::{mpsc, Arc};
use std::thread;

/// A guide entry matching a show search, with the EPG channel (tvg-id) it airs on.
pub struct ShowMatch<'a> {
    pub tvg_id: &'a str,
    pub programme: &'a Programme,
}

/// Find shows by title: every word of `query` must start a word in the title or the
/// description (case-insensitive), so "masters golf" finds "The Masters" described
/// as golf, and "nfl" doesn't match "conflict". Only shows still on at `now` and
/// starting before `until` (if given) count, so `until = now` means "on now".
/// Results: shows with every word in the title first, then description-only
/// matches; within each, airing now first, then by start time. At most `limit`.
pub fn search_programmes<'a>(
    epg: &'a Epg,
    query: &str,
    now: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    limit: usize,
) -> Vec<ShowMatch<'a>> {
    let query = query.to_lowercase();
    let words: Vec<&str> = words_of(&query).collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut matches: Vec<(bool, ShowMatch<'a>)> = Vec::new();
    for (tvg_id, programmes) in &epg.data {
        for p in programmes {
            if p.stop <= now || until.is_some_and(|u| p.start > u) {
                continue;
            }
            let title = p.title.to_lowercase();
            let desc = p.description.as_deref().unwrap_or("").to_lowercase();
            if words
                .iter()
                .all(|w| has_word(&title, w) || has_word(&desc, w))
            {
                matches.push((
                    words.iter().all(|w| has_word(&title, w)),
                    ShowMatch {
                        tvg_id,
                        programme: p,
                    },
                ));
            }
        }
    }

    // Airing now sorts as starting at `now`, so everything on air comes first.
    matches.sort_by(|(a_title, a), (b_title, b)| {
        let start = |m: &ShowMatch| m.programme.start.max(now);
        b_title
            .cmp(a_title)
            .then(start(a).cmp(&start(b)))
            .then(a.programme.title.cmp(&b.programme.title))
            .then(a.tvg_id.cmp(b.tvg_id))
    });
    matches.truncate(limit);
    matches.into_iter().map(|(_, m)| m).collect()
}

/// Words of `text`: runs of letters and digits ("NFL Live: 2026" -> nfl, live, 2026
/// once lowercased).
fn words_of(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Whether some word of `text` starts with `word`.
fn has_word(text: &str, word: &str) -> bool {
    words_of(text).any(|t| t.starts_with(word))
}

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

    fn show(title: &str, desc: Option<&str>, start_h: i64, stop_h: i64) -> Programme {
        let now = Utc::now();
        Programme {
            title: title.to_string(),
            start: now + chrono::Duration::hours(start_h),
            stop: now + chrono::Duration::hours(stop_h),
            description: desc.map(str::to_string),
        }
    }

    fn shows_epg() -> Epg {
        let mut data = HashMap::new();
        data.insert(
            "GOLF.us".to_string(),
            vec![
                show("Golf Central", None, -3, -2), // already over
                show("The Masters", Some("Final round of the golf major"), 2, 6),
                show("Golf Central", None, -1, 1), // airing now
            ],
        );
        data.insert(
            "CBS.us".to_string(),
            vec![show("The Masters Preview", None, 1, 2)],
        );
        Epg { data }
    }

    #[test]
    fn search_programmes_matches_every_word_in_title_or_description() {
        let epg = shows_epg();
        let hits = search_programmes(&epg, "masters golf", Utc::now(), None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].programme.title, "The Masters");
        assert_eq!(hits[0].tvg_id, "GOLF.us");
    }

    #[test]
    fn search_programmes_skips_finished_and_puts_live_first() {
        let epg = shows_epg();
        let titles: Vec<String> = search_programmes(&epg, "GOLF", Utc::now(), None, 10)
            .iter()
            .map(|m| m.programme.title.clone())
            .collect();
        // the finished Golf Central is gone; the live one leads, then by start time
        assert_eq!(titles, ["Golf Central", "The Masters"]);
    }

    #[test]
    fn search_programmes_orders_by_start_and_limits() {
        let epg = shows_epg();
        let hits = search_programmes(&epg, "masters", Utc::now(), None, 10);
        let titles: Vec<&str> = hits.iter().map(|m| m.programme.title.as_str()).collect();
        assert_eq!(titles, ["The Masters Preview", "The Masters"]);
        assert_eq!(
            search_programmes(&epg, "masters", Utc::now(), None, 1).len(),
            1
        );
    }

    #[test]
    fn search_programmes_ranks_title_matches_before_description_matches() {
        let mut epg = shows_epg();
        epg.data.insert(
            "NEWS.us".to_string(),
            vec![show("Sports Desk", Some("Previewing the Masters"), 0, 1)],
        );
        let titles: Vec<String> = search_programmes(&epg, "masters", Utc::now(), None, 10)
            .iter()
            .map(|m| m.programme.title.clone())
            .collect();
        assert_eq!(
            titles,
            ["The Masters Preview", "The Masters", "Sports Desk"]
        );
    }

    #[test]
    fn search_programmes_matches_word_starts_only() {
        let mut data = HashMap::new();
        data.insert(
            "X.us".to_string(),
            vec![
                show("Capturados: Conflictos", None, 0, 1),
                show("NFL Live", None, 1, 2),
                show("Golfers Weekly", None, 2, 3),
            ],
        );
        let epg = Epg { data };
        let titles = |q| -> Vec<String> {
            search_programmes(&epg, q, Utc::now(), None, 10)
                .iter()
                .map(|m| m.programme.title.clone())
                .collect()
        };
        assert_eq!(titles("nfl"), ["NFL Live"]);
        assert_eq!(titles("golf"), ["Golfers Weekly"]);
    }

    #[test]
    fn search_programmes_limits_to_a_time_window() {
        let epg = shows_epg();
        let now = Utc::now();
        let titles = |until| -> Vec<String> {
            search_programmes(&epg, "golf", now, until, 10)
                .iter()
                .map(|m| m.programme.title.clone())
                .collect()
        };
        // on now: the live Golf Central only; The Masters starts in 2h
        assert_eq!(titles(Some(now)), ["Golf Central"]);
        assert_eq!(
            titles(Some(now + chrono::Duration::hours(3))),
            ["Golf Central", "The Masters"]
        );
    }

    #[test]
    fn search_programmes_empty_query_finds_nothing() {
        assert!(search_programmes(&shows_epg(), "   ", Utc::now(), None, 10).is_empty());
    }
}
