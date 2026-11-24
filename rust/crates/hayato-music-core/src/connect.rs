//! Embedded in-process Spotify Connect receiver (`librespot`) so **Hayato Cassette**
//! plays Spotify audio directly on **this Chromebook** by default — with zero
//! resident background daemons when Cassette is closed.
//!
//! Authentication & lifecycle:
//! - Cached credentials live in `~/.config/hayato/cassette/librespot/credentials.json` (`0600`).
//! - On launch, if cached credentials exist, `Spirc` connects immediately and activates
//!   `"Hayato Cassette"` as a `DeviceType::Chromebook` Spotify Connect receiver.
//! - If not yet paired, Cassette:
//!   1. Advertises `"Hayato Cassette"` via LAN Zeroconf (`librespot-discovery`) so tapping
//!      it in any Spotify app on the same Wi-Fi pairs it automatically.
//!   2. Supports 1-click browser PKCE pairing (`PairOAuth`) using `librespot`'s built-in
//!      Keymaster client ID with the `streaming` scope (silent — zero stdout writes).
//! - On `Drop` / `Shutdown`, `Spirc::shutdown()` disconnects cleanly.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::authentication::Credentials;
use librespot_core::cache::Cache;
use librespot_core::config::{DeviceType, SessionConfig};
use librespot_core::session::Session;
use librespot_discovery::Discovery;
use librespot_playback::audio_backend::{self, Sink, SinkResult};
use librespot_playback::config::{AudioFormat, Bitrate, PlayerConfig};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::{self, MixerConfig};
use librespot_playback::player::Player;
use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;

use crate::dsp::PcmTapWriter;
use crate::spotify::{generate_pkce, url_encode_public};
use crate::{cache_dir, config_dir, fnv1a_64};

/// Display name advertised to Spotify Connect for this Chromebook.
pub const LOCAL_CONNECT_NAME: &str = "Hayato Cassette";

const STREAMING_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";

#[derive(Clone, Debug)]
pub enum ConnectEvent {
    Ready { device_id: String },
    Unpaired,
    Status(String),
    Error(String),
}

#[derive(Debug)]
enum ConnectCmd {
    PairOAuth,
    ClearAndDisconnect,
    Shutdown,
}

/// Handle to the embedded in-process Spotify Connect receiver thread.
pub struct LocalConnect {
    tx: tokio_mpsc::UnboundedSender<ConnectCmd>,
}

impl LocalConnect {
    pub fn spawn(
        initial_volume_pct: u8,
        tap: Arc<Mutex<PcmTapWriter>>,
        pcm_active: Arc<AtomicBool>,
        ev_tx: Sender<ConnectEvent>,
    ) -> Self {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        let _ = thread::Builder::new()
            .name("cassette-connect".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                else {
                    let _ = ev_tx.send(ConnectEvent::Error(
                        "Failed to start local Spotify Connect runtime".into(),
                    ));
                    return;
                };
                rt.block_on(connect_loop(initial_volume_pct, tap, pcm_active, rx, ev_tx));
            });
        Self { tx }
    }

    pub fn pair_oauth(&self) {
        let _ = self.tx.send(ConnectCmd::PairOAuth);
    }

    pub fn clear_and_disconnect(&self) {
        let _ = self.tx.send(ConnectCmd::ClearAndDisconnect);
    }
}

impl Drop for LocalConnect {
    fn drop(&mut self) {
        let _ = self.tx.send(ConnectCmd::Shutdown);
    }
}

pub fn credentials_dir() -> PathBuf {
    config_dir().join("librespot")
}

pub fn has_cached_credentials() -> bool {
    credentials_dir().join("credentials.json").is_file()
}

