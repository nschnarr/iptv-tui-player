use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Max in-memory log buffer size in bytes (~10 MB).
const MAX_BUFFER_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Player,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERR "),
            LogLevel::Player => write!(f, "PLAY"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
}

impl LogEntry {
    /// Approximate memory footprint of this entry in bytes.
    fn size_bytes(&self) -> usize {
        // timestamp(8) + level(1) + source + message + String overhead(~48 each)
        57 + self.source.len() + self.message.len() + 96
    }

    /// Format for writing to disk.
    fn format(&self) -> String {
        format!(
            "{} [{}] {}: {}\n",
            self.timestamp.format("%Y-%m-%d %H:%M:%S"),
            self.level,
            self.source,
            self.message,
        )
    }
}

#[derive(Debug)]
struct LogInner {
    entries: Vec<LogEntry>,
    total_bytes: usize,
    flush_count: u32,
}

#[derive(Debug, Clone)]
pub struct AppLog {
    inner: Arc<Mutex<LogInner>>,
}

fn log_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("iptv")
            .join("logs")
    } else {
        PathBuf::from(".config").join("iptv").join("logs")
    }
}

impl AppLog {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogInner {
                entries: Vec::new(),
                total_bytes: 0,
                flush_count: 0,
            })),
        }
    }

    pub fn log(&self, level: LogLevel, source: &str, message: impl Into<String>) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            level,
            source: source.to_string(),
            message: message.into(),
        };
        if let Ok(mut inner) = self.inner.lock() {
            let entry_size = entry.size_bytes();
            inner.entries.push(entry);
            inner.total_bytes += entry_size;

            // If we've exceeded the buffer limit, flush to disk and reset
            if inner.total_bytes >= MAX_BUFFER_BYTES {
                flush_to_disk(&mut inner);
            }
        }
    }

    pub fn info(&self, source: &str, message: impl Into<String>) {
        self.log(LogLevel::Info, source, message);
    }

    #[allow(dead_code)]
    pub fn warn(&self, source: &str, message: impl Into<String>) {
        self.log(LogLevel::Warn, source, message);
    }

    #[allow(dead_code)]
    pub fn error(&self, source: &str, message: impl Into<String>) {
        self.log(LogLevel::Error, source, message);
    }

    pub fn player(&self, source: &str, message: impl Into<String>) {
        self.log(LogLevel::Player, source, message);
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.inner
            .lock()
            .map(|i| i.entries.clone())
            .unwrap_or_default()
    }

    pub fn entry_count(&self) -> usize {
        self.inner.lock().map(|i| i.entries.len()).unwrap_or(0)
    }
}

/// Flush all entries to a timestamped log file, then clear the buffer.
fn flush_to_disk(inner: &mut LogInner) {
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        // Can't write logs — just truncate to avoid OOM
        inner.entries.clear();
        inner.total_bytes = 0;
        return;
    }

    inner.flush_count += 1;
    let filename = format!(
        "iptv-{}-{}.log",
        Utc::now().format("%Y%m%d-%H%M%S"),
        inner.flush_count,
    );
    let path = dir.join(filename);

    if let Ok(mut file) = std::fs::File::create(&path) {
        for entry in &inner.entries {
            let _ = file.write_all(entry.format().as_bytes());
        }
    }

    inner.entries.clear();
    inner.total_bytes = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_log_empty() {
        let log = AppLog::new();
        assert_eq!(log.entry_count(), 0);
        assert!(log.entries().is_empty());
    }

    #[test]
    fn test_log_info() {
        let log = AppLog::new();
        log.info("test", "hello world");
        assert_eq!(log.entry_count(), 1);
        let entries = log.entries();
        assert_eq!(entries[0].source, "test");
        assert_eq!(entries[0].message, "hello world");
    }

    #[test]
    fn test_log_multiple_levels() {
        let log = AppLog::new();
        log.info("src", "info msg");
        log.warn("src", "warn msg");
        log.error("src", "error msg");
        log.player("src", "player msg");
        assert_eq!(log.entry_count(), 4);
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(format!("{}", LogLevel::Info), "INFO");
        assert_eq!(format!("{}", LogLevel::Warn), "WARN");
        assert_eq!(format!("{}", LogLevel::Error), "ERR ");
        assert_eq!(format!("{}", LogLevel::Player), "PLAY");
    }

    #[test]
    fn test_log_entry_format() {
        let entry = LogEntry {
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-01-15T10:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            level: LogLevel::Info,
            source: "test".to_string(),
            message: "hello".to_string(),
        };
        let formatted = entry.format();
        assert!(formatted.contains("2026-01-15 10:30:00"));
        assert!(formatted.contains("[INFO]"));
        assert!(formatted.contains("test"));
        assert!(formatted.contains("hello"));
    }

    #[test]
    fn test_log_entry_size_bytes() {
        let entry = LogEntry {
            timestamp: Utc::now(),
            level: LogLevel::Info,
            source: "src".to_string(),
            message: "msg".to_string(),
        };
        // Should be > 0 and account for string overhead
        assert!(entry.size_bytes() > 100);
    }

    #[test]
    fn test_log_clone_is_independent() {
        let log = AppLog::new();
        let log2 = log.clone();
        log.info("src", "from original");
        // Clone shares the Arc, so both see the entry
        assert_eq!(log2.entry_count(), 1);
    }

    #[test]
    fn test_log_entries_returns_cloned_vec() {
        let log = AppLog::new();
        log.info("src", "msg1");
        let entries = log.entries();
        log.info("src", "msg2");
        // Original snapshot shouldn't change
        assert_eq!(entries.len(), 1);
        assert_eq!(log.entry_count(), 2);
    }
}
