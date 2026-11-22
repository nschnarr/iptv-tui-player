use crate::model::{Channel, ContentType, Playlist};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Xtream Codes API provider configuration.
#[derive(Debug, Clone)]
pub struct XtreamProvider {
    pub server: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
struct XtreamCategory {
    #[serde(deserialize_with = "deserialize_string_or_number")]
    category_id: String,
    category_name: String,
}

#[derive(Debug, Deserialize)]
struct XtreamStream {
    name: String,
    #[serde(deserialize_with = "deserialize_u64_or_string")]
    stream_id: u64,
    stream_icon: Option<String>,
    epg_channel_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_number")]
    category_id: Option<String>,
    container_extension: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XtreamSeries {
    #[serde(deserialize_with = "deserialize_u64_or_string")]
    series_id: u64,
    name: String,
    cover: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_number")]
    category_id: Option<String>,
}

/// Deserialize a value that may be a string or a number into a String.
fn deserialize_string_or_number<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err(serde::de::Error::custom("expected string or number")),
    }
}

fn deserialize_u64_or_string<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("invalid number")),
        serde_json::Value::String(s) => s
            .parse::<u64>()
            .map_err(|_| serde::de::Error::custom("invalid number string")),
        _ => Err(serde::de::Error::custom("expected number or string")),
    }
}

fn deserialize_optional_string_or_number<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(serde_json::Value::Number(n)) => Ok(Some(n.to_string())),
        _ => Ok(None),
    }
}

impl XtreamProvider {
    fn api_url(&self, action: &str) -> String {
        format!(
            "{}/player_api.php?username={}&password={}&action={}",
            self.server, self.username, self.password, action
        )
    }

    fn live_stream_url(&self, stream_id: u64) -> String {
        format!(
            "{}/live/{}/{}/{}.ts",
            self.server, self.username, self.password, stream_id
        )
    }

    fn vod_stream_url(&self, stream_id: u64, ext: &str) -> String {
        let ext = if ext.is_empty() { "mp4" } else { ext };
        format!(
            "{}/movie/{}/{}/{}.{}",
            self.server, self.username, self.password, stream_id, ext
        )
    }

    /// Fetch the full playlist from the Xtream Codes API.
    pub fn fetch_playlist(&self) -> Result<Playlist> {
        let agent = ureq::agent();

        // Fetch categories (build id → name maps)
        let live_cats = self.fetch_categories(&agent, "get_live_categories")?;
        let vod_cats = self.fetch_categories(&agent, "get_vod_categories")?;
        let series_cats = self.fetch_categories(&agent, "get_series_categories")?;

        let mut cat_map: HashMap<String, Arc<str>> = HashMap::new();
        for cat in live_cats
            .iter()
            .chain(vod_cats.iter())
            .chain(series_cats.iter())
        {
            cat_map
                .entry(cat.category_id.clone())
                .or_insert_with(|| Arc::from(cat.category_name.as_str()));
        }

        let uncategorized: Arc<str> = Arc::from("Uncategorized");
        let mut channels: Vec<Channel> = Vec::new();

        // Live streams
        let live_streams: Vec<XtreamStream> = self.fetch_json(&agent, "get_live_streams")?;
        for s in &live_streams {
            let group = s
                .category_id
                .as_ref()
                .and_then(|id| cat_map.get(id))
                .cloned()
                .unwrap_or_else(|| Arc::clone(&uncategorized));
            let idx = channels.len();
            channels.push(Channel {
                name: Arc::from(s.name.as_str()),
                url: Arc::from(self.live_stream_url(s.stream_id).as_str()),
                group,
                logo_url: non_empty_arc(&s.stream_icon),
                tvg_id: non_empty_arc(&s.epg_channel_id),
                content_type: ContentType::Live,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                index: idx,
            });
        }

        // VOD streams
        let vod_streams: Vec<XtreamStream> = self.fetch_json(&agent, "get_vod_streams")?;
        for s in &vod_streams {
            let group = s
                .category_id
                .as_ref()
                .and_then(|id| cat_map.get(id))
                .cloned()
                .unwrap_or_else(|| Arc::clone(&uncategorized));
            let ext = s.container_extension.as_deref().unwrap_or("mp4");
            let idx = channels.len();
            channels.push(Channel {
                name: Arc::from(s.name.as_str()),
                url: Arc::from(self.vod_stream_url(s.stream_id, ext).as_str()),
                group,
                logo_url: non_empty_arc(&s.stream_icon),
                tvg_id: non_empty_arc(&s.epg_channel_id),
                content_type: ContentType::Movie,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                index: idx,
            });
        }

        // Series
        let series_list: Vec<XtreamSeries> = self.fetch_json(&agent, "get_series")?;
        for s in &series_list {
            let group = s
                .category_id
                .as_ref()
                .and_then(|id| cat_map.get(id))
                .cloned()
                .unwrap_or_else(|| Arc::clone(&uncategorized));
            let idx = channels.len();
            channels.push(Channel {
                name: Arc::from(s.name.as_str()),
                url: Arc::from(
                    format!(
                        "{}/player_api.php?username={}&password={}&action=get_series_info&series_id={}",
                        self.server, self.username, self.password, s.series_id
                    )
                    .as_str(),
                ),
                group,
                logo_url: non_empty_arc(&s.cover),
                tvg_id: None,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                content_type: ContentType::Series {
                    series_name: Arc::from(s.name.as_str()),
                    season: 0,
                    episode: 0,
                },
                index: idx,
            });
        }

        // Build group indices
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

    fn fetch_categories(&self, agent: &ureq::Agent, action: &str) -> Result<Vec<XtreamCategory>> {
        self.fetch_json(agent, action)
    }

    fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        agent: &ureq::Agent,
        action: &str,
    ) -> Result<Vec<T>> {
        let url = self.api_url(action);
        let body = agent
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(crate::redact::http_error)
            .with_context(|| format!("Failed to fetch {action}"))?
            .into_string()
            .with_context(|| format!("Failed to read response for {action}"))?;
        let items: Vec<T> = serde_json::from_str(&body)
            .with_context(|| format!("Failed to parse JSON for {action}"))?;
        Ok(items)
    }
}

