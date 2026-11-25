//! On-demand, zero-daemon LocalSend v2 LAN peer discovery and push.
//!
//! - Joins UDP multicast `224.0.0.167:53317` *only* while `ShareScreen` is open.
//! - Announces Hayato once so nearby iOS/Android/macOS/Linux LocalSend devices
//!   reply with their own multicast JSON announcement.
//! - Pushes files or text via `curl -sk` using LocalSend v2 HTTP/HTTPS endpoints
//!   (`/api/localsend/v2/prepare-upload` + `/upload`), requiring zero new crates.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::SharePayload;
use super::server::generate_token;

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);
const MULTICAST_PORT: u16 = 53317;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSendPeer {
    pub alias: String,
    pub fingerprint: String,
    pub ip: String,
    pub port: u16,
    pub protocol: String,
    pub device_model: String,
}

#[derive(Deserialize)]
struct Announcement {
    alias: Option<String>,
    fingerprint: Option<String>,
    port: Option<u16>,
    protocol: Option<String>,
    #[serde(rename = "deviceModel")]
    device_model: Option<String>,
}

#[derive(Serialize)]
struct OurAnnouncement<'a> {
    alias: &'a str,
    version: &'a str,
    #[serde(rename = "deviceModel")]
    device_model: &'a str,
    #[serde(rename = "deviceType")]
    device_type: &'a str,
    fingerprint: &'a str,
    port: u16,
    protocol: &'a str,
    download: bool,
    announce: bool,
}

pub fn parse_announcement(raw: &str, ip: &str, self_fp: &str) -> Option<LocalSendPeer> {
    let ann: Announcement = serde_json::from_str(raw).ok()?;
    let fp = ann.fingerprint?;
    if fp == self_fp || fp.is_empty() {
        return None;
    }
    let alias = ann.alias.filter(|s| !s.trim().is_empty())?;
    let port = ann.port.unwrap_or(MULTICAST_PORT);
    let protocol = ann
        .protocol
        .filter(|p| p == "http" || p == "https")
        .unwrap_or_else(|| "https".to_string());
    let device_model = ann.device_model.unwrap_or_else(|| "Device".to_string());
    Some(LocalSendPeer {
        alias,
        fingerprint: fp,
        ip: ip.to_string(),
        port,
        protocol,
        device_model,
    })
}

pub struct PeerScanner {
    stop: Arc<AtomicBool>,
    peers: Arc<Mutex<Vec<LocalSendPeer>>>,
    scanning: Arc<AtomicBool>,
}

impl Drop for PeerScanner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl PeerScanner {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peers = Arc::new(Mutex::new(Vec::<LocalSendPeer>::new()));
        let scanning = Arc::new(AtomicBool::new(true));

        let stop_bg = Arc::clone(&stop);
        let peers_bg = Arc::clone(&peers);
        let scan_bg = Arc::clone(&scanning);

