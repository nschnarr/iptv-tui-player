use crate::app::{App, AppMode, Focus, GuideViewMode, SeriesRow};
use crate::downloader::DownloadStatus;
use crate::log::LogLevel;
use crate::recorder::RecordingStatus;
use chrono::{Duration, Timelike, Utc};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(f.area());

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(chunks[0]);

    draw_groups(f, app, main_chunks[0]);
    draw_channels(f, app, main_chunks[1]);
    draw_status_bar(f, app, chunks[1]);

    if app.mode == AppMode::Help {
        draw_help_overlay(f, app);
    }

    if app.mode == AppMode::Guide {
        draw_guide_overlay(f, app);
    }

    if app.mode == AppMode::Downloads {
        draw_downloads_overlay(f, app);
    }

    if app.mode == AppMode::Series {
        draw_series_overlay(f, app);
    }

    if app.mode == AppMode::Recordings {
        draw_recordings_overlay(f, app);
    }

    if app.mode == AppMode::SourceInfo {
        draw_source_info_overlay(f, app);
    }

    if app.mode == AppMode::Logs {
        draw_logs_overlay(f, app);
    }

    if app.mode == AppMode::Timezone {
        draw_timezone_overlay(f, app);
    }
}

fn draw_groups(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.mode == AppMode::Search {
        " Search Results "
    } else {
        " Groups "
    };

    let border_style = if app.focus == Focus::Groups && app.mode == AppMode::Normal {
        Style::default().fg(app.theme.accent)
    } else {
        Style::default().fg(app.theme.text_subtle)
    };

    let items: Vec<ListItem> = app
        .playlist
        .groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let count = app.playlist.channels_in_group(group).len();
            let text = format!("  {} ({count})", group);
            let style = if i == app.selected_group && app.mode != AppMode::Search {
                Style::default()
                    .fg(app.theme.highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let mut state = ListState::default();
    if app.mode != AppMode::Search {
        state.select(Some(app.selected_group));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut state);
}

fn draw_channels(f: &mut Frame, app: &App, area: Rect) {
    let indices = app.current_channel_indices();

    let border_style = if app.focus == Focus::Channels || app.mode == AppMode::Search {
        Style::default().fg(app.theme.accent)
    } else {
        Style::default().fg(app.theme.text_subtle)
    };

    let title = format!(" Channels ({}) ", indices.len());

    let items: Vec<ListItem> = indices
        .iter()
        .enumerate()
        .map(|(display_idx, &ch_idx)| {
            let ch = &app.playlist.channels[ch_idx];
            let star = if app.is_favorite(ch_idx) { " ★" } else { "" };
            let num = display_idx + 1;

            let style = if display_idx == app.selected_channel {
                Style::default()
                    .fg(app.theme.success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Build lines: channel name + optional EPG info
            let title_line = Line::from(format!("  {num}. {}{star}", ch.name));

            let mut lines = vec![title_line];

            if let Some(ref epg) = app.epg {
                if let Some(tvg_id) = ch.tvg_id.as_deref() {
                    if let Some(prog) = epg.now_playing(tvg_id) {
                        let until = crate::epg::format_time_tz(&prog.stop, &app.tz);
                        let epg_line = Line::from(vec![
                            Span::raw("     "),
                            Span::styled(
                                format!("▶ {} (until {until})", prog.title),
                                Style::default().fg(app.theme.text_dim),
                            ),
                        ]);
                        lines.push(epg_line);
                    }
                }
            }

            ListItem::new(lines).style(style)
        })
        .collect();

    let mut state = ListState::default();
    if !indices.is_empty() {
        state.select(Some(app.selected_channel));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut state);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (left, right) = match &app.mode {
        AppMode::Search => {
            let search_text = if app.search_pending {
                format!(" 🔍 {} (searching...)", app.search_query)
            } else {
                format!(" 🔍 {}_", app.search_query)
            };
            let count = format!("{} results ", app.search_results.len());
            (search_text, count)
        }
        _ => {
            let left = if !app.search_query.is_empty() && !app.search_results.is_empty() {
                format!(
                    " 🔍 {} ({} results) — Enter play  s fav  Esc clear  / new search",
                    app.search_query,
                    app.search_results.len()
                )
            } else if let Some(msg) = &app.status_message {
                format!(" {msg}")
            } else {
                " [/] search  [d] download  [D] queue  [?] help  [q] quit".to_string()
            };
            let pending = app.download_manager.pending_count();
            let dl_indicator = if pending > 0 {
                format!("⬇ {} ", pending)
            } else {
                String::new()
            };
            let right = format!(
                "{}{}  {} channels  [{}] ",
                dl_indicator,
                if app.show_favorites_only { "★ " } else { "" },
                app.playlist.channels.len(),
                app.selected_player,
            );
            (left, right)
        }
    };

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::default().fg(app.theme.text)),
        Span::raw("  "),
        Span::styled(right, Style::default().fg(app.theme.text_subtle)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.text_subtle)),
    );

    f.render_widget(bar, area);
}

fn draw_help_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 70, f.area());
    f.render_widget(Clear, area);

    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  IPTV TUI - Key Bindings",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  j/k or ↑/↓     Navigate lists"),
        Line::from("  h/l or ←/→     Switch focus (groups/channels)"),
        Line::from("  g               Jump to top"),
        Line::from("  G               Open TV Guide"),
        Line::from("  /               Enter search mode"),
        Line::from("  Esc             Exit search / close help"),
        Line::from("  Enter           Launch stream with selected player"),
        Line::from("  P               Switch player (VLC/mpv)"),
        Line::from("  d / Ctrl+d      Download selected channel"),
        Line::from("  D               Open download queue"),
        Line::from("  S               Series browser (tree view)"),
        Line::from("  r               Record selected channel (60 min)"),
        Line::from("  R               Open recordings list"),
        Line::from("  i               Source info (refresh playlist/EPG)"),
        Line::from("  s or Space      Star/unstar channel"),
        Line::from("  f               Toggle favorites filter"),
        Line::from("  L               Open log viewer"),
        Line::from("  t               Cycle theme"),
        Line::from("  T               Timezone selector"),
        Line::from("  ?               Toggle this help"),
        Line::from("  q               Quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  Guide (G)",
            Style::default().fg(app.theme.highlight),
        )),
        Line::from("  r               Record selected programme"),
        Line::from("                  (schedules future, starts now if current)"),
        Line::from(""),
    ];

    let help = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help ")
            .style(Style::default().bg(app.theme.surface))
            .border_style(Style::default().fg(app.theme.accent)),
    );

    f.render_widget(help, area);
}

