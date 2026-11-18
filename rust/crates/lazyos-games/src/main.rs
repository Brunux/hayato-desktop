//! The `lazyos-games` binary.
//!
//! `--library` (or no argument) is the grid; `--play <id>` is one game and then
//! back; `--scan` is the headless JSON that `check-desktop.sh` reads.

mod app;
mod launch;
mod library;
mod paths;
mod platform;
mod scheme;

use std::process::ExitCode;

const HELP: &str = "\
lazyos-games — the LazyOS game library and HTML game host

Usage:
  lazyos-games                 open the library
  lazyos-games --library       open the library (the return from a game)
  lazyos-games --play <id>     run one game, then return to the library
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
        None | Some("--library") => app::run_library(),
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
    let options = library::ScanOptions::from_env();
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
