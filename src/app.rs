use crate::downloader::{DownloadItem, DownloadManager};
use crate::epg::Epg;
use crate::epg_search::EpgSearchEngine;
use crate::favorites;
use crate::hdhr::HdhrState;
use crate::model::Playlist;
use crate::log::AppLog;
use crate::player::{self, Player};
use crate::recorder::{Recorder, RecordingStatus};
use crate::search::SearchEngine;
use crate::series::{self, Series};
use crate::timezone;
use chrono::{Duration, Utc};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Normal,
    Search,
    Help,
    Guide,
    Downloads,
    Series,
    Recordings,
    SourceInfo,
    Logs,
    Timezone,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GuideViewMode {
    Group,
    Favorites,
    All,
}

pub struct GuideState {
    pub view_mode: GuideViewMode,
    pub time_start: chrono::DateTime<Utc>,
    pub time_span_hours: i64,
    pub selected_channel_idx: usize,
    pub selected_programme_idx: usize,
    pub search_query: String,
    pub search_active: bool,
    /// Channel name fuzzy search results (from async SearchEngine).
    /// Stored as HashSet for O(1) lookups during filtering.
    pub search_results_set: HashSet<usize>,
    /// EPG programme title search results (from async EpgSearchEngine).
    pub epg_search_results: HashSet<usize>,
    /// Cached result of guide_channels(). Rebuilt only when dirty.
    pub cached_channels: Vec<usize>,
    pub channels_dirty: bool,
    /// Group selector popup state.
    pub group_selector_active: bool,
    pub group_selector_idx: usize,
    /// Cached list of group indices that have EPG channels.
    pub epg_groups: Vec<usize>,
}

impl Default for GuideState {
    fn default() -> Self {
        Self {
            view_mode: GuideViewMode::All,
            time_start: Utc::now(),
            time_span_hours: 3,
            selected_channel_idx: 0,
            selected_programme_idx: 0,
            search_query: String::new(),
            search_active: false,
            search_results_set: HashSet::new(),
            epg_search_results: HashSet::new(),
            cached_channels: Vec::new(),
            channels_dirty: true,
            group_selector_active: false,
            group_selector_idx: 0,
            epg_groups: Vec::new(),
        }
    }
}

/// Represents a row in the series tree view.
#[derive(Debug, Clone)]
pub enum SeriesRow {
    /// A series header. series_idx into the series_tree vec.
    SeriesHeader { series_idx: usize },
    /// A season header.
    SeasonHeader { series_idx: usize, season_num: u32 },
    /// An episode row.
    Episode { series_idx: usize, season_num: u32, episode_idx: usize, channel_idx: usize },
}

pub struct SeriesState {
    pub tree: Vec<Series>,
    /// Set of expanded series indices.
    pub expanded_series: HashSet<usize>,
    /// Set of expanded (series_idx, season_num) pairs.
    pub expanded_seasons: HashSet<(usize, u32)>,
    /// Flattened visible rows (rebuilt when expand/collapse changes).
    pub visible_rows: Vec<SeriesRow>,
    pub selected: usize,
    /// Search state.
    pub search_active: bool,
    pub search_query: String,
    /// Indices into tree that match the current search query.
    pub filtered_series: Option<Vec<usize>>,
    /// Whether to show duplicate episodes (same season+episode from different sources).
    pub show_duplicates: bool,
}

impl SeriesState {
    pub fn new(tree: Vec<Series>) -> Self {
        let mut state = Self {
            tree,
            expanded_series: HashSet::new(),
            expanded_seasons: HashSet::new(),
            visible_rows: Vec::new(),
            selected: 0,
            search_active: false,
            search_query: String::new(),
            filtered_series: None,
            show_duplicates: false,
        };
        state.rebuild_rows();
        state
    }

    pub fn rebuild_rows(&mut self) {
        self.visible_rows.clear();
        
        let indices: Vec<usize> = match &self.filtered_series {
            Some(filtered) => filtered.clone(),
            None => (0..self.tree.len()).collect(),
        };
        
        for si in indices {
            self.visible_rows.push(SeriesRow::SeriesHeader { series_idx: si });
            if self.expanded_series.contains(&si) {
                for (&season_num, season) in &self.tree[si].seasons {
                    self.visible_rows.push(SeriesRow::SeasonHeader {
                        series_idx: si,
                        season_num,
                    });
                    if self.expanded_seasons.contains(&(si, season_num)) {
                        for (ei, ep) in season.episodes.iter().enumerate() {
                            if !self.show_duplicates && ep.is_duplicate {
                                continue;
                            }
                            self.visible_rows.push(SeriesRow::Episode {
                                series_idx: si,
                                season_num,
                                episode_idx: ei,
                                channel_idx: ep.channel_idx,
                            });
                        }
                    }
                }
            }
        }
    }

