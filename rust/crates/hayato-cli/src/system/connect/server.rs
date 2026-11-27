//! Session-scoped HTTPS (TLS 1.3) + WSS LAN server for **Hayato Connect**.
//!
//! Zero resident daemons: starts when `ConnectScreen` opens and tears down cleanly
//! on `Drop`. Reuses `share::server` sanitization, MIME, ZIP bundling, and
//! theme CSS primitives while enforcing TLS, one-time QR pairing + 6-digit SAS
//! authorization, and SHA-256 loop-guarded clipboard synchronization.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openssl::ssl::SslStream;
use serde::{Deserialize, Serialize};
use tungstenite::Message;

use super::alerts::{AlertWatcher, send_desktop_alert};
use super::camera::{CamSink, CamTelemetry};
use super::crypto::{ConnectIdentity, derive_sas, sha256_hex};
use super::media::{MediaSnapshot, exec_media_cmd, poll_media};
use super::sync::{
    FileEntry, build_manifest, compute_diff_plan, default_sync_config_path, load_sync_folder,
    safe_rel_path,
};
use super::trust::TrustStore;
use crate::system::share::SharePayload;
use crate::system::share::server::{
    ServeKind, ThemeCss, active_theme_css, constant_time_eq, create_unique_file,
    default_downloads_dir, detect_lan_ip, generate_token, html_escape, human_size,
    prepare_serve_kind, sanitize_filename, security_headers,
};

const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CLIP_BYTES: usize = 512 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct ConnectedPeer {
    pub device_id: String,
    pub name: String,
    pub user_agent: String,
    pub remote_ip: String,
}

