//! Push tuner lineup changes into a Plex DVR so new favourites show up in
//! Live TV without re-running the channel setup in Plex.
//!
//! Plex's DVR endpoints are undocumented. The flow mirrors what Plex Web does
//! when you edit a DVR's channels:
//!   1. GET  /livetv/dvrs                                  find our DVR + device
//!   2. POST /livetv/dvrs/{dvr}/reloadGuide                re-read xmltv.xml
//!   3. GET  /media/grabbers/devices/{dev}/channels        re-read lineup.json
//!   4. PUT  /media/grabbers/devices/{dev}/channelmap      enable + map channels
//!   5. POST /livetv/dvrs/{dvr}/reloadGuide                pick up new mapping
//!
//! One-time prerequisite: add this tuner as a DVR in Plex with
//! `http://<tuner>/xmltv.xml` as its XMLTV guide. Our XMLTV channel IDs equal
//! the guide numbers, so device channel N maps to guide channel N.

use crate::config::PlexConfig;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

pub struct PlexClient {
    cfg: PlexConfig,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub dvr_id: String,
    pub device_key: String,
    pub channels_enabled: usize,
    /// Step-by-step log, shown in the web UI to diagnose Plex quirks.
    pub steps: Vec<String>,
}

struct DvrMatch {
    dvr_key: String,
    device_key: String,
    /// Existing device-channel -> guide-channel mappings to preserve.
    existing: HashMap<String, String>,
}

impl PlexClient {
    pub fn new(cfg: PlexConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(60))
            .build();
        Self { cfg, agent }
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        self.agent
            .request(method, &format!("{}{}", self.cfg.url, path))
            .set("Accept", "application/json")
            .set("X-Plex-Token", &self.cfg.token)
            .set("X-Plex-Client-Identifier", "iptv-server")
            .set("X-Plex-Product", "iptv-server")
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let resp = self.request("GET", path).call().map_err(describe)?;
        let body = resp.into_string()?;
        serde_json::from_str(&body).with_context(|| format!("Invalid JSON from Plex {path}"))
    }

    /// Sync the given guide numbers (the tuner's current lineup) into Plex.
    pub fn sync(&self, device_id: &str, lineup: &[u32]) -> Result<SyncReport> {
        let mut steps = Vec::new();

        let dvrs = self.get_json("/livetv/dvrs")?;
        let m = find_dvr(&dvrs, device_id, self.cfg.dvr_id.as_deref())?;
        steps.push(format!(
            "Found DVR {} with tuner device {}",
            m.dvr_key, m.device_key
        ));

        self.reload_guide(&m.dvr_key)?;
        steps.push("Requested guide reload (picks up new XMLTV channels)".into());

        let channels = self.get_json(&format!(
            "/media/grabbers/devices/{}/channels",
            m.device_key
        ))?;
        let plex_sees: Vec<String> = arr(&channels["MediaContainer"]["DeviceChannel"])
            .iter()
            .filter_map(|c| str_field(c, &["identifier", "channelIdentifier"]))
            .collect();
        steps.push(format!(
            "Plex reads {} channel(s) from the tuner lineup",
            plex_sees.len()
        ));

        let wanted: Vec<String> = lineup.iter().map(|n| n.to_string()).collect();
        let missing: Vec<&String> = wanted.iter().filter(|w| !plex_sees.contains(w)).collect();
        if !missing.is_empty() && !plex_sees.is_empty() {
            steps.push(format!(
                "Warning: Plex doesn't list channel(s) {missing:?} yet; they may appear on the next sync"
            ));
        }

        let mut query: Vec<(String, String)> = Vec::new();
        for id in &wanted {
            let guide = m.existing.get(id).cloned().unwrap_or_else(|| id.clone());
            query.push((format!("channelMappingByKey[{id}]"), guide.clone()));
            query.push((format!("channelMapping[{id}]"), guide));
        }
        query.push(("channelsEnabled".into(), wanted.join(",")));

        let mut req = self.request(
            "PUT",
            &format!("/media/grabbers/devices/{}/channelmap", m.device_key),
        );
        for (k, v) in &query {
            req = req.query(k, v);
        }
        req.call().map_err(describe)?;
        steps.push(format!(
            "Updated channel map: {} channel(s) enabled",
            wanted.len()
        ));

        self.reload_guide(&m.dvr_key)?;
        steps.push("Requested final guide reload".into());

        Ok(SyncReport {
            dvr_id: m.dvr_key,
            device_key: m.device_key,
            channels_enabled: wanted.len(),
            steps,
        })
    }

    fn reload_guide(&self, dvr_key: &str) -> Result<()> {
        self.request("POST", &format!("/livetv/dvrs/{dvr_key}/reloadGuide"))
            .call()
            .map_err(describe)?;
        Ok(())
    }
}

