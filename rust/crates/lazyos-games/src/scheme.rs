//! The `lazyos-game://` scheme: the only way a game page reaches a file.
//!
//! Each game is served from `lazyos-game://<id>/`, so it has a real origin (its
//! own `localStorage`/`IndexedDB`), the web process never needs filesystem
//! access, and a path from a manifest is resolved against its game directory
//! before it is read. A request for `../../.ssh/id_ed25519` is a 404.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use gtk4::gio;
use gtk4::glib;
use webkit6::{URISchemeRequest, WebContext};

use lazyos_games_core::library::Library;

pub const SCHEME: &str = "lazyos-game";

pub struct Handler {
    system_dir: PathBuf,
    config_css: PathBuf,
    library: Arc<Mutex<Library>>,
}

impl Handler {
    pub fn new(library: Library, system_dir: PathBuf, config_css: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            system_dir,
            config_css,
            library: Arc::new(Mutex::new(library)),
        })
    }

    pub fn library(&self) -> std::sync::MutexGuard<'_, Library> {
        self.library
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn install(self: &Arc<Self>, context: &WebContext) {
        if let Some(manager) = context.security_manager() {
            manager.register_uri_scheme_as_secure(SCHEME);
            manager.register_uri_scheme_as_local(SCHEME);
            manager.register_uri_scheme_as_cors_enabled(SCHEME);
        }
        let this = Arc::clone(self);
        context.register_uri_scheme(SCHEME, move |request| this.handle(request));
    }

    fn handle(&self, request: &URISchemeRequest) {
        let Some(uri) = request.uri() else {
            return self.not_found(request);
        };
        let Some((host, path)) = parse_uri(&uri) else {
            return self.not_found(request);
        };
        let relative = path.trim_start_matches('/');

        let game = {
            let library = self.library();
            library
                .find(&format!("web:{host}"))
                .and_then(|game| game.root.clone().zip(game.entry.clone()))
        };
        let Some((root, entry)) = game else {
            return self.not_found(request);
        };
        if relative == "theme.css" {
            return self.serve_theme(request);
        }
        let relative = if relative.is_empty() {
            entry
        } else {
            relative.to_string()
        };
        match resolve_under(&root, &relative) {
            Some(path) => self.serve(request, &path),
            None => self.not_found(request),
        }
    }

    fn serve_theme(&self, request: &URISchemeRequest) {
        let path = if self.config_css.is_file() {
            self.config_css.clone()
        } else {
            self.system_dir.join("theme-default.css")
        };
        self.serve(request, &path);
    }

    fn serve(&self, request: &URISchemeRequest, path: &Path) {
        let Ok(data) = fs::read(path) else {
            return self.not_found(request);
        };
        self.serve_bytes(request, data, mime_for(path));
    }

    fn serve_bytes(&self, request: &URISchemeRequest, data: Vec<u8>, mime: &str) {
        let length = data.len() as i64;
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(data));
        let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
        headers.append("Access-Control-Allow-Origin", "*");
        headers.append("Cache-Control", "no-store");
        let response = webkit6::URISchemeResponse::new(&stream, length);
        response.set_content_type(mime);
        response.set_http_headers(headers);
        request.finish_with_response(&response);
    }

    fn not_found(&self, request: &URISchemeRequest) {
        self.serve_bytes(request, b"not found\n".to_vec(), "text/plain");
    }
}

/// Split `lazyos-game://host/path` into a host and a decoded path.
pub fn parse_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("lazyos-game://")?;
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains(':') {
        return None;
    }
    Some((percent_decode(host)?, percent_decode(path)?))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Resolve a relative path under `root`, or nothing. `canonicalize` is what
/// makes a symlink that points outside the root a 404 rather than a leak.
pub fn resolve_under(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = relative.trim_start_matches('/');
    if relative.is_empty() || relative.contains('\0') || relative.contains('\\') {
        return None;
    }
    let parsed = Path::new(relative);
    if !parsed
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let candidate = root.join(parsed).canonicalize().ok()?;
    if candidate.starts_with(&root) && candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

pub fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_splits_into_a_host_and_a_decoded_path() {
        assert_eq!(
            parse_uri("lazyos-game://snake/index.html"),
            Some(("snake".into(), "/index.html".into()))
        );
        assert_eq!(
            parse_uri("lazyos-game://snake/My%20Game/index.html"),
            Some(("snake".into(), "/My Game/index.html".into()))
        );
        assert_eq!(parse_uri("https://example.com/"), None);
        assert_eq!(parse_uri("lazyos-game:///index.html"), None);
        assert_eq!(parse_uri("lazyos-game://host/%zz"), None);
    }

    #[test]
    fn a_path_cannot_leave_its_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(root.join("levels")).unwrap();
        fs::write(root.join("index.html"), b"ok").unwrap();
        fs::write(root.join("levels/one.json"), b"{}").unwrap();
        fs::write(temp.path().join("secret"), b"no").unwrap();

        assert!(resolve_under(&root, "index.html").is_some());
        assert!(resolve_under(&root, "levels/one.json").is_some());
        assert!(resolve_under(&root, "../secret").is_none());
        assert!(resolve_under(&root, "/etc/passwd").is_none());
        assert!(resolve_under(&root, "levels/../secret").is_none());
        assert!(resolve_under(&root, "").is_none());
        assert!(resolve_under(&root, "missing.html").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_not_served() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        fs::write(temp.path().join("secret"), b"no").unwrap();
        std::os::unix::fs::symlink(temp.path().join("secret"), root.join("link")).unwrap();
        assert!(resolve_under(&root, "link").is_none());
    }

    #[test]
    fn mime_types_are_chosen_by_extension() {
        assert_eq!(mime_for(Path::new("a.html")), "text/html; charset=utf-8");
        assert_eq!(
            mime_for(Path::new("a.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_for(Path::new("a.wasm")), "application/wasm");
        assert_eq!(mime_for(Path::new("a.unknown")), "application/octet-stream");
    }
}