#[derive(Clone, Debug)]
pub struct OfferedItem {
    pub id: String,
    pub kind: ServeKind,
    pub title: String,
    pub size: u64,
    ephemeral_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectTelemetry {
    pub seq: u64,
    pub transfers: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub active_label: String,
    pub last_event: String,
    pub connected_peers: Vec<ConnectedPeer>,
    pub last_clip_preview: String,
    pub media: MediaSnapshot,
    pub cam: CamTelemetry,
    pub sync_summary: String,
}

#[derive(Debug)]
pub enum ConnectEvent {
    AuthRequested {
        req_id: String,
        device_label: String,
        remote_ip: String,
        sas_code: String,
    },
    PeerConnected(ConnectedPeer),
    PeerDisconnected {
        device_id: String,
        name: String,
    },
    ClipboardReceived {
        preview: String,
        bytes: usize,
    },
    LinkReceived {
        url: String,
        from: String,
    },
    NoteReceived {
        title: String,
        text: String,
        from: String,
    },
    AlertReceived {
        title: String,
        body: String,
        from: String,
    },
    MediaCmdExecuted {
        cmd: String,
        from: String,
    },
    SyncUpdated {
        summary: String,
    },
    TransferFinished {
        summary: String,
    },
    PairUrlRotated(String),
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InboundWsMsg {
    Hello {
        #[serde(default)]
        pair_token: String,
        #[serde(default)]
        peer_nonce: String,
        #[serde(default)]
        device_id: String,
        #[serde(default)]
        device_secret: String,
        #[serde(default)]
        device_name: String,
    },
    Clipboard {
        text: String,
    },
    Link {
        url: String,
    },
    Note {
        #[serde(default)]
        title: String,
        text: String,
    },
    MediaCmd {
        cmd: String,
    },
    CamMeta {
        #[serde(default)]
        active: bool,
        #[serde(default)]
        facing: String,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
    },
    Alert {
        #[serde(default)]
        title: String,
        body: String,
    },
    SyncManifest {
        files: Vec<FileEntry>,
    },
    Ping,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutboundWsMsg {
    AwaitingApproval {
        sas: String,
    },
    Paired {
        device_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        device_secret: Option<String>,
        name: String,
        auto: bool,
    },
    Denied {
        reason: String,
    },
    Clipboard {
        text: String,
        sha256: String,
    },
    Media {
        media: MediaSnapshot,
    },
    Alert {
        app: String,
        summary: String,
        body: String,
    },
    CamControl {
        active: bool,
        facing: String,
    },
    SyncDiff {
        push: Vec<FileEntry>,
        pull: Vec<String>,
        up_to_date: usize,
    },
    Offer {
        id: String,
        kind: String,
        title: String,
        size: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Link {
        url: String,
        title: String,
    },
    Pong,
}

type OutboxTx = Sender<String>;

struct SharedState {
    ip: String,
    port: u16,
    pair_token: Mutex<String>,
    used_nonces: Mutex<HashSet<String>>,
    spki_fp: String,
    downloads_dir: PathBuf,
    sync_root: Mutex<PathBuf>,
    theme: ThemeCss,
    trust: Mutex<TrustStore>,
    offers: Mutex<HashMap<String, OfferedItem>>,
    pending_auth: Mutex<HashMap<String, Sender<bool>>>,
    clients: Mutex<Vec<(ConnectedPeer, OutboxTx)>>,
    telemetry: Mutex<ConnectTelemetry>,
    last_clip_hash: Mutex<String>,
    auto_clip_sync: AtomicBool,
    media_mirror: AtomicBool,
    cam: Arc<CamSink>,
    ev_tx: Sender<ConnectEvent>,
}

pub struct ConnectServer {
    pub ip: String,
    pub port: u16,
    pub pair_url: String,
    pub spki_fp: String,
    stop: Arc<AtomicBool>,
    shared: Arc<SharedState>,
    alert_watcher: AlertWatcher,
    runtime_session_file: Option<PathBuf>,
}

impl Drop for ConnectServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.cam.stop_all();
        if let Ok(mut pending) = self.shared.pending_auth.lock() {
            for (_, tx) in pending.drain() {
                let _ = tx.send(false);
            }
        }
        if let Ok(mut offers) = self.shared.offers.lock() {
            for (_, item) in offers.drain() {
                if let Some(p) = item.ephemeral_path {
                    let _ = fs::remove_file(p);
                }
            }
        }
        if let Some(path) = self.runtime_session_file.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl ConnectServer {
    pub fn start(
        trust: TrustStore,
        identity_dir: Option<&Path>,
        downloads_dir: PathBuf,
    ) -> Result<(Self, Receiver<ConnectEvent>), String> {
        let ip = detect_lan_ip();
        let identity = match identity_dir {
            Some(dir) => ConnectIdentity::load_or_create(dir, &ip)?,
            None => ConnectIdentity::load_default(&ip)?,
        };
        let acceptor = Arc::new(identity.ssl_acceptor()?);
        let listener =
            TcpListener::bind("0.0.0.0:0").map_err(|e| format!("Bind LAN TLS port: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Configure TLS socket: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("Read TLS socket port: {e}"))?
            .port();
        let pair_token = generate_token();
        let pair_url = format!(
            "https://{ip}:{port}/?pair={pair_token}#fp={}",
            identity.spki_fp
        );

        let (ev_tx, ev_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let sync_folder = load_sync_folder(&default_sync_config_path());
        let cam = Arc::new(CamSink::new());
        let shared = Arc::new(SharedState {
            ip: ip.clone(),
            port,
            pair_token: Mutex::new(pair_token),
            used_nonces: Mutex::new(HashSet::new()),
            spki_fp: identity.spki_fp.clone(),
            downloads_dir,
            sync_root: Mutex::new(sync_folder),
            theme: active_theme_css(),
            trust: Mutex::new(trust),
            offers: Mutex::new(HashMap::new()),
            pending_auth: Mutex::new(HashMap::new()),
            clients: Mutex::new(Vec::new()),
            telemetry: Mutex::new(ConnectTelemetry {
                last_event: "TLS/WSS ready · scan QR or wait for paired phone".to_string(),
                cam: cam.telemetry(),
                ..ConnectTelemetry::default()
            }),
            last_clip_hash: Mutex::new(String::new()),
            auto_clip_sync: AtomicBool::new(true),
            media_mirror: AtomicBool::new(true),
            cam,
            ev_tx,
        });

        let runtime_session_file = write_runtime_marker(&ip, port);

        // TLS accept loop (dies when `stop` becomes true)
        let stop_bg = Arc::clone(&stop);
        let shared_bg = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("connect-tls".into())
            .spawn(move || {
                while !stop_bg.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((tcp, peer_addr)) => {
                            let _ = tcp.set_nonblocking(false);
                            let acc = Arc::clone(&acceptor);
                            let st = Arc::clone(&shared_bg);
                            let stop_c = Arc::clone(&stop_bg);
                            let ip_str = peer_addr.ip().to_string();
                            let _ = std::thread::Builder::new()
                                .name("connect-conn".into())
                                .spawn(move || {
                                    if let Ok(ssl) = acc.accept(tcp) {
                                        handle_tls_connection(ssl, ip_str, st, stop_c);
                                    }
                                });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(60));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("spawn TLS accept loop: {e}"))?;

        // Session-scoped clipboard + media telemetry loop (dies on Drop)
        let stop_clip = Arc::clone(&stop);
        let shared_clip = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("connect-clip".into())
            .spawn(move || {
                clipboard_watch_loop(stop_clip, shared_clip);
            })
            .map_err(|e| format!("spawn clipboard watcher: {e}"))?;

        let shared_alert = Arc::clone(&shared);
        let alert_watcher = AlertWatcher::start(true, move |app, summary, body| {
            broadcast_ws(
                &shared_alert,
                &OutboundWsMsg::Alert {
                    app: app.clone(),
                    summary: summary.clone(),
                    body: body.clone(),
                },
            );
            let _ = shared_alert.ev_tx.send(ConnectEvent::AlertReceived {
                title: summary,
                body,
                from: app,
            });
        });

        Ok((
            Self {
                ip,
                port,
                pair_url,
                spki_fp: identity.spki_fp,
                stop,
                shared,
                alert_watcher,
                runtime_session_file,
            },
            ev_rx,
        ))
    }

    pub fn set_media_mirror(&self, enabled: bool) {
        self.shared.media_mirror.store(enabled, Ordering::Relaxed);
    }

    pub fn media_mirror(&self) -> bool {
        self.shared.media_mirror.load(Ordering::Relaxed)
    }

    pub fn set_alerts_mirror(&self, enabled: bool) {
        self.alert_watcher.set_enabled(enabled);
    }

    pub fn alerts_mirror(&self) -> bool {
        self.alert_watcher.is_enabled()
    }

    pub fn broadcast_alert(&self, app: &str, summary: &str, body: &str) -> usize {
        broadcast_ws(
            &self.shared,
            &OutboundWsMsg::Alert {
                app: app.to_string(),
                summary: summary.to_string(),
                body: body.to_string(),
            },
        )
    }

    pub fn poll_and_broadcast_media(&self) -> MediaSnapshot {
        let snap = poll_media();
        let changed = self
            .shared
            .telemetry
            .lock()
            .map(|mut tel| {
                let diff = tel.media != snap;
                tel.media = snap.clone();
                if diff {
                    tel.seq += 1;
                }
                diff
            })
            .unwrap_or(false);
        if changed && self.shared.media_mirror.load(Ordering::Relaxed) {
            broadcast_ws(
                &self.shared,
                &OutboundWsMsg::Media {
                    media: snap.clone(),
                },
            );
        }
        snap
    }

    pub fn exec_local_media(&self, cmd: &str) -> Result<String, String> {
        let res = exec_media_cmd(cmd)?;
        self.poll_and_broadcast_media();
        Ok(res)
    }

    pub fn toggle_cam_viewfinder(&self) -> Result<String, String> {
        let res = self.shared.cam.toggle_viewfinder()?;
        if let Ok(mut tel) = self.shared.telemetry.lock() {
            tel.cam = self.shared.cam.telemetry();
            tel.last_event = res.clone();
            tel.seq += 1;
        }
        Ok(res)
    }

    pub fn save_cam_snapshot(&self) -> Result<PathBuf, String> {
        let path = self.shared.cam.save_snapshot()?;
        if let Ok(mut tel) = self.shared.telemetry.lock() {
            tel.last_event = format!("Saved photo {}", path.display());
            tel.seq += 1;
        }
        Ok(path)
    }

    pub fn request_phone_cam(&self, active: bool, facing: &str) -> usize {
        broadcast_ws(
            &self.shared,
            &OutboundWsMsg::CamControl {
                active,
                facing: facing.to_string(),
            },
        )
    }

    pub fn set_sync_root(&self, folder: PathBuf) {
        if let Ok(mut g) = self.shared.sync_root.lock() {
            *g = folder;
        }
    }

    pub fn sync_root(&self) -> PathBuf {
        self.shared
            .sync_root
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| load_sync_folder(&default_sync_config_path()))
    }

    pub fn set_auto_clip(&self, enabled: bool) {
        self.shared.auto_clip_sync.store(enabled, Ordering::Relaxed);
    }

    pub fn auto_clip(&self) -> bool {
        self.shared.auto_clip_sync.load(Ordering::Relaxed)
    }

    pub fn respond_auth(&self, req_id: &str, approved: bool) {
        if let Ok(mut pending) = self.shared.pending_auth.lock()
            && let Some(tx) = pending.remove(req_id)
        {
            let _ = tx.send(approved);
        }
    }

    pub fn broadcast_clipboard(&self, text: &str) -> usize {
        if text.trim().is_empty() || text.len() > MAX_CLIP_BYTES {
            return 0;
        }
        let hash = sha256_hex(text.as_bytes());
        if let Ok(mut last) = self.shared.last_clip_hash.lock() {
            *last = hash.clone();
        }
        if let Ok(mut tel) = self.shared.telemetry.lock() {
            tel.last_clip_preview = preview_one_line(text);
            tel.last_event = format!("Pushed clipboard ({})", human_size(text.len() as u64));
            tel.seq += 1;
        }
        broadcast_ws(
            &self.shared,
            &OutboundWsMsg::Clipboard {
                text: text.to_string(),
                sha256: hash,
            },
        )
    }

    pub fn broadcast_link(&self, url: &str, title: Option<&str>) -> usize {
        let trimmed = url.trim();
        if (!trimmed.starts_with("http://") && !trimmed.starts_with("https://"))
            || trimmed.chars().any(char::is_control)
            || trimmed.len() > 4096
        {
            return 0;
        }
        let label = title.unwrap_or(trimmed).to_string();
        if let Ok(mut tel) = self.shared.telemetry.lock() {
            tel.last_event = format!("Sent link: {}", preview_one_line(trimmed));
            tel.seq += 1;
        }
        broadcast_ws(
            &self.shared,
            &OutboundWsMsg::Link {
                url: trimmed.to_string(),
                title: label,
            },
        )
    }

    pub fn offer_payload(&self, payload: &SharePayload) -> Result<OfferedItem, String> {
        let (kind, ephemeral_path) =
            prepare_serve_kind(payload, self.shared.downloads_dir.clone())?;
        let id = generate_token()[..12].to_string();
        let (kind_tag, title, size, url, text) = match &kind {
            ServeKind::File { filename, size, .. } => ("file", filename.clone(), *size, None, None),
            ServeKind::Url { url, title } => (
                "url",
                title.clone(),
                url.len() as u64,
                Some(url.clone()),
                None,
            ),
            ServeKind::Text { content, label } => (
                "text",
                label.clone(),
                content.len() as u64,
                None,
                Some(content.clone()),
            ),
            ServeKind::Receive { dest_dir } => {
                ("receive", dest_dir.display().to_string(), 0, None, None)
            }
        };
        let item = OfferedItem {
            id: id.clone(),
            kind,
            title: title.clone(),
            size,
            ephemeral_path,
        };
        if let Ok(mut offers) = self.shared.offers.lock() {
            offers.insert(id.clone(), item.clone());
        }
        broadcast_ws(
            &self.shared,
            &OutboundWsMsg::Offer {
                id,
                kind: kind_tag.to_string(),
                title: title.clone(),
                size: human_size(size),
                url,
                text,
            },
        );
        if let Ok(mut tel) = self.shared.telemetry.lock() {
            tel.last_event = format!("Offered {title} to connected devices");
            tel.seq += 1;
        }
        Ok(item)
    }

    pub fn snapshot(&self) -> ConnectTelemetry {
        self.shared
            .telemetry
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    pub fn trust_devices(&self) -> Vec<super::trust::TrustedDevice> {
        self.shared
            .trust
            .lock()
            .map(|t| t.devices().to_vec())
            .unwrap_or_default()
    }

    pub fn unpair_device(&self, id: &str) -> Result<bool, String> {
        let ok = self
            .shared
            .trust
            .lock()
            .map_err(|_| "trust lock poisoned".to_string())?
            .unpair(id)?;
        if ok {
            if let Ok(mut clients) = self.shared.clients.lock() {
                clients.retain(|(p, _)| p.device_id != id);
            }
            refresh_peers_telemetry(&self.shared);
        }
        Ok(ok)
    }
}

struct PrefixedStream<S> {
    prefix: Cursor<Vec<u8>>,
    inner: S,
}

impl<S: Read> Read for PrefixedStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.prefix.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

impl<S: Write> Write for PrefixedStream<S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn handle_tls_connection(
    mut ssl: SslStream<TcpStream>,
    remote_ip: String,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
) {
    let _ = ssl
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(20)));
    let mut head_buf = Vec::with_capacity(2048);
    let mut byte = [0u8; 1];
    while head_buf.len() < 16_384 {
        match ssl.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                head_buf.push(byte[0]);
                if head_buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    if !head_buf.ends_with(b"\r\n\r\n") {
        let _ = ssl.write_all(
            b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 17\r\n\r\nHeaders too large",
        );
        return;
    }
    let head_str = String::from_utf8_lossy(&head_buf).to_string();
    let mut lines = head_str.lines();
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let req_target = parts.next().unwrap_or("/");
    let path_only = req_target.split('?').next().unwrap_or(req_target);
    let query_str = req_target.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut is_ws = false;
    let mut content_length: u64 = 0;
    let mut filename_hdr = String::new();
    let mut rel_path_hdr = String::new();
    let mut device_id_hdr = String::new();
    let mut device_secret_hdr = String::new();
    let mut ua_hdr = String::new();
    let mut host_hdr = String::new();
    let mut origin_hdr = String::new();

    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let val = v.trim();
            if k.eq_ignore_ascii_case("upgrade") && val.eq_ignore_ascii_case("websocket") {
                is_ws = true;
            } else if k.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("x-filename") {
                filename_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("x-rel-path") {
                rel_path_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("x-device-id") {
                device_id_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("x-device-secret") {
                device_secret_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("user-agent") {
                ua_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("host") {
                host_hdr = val.to_string();
            } else if k.eq_ignore_ascii_case("origin") {
                origin_hdr = val.to_string();
            }
        }
    }

    if is_ws && path_only == "/ws" {
        if !origin_hdr.is_empty()
            && !is_allowed_ws_origin(&origin_hdr, &host_hdr, &shared.ip, shared.port)
        {
            let _ = ssl.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\nForbidden");
            return;
        }
        let prefixed = PrefixedStream {
            prefix: Cursor::new(head_buf),
            inner: ssl,
        };
        handle_websocket(prefixed, remote_ip, ua_hdr, shared, stop);
        return;
    }

    match (method, path_only) {
        ("GET", "/") => {
            let pair = query_param(query_str, "pair").unwrap_or_default();
            let current_token = shared
                .pair_token
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            let valid_pair =
                !pair.is_empty() && constant_time_eq(pair.as_bytes(), current_token.as_bytes());
            let html = render_connect_web_app(
                &shared.theme,
                &shared.spki_fp,
                if valid_pair { &current_token } else { "" },
            );
            let hdr = security_headers("text/html; charset=utf-8", html.len());
            let _ = ssl.write_all(hdr.as_bytes());
            let _ = ssl.write_all(html.as_bytes());
        }
        ("POST", "/api/upload") => {
            if !verify_http_auth(&shared, &device_id_hdr, &device_secret_hdr) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            handle_http_upload(ssl, content_length, &filename_hdr, &shared);
        }
        ("POST", "/api/clip") => {
            if !verify_http_auth(&shared, &device_id_hdr, &device_secret_hdr) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            handle_http_clip(ssl, content_length, &shared);
        }
        ("POST", "/api/notify") => {
            if !verify_http_auth(&shared, &device_id_hdr, &device_secret_hdr) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            handle_http_notify(ssl, content_length, &shared);
        }
        ("POST", "/api/sync/upload") => {
            if !verify_http_auth(&shared, &device_id_hdr, &device_secret_hdr) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            handle_sync_upload(ssl, content_length, &rel_path_hdr, &shared);
        }
        ("GET", "/api/sync/download") => {
            let dev_q = query_param(query_str, "device").unwrap_or(device_id_hdr);
            let sec_q = query_param(query_str, "secret").unwrap_or(device_secret_hdr);
            if !verify_http_auth(&shared, &dev_q, &sec_q) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            let rel_q = query_param(query_str, "rel")
                .map(|r| percent_decode(&r))
                .unwrap_or_default();
            handle_sync_download(ssl, &rel_q, &shared);
        }
        ("GET", p) if p.starts_with("/api/download/") => {
            let dev_q = query_param(query_str, "device").unwrap_or(device_id_hdr);
            let sec_q = query_param(query_str, "secret").unwrap_or(device_secret_hdr);
            if !verify_http_auth(&shared, &dev_q, &sec_q) {
                let _ = ssl.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized",
                );
                return;
            }
            let offer_id = p.trim_start_matches("/api/download/");
            handle_http_download(ssl, offer_id, &shared);
        }
        _ => {
            let _ = ssl.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        }
    }
}

fn handle_websocket(
    prefixed: PrefixedStream<SslStream<TcpStream>>,
    remote_ip: String,
    user_agent: String,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
) {
    let Ok(mut ws) = tungstenite::accept(prefixed) else {
        return;
    };
    // Read Hello handshake frame within 15s
    let _ = ws
        .get_mut()
        .inner
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(15)));

    let hello_raw = match ws.read() {
        Ok(Message::Text(txt)) => txt.to_string(),
        _ => return,
    };
    let Ok(InboundWsMsg::Hello {
        pair_token,
        peer_nonce,
        device_id,
        device_secret,
        device_name,
    }) = serde_json::from_str::<InboundWsMsg>(&hello_raw)
    else {
        let _ = send_ws_json(
            &mut ws,
            &OutboundWsMsg::Denied {
                reason: "Invalid hello payload".into(),
            },
        );
        return;
    };

    let authed_peer = if !device_id.is_empty() && !device_secret.is_empty() {
        let verified = shared.trust.lock().ok().and_then(|mut t| {
            let name = t.verify(&device_id, &device_secret)?.name.clone();
            let ua = t.verify(&device_id, &device_secret)?.user_agent.clone();
            t.touch(&device_id);
            Some((name, ua))
        });
        match verified {
            Some((name, ua)) => {
                let _ = send_ws_json(
                    &mut ws,
                    &OutboundWsMsg::Paired {
                        device_id: device_id.clone(),
                        device_secret: None,
                        name: name.clone(),
                        auto: true,
                    },
                );
                ConnectedPeer {
                    device_id,
                    name,
                    user_agent: ua,
                    remote_ip: remote_ip.clone(),
                }
            }
            None => {
                // Fall back to QR pairing if a valid pair_token was provided
                match run_pairing_gate(
                    &mut ws,
                    &pair_token,
                    &peer_nonce,
                    &device_name,
                    &user_agent,
                    &remote_ip,
                    &shared,
                ) {
                    Some(peer) => peer,
                    None => return,
                }
            }
        }
    } else {
        match run_pairing_gate(
            &mut ws,
            &pair_token,
            &peer_nonce,
            &device_name,
            &user_agent,
            &remote_ip,
            &shared,
        ) {
            Some(peer) => peer,
            None => return,
        }
    };

    // Switch underlying TCP socket to short timeout so outbound WS messages push cleanly
    let _ = ws
        .get_mut()
        .inner
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(120)));

    let (out_tx, out_rx) = mpsc::channel::<String>();
    if let Ok(mut clients) = shared.clients.lock() {
        clients.push((authed_peer.clone(), out_tx));
    }
    refresh_peers_telemetry(&shared);
    let _ = shared
        .ev_tx
        .send(ConnectEvent::PeerConnected(authed_peer.clone()));

    let initial_media = poll_media();
    let _ = send_ws_json(
        &mut ws,
        &OutboundWsMsg::Media {
            media: initial_media,
        },
    );

    'ws: while !stop.load(Ordering::Relaxed) {
        loop {
            match out_rx.try_recv() {
                Ok(payload) => {
                    if ws.send(Message::Text(payload.into())).is_err() {
                        break 'ws;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    break 'ws;
                }
            }
        }
        match ws.read() {
            Ok(Message::Text(txt)) => {
                if let Ok(msg) = serde_json::from_str::<InboundWsMsg>(&txt) {
                    handle_ws_message(msg, &authed_peer, &mut ws, &shared);
                }
            }
            Ok(Message::Binary(bin)) => {
                shared.cam.push_jpeg_frame(&bin);
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.cam = shared.cam.telemetry();
                    tel.seq += 1;
                }
            }
            Ok(Message::Ping(p)) => {
                if ws.send(Message::Pong(p)).is_err() {
                    break 'ws;
                }
            }
            Ok(Message::Close(_)) => break 'ws,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Keep looping cleanly to check outbound queue & stop flag
            }
            Err(_) => break 'ws,
            _ => {}
        }
    }

    let empty = if let Ok(mut clients) = shared.clients.lock() {
        clients.retain(|(p, _)| p.device_id != authed_peer.device_id);
        clients.is_empty()
    } else {
        false
    };
    if empty {
        shared.cam.stop_all();
        if let Ok(mut tel) = shared.telemetry.lock() {
            tel.cam = shared.cam.telemetry();
        }
    }
    refresh_peers_telemetry(&shared);
    let _ = shared.ev_tx.send(ConnectEvent::PeerDisconnected {
        device_id: authed_peer.device_id,
        name: authed_peer.name,
    });
}

fn run_pairing_gate<S: Read + Write>(
    ws: &mut tungstenite::WebSocket<S>,
    pair_token: &str,
    peer_nonce: &str,
    device_name: &str,
    user_agent: &str,
    remote_ip: &str,
    shared: &Arc<SharedState>,
) -> Option<ConnectedPeer> {
    let current_token = shared
        .pair_token
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    if pair_token.is_empty()
        || peer_nonce.len() < 8
        || !constant_time_eq(pair_token.as_bytes(), current_token.as_bytes())
    {
        let _ = send_ws_json(
            ws,
            &OutboundWsMsg::Denied {
                reason: "Invalid pairing token — re-scan QR code".into(),
            },
        );
        return None;
    }

    if let Ok(mut nonces) = shared.used_nonces.lock()
        && !nonces.insert(peer_nonce.to_string())
    {
        let _ = send_ws_json(
            ws,
            &OutboundWsMsg::Denied {
                reason: "Pairing nonce replayed — refresh page and re-scan".into(),
            },
        );
        return None;
    }

    let sas_code = derive_sas(&shared.spki_fp, &current_token, peer_nonce);
    let label = if device_name.trim().is_empty() {
        summarize_ua(user_agent)
    } else {
        device_name.trim().to_string()
    };
    let req_id = generate_token()[..12].to_string();
    let (tx, rx) = mpsc::channel::<bool>();
    if let Ok(mut pending) = shared.pending_auth.lock() {
        if !pending.is_empty() {
            let _ = send_ws_json(
                ws,
                &OutboundWsMsg::Denied {
                    reason: "Another pairing request is awaiting desktop approval".into(),
                },
            );
            return None;
        }
        pending.insert(req_id.clone(), tx);
    }
    let _ = send_ws_json(
        ws,
        &OutboundWsMsg::AwaitingApproval {
            sas: sas_code.clone(),
        },
    );
    let _ = shared.ev_tx.send(ConnectEvent::AuthRequested {
        req_id: req_id.clone(),
        device_label: label.clone(),
        remote_ip: remote_ip.to_string(),
        sas_code,
    });

    let approved = rx.recv_timeout(AUTH_TIMEOUT).unwrap_or(false);
    if let Ok(mut pending) = shared.pending_auth.lock() {
        pending.remove(&req_id);
    }
    if !approved {
        let _ = send_ws_json(
            ws,
            &OutboundWsMsg::Denied {
                reason: "Pairing denied on desktop".into(),
            },
        );
        return None;
    }

    let paired = shared
        .trust
        .lock()
        .ok()
        .and_then(|mut t| t.pair_device(&label, user_agent).ok());
    let Some((dev, raw_secret)) = paired else {
        return None;
    };

    // Rotate the one-time QR pairing token after every successful authorization
    let next_token = generate_token();
    if let Ok(mut tok_guard) = shared.pair_token.lock() {
        *tok_guard = next_token.clone();
    }
    if let Ok(mut nonces) = shared.used_nonces.lock() {
        nonces.clear();
    }
    let next_url = format!(
        "https://{}:{}/?pair={next_token}#fp={}",
        shared.ip, shared.port, shared.spki_fp
    );
    let _ = shared.ev_tx.send(ConnectEvent::PairUrlRotated(next_url));

    let _ = send_ws_json(
        ws,
        &OutboundWsMsg::Paired {
            device_id: dev.id.clone(),
            device_secret: Some(raw_secret),
            name: dev.name.clone(),
            auto: false,
        },
    );
    Some(ConnectedPeer {
        device_id: dev.id,
        name: dev.name,
        user_agent: dev.user_agent,
        remote_ip: remote_ip.to_string(),
    })
}

fn handle_ws_message<S: Read + Write>(
    msg: InboundWsMsg,
    peer: &ConnectedPeer,
    ws: &mut tungstenite::WebSocket<S>,
    shared: &Arc<SharedState>,
) {
    match msg {
        InboundWsMsg::Clipboard { text } => {
            apply_inbound_clipboard(&text, &peer.name, shared);
        }
        InboundWsMsg::Link { url } => {
            let trimmed = url.trim().to_string();
            if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
                && !trimmed.chars().any(char::is_control)
                && trimmed.len() <= 4096
            {
                crate::system::spawn_detached_argv("hayato-browser", &[&trimmed]);
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.transfers += 1;
                    tel.last_event = format!(
                        "Opened link from {}: {}",
                        peer.name,
                        preview_one_line(&trimmed)
                    );
                    tel.seq += 1;
                }
                let _ = shared.ev_tx.send(ConnectEvent::LinkReceived {
                    url: trimmed,
                    from: peer.name.clone(),
                });
            }
        }
        InboundWsMsg::Note { title, text } => {
            if !text.trim().is_empty() && text.len() <= MAX_CLIP_BYTES {
                if save_note_to_inbox(&title, &text, &peer.name).is_ok()
                    && let Ok(mut tel) = shared.telemetry.lock()
                {
                    tel.transfers += 1;
                    tel.last_event = format!("Saved note from {} to Notes Inbox", peer.name);
                    tel.seq += 1;
                }
                let _ = shared.ev_tx.send(ConnectEvent::NoteReceived {
                    title,
                    text,
                    from: peer.name.clone(),
                });
            }
        }
        InboundWsMsg::MediaCmd { cmd } => {
            if exec_media_cmd(&cmd).is_ok() {
                let snap = poll_media();
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.media = snap.clone();
                    tel.last_event = format!("{} ran media command: {cmd}", peer.name);
                    tel.seq += 1;
                }
                if shared.media_mirror.load(Ordering::Relaxed) {
                    broadcast_ws(shared, &OutboundWsMsg::Media { media: snap });
                }
                let _ = shared.ev_tx.send(ConnectEvent::MediaCmdExecuted {
                    cmd,
                    from: peer.name.clone(),
                });
            }
        }
        InboundWsMsg::CamMeta {
            active,
            facing,
            width,
            height,
        } => {
            shared.cam.set_meta(active, facing, width, height);
            if let Ok(mut tel) = shared.telemetry.lock() {
                tel.cam = shared.cam.telemetry();
                tel.last_event = if active {
                    format!("{} camera stream LIVE ({}x{})", peer.name, width, height)
                } else {
                    format!("{} camera stream stopped", peer.name)
                };
                tel.seq += 1;
            }
        }
        InboundWsMsg::Alert { title, body } => {
            if !body.trim().is_empty() && body.len() <= MAX_CLIP_BYTES {
                let _ = send_desktop_alert(&peer.name, &title, &body);
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.last_event =
                        format!("Alert from {}: {}", peer.name, preview_one_line(&body));
                    tel.seq += 1;
                }
                let _ = shared.ev_tx.send(ConnectEvent::AlertReceived {
                    title,
                    body,
                    from: peer.name.clone(),
                });
            }
        }
        InboundWsMsg::SyncManifest { files } => {
            let root = shared
                .sync_root
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| load_sync_folder(&default_sync_config_path()));
            let _ = fs::create_dir_all(&root);
            if let Ok(local) = build_manifest(&root) {
                let plan = compute_diff_plan(&local, &files);
                let summary = format!(
                    "{} push · {} pull · {} ok",
                    plan.push_to_phone.len(),
                    plan.pull_from_phone.len(),
                    plan.up_to_date
                );
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.sync_summary = summary.clone();
                    tel.last_event = format!("Folder diff: {summary}");
                    tel.seq += 1;
                }
                let _ = send_ws_json(
                    ws,
                    &OutboundWsMsg::SyncDiff {
                        push: plan.push_to_phone,
                        pull: plan.pull_from_phone,
                        up_to_date: plan.up_to_date,
                    },
                );
                let _ = shared.ev_tx.send(ConnectEvent::SyncUpdated { summary });
            }
        }
        InboundWsMsg::Ping => {
            let _ = send_ws_json(ws, &OutboundWsMsg::Pong);
        }
        InboundWsMsg::Hello { .. } => {}
    }
}

