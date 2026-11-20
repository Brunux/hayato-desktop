//! The `hayato-games` host.
//!
//! `--play <id>` runs one game: RetroArch for a retro id (no GTK), or the
//! WebKit window for a web id, then `hayato menu games` again. `--scan` is the
//! headless JSON `check-desktop.sh` reads. The library itself is the Ratatui
//! screen in `hayato-cli`; this binary has no library mode.

mod app;
mod scheme;

use std::process::ExitCode;

use hayato_games_core::library::{self, ScanOptions};
use hayato_games_core::paths;

const HELP: &str = "\
hayato-games — the GTK + WebKit host for one Hayato HTML game

Usage:
  hayato-games --play <id>     run one game, then return to the TUI library
  hayato-games --scan          print the library as JSON, no window
  hayato-games --ensure-layout create ~/Games (one folder per platform, bios/ and the README)
  hayato-games --version       print the desktop version
  hayato-games --help          this text

Environment:
  HAYATO_GAMES_DIR         system data (default /usr/share/hayato/games)
  HAYATO_GAMES_ROM_DIRS    colon-separated scan roots (default ~/Games)
  HAYATO_LIBRETRO_DIR      cores (default /usr/lib/<multiarch>/libretro)
  HAYATO_RETROARCH         the emulator binary (default retroarch)
  HAYATO_RETROARCH_CONFIG  kiosk overrides (default /etc/hayato/retroarch.cfg)
  HAYATO_GAMES_WINDOWED    start windowed instead of fullscreen
  HAYATO_GAMES_NO_ACCEL    disable WebKit hardware acceleration
  HAYATO_GAMES_NO_RETURN   do not reopen the library after a game
  HAYATO_GAMES_DEBUG       developer extras and console messages
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {
            println!("{HELP}");
            ExitCode::from(2)
        }
        Some("--play") => match args.get(1) {
            Some(id) => app::run_play(id),
            None => {
                eprintln!("hayato-games: --play needs a game id (see --scan)");
                ExitCode::from(2)
            }
        },
        Some(argument) if argument.starts_with("--play=") => {
            app::run_play(argument.trim_start_matches("--play="))
        }
        Some("--scan") => scan(),
        Some("--ensure-layout") => ensure_layout(),
        Some("--version") | Some("-V") => {
            println!("{}", paths::version());
            ExitCode::SUCCESS
        }
        Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("hayato-games: unknown option {other} (try --help)");
            ExitCode::from(2)
        }
    }
}

fn scan() -> ExitCode {
    let options = ScanOptions::from_env();
    let state = library::load_state(&options.state_path());
    let library = library::scan(&options, &state);
    match serde_json::to_string_pretty(&library.public()) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hayato-games: cannot serialize the library: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Create the shelf without opening the window: one folder per platform,
/// `bios/` and the README, all generated from `platform::PLATFORMS`. The image
/// build runs this so `~/Games` exists on a fresh host; opening the library
/// runs the same function, and both are idempotent.
fn ensure_layout() -> ExitCode {
    let options = ScanOptions::from_env();
    match library::ensure_layout(&options) {
        Ok(()) => {
            if let Some(root) = options.roots.first() {
                println!("{}", root.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hayato-games: cannot create the game folders: {error}");
            ExitCode::FAILURE
        }
    }
}