    pub fn update_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_series = None;
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_series = Some(
                self.tree
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.name.to_lowercase().contains(&query))
                    .map(|(i, _)| i)
                    .collect(),
            );
        }
        self.selected = 0;
        self.rebuild_rows();
    }

    pub fn toggle_expand(&mut self) {
        if self.selected >= self.visible_rows.len() {
            return;
        }
        match &self.visible_rows[self.selected] {
            SeriesRow::SeriesHeader { series_idx } => {
                let si = *series_idx;
                if self.expanded_series.contains(&si) {
                    self.expanded_series.remove(&si);
                    // Also collapse all seasons of this series
                    let season_keys: Vec<_> = self.expanded_seasons
                        .iter()
                        .filter(|(s, _)| *s == si)
                        .cloned()
                        .collect();
                    for key in season_keys {
                        self.expanded_seasons.remove(&key);
                    }
                } else {
                    self.expanded_series.insert(si);
                }
                self.rebuild_rows();
            }
            SeriesRow::SeasonHeader { series_idx, season_num } => {
                let key = (*series_idx, *season_num);
                if self.expanded_seasons.contains(&key) {
                    self.expanded_seasons.remove(&key);
                } else {
                    self.expanded_seasons.insert(key);
                }
                self.rebuild_rows();
            }
            SeriesRow::Episode { .. } => {
                // Episodes don't expand
            }
        }
    }

    pub fn collapse(&mut self) {
        if self.selected >= self.visible_rows.len() {
            return;
        }
        match &self.visible_rows[self.selected] {
            SeriesRow::Episode { series_idx, season_num, .. } => {
                // Collapse parent season, move selection to it
                let key = (*series_idx, *season_num);
                self.expanded_seasons.remove(&key);
                self.rebuild_rows();
                // Find the season header
                for (i, row) in self.visible_rows.iter().enumerate() {
                    if let SeriesRow::SeasonHeader { series_idx: si, season_num: sn } = row {
                        if *si == key.0 && *sn == key.1 {
                            self.selected = i;
                            break;
                        }
                    }
                }
            }
            SeriesRow::SeasonHeader { series_idx, season_num } => {
                let key = (*series_idx, *season_num);
                if self.expanded_seasons.contains(&key) {
                    self.expanded_seasons.remove(&key);
                    self.rebuild_rows();
                } else {
                    // Collapse parent series
                    let si = *series_idx;
                    self.expanded_series.remove(&si);
                    let season_keys: Vec<_> = self.expanded_seasons
                        .iter()
                        .filter(|(s, _)| *s == si)
                        .cloned()
                        .collect();
                    for k in season_keys {
                        self.expanded_seasons.remove(&k);
                    }
                    self.rebuild_rows();
                    // Find the series header
                    for (i, row) in self.visible_rows.iter().enumerate() {
                        if let SeriesRow::SeriesHeader { series_idx: s } = row {
                            if *s == si {
                                self.selected = i;
                                break;
                            }
                        }
                    }
                }
            }
            SeriesRow::SeriesHeader { series_idx } => {
                let si = *series_idx;
                if self.expanded_series.contains(&si) {
                    self.expanded_series.remove(&si);
                    let season_keys: Vec<_> = self.expanded_seasons
                        .iter()
                        .filter(|(s, _)| *s == si)
                        .cloned()
                        .collect();
                    for k in season_keys {
                        self.expanded_seasons.remove(&k);
                    }
                    self.rebuild_rows();
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Groups,
    Channels,
}

pub struct App {
    pub playlist: Arc<Playlist>,
    pub selected_group: usize,
    pub selected_channel: usize,
    pub mode: AppMode,
    pub focus: Focus,
    pub search_query: String,
    pub search_results: Vec<usize>,
    pub favorites: HashSet<String>,
    pub show_favorites_only: bool,
    pub epg: Option<Arc<Epg>>,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub search_pending: bool,
    pub guide_state: GuideState,
    search_engine: SearchEngine,
    pub epg_search_engine: Option<EpgSearchEngine>,
    pub download_manager: DownloadManager,
    pub downloads_selected: usize,
    pub series_state: Option<SeriesState>,
    pub hdhr_state: Option<Arc<Mutex<HdhrState>>>,
    /// Cached indices of favorited channels. Rebuilt when favorites change.
    cached_favorite_indices: Vec<usize>,
    favorites_dirty: bool,
    /// Receives refreshed playlists from the background refresh thread.
    pub refresh_rx: Option<mpsc::Receiver<(Playlist, Option<Epg>)>>,
    /// Recording manager.
    pub recorder: Recorder,
    pub recordings_selected: usize,
    /// Source info / refresh tracking.
    pub playlist_last_refresh: chrono::DateTime<Utc>,
    pub epg_last_refresh: Option<chrono::DateTime<Utc>>,
    /// EPG source URL for manual refresh.
    pub epg_source: Option<String>,
    /// Trigger channel for manual refresh (send () to request).
    pub refresh_trigger: Option<mpsc::Sender<()>>,
    /// Whether a refresh is currently in progress.
    pub refresh_in_progress: bool,
    /// Receives refreshed EPG from background thread.
    pub epg_refresh_rx: Option<mpsc::Receiver<Epg>>,
    /// Selected media player for launching streams.
    pub selected_player: Player,
    /// Application log.
    pub log: AppLog,
    /// Log viewer scroll position (offset from bottom).
    pub log_scroll: usize,
    /// Current timezone for displaying times.
    pub tz: chrono_tz::Tz,
    /// Current UI theme.
    pub theme: crate::theme::Theme,
    /// Timezone selector state.
    pub tz_list: Vec<chrono_tz::Tz>,
    pub tz_selected: usize,
    pub tz_search: String,
    pub tz_filtered: Vec<usize>,
}

impl App {
    /// Read current process RSS from /proc/self/status (Linux only). Returns MB.
    fn get_rss_mb() -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<f64>().ok())
            })
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    }

    pub fn new(playlist: Playlist, epg: Option<Epg>) -> Self {
        let mut favorites = favorites::load_favorites();

        // Migrate old URL-based favorites to name+group keys
        let channels_for_migration: Vec<(String, String, String)> = playlist
            .channels
            .iter()
            .map(|c| (c.name.to_string(), c.group.to_string(), c.url.to_string()))
            .collect();
        if let Some(migrated) = favorites::migrate_if_needed(&favorites, &channels_for_migration) {
            eprintln!("Migrated {} favorites from URL-based to name-based format", migrated.len());
            favorites = migrated;
            let _ = favorites::save_favorites(&favorites);
        }

        eprintln!("Loaded {} favorites", favorites.len());

        // Wrap in Arc — single shared copy for app, search engines, HDHR
        let playlist = Arc::new(playlist);
        let epg = epg.map(Arc::new);

        let search_engine = SearchEngine::new(&playlist);

        // Create EPG search engine if EPG data is available
        let epg_search_engine = if epg.is_some() {
            Some(EpgSearchEngine::new(Arc::clone(&playlist), epg.as_ref().map(Arc::clone)))
        } else {
            None
        };

        // Download directory: ~/Downloads/iptv/
        let download_dir = dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("iptv");
        let download_manager = DownloadManager::new(download_dir.clone());

        // Recording directory: ~/Downloads/iptv/recordings/
        let recording_dir = download_dir.join("recordings");
        let recorder = Recorder::new(recording_dir);

        Self {
            playlist,
            selected_group: 0,
            selected_channel: 0,
            mode: AppMode::Normal,
            focus: Focus::Groups,
            search_query: String::new(),
            search_results: Vec::new(),
            favorites,
            show_favorites_only: false,
            epg,
            status_message: None,
            should_quit: false,
            search_pending: false,
            guide_state: GuideState::default(),
            search_engine,
            epg_search_engine,
            download_manager,
            downloads_selected: 0,
            series_state: None,
            hdhr_state: None,
            cached_favorite_indices: Vec::new(),
            favorites_dirty: true,
            refresh_rx: None,
            recorder,
            recordings_selected: 0,
            playlist_last_refresh: Utc::now(),
            epg_last_refresh: None,
            epg_source: None,
            refresh_trigger: None,
            refresh_in_progress: false,
            epg_refresh_rx: None,
            selected_player: Player::Vlc,
            log: AppLog::new(),
            log_scroll: 0,
            tz: timezone::load_timezone(),
            tz_list: timezone::common_timezones(),
            tz_selected: 0,
            tz_search: String::new(),
            tz_filtered: Vec::new(),
            theme: crate::theme::by_name(&crate::theme::load_theme_name()),
        }
    }

    /// Set a status message and also log it.
    #[allow(dead_code)]
    fn set_status(&mut self, source: &str, msg: String) {
        self.log.info(source, &msg);
        self.status_message = Some(msg);
    }

    /// Hot-swap the playlist with a freshly fetched one.
    /// Preserves favorites, rebuilds search engine and all caches.
    pub fn refresh_playlist(&mut self, new_playlist: Playlist) {
        let old_count = self.playlist.channels.len();
        self.playlist_last_refresh = Utc::now();
        self.refresh_in_progress = false;

        let rss_before = Self::get_rss_mb();
        let old_playlist_refs = std::sync::Arc::strong_count(&self.playlist);
        let old_epg_refs = self.epg.as_ref().map(std::sync::Arc::strong_count).unwrap_or(0);
        self.log.info("refresh-mem", format!(
            "STEP 0 (before): RSS={:.0}MB | old playlist refs={} ({} ch) | old EPG refs={}",
            rss_before, old_playlist_refs, old_count, old_epg_refs
        ));

        // Drop old EPG search engine to release old Arc references
        self.epg_search_engine = None;

        let rss_after_epg_drop = Self::get_rss_mb();
        let refs_after_drop = std::sync::Arc::strong_count(&self.playlist);
        self.log.info("refresh-mem", format!(
            "STEP 1 (drop epg_search_engine): RSS={:.0}MB (delta={:+.0}) | old playlist refs={}",
            rss_after_epg_drop, rss_after_epg_drop - rss_before, refs_after_drop
        ));

        // Wrap new playlist in Arc — shared by app, search engines, HDHR (ZERO deep clones)
        let new_arc = Arc::new(new_playlist);

        // Refresh search engine in-place (reuses nucleo threadpool, no new threads)
        self.search_engine.refresh(&new_arc);

        let rss_after_search = Self::get_rss_mb();
        self.log.info("refresh-mem", format!(
            "STEP 2 (search refresh): RSS={:.0}MB (delta={:+.0}) | new playlist refs={} | old playlist refs={}",
            rss_after_search, rss_after_search - rss_before,
            std::sync::Arc::strong_count(&new_arc),
            std::sync::Arc::strong_count(&self.playlist)
        ));

        // Rebuild EPG search engine if EPG exists
        if self.epg.is_some() {
            self.epg_search_engine = Some(EpgSearchEngine::new(
                Arc::clone(&new_arc),
                self.epg.as_ref().map(Arc::clone),
            ));
            let rss_after_epg_rebuild = Self::get_rss_mb();
            self.log.info("refresh-mem", format!(
                "STEP 3 (epg search rebuild): RSS={:.0}MB (delta={:+.0}) | new playlist refs={} | EPG refs={}",
                rss_after_epg_rebuild, rss_after_epg_rebuild - rss_before,
                std::sync::Arc::strong_count(&new_arc),
                self.epg.as_ref().map(std::sync::Arc::strong_count).unwrap_or(0)
            ));
        }

        // Update HDHR state if active
        if let Some(ref hdhr_state) = self.hdhr_state {
            let mut state = hdhr_state.lock().unwrap();
            state.playlist = Arc::clone(&new_arc);
        }

        // Swap — old Arc dropped (freed if no other refs)
        let old_playlist_strong = std::sync::Arc::strong_count(&self.playlist);
        self.playlist = new_arc;

        let rss_after_swap = Self::get_rss_mb();
        self.log.info("refresh-mem", format!(
            "STEP 4 (swap playlist): RSS={:.0}MB (delta={:+.0}) | old playlist had {} refs before swap | new playlist refs={}",
            rss_after_swap, rss_after_swap - rss_before,
            old_playlist_strong,
            std::sync::Arc::strong_count(&self.playlist)
        ));

        // Reset selections to safe values
        if self.selected_group >= self.playlist.groups.len() {
            self.selected_group = 0;
        }
        self.selected_channel = 0;
        self.favorites_dirty = true;
        self.guide_state.channels_dirty = true;

        // Close series browser if open (tree is stale)
        if self.mode == AppMode::Series {
            self.series_state = None;
            self.mode = AppMode::Normal;
        }

        let msg = format!(
            "Playlist refreshed: {} channels (was {})",
            self.playlist.channels.len(),
            old_count
        );
        self.log.info("refresh", &msg);
        self.status_message = Some(msg);
    }

    /// Rebuild the cached favorite indices list.
    pub fn ensure_favorite_cache(&mut self) {
        if !self.favorites_dirty {
            return;
        }
        self.cached_favorite_indices = (0..self.playlist.channels.len())
            .filter(|&i| {
                let ch = &self.playlist.channels[i];
                let key = favorites::favorite_key(&ch.name, &ch.group);
                self.favorites.contains(&key)
            })
            .collect();
        self.favorites_dirty = false;
    }

    pub fn current_channel_indices(&self) -> Vec<usize> {
        // Show search results in both Search mode (typing) and Normal mode (locked results)
        if !self.search_query.is_empty() && !self.search_results.is_empty() {
            return self.search_results.clone();
        }

        if self.show_favorites_only {
            // Use cached favorite indices
            return self.cached_favorite_indices.clone();
        }

        if self.playlist.groups.is_empty() {
            (0..self.playlist.channels.len()).collect::<Vec<_>>()
        } else if self.selected_group < self.playlist.groups.len() {
            let group = &self.playlist.groups[self.selected_group];
            self.playlist.channels_in_group(group).to_vec()
        } else {
            Vec::new()
        }
    }

    pub fn selected_channel_url(&self) -> Option<&str> {
        let indices = self.current_channel_indices();
        if self.selected_channel < indices.len() {
            Some(self.playlist.channels[indices[self.selected_channel]].url.as_ref())
        } else {
            None
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Focus::Groups => {
                if self.selected_group > 0 {
                    self.selected_group -= 1;
                    self.selected_channel = 0;
                }
            }
            Focus::Channels => {
                if self.selected_channel > 0 {
                    self.selected_channel -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            Focus::Groups => {
                if self.selected_group + 1 < self.playlist.groups.len() {
                    self.selected_group += 1;
                    self.selected_channel = 0;
                }
            }
            Focus::Channels => {
                let count = self.current_channel_indices().len();
                if count > 0 && self.selected_channel + 1 < count {
                    self.selected_channel += 1;
                }
            }
        }
    }

    pub fn jump_top(&mut self) {
        match self.focus {
            Focus::Groups => {
                self.selected_group = 0;
                self.selected_channel = 0;
            }
            Focus::Channels => {
                self.selected_channel = 0;
            }
        }
    }

    pub fn jump_bottom(&mut self) {
        match self.focus {
            Focus::Groups => {
                if !self.playlist.groups.is_empty() {
                    self.selected_group = self.playlist.groups.len() - 1;
                    self.selected_channel = 0;
                }
            }
            Focus::Channels => {
                let count = self.current_channel_indices().len();
                if count > 0 {
                    self.selected_channel = count - 1;
                }
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Groups => Focus::Channels,
            Focus::Channels => Focus::Groups,
        };
    }

    pub fn enter_search(&mut self) {
        self.mode = AppMode::Search;
        self.search_query.clear();
        self.search_results.clear();
        self.selected_channel = 0;
        self.focus = Focus::Channels;
    }

    pub fn exit_search(&mut self) {
        self.mode = AppMode::Normal;
        self.search_query.clear();
        self.search_results.clear();
        self.selected_channel = 0;
    }

    pub fn update_search(&mut self) {
        if self.search_query.is_empty() {
            self.search_results.clear();
            self.search_pending = false;
            self.selected_channel = 0;
        } else {
            self.search_engine.send_query(&self.search_query);
            self.search_pending = true;
        }
    }

    pub fn poll_search_results(&mut self) -> bool {
        let mut updated = false;

        // Poll channel name search results
        if let Some(result) = self.search_engine.try_recv() {
            if self.mode == AppMode::Guide {
                if result.query == self.guide_state.search_query {
                    // Store as HashSet for O(1) lookups
                    self.guide_state.search_results_set =
                        result.indices.into_iter().collect();
                    self.guide_state.selected_channel_idx = 0;
                    self.guide_state.channels_dirty = true;
                    updated = true;
                }
            } else if result.query == self.search_query {
                self.search_results = result.indices;
                self.search_pending = false;
                self.selected_channel = 0;
                updated = true;
            }
        }

        // Poll EPG programme search results (guide mode only)
        if self.mode == AppMode::Guide {
            if let Some(ref epg_engine) = self.epg_search_engine {
                if let Some(result) = epg_engine.try_recv() {
                    if result.query == self.guide_state.search_query {
                        self.guide_state.epg_search_results = result.channel_indices;
                        self.guide_state.channels_dirty = true;
                        updated = true;
                    }
                }
            }
        }

        updated
    }

    pub fn toggle_favorite(&mut self) {
        let indices = self.current_channel_indices();
        if self.selected_channel < indices.len() {
            let ch = &self.playlist.channels[indices[self.selected_channel]];
            let key = favorites::favorite_key(&ch.name, &ch.group);
            if self.favorites.contains(&key) {
                self.favorites.remove(&key);
                self.status_message = Some("Removed from favorites".to_string());
            } else {
                self.favorites.insert(key);
                self.status_message = Some("Added to favorites".to_string());
            }
            let _ = favorites::save_favorites(&self.favorites);
            self.favorites_dirty = true;
            self.sync_hdhr_favorites();
        }
    }

    pub fn toggle_favorites_filter(&mut self) {
        self.show_favorites_only = !self.show_favorites_only;
        self.selected_channel = 0;
        self.status_message = Some(if self.show_favorites_only {
            "Showing favorites only".to_string()
        } else {
            "Showing all channels".to_string()
        });
    }

    pub fn launch_selected(&mut self) {
        if let Some(url) = self.selected_channel_url() {
            let url = url.to_string();
            match player::launch(self.selected_player, &url, &self.log) {
                Ok(_) => self.status_message = Some(format!("Launching {}...", self.selected_player)),
                Err(e) => self.status_message = Some(format!("{} error: {e}", self.selected_player)),
            }
        }
    }

    pub fn enter_logs(&mut self) {
        self.log_scroll = 0;
        self.mode = AppMode::Logs;
    }

    pub fn exit_logs(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn log_scroll_up(&mut self) {
        let total = self.log.entry_count();
        if self.log_scroll < total.saturating_sub(1) {
            self.log_scroll += 1;
        }
    }

    pub fn log_scroll_down(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    pub fn log_scroll_page_up(&mut self) {
        let total = self.log.entry_count();
        self.log_scroll = (self.log_scroll + 20).min(total.saturating_sub(1));
    }

    pub fn log_scroll_page_down(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(20);
    }

    pub fn cycle_player(&mut self) {
        self.selected_player = self.selected_player.next();
        self.status_message = Some(format!("Player: {}", self.selected_player));
    }

    pub fn is_favorite(&self, channel_idx: usize) -> bool {
        let ch = &self.playlist.channels[channel_idx];
        let key = favorites::favorite_key(&ch.name, &ch.group);
        self.favorites.contains(&key)
    }

    /// Sync favorites to the HDHR server if running.
    pub fn sync_hdhr_favorites(&self) {
        if let Some(ref hdhr) = self.hdhr_state {
            if let Ok(mut state) = hdhr.lock() {
                state.favorites = self.favorites.clone();
            }
        }
    }

    // --- Guide methods ---

    pub fn enter_guide(&mut self) {
        self.mode = AppMode::Guide;
        self.guide_state.time_start = Utc::now();
        self.guide_state.selected_channel_idx = 0;
        self.guide_state.selected_programme_idx = 0;
        self.guide_state.search_query.clear();
        self.guide_state.search_active = false;
        self.guide_state.search_results_set.clear();
        self.guide_state.epg_search_results.clear();
        self.guide_state.channels_dirty = true;
        self.guide_state.group_selector_active = false;
        self.guide_state.group_selector_idx = 0;

        // Build list of group indices that have at least one EPG channel
        self.guide_state.epg_groups = (0..self.playlist.groups.len())
            .filter(|&gi| {
                let group = &self.playlist.groups[gi];
                self.playlist.channels_in_group(group).iter().any(|&ci| {
                    if let Some(ref epg) = self.epg {
                        if let Some(tvg_id) = self.playlist.channels[ci].tvg_id.as_deref() {
                            return epg.data.contains_key(tvg_id);
                        }
                    }
                    false
                })
            })
            .collect();
    }

    pub fn exit_guide(&mut self) {
        self.mode = AppMode::Normal;
    }

    /// Ensure guide channel cache is up to date. Call before drawing.
    pub fn ensure_guide_channels(&mut self) {
        if self.guide_state.channels_dirty {
            self.rebuild_guide_channels();
            self.guide_state.channels_dirty = false;
        }
    }

    /// Returns cached guide channel indices (read-only, for rendering).
    /// Call ensure_guide_channels() first to rebuild if dirty.
    pub fn guide_channels(&self) -> &[usize] {
        &self.guide_state.cached_channels
    }

    /// Rebuild the cached guide channels list.
    /// Only includes channels that have EPG data (tvg_id matching a known EPG channel).
    fn rebuild_guide_channels(&mut self) {
        let has_epg = |i: &usize| -> bool {
            if let Some(ref epg) = self.epg {
                if let Some(tvg_id) = self.playlist.channels[*i].tvg_id.as_deref() {
                    return epg.data.contains_key(tvg_id);
                }
            }
            false
        };

        let base_indices: Vec<usize> = match self.guide_state.view_mode {
            GuideViewMode::Group => {
                if self.selected_group < self.playlist.groups.len() {
                    let group = &self.playlist.groups[self.selected_group];
                    self.playlist.channels_in_group(group)
                        .iter()
                        .copied()
                        .filter(|i| has_epg(i))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            GuideViewMode::Favorites => (0..self.playlist.channels.len())
                .filter(|i| has_epg(i) && self.is_favorite(*i))
                .collect(),
            GuideViewMode::All => (0..self.playlist.channels.len())
                .filter(|i| has_epg(i))
                .collect(),
        };

        self.guide_state.cached_channels =
            if self.guide_state.search_active && !self.guide_state.search_query.is_empty() {
                // Both sets use HashSet for O(1) lookups
                base_indices
                    .into_iter()
                    .filter(|&i| {
                        self.guide_state.search_results_set.contains(&i)
                            || self.guide_state.epg_search_results.contains(&i)
                    })
                    .collect()
            } else {
                base_indices
            };
    }

    pub fn guide_update_search(&mut self) {
        if self.guide_state.search_query.is_empty() {
            self.guide_state.search_results_set.clear();
            self.guide_state.epg_search_results.clear();
        } else {
            // Async channel name search
            self.search_engine
                .send_query(&self.guide_state.search_query);

            // Async EPG programme search
            if let Some(ref epg_engine) = self.epg_search_engine {
                let time_end = self.guide_state.time_start
                    + Duration::hours(self.guide_state.time_span_hours);
                epg_engine.send_query(
                    &self.guide_state.search_query,
                    self.guide_state.time_start,
                    time_end,
                );
            }
        }
        self.guide_state.channels_dirty = true;
    }

    pub fn guide_move_up(&mut self) {
        if self.guide_state.selected_channel_idx > 0 {
            self.guide_state.selected_channel_idx -= 1;
            self.guide_state.selected_programme_idx = 0;
        }
    }

    pub fn guide_move_down(&mut self) {
        let count = self.guide_state.cached_channels.len();
        if count > 0 && self.guide_state.selected_channel_idx + 1 < count {
            self.guide_state.selected_channel_idx += 1;
            self.guide_state.selected_programme_idx = 0;
        }
    }

    pub fn guide_move_left(&mut self) {
        if self.guide_state.selected_programme_idx > 0 {
            self.guide_state.selected_programme_idx -= 1;
        }
    }

    pub fn guide_move_right(&mut self) {
        // Will be bounded during rendering
        self.guide_state.selected_programme_idx += 1;
    }

    pub fn guide_shift_time_left(&mut self) {
        self.guide_state.time_start -= Duration::hours(1);
    }

    pub fn guide_shift_time_right(&mut self) {
        self.guide_state.time_start += Duration::hours(1);
    }

    pub fn guide_switch_view(&mut self, mode: GuideViewMode) {
        self.guide_state.view_mode = mode;
        self.guide_state.selected_channel_idx = 0;
        self.guide_state.selected_programme_idx = 0;
        self.guide_state.channels_dirty = true;
    }

    pub fn guide_open_group_selector(&mut self) {
        self.guide_state.group_selector_active = true;
        // Pre-select current group if in Group view mode
        if self.guide_state.view_mode == GuideViewMode::Group {
            if let Some(pos) = self.guide_state.epg_groups.iter().position(|&gi| gi == self.selected_group) {
                self.guide_state.group_selector_idx = pos;
            } else {
                self.guide_state.group_selector_idx = 0;
            }
        } else {
            self.guide_state.group_selector_idx = 0;
        }
    }

    pub fn guide_group_selector_up(&mut self) {
        if self.guide_state.group_selector_idx > 0 {
            self.guide_state.group_selector_idx -= 1;
        }
    }

    pub fn guide_group_selector_down(&mut self) {
        if self.guide_state.group_selector_idx + 1 < self.guide_state.epg_groups.len() {
            self.guide_state.group_selector_idx += 1;
        }
    }

    pub fn guide_group_selector_select(&mut self) {
        if self.guide_state.group_selector_idx < self.guide_state.epg_groups.len() {
            let group_idx = self.guide_state.epg_groups[self.guide_state.group_selector_idx];
            self.selected_group = group_idx;
            self.guide_state.view_mode = GuideViewMode::Group;
            self.guide_state.selected_channel_idx = 0;
            self.guide_state.selected_programme_idx = 0;
            self.guide_state.channels_dirty = true;
        }
        self.guide_state.group_selector_active = false;
    }

    pub fn guide_group_selector_cancel(&mut self) {
        self.guide_state.group_selector_active = false;
    }

    pub fn guide_launch_selected(&mut self) {
        let idx = self.guide_state.selected_channel_idx;
        if idx < self.guide_state.cached_channels.len() {
            let channel_idx = self.guide_state.cached_channels[idx];
            let url = self.playlist.channels[channel_idx].url.to_string();
            match player::launch(self.selected_player, &url, &self.log) {
                Ok(_) => self.status_message = Some(format!("Launching {}...", self.selected_player)),
                Err(e) => self.status_message = Some(format!("{} error: {e}", self.selected_player)),
            }
        }
    }

    pub fn guide_toggle_favorite(&mut self) {
        let idx = self.guide_state.selected_channel_idx;
        if idx < self.guide_state.cached_channels.len() {
            let channel_idx = self.guide_state.cached_channels[idx];
            let ch = &self.playlist.channels[channel_idx];
            let key = favorites::favorite_key(&ch.name, &ch.group);
            if self.favorites.contains(&key) {
                self.favorites.remove(&key);
                self.status_message = Some("Removed from favorites".to_string());
            } else {
                self.favorites.insert(key);
                self.status_message = Some("Added to favorites".to_string());
            }
            let _ = favorites::save_favorites(&self.favorites);
            self.favorites_dirty = true;
            self.sync_hdhr_favorites();
            self.guide_state.channels_dirty = true;
        }
    }

    // --- Download methods ---

    /// Queue the currently selected channel for download.
    pub fn download_selected(&mut self) {
        let indices = self.current_channel_indices();
        if self.selected_channel < indices.len() {
            let ch_idx = indices[self.selected_channel];
            self.enqueue_download(ch_idx);
        }
    }

    /// Queue a channel by index for download.
    pub fn enqueue_download(&mut self, channel_idx: usize) {
        let ch = &self.playlist.channels[channel_idx];
        let item = DownloadItem {
            name: ch.name.to_string(),
            url: ch.url.to_string(),
            content_type: ch.content_type.clone(),
            group: ch.group.to_string(),
        };
        self.download_manager.enqueue(item);
        self.status_message = Some(format!("Queued: {}", ch.name));
    }

    /// Queue the selected channel in the guide for download.
    pub fn guide_download_selected(&mut self) {
        let idx = self.guide_state.selected_channel_idx;
        if idx < self.guide_state.cached_channels.len() {
            let channel_idx = self.guide_state.cached_channels[idx];
            self.enqueue_download(channel_idx);
        }
    }

    pub fn enter_downloads(&mut self) {
        self.mode = AppMode::Downloads;
        self.downloads_selected = 0;
    }

    pub fn exit_downloads(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn downloads_move_up(&mut self) {
        if self.downloads_selected > 0 {
            self.downloads_selected -= 1;
        }
    }

    pub fn downloads_move_down(&mut self) {
        let count = self.download_manager.snapshot().len();
        if count > 0 && self.downloads_selected + 1 < count {
            self.downloads_selected += 1;
        }
    }

    pub fn downloads_remove_selected(&mut self) {
        self.download_manager.remove(self.downloads_selected);
        let count = self.download_manager.snapshot().len();
        if self.downloads_selected >= count && count > 0 {
            self.downloads_selected = count - 1;
        }
    }

    // --- Series browser methods ---

    pub fn enter_series(&mut self) {
        let tree = series::build_series_tree(&self.playlist);
        if tree.is_empty() {
            self.status_message = Some("No series found in playlist".to_string());
            return;
        }
        self.series_state = Some(SeriesState::new(tree));
        self.mode = AppMode::Series;
    }

    pub fn series_toggle_duplicates(&mut self) {
        if let Some(ref mut state) = self.series_state {
            state.show_duplicates = !state.show_duplicates;
            state.rebuild_rows();
            if state.selected >= state.visible_rows.len() {
                state.selected = state.visible_rows.len().saturating_sub(1);
            }
            self.status_message = Some(if state.show_duplicates {
                "Showing all sources (duplicates visible)".to_string()
            } else {
                "Hiding duplicates (one per episode)".to_string()
            });
        }
    }

    pub fn exit_series(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn series_move_up(&mut self) {
        if let Some(ref mut state) = self.series_state {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
    }

    pub fn series_move_down(&mut self) {
        if let Some(ref mut state) = self.series_state {
            if state.selected + 1 < state.visible_rows.len() {
                state.selected += 1;
            }
        }
    }

    pub fn series_toggle_expand(&mut self) {
        if let Some(ref mut state) = self.series_state {
            state.toggle_expand();
        }
    }

    pub fn series_collapse(&mut self) {
        if let Some(ref mut state) = self.series_state {
            state.collapse();
        }
    }

    pub fn series_download_selected(&mut self) {
        let items_to_queue: Vec<usize> = {
            let state = match &self.series_state {
                Some(s) => s,
                None => return,
            };
            if state.selected >= state.visible_rows.len() {
                return;
            }
            match &state.visible_rows[state.selected] {
                SeriesRow::Episode { channel_idx, .. } => {
                    vec![*channel_idx]
                }
                SeriesRow::SeasonHeader { series_idx, season_num } => {
                    // Download unique episodes in this season (skip duplicates)
                    let series = &state.tree[*series_idx];
                    if let Some(season) = series.seasons.get(season_num) {
                        season.episodes.iter().filter(|e| !e.is_duplicate).map(|e| e.channel_idx).collect()
                    } else {
                        Vec::new()
                    }
                }
                SeriesRow::SeriesHeader { series_idx } => {
                    // Download unique episodes in this series (skip duplicates)
                    let series = &state.tree[*series_idx];
                    series.seasons.values()
                        .flat_map(|s| s.episodes.iter().filter(|e| !e.is_duplicate).map(|e| e.channel_idx))
                        .collect()
                }
            }
        };

        let count = items_to_queue.len();
        for ch_idx in items_to_queue {
            self.enqueue_download(ch_idx);
        }
        if count == 1 {
            self.status_message = Some("Queued 1 episode".to_string());
        } else {
            self.status_message = Some(format!("Queued {} episodes", count));
        }
    }

    pub fn series_launch_selected(&mut self) {
        let channel_idx = {
            let state = match &self.series_state {
                Some(s) => s,
                None => return,
            };
            if state.selected >= state.visible_rows.len() {
                return;
            }
            match &state.visible_rows[state.selected] {
                SeriesRow::Episode { channel_idx, .. } => Some(*channel_idx),
                _ => None,
            }
        };
        if let Some(ch_idx) = channel_idx {
            let url = self.playlist.channels[ch_idx].url.to_string();
            match player::launch(self.selected_player, &url, &self.log) {
                Ok(_) => self.status_message = Some(format!("Launching {}...", self.selected_player)),
                Err(e) => self.status_message = Some(format!("{} error: {e}", self.selected_player)),
            }
        }
    }

    // --- Source info methods ---

    pub fn enter_source_info(&mut self) {
        self.mode = AppMode::SourceInfo;
    }

    pub fn exit_source_info(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn trigger_playlist_refresh(&mut self) {
        if self.refresh_in_progress {
            self.status_message = Some("Refresh already in progress...".to_string());
            return;
        }
        if let Some(ref tx) = self.refresh_trigger {
            self.refresh_in_progress = true;
            let _ = tx.send(());
            self.status_message = Some("Refreshing playlist...".to_string());
        } else {
            self.status_message = Some("No refresh source configured (need --url or --xtream)".to_string());
        }
    }

    pub fn trigger_epg_refresh(&mut self) {
        if let Some(ref source) = self.epg_source.clone() {
            self.status_message = Some("Refreshing EPG...".to_string());
            // Fetch EPG in a background thread
            let source = source.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("epg-refresh".into())
                .spawn(move || {
                    let result = if source.starts_with("http://") || source.starts_with("https://") {
                        crate::epg::Epg::parse_xmltv_url(&source)
                    } else {
                        crate::epg::Epg::parse_xmltv_file(std::path::Path::new(&source))
                    };
                    let _ = tx.send(result);
                })
                .ok();
            // Check for result (will be picked up in event loop via polling)
            // For simplicity, block briefly then check
            if let Ok(Ok(new_epg)) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
                self.epg_last_refresh = Some(Utc::now());
                let epg_arc = Arc::new(new_epg);
                // Update HDHR state
                if let Some(ref hdhr_state) = self.hdhr_state {
                    let mut state = hdhr_state.lock().unwrap();
                    state.epg = Some(Arc::clone(&epg_arc));
                }
                // Rebuild EPG search engine — zero clones, just Arc bumps
                self.epg_search_engine = Some(EpgSearchEngine::new(
                    Arc::clone(&self.playlist),
                    Some(Arc::clone(&epg_arc)),
                ));
                let prog_count = epg_arc.programme_count();
                let ch_count = epg_arc.channel_count();
                self.epg = Some(epg_arc);
                self.guide_state.channels_dirty = true;
                self.status_message = Some(format!(
                    "EPG refreshed: {} programmes for {} channels",
                    prog_count, ch_count
                ));
            } else {
                self.status_message = Some("EPG refresh failed or timed out".to_string());
            }
        } else {
            self.status_message = Some("No EPG source configured (need --epg)".to_string());
        }
    }

    // --- Recording methods ---

    pub fn enter_recordings(&mut self) {
        self.mode = AppMode::Recordings;
        self.recordings_selected = 0;
    }

    pub fn exit_recordings(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn recordings_move_up(&mut self) {
        if self.recordings_selected > 0 {
            self.recordings_selected -= 1;
        }
    }

    pub fn recordings_move_down(&mut self) {
        let count = self.recorder.snapshot().len();
        if count > 0 && self.recordings_selected + 1 < count {
            self.recordings_selected += 1;
        }
    }

    pub fn recordings_cancel_selected(&mut self) {
        let recs = self.recorder.snapshot();
        if self.recordings_selected < recs.len() {
            let id = recs[self.recordings_selected].id;
            match recs[self.recordings_selected].status {
                RecordingStatus::Scheduled | RecordingStatus::Recording => {
                    self.recorder.cancel(id);
                    self.status_message = Some("Recording cancelled".to_string());
                }
                RecordingStatus::Complete | RecordingStatus::Failed { .. } | RecordingStatus::Cancelled => {
                    self.recorder.remove_finished(id);
                }
            }
        }
    }

    /// Record the currently selected channel (immediate, 60 min default).
    pub fn record_selected_channel(&mut self) {
        let indices = self.current_channel_indices();
        if self.selected_channel >= indices.len() {
            return;
        }
        let ch_idx = indices[self.selected_channel];
        let ch = &self.playlist.channels[ch_idx];
        let id = self.recorder.record_now(
            ch.name.to_string(),
            ch.url.to_string(),
            60,
        );
        self.status_message = Some(format!("Recording started: {} (60 min, #{})", ch.name, id));
    }

    /// Record/schedule from the EPG guide. If the programme is current, starts now.
    /// If future, schedules it.
    pub fn guide_record_selected(&mut self) {
        let idx = self.guide_state.selected_channel_idx;
        if idx >= self.guide_state.cached_channels.len() {
            return;
        }
        let channel_idx = self.guide_state.cached_channels[idx];
        let ch = &self.playlist.channels[channel_idx];
        let url = ch.url.to_string();
        let channel_name = ch.name.to_string();

        // Find the selected programme
        if let Some(ref epg) = self.epg {
            if let Some(tvg_id) = ch.tvg_id.as_deref() {
                if let Some(programmes) = epg.data.get(tvg_id) {
                    let time_start = self.guide_state.time_start;
                    let time_end = time_start + Duration::hours(self.guide_state.time_span_hours);
                    let visible: Vec<_> = programmes.iter()
                        .filter(|p| p.stop > time_start && p.start < time_end)
                        .collect();

                    if let Some(prog) = visible.get(self.guide_state.selected_programme_idx) {
                        let id = self.recorder.schedule(
                            channel_name.clone(),
                            prog.title.clone(),
                            url,
                            prog.start,
                            prog.stop,
                        );
                        let now = Utc::now();
                        if prog.start <= now {
                            self.status_message = Some(format!("Recording now: {} (#{}) ", prog.title, id));
                        } else {
                            let starts_in = prog.start - now;
                            let mins = starts_in.num_minutes();
                            self.status_message = Some(format!(
                                "Scheduled: {} in {}h{}m (#{}) ",
                                prog.title,
                                mins / 60,
                                mins % 60,
                                id,
                            ));
                        }
                        return;
                    }
                }
            }
        }

        // No programme found — record channel for 60 min
        let id = self.recorder.record_now(channel_name.clone(), url, 60);
        self.status_message = Some(format!("Recording now: {} (60 min, #{})", channel_name, id));
    }

    // --- Timezone selector methods ---

    pub fn enter_timezone(&mut self) {
        self.mode = AppMode::Timezone;
        self.tz_search.clear();
        self.tz_filtered.clear();
        
        // Find current timezone in list
        self.tz_selected = self.tz_list
            .iter()
            .position(|&tz| tz == self.tz)
            .unwrap_or(0);
        
        // If not in common list, use all timezones
        if self.tz_list.iter().all(|&tz| tz != self.tz) {
            self.tz_list = timezone::all_timezones();
            self.tz_selected = self.tz_list
                .iter()
                .position(|&tz| tz == self.tz)
                .unwrap_or(0);
        }
    }

    pub fn exit_timezone(&mut self) {
        self.mode = AppMode::Normal;
        // Reset to common timezones for next time
        self.tz_list = timezone::common_timezones();
    }

    pub fn tz_move_up(&mut self) {
        let list_len = if self.tz_filtered.is_empty() {
            self.tz_list.len()
        } else {
            self.tz_filtered.len()
        };
        
        if list_len > 0 && self.tz_selected > 0 {
            self.tz_selected -= 1;
        }
    }

    pub fn tz_move_down(&mut self) {
        let list_len = if self.tz_filtered.is_empty() {
            self.tz_list.len()
        } else {
            self.tz_filtered.len()
        };
        
        if list_len > 0 && self.tz_selected + 1 < list_len {
            self.tz_selected += 1;
        }
    }

    pub fn tz_select(&mut self) {
        let tz = if self.tz_filtered.is_empty() {
            if self.tz_selected < self.tz_list.len() {
                self.tz_list[self.tz_selected]
            } else {
                return;
            }
        } else if self.tz_selected < self.tz_filtered.len() {
            let real_idx = self.tz_filtered[self.tz_selected];
            if real_idx < self.tz_list.len() {
                self.tz_list[real_idx]
            } else {
                return;
            }
        } else {
            return;
        };
        
        self.tz = tz;
        if let Err(e) = timezone::save_timezone(tz) {
            self.status_message = Some(format!("Failed to save timezone: {}", e));
        } else {
            self.status_message = Some(format!("Timezone set to: {}", tz.name()));
        }
        self.exit_timezone();
    }

    pub fn tz_update_search(&mut self, query: String) {
        self.tz_search = query;
        self.tz_filtered.clear();
        self.tz_selected = 0;
        
        if self.tz_search.is_empty() {
            return;
        }
        
        let query_lower = self.tz_search.to_lowercase();
        for (i, tz) in self.tz_list.iter().enumerate() {
            if tz.name().to_lowercase().contains(&query_lower) {
                self.tz_filtered.push(i);
            }
        }
    }
}
