//! 3-tier Spotify integration for **Hayato Cassette**:
//!
//! - **Tier 1 (Embedded / Local Connect):** If `librespot` is installed, Cassette can
//!   advertise `"Hayato Cassette"` as a Spotify Connect speaker on this Chromebook.
//! - **Tier 2 (Web API Remote Control):** OAuth 2.0 PKCE login using the user's own
//!   Spotify Developer `client_id` (complying with Spotify's 2026 Development Mode
//!   5-user / Premium owner rules), enabling in-TUI search, playlists, device picker,
//!   and play/pause/next/prev/seek/volume over HTTPS (`curl`).
//! - **Tier 3 (Free Account / Unlinked):** Browse & open tracks in Firefox cleanly.
//!
//! Credentials are stored at `~/.config/hayato/cassette/spotify.json` with mode `0600`
//! and excluded from release images by `make-release.sh`.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Playlist, SourceKind, Track, TrackId, config_dir};

pub const REDIRECT_URI: &str = "http://127.0.0.1:8898/callback";
const SCOPES: &str = "user-read-private user-read-playback-state user-modify-playback-state user-library-read playlist-read-private playlist-read-collaborative";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpotifyToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_unix: u64,
    pub display_name: String,
    pub is_premium: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpotifyDevice {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_active: bool,
    pub volume_percent: u8,
}

pub fn token_path() -> PathBuf {
    config_dir().join("spotify.json")
}

pub fn load_token() -> Option<SpotifyToken> {
    let text = fs::read_to_string(token_path()).ok()?;
    let tok: SpotifyToken = serde_json::from_str(&text).ok()?;
    (!tok.access_token.is_empty() && !tok.client_id.is_empty()).then_some(tok)
}

pub fn save_token(tok: &SpotifyToken) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {e}"))?;
    let path = token_path();
    let json = serde_json::to_string_pretty(tok).map_err(|e| format!("serialize token: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write token: {e}"))?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok(())
}

pub fn clear_token() {
    let _ = fs::remove_file(token_path());
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Generate a 43-char URL-safe PKCE verifier from `/dev/urandom` + its SHA-256 S256 challenge.
pub fn generate_pkce() -> (String, String) {
    let mut raw = [0u8; 32];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut raw);
    } else {
        let t = now_unix().to_le_bytes();
        for (i, b) in raw.iter_mut().enumerate() {
            *b = t[i % 8].wrapping_add((i as u8).wrapping_mul(31));
        }
    }
    let verifier = base64url_encode(&raw);
    let digest = sha256(verifier.as_bytes());
    let challenge = base64url_encode(&digest);
    (verifier, challenge)
}

/// Build the Spotify OAuth 2.0 PKCE authorize URL.
pub fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "https://accounts.spotify.com/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}",
        url_encode(client_id),
        url_encode(SCOPES),
        url_encode(REDIRECT_URI),
        url_encode(challenge),
        url_encode(state),
    )
}

/// Run the interactive PKCE login: opens Firefox to Spotify's auth page and
/// listens on `127.0.0.1:8898/callback` for up to 120 seconds.
pub fn login_pkce(client_id: &str) -> Result<SpotifyToken, String> {
    let cid = client_id.trim();
    if cid.is_empty() {
        return Err("Enter your Spotify Developer Client ID in Settings first".to_string());
    }
    let listener = TcpListener::bind("127.0.0.1:8898")
        .map_err(|e| format!("bind 127.0.0.1:8898 for OAuth callback: {e}"))?;
    let _ = listener.set_nonblocking(true);

    let (verifier, challenge) = generate_pkce();
    let state = &verifier[..16];
    let url = authorize_url(cid, &challenge, state);

    let _ = Command::new("xdg-open")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    let deadline = SystemTime::now() + Duration::from_secs(120);
    let code = loop {
        if SystemTime::now() >= deadline {
            return Err("Timed out waiting for Spotify browser login (120s)".to_string());
        }
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");
            let query = first_line
                .split_whitespace()
                .nth(1)
                .and_then(|p| p.split_once('?').map(|(_, q)| q))
                .unwrap_or("");
            let mut got_code = None;
            let mut got_err = None;
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "code" {
                        got_code = Some(v.to_string());
                    } else if k == "error" {
                        got_err = Some(v.to_string());
                    }
                }
            }
            let html = if got_code.is_some() {
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style='background:#111;color:#eee;font-family:monospace;padding:2rem'><h2>Hayato Cassette connected!</h2><p>You can close this tab and return to Hayato Cassette.</p></body></html>"
            } else {
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Spotify login cancelled or failed.</h2></body></html>"
            };
            let _ = stream.write_all(html.as_bytes());
            let _ = stream.flush();

            if let Some(e) = got_err {
                return Err(format!("Spotify rejected login: {e}"));
            }
            if let Some(c) = got_code {
                break c;
            }
        }
        std::thread::sleep(Duration::from_millis(120));
    };

    exchange_code(cid, &code, &verifier)
}

