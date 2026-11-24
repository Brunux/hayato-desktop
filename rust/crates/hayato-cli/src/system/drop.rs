//! Hayato Drop (`hayato-drop`, `hayato menu drop [FILE]`): ephemeral encrypted-token
//! LAN nearby share with an in-TUI scannable QR code.
//!
//! Zero resident daemons, zero cloud, zero phone apps required:
//! - Pass a file path to **Send** it to any phone or laptop on the same Wi-Fi.
//! - Run with no arguments to **Receive** files uploaded from a phone browser
//!   straight into `~/Downloads/`.
//! - Closing the window immediately shuts down the ephemeral HTTP listener.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Header, KeyHint, Outcome, QrCode, Screen, TextStyle, draw_badge,
    draw_card, draw_qr,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;

#[derive(Clone, Debug)]
enum DropMode {
    Send {
        path: PathBuf,
        filename: String,
        size: u64,
    },
    Receive {
        dest_dir: PathBuf,
    },
}

#[derive(Default)]
struct SharedState {
    transfers: usize,
    last_event: String,
}

/// Detect the primary LAN IPv4 address without sending any network packets.
pub fn detect_lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
        && let Ok(addr) = sock.local_addr()
    {
        return addr.ip().to_string();
    }
    "127.0.0.1".to_string()
}

/// Generate a 16-hex-character session token from `/dev/urandom` (with a
/// monotonic time + PID fallback).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = File::open("/dev/urandom")
        && f.read_exact(&mut bytes).is_ok()
    {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", nanos ^ (u128::from(std::process::id()) << 32))
}

/// Sanitize an uploaded filename to a safe basename (preventing path traversal).
pub fn sanitize_filename(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload.bin");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        "upload.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn handle_client(
    mut stream: TcpStream,
    token: &str,
    mode: &DropMode,
    shared: &Arc<Mutex<SharedState>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut header_buf = Vec::with_capacity(4096);
    let mut byte = [0u8; 1];
    while header_buf.len() < 16_384 {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                header_buf.push(byte[0]);
                if header_buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let mut lines = header_str.lines();
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let req_path = parts.next().unwrap_or("");

    let mut content_length: usize = 0;
    let mut filename_hdr = String::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("x-filename") {
                filename_hdr = v.trim().to_string();
            }
        }
    }

    let prefix = format!("/{token}");
    if !req_path.starts_with(&prefix) {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\nForbidden");
        return;
    }
    let sub = &req_path[prefix.len()..];

    match (method, sub, mode) {
        ("GET", "" | "/", DropMode::Send { filename, size, .. }) => {
            let html = format!(
                "<!doctype html><html><head><meta name=viewport content='width=device-width,initial-scale=1'>\
                 <title>Hayato Drop</title><style>body{{font-family:system-ui;background:#1a1b26;color:#c0caf5;\
                 display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}}\
                 .card{{background:#24283b;padding:2rem;border-radius:14px;text-align:center;max-width:90vw}}\
                 a{{display:inline-block;margin-top:1rem;padding:.9rem 1.8rem;background:#7aa2f7;color:#1a1b26;\
                 font-weight:700;text-decoration:none;border-radius:10px}}</style></head>\
                 <body><div class=card><h2>Hayato Drop</h2><p>{filename} ({})</p>\
                 <a href='/{token}/download'>Download File</a></div></body></html>",
                human_size(*size)
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _ = stream.write_all(resp.as_bytes());
        }
        (
            "GET",
            "/download",
            DropMode::Send {
                path,
                filename,
                size,
            },
        ) => {
            if let Ok(mut file) = File::open(path) {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Disposition: attachment; filename=\"{filename}\"\r\n\
                     Content-Length: {size}\r\nConnection: close\r\n\r\n"
                );
                if stream.write_all(hdr.as_bytes()).is_ok()
                    && std::io::copy(&mut file, &mut stream).is_ok()
                    && let Ok(mut st) = shared.lock()
                {
                    st.transfers += 1;
                    st.last_event = format!("Sent {filename} ({})", human_size(*size));
                }
            }
        }
        ("GET", "" | "/", DropMode::Receive { .. }) => {
            let html = format!(
                "<!doctype html><html><head><meta name=viewport content='width=device-width,initial-scale=1'>\
                 <title>Hayato Drop</title><style>body{{font-family:system-ui;background:#1a1b26;color:#c0caf5;\
                 display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}}\
                 .card{{background:#24283b;padding:2rem;border-radius:14px;text-align:center;max-width:90vw}}\
                 button{{margin-top:1rem;padding:.9rem 1.8rem;background:#7aa2f7;color:#1a1b26;border:none;\
                 font-weight:700;border-radius:10px;font-size:1rem}}</style></head>\
                 <body><div class=card><h2>Send to Hayato</h2>\
                 <input id=f type=file><br><button onclick='go()'>Upload</button><p id=s></p></div>\
                 <script>async function go(){{const i=document.getElementById('f');if(!i.files.length)return;\
                 const f=i.files[0];document.getElementById('s').textContent='Uploading...';\
                 const r=await fetch('/{token}/upload',{{method:'POST',headers:{{'X-Filename':f.name}},body:f}});\
                 document.getElementById('s').textContent=r.ok?'Sent to ~/Downloads!':'Upload failed';}}</script>\
                 </body></html>"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _ = stream.write_all(resp.as_bytes());
        }
        ("POST", "/upload", DropMode::Receive { dest_dir }) => {
            const MAX_UPLOAD: usize = 512 * 1024 * 1024;
            if content_length == 0 || content_length > MAX_UPLOAD {
                let _ = stream.write_all(
                    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large",
                );
                return;
            }
            let safe_name = sanitize_filename(&filename_hdr);
            let _ = std::fs::create_dir_all(dest_dir);
            let dest = dest_dir.join(&safe_name);
            let mut limited = stream
                .try_clone()
                .ok()
                .map(|s| s.take(content_length as u64));
            if let (Some(mut reader), Ok(mut out)) = (limited.take(), File::create(&dest))
                && std::io::copy(&mut reader, &mut out).is_ok()
            {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
                if let Ok(mut st) = shared.lock() {
                    st.transfers += 1;
                    st.last_event = format!(
                        "Received {safe_name} ({}) → ~/Downloads",
                        human_size(content_length as u64)
                    );
                }
            } else {
                let _ = stream.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
            }
        }
        _ => {
            let _ =
                stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        }
    }
}

pub struct DropScreen {
    mode: DropMode,
    url: String,
    qr: Option<QrCode>,
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<SharedState>>,
    banner: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
}

impl DropScreen {
    fn start(mode: DropMode) -> Result<Self, String> {
        let listener =
            TcpListener::bind("0.0.0.0:0").map_err(|e| format!("Could not bind LAN port: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Could not configure listener: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("Could not read local port: {e}"))?
            .port();
        let ip = detect_lan_ip();
        let token = generate_token();
        let url = format!("http://{ip}:{port}/{token}/");
        let qr = QrCode::encode(&url);
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(SharedState {
            transfers: 0,
            last_event: "Waiting for a device on the same Wi-Fi…".to_string(),
        }));

        let stop_bg = Arc::clone(&stop);
        let shared_bg = Arc::clone(&shared);
        let mode_bg = mode.clone();
        std::thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        handle_client(stream, &token, &mode_bg, &shared_bg);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                    Err(_) => break,
                }
            }
        });

        let subtitle = match &mode {
            DropMode::Send { filename, .. } => format!("Drop · Send {filename}"),
            DropMode::Receive { .. } => "Drop · Receive to ~/Downloads".to_string(),
        };
        Ok(Self {
            mode,
            url,
            qr,
            stop,
            shared,
            banner: None,
            header: Header::new("Hayato")
                .subtitle(subtitle)
                .status("Ephemeral LAN share · closes on Esc"),
            keybar: vec![
                KeyHint::new("c", "copy URL"),
                KeyHint::new("Esc", "stop & close"),
            ],
        })
    }
}

