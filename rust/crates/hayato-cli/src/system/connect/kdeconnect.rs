//! Opt-in, zero-daemon KDE Connect v7/v8 LAN protocol adapter.
//!
//! Speaks KDE Connect's UDP discovery (`1716`) and newline-delimited JSON
//! over TLS TCP (`1716..=1764`) using Hayato's existing `openssl` dependency —
//! zero Qt, zero C++, and zero background `kdeconnectd` process. Starts when
//! `ConnectScreen` opens and tears down cleanly on `Drop`.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::sha::sha256;
use openssl::ssl::{SslAcceptor, SslConnector, SslMethod, SslStream, SslVerifyMode};
use openssl::x509::{X509, X509Builder, X509NameBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::alerts::send_desktop_alert;
use super::crypto::{default_identity_dir, sha256_hex};
use super::media::{MediaSnapshot, exec_media_cmd, poll_media};
use super::server::ConnectEvent;
use super::trust::TrustStore;
use crate::system::share::server::{
    create_unique_file, generate_token, human_size, sanitize_filename,
};

const KDE_UDP_PORT: u16 = 1716;
const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;
const PROTOCOL_VERSION: u32 = 7;

#[derive(Clone, Debug)]
pub struct KdePeerInfo {
    pub device_id: String,
    pub name: String,
    pub device_type: String,
    pub remote_ip: String,
    pub battery_pct: Option<u8>,
    pub charging: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdePacket {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub body: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_transfer_info: Option<Value>,
}

struct KdeIdentity {
    device_id: String,
    device_name: String,
    cert: X509,
    key: PKey<Private>,
    cert_fp_hex: String,
}

impl KdeIdentity {
    fn load_or_create(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("create kdeconnect identity dir: {e}"))?;
        let id_path = dir.join("kdeconnect.id");
        let key_path = dir.join("kdeconnect.key");
        let cert_path = dir.join("kdeconnect.pem");

        let device_id = if let Ok(s) = fs::read_to_string(&id_path)
            && s.trim().len() >= 16
        {
            s.trim().to_string()
        } else {
            let tok = generate_token();
            let id = format!(
                "{}-{}-4{}-8{}-{}",
                &tok[0..8],
                &tok[8..12],
                &tok[12..15],
                &tok[15..18],
                &tok[18..30]
            );
            write_mode_600(&id_path, id.as_bytes())?;
            id
        };

        let (key, cert) = if let (Ok(k_pem), Ok(c_pem)) =
            (fs::read(&key_path), fs::read(&cert_path))
            && let (Ok(k), Ok(c)) = (PKey::private_key_from_pem(&k_pem), X509::from_pem(&c_pem))
        {
            (k, c)
        } else {
            let rsa = Rsa::generate(2048).map_err(|e| format!("gen RSA key: {e}"))?;
            let key = PKey::from_rsa(rsa).map_err(|e| format!("wrap RSA key: {e}"))?;
            let cert = build_kde_cert(&key, &device_id)?;
            let k_pem = key
                .private_key_to_pem_pkcs8()
                .map_err(|e| format!("export RSA key: {e}"))?;
            let c_pem = cert.to_pem().map_err(|e| format!("export KDE cert: {e}"))?;
            write_mode_600(&key_path, &k_pem)?;
            write_mode_600(&cert_path, &c_pem)?;
            (key, cert)
        };

        let der = cert.to_der().map_err(|e| e.to_string())?;
        let cert_fp_hex = sha256_hex(&der);
        let device_name = fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Hayato".to_string());

        Ok(Self {
            device_id,
            device_name,
            cert,
            key,
            cert_fp_hex,
        })
    }

    fn identity_packet(&self, tcp_port: u16) -> KdePacket {
        KdePacket {
            id: now_ms(),
            kind: "kdeconnect.identity".to_string(),
            body: json!({
                "deviceId": self.device_id,
                "deviceName": self.device_name,
                "protocolVersion": PROTOCOL_VERSION,
                "deviceType": "laptop",
                "tcpPort": tcp_port,
                "incomingCapabilities": [
                    "kdeconnect.ping",
                    "kdeconnect.battery",
                    "kdeconnect.battery.request",
                    "kdeconnect.notification",
                    "kdeconnect.clipboard",
                    "kdeconnect.clipboard.connect",
                    "kdeconnect.share.request",
                    "kdeconnect.share.request.update",
                    "kdeconnect.mousepad.request",
                    "kdeconnect.mousepad.echo",
                    "kdeconnect.presenter",
                    "kdeconnect.mpris.request",
                    "kdeconnect.findmyphone.request",
                    "kdeconnect.runcommand.request"
                ],
                "outgoingCapabilities": [
                    "kdeconnect.ping",
                    "kdeconnect.battery",
                    "kdeconnect.battery.request",
                    "kdeconnect.notification",
                    "kdeconnect.clipboard",
                    "kdeconnect.clipboard.connect",
                    "kdeconnect.share.request",
                    "kdeconnect.mousepad.keyboardstate",
                    "kdeconnect.mpris",
                    "kdeconnect.findmyphone.request",
                    "kdeconnect.runcommand"
                ]
            }),
            payload_size: None,
            payload_transfer_info: None,
        }
    }

    fn ssl_acceptor(&self) -> Result<SslAcceptor, String> {
        let mut b = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())
            .map_err(|e| format!("KDE SSL acceptor: {e}"))?;
        b.set_private_key(&self.key).map_err(|e| e.to_string())?;
        b.set_certificate(&self.cert).map_err(|e| e.to_string())?;
        b.set_verify_callback(SslVerifyMode::PEER, |_, _| true);
        Ok(b.build())
    }

    fn ssl_connector(&self) -> Result<SslConnector, String> {
        let mut b = SslConnector::builder(SslMethod::tls())
            .map_err(|e| format!("KDE SSL connector: {e}"))?;
        b.set_private_key(&self.key).map_err(|e| e.to_string())?;
        b.set_certificate(&self.cert).map_err(|e| e.to_string())?;
        b.set_verify_callback(SslVerifyMode::NONE, |_, _| true);
        Ok(b.build())
    }
}

struct LiveSession {
    info: KdePeerInfo,
    cert_fp: String,
    out_tx: Sender<String>,
}

