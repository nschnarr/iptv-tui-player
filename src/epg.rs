use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::io::BufRead;

/// A single TV programme from an XMLTV EPG.
#[derive(Debug, Clone)]
pub struct Programme {
    pub title: String,
    pub start: DateTime<Utc>,
    pub stop: DateTime<Utc>,
    #[allow(dead_code)]
    pub description: Option<String>,
}

/// Electronic Programme Guide parsed from XMLTV format.
#[derive(Debug, Default, Clone)]
pub struct Epg {
    /// Maps channel_id (tvg-id) → sorted vec of programmes.
    pub data: HashMap<String, Vec<Programme>>,
}

impl Epg {
    /// Parse XMLTV data from a buffered reader.
    pub fn parse_xmltv<R: BufRead>(reader: R) -> Result<Self> {
        use quick_xml::events::Event;
        use quick_xml::Reader;

        let mut xml_reader = Reader::from_reader(reader);
        let mut buf = Vec::new();

        let mut data: HashMap<String, Vec<Programme>> = HashMap::new();

        // State for current <programme>
        let mut in_programme = false;
        let mut current_channel_id = String::new();
        let mut current_start: Option<DateTime<Utc>> = None;
        let mut current_stop: Option<DateTime<Utc>> = None;
        let mut current_title: Option<String> = None;
        let mut current_desc: Option<String> = None;
        let mut in_title = false;
        let mut in_desc = false;
        let mut text_buf = String::new();

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    match name.as_ref() {
                        b"programme" => {
                            in_programme = true;
                            current_channel_id.clear();
                            current_start = None;
                            current_stop = None;
                            current_title = None;
                            current_desc = None;

                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"channel" => {
                                        current_channel_id =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"start" => {
                                        let val = String::from_utf8_lossy(&attr.value);
                                        current_start = parse_xmltv_datetime(&val);
                                    }
                                    b"stop" => {
                                        let val = String::from_utf8_lossy(&attr.value);
                                        current_stop = parse_xmltv_datetime(&val);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        b"title" if in_programme => {
                            in_title = true;
                            text_buf.clear();
                        }
                        b"desc" if in_programme => {
                            in_desc = true;
                            text_buf.clear();
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if in_title || in_desc {
                        if let Ok(text) = e.unescape() {
                            text_buf.push_str(&text);
                        }
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"title" if in_title => {
                        current_title = Some(text_buf.clone());
                        in_title = false;
                    }
                    b"desc" if in_desc => {
                        current_desc = if text_buf.is_empty() {
                            None
                        } else {
                            Some(text_buf.clone())
                        };
                        in_desc = false;
                    }
                    b"programme" if in_programme => {
                        if let (Some(start), Some(stop), Some(title)) =
                            (current_start, current_stop, current_title.take())
                        {
                            let programme = Programme {
                                title,
                                start,
                                stop,
                                description: current_desc.take(),
                            };
                            data.entry(current_channel_id.clone())
                                .or_default()
                                .push(programme);
                        }
                        in_programme = false;
                    }
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(anyhow::anyhow!("XML parse error at position {}: {e}", xml_reader.error_position()));
                }
                _ => {}
            }
            buf.clear();
        }

        // Sort programmes by start time
        for programmes in data.values_mut() {
            programmes.sort_by_key(|p| p.start);
        }

        Ok(Epg { data })
    }

    /// Parse XMLTV from a file path.
    pub fn parse_xmltv_file(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open EPG file: {}", path.display()))?;
        let reader = std::io::BufReader::new(file);
        Self::parse_xmltv(reader)
    }

    /// Parse XMLTV from a URL. Caches the raw data to disk.
    pub fn parse_xmltv_url(url: &str) -> Result<Self> {
        let response = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(60))
            .build()
            .get(url)
            .call()
            .with_context(|| format!("Failed to fetch EPG from {url}"))?;
        
        // Read into memory to both cache and parse
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut response.into_reader(), &mut data)
            .context("Failed to read EPG response")?;
        crate::cache::save_epg(&data, Some(url));
        let cursor = std::io::Cursor::new(data);
        let reader = std::io::BufReader::new(cursor);
        Self::parse_xmltv(reader)
    }

    /// Try loading EPG from cache. Returns None if no cache exists.
    pub fn load_cached() -> Option<Self> {
        let path = crate::cache::load_epg_path()?;
        Self::parse_xmltv_file(&path).ok()
    }

    /// Get the currently airing programme for a channel.
    pub fn now_playing(&self, channel_id: &str) -> Option<&Programme> {
        self.now_playing_at(channel_id, Utc::now())
    }

    /// Get the currently airing programme for a channel at a given time.
    pub fn now_playing_at(&self, channel_id: &str, at: DateTime<Utc>) -> Option<&Programme> {
        let programmes = self.data.get(channel_id)?;
        programmes.iter().find(|p| p.start <= at && p.stop > at)
    }

    /// Get the next upcoming programme for a channel.
    #[allow(dead_code)]
    pub fn next_programme(&self, channel_id: &str) -> Option<&Programme> {
        self.next_programme_at(channel_id, Utc::now())
    }

    /// Get all programmes for a channel that overlap with a time window.
    pub fn programmes_in_range(
        &self,
        channel_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<&Programme> {
        match self.data.get(channel_id) {
            Some(programmes) => programmes
                .iter()
                .filter(|p| p.start < end && p.stop > start)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Get the next upcoming programme after a given time.
    #[allow(dead_code)]
    pub fn next_programme_at(&self, channel_id: &str, at: DateTime<Utc>) -> Option<&Programme> {
        let programmes = self.data.get(channel_id)?;
        programmes.iter().find(|p| p.start > at)
    }

    /// Total number of programmes loaded.
    pub fn programme_count(&self) -> usize {
        self.data.values().map(|v| v.len()).sum()
    }

    /// Number of channels with EPG data.
    pub fn channel_count(&self) -> usize {
        self.data.len()
    }
}

/// Parse XMLTV datetime format: "20260213120000 +0000" → DateTime<Utc>
fn parse_xmltv_datetime(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();

    // Try with timezone offset: "20260213120000 +0000"
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y%m%d%H%M%S %z") {
        return Some(dt.with_timezone(&Utc));
    }

    // Try without space before offset: "20260213120000+0000"
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y%m%d%H%M%S%z") {
        return Some(dt.with_timezone(&Utc));
    }

    // Try without timezone (assume UTC): "20260213120000"
    if s.len() >= 14 {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s[..14], "%Y%m%d%H%M%S") {
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
        }
    }

    None
}

/// Format a programme's time for display using a specific timezone.
pub fn format_time_tz(dt: &DateTime<Utc>, tz: &chrono_tz::Tz) -> String {
    dt.with_timezone(tz).format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SAMPLE_XMLTV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <channel id="CNN">
    <display-name>CNN</display-name>
  </channel>
  <channel id="BBC">
    <display-name>BBC World News</display-name>
  </channel>
  <programme start="20260213120000 +0000" stop="20260213130000 +0000" channel="CNN">
    <title>CNN Newsroom</title>
    <desc>Breaking news coverage.</desc>
  </programme>
  <programme start="20260213130000 +0000" stop="20260213140000 +0000" channel="CNN">
    <title>Anderson Cooper 360</title>
  </programme>
  <programme start="20260213120000 +0000" stop="20260213133000 +0000" channel="BBC">
    <title>World News Today</title>
    <desc>International news.</desc>
  </programme>
</tv>"#;

    #[test]
    fn test_parse_xmltv() {
        let epg = Epg::parse_xmltv(std::io::Cursor::new(SAMPLE_XMLTV)).unwrap();
        assert_eq!(epg.channel_count(), 2);
        assert_eq!(epg.programme_count(), 3);

        let cnn = &epg.data["CNN"];
        assert_eq!(cnn.len(), 2);
        assert_eq!(cnn[0].title, "CNN Newsroom");
        assert_eq!(cnn[0].description.as_deref(), Some("Breaking news coverage."));
        assert_eq!(cnn[1].title, "Anderson Cooper 360");
        assert!(cnn[1].description.is_none());
    }

    #[test]
    fn test_now_playing() {
        let epg = Epg::parse_xmltv(std::io::Cursor::new(SAMPLE_XMLTV)).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 2, 13, 12, 30, 0).unwrap();

        let now = epg.now_playing_at("CNN", at);
        assert!(now.is_some());
        assert_eq!(now.unwrap().title, "CNN Newsroom");

        let now_bbc = epg.now_playing_at("BBC", at);
        assert!(now_bbc.is_some());
        assert_eq!(now_bbc.unwrap().title, "World News Today");
    }