fn draw_guide_overlay(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Clear, area);

    let channel_col_width: u16 = 18;
    let view_label = match app.guide_state.view_mode {
        GuideViewMode::Group => {
            if app.selected_group < app.playlist.groups.len() {
                format!("Group: {}", app.playlist.groups[app.selected_group])
            } else {
                "Group".to_string()
            }
        }
        GuideViewMode::Favorites => "Favorites".to_string(),
        GuideViewMode::All => "All Channels".to_string(),
    };
    let title = format!(" TV GUIDE: {} ", view_label);

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(app.theme.accent));

    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    if inner.height < 4 || inner.width < channel_col_width + 10 {
        return;
    }

    // Layout: time header (1 row), now-marker (1 row), channels, status bar (1-2 rows)
    let status_height = 1;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),             // time header
            Constraint::Length(1),             // now marker
            Constraint::Min(1),                // channel rows
            Constraint::Length(status_height), // status
        ])
        .split(inner);

    let time_header_area = chunks[0];
    let marker_area = chunks[1];
    let channels_area = chunks[2];
    let status_area = chunks[3];

    let time_start = app.guide_state.time_start;
    let time_end = time_start + Duration::hours(app.guide_state.time_span_hours);
    let grid_width = inner.width.saturating_sub(channel_col_width) as f64;
    let total_minutes = (app.guide_state.time_span_hours * 60) as f64;

    // Render time header
    let buf = f.buffer_mut();
    let grid_x = inner.x + channel_col_width;

    // Draw 30-min ticks
    {
        // Round start to nearest 30 min
        let mut tick = time_start;
        let mins = tick.minute();
        let round_to = if mins < 30 { 30 - mins } else { 60 - mins };
        if !mins.is_multiple_of(30) {
            tick += Duration::minutes(round_to as i64);
        }

        while tick < time_end {
            let offset_min = (tick - time_start).num_minutes() as f64;
            let x = grid_x + (offset_min / total_minutes * grid_width) as u16;
            let label = tick.with_timezone(&app.tz).format("%H:%M").to_string();
            // Add 1 space padding so labels don't overlap with the channel column separator
            let render_x = x + 1;
            if render_x >= grid_x && render_x + label.len() as u16 <= inner.x + inner.width {
                buf.set_string(
                    render_x,
                    time_header_area.y,
                    &label,
                    Style::default().fg(app.theme.highlight),
                );
            }
            tick += Duration::minutes(30);
        }
    }

    // Draw current time marker
    let now = Utc::now();
    if now >= time_start && now < time_end {
        let offset_min = (now - time_start).num_minutes() as f64;
        let x = grid_x + (offset_min / total_minutes * grid_width) as u16;
        if x < inner.x + inner.width {
            buf.set_string(x, marker_area.y, "▼", Style::default().fg(app.theme.error));
            let now_label = now.with_timezone(&app.tz).format("%H:%M").to_string();
            if x + 1 + now_label.len() as u16 <= inner.x + inner.width {
                buf.set_string(
                    x + 1,
                    marker_area.y,
                    &now_label,
                    Style::default().fg(app.theme.error),
                );
            }
        }
    }

    // Draw channel rows
    let guide_channels = app.guide_channels();
    let visible_rows = channels_area.height as usize;

    // Scroll offset
    let scroll_offset = if app.guide_state.selected_channel_idx >= visible_rows {
        app.guide_state.selected_channel_idx - visible_rows + 1
    } else {
        0
    };

    for row in 0..visible_rows {
        let ch_list_idx = scroll_offset + row;
        if ch_list_idx >= guide_channels.len() {
            break;
        }
        let ch_idx = guide_channels[ch_list_idx];
        let ch = &app.playlist.channels[ch_idx];
        let y = channels_area.y + row as u16;

        let is_selected = ch_list_idx == app.guide_state.selected_channel_idx;
        let name_style = if is_selected {
            Style::default()
                .fg(app.theme.success)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text)
        };

        // Channel name (truncated)
        let name: String = ch
            .name
            .chars()
            .take((channel_col_width - 2) as usize)
            .collect();
        let star = if app.is_favorite(ch_idx) { "★" } else { "" };
        let display_name = format!("{}{}", name, star);
        buf.set_string(inner.x, y, &display_name, name_style);

        // Separator
        buf.set_string(
            inner.x + channel_col_width - 1,
            y,
            "│",
            Style::default().fg(app.theme.text_dim),
        );

        // Programme blocks
        if let Some(ref epg) = app.epg {
            if let Some(tvg_id) = ch.tvg_id.as_deref() {
                let progs = epg.programmes_in_range(tvg_id, time_start, time_end);

                // Clamp selected_programme_idx for this channel
                let max_prog_idx = if progs.is_empty() { 0 } else { progs.len() - 1 };

                for (pidx, prog) in progs.iter().enumerate() {
                    let prog_start_min = (prog.start - time_start).num_minutes().max(0) as f64;
                    let prog_end_min =
                        ((prog.stop - time_start).num_minutes() as f64).min(total_minutes);
                    let px_start = grid_x + (prog_start_min / total_minutes * grid_width) as u16;
                    let px_end = grid_x + (prog_end_min / total_minutes * grid_width) as u16;
                    let block_width = px_end.saturating_sub(px_start);

                    if block_width == 0 {
                        continue;
                    }

                    let is_prog_selected = is_selected
                        && pidx == app.guide_state.selected_programme_idx.min(max_prog_idx);

                    let is_now_playing = now >= prog.start && now < prog.stop;

                    let style = if is_prog_selected {
                        Style::default().bg(app.theme.info).fg(app.theme.text)
                    } else if is_now_playing {
                        Style::default()
                            .bg(app.theme.selected_bg)
                            .fg(app.theme.text)
                    } else {
                        Style::default().fg(app.theme.text_subtle)
                    };

                    // Duration
                    let dur_min = (prog.stop - prog.start).num_minutes();
                    let label = if block_width as usize > prog.title.len() + 6 {
                        format!("{} ({}m)", prog.title, dur_min)
                    } else {
                        let avail = block_width as usize;
                        if avail > 3 {
                            let truncated: String = prog.title.chars().take(avail - 1).collect();
                            truncated
                        } else {
                            prog.title.chars().take(avail).collect()
                        }
                    };

                    // Draw separator between programmes first
                    if px_start > grid_x && px_start < inner.x + inner.width {
                        buf.set_string(px_start, y, "│", Style::default().fg(app.theme.text_dim));
                    }

                    // Fill the block background (skip separator position)
                    let label_start = if px_start > grid_x {
                        px_start + 1
                    } else {
                        px_start
                    };
                    for x in label_start..px_end.min(inner.x + inner.width) {
                        buf.set_string(x, y, " ", style);
                    }
                    // Write label (start after separator)
                    let label_chars: Vec<char> = label.chars().collect();
                    let max_chars =
                        block_width.saturating_sub(if px_start > grid_x { 1 } else { 0 }) as usize;
                    for (i, &c) in label_chars.iter().take(max_chars).enumerate() {
                        let x = label_start + i as u16;
                        if x < inner.x + inner.width {
                            buf.set_string(x, y, c.to_string(), style);
                        }
                    }
                }

                if progs.is_empty() {
                    buf.set_string(grid_x, y, "—", Style::default().fg(app.theme.text_dim));
                }
            } else {
                buf.set_string(
                    grid_x,
                    y,
                    "— no EPG ID",
                    Style::default().fg(app.theme.text_dim),
                );
            }
        } else {
            buf.set_string(
                grid_x,
                y,
                "— no EPG loaded",
                Style::default().fg(app.theme.text_dim),
            );
        }
    }

    // Status bar
    let status_text = if app.guide_state.search_active {
        format!(
            " 🔍 {}_ | g/f/a views  [/] ←→ scroll  Esc close",
            app.guide_state.search_query
        )
    } else if let Some(ref msg) = app.status_message {
        format!(" {} | g/f/a views  [/] ←→ scroll  Esc close", msg)
    } else {
        " / search  g/f/a views  []/<> ←→ scroll  s fav  r record  Enter play  Esc close"
            .to_string()
    };

    buf.set_string(
        status_area.x,
        status_area.y,
        &status_text,
        Style::default().fg(app.theme.text_dim),
    );

    // Group selector popup
    if app.guide_state.group_selector_active {
        draw_group_selector(f, app);
    }
}

