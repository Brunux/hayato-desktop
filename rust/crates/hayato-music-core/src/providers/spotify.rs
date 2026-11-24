//! Modular Spotify provider (`SpotifyProvider`) implementing [`MusicProvider`]
//! with full Spotify 2026 Web API support:
//!
//! - HTTP status-aware `curl` (`204`, `401` auto-refresh + retry, `403` detection)
//! - Real playlist track counts (`items.total` falling back to `tracks.total`)
//! - 2026 `GET /v1/playlists/{id}/items` endpoint (`entry["item"]` falling back to `entry["track"]`)
//! - `403 Forbidden` editorial/followed playlist handling via `CollectionItems::Restricted`
//!   so pressing `P` / `Enter` plays directly via `context_uri: "spotify:playlist:<id>"`
//! - `GET /v1/me/tracks` (**♥ Liked Songs**)
//! - `GET /v1/me/player` live Connect state sync + album art caching for Sixel rendering
//! - Full remote transport (`play`, `context_uri` + `offset`, `pause`, `resume`, `next`, `prev`,
//!   `seek`, `volume`, `shuffle`, `repeat`, `devices`, `transfer`).

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::providers::{
    Collection, CollectionItems, CollectionKind, MusicProvider, PlaybackDevice, PlaybackSnapshot,
    ProviderCaps, ProviderId, SearchResults,
};
use crate::spotify::{self, SpotifyToken};
use crate::{PlayState, RepeatMode, SourceKind, Track, TrackId, cache_dir, fnv1a_64};

#[derive(Debug)]
enum ApiError {
    Http { status: u16, message: String },
    Network(String),
}

impl ApiError {
    fn into_string(self) -> String {
        match self {
            Self::Http { status, message } => format!("Spotify HTTP {status}: {message}"),
            Self::Network(s) => s,
        }
    }
}

pub struct SpotifyProvider {
    token: Option<SpotifyToken>,
    liked_total: Option<u32>,
}

impl SpotifyProvider {
    pub fn from_saved() -> Self {
        Self {
            token: spotify::load_token(),
            liked_total: None,
        }
    }

    pub fn account_summary(&self) -> Option<(String, bool)> {
        self.token
            .as_ref()
            .map(|t| (t.display_name.clone(), t.is_premium))
    }

    pub fn login_pkce(&mut self, client_id: &str) -> Result<(String, bool), String> {
        let tok = spotify::login_pkce(client_id)?;
        let summary = (tok.display_name.clone(), tok.is_premium);
        self.token = Some(tok);
        Ok(summary)
    }

    pub fn logout(&mut self) {
        spotify::clear_token();
        self.token = None;
        self.liked_total = None;
    }

    fn token_mut(&mut self) -> Result<&mut SpotifyToken, String> {
        self.token
            .as_mut()
            .ok_or_else(|| "Not signed in to Spotify".to_string())
    }

    fn get_json(&mut self, url: &str) -> Result<Option<Value>, ApiError> {
        let tok = self
            .token
            .as_mut()
            .ok_or_else(|| ApiError::Network("Not signed in to Spotify".into()))?;
        spotify::ensure_fresh_public(tok).map_err(ApiError::Network)?;

        match raw_curl("GET", url, &tok.access_token, None) {
            Err(ApiError::Http { status: 401, .. }) => {
                // Force refresh and retry once
                tok.expires_at_unix = 0;
                spotify::ensure_fresh_public(tok).map_err(ApiError::Network)?;
                raw_curl("GET", url, &tok.access_token, None)
            }
            other => other,
        }
    }

    fn send_action(
        &mut self,
        method: &str,
        url: &str,
        body: Option<&str>,
    ) -> Result<(), String> {
        let tok = self.token_mut()?;
        spotify::ensure_fresh_public(tok)?;
        match raw_curl(method, url, &tok.access_token, body) {
            Ok(_) => Ok(()),
            Err(ApiError::Http { status: 401, .. }) => {
                tok.expires_at_unix = 0;
                spotify::ensure_fresh_public(tok)?;
                raw_curl(method, url, &tok.access_token, body)
                    .map(|_| ())
                    .map_err(ApiError::into_string)
            }
            Err(ApiError::Http { status: 404, .. }) => {
                Err("No active Spotify Connect device — press D to pick a device or open Spotify on your phone".into())
            }
            Err(e) => Err(e.into_string()),
        }
    }
}