    #[test]
    fn test_next_programme() {
        let epg = Epg::parse_xmltv(std::io::Cursor::new(SAMPLE_XMLTV)).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 2, 13, 12, 30, 0).unwrap();

        let next = epg.next_programme_at("CNN", at);
        assert!(next.is_some());
        assert_eq!(next.unwrap().title, "Anderson Cooper 360");
    }

    #[test]
    fn test_no_programme_found() {
        let epg = Epg::parse_xmltv(std::io::Cursor::new(SAMPLE_XMLTV)).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 2, 13, 20, 0, 0).unwrap();

        assert!(epg.now_playing_at("CNN", at).is_none());
        assert!(epg.now_playing_at("NONEXISTENT", at).is_none());
    }

    #[test]
    fn test_parse_datetime_formats() {
        // With space
        let dt = parse_xmltv_datetime("20260213120000 +0000");
        assert!(dt.is_some());
        assert_eq!(dt.unwrap(), Utc.with_ymd_and_hms(2026, 2, 13, 12, 0, 0).unwrap());

        // Without space
        let dt = parse_xmltv_datetime("20260213120000+0000");
        assert!(dt.is_some());

        // Without timezone
        let dt = parse_xmltv_datetime("20260213120000");
        assert!(dt.is_some());
        assert_eq!(dt.unwrap(), Utc.with_ymd_and_hms(2026, 2, 13, 12, 0, 0).unwrap());

        // With non-UTC offset
        let dt = parse_xmltv_datetime("20260213120000 +0200");
        assert!(dt.is_some());
        assert_eq!(dt.unwrap(), Utc.with_ymd_and_hms(2026, 2, 13, 10, 0, 0).unwrap());
    }

    #[test]
    fn test_empty_xmltv() {
        let xml = r#"<?xml version="1.0"?><tv></tv>"#;
        let epg = Epg::parse_xmltv(std::io::Cursor::new(xml)).unwrap();
        assert_eq!(epg.channel_count(), 0);
        assert_eq!(epg.programme_count(), 0);
    }

    #[test]
    fn test_programme_without_required_fields_skipped() {
        let xml = r#"<?xml version="1.0"?>
<tv>
  <programme start="20260213120000 +0000" channel="CNN">
    <title>Missing stop</title>
  </programme>
  <programme start="20260213120000 +0000" stop="20260213130000 +0000" channel="CNN">
    <title>Valid Programme</title>
  </programme>
</tv>"#;
        let epg = Epg::parse_xmltv(std::io::Cursor::new(xml)).unwrap();
        assert_eq!(epg.programme_count(), 1);
        assert_eq!(epg.data["CNN"][0].title, "Valid Programme");
    }

    #[test]
    fn test_format_time() {
        let dt = Utc.with_ymd_and_hms(2026, 2, 13, 13, 0, 0).unwrap();
        assert_eq!(format_time_tz(&dt, &chrono_tz::Tz::UTC), "13:00");
    }
}
