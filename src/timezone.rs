use chrono_tz::Tz;
use std::path::PathBuf;

fn config_path() -> PathBuf {
    iptv_core::paths::data_dir().join("timezone.json")
}

pub fn load_timezone() -> Tz {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            // stored as just a string like "America/Toronto"
            let tz_str: String = serde_json::from_str(&contents).unwrap_or_default();
            tz_str.parse().unwrap_or(Tz::UTC)
        }
        Err(_) => Tz::UTC,
    }
}

pub fn save_timezone(tz: Tz) -> anyhow::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(&tz.name())?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Common timezone list for the selector UI (most useful ones first, then all).
pub fn common_timezones() -> Vec<Tz> {
    // A curated list of common timezones
    vec![
        "US/Eastern".parse().unwrap(),
        "US/Central".parse().unwrap(),
        "US/Mountain".parse().unwrap(),
        "US/Pacific".parse().unwrap(),
        "America/Toronto".parse().unwrap(),
        "America/New_York".parse().unwrap(),
        "America/Chicago".parse().unwrap(),
        "America/Denver".parse().unwrap(),
        "America/Los_Angeles".parse().unwrap(),
        "America/Vancouver".parse().unwrap(),
        "America/Winnipeg".parse().unwrap(),
        "America/Halifax".parse().unwrap(),
        "America/St_Johns".parse().unwrap(),
        "Europe/London".parse().unwrap(),
        "Europe/Paris".parse().unwrap(),
        "Europe/Berlin".parse().unwrap(),
        "Europe/Amsterdam".parse().unwrap(),
        "Europe/Rome".parse().unwrap(),
        "Europe/Madrid".parse().unwrap(),
        "Europe/Moscow".parse().unwrap(),
        "Asia/Tokyo".parse().unwrap(),
        "Asia/Shanghai".parse().unwrap(),
        "Asia/Kolkata".parse().unwrap(),
        "Asia/Dubai".parse().unwrap(),
        "Asia/Singapore".parse().unwrap(),
        "Australia/Sydney".parse().unwrap(),
        "Australia/Melbourne".parse().unwrap(),
        "Australia/Perth".parse().unwrap(),
        "Pacific/Auckland".parse().unwrap(),
        "Pacific/Honolulu".parse().unwrap(),
        "UTC".parse().unwrap(),
    ]
}

/// Full list of all timezones for search.
pub fn all_timezones() -> Vec<Tz> {
    chrono_tz::TZ_VARIANTS.to_vec()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_timezones_not_empty() {
        let zones = common_timezones();
        assert!(!zones.is_empty());
    }

    #[test]
    fn test_common_timezones_contains_utc() {
        let zones = common_timezones();
        assert!(zones.iter().any(|z| z.name() == "UTC"));
    }

    #[test]
    fn test_common_timezones_contains_us_eastern() {
        let zones = common_timezones();
        assert!(zones.iter().any(|z| z.name() == "US/Eastern"));
    }

    #[test]
    fn test_common_timezones_contains_europe_london() {
        let zones = common_timezones();
        assert!(zones.iter().any(|z| z.name() == "Europe/London"));
    }
}