fn exchange_code(client_id: &str, code: &str, verifier: &str) -> Result<SpotifyToken, String> {
    let form = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        url_encode(code),
        url_encode(REDIRECT_URI),
        url_encode(client_id),
        url_encode(verifier),
    );
    let body = curl_post_form("https://accounts.spotify.com/api/token", &form)?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("parse token JSON: {e}"))?;
    let access_token = v["access_token"]
        .as_str()
        .ok_or_else(|| format!("missing access_token: {body}"))?
        .to_string();
    let refresh_token = v["refresh_token"].as_str().unwrap_or("").to_string();
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);

    let mut tok = SpotifyToken {
        client_id: client_id.to_string(),
        access_token,
        refresh_token,
        expires_at_unix: now_unix() + expires_in.saturating_sub(60),
        display_name: String::new(),
        is_premium: false,
    };
    if let Ok(me) = api_get(&mut tok, "https://api.spotify.com/v1/me") {
        tok.display_name = me["display_name"]
            .as_str()
            .or_else(|| me["id"].as_str())
            .unwrap_or("Spotify User")
            .to_string();
        tok.is_premium = me["product"].as_str() == Some("premium");
    }
    save_token(&tok)?;
    Ok(tok)
}

pub(crate) fn ensure_fresh_public(tok: &mut SpotifyToken) -> Result<(), String> {
    ensure_fresh(tok)
}

pub(crate) fn url_encode_public(input: &str) -> String {
    url_encode(input)
}

fn ensure_fresh(tok: &mut SpotifyToken) -> Result<(), String> {
    if now_unix() < tok.expires_at_unix {
        return Ok(());
    }
    if tok.refresh_token.is_empty() {
        return Err("Spotify session expired — please sign in again".to_string());
    }
    let form = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        url_encode(&tok.refresh_token),
        url_encode(&tok.client_id),
    );
    let body = curl_post_form("https://accounts.spotify.com/api/token", &form)?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("parse refresh JSON: {e}"))?;
    let access = v["access_token"]
        .as_str()
        .ok_or_else(|| "Spotify token refresh failed".to_string())?;
    tok.access_token = access.to_string();
    if let Some(rt) = v["refresh_token"].as_str()
        && !rt.is_empty()
    {
        tok.refresh_token = rt.to_string();
    }
    tok.expires_at_unix = now_unix() + v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
    let _ = save_token(tok);
    Ok(())
}

fn curl_post_form(url: &str, form: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "10",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/x-www-form-urlencoded",
            "--data",
            form,
            url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn api_get(tok: &mut SpotifyToken, url: &str) -> Result<Value, String> {
    ensure_fresh(tok)?;
    let auth = format!("Authorization: Bearer {}", tok.access_token);
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "10", "-H", &auth, url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl GET failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("parse Spotify JSON: {e}"))
}