fn handle_http_upload(
    mut ssl: SslStream<TcpStream>,
    content_length: u64,
    filename_hdr: &str,
    shared: &Arc<SharedState>,
) {
    if content_length == 0 || content_length > MAX_UPLOAD_BYTES {
        let _ =
            ssl.write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large");
        return;
    }
    let safe_name = sanitize_filename(filename_hdr);
    let Ok((dest_path, mut out_file)) = create_unique_file(&shared.downloads_dir, &safe_name)
    else {
        let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
        return;
    };
    let saved_name = dest_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&safe_name)
        .to_string();
    if let Ok(mut tel) = shared.telemetry.lock() {
        tel.bytes_done = 0;
        tel.bytes_total = content_length;
        tel.active_label = format!("Receiving {saved_name}");
        tel.seq += 1;
    }
    let mut remaining = content_length;
    let mut buf = [0u8; 32_768];
    let mut got = 0u64;
    let mut ok = true;
    while remaining > 0 {
        let want = (remaining as usize).min(buf.len());
        match ssl.read(&mut buf[..want]) {
            Ok(0) | Err(_) => {
                ok = false;
                break;
            }
            Ok(n) => {
                if out_file.write_all(&buf[..n]).is_err() {
                    ok = false;
                    break;
                }
                remaining -= n as u64;
                got += n as u64;
                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.bytes_done = got;
                    tel.seq += 1;
                }
            }
        }
    }
    if ok {
        let _ = ssl.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
        let summary = format!(
            "Received {saved_name} ({}) \u{2192} {}",
            human_size(content_length),
            shared.downloads_dir.display()
        );
        if let Ok(mut tel) = shared.telemetry.lock() {
            tel.transfers += 1;
            tel.bytes_done = 0;
            tel.bytes_total = 0;
            tel.active_label.clear();
            tel.last_event = summary.clone();
            tel.seq += 1;
        }
        let _ = shared
            .ev_tx
            .send(ConnectEvent::TransferFinished { summary });
    } else {
        if let Ok(mut tel) = shared.telemetry.lock() {
            tel.bytes_done = 0;
            tel.bytes_total = 0;
            tel.active_label.clear();
            tel.last_event = format!("Upload failed: {saved_name}");
            tel.seq += 1;
        }
        let _ = fs::remove_file(&dest_path);
        let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
    }
}

