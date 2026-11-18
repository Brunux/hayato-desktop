//! The `lazyos-games` host.
//!
//! `--play <id>` runs one game: RetroArch for a retro id (no GTK), or the
//! WebKit window for a web id, then `lazyos menu games` again. `--scan` is the
//! headless JSON `check-desktop.sh` reads. The library itself is the Ratatui
//! screen in `lazyos-cli`; this binary has no library mode.

mod app;
mod scheme;

use std::process::ExitCode;

use lazyos_games_core::library::{self, ScanOptions};
use lazyos_games_core::paths;

const HELP: &str = "\
lazyos-games — the GTK + WebKit host for one LazyOS HTML game

Usage:
  lazyos-games --play <id>     run one game, then return to the TUI library
  lazyos-games --scan          print the library as JSON, no window
  lazyos-games --version       print the desktop version
  lazyos-games --help          this text

Environment:
  LAZYOS_GAMES_DIR         system data (default /usr/share/lazyos/games)
  LAZYOS_GAMES_ROM_DIRS    colon-separated scan roots (default ~/Games)
  LAZYOS_LIBRETRO_DIR      cores (default /usr/lib/<multiarch>/libretro)
  LAZYOS_RETROARCH         the emulator binary (default retroarch)
  LAZYOS_RETROARCH_CONFIG  kiosk overrides (default /etc/lazyos/retroarch.cfg)
  LAZYOS_GAMES_WINDOWED    start windowed instead of fullscreen
  LAZYOS_GAMES_NO_ACCEL    disable WebKit hardware acceleration
  LAZYOS_GAMES_NO_RETURN   do not reopen the library after a game
  LAZYOS_GAMES_DEBUG       developer extras and console messages
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
                eprintln!("lazyos-games: --play needs a game id (see --scan)");
                ExitCode::from(2)
            }
        },
        Some(argument) if argument.starts_with("--play=") => {
            app::run_play(argument.trim_start_matches("--play="))
        }
        Some("--scan") => scan(),
        Some("--version") | Some("-V") => {
            println!("{}", paths::version());
            ExitCode::SUCCESS
        }
        Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("lazyos-games: unknown option {other} (try --help)");
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
            eprintln!("lazyos-games: cannot serialize the library: {error}");
            ExitCode::FAILURE
        }
    }
}