fn draw_series_overlay(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Clear, area);

    let state = match &app.series_state {
        Some(s) => s,
        None => return,
    };

    let items: Vec<ListItem> = state
        .visible_rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (text, style) = match row {
                SeriesRow::SeriesHeader { series_idx } => {
                    let series = &state.tree[*series_idx];
                    let expanded = state.expanded_series.contains(series_idx);
                    let arrow = if expanded { "▼" } else { "▶" };
                    let season_count = series.seasons.len();
                    let unique_count = series.unique_episode_count();
                    let dupe_count = series.duplicate_count();
                    let ep_str = if state.show_duplicates && dupe_count > 0 {
                        format!("{} ep, {} dupes", series.episode_count(), dupe_count)
                    } else if dupe_count > 0 {
                        format!("{} ep (+{} hidden)", unique_count, dupe_count)
                    } else {
                        format!("{} ep", unique_count)
                    };
                    let text = format!(
                        "{} {} ({} season{}, {})",
                        arrow,
                        series.name,
                        season_count,
                        if season_count == 1 { "" } else { "s" },
                        ep_str,
                    );
                    let style = Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD);
                    (text, style)
                }
                SeriesRow::SeasonHeader {
                    series_idx,
                    season_num,
                } => {
                    let expanded = state.expanded_seasons.contains(&(*series_idx, *season_num));
                    let arrow = if expanded { "▼" } else { "▶" };
                    let series = &state.tree[*series_idx];
                    let season = series.seasons.get(season_num);
                    let unique = season.map(|s| s.unique_episode_count()).unwrap_or(0);
                    let total = season.map(|s| s.episodes.len()).unwrap_or(0);
                    let dupes = total - unique;
                    let ep_str = if state.show_duplicates && dupes > 0 {
                        format!(
                            "{} episode{}, {} dupe{}",
                            total,
                            if total == 1 { "" } else { "s" },
                            dupes,
                            if dupes == 1 { "" } else { "s" }
                        )
                    } else if dupes > 0 {
                        format!(
                            "{} episode{} (+{} hidden)",
                            unique,
                            if unique == 1 { "" } else { "s" },
                            dupes
                        )
                    } else {
                        format!("{} episode{}", unique, if unique == 1 { "" } else { "s" })
                    };
                    let text = format!("   {} Season {} ({})", arrow, season_num, ep_str,);
                    let style = Style::default()
                        .fg(app.theme.highlight)
                        .add_modifier(Modifier::BOLD);
                    (text, style)
                }
                SeriesRow::Episode {
                    series_idx,
                    season_num,
                    episode_idx,
                    ..
                } => {
                    let series = &state.tree[*series_idx];
                    let ep = &series.seasons[season_num].episodes[*episode_idx];
                    let ch = &app.playlist.channels[ep.channel_idx];
                    let text = if state.show_duplicates {
                        let info = build_source_info(ch);
                        let label = if ep.is_duplicate { "dupe" } else { "primary" };
                        format!(
                            "      S{:02}E{:02}  {} [{}:{}]",
                            ep.season, ep.episode, ep.name, label, info
                        )
                    } else {
                        format!("      S{:02}E{:02}  {}", ep.season, ep.episode, ep.name)
                    };
                    let style = if ep.is_duplicate {
                        Style::default().fg(app.theme.text_dim)
                    } else {
                        Style::default().fg(app.theme.text)
                    };
                    (text, style)
                }
            };

            let final_style = if i == state.selected {
                style.bg(app.theme.selected_bg)
            } else {
                style
            };

            ListItem::new(text).style(final_style)
        })
        .collect();

    let mut list_state = ListState::default();
    if !state.visible_rows.is_empty() {
        list_state.select(Some(state.selected));
    }

    let visible_count = state
        .filtered_series
        .as_ref()
        .map_or(state.tree.len(), |f| f.len());
    let total_count = state.tree.len();
    let title = if state.filtered_series.is_some() {
        format!(" Series Browser ({}/{} shows) ", visible_count, total_count)
    } else {
        format!(" Series Browser ({} shows) ", total_count)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(app.theme.accent)),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, chunks[0], &mut list_state);

    // Status bar
    let hint = if state.search_active {
        format!(" 🔍 {}_ ", state.search_query)
    } else {
        match state.visible_rows.get(state.selected) {
            Some(SeriesRow::SeriesHeader { .. }) => format!(
                " / search  Enter expand  d download all  u {}dupes  Esc close",
                if state.show_duplicates {
                    "hide "
                } else {
                    "show "
                }
            ),
            Some(SeriesRow::SeasonHeader { .. }) => format!(
                " / search  Enter expand  d download season  u {}dupes  Esc close",
                if state.show_duplicates {
                    "hide "
                } else {
                    "show "
                }
            ),
            Some(SeriesRow::Episode { .. }) => format!(
                " / search  Enter play  d download  u {}dupes  h collapse  Esc close",
                if state.show_duplicates {
                    "hide "
                } else {
                    "show "
                }
            ),
            None => " / search  Esc close".to_string(),
        }
    };
    let buf = f.buffer_mut();
    buf.set_string(
        chunks[1].x,
        chunks[1].y,
        &hint,
        Style::default().fg(app.theme.text_dim),
    );
}

