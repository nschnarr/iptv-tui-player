use crate::epg::Epg;
use crate::favorites;
use crate::model::Playlist;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum BufferMode {
    /// Raw HTTP pipe (no buffering)
    None,
    /// FFmpeg remux + buffer
    FFmpeg,
}

impl BufferMode {
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "none" | "raw" => Ok(Self::None),
            "ffmpeg" => Ok(Self::FFmpeg),
            _ => anyhow::bail!("Unknown buffer mode: {}. Use 'none' or 'ffmpeg'", s),
        }
    }
}

const DEVICE_ID: &str = "12345678";
const FIRMWARE_NAME: &str = "iptv-tui";
const FIRMWARE_VERSION: &str = "0.1.0";
const MODEL_NUMBER: &str = "HDTC-2US";
const TUNER_COUNT: u32 = 2;

#[derive(Serialize)]
struct DiscoverResponse {
    #[serde(rename = "FriendlyName")]
    friendly_name: String,
    #[serde(rename = "Manufacturer")]
    manufacturer: String,
    #[serde(rename = "ModelNumber")]
    model_number: String,
    #[serde(rename = "FirmwareName")]
    firmware_name: String,
    #[serde(rename = "FirmwareVersion")]
    firmware_version: String,
    #[serde(rename = "DeviceID")]
    device_id: String,
    #[serde(rename = "DeviceAuth")]
    device_auth: String,
    #[serde(rename = "BaseURL")]
    base_url: String,
    #[serde(rename = "LineupURL")]
    lineup_url: String,
    #[serde(rename = "TunerCount")]
    tuner_count: u32,
}

#[derive(Serialize)]
struct LineupStatus {
    #[serde(rename = "ScanInProgress")]
    scan_in_progress: u32,
    #[serde(rename = "ScanPossible")]
    scan_possible: u32,
    #[serde(rename = "Source")]
    source: String,
    #[serde(rename = "SourceList")]
    source_list: Vec<String>,
}

#[derive(Serialize)]
struct LineupEntry {
    #[serde(rename = "GuideNumber")]
    guide_number: String,
    #[serde(rename = "GuideName")]
    guide_name: String,
    #[serde(rename = "URL")]
    url: String,
}

/// Shared state for the HDHR server — favorites can change at runtime.
pub struct HdhrState {
    pub playlist: Arc<Playlist>,
    pub favorites: HashSet<String>,
    pub epg: Option<Arc<Epg>>,
    pub buffer_mode: BufferMode,
    pub buffer_secs: f64,
}

pub struct HdhrServer {
    port: u16,
    bind_addr: String,
    state: Arc<Mutex<HdhrState>>,
}

impl HdhrServer {
    pub fn new(
        port: u16,
        bind_addr: String,
        playlist: Arc<Playlist>,
        favorites: HashSet<String>,
        epg: Option<Arc<Epg>>,
        buffer_mode: BufferMode,
        buffer_secs: f64,
    ) -> Self {
        Self {
            port,
            bind_addr,
            state: Arc::new(Mutex::new(HdhrState {
                playlist,
                favorites,
                epg,
                buffer_mode,
                buffer_secs,
            })),
        }
    }

    /// Get a handle to update favorites at runtime.
    pub fn state(&self) -> Arc<Mutex<HdhrState>> {
        Arc::clone(&self.state)
    }

    /// Start the HDHR server in a background thread. Returns the actual port.
    pub fn start(self) -> anyhow::Result<u16> {
        let listener = TcpListener::bind(format!("{}:{}", self.bind_addr, self.port))?;
        let actual_port = listener.local_addr()?.port();
        let state = self.state;

        thread::Builder::new()
            .name("hdhr-server".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let state = Arc::clone(&state);
                    thread::spawn(move || {
                        if let Err(e) = handle_connection(stream, &state, actual_port) {
                            // Connection errors are normal (client disconnect, etc.)
                            let _ = e;
                        }
                    });
                }
            })?;

        Ok(actual_port)
    }
}

