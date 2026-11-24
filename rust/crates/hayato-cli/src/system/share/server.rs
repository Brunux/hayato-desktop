//! Ephemeral LAN HTTP server backing both **Hayato Share** and **Hayato Drop**.
//!
//! Zero resident daemons, zero new Cargo crates:
//! - Multi-threaded `std::net::TcpListener` bound to an ephemeral port.
//! - 128-bit CSPRNG token from `/dev/urandom` checked with constant-time equality.
//! - Active `hayato-theme` CSS variables injected into responsive mobile web cards.
//! - Supports single files, folders / multi-file ZIP bundles (via `zip`), inline
//!   images, text snippets (with HTTP-safe 1-tap copy), URLs, and two-way
//!   phone→Hayato upload (`/upload`) + clipboard push (`/clip`).
//! - 5-minute auto-expire TTL (extendable in-TUI) and automatic cleanup on `Drop`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::SharePayload;

pub const DEFAULT_TTL: Duration = Duration::from_secs(300);
const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CLIP_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct TransferTelemetry {
    pub seq: u64,
    pub transfers: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub active_label: String,
    pub last_event: String,
    pub expires_at: Instant,
    pub expired: bool,
}

impl Default for TransferTelemetry {
    fn default() -> Self {
        Self {
            seq: 0,
            transfers: 0,
            bytes_done: 0,
            bytes_total: 0,
            active_label: String::new(),
            last_event: "Waiting for a device on the same Wi-Fi\u{2026}".to_string(),
            expires_at: Instant::now() + DEFAULT_TTL,
            expired: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ServeKind {
    File {
        path: PathBuf,
        filename: String,
        size: u64,
        mime: &'static str,
        is_image: bool,
        ephemeral: bool,
    },
    Text {
        content: String,
        label: String,
    },
    Url {
        url: String,
        title: String,
    },
    Receive {
        dest_dir: PathBuf,
    },
}

pub struct ShareServer {
    pub url: String,
    pub ip: String,
    pub port: u16,
    pub token: String,
    pub kind: ServeKind,
    stop: Arc<AtomicBool>,
    pub telemetry: Arc<Mutex<TransferTelemetry>>,
    ephemeral_path: Option<PathBuf>,
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(p) = self.ephemeral_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl ShareServer {
    pub fn start(payload: &SharePayload, receive_dir: PathBuf) -> Result<Self, String> {
        let (kind, ephemeral_path) = prepare_serve_kind(payload, receive_dir)?;
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

        let stop = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(Mutex::new(TransferTelemetry::default()));
        let theme_css = active_theme_css();

        let stop_bg = Arc::clone(&stop);
        let tel_bg = Arc::clone(&telemetry);
        let kind_bg = kind.clone();
        let token_bg = token.clone();
        let recv_dir = default_downloads_dir();

        std::thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                let is_expired = tel_bg
                    .lock()
                    .map(|mut t| {
                        if Instant::now() >= t.expires_at {
                            if !t.expired {
                                t.expired = true;
                                t.seq += 1;
                                t.last_event =
                                    "Session expired (press r to extend +5 min)".to_string();
                            }
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if is_expired {
                    std::thread::sleep(Duration::from_millis(120));
                    continue;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let tok = token_bg.clone();
                        let k = kind_bg.clone();
                        let tel = Arc::clone(&tel_bg);
                        let css = theme_css.clone();
                        let rdir = recv_dir.clone();
                        std::thread::spawn(move || {
                            handle_client(stream, &tok, &k, &rdir, &css, &tel);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(60));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            url,
            ip,
            port,
            token,
            kind,
            stop,
            telemetry,
            ephemeral_path,
        })
    }

    pub fn extend_ttl(&self, extra: Duration) {
        if let Ok(mut t) = self.telemetry.lock() {
            let base = t.expires_at.max(Instant::now());
            t.expires_at = base + extra;
            t.expired = false;
            t.seq += 1;
            t.last_event = "Extended session by +5 minutes".to_string();
        }
    }

    pub fn snapshot(&self) -> TransferTelemetry {
        self.telemetry
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }
}

pub fn default_downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join("Downloads")
    })
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

/// Generate a 32-hex-character (128-bit) session token from `/dev/urandom`.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = File::open("/dev/urandom")
        && f.read_exact(&mut bytes).is_ok()
    {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:016x}{:016x}",
        nanos,
        nanos.rotate_left(17) ^ (u128::from(std::process::id()) << 32)
    )
}

/// Constant-time byte comparison to prevent token timing side channels.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

/// HTML-escape untrusted strings before interpolating into served HTML.
pub fn html_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn human_size(bytes: u64) -> String {
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

/// Atomically create a non-colliding file in `dest_dir`, appending `-1`, `-2`, …
/// if `safe_name` already exists.
pub fn create_unique_file(dest_dir: &Path, safe_name: &str) -> std::io::Result<(PathBuf, File)> {
    let _ = std::fs::create_dir_all(dest_dir);
    let candidate = dest_dir.join(safe_name);
    if let Ok(f) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)
    {
        return Ok((candidate, f));
    }
    let stem = Path::new(safe_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(safe_name);
    let ext = Path::new(safe_name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 1..=9999 {
        let path = dest_dir.join(format!("{stem}-{i}{ext}"));
        if let Ok(f) = OpenOptions::new().write(true).create_new(true).open(&path) {
            return Ok((path, f));
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many colliding filenames",
    ))
}

fn mime_for_path(path: &Path) -> (&'static str, bool) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => ("image/png", true),
        "jpg" | "jpeg" => ("image/jpeg", true),
        "webp" => ("image/webp", true),
        "gif" => ("image/gif", true),
        "svg" => ("image/svg+xml", true),
        "zip" => ("application/zip", false),
        "pdf" => ("application/pdf", false),
        "mp4" => ("video/mp4", false),
        "mp3" => ("audio/mpeg", false),
        "txt" | "md" | "json" | "toml" | "rs" | "sh" => ("text/plain; charset=utf-8", false),
        _ => ("application/octet-stream", false),
    }
}

fn create_zip_bundle(paths: &[PathBuf]) -> Result<(PathBuf, String, u64), String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let base_name = if paths.len() == 1 {
        paths[0]
            .file_name()
            .and_then(|n| n.to_str())
            .map(sanitize_filename)
            .unwrap_or_else(|| "bundle".to_string())
    } else {
        format!("hayato-share-{}items", paths.len())
    };
    let zip_name = format!("{base_name}.zip");
    let zip_path = runtime.join(format!(
        "hayato-share-{}-{}.zip",
        std::process::id(),
        &generate_token()[..8]
    ));
    let _ = std::fs::remove_file(&zip_path);

    let mut cmd = Command::new("zip");
    cmd.arg("-r").arg("-q").arg(&zip_path);
    for p in paths {
        cmd.arg(p);
    }
    let status = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("zip failed to start: {e}"))?;
    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "zip failed to archive selection".to_string()
        } else {
            format!("zip failed: {err}")
        });
    }
    let size = std::fs::metadata(&zip_path).map(|m| m.len()).unwrap_or(0);
    Ok((zip_path, zip_name, size))
}