fn draw_downloads_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 80, f.area());
    f.render_widget(Clear, area);

    let queue = app.download_manager.snapshot();

    // Inner width for progress bars (subtract borders)
    let inner_width = area.width.saturating_sub(4) as usize;

    // Count stats for title
    let completed = queue
        .iter()
        .filter(|e| matches!(e.status, DownloadStatus::Complete { .. }))
        .count();
    let active = queue
        .iter()
        .filter(|e| matches!(e.status, DownloadStatus::Downloading { .. }))
        .count();

    let items: Vec<ListItem> = queue
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == app.downloads_selected;
            let bg = if selected {
                app.theme.selected_bg
            } else {
                app.theme.surface
            };

            match &entry.status {
                DownloadStatus::Queued => {
                    let line1 = Line::from(vec![
                        Span::styled("  -- ", Style::default().fg(app.theme.text_dim).bg(bg)),
                        Span::styled(&entry.item.name, Style::default().fg(app.theme.text).bg(bg)),
                        Span::styled("  queued", Style::default().fg(app.theme.text_dim).bg(bg)),
                    ]);
                    ListItem::new(line1)
                }
                DownloadStatus::Downloading {
                    bytes_downloaded,
                    total_bytes,
                    bytes_per_sec,
                } => {
                    let dl_mb = *bytes_downloaded as f64 / 1_048_576.0;
                    let speed_str = match bytes_per_sec {
                        Some(bps) if *bps > 1_048_576 => {
                            format!("{:.1} MB/s", *bps as f64 / 1_048_576.0)
                        }
                        Some(bps) if *bps > 0 => format!("{:.0} KB/s", *bps as f64 / 1024.0),
                        _ => "".to_string(),
                    };

                    let (pct, size_str) = if let Some(total) = total_bytes {
                        let total_mb = *total as f64 / 1_048_576.0;
                        let p = if *total > 0 {
                            *bytes_downloaded as f64 / *total as f64
                        } else {
                            0.0
                        };
                        (p, format!("{:.1}/{:.1} MB", dl_mb, total_mb))
                    } else {
                        (0.0, format!("{:.1} MB", dl_mb))
                    };

                    let pct_str = format!("{:>3.0}%", pct * 100.0);

                    // Line 1: name + speed to the right
                    let speed_display = if speed_str.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", speed_str)
                    };
                    let line1 = Line::from(vec![
                        Span::styled("  ", Style::default().bg(bg)),
                        Span::styled(
                            &entry.item.name,
                            Style::default().fg(app.theme.highlight).bg(bg),
                        ),
                        Span::styled(
                            speed_display,
                            Style::default().fg(app.theme.highlight).bg(bg),
                        ),
                    ]);

                    // Line 2: full-width progress bar
                    let bar_width = inner_width.saturating_sub(2);
                    let filled = (pct * bar_width as f64).round() as usize;
                    let empty = bar_width.saturating_sub(filled);
                    let bar_filled_str: String = "━".repeat(filled);
                    let bar_empty_str: String = "─".repeat(empty);

                    let line2 = Line::from(vec![
                        Span::styled("  ", Style::default().bg(bg)),
                        Span::styled(
                            bar_filled_str,
                            Style::default().fg(app.theme.progress).bg(bg),
                        ),
                        Span::styled(
                            bar_empty_str,
                            Style::default().fg(app.theme.progress_empty).bg(bg),
                        ),
                    ]);

                    let line3 = Line::from(vec![Span::styled(
                        format!("  {} {}", pct_str, size_str),
                        Style::default().fg(app.theme.highlight).bg(bg),
                    )]);

                    ListItem::new(vec![line1, line2, line3])
                }
                DownloadStatus::Complete { path } => {
                    let fname = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let bar_str: String = "━".repeat(inner_width.saturating_sub(2));
                    let line1 = Line::from(vec![
                        Span::styled("  ", Style::default().bg(bg)),
                        Span::styled(
                            &entry.item.name,
                            Style::default().fg(app.theme.success).bg(bg),
                        ),
                    ]);
                    let line2 = Line::from(vec![
                        Span::styled("  ", Style::default().bg(bg)),
                        Span::styled(bar_str, Style::default().fg(app.theme.success).bg(bg)),
                    ]);
                    let line3 = Line::from(vec![Span::styled(
                        format!("  {}", fname),
                        Style::default().fg(app.theme.text_dim).bg(bg),
                    )]);
                    ListItem::new(vec![line1, line2, line3])
                }
                DownloadStatus::Failed { error } => {
                    let line1 = Line::from(vec![
                        Span::styled(
                            " FAIL ",
                            Style::default()
                                .fg(app.theme.error)
                                .bg(bg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            &entry.item.name,
                            Style::default().fg(app.theme.error).bg(bg),
                        ),
                    ]);
                    let line2 = Line::from(vec![Span::styled(
                        format!("  {}", error),
                        Style::default().fg(app.theme.text_dim).bg(bg),
                    )]);
                    ListItem::new(vec![line1, line2])
                }
            }
        })
        .collect();

    let mut state = ListState::default();
    if !queue.is_empty() {
        state.select(Some(app.downloads_selected));
    }

    let title = if active > 0 {
        format!(
            " Downloads ({}/{} complete, {} active) ",
            completed,
            queue.len(),
            active
        )
    } else {
        format!(" Downloads ({}/{} complete) ", completed, queue.len())
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().bg(app.theme.surface))
                .border_style(Style::default().fg(app.theme.accent)),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, &mut state);

    // Status bar at bottom of overlay
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let status_y = area.y + area.height - 1;
    let buf = f.buffer_mut();
    buf.set_string(
        inner.x + 1,
        status_y,
        " x remove  Esc close ",
        Style::default().fg(app.theme.text_dim),
    );
}