fn handle_connection(
    mut stream: TcpStream,
    state: &Arc<Mutex<HdhrState>>,
    port: u16,
) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse: "GET /path HTTP/1.1"
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    // Consume remaining headers, capture Host
    let mut host = format!("localhost:{}", port);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(h) = line
            .strip_prefix("Host: ")
            .or_else(|| line.strip_prefix("host: "))
        {
            host = h.trim().to_string();
        }
    }

    match path.as_str() {
        "/discover.json" => {
            let base_url = format!("http://{}", host);
            let resp = DiscoverResponse {
                friendly_name: "IPTV TUI".to_string(),
                manufacturer: "iptv-tui".to_string(),
                model_number: MODEL_NUMBER.to_string(),
                firmware_name: FIRMWARE_NAME.to_string(),
                firmware_version: FIRMWARE_VERSION.to_string(),
                device_id: DEVICE_ID.to_string(),
                device_auth: DEVICE_ID.to_string(),
                base_url: base_url.clone(),
                lineup_url: format!("{}/lineup.json", base_url),
                tuner_count: TUNER_COUNT,
            };
            send_json(&mut stream, &resp)?;
        }
        "/lineup_status.json" => {
            let resp = LineupStatus {
                scan_in_progress: 0,
                scan_possible: 1,
                source: "Cable".to_string(),
                source_list: vec!["Cable".to_string()],
            };
            send_json(&mut stream, &resp)?;
        }
        "/lineup.json" => {
            let lineup = build_lineup(state, &host);
            send_json(&mut stream, &lineup)?;
        }
        p if p.starts_with("/auto/v") => {
            let channel_num = p.trim_start_matches("/auto/v");
            if let Ok(num) = channel_num.parse::<usize>() {
                let (url, buffer_mode, buffer_secs) = {
                    let s = state.lock().unwrap();
                    (
                        get_channel_url_inner(&s, num),
                        s.buffer_mode.clone(),
                        s.buffer_secs,
                    )
                };
                if let Some(url) = url {
                    match buffer_mode {
                        BufferMode::FFmpeg => ffmpeg_stream(&mut stream, &url, buffer_secs)?,
                        BufferMode::None => proxy_stream(&mut stream, &url, buffer_secs)?,
                    }
                } else {
                    send_404(&mut stream)?;
                }
            } else {
                send_404(&mut stream)?;
            }
        }
        "/xmltv.xml" => {
            let xml = build_xmltv(state);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                xml.len(),
                xml
            );
            stream.write_all(response.as_bytes())?;
        }
        "/device.xml" => {
            let xml = format!(
                r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>IPTV TUI</friendlyName>
    <manufacturer>iptv-tui</manufacturer>
    <modelName>{}</modelName>
    <modelNumber>{}</modelNumber>
    <serialNumber>{}</serialNumber>
    <UDN>uuid:{}</UDN>
  </device>
</root>"#,
                MODEL_NUMBER, FIRMWARE_VERSION, DEVICE_ID, DEVICE_ID
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                xml.len(),
                xml
            );
            stream.write_all(response.as_bytes())?;
        }
        _ => {
            send_404(&mut stream)?;
        }
    }

    Ok(())
}

/// Stream via FFmpeg — remux + buffer for reliable Plex playback.
/// FFmpeg handles: HLS/m3u8, codec container issues, buffering, reconnects.
fn ffmpeg_stream(client: &mut TcpStream, url: &str, buffer_secs: f64) -> anyhow::Result<()> {
    // Spawn FFmpeg: fetch stream, remux to MPEG-TS, output to stdout
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            // Input options
            "-user_agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            "-reconnect",
            "1",
            "-reconnect_streamed",
            "1",
            "-reconnect_delay_max",
            "5",
            "-i",
            url,
            // Output options: copy codecs (no re-encoding), output MPEG-TS to stdout
            "-c",
            "copy",
            "-f",
            "mpegts",
            "-metadata",
            "service_name=IPTV",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;

    let mut stdout = child
        .stdout
        .take()
        .expect("failed to capture ffmpeg stdout");

    // Pre-roll: accumulate data for buffer_secs before sending anything
    let preroll = prebuffer(&mut stdout, buffer_secs);

    // Now send HTTP headers + pre-rolled data
    let headers = "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    client.write_all(headers.as_bytes())?;

    // Flush pre-roll buffer
    if !preroll.is_empty() {
        write_chunk(client, &preroll)?;
    }

    // Continue streaming
    let mut buf = [0u8; 65536];
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        };

        if write_chunk(client, &buf[..n]).is_err() {
            break; // Client disconnected
        }
    }

    // Final chunk
    let _ = client.write_all(b"0\r\n\r\n");

    // Clean up FFmpeg process
    let _ = child.kill();
    let _ = child.wait();

    Ok(())
}