impl Drop for DropScreen {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Screen for DropScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((820, 560))
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("Hayato Drop"), true);
        if inner.width < 20 || inner.height < 6 {
            return;
        }
        let (badge, role, summary) = match &self.mode {
            DropMode::Send { filename, size, .. } => (
                "SEND",
                ColorRole::Accent,
                format!("{filename} ({})", human_size(*size)),
            ),
            DropMode::Receive { .. } => (
                "RECEIVE",
                ColorRole::Ok,
                "Scan with phone camera to upload to ~/Downloads".to_string(),
            ),
        };
        let x = draw_badge(ctx.buf, inner.x, inner.y, ctx.design, badge, role);
        let _ = put_trunc(
            ctx.buf,
            x + 2,
            inner.y,
            &summary,
            inner.right().saturating_sub(x + 2),
            ctx.design.style(TextStyle::Title),
        );
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            inner.y + 1,
            &self.url,
            inner.width,
            ctx.design.style(TextStyle::Code),
        );

        let (transfers, last_event) = self
            .shared
            .lock()
            .map(|s| (s.transfers, s.last_event.clone()))
            .unwrap_or_default();
        let status_line = self.banner.as_deref().unwrap_or(&last_event);
        let footer_y = inner.bottom().saturating_sub(1);
        let status_text = if transfers > 0 {
            format!("[{transfers} done] {status_line}")
        } else {
            status_line.to_string()
        };
        let _ = put(
            ctx.buf,
            inner.x,
            footer_y,
            &status_text,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );

        if let Some(qr) = &self.qr {
            let qr_area = Rect::new(
                inner.x,
                inner.y + 3,
                inner.width,
                footer_y.saturating_sub(inner.y + 4),
            );
            if qr_area.height >= 4 {
                draw_qr(ctx.buf, qr_area, ctx.design, qr);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Char('c') => {
                let ok = Command::new("wl-copy")
                    .arg(&self.url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                self.banner = Some(if ok {
                    "Copied Drop URL to clipboard".to_string()
                } else {
                    "Could not copy URL (wl-copy failed)".to_string()
                });
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

/// `hayato menu drop [FILE]` (`hayato-drop`): Send a file or Receive uploads over LAN.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mode = if let Some(arg) = args.first() {
        let path = PathBuf::from(arg);
        let meta = std::fs::metadata(&path).map_err(|e| {
            crate::Failure::new(
                "Hayato Drop",
                format!("Cannot read {}: {e}", path.display()),
            )
        })?;
        if !meta.is_file() {
            return Err(crate::Failure::new(
                "Hayato Drop",
                format!("{} is not a regular file", path.display()),
            ));
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        DropMode::Send {
            path,
            filename,
            size: meta.len(),
        }
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        DropMode::Receive {
            dest_dir: PathBuf::from(home).join("Downloads"),
        }
    };

    let mut screen = DropScreen::start(mode).map_err(|e| crate::Failure::new("Hayato Drop", e))?;
    crate::run_screen(terminal, design, "Hayato Drop", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_blocks_path_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("photo 01.jpg"), "photo 01.jpg");
        assert_eq!(sanitize_filename("///"), "upload.bin");
    }

    #[test]
    fn generate_token_is_16_hex_chars() {
        let tok = generate_token();
        assert_eq!(tok.len(), 16);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