fn draw_source_info_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(55, 40, f.area());
    f.render_widget(Clear, area);

    let now = Utc::now();

    let playlist_ago = {
        let d = now - app.playlist_last_refresh;
        if d.num_hours() > 0 {
            format!("{}h {}m ago", d.num_hours(), d.num_minutes() % 60)
        } else if d.num_minutes() > 0 {
            format!("{}m ago", d.num_minutes())
        } else {
            "just now".to_string()
        }
    };

    let epg_ago = match app.epg_last_refresh {
        Some(t) => {
            let d = now - t;
            if d.num_hours() > 0 {
                format!("{}h {}m ago", d.num_hours(), d.num_minutes() % 60)
            } else if d.num_minutes() > 0 {
                format!("{}m ago", d.num_minutes())
            } else {
                "just now".to_string()
            }
        }
        None => "never".to_string(),
    };

    let epg_stats = if let Some(ref epg) = app.epg {
        format!(
            "{} programmes, {} channels",
            epg.programme_count(),
            epg.channel_count()
        )
    } else {
        "not loaded".to_string()
    };

    let playlist_cache = crate::cache::playlist_cache_age()
        .map(|a| format!("cached ({})", a))
        .unwrap_or_else(|| "no cache".to_string());

    let epg_cache = crate::cache::epg_cache_age()
        .map(|a| format!("cached ({})", a))
        .unwrap_or_else(|| "no cache".to_string());

    let refresh_status = if app.refresh_in_progress {
        "  ⟳ Refreshing..."
    } else {
        ""
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Source Information",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Playlist:  ", Style::default().fg(app.theme.accent)),
            Span::styled(
                format!(
                    "{} channels, {} groups",
                    app.playlist.channels.len(),
                    app.playlist.groups.len()
                ),
                Style::default().fg(app.theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Refreshed: ", Style::default().fg(app.theme.accent)),
            Span::styled(&playlist_ago, Style::default().fg(app.theme.highlight)),
        ]),
        Line::from(vec![
            Span::styled("  Cache:     ", Style::default().fg(app.theme.accent)),
            Span::styled(&playlist_cache, Style::default().fg(app.theme.text)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  EPG:       ", Style::default().fg(app.theme.accent)),
            Span::styled(&epg_stats, Style::default().fg(app.theme.text)),
        ]),
        Line::from(vec![
            Span::styled("  Refreshed: ", Style::default().fg(app.theme.accent)),
            Span::styled(&epg_ago, Style::default().fg(app.theme.highlight)),
        ]),
        Line::from(vec![
            Span::styled("  Cache:     ", Style::default().fg(app.theme.accent)),
            Span::styled(&epg_cache, Style::default().fg(app.theme.text)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            refresh_status,
            Style::default().fg(app.theme.accent),
        )),
    ];

    let block = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Source Info ")
            .style(Style::default().bg(app.theme.surface))
            .border_style(Style::default().fg(app.theme.accent)),
    );
    f.render_widget(block, area);

    // Status bar at bottom
    let status_y = area.y + area.height - 1;
    let buf = f.buffer_mut();
    let inner_x = area.x + 2;
    buf.set_string(
        inner_x,
        status_y,
        " p refresh playlist  e refresh EPG  Esc close ",
        Style::default().fg(app.theme.text),
    );
}

