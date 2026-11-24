//! Hayato Share (`hayato share` / `hayato-share`): OS-wide Share Sheet.
//!
//! Unified entrypoint for sharing files, folders, images, Lens captures,
//! clipboard text, URLs, and notes across mobile (Phone QR + LocalSend v2) and
//! local targets (Clipboard, Notes Inbox, Default App), with zero resident daemons.

pub mod localsend;
pub mod screen;
pub mod server;

use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hayato_ui::Design;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde::{Deserialize, Serialize};

use self::screen::ShareScreen;
use self::server::{default_downloads_dir, generate_token};

const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareSource {
    Files,
    Lens,
    Clipboard,
    Notes,
    Capture,
    Qr,
    Cli,
    Stdin,
}

impl ShareSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Lens => "Lens",
            Self::Clipboard => "Clipboard",
            Self::Notes => "Notes",
            Self::Capture => "Capture",
            Self::Qr => "QR",
            Self::Cli => "CLI",
            Self::Stdin => "Stdin",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharePayload {
    File(PathBuf),
    Files(Vec<PathBuf>),
    Text {
        content: String,
        label: Option<String>,
        source: ShareSource,
    },
    Url {
        url: String,
        title: Option<String>,
        source: ShareSource,
    },
    Image {
        path: PathBuf,
        ephemeral: bool,
        source: ShareSource,
    },
    Receive {
        dest_dir: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharePreset {
    Sheet,
    LanQr,
    DirectQr,
    Receive,
}

impl SharePayload {
    pub fn from_path(path: PathBuf) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if path.is_file() && matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
            Self::Image {
                path,
                ephemeral: false,
                source: ShareSource::Files,
            }
        } else {
            Self::File(path)
        }
    }

    pub fn from_text_or_url(raw: String, label: Option<String>, source: ShareSource) -> Self {
        let trimmed = raw.trim();
        if !trimmed.contains(char::is_whitespace)
            && (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            Self::Url {
                url: trimmed.to_string(),
                title: label,
                source,
            }
        } else {
            Self::Text {
                content: raw,
                label,
                source,
            }
        }
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn write_manifest(payload: &SharePayload) -> Result<PathBuf, String> {
    let dir = runtime_dir();
    let path = dir.join(format!(
        "hayato-share-{}-{}.json",
        std::process::id(),
        &generate_token()[..12]
    ));
    let json = serde_json::to_vec(payload).map_err(|e| format!("serialize payload: {e}"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("create manifest: {e}"))?;
    file.write_all(&json)
        .map_err(|e| format!("write manifest: {e}"))?;
    Ok(path)
}

/// Spawn `hayato share` in a new floating window with a `0600` runtime manifest
/// (avoids leaking clipboard text or secrets into `/proc/<pid>/cmdline`).
pub fn spawn_detached(payload: &SharePayload) -> Result<(), String> {
    let manifest = write_manifest(payload)?;
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/bin/hayato"));
    let manifest_str = manifest.to_string_lossy().into_owned();
    crate::system::spawn_detached_argv(exe, &["share", "--manifest", &manifest_str]);
    Ok(())
}

/// Pre-capture piped stdin (or `--stdin`) before `host::plan` spawns `foot`
/// with `Stdio::null()`. Returns `Some(exit_code)` when handled.
pub fn try_capture_stdin(args: &[String]) -> Option<i32> {
    let want_stdin =
        args.iter().any(|a| a == "--stdin") || (args.is_empty() && !std::io::stdin().is_terminal());
    if !want_stdin {
        return None;
    }
    let mut buf = Vec::new();
    let mut limited = std::io::stdin().take(MAX_STDIN_BYTES as u64);
    if limited.read_to_end(&mut buf).is_err() || buf.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let payload =
        SharePayload::from_text_or_url(text, Some("Piped input".into()), ShareSource::Stdin);
    match spawn_detached(&payload) {
        Ok(()) => Some(0),
        Err(e) => {
            eprintln!("hayato share: {e}");
            Some(3)
        }
    }
}

/// Build a `SharePayload` from the current Wayland clipboard (`wl-paste`).
pub fn clipboard_payload() -> Option<SharePayload> {
    let types = Command::new("wl-paste")
        .arg("--list-types")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    if types.lines().any(|t| t.trim() == "image/png") {
        let out = Command::new("wl-paste")
            .args(["--type", "image/png"])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if out.status.success() && !out.stdout.is_empty() {
            let tmp = runtime_dir().join(format!(
                "hayato-share-clip-{}-{}.png",
                std::process::id(),
                &generate_token()[..8]
            ));
            if std::fs::write(&tmp, &out.stdout).is_ok() {
                return Some(SharePayload::Image {
                    path: tmp,
                    ephemeral: true,
                    source: ShareSource::Clipboard,
                });
            }
        }
    }

    let text = Command::new("wl-paste")
        .arg("--no-newline")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .filter(|s| !s.trim().is_empty())?;
    Some(SharePayload::from_text_or_url(
        text,
        Some("Clipboard".into()),
        ShareSource::Clipboard,
    ))
}

/// Open the Share Sheet in-process (for callers that already own `&mut Terminal`).
pub fn open(
    payload: SharePayload,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    open_with_preset(payload, SharePreset::Sheet, terminal, design)
}

pub fn open_with_preset(
    payload: SharePayload,
    preset: SharePreset,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen =
        ShareScreen::new(payload, preset).map_err(|e| crate::Failure::new("Hayato Share", e))?;
    crate::run_screen(terminal, design, "Hayato Share", &mut screen)
}

fn parse_cli(args: &[String]) -> Result<(SharePayload, SharePreset), String> {
    let mut i = 0;
    let mut files: Vec<PathBuf> = Vec::new();
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| "--manifest requires a file path".to_string())?;
                let raw =
                    std::fs::read(path).map_err(|e| format!("cannot read share manifest: {e}"))?;
                let _ = std::fs::remove_file(path);
                let payload: SharePayload = serde_json::from_slice(&raw)
                    .map_err(|e| format!("invalid share manifest: {e}"))?;
                return Ok((payload, SharePreset::Sheet));
            }
            "--receive" => {
                let dest = args
                    .get(i + 1)
                    .filter(|s| !s.starts_with('-'))
                    .map(PathBuf::from)
                    .unwrap_or_else(default_downloads_dir);
                return Ok((
                    SharePayload::Receive { dest_dir: dest },
                    SharePreset::Receive,
                ));
            }
            "--clipboard" => {
                if let Some(p) = clipboard_payload() {
                    return Ok((p, SharePreset::Sheet));
                }
                return Ok((
                    SharePayload::Receive {
                        dest_dir: default_downloads_dir(),
                    },
                    SharePreset::Receive,
                ));
            }
            "--text" => {
                let text = args
                    .get(i + 1)
                    .ok_or_else(|| "--text requires a value".to_string())?;
                return Ok((
                    SharePayload::Text {
                        content: text.clone(),
                        label: None,
                        source: ShareSource::Cli,
                    },
                    SharePreset::Sheet,
                ));
            }
            "--url" => {
                let url = args
                    .get(i + 1)
                    .ok_or_else(|| "--url requires a value".to_string())?;
                return Ok((
                    SharePayload::Url {
                        url: url.clone(),
                        title: None,
                        source: ShareSource::Cli,
                    },
                    SharePreset::Sheet,
                ));
            }
            "--image" => {
                let p = args
                    .get(i + 1)
                    .ok_or_else(|| "--image requires a path".to_string())?;
                return Ok((
                    SharePayload::Image {
                        path: PathBuf::from(p),
                        ephemeral: false,
                        source: ShareSource::Cli,
                    },
                    SharePreset::Sheet,
                ));
            }
            "--file" => {
                let p = args
                    .get(i + 1)
                    .ok_or_else(|| "--file requires a path".to_string())?;
                files.push(PathBuf::from(p));
                i += 2;
                continue;
            }
            other if !other.starts_with('-') => {
                files.push(PathBuf::from(other));
            }
            _ => {}
        }
        i += 1;
    }

    if files.len() == 1 {
        Ok((SharePayload::from_path(files.remove(0)), SharePreset::Sheet))
    } else if files.len() > 1 {
        Ok((SharePayload::Files(files), SharePreset::Sheet))
    } else if let Some(p) = clipboard_payload() {
        Ok((p, SharePreset::Sheet))
    } else {
        Ok((
            SharePayload::Receive {
                dest_dir: default_downloads_dir(),
            },
            SharePreset::Receive,
        ))
    }
}