/// Proxy an IPTV stream to the client (Plex).
/// We fetch the stream ourselves with proper headers and pipe it through.
fn proxy_stream(client: &mut TcpStream, url: &str, buffer_secs: f64) -> anyhow::Result<()> {
    let response = ureq::get(url)
        .set(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .set("Accept", "*/*")
        .set("Connection", "keep-alive")
        .call()?;

    // Get content type from upstream
    let content_type = response
        .header("Content-Type")
        .unwrap_or("video/mp2t")
        .to_string();

    let mut reader = response.into_reader();

    // Pre-roll: accumulate data for buffer_secs before sending anything
    let preroll = prebuffer(&mut reader, buffer_secs);

    // Now send HTTP headers + pre-rolled data
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        content_type
    );
    client.write_all(headers.as_bytes())?;

    // Flush pre-roll buffer
    if !preroll.is_empty() {
        write_chunk(client, &preroll)?;
    }

    // Continue streaming
    let mut buf = [0u8; 65536];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        };

        if write_chunk(client, &buf[..n]).is_err() {
            break; // Client disconnected
        }
    }

    // Final chunk
    let _ = client.write_all(b"0\r\n\r\n");
    Ok(())
}

/// Accumulate data from a reader for `secs` wall-clock seconds.
/// Returns the buffered bytes. If secs <= 0, returns empty (no buffering).
fn prebuffer(reader: &mut dyn Read, secs: f64) -> Vec<u8> {
    if secs <= 0.0 {
        return Vec::new();
    }

    let deadline = Instant::now() + Duration::from_secs_f64(secs);
    let mut accumulated = Vec::with_capacity(2 * 1024 * 1024); // Start with 2MB capacity
    let mut buf = [0u8; 65536];

    while Instant::now() < deadline {
        match reader.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => accumulated.extend_from_slice(&buf[..n]),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        }
    }

    accumulated
}

/// Write a single HTTP chunked-transfer-encoding chunk.
fn write_chunk(client: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let header = format!("{:x}\r\n", data.len());
    client.write_all(header.as_bytes())?;
    client.write_all(data)?;
    client.write_all(b"\r\n")?;
    Ok(())
}

fn build_lineup(state: &Arc<Mutex<HdhrState>>, host: &str) -> Vec<LineupEntry> {
    let state = state.lock().unwrap();
    let mut entries = Vec::new();
    let mut channel_num = 1;

    for channel in &state.playlist.channels {
        if state
            .favorites
            .contains(&favorites::favorite_key(&channel.name, &channel.group))
        {
            entries.push(LineupEntry {
                guide_number: format!("{}", channel_num),
                guide_name: channel.name.to_string(),
                // Proxy URL — stream goes through our server with proper headers
                url: format!("http://{}/auto/v{}", host, channel_num),
            });
            channel_num += 1;
        }
    }

    entries
}

/// Build XMLTV EPG data for favorited channels.
fn build_xmltv(state: &Arc<Mutex<HdhrState>>) -> String {
    let state = state.lock().unwrap();

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");

    // Collect favorited channels with their guide numbers
    let mut channel_num = 1u32;
    let mut fav_channels: Vec<(u32, usize)> = Vec::new(); // (guide_number, channel_idx)

    for (idx, channel) in state.playlist.channels.iter().enumerate() {
        if state
            .favorites
            .contains(&favorites::favorite_key(&channel.name, &channel.group))
        {
            fav_channels.push((channel_num, idx));
            channel_num += 1;
        }
    }

    // Write channel definitions
    for &(num, idx) in &fav_channels {
        let ch = &state.playlist.channels[idx];
        let name_escaped = xml_escape(&ch.name);
        xml.push_str(&format!(
            "  <channel id=\"{}\">\n    <display-name>{}</display-name>\n  </channel>\n",
            num, name_escaped
        ));
    }

    // Write programme data if EPG is available
    if let Some(ref epg) = state.epg {
        for &(num, idx) in &fav_channels {
            let ch = &state.playlist.channels[idx];
            if let Some(tvg_id) = ch.tvg_id.as_deref() {
                if let Some(programmes) = epg.data.get(tvg_id) {
                    for prog in programmes {
                        let start = prog.start.format("%Y%m%d%H%M%S +0000").to_string();
                        let stop = prog.stop.format("%Y%m%d%H%M%S +0000").to_string();
                        let title_escaped = xml_escape(&prog.title);
                        xml.push_str(&format!(
                            "  <programme start=\"{}\" stop=\"{}\" channel=\"{}\">\n    <title>{}</title>\n",
                            start, stop, num, title_escaped
                        ));
                        if let Some(ref desc) = prog.description {
                            let desc_escaped = xml_escape(desc);
                            xml.push_str(&format!("    <desc>{}</desc>\n", desc_escaped));
                        }
                        xml.push_str("  </programme>\n");
                    }
                }
            }
        }
    }

    xml.push_str("</tv>\n");
    xml
}

