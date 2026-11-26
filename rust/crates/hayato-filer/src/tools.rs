//! Detached launchers integrating `hayato-filer` with Hayato OS tools.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn spawn_detached(program: &str, args: &[String]) {
    let mut cmd = Command::new("setsid");
    cmd.arg(program)
        .args(args)
        .env_remove("HAYATO_UI")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if cmd.spawn().is_err() {
        let _ = Command::new(program)
            .args(args)
            .env_remove("HAYATO_UI")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

pub fn open_file(path: &Path) {
    let p = path.to_string_lossy().into_owned();
    let home_view = crate::state::home_dir().join(".config/bin/hayato-view");
    if home_view.exists() {
        spawn_detached(&home_view.to_string_lossy(), &[p]);
    } else {
        spawn_detached("xdg-open", &[p]);
    }
}

pub fn share_paths(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let mut args = vec!["share".to_string()];
    for p in paths {
        args.push(p.to_string_lossy().into_owned());
    }
    spawn_detached("hayato", &args);
}

pub fn play_in_cassette(path: &Path) {
    spawn_detached(
        "hayato-cassette",
        &["--play".to_string(), path.to_string_lossy().into_owned()],
    );
}

pub fn open_in_lens(path: &Path) {
    spawn_detached(
        "hayato",
        &[
            "menu".to_string(),
            "lens".to_string(),
            "--image".to_string(),
            path.to_string_lossy().into_owned(),
        ],
    );
}

pub fn open_terminal_here(dir: &Path) {
    spawn_detached("foot", &[format!("--working-directory={}", dir.display())]);
}

pub fn search_in_folder(dir: &Path) {
    let home_files = crate::state::home_dir().join(".config/bin/hayato-files");
    let prog = if home_files.exists() {
        home_files.to_string_lossy().into_owned()
    } else {
        "hayato-files".to_string()
    };
    spawn_detached(
        &prog,
        &["--search".to_string(), dir.to_string_lossy().into_owned()],
    );
}

pub fn open_file_actions(path: &Path) {
    spawn_detached(
        "hayato",
        &[
            "menu".to_string(),
            "file-actions".to_string(),
            path.to_string_lossy().into_owned(),
        ],
    );
}

pub fn open_format_drive() {
    let home_fmt = crate::state::home_dir().join(".config/bin/hayato-format");
    if home_fmt.exists() {
        spawn_detached(&home_fmt.to_string_lossy(), &[]);
    } else {
        spawn_detached("hayato", &["menu".to_string(), "format".to_string()]);
    }
}

/// Send a desktop notification through `hayato-notify` (`org.freedesktop.Notifications`).
pub fn notify(msg: &str, tone: &str) {
    let (urgency, timeout) = match tone {
        "danger" => ("critical", "5000"),
        "warn" => ("normal", "4000"),
        _ => ("normal", "3000"),
    };
    let _ = Command::new("notify-send")
        .args([
            "-a",
            "Hayato Files",
            "-i",
            "system-file-manager",
            "-u",
            urgency,
            "-t",
            timeout,
            "-h",
            "string:x-canonical-private-synchronous:hayato-filer",
            "Files",
            msg,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn copy_paths_to_clipboard(paths: &[PathBuf]) {
    let text = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
}

/// Write Wayland clipboard for file Copy/Cut (`x-special/gnome-copied-files` + `text/uri-list`).
pub fn write_file_clipboard(paths: &[PathBuf], is_cut: bool) {
    let uris: Vec<String> = paths
        .iter()
        .map(|p| format!("file://{}", encode_uri_path(&p.to_string_lossy())))
        .collect();
    let action = if is_cut { "cut" } else { "copy" };
    let gnome_payload = format!("{action}\n{}", uris.join("\n"));
    if let Ok(mut child) = Command::new("wl-copy")
        .args(["--type", "x-special/gnome-copied-files"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        let _ = stdin.write_all(gnome_payload.as_bytes());
    }
}

/// Read Wayland clipboard for Paste: returns `(is_cut, paths)`.
pub fn read_file_clipboard() -> Option<(bool, Vec<PathBuf>)> {
    if let Ok(out) = Command::new("wl-paste")
        .args(["--no-newline", "--type", "x-special/gnome-copied-files"])
        .stdin(Stdio::null())
        .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let is_cut = lines.next().is_some_and(|l| l.trim() == "cut");
        let paths: Vec<PathBuf> = lines.filter_map(uri_to_path).collect();
        if !paths.is_empty() {
            return Some((is_cut, paths));
        }
    }
    if let Ok(out) = Command::new("wl-paste")
        .args(["--no-newline", "--type", "text/uri-list"])
        .stdin(Stdio::null())
        .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let paths: Vec<PathBuf> = text.lines().filter_map(uri_to_path).collect();
        if !paths.is_empty() {
            return Some((false, paths));
        }
    }
    None
}

pub fn uri_to_path(line: &str) -> Option<PathBuf> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed.strip_prefix("file://")?;
    Some(PathBuf::from(crate::fs::trash::percent_decode(rest)))
}

fn encode_uri_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
