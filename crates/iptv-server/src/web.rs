//! Web UI + JSON API for browsing channels and managing favourites.

use crate::state::{AppState, SyncRequest};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use iptv_core::favorites::{self, favorite_key};
use iptv_core::model::ContentType;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

type AppStateRef = State<Arc<AppState>>;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/status", get(status))
        .route("/api/groups", get(groups))
        .route("/api/channels", get(channels))
        .route("/api/lineup", get(lineup))
        .route("/api/favorites", post(set_favorite))
        .route("/api/refresh", post(refresh))
        .route("/api/plex/sync", post(plex_sync))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-cache")],
        Html(include_str!("../static/index.html")),
    )
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

/// Run a blocking closure (playlist scans can touch 1M+ channels).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    Ok(tokio::task::spawn_blocking(f).await?)
}

async fn status(State(s): AppStateRef) -> Json<serde_json::Value> {
    let (channels, groups, favorites, lineup, device) = {
        let h = s.hdhr.lock().unwrap();
        (
            h.playlist().channels.len(),
            h.playlist().groups.len(),
            h.favorites().len(),
            h.lineup().len(),
            h.device.clone(),
        )
    };
    Json(json!({
        "load": *s.load.lock().unwrap(),
        "plex": *s.plex.lock().unwrap(),
        "channels": channels,
        "groups": groups,
        "favorites": favorites,
        "lineup": lineup,
        "tuner": {
            "port": s.hdhr_port,
            "device_id": device.device_id,
            "name": device.friendly_name,
            "tuner_count": device.tuner_count,
        },
    }))
}

#[derive(Serialize)]
struct GroupSummary {
    name: String,
    channels: usize,
    favorites: usize,
}

/// Live TV groups only: VOD/series entries can't be tuned by Plex.
async fn groups(State(s): AppStateRef) -> Result<Json<Vec<GroupSummary>>, ApiError> {
    let (playlist, favs) = {
        let h = s.hdhr.lock().unwrap();
        (Arc::clone(h.playlist()), h.favorites().clone())
    };
    blocking(move || {
        let mut out = Vec::new();
        for g in &playlist.groups {
            let (mut channels, mut favorites) = (0, 0);
            for &i in playlist.channels_in_group(g) {
                let ch = &playlist.channels[i];
                if ch.content_type == ContentType::Live {
                    channels += 1;
                    if favs.contains(&favorite_key(&ch.name, &ch.group)) {
                        favorites += 1;
                    }
                }
            }
            if channels > 0 {
                out.push(GroupSummary {
                    name: g.to_string(),
                    channels,
                    favorites,
                });
            }
        }
        Json(out)
    })
    .await
}

#[derive(Deserialize)]
struct ChannelQuery {
    group: Option<String>,
    q: Option<String>,
    #[serde(default)]
    favorites: bool,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct ChannelItem {
    name: String,
    group: String,
    logo: Option<String>,
    tvg_id: Option<String>,
    favorite: bool,
    number: Option<u32>,
}

async fn channels(
    State(s): AppStateRef,
    Query(q): Query<ChannelQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (playlist, favs, numbers) = {
        let h = s.hdhr.lock().unwrap();
        let numbers: HashMap<usize, u32> = h.lineup().iter().map(|c| (c.index, c.number)).collect();
        (Arc::clone(h.playlist()), h.favorites().clone(), numbers)
    };
    blocking(move || {
        let limit = q.limit.unwrap_or(100).min(500);
        let needle =
            q.q.as_deref()
                .map(str::to_lowercase)
                .filter(|n| !n.is_empty());
        let candidates: Box<dyn Iterator<Item = usize>> = match q.group.as_deref() {
            Some(g) if !g.is_empty() => Box::new(playlist.channels_in_group(g).iter().copied()),
            _ => Box::new(0..playlist.channels.len()),
        };
        let mut total = 0;
        let mut items = Vec::new();
        for i in candidates {
            let ch = &playlist.channels[i];
            if ch.content_type != ContentType::Live {
                continue;
            }
            let favorite = favs.contains(&favorite_key(&ch.name, &ch.group));
            if q.favorites && !favorite {
                continue;
            }
            if let Some(n) = &needle {
                if !ch.name.to_lowercase().contains(n.as_str()) {
                    continue;
                }
            }
            total += 1;
            if total > q.offset && items.len() < limit {
                items.push(ChannelItem {
                    name: ch.name.to_string(),
                    group: ch.group.to_string(),
                    logo: ch.logo_url.as_deref().map(str::to_string),
                    tvg_id: ch.tvg_id.as_deref().map(str::to_string),
                    favorite,
                    number: numbers.get(&i).copied(),
                });
            }
        }
        Json(json!({ "total": total, "offset": q.offset, "items": items }))
    })
    .await
}

async fn lineup(State(s): AppStateRef) -> Json<serde_json::Value> {
    let h = s.hdhr.lock().unwrap();
    Json(json!(h.lineup()))
}

#[derive(Deserialize)]
struct FavoriteBody {
    name: String,
    group: String,
    favorite: bool,
}

async fn set_favorite(
    State(s): AppStateRef,
    Json(body): Json<FavoriteBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let key = favorite_key(&body.name, &body.group);
    let number = {
        let mut h = s.hdhr.lock().unwrap();
        let mut favs = h.favorites().clone();
        let changed = if body.favorite {
            favs.insert(key.clone())
        } else {
            favs.remove(&key)
        };
        if changed {
            favorites::save_favorites(&favs)?;
            h.set_favorites(favs);
        }
        h.lineup()
            .iter()
            .find(|c| c.name == body.name && c.group == body.group)
            .map(|c| c.number)
    };
    s.lineup_changed();
    Ok(Json(json!({ "favorite": body.favorite, "number": number })))
}

async fn refresh(State(s): AppStateRef) -> Result<StatusCode, ApiError> {
    s.refresh_tx
        .send(())
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "loader stopped".into()))?;
    s.load.lock().unwrap().loading = true;
    Ok(StatusCode::ACCEPTED)
}

async fn plex_sync(State(s): AppStateRef) -> Result<StatusCode, ApiError> {
    if s.request_sync(SyncRequest::Now) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Plex is not configured (set PLEX_URL and PLEX_TOKEN)".into(),
        ))
    }
}
