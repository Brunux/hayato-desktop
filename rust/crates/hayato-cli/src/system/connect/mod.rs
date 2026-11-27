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