fn api_action(
    tok: &mut SpotifyToken,
    method: &str,
    url: &str,
    json_body: Option<&str>,
) -> Result<(), String> {
    ensure_fresh(tok)?;
    let auth = format!("Authorization: Bearer {}", tok.access_token);
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        "10",
        "-X",
        method,
        "-H",
        &auth,
        "-H",
        "Content-Type: application/json",
    ]);
    if let Some(body) = json_body {
        cmd.args(["--data", body]);
    } else {
        cmd.args(["--data", ""]);
    }
    cmd.arg(url).stdin(Stdio::null());
    let out = cmd.output().map_err(|e| format!("curl {method} failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// Search Spotify tracks via Web API.
pub fn search_tracks(tok: &mut SpotifyToken, query: &str, limit: usize) -> Result<Vec<Track>, String> {
    let url = format!(
        "https://api.spotify.com/v1/search?type=track&limit={}&q={}",
        limit.clamp(1, 10),
        url_encode(query)
    );
    let v = api_get(tok, &url)?;
    let mut out = Vec::new();
    if let Some(items) = v["tracks"]["items"].as_array() {
        for item in items {
            if let Some(t) = parse_spotify_track(item) {
                out.push(t);
            }
        }
    }
    Ok(out)
}

/// Fetch user's Spotify playlists.
pub fn list_playlists(tok: &mut SpotifyToken) -> Result<Vec<Playlist>, String> {
    let v = api_get(tok, "https://api.spotify.com/v1/me/playlists?limit=25")?;
    let mut out = Vec::new();
    if let Some(items) = v["items"].as_array() {
        for item in items {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            let name = item["name"].as_str().unwrap_or("Playlist").to_string();
            out.push(Playlist {
                id: format!("spotify:{id}"),
                name,
                is_spotify: true,
                path: None,
                tracks: Vec::new(),
            });
        }
    }
    Ok(out)
}

/// Fetch tracks inside a Spotify playlist.
pub fn playlist_tracks(tok: &mut SpotifyToken, playlist_id: &str) -> Result<Vec<Track>, String> {
    let raw_id = playlist_id.strip_prefix("spotify:").unwrap_or(playlist_id);
    let url = format!("https://api.spotify.com/v1/playlists/{raw_id}/tracks?limit=50");
    let v = api_get(tok, &url)?;
    let mut out = Vec::new();
    if let Some(items) = v["items"].as_array() {
        for entry in items {
            if let Some(t) = parse_spotify_track(&entry["track"]) {
                out.push(t);
            }
        }
    }
    Ok(out)
}

/// List available Spotify Connect devices.
pub fn list_devices(tok: &mut SpotifyToken) -> Result<Vec<SpotifyDevice>, String> {
    let v = api_get(tok, "https://api.spotify.com/v1/me/player/devices")?;
    let mut out = Vec::new();
    if let Some(items) = v["devices"].as_array() {
        for d in items {
            let Some(id) = d["id"].as_str() else {
                continue;
            };
            out.push(SpotifyDevice {
                id: id.to_string(),
                name: d["name"].as_str().unwrap_or("Device").to_string(),
                device_type: d["type"].as_str().unwrap_or("Speaker").to_string(),
                is_active: d["is_active"].as_bool().unwrap_or(false),
                volume_percent: d["volume_percent"].as_u64().unwrap_or(80) as u8,
            });
        }
    }
    Ok(out)
}

pub fn play_uri(tok: &mut SpotifyToken, uri: &str, device_id: Option<&str>) -> Result<(), String> {
    let url = match device_id {
        Some(id) => format!(
            "https://api.spotify.com/v1/me/player/play?device_id={}",
            url_encode(id)
        ),
        None => "https://api.spotify.com/v1/me/player/play".to_string(),
    };
    let body = serde_json::json!({ "uris": [uri] }).to_string();
    api_action(tok, "PUT", &url, Some(&body))
}

pub fn pause_remote(tok: &mut SpotifyToken) -> Result<(), String> {
    api_action(tok, "PUT", "https://api.spotify.com/v1/me/player/pause", None)
}

pub fn resume_remote(tok: &mut SpotifyToken) -> Result<(), String> {
    api_action(tok, "PUT", "https://api.spotify.com/v1/me/player/play", None)
}

pub fn transfer_device(tok: &mut SpotifyToken, device_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "device_ids": [device_id], "play": true }).to_string();
    api_action(tok, "PUT", "https://api.spotify.com/v1/me/player", Some(&body))
}

fn parse_spotify_track(item: &Value) -> Option<Track> {
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
        art_path: None,
    })
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn base64url_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 { u32::from(chunk[1]) } else { 0 };
        let b2 = if chunk.len() > 2 { u32::from(chunk[2]) } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

/// Pure-Rust SHA-256 (FIPS 180-4) for PKCE S256 without adding a crypto crate.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_valid_base64url_sha256() {
        let (verifier, challenge) = generate_pkce();
        assert_eq!(verifier.len(), 43);
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
        // Known vector: SHA-256("abc")
        let d = sha256(b"abc");
        assert_eq!(d[0], 0xba);
        assert_eq!(d[1], 0x78);
    }

    #[test]
    fn authorize_url_contains_pkce_params() {
        let u = authorize_url("my_cid", "chal123", "st456");
        assert!(u.contains("client_id=my_cid"));
        assert!(u.contains("code_challenge_method=S256"));
        assert!(u.contains("code_challenge=chal123"));
    }
}