fn draw_recordings_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 80, f.area());
    f.render_widget(Clear, area);

    let recs = app.recorder.snapshot();
    let now = Utc::now();

    let active_count = recs
        .iter()
        .filter(|r| r.status == RecordingStatus::Recording)
        .count();
    let scheduled_count = recs
        .iter()
        .filter(|r| r.status == RecordingStatus::Scheduled)
        .count();

    let items: Vec<ListItem> = recs
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let selected = i == app.recordings_selected;
            let bg = if selected {
                app.theme.selected_bg
            } else {
                app.theme.surface
            };

            let (icon, status_text, color) = match &rec.status {
                RecordingStatus::Scheduled => {
                    let starts_in = rec.start - now;
                    let mins = starts_in.num_minutes().max(0);
                    (
                        "⏰",
                        format!("starts in {}h{}m", mins / 60, mins % 60),
                        app.theme.highlight,
                    )
                }
                RecordingStatus::Recording => {
                    let remaining = rec.stop - now;
                    let mins = remaining.num_minutes().max(0);
                    ("🔴", format!("recording ({}m left)", mins), app.theme.error)
                }
                RecordingStatus::Complete => ("✅", "complete".to_string(), app.theme.success),
                RecordingStatus::Failed { error } => ("❌", error.clone(), app.theme.error),
                RecordingStatus::Cancelled => ("⊘", "cancelled".to_string(), app.theme.text_subtle),
            };

            let time_str = format!(
                "{} - {}",
                rec.start.with_timezone(&app.tz).format("%H:%M"),
                rec.stop.with_timezone(&app.tz).format("%H:%M"),
            );

            let line1 = Line::from(vec![
                Span::styled(format!("  {} ", icon), Style::default().fg(color).bg(bg)),
                Span::styled(
                    &rec.programme_title,
                    Style::default()
                        .fg(color)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let line2 = Line::from(vec![Span::styled(
                format!("     {} │ {} │ {}", rec.channel_name, time_str, status_text),
                Style::default().fg(app.theme.text).bg(bg),
            )]);

            ListItem::new(vec![line1, line2])
        })
        .collect();

    let mut state = ListState::default();
    if !recs.is_empty() {
        state.select(Some(app.recordings_selected));
    }

    let title = if active_count > 0 {
        format!(
            " Recordings ({} active, {} scheduled, {} total) ",
            active_count,
            scheduled_count,
            recs.len()
        )
    } else if scheduled_count > 0 {
        format!(
            " Recordings ({} scheduled, {} total) ",
            scheduled_count,
            recs.len()
        )
    } else {
        format!(" Recordings ({}) ", recs.len())
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().bg(app.theme.surface))
                .border_style(Style::default().fg(app.theme.error)),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, &mut state);

    // Status bar
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let status_y = area.y + area.height - 1;
    let buf = f.buffer_mut();
    buf.set_string(
        inner.x + 1,
        status_y,
        " x cancel/remove  Esc close ",
        Style::default().fg(app.theme.text),
    );
}