        std::thread::spawn(move || {
            let fp = generate_token();
            let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MULTICAST_PORT))
                .or_else(|_| UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
            let Ok(sock) = sock else {
                scan_bg.store(false, Ordering::Relaxed);
                return;
            };
            let _ = sock.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED);
            let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));

            let ann = OurAnnouncement {
                alias: "Hayato",
                version: "2.0",
                device_model: "Hayato OS",
                device_type: "laptop",
                fingerprint: &fp,
                port: MULTICAST_PORT,
                protocol: "http",
                download: false,
                announce: true,
            };
            if let Ok(json) = serde_json::to_vec(&ann) {
                let target = SocketAddrV4::new(MULTICAST_ADDR, MULTICAST_PORT);
                let _ = sock.send_to(&json, target);
            }

            let mut buf = [0u8; 4096];
            let deadline = std::time::Instant::now() + Duration::from_secs(4);
            while !stop_bg.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
                if let Ok((n, src)) = sock.recv_from(&mut buf) {
                    let ip = src.ip().to_string();
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if let Some(peer) = parse_announcement(&text, &ip, &fp)
                        && let Ok(mut list) = peers_bg.lock()
                        && !list.iter().any(|p| p.fingerprint == peer.fingerprint)
                    {
                        list.push(peer);
                    }
                }
            }
            scan_bg.store(false, Ordering::Relaxed);
        });

        Self {
            stop,
            peers,
            scanning,
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Vec<LocalSendPeer> {
        self.peers.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

/// Push a single file or text payload to a discovered LocalSend v2 peer using `curl`.
pub fn push_to_peer(peer: &LocalSendPeer, payload: &SharePayload) -> Result<String, String> {
    let (file_name, mime, bytes_len, temp_path, real_path) = match payload {
        SharePayload::File(p) | SharePayload::Image { path: p, .. } => {
            let meta = std::fs::metadata(p).map_err(|e| format!("Cannot read file: {e}"))?;
            if !meta.is_file() {
                return Err("Use Phone QR to share folders as a ZIP bundle".to_string());
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file.bin")
                .to_string();
            (
                name,
                "application/octet-stream",
                meta.len(),
                None,
                p.clone(),
            )
        }
        SharePayload::Text { content, .. } => {
            let tmp = std::env::temp_dir().join(format!(
                "hayato-ls-{}-{}.txt",
                std::process::id(),
                &generate_token()[..8]
            ));
            std::fs::write(&tmp, content.as_bytes())
                .map_err(|e| format!("Could not stage text: {e}"))?;
            (
                "hayato-share.txt".to_string(),
                "text/plain",
                content.len() as u64,
                Some(tmp.clone()),
                tmp,
            )
        }
        SharePayload::Url { url, .. } => {
            let tmp = std::env::temp_dir().join(format!(
                "hayato-ls-{}-{}.txt",
                std::process::id(),
                &generate_token()[..8]
            ));
            std::fs::write(&tmp, url.as_bytes())
                .map_err(|e| format!("Could not stage link: {e}"))?;
            (
                "hayato-link.txt".to_string(),
                "text/plain",
                url.len() as u64,
                Some(tmp.clone()),
                tmp,
            )
        }
        _ => return Err("Use Phone QR for multi-file or receive mode".to_string()),
    };

    let result = push_file_v2(peer, &real_path, &file_name, mime, bytes_len);
    if let Some(tmp) = temp_path {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

fn push_file_v2(
    peer: &LocalSendPeer,
    path: &Path,
    file_name: &str,
    mime: &str,
    size: u64,
) -> Result<String, String> {
    let base = format!("{}://{}:{}", peer.protocol, peer.ip, peer.port);
    let fp = generate_token();
    let prepare_body = serde_json::json!({
        "info": {
            "alias": "Hayato",
            "version": "2.0",
            "deviceModel": "Hayato OS",
            "deviceType": "laptop",
            "fingerprint": fp,
            "port": MULTICAST_PORT,
            "protocol": "http",
            "download": false
        },
        "files": {
            "file1": {
                "id": "file1",
                "fileName": file_name,
                "size": size,
                "fileType": mime
            }
        }
    })
    .to_string();

    let prep_url = format!("{base}/api/localsend/v2/prepare-upload");
    let prep = Command::new("curl")
        .args([
            "-sk",
            "--max-time",
            "25",
            "-w",
            "\n%{http_code}",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &prepare_body,
            &prep_url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;

    let raw = String::from_utf8_lossy(&prep.stdout);
    let (body, code) = raw.rsplit_once('\n').unwrap_or(("", "000"));
    if code.trim() == "403" {
        return Err(format!("{} declined the transfer", peer.alias));
    }
    if code.trim() != "200" {
        return Err(format!(
            "{} did not accept (HTTP {})",
            peer.alias,
            code.trim()
        ));
    }

    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "Invalid LocalSend response".to_string())?;
    let session_id = parsed["sessionId"]
        .as_str()
        .ok_or_else(|| "Missing LocalSend sessionId".to_string())?;
    let file_token = parsed["files"]["file1"]
        .as_str()
        .ok_or_else(|| "Missing LocalSend upload token".to_string())?;

    let upload_url = format!(
        "{base}/api/localsend/v2/upload?sessionId={session_id}&fileId=file1&token={file_token}"
    );
    let up = Command::new("curl")
        .args([
            "-sk",
            "--max-time",
            "120",
            "-w",
            "%{http_code}",
            "-o",
            "/dev/null",
            "-X",
            "POST",
            "--data-binary",
            &format!("@{}", path.display()),
            &upload_url,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Upload failed: {e}"))?;
    let up_code = String::from_utf8_lossy(&up.stdout).trim().to_string();
    if up_code == "200" {
        Ok(format!("Sent {file_name} to {}", peer.alias))
    } else {
        Err(format!("Upload to {} failed (HTTP {up_code})", peer.alias))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_localsend_announcement_and_skips_self() {
        let json = r#"{"alias":"Bruno iPhone","fingerprint":"abc123","port":53317,"protocol":"https","deviceModel":"iPhone"}"#;
        let peer = parse_announcement(json, "192.168.1.50", "other").unwrap();
        assert_eq!(peer.alias, "Bruno iPhone");
        assert_eq!(peer.ip, "192.168.1.50");
        assert_eq!(peer.port, 53317);
        assert_eq!(peer.protocol, "https");
        assert!(parse_announcement(json, "192.168.1.50", "abc123").is_none());
    }
}
