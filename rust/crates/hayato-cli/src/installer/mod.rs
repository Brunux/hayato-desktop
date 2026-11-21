//! The full TUI & headless pure-Rust installer (spec 018 & spec 023).

pub mod boot;
pub mod config;
pub mod deviceinfo;
pub mod disk;
pub mod identity;
pub mod preflight;
pub mod ui;
pub mod worker;

use std::io::{self, Read};

use hayato_ui::app::App;
use hayato_ui::design::Design;
use hayato_ui::widgets::Header;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::Failure;
use config::{ApplyRequest, InstallConfig, InstallMode};
use preflight::Preflight;
use ui::InstallerScreen;

/// Run the interactive TUI installer wizard.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
    dry_run: bool,
) -> Result<i32, Failure> {
    let preflight = Preflight::from_machine();
    let mut screen = InstallerScreen::new(preflight, dry_run);

    let mut app = App::new(Header::new("Hayato").subtitle("System Installer"))
        .design(design.clone())
        .screen(&mut screen);

    app.run(terminal)
        .map_err(|err| Failure::new("Installer terminal session failed", err.to_string()))
}

/// Headless `--check` flag: evaluate hardware safety invariants, target discovery, and tool prerequisites.
pub fn check() -> i32 {
    let preflight = Preflight::from_machine();
    let (ok, summary) = preflight.check_summary();
    println!("{summary}");
    if ok {
        hayato_ui::app::exit::OK
    } else {
        hayato_ui::app::exit::ESCAPE
    }
}

/// Headless `apply` command:
///   `hayato installer apply --stdin [--state-dir DIR]`
///   `hayato installer apply --clone --confirm [--target /dev/...] [--state-dir DIR]`
///   `hayato installer apply [config.json]`
pub fn apply(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "usage: hayato installer apply --stdin [--state-dir DIR]\n\
             \x20      hayato installer apply --clone --confirm [--target DEV] [--state-dir DIR]\n\
             \x20      hayato installer apply <config.json>"
        );
        return hayato_ui::app::exit::OK;
    }

    let state_dir = arg_value(args, "--state-dir");

    if args.iter().any(|a| a == "--stdin") {
        let mut raw = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut raw) {
            eprintln!("installer apply: failed reading stdin payload: {e}");
            return hayato_ui::app::exit::FAILED;
        }
        let Ok(req) = serde_json::from_str::<ApplyRequest>(&raw) else {
            eprintln!("installer apply: malformed JSON ApplyRequest on stdin");
            return hayato_ui::app::exit::USAGE;
        };
        let mut config = req.config;
        if config.state_dir.is_none() {
            config.state_dir = state_dir;
        }
        return finish_apply(worker::run_worker_with_secret(
            config,
            req.secret_password.as_deref(),
            None,
        ));
    }

    if args.iter().any(|a| a == "--clone") {
        let confirmed = args.iter().any(|a| a == "--confirm")
            || std::env::var("CONFIRM").ok().as_deref() == Some("yes");
        if !confirmed {
            eprintln!("refusing to write target disk without --confirm or CONFIRM=yes");
            return hayato_ui::app::exit::USAGE;
        }
        let preflight = Preflight::from_machine();
        let target_disk = arg_value(args, "--target")
            .or_else(|| std::env::var("DEVICE_BLOCK").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| preflight.target_disk.clone());

        if let Err(reason) = preflight::check_safety(&preflight.root_source, &target_disk) {
            eprintln!("installer safety check failed: {reason}");
            return hayato_ui::app::exit::FAILED;
        }

        let config = InstallConfig {
            mode: InstallMode::Clone,
            target_disk,
            dry_run: args.iter().any(|a| a == "--dry-run"),
            state_dir,
            ..Default::default()
        };
        return finish_apply(worker::run_worker_with_secret(config, None, None));
    }

    let mut config = if let Some(path) = args.first().filter(|a| !a.starts_with('-')) {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("hayato-installer: cannot read config {path}: {e}");
                return hayato_ui::app::exit::FAILED;
            }
        };
        match serde_json::from_str(&contents) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("hayato-installer: invalid config JSON in {path}: {e}");
                return hayato_ui::app::exit::FAILED;
            }
        }
    } else {
        InstallConfig::default()
    };
    if config.state_dir.is_none() {
        config.state_dir = state_dir;
    }

    finish_apply(worker::run_worker(config, None))
}

fn finish_apply(res: io::Result<()>) -> i32 {
    match res {
        Ok(()) => {
            if let Some(st) = worker::load_state() {
                if st.status == worker::WorkerPhase::Failed {
                    if let Some(err) = st.error {
                        eprintln!("installer worker failed: {err}");
                    }
                    return hayato_ui::app::exit::FAILED;
                }
            }
            hayato_ui::app::exit::OK
        }
        Err(err) => {
            eprintln!("installer worker failed: {err}");
            hayato_ui::app::exit::FAILED
        }
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}