pub type ClipboardCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct SharedKde {
    identity: KdeIdentity,
    tcp_port: u16,
    downloads_dir: PathBuf,
    trust: Arc<Mutex<TrustStore>>,
    pending_auth: Mutex<HashMap<String, Sender<bool>>>,
    sessions: Mutex<HashMap<String, LiveSession>>,
    last_clip_hash: Mutex<String>,
    on_clipboard: ClipboardCallback,
    ev_tx: Sender<ConnectEvent>,
}

pub struct KdeConnectBridge {
    stop: Arc<AtomicBool>,
    shared: Arc<SharedKde>,
    udp_port: u16,
    pub tcp_port: u16,
}

impl Drop for KdeConnectBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut pending) = self.shared.pending_auth.lock() {
            for (_, tx) in pending.drain() {
                let _ = tx.send(false);
            }
        }
        if let Ok(mut sess) = self.shared.sessions.lock() {
            sess.clear();
        }
    }
}

impl KdeConnectBridge {
    pub fn start(
        trust: Arc<Mutex<TrustStore>>,
        identity_dir: Option<&Path>,
        downloads_dir: PathBuf,
        ev_tx: Sender<ConnectEvent>,
        on_clipboard: ClipboardCallback,
    ) -> Result<Self, String> {
        let dir = identity_dir
            .map(PathBuf::from)
            .unwrap_or_else(default_identity_dir);
        let identity = KdeIdentity::load_or_create(&dir)?;
        let tcp_listener = bind_kde_tcp()?;
        tcp_listener
            .set_nonblocking(true)
            .map_err(|e| format!("configure KDE TCP socket: {e}"))?;
        let tcp_port = tcp_listener
            .local_addr()
            .map_err(|e| format!("read KDE TCP port: {e}"))?
            .port();

        let shared = Arc::new(SharedKde {
            identity,
            tcp_port,
            downloads_dir,
            trust,
            pending_auth: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            last_clip_hash: Mutex::new(String::new()),
            on_clipboard,
            ev_tx,
        });
        let stop = Arc::new(AtomicBool::new(false));

        // TCP accept loop (dies strictly when `stop` flips)
        let stop_tcp = Arc::clone(&stop);
        let shared_tcp = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("kde-tcp".into())
            .spawn(move || {
                while !stop_tcp.load(Ordering::Relaxed) {
                    match tcp_listener.accept() {
                        Ok((stream, addr)) => {
                            let _ = stream.set_nonblocking(false);
                            let sh = Arc::clone(&shared_tcp);
                            let st = Arc::clone(&stop_tcp);
                            let _ = std::thread::Builder::new().name("kde-peer".into()).spawn(
                                move || {
                                    handle_tcp_stream(stream, addr, true, None, sh, st);
                                },
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(80));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("spawn KDE TCP loop: {e}"))?;

        // UDP 1716 discovery loop (gracefully optional if blocked)
        let udp_sock = UdpSocket::bind(("0.0.0.0", KDE_UDP_PORT))
            .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
            .ok();
        let udp_port = udp_sock
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        if let Some(sock) = udp_sock {
            let _ = sock.set_broadcast(true);
            let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
            let stop_udp = Arc::clone(&stop);
            let shared_udp = Arc::clone(&shared);
            let _ = std::thread::Builder::new()
                .name("kde-udp".into())
                .spawn(move || {
                    udp_discovery_loop(sock, shared_udp, stop_udp);
                });
        }

        let bridge = Self {
            stop,
            shared,
            udp_port,
            tcp_port,
        };
        bridge.announce_now();
        Ok(bridge)
    }

    pub fn respond_auth(&self, req_id: &str, approved: bool) -> bool {
        let Some(kde_id) = req_id.strip_prefix("kde:") else {
            return false;
        };
        if let Ok(mut pending) = self.shared.pending_auth.lock()
            && let Some(tx) = pending.remove(kde_id)
        {
            let _ = tx.send(approved);
            return true;
        }
        false
    }

    pub fn announce_now(&self) {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            let _ = sock.set_broadcast(true);
            send_udp_identity(&sock, &self.shared, self.udp_port.max(KDE_UDP_PORT));
        }
    }

    pub fn live_peers(&self) -> Vec<KdePeerInfo> {
        self.shared
            .sessions
            .lock()
            .map(|s| s.values().map(|v| v.info.clone()).collect())
            .unwrap_or_default()
    }

    pub fn set_last_clip_hash(&self, hash: &str) {
        if let Ok(mut g) = self.shared.last_clip_hash.lock() {
            *g = hash.to_string();
        }
    }

    pub fn send_clipboard(&self, text: &str) -> usize {
        self.set_last_clip_hash(&sha256_hex(text.as_bytes()));
        let pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.clipboard".to_string(),
            body: json!({ "content": text }),
            payload_size: None,
            payload_transfer_info: None,
        };
        broadcast_kde_packet(&self.shared, &pkt)
    }

    pub fn send_ring(&self) -> usize {
        let pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.findmyphone.request".to_string(),
            body: json!({}),
            payload_size: None,
            payload_transfer_info: None,
        };
        broadcast_kde_packet(&self.shared, &pkt)
    }

    pub fn send_mpris(&self, snap: &MediaSnapshot) -> usize {
        let pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.mpris".to_string(),
            body: json!({
                "player": if snap.player.is_empty() { "Hayato" } else { snap.player.as_str() },
                "title": snap.title,
                "artist": snap.artist,
                "isPlaying": snap.status == "Playing",
                "canPause": true,
                "canPlay": true,
                "canGoNext": true,
                "canGoPrevious": true,
                "canSeek": true,
                "pos": snap.position_s * 1000,
                "length": snap.length_s * 1000,
                "volume": snap.volume_pct
            }),
            payload_size: None,
            payload_transfer_info: None,
        };
        broadcast_kde_packet(&self.shared, &pkt)
    }