/// Stable 40-hex-char device ID derived from the machine hostname so Spotify
/// remembers this Chromebook across Cassette restarts.
pub fn stable_device_id() -> String {
    let host = fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "hayato-chromebook".to_string());
    let h1 = fnv1a_64(format!("hayato-cassette-1:{host}").as_bytes());
    let h2 = fnv1a_64(format!("hayato-cassette-2:{host}").as_bytes());
    let h3 = fnv1a_64(format!("hayato-cassette-3:{host}").as_bytes());
    format!("{h1:016x}{h2:016x}{:08x}", (h3 & 0xFFFF_FFFF) as u32)
}

struct ActiveSpirc {
    spirc: Spirc,
    task: tokio::task::JoinHandle<()>,
}

impl ActiveSpirc {
    fn stop(self) {
        let _ = self.spirc.shutdown();
        self.task.abort();
    }
}

/// Wraps `librespot`'s audio sink to mirror decoded stereo PCM straight into
/// Cassette's real-time 32-band FFT spectrum & oscilloscope analyzer.
struct TapSink {
    inner: Box<dyn Sink>,
    tap: Arc<Mutex<PcmTapWriter>>,
    active: Arc<AtomicBool>,
    buf: Vec<f32>,
}

impl Sink for TapSink {
    fn start(&mut self) -> SinkResult<()> {
        self.active.store(true, Ordering::Relaxed);
        self.inner.start()
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.active.store(false, Ordering::Relaxed);
        self.inner.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(ref samples) = packet {
            self.active.store(true, Ordering::Relaxed);
            self.buf.clear();
            self.buf.extend(samples.iter().map(|&s| s as f32));
            if let Ok(mut guard) = self.tap.try_lock() {
                guard.push_interleaved(&self.buf, 2);
            }
        }
        self.inner.write(packet, converter)
    }
}

impl Drop for TapSink {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

async fn start_spirc(
    device_id: &str,
    initial_volume_pct: u8,
    cache: &Cache,
    credentials: Credentials,
    tap: Arc<Mutex<PcmTapWriter>>,
    pcm_active: Arc<AtomicBool>,
) -> Result<ActiveSpirc, String> {
    let session_cfg = SessionConfig {
        device_id: device_id.to_string(),
        ..SessionConfig::default()
    };

    let session = Session::new(session_cfg, Some(cache.clone()));

    let backend = audio_backend::find(None)
        .ok_or_else(|| "No local audio backend available for Spotify Connect".to_string())?;
    let mixer_fn =
        mixer::find(None).ok_or_else(|| "No audio mixer available for Spotify Connect".to_string())?;
    let mixer = mixer_fn(MixerConfig::default())
        .map_err(|e| format!("init Spotify Connect mixer: {e}"))?;

    let player_cfg = PlayerConfig {
        bitrate: Bitrate::Bitrate320,
        ..PlayerConfig::default()
    };
    let player = Player::new(
        player_cfg,
        session.clone(),
        mixer.get_soft_volume(),
        move || {
            Box::new(TapSink {
                inner: backend(None, AudioFormat::default()),
                tap,
                active: pcm_active,
                buf: Vec::with_capacity(4096),
            })
        },
    );

    let init_vol = ((u32::from(initial_volume_pct.min(100)) * u32::from(u16::MAX)) / 100) as u16;
    let connect_cfg = ConnectConfig {
        name: LOCAL_CONNECT_NAME.to_string(),
        device_type: DeviceType::Chromebook,
        initial_volume: init_vol,
        ..ConnectConfig::default()
    };

    let (spirc, spirc_task) = Spirc::new(connect_cfg, session, credentials, player, mixer)
        .await
        .map_err(|e| format!("Spotify Connect session failed: {e}"))?;

    let _ = spirc.activate();
    let task = tokio::spawn(spirc_task);
    tighten_credentials_perms();
    Ok(ActiveSpirc { spirc, task })
}

fn tighten_credentials_perms() {
    let p = credentials_dir().join("credentials.json");
    if p.is_file() {
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
}

async fn connect_loop(
    initial_volume_pct: u8,
    tap: Arc<Mutex<PcmTapWriter>>,
    pcm_active: Arc<AtomicBool>,
    mut cmd_rx: tokio_mpsc::UnboundedReceiver<ConnectCmd>,
    ev_tx: Sender<ConnectEvent>,
) {
    let cred_dir = credentials_dir();
    let _ = fs::create_dir_all(&cred_dir);
    let _ = fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700));
    let audio_dir = cache_dir().join("librespot-audio");

