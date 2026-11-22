use crate::model::{Channel, ContentType, Playlist};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

/// Parse an M3U playlist from a buffered reader.
pub fn parse_m3u<R: BufRead>(reader: R) -> Result<Playlist> {
    let mut channels = Vec::new();
    let mut group_intern: HashMap<String, Arc<str>> = HashMap::new();
    let mut pending_extinf: Option<String> = None;
    let mut seen_header = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Warning: failed to read line: {e}");
                continue;
            }
        };

        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("#EXTM3U") {
            seen_header = true;
            continue;
        }

        if trimmed.starts_with("#EXTINF") {
            // If there was a previous pending EXTINF without a URL, skip it
            if pending_extinf.is_some() {
                eprintln!("Warning: skipping EXTINF without URL");
            }
            pending_extinf = Some(trimmed.to_string());
            continue;
        }

        // Skip other comments
        if trimmed.starts_with('#') {
            continue;
        }

        // This should be a URL line
        if let Some(extinf) = pending_extinf.take() {
            let idx = channels.len();
            if let Some(channel) = parse_extinf_and_url(&extinf, trimmed, &mut group_intern, idx) {
                channels.push(channel);
            }
        }
        // else: URL line without preceding EXTINF, skip
    }

    if pending_extinf.is_some() {
        eprintln!("Warning: trailing EXTINF without URL at end of file");
    }

    let _ = seen_header;

    // Build group list and indices
    let mut group_indices: HashMap<Arc<str>, Vec<usize>> = HashMap::new();
    for (i, ch) in channels.iter().enumerate() {
        group_indices
            .entry(Arc::clone(&ch.group))
            .or_default()
            .push(i);
    }

    let mut groups: Vec<Arc<str>> = group_indices.keys().cloned().collect();
    groups.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));

    Ok(Playlist {
        channels,
        groups,
        group_indices,
    })
}

// Replace the placeholder parse_m3u with the stateful version
pub fn parse_m3u_file(path: &std::path::Path) -> Result<Playlist> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open playlist: {}", path.display()))?;
    let reader = std::io::BufReader::new(file);
    parse_m3u(reader)
}

fn parse_extinf_and_url(
    extinf_line: &str,
    url: &str,
    group_intern: &mut HashMap<String, Arc<str>>,
    index: usize,
) -> Option<Channel> {
    // Format: #EXTINF:-1 tvg-id="..." tvg-name="..." tvg-logo="..." group-title="...",Channel Name
    // Extract attributes and channel name

    let after_extinf = extinf_line.strip_prefix("#EXTINF:")?;

    // Find the channel name (after the last comma)
    let channel_name = match after_extinf.rfind(',') {
        Some(pos) => after_extinf[pos + 1..].trim(),
        None => {
            eprintln!("Warning: EXTINF line has no comma, skipping");
            return None;
        }
    };

    if channel_name.is_empty() {
        eprintln!("Warning: empty channel name, skipping");
        return None;
    }

    // Extract attributes from the part before the last comma
    let attrs_part = match after_extinf.rfind(',') {
        Some(pos) => &after_extinf[..pos],
        None => "",
    };

    let tvg_id = extract_attribute(attrs_part, "tvg-id");
    let tvg_logo = extract_attribute(attrs_part, "tvg-logo");
    let tvg_name = extract_attribute(attrs_part, "tvg-name");
    let tvg_language = extract_attribute(attrs_part, "tvg-language");
    let tvg_country = extract_attribute(attrs_part, "tvg-country");
    let group_title = extract_attribute(attrs_part, "group-title").unwrap_or_default();

    // Intern group name
    let group: Arc<str> = if let Some(existing) = group_intern.get(&group_title) {
        Arc::clone(existing)
    } else {
        let arc: Arc<str> = Arc::from(group_title.as_str());
        group_intern.insert(group_title, Arc::clone(&arc));
        arc
    };

    // Detect content type
    let content_type = detect_content_type(channel_name, &group);

    Some(Channel {
        name: Arc::from(channel_name),
        url: Arc::from(url),
        group,
        logo_url: tvg_logo.map(|s| Arc::from(s.as_str())),
        tvg_id: tvg_id.map(|s| Arc::from(s.as_str())),
        tvg_name: tvg_name.map(|s| Arc::from(s.as_str())),
        tvg_language: tvg_language.map(|s| Arc::from(s.as_str())),
        tvg_country: tvg_country.map(|s| Arc::from(s.as_str())),
        content_type,
        index,
    })
}