fn draw_group_selector(f: &mut Frame, app: &App) {
    let area = centered_rect(40, 70, f.area());
    f.render_widget(Clear, area);

    let epg_groups = &app.guide_state.epg_groups;

    let items: Vec<ListItem> = epg_groups
        .iter()
        .enumerate()
        .map(|(i, &group_idx)| {
            let group_name = &app.playlist.groups[group_idx];
            let count = app.playlist.channels_in_group(group_name).len();
            let text = format!("  {} ({count})", group_name);
            let style = if i == app.guide_state.group_selector_idx {
                Style::default()
                    .fg(app.theme.highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let mut state = ListState::default();
    if !epg_groups.is_empty() {
        state.select(Some(app.guide_state.group_selector_idx));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Select Group ({} with EPG) ", epg_groups.len()))
                .style(Style::default().bg(app.theme.surface))
                .border_style(Style::default().fg(app.theme.accent)),
        )
        .highlight_style(
            Style::default()
                .bg(app.theme.selected_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut state);
}

/// Build a rich source info string from all available channel metadata.
fn build_source_info(ch: &crate::model::Channel) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Quality from name/group
    if let Some(q) = detect_quality_tag(&ch.name, &ch.group) {
        parts.push(q);
    }

    // Container format from URL extension
    if let Some(ext) = url_format(&ch.url) {
        parts.push(ext);
    }

    // Language
    if let Some(ref lang) = ch.tvg_language {
        if !lang.is_empty() {
            parts.push(lang.to_string());
        }
    }

    // Country
    if let Some(ref country) = ch.tvg_country {
        if !country.is_empty() {
            parts.push(country.to_string());
        }
    }

    // Group name
    if !ch.group.is_empty() {
        parts.push(ch.group.to_string());
    }

    // Server domain
    let domain = extract_domain(&ch.url);
    parts.push(domain);

    // Port (if non-standard, can hint at different servers)
    if let Some(port) = extract_port(&ch.url) {
        if port != 80 && port != 443 {
            parts.push(format!(":{}", port));
        }
    }

    format!(" {}", parts.join(" │ "))
}

/// Extract file format/container from URL extension
fn url_format(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next()?;
    let ext = filename.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        "ts" => Some("MPEG-TS".to_string()),
        "mp4" => Some("MP4".to_string()),
        "mkv" => Some("MKV".to_string()),
        "m3u8" => Some("HLS".to_string()),
        "flv" => Some("FLV".to_string()),
        "avi" => Some("AVI".to_string()),
        _ => None,
    }
}

/// Extract port from URL
fn extract_port(url: &str) -> Option<u16> {
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host_part = without_scheme.split('/').next()?;
    let port_str = host_part.split(':').nth(1)?;
    port_str.parse().ok()
}

/// Extract domain from a URL (e.g. "http://server.example.com:8080/path" → "server.example.com")
fn extract_domain(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    // Strip port
    host.split(':').next().unwrap_or(host).to_string()
}

/// Detect quality tags from channel name or group (e.g. "4K", "FHD", "HD", "SD")
fn detect_quality_tag(name: &str, group: &str) -> Option<String> {
    let combined = format!("{} {}", name, group).to_uppercase();
    if combined.contains("4K") || combined.contains("2160") || combined.contains("UHD") {
        Some("4K".to_string())
    } else if combined.contains("FHD") || combined.contains("1080") {
        Some("FHD".to_string())
    } else if combined.contains(" HD") || combined.contains("720") || combined.contains("[HD]") {
        Some("HD".to_string())
    } else if combined.contains("SD") || combined.contains("480") || combined.contains("LQ") {
        Some("SD".to_string())
    } else {
        None
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn draw_logs_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(90, 85, f.area());
    f.render_widget(Clear, area);

    let entries = app.log.entries();
    let total = entries.len();

    // Split area into content + status bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let content_area = chunks[0];
    let inner_height = content_area.height.saturating_sub(2) as usize; // minus borders

    // Scroll: log_scroll=0 means show newest at bottom
    let end = total.saturating_sub(app.log_scroll);
    let start = end.saturating_sub(inner_height);

    let items: Vec<ListItem> = entries[start..end]
        .iter()
        .map(|entry| {
            let time = entry.timestamp.with_timezone(&app.tz).format("%H:%M:%S");
            let (level_color, level_str) = match entry.level {
                LogLevel::Info => (app.theme.accent, "INFO"),
                LogLevel::Warn => (app.theme.highlight, "WARN"),
                LogLevel::Error => (app.theme.error, "ERR "),
                LogLevel::Player => (app.theme.special, "PLAY"),
            };
            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", time),
                    Style::default().fg(app.theme.text_dim),
                ),
                Span::styled(
                    format!("{} ", level_str),
                    Style::default()
                        .fg(level_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{}] ", entry.source),
                    Style::default().fg(app.theme.text_dim),
                ),
                Span::styled(entry.message.clone(), Style::default().fg(app.theme.text)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .title(format!(" Logs ({}) ", total));

    let list = List::new(items).block(block);
    f.render_widget(list, content_area);

    // Status bar
    let status = Line::from(vec![Span::styled(
        " j/k scroll  PgUp/PgDn page  g/G top/bottom  Esc close",
        Style::default().fg(app.theme.text_dim),
    )]);
    f.render_widget(Paragraph::new(status), chunks[1]);
}

fn draw_timezone_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 70, f.area());
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Search
            Constraint::Min(1),    // List
            Constraint::Length(1), // Status
        ])
        .split(area);

    // Title
    let title = Paragraph::new("Select Timezone")
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.accent)),
        )
        .style(
            Style::default()
                .fg(app.theme.text)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(title, chunks[0]);

    // Search
    let search_text = if app.tz_search.is_empty() {
        String::from("Start typing to search...")
    } else {
        format!("Search: {}", app.tz_search)
    };
    let search = Paragraph::new(search_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Filter ")
            .border_style(Style::default().fg(app.theme.highlight)),
    );
    f.render_widget(search, chunks[1]);

    // Timezone list
    let now = Utc::now();

    let selected_idx = app.tz_selected;
    let items: Vec<ListItem> = if app.tz_filtered.is_empty() {
        // Show all timezones
        app.tz_list
            .iter()
            .enumerate()
            .map(|(i, &tz)| {
                let local_time = now.with_timezone(&tz).format("%H:%M").to_string();
                let current_marker = if tz == app.tz { " ✓" } else { "" };
                let line = format!("{:<30} {} {}", tz.name(), local_time, current_marker);
                let style = if i == selected_idx {
                    Style::default().bg(app.theme.info).fg(app.theme.text)
                } else if tz == app.tz {
                    Style::default().fg(app.theme.success)
                } else {
                    Style::default().fg(app.theme.text)
                };
                ListItem::new(line).style(style)
            })
            .collect()
    } else {
        // Show filtered timezones
        app.tz_filtered
            .iter()
            .enumerate()
            .map(|(i, &real_idx)| {
                if let Some(&tz) = app.tz_list.get(real_idx) {
                    let local_time = now.with_timezone(&tz).format("%H:%M").to_string();
                    let current_marker = if tz == app.tz { " ✓" } else { "" };
                    let line = format!("{:<30} {} {}", tz.name(), local_time, current_marker);
                    let style = if i == selected_idx {
                        Style::default().bg(app.theme.info).fg(app.theme.text)
                    } else if tz == app.tz {
                        Style::default().fg(app.theme.success)
                    } else {
                        Style::default().fg(app.theme.text)
                    };
                    ListItem::new(line).style(style)
                } else {
                    ListItem::new("Invalid timezone").style(Style::default().fg(app.theme.error))
                }
            })
            .collect()
    };

    let items_count = items.len();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Timezones ({}) ", items_count))
            .border_style(Style::default().fg(app.theme.accent)),
    );
    f.render_widget(list, chunks[2]);

    // Status
    let status = Line::from(vec![Span::styled(
        " j/k select  Enter apply  Esc cancel  Type to search",
        Style::default().fg(app.theme.text_dim),
    )]);
    f.render_widget(Paragraph::new(status), chunks[3]);
}