fn handle_http_notify(
    mut ssl: SslStream<TcpStream>,
    content_length: u64,
    shared: &Arc<SharedState>,
) {
    if content_length == 0 || content_length > 8192 {
        let _ = ssl.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\n\r\nBad payload");
        return;
    }
    let mut body = vec![0u8; content_length as usize];
    if ssl.read_exact(&mut body).is_err() {
        let _ = ssl.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\n\r\nRead failed");
        return;
    }
    let raw = String::from_utf8_lossy(&body).to_string();
    let (title, text) = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
        (
            v.get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Phone Alert")
                .to_string(),
            v.get("body")
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string(),
        )
    } else {
        ("Phone Alert".to_string(), raw)
    };
    let _ = send_desktop_alert("Phone", &title, &text);
    if let Ok(mut tel) = shared.telemetry.lock() {
        tel.last_event = format!("Webhook alert: {title}");
        tel.seq += 1;
    }
    let _ = shared.ev_tx.send(ConnectEvent::AlertReceived {
        title,
        body: text,
        from: "Webhook".to_string(),
    });
    let _ = ssl.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
}

fn handle_sync_upload(
    mut ssl: SslStream<TcpStream>,
    content_length: u64,
    rel_hdr: &str,
    shared: &Arc<SharedState>,
) {
    if content_length > MAX_UPLOAD_BYTES {
        let _ =
            ssl.write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large");
        return;
    }
    let root = shared
        .sync_root
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| load_sync_folder(&default_sync_config_path()));
    let _ = fs::create_dir_all(&root);
    let Some(target) = safe_rel_path(&root, rel_hdr, false) else {
        let _ =
            ssl.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\n\r\nInvalid path");
        return;
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(mut out) = File::create(&target) else {
        let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 11\r\n\r\nWrite error");
        return;
    };
    let mut buf = [0u8; 32_768];
    let mut rem = content_length;
    while rem > 0 {
        let want = (rem as usize).min(buf.len());
        match ssl.read(&mut buf[..want]) {
            Ok(0) | Err(_) => {
                let _ = fs::remove_file(&target);
                let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
                return;
            }
            Ok(n) => {
                if out.write_all(&buf[..n]).is_err() {
                    let _ = fs::remove_file(&target);
                    let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
                    return;
                }
                rem -= n as u64;
            }
        }
    }
    if let Ok(mut tel) = shared.telemetry.lock() {
        tel.transfers += 1;
        tel.last_event = format!("Synced in: {rel_hdr}");
        tel.seq += 1;
    }
    let _ = ssl.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
}

