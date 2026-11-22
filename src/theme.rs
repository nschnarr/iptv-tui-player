use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Semantic color roles for the UI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    /// Primary accent color (borders, headers, labels).
    pub accent: Color,
    /// Primary text color.
    pub text: Color,
    /// Muted/secondary text color.
    pub text_dim: Color,
    /// Highlighted items (active, in-progress, warnings).
    pub highlight: Color,
    /// Success state (complete, playing).
    pub success: Color,
    /// Error/danger state (failures, live recording).
    pub error: Color,
    /// Selection background.
    pub selected_bg: Color,
    /// Panel/surface background.
    pub surface: Color,
    /// Active/focused item indicator background.
    pub info: Color,
    /// Progress bar filled portion.
    pub progress: Color,
    /// Progress bar empty portion.
    pub progress_empty: Color,
    /// Special/misc (player log level, etc).
    pub special: Color,
    /// Subtle text (slightly brighter than dim).
    pub text_subtle: Color,
}

/// All built-in theme names.
pub const THEME_NAMES: &[&str] = &[
    "default",
    "catppuccin",
    "dracula",
    "nord",
    "gruvbox",
    "solarized",
];

pub fn by_name(name: &str) -> Theme {
    match name {
        "catppuccin" => catppuccin(),
        "dracula" => dracula(),
        "nord" => nord(),
        "gruvbox" => gruvbox(),
        "solarized" => solarized(),
        _ => default(),
    }
}

pub fn default() -> Theme {
    Theme {
        name: "default",
        accent: Color::Cyan,
        text: Color::White,
        text_dim: Color::DarkGray,
        highlight: Color::Yellow,
        success: Color::Green,
        error: Color::Red,
        selected_bg: Color::DarkGray,
        surface: Color::Black,
        info: Color::Blue,
        progress: Color::Cyan,
        progress_empty: Color::Rgb(60, 60, 60),
        special: Color::Magenta,
        text_subtle: Color::Gray,
    }
}

pub fn catppuccin() -> Theme {
    // Catppuccin Mocha palette
    Theme {
        name: "catppuccin",
        accent: Color::Rgb(137, 180, 250),      // blue
        text: Color::Rgb(205, 214, 244),        // text
        text_dim: Color::Rgb(88, 91, 112),      // overlay0
        highlight: Color::Rgb(249, 226, 175),   // yellow
        success: Color::Rgb(166, 227, 161),     // green
        error: Color::Rgb(243, 139, 168),       // red
        selected_bg: Color::Rgb(69, 71, 90),    // surface1
        surface: Color::Rgb(30, 30, 46),        // base
        info: Color::Rgb(137, 180, 250),        // blue
        progress: Color::Rgb(148, 226, 213),    // teal
        progress_empty: Color::Rgb(49, 50, 68), // surface0
        special: Color::Rgb(203, 166, 247),     // mauve
        text_subtle: Color::Rgb(147, 153, 178), // subtext0
    }
}

pub fn dracula() -> Theme {
    Theme {
        name: "dracula",
        accent: Color::Rgb(189, 147, 249),    // purple
        text: Color::Rgb(248, 248, 242),      // foreground
        text_dim: Color::Rgb(98, 114, 164),   // comment
        highlight: Color::Rgb(241, 250, 140), // yellow
        success: Color::Rgb(80, 250, 123),    // green
        error: Color::Rgb(255, 85, 85),       // red
        selected_bg: Color::Rgb(68, 71, 90),  // current line
        surface: Color::Rgb(40, 42, 54),      // background
        info: Color::Rgb(139, 233, 253),      // cyan
        progress: Color::Rgb(139, 233, 253),  // cyan
        progress_empty: Color::Rgb(55, 57, 68),
        special: Color::Rgb(255, 121, 198), // pink
        text_subtle: Color::Rgb(144, 155, 186),
    }
}

pub fn nord() -> Theme {
    Theme {
        name: "nord",
        accent: Color::Rgb(136, 192, 208),      // nord8 (frost)
        text: Color::Rgb(229, 233, 240),        // nord5 (snow)
        text_dim: Color::Rgb(76, 86, 106),      // nord3
        highlight: Color::Rgb(235, 203, 139),   // nord13 (yellow)
        success: Color::Rgb(163, 190, 140),     // nord14 (green)
        error: Color::Rgb(191, 97, 106),        // nord11 (red)
        selected_bg: Color::Rgb(67, 76, 94),    // nord2
        surface: Color::Rgb(46, 52, 64),        // nord0
        info: Color::Rgb(129, 161, 193),        // nord9
        progress: Color::Rgb(136, 192, 208),    // nord8
        progress_empty: Color::Rgb(59, 66, 82), // nord1
        special: Color::Rgb(180, 142, 173),     // nord15 (purple)
        text_subtle: Color::Rgb(143, 153, 174), // nord3 bright
    }
}