    pub fn send_share_url(&self, url: &str) -> usize {
        let pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.share.request".to_string(),
            body: json!({ "url": url }),
            payload_size: None,
            payload_transfer_info: None,
        };
        broadcast_kde_packet(&self.shared, &pkt)
    }

    pub fn send_share_file(&self, path: &Path) -> Result<usize, String> {
        let meta = fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
        let size = meta.len();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file.bin")
            .to_string();
        let peers = self.live_peers();
        if peers.is_empty() {
            return Ok(0);
        }

        let listener = TcpListener::bind("0.0.0.0:0").map_err(|e| format!("bind payload: {e}"))?;
        let _ = listener.set_nonblocking(true);
        let port = listener
            .local_addr()
            .map_err(|e| format!("payload port: {e}"))?
            .port();
        let acceptor = self.shared.identity.ssl_acceptor()?;
        let file_path = path.to_path_buf();
        let stop_bg = Arc::clone(&self.stop);

        let _ = std::thread::Builder::new()
            .name("kde-payload-tx".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(45);
                while !stop_bg.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            if let Ok(mut ssl) = acceptor.accept(tcp)
                                && let Ok(mut f) = File::open(&file_path)
                            {
                                let mut buf = [0u8; 32_768];
                                while let Ok(n) = f.read(&mut buf) {
                                    if n == 0 || ssl.write_all(&buf[..n]).is_err() {
                                        break;
                                    }
                                }
                            }
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(40));
                        }
                        Err(_) => break,
                    }
                }
            });

        let pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.share.request".to_string(),
            body: json!({ "filename": filename }),
            payload_size: Some(size as i64),
            payload_transfer_info: Some(json!({ "port": port })),
        };
        Ok(broadcast_kde_packet(&self.shared, &pkt))
    }

    pub fn unpair_device(&self, id: &str) -> Result<bool, String> {
        let unpair_pkt = KdePacket {
            id: now_ms(),
            kind: "kdeconnect.pair".to_string(),
            body: json!({ "pair": false }),
            payload_size: None,
            payload_transfer_info: None,
        };
        if let Ok(line) = serde_json::to_string(&unpair_pkt)
            && let Ok(sess) = self.shared.sessions.lock()
            && let Some(s) = sess.get(id)
        {
            let _ = s.out_tx.send(format!("{line}\n"));
        }
        if let Ok(mut sess) = self.shared.sessions.lock() {
            sess.remove(id);
        }
        self.shared
            .trust
            .lock()
            .map_err(|_| "trust lock poisoned".to_string())?
            .unpair(id)
    }
}