    let Ok(cache) = Cache::new(
        Some(cred_dir.clone()),
        None,
        Some(audio_dir),
        Some(128 * 1024 * 1024),
    ) else {
        let _ = ev_tx.send(ConnectEvent::Error(
            "Failed to open local Spotify Connect cache".into(),
        ));
        return;
    };

    let device_id = stable_device_id();
    let client_id = SessionConfig::default().client_id;
    let mut active: Option<ActiveSpirc> = None;

    // 1. Auto-start immediately if we already have cached streaming credentials.
    if let Some(creds) = cache.credentials() {
        let _ = ev_tx.send(ConnectEvent::Status(
            "Connecting This Device audio (Hayato Cassette)…".into(),
        ));
        match start_spirc(
            &device_id,
            initial_volume_pct,
            &cache,
            creds,
            Arc::clone(&tap),
            Arc::clone(&pcm_active),
        )
        .await
        {
            Ok(sp) => {
                active = Some(sp);
                let _ = ev_tx.send(ConnectEvent::Ready {
                    device_id: device_id.clone(),
                });
            }
            Err(e) => {
                let _ = ev_tx.send(ConnectEvent::Error(e));
                let _ = ev_tx.send(ConnectEvent::Unpaired);
            }
        }
    } else {
        let _ = ev_tx.send(ConnectEvent::Unpaired);
    }

    // 2. Also advertise via LAN Zeroconf so any Spotify app on the Wi-Fi can pair us.
    let mut discovery = Discovery::builder(device_id.clone(), client_id.clone())
        .name(LOCAL_CONNECT_NAME)
        .device_type(DeviceType::Chromebook)
        .launch()
        .ok();

