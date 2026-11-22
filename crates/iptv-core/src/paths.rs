use std::path::PathBuf;

/// Directory holding favourites, cache and other persistent state.
///
/// Set `IPTV_DATA_DIR` to override (e.g. a mounted volume in a container);
/// otherwise defaults to `$HOME/.config/iptv`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("IPTV_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".config").join("iptv"),
        None => PathBuf::from(".config").join("iptv"),
    }
}

/// Test-only: point `data_dir()` at a throwaway per-process directory so tests
/// never touch the real `~/.config/iptv`, and serialise tests that share files.
/// Hold the returned guard for the duration of the test.
#[doc(hidden)]
pub fn test_data_dir() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, Once};
    static INIT: Once = Once::new();
    static LOCK: Mutex<()> = Mutex::new(());
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("iptv-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("IPTV_DATA_DIR", dir);
    });
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