/// `hayato share [OPTIONS | PATH...]` (`hayato-share`).
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let (payload, preset) = parse_cli(args).map_err(|e| crate::Failure::new("Hayato Share", e))?;
    open_with_preset(payload, preset, terminal, design)
}

pub fn directory_size_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            total = total.saturating_add(directory_size_bytes(&entry.path()));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_or_url_classifies_http_urls() {
        let p = SharePayload::from_text_or_url(
            "https://hayato.dev/docs".into(),
            None,
            ShareSource::Lens,
        );
        assert!(matches!(p, SharePayload::Url { .. }));

        let t = SharePayload::from_text_or_url(
            "hello world https://example.com".into(),
            None,
            ShareSource::Clipboard,
        );
        assert!(matches!(t, SharePayload::Text { .. }));
    }

    #[test]
    fn parse_cli_handles_text_url_and_receive() {
        let (p, preset) = parse_cli(&["--text".into(), "hi".into()]).unwrap();
        assert!(matches!(p, SharePayload::Text { content, .. } if content == "hi"));
        assert_eq!(preset, SharePreset::Sheet);

        let (p, preset) = parse_cli(&["--receive".into()]).unwrap();
        assert!(matches!(p, SharePayload::Receive { .. }));
        assert_eq!(preset, SharePreset::Receive);
    }
}
