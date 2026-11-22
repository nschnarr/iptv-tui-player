use crate::model::Playlist;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use std::sync::{mpsc, Arc};
use std::thread;

const MAX_RESULTS: usize = 500;

enum SearchCommand {
    Query(String),
    Refresh(Vec<Arc<str>>),
}

pub struct SearchEngine {
    cmd_tx: mpsc::SyncSender<SearchCommand>,
    result_rx: mpsc::Receiver<SearchResult>,
}

pub struct SearchResult {
    pub query: String,
    pub indices: Vec<usize>,
}

impl SearchEngine {
    pub fn new(playlist: &Playlist) -> Self {
        let channel_names: Vec<Arc<str>> =
            playlist.channels.iter().map(|c| c.name.clone()).collect();

        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<SearchCommand>(4);
        let (result_tx, result_rx) = mpsc::channel::<SearchResult>();

        thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                let mut nucleo: Nucleo<u32> =
                    Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1);

                inject_names(&mut nucleo, &channel_names);
                drop(channel_names); // free the vec, nucleo owns the data now

                let mut last_query = String::new();

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        SearchCommand::Refresh(new_names) => {
                            // Reuse the same nucleo instance — restart clears items
                            // and disconnects old injectors without destroying the threadpool
                            nucleo.restart(true);
                            inject_names(&mut nucleo, &new_names);
                            last_query.clear();
                            // Send empty result to clear UI
                            let _ = result_tx.send(SearchResult {
                                query: String::new(),
                                indices: vec![],
                            });
                        }
                        SearchCommand::Query(mut query) => {
                            // Drain to latest query
                            while let Ok(cmd) = cmd_rx.try_recv() {
                                match cmd {
                                    SearchCommand::Query(newer) => query = newer,
                                    SearchCommand::Refresh(new_names) => {
                                        nucleo.restart(true);
                                        inject_names(&mut nucleo, &new_names);
                                        last_query.clear();
                                    }
                                }
                            }

                            if query == last_query {
                                continue;
                            }
                            last_query = query.clone();

                            if query.is_empty() {
                                let _ = result_tx.send(SearchResult {
                                    query,
                                    indices: vec![],
                                });
                                continue;
                            }

                            nucleo.pattern.reparse(
                                0,
                                &query,
                                CaseMatching::Ignore,
                                Normalization::Smart,
                                false,
                            );

                            loop {
                                let status = nucleo.tick(10);
                                if !status.running {
                                    break;
                                }
                            }

                            let snapshot = nucleo.snapshot();
                            let matched = snapshot.matched_item_count() as usize;
                            let take = MAX_RESULTS.min(matched) as u32;

                            let indices: Vec<usize> = snapshot
                                .matched_items(..take)
                                .map(|item| *item.data as usize)
                                .collect();

                            let _ = result_tx.send(SearchResult { query, indices });
                        }
                    }
                }
            })
            .expect("failed to spawn search thread");

        Self { cmd_tx, result_rx }
    }

    /// Refresh the search index with new channel names (reuses nucleo threadpool).
    pub fn refresh(&self, playlist: &Playlist) {
        let names: Vec<Arc<str>> = playlist.channels.iter().map(|c| c.name.clone()).collect();
        let _ = self.cmd_tx.send(SearchCommand::Refresh(names));
    }

    pub fn send_query(&self, query: &str) {
        let _ = self
            .cmd_tx
            .try_send(SearchCommand::Query(query.to_string()));
    }

    pub fn try_recv(&self) -> Option<SearchResult> {
        self.result_rx.try_recv().ok()
    }
}

fn inject_names(nucleo: &mut Nucleo<u32>, names: &[Arc<str>]) {
    let injector = nucleo.injector();
    for (i, name) in names.iter().enumerate() {
        let name = name.clone();
        injector.push(i as u32, move |_data, cols| {
            cols[0] = name.as_ref().into();
        });
    }
    // Tick until injection is complete
    loop {
        let status = nucleo.tick(10);
        if !status.running {
            break;
        }
    }
}