fn prepare_serve_kind(
    payload: &SharePayload,
    receive_dir: PathBuf,
) -> Result<(ServeKind, Option<PathBuf>), String> {
    match payload {
        SharePayload::File(path) => {
            let meta = std::fs::metadata(path)
                .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
            if meta.is_dir() {
                let (zip_path, zip_name, size) = create_zip_bundle(std::slice::from_ref(path))?;
                Ok((
                    ServeKind::File {
                        path: zip_path.clone(),
                        filename: zip_name,
                        size,
                        mime: "application/zip",
                        is_image: false,
                        ephemeral: true,
                    },
                    Some(zip_path),
                ))
            } else {
                let filename = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string();
                let (mime, is_image) = mime_for_path(path);
                Ok((
                    ServeKind::File {
                        path: path.clone(),
                        filename,
                        size: meta.len(),
                        mime,
                        is_image,
                        ephemeral: false,
                    },
                    None,
                ))
            }
        }
        SharePayload::Files(paths) => {
            if paths.is_empty() {
                return Err("No files selected to share".to_string());
            }
            if paths.len() == 1 && paths[0].is_file() {
                return prepare_serve_kind(&SharePayload::File(paths[0].clone()), receive_dir);
            }
            let (zip_path, zip_name, size) = create_zip_bundle(paths)?;
            Ok((
                ServeKind::File {
                    path: zip_path.clone(),
                    filename: zip_name,
                    size,
                    mime: "application/zip",
                    is_image: false,
                    ephemeral: true,
                },
                Some(zip_path),
            ))
        }
        SharePayload::Image {
            path, ephemeral, ..
        } => {
            let meta = std::fs::metadata(path)
                .map_err(|e| format!("Cannot read image {}: {e}", path.display()))?;
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("image.png")
                .to_string();
            let (mime, _) = mime_for_path(path);
            Ok((
                ServeKind::File {
                    path: path.clone(),
                    filename,
                    size: meta.len(),
                    mime,
                    is_image: true,
                    ephemeral: *ephemeral,
                },
                ephemeral.then(|| path.clone()),
            ))
        }
        SharePayload::Text { content, label, .. } => Ok((
            ServeKind::Text {
                content: content.clone(),
                label: label
                    .clone()
                    .unwrap_or_else(|| "Shared Text".to_string()),
            },
            None,
        )),
        SharePayload::Url { url, title, .. } => Ok((
            ServeKind::Url {
                url: url.clone(),
                title: title.clone().unwrap_or_else(|| url.clone()),
            },
            None,
        )),
        SharePayload::Receive { dest_dir } => Ok((
            ServeKind::Receive {
                dest_dir: if dest_dir.as_os_str().is_empty() {
                    receive_dir
                } else {
                    dest_dir.clone()
                },
            },
            None,
        )),
    }
}

