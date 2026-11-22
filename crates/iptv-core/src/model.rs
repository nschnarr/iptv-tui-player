use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum ContentType {
    Live,
    Movie,
    Series {
        series_name: Arc<str>,
        season: u32,
        episode: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub name: Arc<str>,
    pub url: Arc<str>,
    pub group: Arc<str>,
    #[allow(dead_code)]
    pub logo_url: Option<Arc<str>>,
    pub tvg_id: Option<Arc<str>>,
    #[allow(dead_code)]
    pub tvg_name: Option<Arc<str>>,
    pub tvg_language: Option<Arc<str>>,
    pub tvg_country: Option<Arc<str>>,
    pub content_type: ContentType,
    #[allow(dead_code)]
    pub index: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Playlist {
    pub channels: Vec<Channel>,
    pub groups: Vec<Arc<str>>,
    pub group_indices: HashMap<Arc<str>, Vec<usize>>,
}

impl Playlist {
    pub fn channels_in_group(&self, group: &str) -> &[usize] {
        self.group_indices
            .get(group)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn stats(&self) -> String {
        format!(
            "{} channels across {} groups",
            self.channels.len(),
            self.groups.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_playlist() -> Playlist {
        let mut pl = Playlist::default();
        let uk: Arc<str> = Arc::from("UK");
        let news: Arc<str> = Arc::from("News");
        pl.groups = vec![uk.clone(), news.clone()];
        pl.channels = vec![
            Channel {
                name: Arc::from("BBC One"),
                url: Arc::from("http://bbc1"),
                group: uk.clone(),
                logo_url: None,
                tvg_id: None,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                content_type: ContentType::Live,
                index: 0,
            },
            Channel {
                name: Arc::from("BBC Two"),
                url: Arc::from("http://bbc2"),
                group: uk.clone(),
                logo_url: None,
                tvg_id: None,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                content_type: ContentType::Live,
                index: 1,
            },
            Channel {
                name: Arc::from("CNN"),
                url: Arc::from("http://cnn"),
                group: news.clone(),
                logo_url: None,
                tvg_id: None,
                tvg_name: None,
                tvg_language: None,
                tvg_country: None,
                content_type: ContentType::Live,
                index: 2,
            },
        ];
        pl.group_indices.insert(uk, vec![0, 1]);
        pl.group_indices.insert(news, vec![2]);
        pl
    }

    #[test]
    fn test_channels_in_group() {
        let pl = make_playlist();
        assert_eq!(pl.channels_in_group("UK"), &[0, 1]);
        assert_eq!(pl.channels_in_group("News"), &[2]);
        assert_eq!(pl.channels_in_group("NonExistent"), &[] as &[usize]);
    }

    #[test]
    fn test_stats() {
        let pl = make_playlist();
        assert_eq!(pl.stats(), "3 channels across 2 groups");
    }

    #[test]
    fn test_empty_playlist_stats() {
        let pl = Playlist::default();
        assert_eq!(pl.stats(), "0 channels across 0 groups");
    }

    #[test]
    fn test_content_type_eq() {
        assert_eq!(ContentType::Live, ContentType::Live);
        assert_eq!(ContentType::Movie, ContentType::Movie);
        assert_ne!(ContentType::Live, ContentType::Movie);
        let s1 = ContentType::Series {
            series_name: Arc::from("X"),
            season: 1,
            episode: 1,
        };
        let s2 = ContentType::Series {
            series_name: Arc::from("X"),
            season: 1,
            episode: 1,
        };
        let s3 = ContentType::Series {
            series_name: Arc::from("Y"),
            season: 1,
            episode: 1,
        };
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }
}