fn udp_discovery_loop(sock: UdpSocket, shared: Arc<SharedKde>, stop: Arc<AtomicBool>) {
    let mut buf = [0u8; 8192];
    let mut last_bcast = std::time::Instant::now() - Duration::from_secs(10);
    while !stop.load(Ordering::Relaxed) {
        if last_bcast.elapsed() >= Duration::from_secs(5) {
            send_udp_identity(&sock, &shared, KDE_UDP_PORT);
            last_bcast = std::time::Instant::now();
        }
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Ok(pkt) = serde_json::from_slice::<KdePacket>(&buf[..n])
                    && pkt.kind == "kdeconnect.identity"
                {
                    let peer_id = pkt.body["deviceId"].as_str().unwrap_or("");
                    let peer_port = pkt.body["tcpPort"].as_u64().unwrap_or(0) as u16;
                    if !peer_id.is_empty() && peer_id != shared.identity.device_id && peer_port > 0
                    {
                        let already_live = shared
                            .sessions
                            .lock()
                            .map(|s| s.contains_key(peer_id))
                            .unwrap_or(false);
                        if !already_live {
                            let mut target = src;
                            target.set_port(peer_port);
                            let sh = Arc::clone(&shared);
                            let st = Arc::clone(&stop);
                            let expected_id = peer_id.to_string();
                            let _ = std::thread::Builder::new().name("kde-dial".into()).spawn(
                                move || {
                                    if let Ok(stream) =
                                        TcpStream::connect_timeout(&target, Duration::from_secs(3))
                                    {
                                        handle_tcp_stream(
                                            stream,
                                            target,
                                            false,
                                            Some(expected_id),
                                            sh,
                                            st,
                                        );
                                    }
                                },
                            );
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
}

fn send_udp_identity(sock: &UdpSocket, shared: &Arc<SharedKde>, target_port: u16) {
    let pkt = shared.identity.identity_packet(shared.tcp_port);
    if let Ok(mut line) = serde_json::to_string(&pkt) {
        line.push('\n');
        let _ = sock.send_to(line.as_bytes(), (Ipv4Addr::BROADCAST, target_port));
    }
}

fn handle_tcp_stream(
    mut tcp: TcpStream,
    peer_addr: SocketAddr,
    incoming_tcp: bool,
    _expected_id: Option<String>,
    shared: Arc<SharedKde>,
    stop: Arc<AtomicBool>,
) {
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(8)));

    // Step 1: plain TCP `kdeconnect.identity` handshake
    let peer_ident = if incoming_tcp {
        let Some(p) = read_plain_identity(&mut tcp) else {
            return;
        };
        let own = shared.identity.identity_packet(shared.tcp_port);
        if write_plain_packet(&mut tcp, &own).is_err() {
            return;
        }
        p
    } else {
        let own = shared.identity.identity_packet(shared.tcp_port);
        if write_plain_packet(&mut tcp, &own).is_err() {
            return;
        }
        match read_plain_identity(&mut tcp) {
            Some(p) => p,
            None => return,
        }
    };

    if peer_ident.kind != "kdeconnect.identity" {
        return;
    }
    let peer_id = peer_ident.body["deviceId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if peer_id.is_empty() || peer_id == shared.identity.device_id {
        return;
    }
    let peer_name = peer_ident.body["deviceName"]
        .as_str()
        .unwrap_or("KDE Connect Device")
        .to_string();
    let peer_type = peer_ident.body["deviceType"]
        .as_str()
        .unwrap_or("phone")
        .to_string();

    // Step 2: Upgrade TCP socket to TLS (peek 1st byte so Android & Desktop
    // implementations never deadlock on ClientHello vs ServerHello role order).
    let _ = tcp.set_read_timeout(Some(Duration::from_millis(650)));
    let mut peek_b = [0u8; 1];
    let peer_sent_client_hello = tcp
        .peek(&mut peek_b)
        .map(|n| n > 0 && peek_b[0] == 0x16)
        .unwrap_or(false);
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));

    let mut ssl = if peer_sent_client_hello {
        let Ok(acc) = shared.identity.ssl_acceptor() else {
            return;
        };
        let Ok(s) = acc.accept(tcp) else {
            return;
        };
        s
    } else {
        let Ok(conn) = shared.identity.ssl_connector() else {
            return;
        };
        let Ok(s) = conn.connect(&peer_id, tcp) else {
            return;
        };
        s
    };

    let peer_cert_fp = ssl
        .ssl()
        .peer_certificate()
        .and_then(|c| c.digest(MessageDigest::sha256()).ok())
        .map(|d| sha256_hex_slice(&d))
        .unwrap_or_default();
    if peer_cert_fp.is_empty() {
        return;
    }

    let ver_key = derive_kde_verification_key(&shared.identity.cert_fp_hex, &peer_cert_fp);
    let remote_ip = peer_addr.ip().to_string();
    let (out_tx, out_rx) = mpsc::channel::<String>();

    let info = KdePeerInfo {
        device_id: peer_id.clone(),
        name: peer_name.clone(),
        device_type: peer_type.clone(),
        remote_ip: remote_ip.clone(),
        battery_pct: None,
        charging: false,
    };

    if let Ok(mut sess) = shared.sessions.lock() {
        sess.insert(
            peer_id.clone(),
            LiveSession {
                info: info.clone(),
                cert_fp: peer_cert_fp.clone(),
                out_tx: out_tx.clone(),
            },
        );
    }

    let is_trusted_initially = shared
        .trust
        .lock()
        .map(|t| t.verify_kde(&peer_id, &peer_cert_fp).is_some())
        .unwrap_or(false);
    if is_trusted_initially {
        send_initial_kde_state(&out_tx);
        let _ = shared.ev_tx.send(ConnectEvent::SyncUpdated {
            summary: format!("KDE Connect linked: {peer_name}"),
        });
    }

    // Step 3: Encrypted JSON-lines packet loop
    let _ = ssl
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(160)));
    let mut line_buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];

    'pkt: while !stop.load(Ordering::Relaxed) {
        // Ensure peer was not unpaired mid-session
        if let Ok(sess) = shared.sessions.lock()
            && !sess.contains_key(&peer_id)
        {
            break 'pkt;
        }
        loop {
            match out_rx.try_recv() {
                Ok(msg) => {
                    if ssl.write_all(msg.as_bytes()).is_err() || ssl.flush().is_err() {
                        break 'pkt;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'pkt,
            }
        }
        match ssl.read(&mut byte) {
            Ok(0) => break 'pkt,
            Ok(_) => {
                if byte[0] == b'\n' {
                    if let Ok(pkt) = serde_json::from_slice::<KdePacket>(&line_buf) {
                        handle_kde_packet(
                            pkt,
                            &peer_id,
                            &peer_name,
                            &peer_type,
                            &peer_cert_fp,
                            &remote_ip,
                            &ver_key,
                            &mut ssl,
                            &shared,
                        );
                    }
                    line_buf.clear();
                } else if line_buf.len() < 128 * 1024 {
                    line_buf.push(byte[0]);
                }
            }
            Err(ref e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break 'pkt,
        }
    }

    if let Ok(mut sess) = shared.sessions.lock() {
        sess.remove(&peer_id);
    }
    let _ = shared.ev_tx.send(ConnectEvent::PeerDisconnected {
        device_id: peer_id,
        name: peer_name,
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_kde_packet(
    pkt: KdePacket,
    peer_id: &str,
    peer_name: &str,
    peer_type: &str,
    peer_cert_fp: &str,
    remote_ip: &str,
    ver_key: &str,
    ssl: &mut SslStream<TcpStream>,
    shared: &Arc<SharedKde>,
) {
    if pkt.kind == "kdeconnect.pair" {
        let want_pair = pkt.body["pair"].as_bool().unwrap_or(false);
        if !want_pair {
            if let Ok(mut trust) = shared.trust.lock() {
                let _ = trust.unpair(peer_id);
            }
            return;
        }
        let already = shared
            .trust
            .lock()
            .map(|t| t.verify_kde(peer_id, peer_cert_fp).is_some())
            .unwrap_or(false);
        if already {
            let reply = KdePacket {
                id: now_ms(),
                kind: "kdeconnect.pair".to_string(),
                body: json!({ "pair": true }),
                payload_size: None,
                payload_transfer_info: None,
            };
            let _ = write_ssl_packet(ssl, &reply);
            return;
        }

        let (tx, rx) = mpsc::channel::<bool>();
        if let Ok(mut pending) = shared.pending_auth.lock() {
            if !pending.is_empty() {
                let deny = KdePacket {
                    id: now_ms(),
                    kind: "kdeconnect.pair".to_string(),
                    body: json!({ "pair": false }),
                    payload_size: None,
                    payload_transfer_info: None,
                };
                let _ = write_ssl_packet(ssl, &deny);
                return;
            }
            pending.insert(peer_id.to_string(), tx);
        }
        let _ = shared.ev_tx.send(ConnectEvent::AuthRequested {
            req_id: format!("kde:{peer_id}"),
            device_label: format!("{peer_name} (KDE Connect)"),
            remote_ip: remote_ip.to_string(),
            sas_code: ver_key.to_string(),
        });

        let sh_wait = Arc::clone(shared);
        let pid = peer_id.to_string();
        let pname = peer_name.to_string();
        let ptype = peer_type.to_string();
        let cfp = peer_cert_fp.to_string();
        let _ = std::thread::Builder::new()
            .name("kde-pair-gate".into())
            .spawn(move || {
                let approved = rx.recv_timeout(Duration::from_secs(60)).unwrap_or(false);
                if let Ok(mut pending) = sh_wait.pending_auth.lock() {
                    pending.remove(&pid);
                }
                let mut reply = KdePacket {
                    id: now_ms(),
                    kind: "kdeconnect.pair".to_string(),
                    body: json!({ "pair": false }),
                    payload_size: None,
                    payload_transfer_info: None,
                };
                if approved
                    && let Ok(mut trust) = sh_wait.trust.lock()
                    && trust.pair_kde_device(&pid, &pname, &ptype, &cfp).is_ok()
                {
                    reply.body = json!({ "pair": true });
                    let _ = sh_wait.ev_tx.send(ConnectEvent::SyncUpdated {
                        summary: format!("Paired KDE Connect: {pname}"),
                    });
                }
                if let Ok(line) = serde_json::to_string(&reply)
                    && let Ok(sess) = sh_wait.sessions.lock()
                    && let Some(s) = sess.get(&pid)
                {
                    let _ = s.out_tx.send(format!("{line}\n"));
                    if approved {
                        send_initial_kde_state(&s.out_tx);
                    }
                }
            });
        return;
    }

    // All plugin packets require prior explicit pairing verification
    let is_trusted = shared
        .trust
        .lock()
        .map(|t| t.verify_kde(peer_id, peer_cert_fp).is_some())
        .unwrap_or(false);
    if !is_trusted {
        return;
    }

    match pkt.kind.as_str() {
        "kdeconnect.battery" => {
            let pct = pkt.body["currentCharge"]
                .as_i64()
                .filter(|&v| (0..=100).contains(&v))
                .map(|v| v as u8);
            let charging = pkt.body["isCharging"].as_bool().unwrap_or(false);
            if let Ok(mut sess) = shared.sessions.lock()
                && let Some(s) = sess.get_mut(peer_id)
            {
                s.info.battery_pct = pct;
                s.info.charging = charging;
            }
            if let Some(p) = pct {
                let _ = shared.ev_tx.send(ConnectEvent::SyncUpdated {
                    summary: format!(
                        "{peer_name} battery {p}%{}",
                        if charging { " ⚡" } else { "" }
                    ),
                });
            }
        }
        "kdeconnect.clipboard" | "kdeconnect.clipboard.connect" => {
            if let Some(content) = pkt.body["content"].as_str()
                && !content.is_empty()
            {
                let h = sha256_hex(content.as_bytes());
                let is_echo = shared
                    .last_clip_hash
                    .lock()
                    .map(|mut prev| {
                        if *prev == h {
                            true
                        } else {
                            *prev = h;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !is_echo {
                    (shared.on_clipboard)(content, peer_name);
                }
            }
        }
        "kdeconnect.notification" => {
            if pkt.body["isCancel"].as_bool().unwrap_or(false) {
                return;
            }
            let app = pkt.body["appName"].as_str().unwrap_or(peer_name);
            let title = pkt.body["title"].as_str().unwrap_or(app);
            let text = pkt.body["text"].as_str().unwrap_or("");
            if !title.is_empty() || !text.is_empty() {
                let _ = send_desktop_alert(app, title, text);
                let _ = shared.ev_tx.send(ConnectEvent::AlertReceived {
                    title: title.to_string(),
                    body: text.to_string(),
                    from: format!("{peer_name} · {app}"),
                });
            }
        }
        "kdeconnect.share.request" => {
            if let Some(url) = pkt.body["url"].as_str() {
                let trimmed = url.trim();
                if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
                    && !trimmed.chars().any(char::is_control)
                {
                    crate::system::spawn_detached_argv("hayato-browser", &[trimmed]);
                    let _ = shared.ev_tx.send(ConnectEvent::LinkReceived {
                        url: trimmed.to_string(),
                        from: peer_name.to_string(),
                    });
                }
            } else if let Some(text) = pkt.body["text"].as_str() {
                (shared.on_clipboard)(text, peer_name);
            } else if let Some(fname) = pkt.body["filename"].as_str()
                && let Some(port) = pkt
                    .payload_transfer_info
                    .as_ref()
                    .and_then(|v| v["port"].as_u64())
                    .map(|p| p as u16)
                && port > 0
            {
                let size = pkt.payload_size.unwrap_or(0).max(0) as u64;
                if size <= MAX_PAYLOAD_BYTES {
                    let sh = Arc::clone(shared);
                    let ip = remote_ip.to_string();
                    let safe = sanitize_filename(fname);
                    let expected_fp = peer_cert_fp.to_string();
                    let _ = std::thread::Builder::new()
                        .name("kde-payload-rx".into())
                        .spawn(move || {
                            receive_kde_payload(
                                &ip,
                                port,
                                &safe,
                                size,
                                &expected_fp,
                                &sh.identity,
                                &sh.downloads_dir,
                                &sh.ev_tx,
                            );
                        });
                }
            }
        }
        "kdeconnect.mpris.request" => {
            let cmd = if pkt.body["action"].as_str() == Some("PlayPause") {
                Some("play_pause")
            } else if pkt.body["action"].as_str() == Some("Next") {
                Some("next")
            } else if pkt.body["action"].as_str() == Some("Previous") {
                Some("prev")
            } else if let Some(vol) = pkt.body["setVolume"].as_i64() {
                let _ = std::process::Command::new("wpctl")
                    .args([
                        "set-volume",
                        "@DEFAULT_AUDIO_SINK@",
                        &format!("{}%", vol.clamp(0, 100)),
                    ])
                    .status();
                None
            } else {
                None
            };
            if let Some(c) = cmd {
                let _ = exec_media_cmd(c);
                let _ = shared.ev_tx.send(ConnectEvent::MediaCmdExecuted {
                    cmd: c.to_string(),
                    from: peer_name.to_string(),
                });
            }
            let snap = poll_media();
            let reply = KdePacket {
                id: now_ms(),
                kind: "kdeconnect.mpris".to_string(),
                body: json!({
                    "player": if snap.player.is_empty() { "Hayato" } else { snap.player.as_str() },
                    "title": snap.title,
                    "artist": snap.artist,
                    "isPlaying": snap.status == "Playing",
                    "volume": snap.volume_pct
                }),
                payload_size: None,
                payload_transfer_info: None,
            };
            let _ = write_ssl_packet(ssl, &reply);
        }
        "kdeconnect.ping" => {
            let msg = pkt.body["message"].as_str().unwrap_or("Ping!");
            let _ = send_desktop_alert(peer_name, "KDE Connect Ping", msg);
            let _ = shared.ev_tx.send(ConnectEvent::AlertReceived {
                title: "Ping".to_string(),
                body: msg.to_string(),
                from: peer_name.to_string(),
            });
        }
        "kdeconnect.battery.request" => {
            if let Some(pkt) = build_laptop_battery_packet() {
                let _ = write_ssl_packet(ssl, &pkt);
            }
        }
        "kdeconnect.findmyphone.request" => {
            let _ = send_desktop_alert(
                peer_name,
                "Find My Computer",
                "Locate ping triggered from phone",
            );
            let _ = shared.ev_tx.send(ConnectEvent::AlertReceived {
                title: "Find My Computer".to_string(),
                body: "Locate ping from phone".to_string(),
                from: peer_name.to_string(),
            });
        }
        "kdeconnect.mousepad.request" => {
            handle_mousepad_packet(&pkt.body);
        }
        "kdeconnect.presenter" => {
            if !pkt.body["stop"].as_bool().unwrap_or(false) {
                let dx = (pkt.body["dx"].as_f64().unwrap_or(0.0) * 180.0).round() as i32;
                let dy = (pkt.body["dy"].as_f64().unwrap_or(0.0) * 180.0).round() as i32;
                if dx != 0 || dy != 0 {
                    let _ = sway_cursor_move(dx, dy);
                }
            }
        }
        "kdeconnect.runcommand.request" => {
            if pkt.body["requestCommandList"].as_bool().unwrap_or(false) {
                let _ = write_ssl_packet(ssl, &build_runcommand_list_packet());
            } else if let Some(key) = pkt.body["key"].as_str()
                && let Some(label) = exec_curated_command(key)
            {
                let _ = shared.ev_tx.send(ConnectEvent::SyncUpdated {
                    summary: format!("{peer_name} ran: {label}"),
                });
            }
        }
        _ => {}
    }
}

struct CuratedCmd {
    id: &'static str,
    name: &'static str,
    display: &'static str,
    bin: &'static str,
    args: &'static [&'static str],
}

const CURATED_COMMANDS: &[CuratedCmd] = &[
    CuratedCmd {
        id: "hayato-media-toggle",
        name: "Play / Pause Media",
        display: "playerctl play-pause",
        bin: "playerctl",
        args: &["play-pause"],
    },
    CuratedCmd {
        id: "hayato-audio-mute",
        name: "Mute / Unmute Audio",
        display: "wpctl set-mute toggle",
        bin: "wpctl",
        args: &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"],
    },
    CuratedCmd {
        id: "hayato-screenshot",
        name: "Take Screenshot",
        display: "hayato screenshot",
        bin: "hayato",
        args: &["screenshot"],
    },
    CuratedCmd {
        id: "hayato-cassette",
        name: "Toggle Cassette Music",
        display: "hayato-cassette --toggle",
        bin: "hayato-cassette",
        args: &["--toggle"],
    },
    CuratedCmd {
        id: "hayato-lock",
        name: "Lock Screen",
        display: "loginctl lock-session",
        bin: "loginctl",
        args: &["lock-session"],
    },
    CuratedCmd {
        id: "hayato-suspend",
        name: "Sleep / Suspend",
        display: "systemctl suspend",
        bin: "systemctl",
        args: &["suspend"],
    },
];

fn build_runcommand_list_packet() -> KdePacket {
    let mut map = serde_json::Map::new();
    for c in CURATED_COMMANDS {
        map.insert(
            c.id.to_string(),
            json!({ "name": c.name, "command": c.display }),
        );
    }
    let list_json = Value::Object(map).to_string();
    KdePacket {
        id: now_ms(),
        kind: "kdeconnect.runcommand".to_string(),
        body: json!({ "commandList": list_json }),
        payload_size: None,
        payload_transfer_info: None,
    }
}

fn exec_curated_command(key: &str) -> Option<&'static str> {
    let entry = CURATED_COMMANDS.iter().find(|c| c.id == key)?;
    let _ = std::process::Command::new(entry.bin)
        .args(entry.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    Some(entry.name)
}

fn send_initial_kde_state(out_tx: &Sender<String>) {
    let packets: Vec<Option<KdePacket>> = vec![
        Some(KdePacket {
            id: now_ms(),
            kind: "kdeconnect.battery.request".to_string(),
            body: json!({ "request": true }),
            payload_size: None,
            payload_transfer_info: None,
        }),
        build_laptop_battery_packet(),
        Some(build_runcommand_list_packet()),
        Some(KdePacket {
            id: now_ms(),
            kind: "kdeconnect.mousepad.keyboardstate".to_string(),
            body: json!({ "state": true }),
            payload_size: None,
            payload_transfer_info: None,
        }),
    ];
    for pkt in packets.into_iter().flatten() {
        if let Ok(line) = serde_json::to_string(&pkt) {
            let _ = out_tx.send(format!("{line}\n"));
        }
    }
}

fn build_laptop_battery_packet() -> Option<KdePacket> {
    let (pct, charging) = read_laptop_battery()?;
    Some(KdePacket {
        id: now_ms(),
        kind: "kdeconnect.battery".to_string(),
        body: json!({
            "currentCharge": pct,
            "isCharging": charging,
            "thresholdEvent": 0
        }),
        payload_size: None,
        payload_transfer_info: None,
    })
}

fn read_laptop_battery() -> Option<(u8, bool)> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let kind = fs::read_to_string(p.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        let cap: u8 = fs::read_to_string(p.join("capacity"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let status = fs::read_to_string(p.join("status")).unwrap_or_default();
        return Some((cap.min(100), status.trim() == "Charging"));
    }
    None
}

fn sway_cursor_move(dx: i32, dy: i32) -> bool {
    std::process::Command::new("swaymsg")
        .args([
            "-q",
            "seat",
            "-",
            "cursor",
            "move",
            &dx.to_string(),
            &dy.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn sway_cursor_button(btn: &str, action: &str) {
    let _ = std::process::Command::new("swaymsg")
        .args(["-q", "seat", "-", "cursor", action, btn])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn handle_mousepad_packet(body: &Value) {
    if body["singleclick"].as_bool().unwrap_or(false) {
        sway_cursor_button("button1", "press");
        sway_cursor_button("button1", "release");
    } else if body["doubleclick"].as_bool().unwrap_or(false) {
        for _ in 0..2 {
            sway_cursor_button("button1", "press");
            sway_cursor_button("button1", "release");
        }
    } else if body["rightclick"].as_bool().unwrap_or(false) {
        sway_cursor_button("button3", "press");
        sway_cursor_button("button3", "release");
    } else if body["middleclick"].as_bool().unwrap_or(false) {
        sway_cursor_button("button2", "press");
        sway_cursor_button("button2", "release");
    } else if body["singlehold"].as_bool().unwrap_or(false) {
        sway_cursor_button("button1", "press");
    } else if body["singlerelease"].as_bool().unwrap_or(false) {
        sway_cursor_button("button1", "release");
    }

    if !body["scroll"].as_bool().unwrap_or(false) {
        let dx = body["dx"].as_f64().unwrap_or(0.0).round() as i32;
        let dy = body["dy"].as_f64().unwrap_or(0.0).round() as i32;
        if dx != 0 || dy != 0 {
            let _ = sway_cursor_move(dx, dy);
        }
    }

    if let Some(sk) = body["specialKey"].as_u64()
        && let Some(key_name) = map_kde_special_key(sk as u32)
    {
        let mut args: Vec<&str> = Vec::new();
        if body["ctrl"].as_bool().unwrap_or(false) {
            args.extend(["-M", "ctrl"]);
        }
        if body["alt"].as_bool().unwrap_or(false) {
            args.extend(["-M", "alt"]);
        }
        if body["shift"].as_bool().unwrap_or(false) {
            args.extend(["-M", "shift"]);
        }
        args.extend(["-k", key_name]);
        let _ = std::process::Command::new("wtype")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    } else if let Some(text) = body["key"].as_str()
        && !text.is_empty()
        && text.len() <= 64
        && !text.chars().any(char::is_control)
    {
        let _ = std::process::Command::new("wtype")
            .args(["--", text])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

pub fn map_kde_special_key(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("BackSpace"),
        2 | 32 => Some("Tab"),
        4 => Some("Left"),
        5 => Some("Up"),
        6 => Some("Right"),
        7 => Some("Down"),
        8 => Some("Page_Up"),
        9 => Some("Page_Down"),
        10 => Some("Home"),
        11 => Some("End"),
        12 => Some("Return"),
        13 => Some("Delete"),
        14 => Some("Escape"),
        15 => Some("Print"),
        16 => Some("F1"),
        17 => Some("F2"),
        18 => Some("F3"),
        19 => Some("F4"),
        20 => Some("F5"),
        21 => Some("F6"),
        22 => Some("F7"),
        23 => Some("F8"),
        24 => Some("F9"),
        25 => Some("F10"),
        26 => Some("F11"),
        27 => Some("F12"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn receive_kde_payload(
    ip: &str,
    port: u16,
    safe_name: &str,
    expected_size: u64,
    expected_fp: &str,
    identity: &KdeIdentity,
    downloads_dir: &Path,
    ev_tx: &Sender<ConnectEvent>,
) {
    let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() else {
        return;
    };
    let Ok(tcp) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) else {
        return;
    };
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(20)));
    let Ok(conn) = identity.ssl_connector() else {
        return;
    };
    let Ok(mut ssl) = conn.connect(ip, tcp) else {
        return;
    };
    let got_fp = ssl
        .ssl()
        .peer_certificate()
        .and_then(|c| c.digest(MessageDigest::sha256()).ok())
        .map(|d| sha256_hex_slice(&d))
        .unwrap_or_default();
    if got_fp != expected_fp {
        return;
    }

    let _ = fs::create_dir_all(downloads_dir);
    let Ok((dest_path, mut out_file)) = create_unique_file(downloads_dir, safe_name) else {
        return;
    };
    let saved_name = dest_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(safe_name);
    let mut buf = [0u8; 32_768];
    let mut total = 0u64;
    loop {
        match ssl.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > MAX_PAYLOAD_BYTES || out_file.write_all(&buf[..n]).is_err() {
                    let _ = fs::remove_file(&dest_path);
                    return;
                }
            }
            Err(_) => break,
        }
    }
    let summary = format!(
        "KDE Connect received {saved_name} ({}) \u{2192} {}",
        human_size(if expected_size > 0 {
            expected_size
        } else {
            total
        }),
        downloads_dir.display()
    );
    let _ = ev_tx.send(ConnectEvent::TransferFinished { summary });
}

fn broadcast_kde_packet(shared: &Arc<SharedKde>, pkt: &KdePacket) -> usize {
    let Ok(mut line) = serde_json::to_string(pkt) else {
        return 0;
    };
    line.push('\n');
    let Ok(sess) = shared.sessions.lock() else {
        return 0;
    };
    let Ok(trust) = shared.trust.lock() else {
        return 0;
    };
    let mut count = 0;
    for (id, s) in sess.iter() {
        if trust.verify_kde(id, &s.cert_fp).is_some() && s.out_tx.send(line.clone()).is_ok() {
            count += 1;
        }
    }
    count
}

fn read_plain_identity(tcp: &mut TcpStream) -> Option<KdePacket> {
    let mut buf = Vec::with_capacity(512);
    let mut b = [0u8; 1];
    while buf.len() < 16 * 1024 {
        match tcp.read(&mut b) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                if b[0] == b'\n' {
                    break;
                }
                buf.push(b[0]);
            }
        }
    }
    serde_json::from_slice::<KdePacket>(&buf).ok()
}

fn write_plain_packet(tcp: &mut TcpStream, pkt: &KdePacket) -> std::io::Result<()> {
    let line = serde_json::to_string(pkt).unwrap_or_default();
    tcp.write_all(format!("{line}\n").as_bytes())?;
    tcp.flush()
}

fn write_ssl_packet(ssl: &mut SslStream<TcpStream>, pkt: &KdePacket) -> std::io::Result<()> {
    let line = serde_json::to_string(pkt).unwrap_or_default();
    ssl.write_all(format!("{line}\n").as_bytes())?;
    ssl.flush()
}

fn bind_kde_tcp() -> Result<TcpListener, String> {
    for port in KDE_UDP_PORT..=1764 {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", port)) {
            return Ok(l);
        }
    }
    TcpListener::bind("0.0.0.0:0").map_err(|e| format!("bind KDE TCP fallback: {e}"))
}

fn build_kde_cert(key: &PKey<Private>, cn_device_id: &str) -> Result<X509, String> {
    let mut b = X509Builder::new().map_err(|e| e.to_string())?;
    b.set_version(2).map_err(|e| e.to_string())?;
    let mut serial = BigNum::new().map_err(|e| e.to_string())?;
    serial
        .rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
        .map_err(|e| e.to_string())?;
    let sasn1 = serial.to_asn1_integer().map_err(|e| e.to_string())?;
    b.set_serial_number(&sasn1).map_err(|e| e.to_string())?;

    let mut name = X509NameBuilder::new().map_err(|e| e.to_string())?;
    name.append_entry_by_text("CN", cn_device_id)
        .map_err(|e| e.to_string())?;
    let subj = name.build();
    b.set_subject_name(&subj).map_err(|e| e.to_string())?;
    b.set_issuer_name(&subj).map_err(|e| e.to_string())?;
    b.set_pubkey(key).map_err(|e| e.to_string())?;
    let nb = Asn1Time::days_from_now(0).map_err(|e| e.to_string())?;
    let na = Asn1Time::days_from_now(3650).map_err(|e| e.to_string())?;
    b.set_not_before(&nb).map_err(|e| e.to_string())?;
    b.set_not_after(&na).map_err(|e| e.to_string())?;
    b.sign(key, MessageDigest::sha256())
        .map_err(|e| e.to_string())?;
    Ok(b.build())
}

fn derive_kde_verification_key(fp_a: &str, fp_b: &str) -> String {
    let (first, second) = if fp_a <= fp_b {
        (fp_a, fp_b)
    } else {
        (fp_b, fp_a)
    };
    let digest = sha256(format!("{first}:{second}").as_bytes());
    format!(
        "{:02X}{:02X}-{:02X}{:02X}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

fn sha256_hex_slice(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn write_mode_600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    f.write_all(bytes)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};

    use super::*;

    #[test]
    fn identity_cert_and_kde_packet_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ident = KdeIdentity::load_or_create(dir.path()).unwrap();
        assert_eq!(ident.device_id.len(), 36);
        let pkt = ident.identity_packet(1716);
        let json_line = serde_json::to_string(&pkt).unwrap();
        let parsed: KdePacket = serde_json::from_str(&json_line).unwrap();
        assert_eq!(parsed.kind, "kdeconnect.identity");
        assert_eq!(parsed.body["tcpPort"], 1716);

        // Verification key is order-independent across peers
        let k1 = derive_kde_verification_key(&ident.cert_fp_hex, "deadbeef");
        let k2 = derive_kde_verification_key("deadbeef", &ident.cert_fp_hex);
        assert_eq!(k1, k2);

        // Curated runcommand list & special key mapping
        let cmd_pkt = build_runcommand_list_packet();
        assert_eq!(cmd_pkt.kind, "kdeconnect.runcommand");
        assert!(
            cmd_pkt.body["commandList"]
                .as_str()
                .unwrap()
                .contains("hayato-lock")
        );
        assert_eq!(map_kde_special_key(12), Some("Return"));
        assert_eq!(map_kde_special_key(1), Some("BackSpace"));
        assert_eq!(exec_curated_command("unknown-shell-injection"), None);
    }

    #[test]
    fn tls_loopback_pairing_and_battery_packet_work() {
        let dir_a = tempfile::tempdir().unwrap();
        let trust_a = Arc::new(Mutex::new(TrustStore::load_from(
            dir_a.path().join("devices.json"),
        )));
        let (ev_tx, ev_rx) = mpsc::channel();
        let clip_hit = Arc::new(Mutex::new(String::new()));
        let clip_ref = Arc::clone(&clip_hit);
        let bridge = KdeConnectBridge::start(
            trust_a,
            Some(dir_a.path()),
            dir_a.path().to_path_buf(),
            ev_tx,
            Arc::new(move |text, _| {
                *clip_ref.lock().unwrap() = text.to_string();
            }),
        )
        .expect("start KDE Connect bridge");

        // Simulated remote KDE Connect phone connecting over localhost TCP + TLS
        let dir_b = tempfile::tempdir().unwrap();
        let ident_b = KdeIdentity::load_or_create(dir_b.path()).unwrap();
        let mut tcp = TcpStream::connect(("127.0.0.1", bridge.tcp_port)).unwrap();
        let mut hello_b = ident_b.identity_packet(1716);
        hello_b.body["deviceName"] = json!("Test KDE Phone");
        write_plain_packet(&mut tcp, &hello_b).unwrap();
        let peer_ident = read_plain_identity(&mut tcp).unwrap();
        assert_eq!(peer_ident.kind, "kdeconnect.identity");

        // Upgrade as TLS Client matching KDE Connect incoming peer role
        let conn_b = ident_b.ssl_connector().unwrap();
        let mut ssl = conn_b
            .connect(&bridge.shared.identity.device_id, tcp)
            .unwrap();

        // Send pair request -> triggers desktop AuthRequested with `kde:` prefix
        write_ssl_packet(
            &mut ssl,
            &KdePacket {
                id: 1,
                kind: "kdeconnect.pair".into(),
                body: json!({ "pair": true }),
                payload_size: None,
                payload_transfer_info: None,
            },
        )
        .unwrap();

        let req_id = loop {
            if let ConnectEvent::AuthRequested { req_id, .. } =
                ev_rx.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                break req_id;
            }
        };
        assert!(req_id.starts_with("kde:"));
        assert!(bridge.respond_auth(&req_id, true));

        let mut rd = BufReader::new(&mut ssl);
        let mut line = String::new();
        rd.read_line(&mut line).unwrap();
        let reply: KdePacket = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(reply.kind, "kdeconnect.pair");
        assert_eq!(reply.body["pair"], true);

        // Paired peer can send clipboard packet -> delivered cleanly
        write_ssl_packet(
            &mut ssl,
            &KdePacket {
                id: 2,
                kind: "kdeconnect.clipboard".into(),
                body: json!({ "content": "kde-sync-ok" }),
                payload_size: None,
                payload_transfer_info: None,
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(*clip_hit.lock().unwrap(), "kde-sync-ok");
    }
}