fn non_empty_arc(opt: &Option<String>) -> Option<Arc<str>> {
    opt.as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| Arc::from(s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_live_stream_json() {
        let json = r#"[
            {
                "num": 1,
                "name": "CNN",
                "stream_id": 101,
                "stream_icon": "http://logo.com/cnn.png",
                "epg_channel_id": "CNN.us",
                "category_id": "1",
                "container_extension": null
            },
            {
                "num": 2,
                "name": "BBC",
                "stream_id": 102,
                "stream_icon": "",
                "epg_channel_id": null,
                "category_id": "1"
            }
        ]"#;
        let streams: Vec<XtreamStream> = serde_json::from_str(json).unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].name, "CNN");
        assert_eq!(streams[0].stream_id, 101);
        assert_eq!(streams[0].epg_channel_id.as_deref(), Some("CNN.us"));
        assert_eq!(streams[1].epg_channel_id, None);
    }

    #[test]
    fn test_parse_category_json() {
        let json = r#"[
            {"category_id": "1", "category_name": "News"},
            {"category_id": 2, "category_name": "Sports"}
        ]"#;
        let cats: Vec<XtreamCategory> = serde_json::from_str(json).unwrap();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].category_id, "1");
        assert_eq!(cats[1].category_id, "2");
    }

    #[test]
    fn test_parse_series_json() {
        let json = r#"[
            {
                "series_id": 50,
                "name": "Breaking Bad",
                "cover": "http://img.com/bb.jpg",
                "category_id": "5"
            }
        ]"#;
        let series: Vec<XtreamSeries> = serde_json::from_str(json).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].series_id, 50);
        assert_eq!(series[0].name, "Breaking Bad");
    }

    #[test]
    fn test_api_url() {
        let provider = XtreamProvider {
            server: "http://example.com:8080".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        assert_eq!(
            provider.api_url("get_live_streams"),
            "http://example.com:8080/player_api.php?username=user&password=pass&action=get_live_streams"
        );
    }

    #[test]
    fn test_stream_urls() {
        let provider = XtreamProvider {
            server: "http://example.com:8080".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        assert_eq!(
            provider.live_stream_url(101),
            "http://example.com:8080/live/user/pass/101.ts"
        );
        assert_eq!(
            provider.vod_stream_url(202, "mkv"),
            "http://example.com:8080/movie/user/pass/202.mkv"
        );
        assert_eq!(
            provider.vod_stream_url(303, ""),
            "http://example.com:8080/movie/user/pass/303.mp4"
        );
    }
}