#[derive(Clone)]
struct ThemeCss {
    bg: String,
    surface: String,
    border: String,
    text: String,
    muted: String,
    accent: String,
    ok: String,
}

fn active_theme_css() -> ThemeCss {
    let paths = hayato_theme::store::Paths::from_env();
    let slug = hayato_theme::store::active(&paths).unwrap_or_else(|| "tokyo-night".to_string());
    crate::theme::theme(&slug)
        .or_else(|_| crate::theme::theme("tokyo-night"))
        .ok()
        .map(|t| {
            let r = t.roles();
            ThemeCss {
                bg: t.colors.background.hashed(),
                surface: r.surface.hashed(),
                border: r.surface_alt.hashed(),
                text: r.text.hashed(),
                muted: r.dim.hashed(),
                accent: r.accent.hashed(),
                ok: r.ok.hashed(),
            }
        })
        .unwrap_or(ThemeCss {
            bg: "#1a1b26".to_string(),
            surface: "#24283b".to_string(),
            border: "#3b4261".to_string(),
            text: "#c0caf5".to_string(),
            muted: "#565f89".to_string(),
            accent: "#7aa2f7".to_string(),
            ok: "#9ece6a".to_string(),
        })
}

fn security_headers(content_type: &str, content_len: usize) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {content_len}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self' 'unsafe-inline'; img-src 'self' data:\r\n\
         Connection: close\r\n\r\n"
    )
}