/// Plex returns single objects or arrays depending on count; normalise.
fn arr(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Read a field that Plex may send as a string or a number.
fn str_field(v: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| match &v[*n] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn find_dvr(dvrs: &Value, device_id: &str, dvr_id: Option<&str>) -> Result<DvrMatch> {
    let list = arr(&dvrs["MediaContainer"]["Dvr"]);
    if list.is_empty() {
        bail!("Plex has no DVRs set up. Add this tuner in Plex (Settings → Live TV & DVR) first");
    }
    let wanted_id = device_id.to_ascii_lowercase();
    for dvr in &list {
        let Some(dvr_key) = str_field(dvr, &["key"]) else {
            continue;
        };
        if dvr_id.is_some_and(|id| id != dvr_key) {
            continue;
        }
        for dev in arr(&dvr["Device"]) {
            let uuid = str_field(&dev, &["uuid"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            let matches_device = uuid.ends_with(&format!("/{wanted_id}")) || uuid == wanted_id;
            if dvr_id.is_none() && !matches_device {
                continue;
            }
            let device_key = str_field(&dev, &["key"])
                .ok_or_else(|| anyhow!("Plex device on DVR {dvr_key} has no key"))?;
            let existing = arr(&dev["ChannelMapping"])
                .iter()
                .filter_map(|cm| {
                    Some((
                        str_field(cm, &["channelKey", "deviceIdentifier"])?,
                        str_field(cm, &["lineupIdentifier"])?,
                    ))
                })
                .collect();
            return Ok(DvrMatch {
                dvr_key,
                device_key,
                existing,
            });
        }
    }
    match dvr_id {
        Some(id) => bail!("PLEX_DVR_ID={id} not found (or it has no tuner device)"),
        None => bail!(
            "No Plex DVR uses tuner device ID {device_id}. Add this tuner in Plex, \
             or set PLEX_DVR_ID"
        ),
    }
}

/// Turn ureq errors into messages that say what Plex returned.
fn describe(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(401, _) => anyhow!("Plex rejected the token (401); check PLEX_TOKEN"),
        ureq::Error::Status(code, resp) => {
            let url = resp.get_url().split('?').next().unwrap_or("").to_string();
            let body: String = resp
                .into_string()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            anyhow!("Plex returned HTTP {code} for {url}: {body}")
        }
        ureq::Error::Transport(t) => anyhow!("Can't reach Plex: {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dvrs() -> Value {
        json!({"MediaContainer": {"Dvr": [
            {"key": "5", "Device": {"key": "9", "uuid": "device://tv.plex.grabbers.hdhomerun/OTHER"}},
            {"key": "7", "Device": [{"key": "12",
                "uuid": "device://tv.plex.grabbers.hdhomerun/1F7E5E07",
                "ChannelMapping": [{"channelKey": "1", "lineupIdentifier": "1"},
                                   {"channelKey": 2, "lineupIdentifier": "custom"}]}]}
        ]}})
    }

    #[test]
    fn finds_dvr_by_device_id_case_insensitive() {
        let m = find_dvr(&dvrs(), "1f7e5e07", None).unwrap();
        assert_eq!((m.dvr_key.as_str(), m.device_key.as_str()), ("7", "12"));
        assert_eq!(m.existing.get("2").map(String::as_str), Some("custom"));
    }

    #[test]
    fn explicit_dvr_id_wins() {
        let m = find_dvr(&dvrs(), "nomatch", Some("5")).unwrap();
        assert_eq!(m.device_key, "9");
    }

    #[test]
    fn errors_when_no_match() {
        assert!(find_dvr(&dvrs(), "nomatch", None).is_err());
        assert!(find_dvr(&json!({"MediaContainer": {}}), "x", None).is_err());
    }
}