impl MusicProvider for SpotifyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Spotify
    }

    fn name(&self) -> &'static str {
        "Spotify"
    }

    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            remote_devices: true,
            remote_playback_sync: true,
            liked_tracks: true,
            playlist_context_play: true,
            seek: true,
            volume: true,
            live_pcm_fft: false,
        }
    }

    fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    fn list_collections(&mut self) -> Result<Vec<Collection>, String> {
        if !self.is_authenticated() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();

        // 1. Pinned Liked Songs total (lightweight limit=1 probe)
        if let Ok(Some(liked)) = self.get_json("https://api.spotify.com/v1/me/tracks?limit=1") {
            self.liked_total = liked["total"].as_u64().map(|n| n as u32);
        }
        out.push(Collection {
            id: "spotify:liked".to_string(),
            provider: ProviderId::Spotify,
            kind: CollectionKind::LikedSongs,
            name: "Liked Songs".to_string(),
            owner: self.token.as_ref().map(|t| t.display_name.clone()),
            track_count: self.liked_total,
            context_uri: None,
            tracks_restricted: false,
        });

        // 2. User playlists (Spotify 2026: reads `items.total` with `tracks.total` fallback)
        if let Some(v) = self
            .get_json("https://api.spotify.com/v1/me/playlists?limit=50")
            .map_err(ApiError::into_string)?
        {
            out.extend(parse_playlist_collections(&v));
        }

        Ok(out)
    }

    fn collection_items(&mut self, collection: &Collection) -> Result<CollectionItems, String> {
        if collection.kind == CollectionKind::LikedSongs {
            let v = self
                .get_json("https://api.spotify.com/v1/me/tracks?limit=50")
                .map_err(ApiError::into_string)?;
            let tracks = v.as_ref().map(parse_items_array).unwrap_or_default();
            return Ok(CollectionItems::Tracks(tracks));
        }

        let raw_id = collection
            .id
            .strip_prefix("spotify:playlist:")
            .or_else(|| collection.id.strip_prefix("spotify:"))
            .unwrap_or(&collection.id);
        let ctx_uri = collection
            .context_uri
            .clone()
            .unwrap_or_else(|| format!("spotify:playlist:{raw_id}"));

        // Spotify 2026 primary endpoint: `/v1/playlists/{id}/items`
        let url_2026 = format!("https://api.spotify.com/v1/playlists/{raw_id}/items?limit=50");
        match self.get_json(&url_2026) {
            Ok(Some(v)) => Ok(CollectionItems::Tracks(parse_items_array(&v))),
            Ok(None) => Ok(CollectionItems::Tracks(Vec::new())),
            Err(ApiError::Http { status: 403, .. }) => Ok(CollectionItems::Restricted {
                context_uri: ctx_uri,
                reason: "Spotify restricts track listing for this editorial/followed playlist — press [P] or [Enter] to play it directly on your active Connect device.".into(),
            }),
            Err(ApiError::Http { status: 404 | 405, .. }) => {
                // Legacy fallback `/v1/playlists/{id}/tracks`
                let legacy =
                    format!("https://api.spotify.com/v1/playlists/{raw_id}/tracks?limit=50");
                match self.get_json(&legacy) {
                    Ok(Some(v)) => Ok(CollectionItems::Tracks(parse_items_array(&v))),
                    Ok(None) => Ok(CollectionItems::Tracks(Vec::new())),
                    Err(ApiError::Http { status: 403, .. }) => Ok(CollectionItems::Restricted {
                        context_uri: ctx_uri,
                        reason: "Spotify restricts track listing for this playlist — press [P] or [Enter] to play directly on Connect.".into(),
                    }),
                    Err(e) => Err(e.into_string()),
                }
            }
            Err(e) => Err(e.into_string()),
        }
    }

    fn search(&mut self, query: &str, limit: usize) -> Result<SearchResults, String> {
        let q = query.trim();
        if q.is_empty() || !self.is_authenticated() {
            return Ok(SearchResults::default());
        }
        let url = format!(
            "https://api.spotify.com/v1/search?type=track,playlist&limit={}&q={}",
            limit.clamp(1, 20),
            spotify::url_encode_public(q)
        );
        let Some(v) = self.get_json(&url).map_err(ApiError::into_string)? else {
            return Ok(SearchResults::default());
        };
        let mut tracks = Vec::new();
        if let Some(arr) = v["tracks"]["items"].as_array() {
            for item in arr {
                if let Some(t) = parse_track_obj(item, false) {
                    tracks.push(t);
                }
            }
        }
        let collections = parse_playlist_collections(&v["playlists"]);
        Ok(SearchResults {
            tracks,
            collections,
        })
    }

    fn play_track(&mut self, track: &Track, device_id: Option<&str>) -> Result<(), String> {
        let SourceKind::Spotify { uri, url } = &track.source else {
            return Err("Not a Spotify track".into());
        };
        if let Some(tok) = &self.token
            && !tok.is_premium
        {
            let target = if url.is_empty() { uri } else { url };
            let _ = Command::new("xdg-open")
                .arg(target)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            return Err("Spotify Free account: opened track in Firefox".into());
        }
        let endpoint = match device_id {
            Some(id) => format!(
                "https://api.spotify.com/v1/me/player/play?device_id={}",
                spotify::url_encode_public(id)
            ),
            None => "https://api.spotify.com/v1/me/player/play".to_string(),
        };
        let body = serde_json::json!({ "uris": [uri] }).to_string();
        self.send_action("PUT", &endpoint, Some(&body))
    }

    fn play_context(
        &mut self,
        context_uri: &str,
        offset_uri: Option<&str>,
        device_id: Option<&str>,
    ) -> Result<(), String> {
        let endpoint = match device_id {
            Some(id) => format!(
                "https://api.spotify.com/v1/me/player/play?device_id={}",
                spotify::url_encode_public(id)
            ),
            None => "https://api.spotify.com/v1/me/player/play".to_string(),
        };
        let body = match offset_uri {
            Some(off) => serde_json::json!({
                "context_uri": context_uri,
                "offset": { "uri": off }
            }),
            None => serde_json::json!({ "context_uri": context_uri }),
        }
        .to_string();
        self.send_action("PUT", &endpoint, Some(&body))
    }

    fn pause(&mut self) -> Result<(), String> {
        self.send_action("PUT", "https://api.spotify.com/v1/me/player/pause", None)
    }

    fn resume(&mut self) -> Result<(), String> {
        self.send_action("PUT", "https://api.spotify.com/v1/me/player/play", None)
    }

    fn next(&mut self) -> Result<(), String> {
        self.send_action("POST", "https://api.spotify.com/v1/me/player/next", None)
    }

    fn prev(&mut self) -> Result<(), String> {
        self.send_action(
            "POST",
            "https://api.spotify.com/v1/me/player/previous",
            None,
        )
    }

    fn seek_ms(&mut self, pos_ms: u64) -> Result<(), String> {
        let url = format!("https://api.spotify.com/v1/me/player/seek?position_ms={pos_ms}");
        self.send_action("PUT", &url, None)
    }

    fn set_volume(&mut self, pct: u8) -> Result<(), String> {
        let url = format!(
            "https://api.spotify.com/v1/me/player/volume?volume_percent={}",
            pct.min(100)
        );
        self.send_action("PUT", &url, None)
    }

    fn set_shuffle(&mut self, on: bool) -> Result<(), String> {
        let url = format!("https://api.spotify.com/v1/me/player/shuffle?state={on}");
        self.send_action("PUT", &url, None)
    }

    fn set_repeat(&mut self, mode: RepeatMode) -> Result<(), String> {
        let state = match mode {
            RepeatMode::Off => "off",
            RepeatMode::All => "context",
            RepeatMode::One => "track",
        };
        let url = format!("https://api.spotify.com/v1/me/player/repeat?state={state}");
        self.send_action("PUT", &url, None)
    }

    fn list_devices(&mut self) -> Result<Vec<PlaybackDevice>, String> {
        let Some(v) = self
            .get_json("https://api.spotify.com/v1/me/player/devices")
            .map_err(ApiError::into_string)?
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(arr) = v["devices"].as_array() {
            for d in arr {
                let Some(id) = d["id"].as_str() else {
                    continue;
                };
                out.push(PlaybackDevice {
                    id: id.to_string(),
                    provider: ProviderId::Spotify,
                    name: d["name"].as_str().unwrap_or("Spotify Device").to_string(),
                    kind: d["type"].as_str().unwrap_or("Speaker").to_string(),
                    is_active: d["is_active"].as_bool().unwrap_or(false),
                    volume_pct: d["volume_percent"].as_u64().unwrap_or(80) as u8,
                });
            }
        }
        Ok(out)
    }

    fn transfer_device(&mut self, device_id: &str) -> Result<(), String> {
        let body = serde_json::json!({ "device_ids": [device_id], "play": true }).to_string();
        self.send_action("PUT", "https://api.spotify.com/v1/me/player", Some(&body))
    }

    fn poll_playback(&mut self) -> Result<Option<PlaybackSnapshot>, String> {
        let Some(v) = self
            .get_json("https://api.spotify.com/v1/me/player")
            .map_err(ApiError::into_string)?
        else {
            return Ok(None);
        };
        let is_playing = v["is_playing"].as_bool().unwrap_or(false);
        let state = if is_playing {
            PlayState::Playing
        } else {
            PlayState::Paused
        };
        let track = parse_track_obj(&v["item"], true);
        let position_ms = v["progress_ms"].as_u64().unwrap_or(0);
        let duration_ms = v["item"]["duration_ms"].as_u64().unwrap_or(0);
        let shuffle = v["shuffle_state"].as_bool().unwrap_or(false);
        let repeat = match v["repeat_state"].as_str() {
            Some("context") => RepeatMode::All,
            Some("track") => RepeatMode::One,
            _ => RepeatMode::Off,
        };
        let device = v["device"]["id"].as_str().map(|id| PlaybackDevice {
            id: id.to_string(),
            provider: ProviderId::Spotify,
            name: v["device"]["name"]
                .as_str()
                .unwrap_or("Spotify Device")
                .to_string(),
            kind: v["device"]["type"]
                .as_str()
                .unwrap_or("Speaker")
                .to_string(),
            is_active: true,
            volume_pct: v["device"]["volume_percent"].as_u64().unwrap_or(80) as u8,
        });
        let volume_pct = device.as_ref().map(|d| d.volume_pct).unwrap_or(80);
        Ok(Some(PlaybackSnapshot {
            provider: ProviderId::Spotify,
            state,
            track,
            position_ms,
            duration_ms,
            device,
            shuffle,
            repeat,
            volume_pct,
        }))
    }
}