fn handle_sync_download(mut ssl: SslStream<TcpStream>, rel: &str, shared: &Arc<SharedState>) {
    let root = shared
        .sync_root
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| load_sync_folder(&default_sync_config_path()));
    let Some(target) = safe_rel_path(&root, rel, true) else {
        let _ = ssl.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        return;
    };
    let Ok(mut file) = File::open(&target) else {
        let _ = ssl.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        return;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let fname = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sync.bin");
    let hdr = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Disposition: attachment; filename=\"{}\"\r\n\
         Content-Length: {size}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        sanitize_filename(fname)
    );
    if ssl.write_all(hdr.as_bytes()).is_err() {
        return;
    }
    let mut buf = [0u8; 32_768];
    loop {
        match file.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if ssl.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

fn handle_http_clip(mut ssl: SslStream<TcpStream>, content_length: u64, shared: &Arc<SharedState>) {
    if content_length == 0 || content_length as usize > MAX_CLIP_BYTES {
        let _ =
            ssl.write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large");
        return;
    }
    let mut body = vec![0u8; content_length as usize];
    if ssl.read_exact(&mut body).is_ok() {
        let text = String::from_utf8_lossy(&body).to_string();
        if apply_inbound_clipboard(&text, "Phone", shared) {
            let _ = ssl.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
        } else {
            let _ = ssl.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
        }
    }
}

fn handle_http_download(mut ssl: SslStream<TcpStream>, offer_id: &str, shared: &Arc<SharedState>) {
    let item = shared
        .offers
        .lock()
        .ok()
        .and_then(|offers| offers.get(offer_id).cloned());
    let Some(item) = item else {
        let _ = ssl.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        return;
    };
    match &item.kind {
        ServeKind::File {
            path,
            filename,
            size,
            mime,
            ..
        } => {
            if let Ok(mut file) = File::open(path) {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n\
                     Content-Disposition: attachment; filename=\"{}\"\r\n\
                     Content-Length: {size}\r\nCache-Control: no-store\r\n\
                     X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
                    sanitize_filename(filename)
                );
                if ssl.write_all(hdr.as_bytes()).is_ok() {
                    if let Ok(mut tel) = shared.telemetry.lock() {
                        tel.bytes_done = 0;
                        tel.bytes_total = *size;
                        tel.active_label = format!("Sending {filename}");
                        tel.seq += 1;
                    }
                    let mut buf = [0u8; 32_768];
                    let mut sent = 0u64;
                    let mut ok = true;
                    loop {
                        match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if ssl.write_all(&buf[..n]).is_err() {
                                    ok = false;
                                    break;
                                }
                                sent += n as u64;
                                if let Ok(mut tel) = shared.telemetry.lock() {
                                    tel.bytes_done = sent;
                                    tel.seq += 1;
                                }
                            }
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        let summary = format!("Sent {filename} ({})", human_size(*size));
                        if let Ok(mut tel) = shared.telemetry.lock() {
                            tel.transfers += 1;
                            tel.bytes_done = 0;
                            tel.bytes_total = 0;
                            tel.active_label.clear();
                            tel.last_event = summary.clone();
                            tel.seq += 1;
                        }
                        let _ = shared
                            .ev_tx
                            .send(ConnectEvent::TransferFinished { summary });
                    } else if let Ok(mut tel) = shared.telemetry.lock() {
                        tel.bytes_done = 0;
                        tel.bytes_total = 0;
                        tel.active_label.clear();
                        tel.last_event = format!("Download interrupted: {filename}");
                        tel.seq += 1;
                    }
                }
            }
        }
        ServeKind::Text { content, .. } => {
            let bytes = content.as_bytes();
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = ssl.write_all(hdr.as_bytes());
            let _ = ssl.write_all(bytes);
        }
        _ => {
            let _ = ssl.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        }
    }
}

fn verify_http_auth(shared: &Arc<SharedState>, dev_id: &str, secret: &str) -> bool {
    if dev_id.is_empty() || secret.is_empty() {
        return false;
    }
    shared
        .trust
        .lock()
        .map(|t| t.verify(dev_id, secret).is_some())
        .unwrap_or(false)
}

fn apply_inbound_clipboard(text: &str, from: &str, shared: &Arc<SharedState>) -> bool {
    if text.is_empty() || text.len() > MAX_CLIP_BYTES {
        return false;
    }
    let hash = sha256_hex(text.as_bytes());
    if let Ok(mut last) = shared.last_clip_hash.lock() {
        if *last == hash {
            return true;
        }
        *last = hash;
    }
    let copied = write_wayland_clipboard(text);
    let preview = preview_one_line(text);
    if let Ok(mut tel) = shared.telemetry.lock() {
        tel.transfers += 1;
        tel.last_clip_preview = preview.clone();
        tel.last_event = format!("Clipboard from {from}: {preview}");
        tel.seq += 1;
    }
    let _ = shared.ev_tx.send(ConnectEvent::ClipboardReceived {
        preview,
        bytes: text.len(),
    });
    copied
}

fn clipboard_watch_loop(stop: Arc<AtomicBool>, shared: Arc<SharedState>) {
    // Seed current hash so opening Connect does not immediately re-push stale clipboard
    if let Some(cur) = read_wayland_clipboard()
        && let Ok(mut last) = shared.last_clip_hash.lock()
    {
        *last = sha256_hex(cur.as_bytes());
    }
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(750));
        if stop.load(Ordering::Relaxed) || !shared.auto_clip_sync.load(Ordering::Relaxed) {
            continue;
        }
        let has_clients = shared
            .clients
            .lock()
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        if !has_clients {
            continue;
        }
        let Some(text) = read_wayland_clipboard() else {
            continue;
        };
        if text.is_empty() || text.len() > MAX_CLIP_BYTES {
            continue;
        }
        let hash = sha256_hex(text.as_bytes());
        let changed = shared
            .last_clip_hash
            .lock()
            .map(|mut prev| {
                if *prev == hash {
                    false
                } else {
                    *prev = hash.clone();
                    true
                }
            })
            .unwrap_or(false);
        if changed {
            if let Ok(mut tel) = shared.telemetry.lock() {
                tel.last_clip_preview = preview_one_line(&text);
                tel.last_event = format!("Synced clipboard ({})", human_size(text.len() as u64));
                tel.seq += 1;
            }
            broadcast_ws(&shared, &OutboundWsMsg::Clipboard { text, sha256: hash });
        }
    }
}