fn get_channel_url_inner(state: &HdhrState, channel_num: usize) -> Option<String> {
    let mut num = 1;
    for channel in &state.playlist.channels {
        if state
            .favorites
            .contains(&favorites::favorite_key(&channel.name, &channel.group))
        {
            if num == channel_num {
                return Some(channel.url.to_string());
            }
            num += 1;
        }
    }
    None
}

fn send_json<T: Serialize>(stream: &mut TcpStream, data: &T) -> anyhow::Result<()> {
    let body = serde_json::to_string(data)?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}

fn send_404(stream: &mut TcpStream) -> anyhow::Result<()> {
    let response = "HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n";
    stream.write_all(response.as_bytes())?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, ContentType, Playlist};
    use std::collections::HashMap;

    fn make_channel(name: &str, group: &str, url: &str, tvg_id: Option<&str>) -> Channel {
        Channel {
            name: Arc::from(name),
            url: Arc::from(url),
            group: Arc::from(group),
            logo_url: None,
            tvg_id: tvg_id.map(Arc::from),
            tvg_name: None,
            tvg_language: None,
            tvg_country: None,
            content_type: ContentType::Live,
            index: 0,
        }
    }

    fn make_state(channels: Vec<Channel>, favs: Vec<&str>) -> Arc<Mutex<HdhrState>> {
        let groups: Vec<Arc<str>> = channels
            .iter()
            .map(|c| c.group.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let mut group_indices: HashMap<Arc<str>, Vec<usize>> = HashMap::new();
        for (i, ch) in channels.iter().enumerate() {
            group_indices.entry(ch.group.clone()).or_default().push(i);
        }
        let playlist = Playlist {
            channels,
            groups,
            group_indices,
        };
        let favorites: HashSet<String> = favs.into_iter().map(|s| s.to_string()).collect();
        Arc::new(Mutex::new(HdhrState {
            playlist: Arc::new(playlist),
            favorites,
            epg: None,
            buffer_mode: BufferMode::None,
            buffer_secs: 0.0,
        }))
    }

    // --- BufferMode ---

    #[test]
    fn test_buffer_mode_from_str_none() {
        assert_eq!(BufferMode::from_str("none").unwrap(), BufferMode::None);
        assert_eq!(BufferMode::from_str("raw").unwrap(), BufferMode::None);
        assert_eq!(BufferMode::from_str("NONE").unwrap(), BufferMode::None);
    }

    #[test]
    fn test_buffer_mode_from_str_ffmpeg() {
        assert_eq!(BufferMode::from_str("ffmpeg").unwrap(), BufferMode::FFmpeg);
        assert_eq!(BufferMode::from_str("FFmpeg").unwrap(), BufferMode::FFmpeg);
    }

    #[test]
    fn test_buffer_mode_from_str_invalid() {
        assert!(BufferMode::from_str("vlc").is_err());
        assert!(BufferMode::from_str("").is_err());
    }

    // --- xml_escape ---

    #[test]
    fn test_xml_escape_plain() {
        assert_eq!(xml_escape("Hello World"), "Hello World");
    }

    #[test]
    fn test_xml_escape_special_chars() {
        assert_eq!(
            xml_escape("A & B < C > D \"E\" 'F'"),
            "A &amp; B &lt; C &gt; D &quot;E&quot; &apos;F&apos;"
        );
    }

    #[test]
    fn test_xml_escape_empty() {
        assert_eq!(xml_escape(""), "");
    }

    // --- build_lineup ---

    #[test]
    fn test_build_lineup_empty_favorites() {
        let state = make_state(
            vec![make_channel("CNN", "News", "http://cnn.stream", None)],
            vec![],
        );
        let lineup = build_lineup(&state, "localhost:5004");
        assert!(lineup.is_empty());
    }

    #[test]
    fn test_build_lineup_with_favorites() {
        let ch1 = make_channel("CNN", "News", "http://cnn.stream", None);
        let ch2 = make_channel("BBC", "News", "http://bbc.stream", None);
        let ch3 = make_channel("ESPN", "Sports", "http://espn.stream", None);
        let fav_key1 = favorites::favorite_key("CNN", "News");
        let fav_key3 = favorites::favorite_key("ESPN", "Sports");
        let state = make_state(vec![ch1, ch2, ch3], vec![&fav_key1, &fav_key3]);

        let lineup = build_lineup(&state, "myhost:5004");
        assert_eq!(lineup.len(), 2);
        assert_eq!(lineup[0].guide_name, "CNN");
        assert_eq!(lineup[0].guide_number, "1");
        assert_eq!(lineup[0].url, "http://myhost:5004/auto/v1");
        assert_eq!(lineup[1].guide_name, "ESPN");
        assert_eq!(lineup[1].guide_number, "2");
    }

    // --- get_channel_url_inner ---

    #[test]
    fn test_get_channel_url_by_number() {
        let ch1 = make_channel("CNN", "News", "http://cnn.stream", None);
        let ch2 = make_channel("BBC", "News", "http://bbc.stream", None);
        let fav1 = favorites::favorite_key("CNN", "News");
        let fav2 = favorites::favorite_key("BBC", "News");
        let state = make_state(vec![ch1, ch2], vec![&fav1, &fav2]);

        let s = state.lock().unwrap();
        assert_eq!(get_channel_url_inner(&s, 1).unwrap(), "http://cnn.stream");
        assert_eq!(get_channel_url_inner(&s, 2).unwrap(), "http://bbc.stream");
        assert!(get_channel_url_inner(&s, 3).is_none());
        assert!(get_channel_url_inner(&s, 0).is_none());
    }

    // --- build_xmltv ---

    #[test]
    fn test_build_xmltv_no_epg() {
        let ch = make_channel("CNN", "News", "http://cnn.stream", Some("CNN.us"));
        let fav = favorites::favorite_key("CNN", "News");
        let state = make_state(vec![ch], vec![&fav]);

        let xml = build_xmltv(&state);
        assert!(xml.contains("<tv>"));
        assert!(xml.contains("</tv>"));
        assert!(xml.contains("<display-name>CNN</display-name>"));
        // No programmes without EPG
        assert!(!xml.contains("<programme"));
    }

    #[test]
    fn test_build_xmltv_escapes_channel_names() {
        let ch = make_channel("Rock & Roll <TV>", "Music", "http://r.stream", None);
        let fav = favorites::favorite_key("Rock & Roll <TV>", "Music");
        let state = make_state(vec![ch], vec![&fav]);

        let xml = build_xmltv(&state);
        assert!(xml.contains("Rock &amp; Roll &lt;TV&gt;"));
    }

    #[test]
    fn test_build_xmltv_empty_favorites() {
        let ch = make_channel("CNN", "News", "http://cnn.stream", None);
        let state = make_state(vec![ch], vec![]);
        let xml = build_xmltv(&state);
        assert!(!xml.contains("<channel"));
    }

    // --- prebuffer ---

    #[test]
    fn test_prebuffer_zero_seconds() {
        let data = b"hello world";
        let mut cursor = std::io::Cursor::new(data.to_vec());
        let result = prebuffer(&mut cursor, 0.0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_prebuffer_negative_seconds() {
        let data = b"hello";
        let mut cursor = std::io::Cursor::new(data.to_vec());
        let result = prebuffer(&mut cursor, -1.0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_prebuffer_reads_until_eof() {
        let data = b"short data";
        let mut cursor = std::io::Cursor::new(data.to_vec());
        // 10 seconds but data is tiny — should return all of it
        let result = prebuffer(&mut cursor, 10.0);
        assert_eq!(result, data);
    }
}