fn raw_curl(
    method: &str,
    url: &str,
    access_token: &str,
    body: Option<&str>,
) -> Result<Option<Value>, ApiError> {
    let auth = format!("Authorization: Bearer {access_token}");
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        "10",
        "-w",
        "\n%{http_code}",
        "-X",
        method,
        "-H",
        &auth,
    ]);
    if method != "GET" {
        cmd.args(["-H", "Content-Type: application/json"]);
        if let Some(b) = body {
            cmd.args(["--data", b]);
        } else {
            cmd.args(["--data", ""]);
        }
    }
    cmd.arg(url).stdin(Stdio::null());

    let out = cmd
        .output()
        .map_err(|e| ApiError::Network(format!("curl failed: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::Network(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (resp_body, code_str) = text.rsplit_once('\n').unwrap_or(("", text.trim()));
    let status: u16 = code_str.trim().parse().unwrap_or(0);

    if status == 204 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        let msg = serde_json::from_str::<Value>(resp_body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or_else(|| resp_body.chars().take(120).collect());
        return Err(ApiError::Http { status, message: msg });
    }
    if resp_body.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Value>(resp_body)
        .map(Some)
        .map_err(|e| ApiError::Network(format!("parse Spotify JSON: {e}")))
}

/// Parse playlist paging object, supporting both 2026 `items.total` and legacy `tracks.total`.
pub fn parse_playlist_collections(v: &Value) -> Vec<Collection> {
    let mut out = Vec::new();
    let Some(items) = v["items"].as_array() else {
        return out;
    };
    for item in items {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        let name = item["name"].as_str().unwrap_or("Playlist").to_string();
        let owner = item["owner"]["display_name"]
            .as_str()
            .or_else(|| item["owner"]["id"].as_str())
            .map(String::from);
        let track_count = item["items"]["total"]
            .as_u64()
            .or_else(|| item["tracks"]["total"].as_u64())
            .map(|n| n as u32);
        let context_uri = item["uri"]
            .as_str()
            .map(String::from)
            .or_else(|| Some(format!("spotify:playlist:{id}")));
        out.push(Collection {
            id: format!("spotify:playlist:{id}"),
            provider: ProviderId::Spotify,
            kind: CollectionKind::Playlist,
            name,
            owner,
            track_count,
            context_uri,
            tracks_restricted: false,
        });
    }
    out
}

/// Parse playlist/liked items array, supporting both 2026 `entry["item"]` and legacy `entry["track"]`.
pub fn parse_items_array(v: &Value) -> Vec<Track> {
    let mut out = Vec::new();
    let Some(items) = v["items"].as_array() else {
        return out;
    };
    for entry in items {
        let inner = entry
            .get("item")
            .filter(|x| !x.is_null())
            .unwrap_or(&entry["track"]);
        if let Some(t) = parse_track_obj(inner, false) {
            out.push(t);
        }
    }
    out
}

fn parse_track_obj(item: &Value, fetch_art: bool) -> Option<Track> {
    let uri = item["uri"].as_str()?.to_string();
    let title = item["name"].as_str()?.to_string();
    let artist = item["artists"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["name"].as_str())
        .unwrap_or("Unknown Artist")
        .to_string();
    let album = item["album"]["name"]
        .as_str()
        .unwrap_or("Spotify")
        .to_string();
    let duration_secs = item["duration_ms"].as_u64().unwrap_or(0) / 1000;
    let url = item["external_urls"]["spotify"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let art_path = if fetch_art {
        item["album"]["images"]
            .as_array()
            .and_then(|imgs| {
                // Prefer ~300px thumbnail (middle entry) for fast Sixel decoding
                imgs.iter()
                    .find(|i| i["width"].as_u64().is_some_and(|w| (200..=400).contains(&w)))
                    .or_else(|| imgs.first())
            })
            .and_then(|i| i["url"].as_str())
            .and_then(download_art_cached)
    } else {
        None
    };

    Some(Track {
        id: TrackId::Spotify(uri.clone()),
        title,
        artist,
        album,
        track_number: item["track_number"].as_u64().map(|n| n as u32),
        year: item["album"]["release_date"]
            .as_str()
            .and_then(|s| s.get(..4))
            .and_then(|y| y.parse().ok()),
        duration_secs,
        source: SourceKind::Spotify { uri, url },
        art_path,
    })
}

fn download_art_cached(url: &str) -> Option<PathBuf> {
    let art_dir = cache_dir().join("art");
    let _ = fs::create_dir_all(&art_dir);
    let hash = fnv1a_64(url.as_bytes());
    let dst = art_dir.join(format!("sp-{hash:016x}.jpg"));
    if dst.is_file() {
        return Some(dst);
    }
    let tmp = art_dir.join(format!("sp-{hash:016x}.tmp"));
    let ok = Command::new("curl")
        .args(["-sS", "-L", "--max-time", "6", "-o"])
        .arg(&tmp)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok && fs::rename(&tmp, &dst).is_ok() {
        Some(dst)
    } else {
        let _ = fs::remove_file(&tmp);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_2026_items_total_and_legacy_tracks_total() {
        let payload = serde_json::json!({
            "items": [
                {
                    "id": "pl_new",
                    "name": "Night Drive 2026",
                    "owner": { "display_name": "Bruno" },
                    "items": { "total": 47 }
                },
                {
                    "id": "pl_old",
                    "name": "Classic Mix",
                    "owner": { "id": "spotify" },
                    "tracks": { "total": 30 }
                }
            ]
        });
        let cols = parse_playlist_collections(&payload);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].track_count, Some(47));
        assert_eq!(
            cols[0].context_uri.as_deref(),
            Some("spotify:playlist:pl_new")
        );
        assert_eq!(cols[1].track_count, Some(30));
    }

    #[test]
    fn parses_2026_item_and_legacy_track_entries() {
        let payload = serde_json::json!({
            "items": [
                {
                    "item": {
                        "uri": "spotify:track:111",
                        "name": "Nightcall",
                        "duration_ms": 258000,
                        "artists": [{ "name": "Kavinsky" }],
                        "album": { "name": "OutRun" }
                    }
                },
                {
                    "track": {
                        "uri": "spotify:track:222",
                        "name": "Resonance",
                        "duration_ms": 212000,
                        "artists": [{ "name": "HOME" }],
                        "album": { "name": "Odyssey" }
                    }
                }
            ]
        });
        let tracks = parse_items_array(&payload);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Nightcall");
        assert_eq!(tracks[1].title, "Resonance");
    }
}