pub(super) fn read_wayland_clipboard() -> Option<String> {
    let types_out = Command::new("wl-paste")
        .arg("--list-types")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !types_out.status.success() {
        return None;
    }
    let types_str = String::from_utf8_lossy(&types_out.stdout);
    let mut mime: Option<&str> = None;
    for line in types_str.lines() {
        let t = line.trim();
        if t.eq_ignore_ascii_case("x-kde-passwordManagerHint")
            || t.eq_ignore_ascii_case("application/x-kde-passwordManagerHint")
        {
            return None;
        }
        if t.eq_ignore_ascii_case("text/plain;charset=utf-8") {
            mime = Some("text/plain;charset=utf-8");
        } else if mime.is_none() && (t.eq_ignore_ascii_case("text/plain") || t.starts_with("text/"))
        {
            mime = Some(t);
        }
    }
    let mime = mime?;
    let out = Command::new("wl-paste")
        .args(["--no-newline", "--type", mime])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn is_allowed_ws_origin(origin: &str, host: &str, lan_ip: &str, port: u16) -> bool {
    let clean = origin.trim().trim_end_matches('/');
    let mut allowed = vec![
        format!("https://{lan_ip}:{port}"),
        format!("https://127.0.0.1:{port}"),
        format!("https://localhost:{port}"),
    ];
    if !host.is_empty() {
        allowed.push(format!("https://{host}"));
    }
    allowed.iter().any(|a| clean.eq_ignore_ascii_case(a))
}

pub(super) fn write_wayland_clipboard(text: &str) -> bool {
    Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            child.wait()
        })
        .map(|s| s.success())
        .unwrap_or(false)
}

fn save_note_to_inbox(title: &str, text: &str, from: &str) -> Result<PathBuf, String> {
    let dir = dirs::document_dir()
        .unwrap_or_else(|| default_downloads_dir().with_file_name("Documents"))
        .join("Notes");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("Inbox.md");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open Notes Inbox: {e}"))?;
    let heading = if title.trim().is_empty() {
        format!("From {from}")
    } else {
        title.trim().to_string()
    };
    writeln!(f, "\n## {heading}\n\n{}\n", text.trim()).map_err(|e| e.to_string())?;
    Ok(path)
}

fn broadcast_ws(shared: &Arc<SharedState>, msg: &OutboundWsMsg) -> usize {
    let Ok(json) = serde_json::to_string(msg) else {
        return 0;
    };
    let Ok(mut clients) = shared.clients.lock() else {
        return 0;
    };
    let mut sent = 0;
    clients.retain(|(_, tx)| {
        if tx.send(json.clone()).is_ok() {
            sent += 1;
            true
        } else {
            false
        }
    });
    sent
}

fn send_ws_json<S: Read + Write>(
    ws: &mut tungstenite::WebSocket<S>,
    msg: &OutboundWsMsg,
) -> Result<(), ()> {
    let txt = serde_json::to_string(msg).map_err(|_| ())?;
    ws.send(Message::Text(txt.into())).map_err(|_| ())
}

fn refresh_peers_telemetry(shared: &Arc<SharedState>) {
    let peers: Vec<ConnectedPeer> = shared
        .clients
        .lock()
        .map(|c| c.iter().map(|(p, _)| p.clone()).collect())
        .unwrap_or_default();
    if let Ok(mut tel) = shared.telemetry.lock() {
        tel.connected_peers = peers;
        tel.seq += 1;
    }
}

fn write_runtime_marker(ip: &str, port: u16) -> Option<PathBuf> {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)?;
    let dir = base.join("hayato-connect");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("session.json");
    let body = serde_json::json!({
        "ip": ip,
        "port": port,
        "pid": std::process::id(),
    })
    .to_string();
    fs::write(&path, body).ok()?;
    Some(path)
}

fn percent_decode(raw: &str) -> String {
    let mut bytes = Vec::with_capacity(raw.len());
    let src = raw.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'%'
            && i + 2 < src.len()
            && let Ok(hex) = std::str::from_utf8(&src[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            bytes.push(val);
            i += 3;
            continue;
        }
        bytes.push(if src[i] == b'+' { b' ' } else { src[i] });
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=')
            && k == key
        {
            return Some(v.to_string());
        }
    }
    None
}

fn summarize_ua(ua: &str) -> String {
    if ua.contains("iPhone") {
        "iPhone Browser".to_string()
    } else if ua.contains("iPad") {
        "iPad Browser".to_string()
    } else if ua.contains("Android") {
        "Android Browser".to_string()
    } else if ua.contains("Linux") {
        "Linux Browser".to_string()
    } else {
        "Mobile Browser".to_string()
    }
}

fn preview_one_line(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 48 {
        format!("{}…", collapsed.chars().take(47).collect::<String>())
    } else {
        collapsed
    }
}