fn render_mobile_page(token: &str, kind: &ServeKind, css: &ThemeCss) -> String {
    let open_recv = matches!(kind, ServeKind::Receive { .. });
    let primary = match kind {
        ServeKind::File {
            filename,
            size,
            is_image,
            ..
        } => {
            let safe_name = html_escape(filename);
            let sz = human_size(*size);
            let preview = if *is_image {
                format!(
                    "<img class='preview' src='/{token}/raw' alt='{safe_name}'><p class='hint'>Long-press image to Save to Photos</p>"
                )
            } else {
                String::new()
            };
            format!(
                "<div class='badge'>FILE · {sz}</div>\
                 <h2>{safe_name}</h2>{preview}\
                 <div class='actions'>\
                   <a class='btn primary' href='/{token}/download' download='{safe_name}'>Download</a>\
                   <button id='webshare' class='btn ghost' style='display:none' onclick='shareFile()'>Share via Phone\u{2026}</button>\
                 </div>\
                 <script>if(navigator.share){{document.getElementById('webshare').style.display='inline-block';}}\
                 async function shareFile(){{try{{const r=await fetch('/{token}/download');const b=await r.blob();\
                 const f=new File([b],{fname_js},{{type:b.type}});await navigator.share({{files:[f],title:{fname_js}}});}}catch(_e){{}}}}</script>",
                fname_js = serde_json::to_string(filename).unwrap_or_else(|_| "\"file\"".to_string())
            )
        }
        ServeKind::Text { content, label } => {
            let safe_label = html_escape(label);
            let safe_body = html_escape(content);
            let sz = human_size(content.len() as u64);
            format!(
                "<div class='badge'>TEXT · {sz}</div>\
                 <h2>{safe_label}</h2>\
                 <textarea id='txt' readonly>{safe_body}</textarea>\
                 <div class='actions'>\
                   <button class='btn primary' onclick='copyTxt()'>Copy Text</button>\
                   <a class='btn ghost' href='/{token}/download' download='hayato-share.txt'>Download .txt</a>\
                   <button id='webshare' class='btn ghost' style='display:none' onclick='shareTxt()'>Share\u{2026}</button>\
                 </div><p id='cp' class='hint'></p>\
                 <script>if(navigator.share){{document.getElementById('webshare').style.display='inline-block';}}\
                 function copyTxt(){{const el=document.getElementById('txt');el.select();el.setSelectionRange(0,999999);\
                 if(navigator.clipboard&&navigator.clipboard.writeText){{navigator.clipboard.writeText(el.value).then(()=>{{document.getElementById('cp').textContent='Copied!';}}).catch(()=>{{document.execCommand('copy');document.getElementById('cp').textContent='Copied!';}});}}\
                 else{{document.execCommand('copy');document.getElementById('cp').textContent='Copied!';}}}}\
                 async function shareTxt(){{try{{await navigator.share({{text:document.getElementById('txt').value}});}}catch(_e){{}}}}</script>"
            )
        }
        ServeKind::Url { url, title } => {
            let safe_title = html_escape(title);
            let safe_url = html_escape(url);
            format!(
                "<div class='badge'>LINK</div>\
                 <h2>{safe_title}</h2>\
                 <p class='url'>{safe_url}</p>\
                 <textarea id='txt' style='position:absolute;left:-9999px' readonly>{safe_url}</textarea>\
                 <div class='actions'>\
                   <a class='btn primary' href='{safe_url}' rel='noopener noreferrer'>Open Link</a>\
                   <button class='btn ghost' onclick='copyUrl()'>Copy Link</button>\
                   <button id='webshare' class='btn ghost' style='display:none' onclick='shareUrl()'>Share\u{2026}</button>\
                 </div><p id='cp' class='hint'></p>\
                 <script>if(navigator.share){{document.getElementById('webshare').style.display='inline-block';}}\
                 function copyUrl(){{const el=document.getElementById('txt');el.select();if(navigator.clipboard){{navigator.clipboard.writeText(el.value).catch(()=>document.execCommand('copy'));}}else{{document.execCommand('copy');}}document.getElementById('cp').textContent='Link copied!';}}\
                 async function shareUrl(){{try{{await navigator.share({{url:document.getElementById('txt').value}});}}catch(_e){{}}}}</script>"
            )
        }
        ServeKind::Receive { .. } => {
            "<div class='badge'>RECEIVE</div><h2>Send to Hayato</h2>\
             <p class='hint'>Upload files to ~/Downloads or push text straight to Hayato's clipboard.</p>"
                .to_string()
        }
    };

    let details_open = if open_recv { " open" } else { "" };
    let recv_drawer = format!(
        "<details class='drawer'{details_open}><summary>Send back to Hayato</summary>\
           <div class='recv-box'>\
             <input id='uf' type='file' multiple>\
             <button class='btn primary' onclick='upFiles()'>Upload to ~/Downloads</button>\
             <progress id='up' value='0' max='100' style='display:none'></progress>\
             <p id='us' class='hint'></p>\
             <hr>\
             <textarea id='ct' rows='3' placeholder='Paste text or URL to send to Hayato clipboard\u{2026}'></textarea>\
             <button class='btn ghost' onclick='sendClip()'>Send to Clipboard</button>\
             <p id='cs' class='hint'></p>\
           </div>\
         </details>\
         <script>\
         function upFiles(){{const inp=document.getElementById('uf');if(!inp.files.length)return;\
         const bar=document.getElementById('up');const st=document.getElementById('us');bar.style.display='block';\
         let idx=0;function next(){{if(idx>=inp.files.length){{st.textContent='Sent '+inp.files.length+' file(s) to ~/Downloads!';return;}}\
         const f=inp.files[idx++];st.textContent='Uploading '+f.name+'...';\
         const xhr=new XMLHttpRequest();xhr.open('POST','/{token}/upload');xhr.setRequestHeader('X-Filename',f.name);\
         xhr.upload.onprogress=e=>{{if(e.lengthComputable)bar.value=Math.round((e.loaded/e.total)*100);}};\
         xhr.onload=()=>{{if(xhr.status===200)next();else st.textContent='Upload failed ('+xhr.status+')';}};\
         xhr.onerror=()=>{{st.textContent='Network error';}};xhr.send(f);}}next();}}\
         async function sendClip(){{const t=document.getElementById('ct').value;if(!t)return;\
         const r=await fetch('/{token}/clip',{{method:'POST',body:t}});\
         document.getElementById('cs').textContent=r.ok?'Copied to Hayato clipboard!':'Failed';}}\
         </script>"
    );

    format!(
        "<!doctype html><html><head><meta charset='utf-8'>\
         <meta name='viewport' content='width=device-width,initial-scale=1'>\
         <title>Hayato Share</title><style>\
         :root{{--bg:{bg};--surface:{surface};--border:{border};--text:{text};--muted:{muted};--accent:{accent};--ok:{ok}}}\
         *{{box-sizing:border-box}}body{{font-family:system-ui,-apple-system,sans-serif;background:var(--bg);color:var(--text);\
         margin:0;padding:1.2rem;min-height:100vh;display:flex;align-items:center;justify-content:center}}\
         .card{{background:var(--surface);border:1px solid var(--border);border-left:4px solid var(--accent);\
         border-radius:14px;padding:1.5rem;width:100%;max-width:460px;box-shadow:0 12px 32px rgba(0,0,0,.35)}}\
         .badge{{display:inline-block;font:700 .75rem monospace;letter-spacing:.08em;padding:.25rem .6rem;\
         border-radius:6px;background:var(--accent);color:var(--bg);margin-bottom:.6rem}}\
         h2{{margin:.2rem 0 .8rem;font-size:1.25rem;word-break:break-word}}\
         .url{{font-family:monospace;font-size:.85rem;color:var(--accent);word-break:break-all;background:var(--bg);\
         padding:.6rem;border-radius:8px;border:1px solid var(--border)}}\
         .preview{{width:100%;max-height:52vh;object-fit:contain;border-radius:10px;border:1px solid var(--border);margin:.4rem 0}}\
         textarea{{width:100%;min-height:110px;background:var(--bg);color:var(--text);border:1px solid var(--border);\
         border-radius:8px;padding:.7rem;font-family:monospace;font-size:.85rem;resize:vertical}}\
         .actions{{display:flex;flex-wrap:wrap;gap:.6rem;margin-top:.9rem}}\
         .btn{{display:inline-block;padding:.75rem 1.2rem;border-radius:10px;font-weight:700;font-size:.95rem;\
         text-decoration:none;cursor:pointer;border:1px solid transparent;text-align:center}}\
         .primary{{background:var(--accent);color:var(--bg)}}.ghost{{background:var(--bg);color:var(--text);border-color:var(--border)}}\
         .hint{{color:var(--muted);font-size:.85rem;margin:.5rem 0 0}}hr{{border:0;border-top:1px solid var(--border);margin:1rem 0}}\
         .drawer{{margin-top:1.1rem;border-top:1px solid var(--border);padding-top:.8rem}}\
         summary{{cursor:pointer;font-weight:600;color:var(--accent)}}.recv-box{{margin-top:.8rem;display:flex;flex-direction:column;gap:.6rem}}\
         progress{{width:100%;height:8px;accent-color:var(--ok)}}\
         </style></head><body><div class='card'>{primary}{recv_drawer}</div></body></html>",
        bg = css.bg,
        surface = css.surface,
        border = css.border,
        text = css.text,
        muted = css.muted,
        accent = css.accent,
        ok = css.ok
    )
}