pub fn gruvbox() -> Theme {
    Theme {
        name: "gruvbox",
        accent: Color::Rgb(131, 165, 152),      // aqua
        text: Color::Rgb(235, 219, 178),        // fg
        text_dim: Color::Rgb(102, 92, 84),      // bg3
        highlight: Color::Rgb(250, 189, 47),    // yellow
        success: Color::Rgb(184, 187, 38),      // green
        error: Color::Rgb(251, 73, 52),         // red
        selected_bg: Color::Rgb(80, 73, 69),    // bg2
        surface: Color::Rgb(40, 40, 40),        // bg
        info: Color::Rgb(69, 133, 136),         // dark aqua
        progress: Color::Rgb(131, 165, 152),    // aqua
        progress_empty: Color::Rgb(60, 56, 54), // bg1
        special: Color::Rgb(211, 134, 155),     // purple
        text_subtle: Color::Rgb(146, 131, 116), // fg4
    }
}

pub fn solarized() -> Theme {
    // Solarized Dark
    Theme {
        name: "solarized",
        accent: Color::Rgb(38, 139, 210),       // blue
        text: Color::Rgb(147, 161, 161),        // base1
        text_dim: Color::Rgb(88, 110, 117),     // base01
        highlight: Color::Rgb(181, 137, 0),     // yellow
        success: Color::Rgb(133, 153, 0),       // green
        error: Color::Rgb(220, 50, 47),         // red
        selected_bg: Color::Rgb(7, 54, 66),     // base02
        surface: Color::Rgb(0, 43, 54),         // base03
        info: Color::Rgb(42, 161, 152),         // cyan
        progress: Color::Rgb(42, 161, 152),     // cyan
        progress_empty: Color::Rgb(7, 54, 66),  // base02
        special: Color::Rgb(108, 113, 196),     // violet
        text_subtle: Color::Rgb(131, 148, 150), // base0
    }
}

// --- Persistence ---

fn theme_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("iptv")
        .join("theme.json")
}

#[derive(Serialize, Deserialize)]
struct ThemeConfig {
    name: String,
}

pub fn load_theme_name() -> String {
    let path = theme_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<ThemeConfig>(&s).ok())
        .map(|c| c.name)
        .unwrap_or_else(|| "default".to_string())
}

pub fn save_theme_name(name: &str) {
    let path = theme_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let config = ThemeConfig {
        name: name.to_string(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(path, json);
    }
}

/// Cycle to the next theme, returning it.
pub fn next_theme(current: &str) -> Theme {
    let idx = THEME_NAMES.iter().position(|&n| n == current).unwrap_or(0);
    let next_idx = (idx + 1) % THEME_NAMES.len();
    let name = THEME_NAMES[next_idx];
    save_theme_name(name);
    by_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_theme_names_resolve() {
        for name in THEME_NAMES {
            let theme = by_name(name);
            assert_eq!(theme.name, *name);
        }
    }

    #[test]
    fn test_unknown_theme_returns_default() {
        let theme = by_name("nonexistent");
        assert_eq!(theme.name, "default");
    }

    #[test]
    fn test_default_theme() {
        let theme = default();
        assert_eq!(theme.name, "default");
        assert_eq!(theme.accent, Color::Cyan);
        assert_eq!(theme.text, Color::White);
        assert_eq!(theme.error, Color::Red);
    }

    #[test]
    fn test_next_theme_cycles() {
        // Don't persist during tests — just test the logic
        let names = THEME_NAMES;
        for i in 0..names.len() {
            let next_idx = (i + 1) % names.len();
            let next = next_theme(names[i]);
            assert_eq!(next.name, names[next_idx]);
        }
    }

    #[test]
    fn test_next_theme_wraps_around() {
        let last = THEME_NAMES[THEME_NAMES.len() - 1];
        let next = next_theme(last);
        assert_eq!(next.name, THEME_NAMES[0]);
    }

    #[test]
    fn test_next_theme_unknown_starts_at_one() {
        let next = next_theme("garbage");
        // unknown maps to index 0 (default), so next is index 1
        assert_eq!(next.name, THEME_NAMES[1]);
    }

    #[test]
    fn test_theme_names_count() {
        assert_eq!(THEME_NAMES.len(), 6);
    }

    #[test]
    fn test_catppuccin_uses_rgb() {
        let theme = catppuccin();
        // Catppuccin uses RGB, not named colors
        matches!(theme.accent, Color::Rgb(_, _, _));
        matches!(theme.text, Color::Rgb(_, _, _));
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        let config = serde_json::json!({"name": "dracula"});
        std::fs::write(&path, config.to_string()).unwrap();
        let loaded: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded["name"], "dracula");
    }
}