fn render_connect_web_app(css: &ThemeCss, spki_fp: &str, pair_token: &str) -> String {
    let safe_fp = html_escape(spki_fp);
    let safe_pair = html_escape(pair_token);
    format!(
        "<!doctype html><html><head><meta charset='utf-8'>\
         <meta name='viewport' content='width=device-width,initial-scale=1'>\
         <title>Hayato Connect</title><style>\
         :root{{--bg:{bg};--surface:{surface};--border:{border};--text:{text};--muted:{muted};--accent:{accent};--ok:{ok}}}\
         *{{box-sizing:border-box}}body{{font-family:system-ui,-apple-system,sans-serif;background:var(--bg);color:var(--text);\
         margin:0;padding:1rem;min-height:100vh;display:flex;justify-content:center}}\
         .wrap{{width:100%;max-width:480px;display:flex;flex-direction:column;gap:.85rem}}\
         .card{{background:var(--surface);border:1px solid var(--border);border-left:4px solid var(--accent);\
         border-radius:14px;padding:1.1rem;box-shadow:0 8px 24px rgba(0,0,0,.3)}}\
         .row{{display:flex;align-items:center;justify-content:space-between;gap:.6rem;flex-wrap:wrap}}\
         .badge{{font:700 .75rem monospace;padding:.2rem .55rem;border-radius:6px;background:var(--accent);color:var(--bg)}}\
         .sas{{font:700 1.4rem monospace;letter-spacing:.12em;color:var(--ok);margin:.4rem 0}}\
         textarea,input[type=text]{{width:100%;background:var(--bg);color:var(--text);border:1px solid var(--border);\
         border-radius:8px;padding:.65rem;font-family:monospace;font-size:.85rem}}\
         .btn{{display:inline-block;padding:.65rem 1rem;border-radius:10px;font-weight:700;border:1px solid var(--border);\
         background:var(--bg);color:var(--text);cursor:pointer;text-decoration:none}}\
         .btn.primary{{background:var(--accent);color:var(--bg);border-color:transparent}}\
         .hint{{color:var(--muted);font-size:.82rem;margin:.35rem 0 0}}\
         progress{{width:100%;height:8px;accent-color:var(--ok)}}\
         .feed{{display:flex;flex-direction:column;gap:.5rem;margin-top:.5rem}}\
         .item{{background:var(--bg);border:1px solid var(--border);border-radius:8px;padding:.6rem;font-size:.85rem;word-break:break-word}}\
         </style></head><body><div class='wrap'>\
         <div class='card'>\
           <div class='row'><span class='badge'>HAYATO CONNECT · TLS</span><span id='st' class='hint'>Connecting\u{2026}</span></div>\
           <div id='sasbox' style='display:none'><p class='hint'>Verify code on desktop:</p><div id='sas' class='sas'></div></div>\
           <input id='name' type='text' placeholder='Device label (optional)' style='margin-top:.6rem'>\
         </div>\
         <div class='card'>\
           <strong>Files \u{2192} ~/Downloads</strong>\
           <input id='uf' type='file' multiple style='margin:.5rem 0;width:100%'>\
           <button class='btn primary' onclick='uploadFiles()'>Send Selected Files</button>\
           <progress id='up' value='0' max='100' style='display:none;margin-top:.5rem'></progress>\
           <p id='ust' class='hint'></p>\
         </div>\
         <div class='card'>\
           <div class='row'><strong>Folder Sync (1-Shot)</strong><span id='sst' class='hint'>Idle</span></div>\
           <input id='sf' type='file' webkitdirectory multiple style='margin:.5rem 0;width:100%'>\
           <button class='btn' onclick='runFolderSync()'>Diff & Sync Selected Folder</button>\
         </div>\
         <div class='card'>\
           <div class='row'><strong>Remote Media</strong><span id='mst' class='hint'>No player</span></div>\
           <p id='mtrack' class='hint' style='margin:.35rem 0 .6rem'>\u{2014}</p>\
           <div class='row'>\
             <button class='btn' onclick='mCmd(\"prev\")'>\u{23ee}</button>\
             <button class='btn' onclick='mCmd(\"seek_back\")'>-10s</button>\
             <button class='btn primary' onclick='mCmd(\"play_pause\")'>\u{23ef}</button>\
             <button class='btn' onclick='mCmd(\"seek_fwd\")'>+10s</button>\
             <button class='btn' onclick='mCmd(\"next\")'>\u{23ed}</button>\
             <button class='btn' onclick='mCmd(\"vol_down\")'>Vol-</button>\
             <button class='btn' onclick='mCmd(\"mute\")'>Mute</button>\
             <button class='btn' onclick='mCmd(\"vol_up\")'>Vol+</button>\
           </div>\
         </div>\
         <div class='card'>\
           <div class='row'><strong>Phone Camera</strong><span id='camst' class='hint'>Off</span></div>\
           <div class='row' style='margin-top:.5rem'>\
             <button class='btn primary' id='cambtn' onclick='toggleCam()'>Start Webcam Stream</button>\
             <button class='btn' onclick='flipCam()'>Flip Lens</button>\
           </div>\
           <video id='cvid' autoplay playsinline muted style='display:none;width:100%;border-radius:8px;margin-top:.5rem'></video>\
         </div>\
         <div class='card'>\
           <strong>Clipboard, Links, Notes & Alerts</strong>\
           <textarea id='txt' rows='3' placeholder='Paste text, URL, note, or desktop alert\u{2026}' style='margin:.5rem 0'></textarea>\
           <div class='row'>\
             <button class='btn primary' onclick='sendClip()'>Sync Clipboard</button>\
             <button class='btn' onclick='sendLink()'>Open Link</button>\
             <button class='btn' onclick='sendNote()'>Save Note</button>\
             <button class='btn' onclick='sendAlert()'>Send Alert</button>\
             <button class='btn' onclick='enableWebNotif()'>Browser Alerts</button>\
           </div>\
           <p id='cst' class='hint'></p>\
         </div>\
         <div class='card'>\
           <strong>From Desktop</strong>\
           <div id='feed' class='feed'><p class='hint'>Waiting for desktop shares\u{2026}</p></div>\
         </div>\
         </div>\
         <script>\
         const FP='{safe_fp}';const PAIR='{safe_pair}';const STORE='hayato_connect_device';\
         let dev=JSON.parse(localStorage.getItem(STORE)||'null')||{{}};\
         const st=document.getElementById('st');const feed=document.getElementById('feed');\
         const hashFp=(location.hash.match(/fp=([A-Za-z0-9_-]+)/)||[])[1]||'';\
         function safeHttp(u){{return typeof u==='string'&&/^https?:/.test(u.trim().toLowerCase())&&u.trim().toLowerCase().indexOf('http')===0&&(u.trim().slice(0,7).toLowerCase()==='http://'||u.trim().slice(0,8).toLowerCase()==='https://');}}\
         function addTextFeed(txt){{if(feed.querySelector('.hint'))feed.innerHTML='';const d=document.createElement('div');d.className='item';d.textContent=txt;feed.prepend(d);}}\
         function randHex(n){{const a=new Uint8Array(n);crypto.getRandomValues(a);return Array.from(a,b=>b.toString(16).padStart(2,'0')).join('');}}\
         const peerNonce=randHex(12);\
         let camStream=null,camTimer=null,camFacing='environment',syncFiles={{}};\
         async function localSas(){{if(!crypto.subtle)return '';const b=await crypto.subtle.digest('SHA-256',new TextEncoder().encode(FP+'\\x00'+PAIR+'\\x00'+peerNonce));const n=new DataView(b).getUint32(0,false)%1000000;const s=String(n).padStart(6,'0');return s.slice(0,3)+'-'+s.slice(3);}}\
         if(hashFp&&hashFp!==FP){{st.textContent='Certificate fingerprint mismatch — re-scan QR';}}else{{\
         const wsUrl=(location.protocol==='https:'?'wss://':'ws://')+location.host+'/ws';\
         const ws=new WebSocket(wsUrl);window._hws=ws;\
         ws.onopen=()=>{{ws.send(JSON.stringify({{type:'hello',pair_token:PAIR,peer_nonce:peerNonce,device_id:dev.id||'',device_secret:dev.secret||'',device_name:document.getElementById('name').value||''}}));}};\
         ws.onmessage=async (ev)=>{{const m=JSON.parse(ev.data);\
           if(m.type==='awaiting_approval'){{const exp=await localSas();if(exp&&m.sas!==exp){{st.textContent='SAS mismatch — aborting';ws.close();return;}}st.textContent='Approve on desktop';document.getElementById('sasbox').style.display='block';document.getElementById('sas').textContent=exp||m.sas;}}\
           else if(m.type==='paired'){{document.getElementById('sasbox').style.display='none';if(m.device_secret){{dev={{id:m.device_id,secret:m.device_secret,name:m.name}};localStorage.setItem(STORE,JSON.stringify(dev));}}st.textContent='Paired ('+m.name+')';}}\
           else if(m.type==='denied'){{st.textContent=m.reason||'Denied';}}\
           else if(m.type==='clipboard'){{document.getElementById('txt').value=m.text;if(navigator.clipboard)navigator.clipboard.writeText(m.text).catch(()=>{{}});addTextFeed('Clipboard updated');}}\
           else if(m.type==='media'){{const md=m.media||{{}};document.getElementById('mst').textContent=(md.status||'Idle')+' · '+(md.volume_pct||0)+'%'+(md.muted?' [MUTE]':'');document.getElementById('mtrack').textContent=md.title?((md.artist?md.artist+' \u{2014} ':'')+md.title):'No active media player';}}\
           else if(m.type==='alert'){{addTextFeed('['+m.app+'] '+m.summary+(m.body?': '+m.body:''));if(window.Notification&&Notification.permission==='granted'){{new Notification(m.app+': '+m.summary,{{body:m.body}});}}}}\
           else if(m.type==='cam_control'){{if(m.facing)camFacing=m.facing;if(m.active&&!camStream)startCam();else if(!m.active&&camStream)stopCam();}}\
           else if(m.type==='sync_diff'){{handleSyncDiff(m);}}\
           else if(m.type==='link'){{if(!safeHttp(m.url))return;const a=document.createElement('a');a.href=m.url;a.target='_blank';a.rel='noopener noreferrer';a.textContent=m.title||m.url;const d=document.createElement('div');d.className='item';d.appendChild(a);if(feed.querySelector('.hint'))feed.innerHTML='';feed.prepend(d);}}\
           else if(m.type==='offer'){{if(m.kind==='file'){{const u='/api/download/'+encodeURIComponent(m.id)+'?device='+encodeURIComponent(dev.id||'')+'&secret='+encodeURIComponent(dev.secret||'');\
             const a=document.createElement('a');a.href=u;a.className='btn primary';a.textContent='Download '+m.title+' ('+m.size+')';const d=document.createElement('div');d.className='item';d.appendChild(a);if(feed.querySelector('.hint'))feed.innerHTML='';feed.prepend(d);}}else if(m.url&&safeHttp(m.url)){{const a=document.createElement('a');a.href=m.url;a.target='_blank';a.rel='noopener noreferrer';a.textContent=m.title||m.url;const d=document.createElement('div');d.className='item';d.appendChild(a);if(feed.querySelector('.hint'))feed.innerHTML='';feed.prepend(d);}}else if(m.text){{document.getElementById('txt').value=m.text;addTextFeed('Text snippet ready above');}}}}\
         }};\
         ws.onclose=()=>{{stopCam();st.textContent='Disconnected';}};}}\
         function uploadFiles(){{const inp=document.getElementById('uf');if(!inp.files.length||!dev.id)return;\
           const bar=document.getElementById('up');const ust=document.getElementById('ust');bar.style.display='block';\
           let i=0;function next(){{if(i>=inp.files.length){{ust.textContent='Sent '+inp.files.length+' file(s)';return;}}\
           const f=inp.files[i++];const x=new XMLHttpRequest();x.open('POST','/api/upload');\
           x.setRequestHeader('X-Filename',f.name);x.setRequestHeader('X-Device-Id',dev.id);x.setRequestHeader('X-Device-Secret',dev.secret);\
           x.upload.onprogress=e=>{{if(e.lengthComputable)bar.value=Math.round(e.loaded/e.total*100);}};x.onload=()=>next();x.send(f);}}next();}}\
         function runFolderSync(){{const inp=document.getElementById('sf');if(!inp.files.length||!window._hws)return;\
           syncFiles={{}};const manifest=[];for(const f of inp.files){{const rel=f.webkitRelativePath||f.name;const clean=rel.indexOf('/')>0?rel.slice(rel.indexOf('/')+1):rel;syncFiles[clean]=f;manifest.push({{rel_path:clean,size:f.size,mtime_s:Math.floor(f.lastModified/1000)}});}}\
           document.getElementById('sst').textContent='Computing diff ('+manifest.length+' files)\u{2026}';\
           window._hws.send(JSON.stringify({{type:'sync_manifest',files:manifest}}));}}\
         async function handleSyncDiff(d){{const sst=document.getElementById('sst');let pushed=0;\
           for(const rel of (d.pull||[])){{const f=syncFiles[rel];if(!f)continue;await fetch('/api/sync/upload',{{method:'POST',headers:{{'X-Rel-Path':rel,'X-Device-Id':dev.id,'X-Device-Secret':dev.secret}},body:f}});pushed++;}}\
           for(const entry of (d.push||[])){{const u='/api/sync/download?rel='+encodeURIComponent(entry.rel_path)+'&device='+encodeURIComponent(dev.id||'')+'&secret='+encodeURIComponent(dev.secret||'');const a=document.createElement('a');a.href=u;a.className='btn';a.textContent='Pull updated: '+entry.rel_path;const item=document.createElement('div');item.className='item';item.appendChild(a);feed.prepend(item);}}\
           sst.textContent='Uploaded '+pushed+', ready to pull '+((d.push||[]).length)+' ('+(d.up_to_date||0)+' ok)';}}\
         function mCmd(c){{if(window._hws)window._hws.send(JSON.stringify({{type:'media_cmd',cmd:c}}));}}\
         async function startCam(){{if(!navigator.mediaDevices||!window._hws)return;\
           try{{camStream=await navigator.mediaDevices.getUserMedia({{video:{{facingMode:camFacing,width:{{ideal:960}},height:{{ideal:540}}}},audio:false}});const v=document.getElementById('cvid');v.srcObject=camStream;v.style.display='block';await v.play();\
           const cv=document.createElement('canvas');const ctx=cv.getContext('2d');document.getElementById('camst').textContent='LIVE ('+camFacing+')';document.getElementById('cambtn').textContent='Stop Camera';\
           window._hws.send(JSON.stringify({{type:'cam_meta',active:true,facing:camFacing,width:v.videoWidth||960,height:v.videoHeight||540}}));\
           camTimer=setInterval(()=>{{if(!camStream||window._hws.readyState!==1||v.videoWidth===0)return;cv.width=v.videoWidth;cv.height=v.videoHeight;ctx.drawImage(v,0,0);cv.toBlob(b=>{{if(b&&window._hws.readyState===1)window._hws.send(b);}},'image/jpeg',0.72);}},66);}}catch(e){{document.getElementById('camst').textContent='Camera denied';}}}}\
         function stopCam(){{if(camTimer){{clearInterval(camTimer);camTimer=null;}}if(camStream){{camStream.getTracks().forEach(t=>t.stop());camStream=null;}}const v=document.getElementById('cvid');if(v){{v.srcObject=null;v.style.display='none';}}document.getElementById('camst').textContent='Off';document.getElementById('cambtn').textContent='Start Webcam Stream';if(window._hws&&window._hws.readyState===1)window._hws.send(JSON.stringify({{type:'cam_meta',active:false,facing:camFacing,width:0,height:0}}));}}\
         function toggleCam(){{if(camStream)stopCam();else startCam();}}\
         function flipCam(){{camFacing=camFacing==='environment'?'user':'environment';if(camStream){{stopCam();startCam();}}}}\
         function sendClip(){{const t=document.getElementById('txt').value;if(!t||!window._hws)return;window._hws.send(JSON.stringify({{type:'clipboard',text:t}}));document.getElementById('cst').textContent='Sent clipboard';}}\
         function sendLink(){{const t=document.getElementById('txt').value.trim();if(!t||!window._hws)return;window._hws.send(JSON.stringify({{type:'link',url:t}}));document.getElementById('cst').textContent='Sent link to browser';}}\
         function sendNote(){{const t=document.getElementById('txt').value;if(!t||!window._hws)return;window._hws.send(JSON.stringify({{type:'note',title:'',text:t}}));document.getElementById('cst').textContent='Saved to Notes Inbox';}}\
         function sendAlert(){{const t=document.getElementById('txt').value.trim();if(!t||!window._hws)return;window._hws.send(JSON.stringify({{type:'alert',title:'Phone Alert',body:t}}));document.getElementById('cst').textContent='Sent alert to desktop';}}\
         function enableWebNotif(){{if(window.Notification)Notification.requestPermission().then(p=>{{document.getElementById('cst').textContent='Browser alerts: '+p;}});}}\
         </script></body></html>",
        bg = css.bg,
        surface = css.surface,
        border = css.border,
        text = css.text,
        muted = css.muted,
        accent = css.accent,
        ok = css.ok
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_auth_rejects_missing_or_wrong_device_secret() {
        let dir = tempfile::tempdir().unwrap();
        let trust = TrustStore::load_from(dir.path().join("devices.json"));
        let (_srv, _rx) = ConnectServer::start(trust, Some(dir.path()), dir.path().to_path_buf())
            .expect("start connect server");
        assert!(!verify_http_auth(&_srv.shared, "", ""));
        assert!(!verify_http_auth(&_srv.shared, "dev-nope", "secret"));
    }

    #[test]
    fn websocket_pairing_requires_explicit_approval_then_reuses_secret() {
        let dir = tempfile::tempdir().unwrap();
        let trust = TrustStore::load_from(dir.path().join("devices.json"));
        let (srv, rx) = ConnectServer::start(trust, Some(dir.path()), dir.path().to_path_buf())
            .expect("start connect server");
        assert!(srv.pair_url.starts_with("https://"));

        // Approve pending auth automatically from test driver once AuthRequested arrives
        let mut connector =
            openssl::ssl::SslConnector::builder(openssl::ssl::SslMethod::tls()).unwrap();
        connector.set_verify(openssl::ssl::SslVerifyMode::NONE);
        let ssl_conn = connector.build();
        let tcp = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let tls = ssl_conn.connect("hayato.local", tcp).unwrap();
        let ws_url = format!("wss://127.0.0.1:{}/ws", srv.port);
        let (mut ws, _) = tungstenite::client(ws_url, tls).unwrap();

        let initial_token = srv.shared.pair_token.lock().unwrap().clone();
        let hello = serde_json::json!({
            "type": "hello",
            "pair_token": initial_token,
            "peer_nonce": "peer-nonce-1234",
            "device_name": "Test Pixel"
        });
        ws.send(Message::Text(hello.to_string().into())).unwrap();

        // Replaying the same peer_nonce immediately is rejected
        let tcp_rep = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let tls_rep = ssl_conn.connect("hayato.local", tcp_rep).unwrap();
        let (mut ws_rep, _) =
            tungstenite::client(format!("wss://127.0.0.1:{}/ws", srv.port), tls_rep).unwrap();
        ws_rep
            .send(Message::Text(hello.to_string().into()))
            .unwrap();
        let rep_msg = ws_rep.read().unwrap().to_string();
        assert!(rep_msg.contains("denied"));

        // First frame must be awaiting_approval with 6-digit SAS matching server
        let msg1 = ws.read().unwrap().to_string();
        assert!(msg1.contains("awaiting_approval"));

        let req_id = loop {
            if let ConnectEvent::AuthRequested {
                req_id, sas_code, ..
            } = rx.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                assert!(msg1.contains(&sas_code));
                break req_id;
            }
        };

        srv.respond_auth(&req_id, true);
        let msg2 = ws.read().unwrap().to_string();
        let parsed: serde_json::Value = serde_json::from_str(&msg2).unwrap();
        assert_eq!(parsed["type"], "paired");
        let dev_id = parsed["device_id"].as_str().unwrap().to_string();
        let dev_secret = parsed["device_secret"].as_str().unwrap().to_string();
        assert!(verify_http_auth(&srv.shared, &dev_id, &dev_secret));

        // Pairing token rotated after approval
        let rotated_token = srv.shared.pair_token.lock().unwrap().clone();
        assert_ne!(rotated_token, initial_token);

        // Returning trusted client authenticates without prompting
        let tcp2 = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let tls2 = ssl_conn.connect("hayato.local", tcp2).unwrap();
        let (mut ws2, _) =
            tungstenite::client(format!("wss://127.0.0.1:{}/ws", srv.port), tls2).unwrap();
        let re_hello = serde_json::json!({
            "type": "hello",
            "device_id": dev_id,
            "device_secret": dev_secret,
        });
        ws2.send(Message::Text(re_hello.to_string().into()))
            .unwrap();
        let auto_msg = ws2.read().unwrap().to_string();
        let auto_val: serde_json::Value = serde_json::from_str(&auto_msg).unwrap();
        assert_eq!(auto_val["type"], "paired");
        assert_eq!(auto_val["auto"], true);

        // Unpairing revokes trust and closes active WebSocket connections
        assert!(srv.unpair_device(&dev_id).unwrap());
        assert!(!verify_http_auth(&srv.shared, &dev_id, &dev_secret));
        let _ = ws2
            .get_ref()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)));
        let mut saw_close = false;
        for _ in 0..4 {
            match ws2.read() {
                Ok(Message::Close(_)) | Err(_) => {
                    saw_close = true;
                    break;
                }
                Ok(_) => continue,
            }
        }
        assert!(saw_close);
    }
}