fn extract_attribute(attrs: &str, name: &str) -> Option<String> {
    let search = format!("{}=\"", name);
    let start = attrs.find(&search)?;
    let value_start = start + search.len();
    let rest = &attrs[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn detect_content_type(name: &str, group: &str) -> ContentType {
    let group_lower = group.to_lowercase();

    // Check for movie/VOD
    if group_lower.contains("movie") || group_lower.contains("vod") {
        return ContentType::Movie;
    }

    // Check for series pattern: S01E01, S1E1, etc.
    if let Some(series_info) = parse_series_info(name) {
        return series_info;
    }

    if group_lower.contains("series") {
        // Try to parse as series even without SxxExx pattern
        return ContentType::Series {
            series_name: Arc::from(name),
            season: 0,
            episode: 0,
        };
    }

    ContentType::Live
}

fn parse_series_info(name: &str) -> Option<ContentType> {
    // Look for patterns like S01E02, S1E1
    let mut i = 0;
    let bytes = name.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'S' || bytes[i] == b's' {
            if let Some((season, next)) = parse_number(&bytes[i + 1..]) {
                if next < bytes.len() - i - 1
                    && (bytes[i + 1 + next] == b'E' || bytes[i + 1 + next] == b'e')
                {
                    if let Some((episode, _)) = parse_number(&bytes[i + 2 + next..]) {
                        // Extract series name (everything before the SxxExx pattern)
                        let series_name = name[..i].trim().trim_end_matches(&['-', ' ', '.'][..]);
                        return Some(ContentType::Series {
                            series_name: Arc::from(if series_name.is_empty() {
                                name
                            } else {
                                series_name
                            }),
                            season,
                            episode,
                        });
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn parse_number(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut n: u32 = 0;
    let mut count = 0;
    for &b in bytes {
        if b.is_ascii_digit() {
            n = n * 10 + (b - b'0') as u32;
            count += 1;
        } else {
            break;
        }
    }
    if count > 0 {
        Some((n, count))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse_str(s: &str) -> Result<Playlist> {
        parse_m3u(Cursor::new(s))
    }

    #[test]
    fn test_basic_parsing() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 tvg-id="cnn" tvg-logo="http://logo.com/cnn.png" group-title="News",CNN
http://stream.example.com/cnn
#EXTINF:-1 tvg-id="bbc" group-title="News",BBC World
http://stream.example.com/bbc
#EXTINF:-1 group-title="Sports",ESPN
http://stream.example.com/espn
"#;
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels.len(), 3);
        assert_eq!(playlist.groups.len(), 2);
        assert_eq!(playlist.channels[0].name.as_ref(), "CNN");
        assert_eq!(playlist.channels[0].tvg_id.as_deref(), Some("cnn"));
        assert_eq!(
            playlist.channels[0].logo_url.as_deref(),
            Some("http://logo.com/cnn.png")
        );
        assert_eq!(playlist.channels[0].group.as_ref(), "News");
    }

    #[test]
    fn test_group_title_with_spaces() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="United States News",US CNN
http://stream.example.com/uscnn
"#;
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels[0].group.as_ref(), "United States News");
    }

    #[test]
    fn test_content_type_detection_movie() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="VOD Movies",Inception (2010)
http://stream.example.com/inception
"#;
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels[0].content_type, ContentType::Movie);
    }

    #[test]
    fn test_content_type_detection_series() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="Drama",Breaking Bad S01E01
http://stream.example.com/bb
"#;
        let playlist = parse_str(m3u).unwrap();
        match &playlist.channels[0].content_type {
            ContentType::Series {
                series_name,
                season,
                episode,
            } => {
                assert_eq!(series_name.as_ref(), "Breaking Bad");
                assert_eq!(*season, 1);
                assert_eq!(*episode, 1);
            }
            other => panic!("Expected Series, got {:?}", other),
        }
    }

    #[test]
    fn test_content_type_detection_live() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="News",CNN Live
http://stream.example.com/cnn
"#;
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels[0].content_type, ContentType::Live);
    }

    #[test]
    fn test_malformed_lines_skipped() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="News",CNN
http://stream.example.com/cnn
#EXTINF:-1 group-title="News"
#EXTINF:-1 group-title="Sports",ESPN
http://stream.example.com/espn
random garbage line
"#;
        let playlist = parse_str(m3u).unwrap();
        // Should parse CNN and ESPN, skip the malformed one (no comma) and garbage
        assert_eq!(playlist.channels.len(), 2);
    }

    #[test]
    fn test_empty_playlist() {
        let m3u = "#EXTM3U\n";
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels.len(), 0);
        assert_eq!(playlist.groups.len(), 0);
    }

    #[test]
    fn test_group_indices() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="News",CNN
http://a.com/1
#EXTINF:-1 group-title="Sports",ESPN
http://a.com/2
#EXTINF:-1 group-title="News",BBC
http://a.com/3
"#;
        let playlist = parse_str(m3u).unwrap();
        assert_eq!(playlist.channels_in_group("News").len(), 2);
        assert_eq!(playlist.channels_in_group("Sports").len(), 1);
        assert_eq!(playlist.channels_in_group("Nonexistent").len(), 0);
    }

    #[test]
    fn test_arc_interning() {
        let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="News",CNN
http://a.com/1
#EXTINF:-1 group-title="News",BBC
http://a.com/2
"#;
        let playlist = parse_str(m3u).unwrap();
        // Both channels should share the same Arc for "News"
        assert!(Arc::ptr_eq(
            &playlist.channels[0].group,
            &playlist.channels[1].group
        ));
    }
}