    loop {
        if let Some(disc) = discovery.as_mut() {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    if !handle_cmd(cmd, &mut active, &device_id, &client_id, initial_volume_pct, &cache, &tap, &pcm_active, &ev_tx).await {
                        break;
                    }
                }
                ev = disc.next() => {
                    if let Some(creds) = ev {
                        if let Some(old) = active.take() {
                            old.stop();
                        }
                        cache.save_credentials(&creds);
                        tighten_credentials_perms();
                        match start_spirc(&device_id, initial_volume_pct, &cache, creds, Arc::clone(&tap), Arc::clone(&pcm_active)).await {
                            Ok(sp) => {
                                active = Some(sp);
                                let _ = ev_tx.send(ConnectEvent::Ready {
                                    device_id: device_id.clone(),
                                });
                                let _ = ev_tx.send(ConnectEvent::Status(
                                    "This Device audio ready (Hayato Cassette)".into(),
                                ));
                            }
                            Err(e) => {
                                let _ = ev_tx.send(ConnectEvent::Error(e));
                            }
                        }
                    }
                }
            }
        } else {
            let cmd = cmd_rx.recv().await;
            if !handle_cmd(
                cmd,
                &mut active,
                &device_id,
                &client_id,
                initial_volume_pct,
                &cache,
                &tap,
                &pcm_active,
                &ev_tx,
            )
            .await
            {
                break;
            }
        }
    }

    if let Some(sp) = active.take() {
        sp.stop();
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_cmd(
    cmd: Option<ConnectCmd>,
    active: &mut Option<ActiveSpirc>,
    device_id: &str,
    client_id: &str,
    initial_volume_pct: u8,
    cache: &Cache,
    tap: &Arc<Mutex<PcmTapWriter>>,
    pcm_active: &Arc<AtomicBool>,
    ev_tx: &Sender<ConnectEvent>,
) -> bool {
    match cmd {
        None | Some(ConnectCmd::Shutdown) => return false,
        Some(ConnectCmd::ClearAndDisconnect) => {
            if let Some(sp) = active.take() {
                sp.stop();
            }
            let _ = fs::remove_file(credentials_dir().join("credentials.json"));
            let _ = ev_tx.send(ConnectEvent::Unpaired);
        }
        Some(ConnectCmd::PairOAuth) => {
            let _ = ev_tx.send(ConnectEvent::Status(
                "Opening browser to enable This Device audio…".into(),
            ));
            let cid = client_id.to_string();
            let res = tokio::task::spawn_blocking(move || run_streaming_pkce(&cid)).await;
            match res {
                Ok(Ok(access_token)) => {
                    if let Some(old) = active.take() {
                        old.stop();
                    }
                    let creds = Credentials::with_access_token(access_token);
                    match start_spirc(
                        device_id,
                        initial_volume_pct,
                        cache,
                        creds,
                        Arc::clone(tap),
                        Arc::clone(pcm_active),
                    )
                    .await
                    {
                        Ok(sp) => {
                            *active = Some(sp);
                            let _ = ev_tx.send(ConnectEvent::Ready {
                                device_id: device_id.to_string(),
                            });
                            let _ = ev_tx.send(ConnectEvent::Status(
                                "This Device audio enabled (Hayato Cassette)".into(),
                            ));
                        }
                        Err(e) => {
                            let _ = ev_tx.send(ConnectEvent::Error(e));
                        }
                    }
                }
                Ok(Err(e)) => {
                    let _ = ev_tx.send(ConnectEvent::Error(e));
                }
                Err(e) => {
                    let _ = ev_tx.send(ConnectEvent::Error(format!("OAuth task failed: {e}")));
                }
            }
        }
    }
    true
}

/// Silent browser PKCE flow requesting the `streaming` scope using librespot's
/// built-in Keymaster client ID (never writes to stdout, safe inside Ratatui TUI).
fn run_streaming_pkce(client_id: &str) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:8898")
        .map_err(|e| format!("bind 127.0.0.1:8898 for local audio login: {e}"))?;
    let _ = listener.set_nonblocking(true);

    let (verifier, challenge) = generate_pkce();
    let state = &verifier[..16];
    let url = format!(
        "https://accounts.spotify.com/authorize?response_type=code&client_id={}&scope=streaming&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}",
        url_encode_public(client_id),
        url_encode_public(STREAMING_REDIRECT_URI),
        url_encode_public(&challenge),
        url_encode_public(state),
    );

    let _ = Command::new("xdg-open")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    let deadline = SystemTime::now() + Duration::from_secs(120);
    let code = loop {
        if SystemTime::now() >= deadline {
            return Err("Timed out waiting for This Device audio login (120s)".into());
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
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body style='background:#111;color:#eee;font-family:monospace;padding:2rem'><h2>Hayato Cassette This Device Audio Enabled!</h2><p>You can close this tab and return to Hayato Cassette.</p></body></html>"
            } else {
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h2>Audio pairing cancelled or failed.</h2></body></html>"
            };
            let _ = stream.write_all(html.as_bytes());
            let _ = stream.flush();

            if let Some(e) = got_err {
                return Err(format!("Spotify rejected streaming login: {e}"));
            }
            if let Some(c) = got_code {
                break c;
            }
        }
        thread::sleep(Duration::from_millis(120));
    };

    let form = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        url_encode_public(&code),
        url_encode_public(STREAMING_REDIRECT_URI),
        url_encode_public(client_id),
        url_encode_public(&verifier),
    );
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
            &form,
            "https://accounts.spotify.com/api/token",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl token exchange failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("parse token JSON: {e}"))?;
    v["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("missing access_token: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_device_id_is_deterministic_40_hex() {
        let a = stable_device_id();
        let b = stable_device_id();
        assert_eq!(a, b);
        assert_eq!(a.len(), 40);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