fn handle_client(
    mut stream: TcpStream,
    token: &str,
    kind: &ServeKind,
    default_recv_dir: &Path,
    css: &ThemeCss,
    telemetry: &Arc<Mutex<TransferTelemetry>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
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

    let mut content_length: u64 = 0;
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
    let req_bytes = req_path.as_bytes();
    let pref_bytes = prefix.as_bytes();
    if req_bytes.len() < pref_bytes.len()
        || !constant_time_eq(&req_bytes[..pref_bytes.len()], pref_bytes)
    {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\nForbidden");
        return;
    }
    let sub = &req_path[prefix.len()..];

    match (method, sub) {
        ("GET", "" | "/") => {
            let html = render_mobile_page(token, kind, css);
            let hdr = security_headers("text/html; charset=utf-8", html.len());
            let _ = stream.write_all(hdr.as_bytes());
            let _ = stream.write_all(html.as_bytes());
        }
        ("GET", "/download" | "/raw") => match kind {
            ServeKind::File {
                path,
                filename,
                size,
                mime,
                ..
            } => {
                if let Ok(mut file) = File::open(path) {
                    let disp = if sub == "/raw" {
                        "inline".to_string()
                    } else {
                        format!(
                            "attachment; filename=\"{}\"",
                            sanitize_filename(filename)
                        )
                    };
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n\
                         Content-Disposition: {disp}\r\n\
                         Content-Length: {size}\r\n\
                         Cache-Control: no-store\r\n\
                         X-Content-Type-Options: nosniff\r\n\
                         Connection: close\r\n\r\n"
                    );
                    if stream.write_all(hdr.as_bytes()).is_ok() {
                        if let Ok(mut t) = telemetry.lock() {
                            t.bytes_done = 0;
                            t.bytes_total = *size;
                            t.active_label = format!("Sending {filename}");
                            t.seq += 1;
                        }
                        let mut buf = [0u8; 32_768];
                        let mut sent: u64 = 0;
                        let mut ok = true;
                        loop {
                            match file.read(&mut buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    if stream.write_all(&buf[..n]).is_err() {
                                        ok = false;
                                        break;
                                    }
                                    sent += n as u64;
                                    if let Ok(mut t) = telemetry.lock() {
                                        t.bytes_done = sent;
                                        t.seq += 1;
                                    }
                                }
                                Err(_) => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok && let Ok(mut t) = telemetry.lock() {
                            t.transfers += 1;
                            t.bytes_done = *size;
                            t.active_label.clear();
                            t.last_event = format!("Sent {filename} ({})", human_size(*size));
                            t.seq += 1;
                        }
                    }
                }
            }
            ServeKind::Text { content, .. } => {
                let bytes = content.as_bytes();
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
                     Content-Disposition: attachment; filename=\"hayato-share.txt\"\r\n\
                     Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                if stream.write_all(hdr.as_bytes()).is_ok()
                    && stream.write_all(bytes).is_ok()
                    && let Ok(mut t) = telemetry.lock()
                {
                    t.transfers += 1;
                    t.last_event = format!("Sent text ({})", human_size(bytes.len() as u64));
                    t.seq += 1;
                }
            }
            _ => {
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
            }
        },
        ("POST", "/upload") => {
            if content_length == 0 || content_length > MAX_UPLOAD_BYTES {
                let _ = stream.write_all(
                    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large",
                );
                return;
            }
            let dest_dir = match kind {
                ServeKind::Receive { dest_dir } => dest_dir.as_path(),
                _ => default_recv_dir,
            };
            let safe_name = sanitize_filename(&filename_hdr);
            if let Ok((dest_path, mut out_file)) = create_unique_file(dest_dir, &safe_name) {
                let saved_name = dest_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&safe_name)
                    .to_string();
                if let Ok(mut t) = telemetry.lock() {
                    t.bytes_done = 0;
                    t.bytes_total = content_length;
                    t.active_label = format!("Receiving {saved_name}");
                    t.seq += 1;
                }
                let mut remaining = content_length;
                let mut buf = [0u8; 32_768];
                let mut got: u64 = 0;
                let mut ok = true;
                while remaining > 0 {
                    let want = (remaining as usize).min(buf.len());
                    match stream.read(&mut buf[..want]) {
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
                            if let Ok(mut t) = telemetry.lock() {
                                t.bytes_done = got;
                                t.seq += 1;
                            }
                        }
                    }
                }
                if ok {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
                    if let Ok(mut t) = telemetry.lock() {
                        t.transfers += 1;
                        t.active_label.clear();
                        t.last_event = format!(
                            "Received {saved_name} ({}) \u{2192} {}",
                            human_size(content_length),
                            dest_dir.display()
                        );
                        t.seq += 1;
                    }
                } else {
                    let _ = std::fs::remove_file(&dest_path);
                    let _ =
                        stream.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
                }
            } else {
                let _ = stream.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
            }
        }
        ("POST", "/clip") => {
            if content_length == 0 || content_length > MAX_CLIP_BYTES {
                let _ = stream.write_all(
                    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 9\r\n\r\nToo large",
                );
                return;
            }
            let mut body = vec![0u8; content_length as usize];
            if stream.read_exact(&mut body).is_ok() {
                let text = String::from_utf8_lossy(&body).to_string();
                let copied = Command::new("wl-copy")
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
                    .unwrap_or(false);
                if copied {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK");
                    if let Ok(mut t) = telemetry.lock() {
                        t.transfers += 1;
                        t.last_event = format!(
                            "Received snippet ({}) \u{2192} clipboard",
                            human_size(content_length)
                        );
                        t.seq += 1;
                    }
                } else {
                    let _ =
                        stream.write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 5\r\n\r\nError");
                }
            }
        }
        _ => {
            let _ =
                stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        }
    }
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
    fn generate_token_is_32_hex_chars_and_constant_time_eq_works() {
        let tok = generate_token();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(constant_time_eq(tok.as_bytes(), tok.as_bytes()));
        assert!(!constant_time_eq(b"abcdef", b"abcdeg"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn html_escape_neutralizes_xss_payloads() {
        assert_eq!(
            html_escape("<script>alert('x\"&')</script>"),
            "&lt;script&gt;alert(&#39;x&quot;&amp;&#39;)&lt;/script&gt;"
        );
    }

    #[test]
    fn create_unique_file_never_overwrites_existing() {
        let dir = std::env::temp_dir().join(format!("hayato-share-uniq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (p1, _) = create_unique_file(&dir, "note.txt").unwrap();
        let (p2, _) = create_unique_file(&dir, "note.txt").unwrap();
        assert_eq!(p1.file_name().unwrap(), "note.txt");
        assert_eq!(p2.file_name().unwrap(), "note-1.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
