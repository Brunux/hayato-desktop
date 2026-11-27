//! **Hayato Connect** (`hayato connect` / `hayato-connect`): opt-in,
//! zero-daemon encrypted phone-to-computer workspace extending Hayato Drop.
//!
//! Runs strictly while the Connect window is open (`Drop` tears down the TLS/WSS
//! server, discovery scanner, and clipboard watcher cleanly).

pub mod alerts;
pub mod camera;
pub mod crypto;
pub mod kdeconnect;
pub mod media;
pub mod screen;
pub mod server;
pub mod sync;
pub mod trust;

use std::path::PathBuf;

use hayato_ui::Design;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::screen::ConnectRequest;
pub use self::screen::ConnectScreen;
use self::trust::TrustStore;
use crate::Failure;

/// Handle non-interactive CLI query subcommands before host `foot` spawning.
pub fn try_query(args: &[String]) -> Option<i32> {
    if args.iter().any(|a| a == "--bar" || a == "bar") {
        println!("{}", bar_json());
        return Some(0);
    }
    if args.iter().any(|a| a == "--stop" || a == "stop") {
        return Some(stop_active_session());
    }
    if args.iter().any(|a| a == "--status" || a == "status") {
        let store = TrustStore::load_default();
        println!(
            "paired={} store={}",
            store.devices().len(),
            store.path().display()
        );
        return Some(0);
    }
    if args.iter().any(|a| a == "--devices" || a == "devices") {
        let store = TrustStore::load_default();
        for d in store.devices() {
            println!("{}\t{}\t{}", d.id, d.name, d.user_agent);
        }
        return Some(0);
    }
    None
}

pub fn signal_bar() {
    let _ = crate::system::out(&["pkill", "-RTMIN+8", "waybar"]);
}

pub fn runtime_session_path() -> Option<PathBuf> {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|base| PathBuf::from(base).join("hayato-connect/session.json"))
}

#[derive(serde::Deserialize)]
struct RuntimeSession {
    ip: String,
    port: u16,
    pid: u32,
    #[serde(default)]
    kde_port: u16,
}

fn live_session() -> Option<(PathBuf, RuntimeSession)> {
    let path = runtime_session_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let sess: RuntimeSession = serde_json::from_str(&raw).ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{}/comm", sess.pid)).ok()?;
    if !comm.trim().contains("hayato") {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some((path, sess))
}

pub fn bar_json() -> String {
    let Some((_, sess)) = live_session() else {
        return r#"{"text":"","class":"empty","tooltip":""}"#.to_string();
    };
    let paired = TrustStore::load_default().devices().len();
    let badge = if paired > 0 {
        format!("<span font_desc='JetBrains Mono 13'>\u{f10b}</span> {paired}")
    } else {
        "<span font_desc='JetBrains Mono 13'>\u{f10b}</span>".to_string()
    };
    let kde_line = if sess.kde_port > 0 {
        format!(
            "\n<span alpha='45%'>│</span> <span alpha='60%'>KDE </span>  UDP/TLS {}",
            sess.kde_port
        )
    } else {
        String::new()
    };
    let tooltip = format!(
        "<span alpha='50%'>╭─</span> <b>CONNECT // LIVE</b>\n\
         <span alpha='45%'>│</span> <span alpha='60%'>URL </span>  https://{}:{}\
         {kde_line}\n\
         <span alpha='45%'>│</span> <span alpha='60%'>DEV </span>  {paired} paired\n\
         <span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] open · [RMB] close</span>",
        sess.ip, sess.port
    );
    serde_json::json!({
        "text": badge,
        "class": "active",
        "tooltip": tooltip,
    })
    .to_string()
}

pub fn stop_active_session() -> i32 {
    if let Some((path, sess)) = live_session() {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &sess.pid.to_string()])
            .status();
        let _ = std::fs::remove_file(path);
    }
    signal_bar();
    0
}

/// Entrypoint for `hayato connect` (`hayato-connect`).
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    if let Some(code) = try_query(args) {
        return Ok(code);
    }

    let mut screen = ConnectScreen::new().map_err(|e| Failure::new("Hayato Connect", e))?;
    if let Some(first) = args.iter().find(|a| !a.starts_with('-')) {
        let p = PathBuf::from(first);
        if p.exists() {
            screen.send_path(p);
        }
    }

    loop {
        let code = crate::run_screen(terminal, design, "Hayato Connect", &mut screen)?;
        match screen.pending_request.take() {
            Some(ConnectRequest::PickFileToSend) => {
                if let Some(chosen) = pick_path_to_send(terminal, design, false)? {
                    screen.send_path(chosen);
                }
            }
            Some(ConnectRequest::PickSyncFolder) => {
                if let Some(chosen) = pick_path_to_send(terminal, design, true)? {
                    screen.set_sync_folder(chosen);
                }
            }
            None => return Ok(code),
        }
    }
}

fn pick_path_to_send(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    folder_only: bool,
) -> Result<Option<PathBuf>, Failure> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let state_dir = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/state"))
        .join("hayato");
    let setup = crate::files::browse::Setup {
        dir: home.clone(),
        mode: crate::files::browse::Mode::Browse,
        hidden: false,
        home: home.clone(),
        memory: state_dir.join("connect-browse-last"),
        path_var: std::env::var("PATH").unwrap_or_default(),
    };
    let previewer = crate::files::preview::Previewer::for_session(&home);
    let mut screen = crate::files::browse::FilesScreen::new(setup, Some(previewer));
    let code = crate::run_screen(terminal, design, "Send to Phone", &mut screen)?;
    if code != 0 {
        return Ok(None);
    }
    match screen.chosen {
        Some(crate::files::Chosen::Open(path)) | Some(crate::files::Chosen::Actions(path)) => {
            if folder_only {
                let dir = if path.is_dir() {
                    path
                } else {
                    path.parent().unwrap_or(&home).to_path_buf()
                };
                Ok(Some(dir))
            } else {
                Ok(Some(path))
            }
        }
        _ => {
            if folder_only {
                Ok(Some(screen.current_dir().to_path_buf()))
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_json_collapses_when_no_session_running() {
        let out = bar_json();
        assert!(out.contains("text"));
    }
}
