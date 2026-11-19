//! Starting something, and coming back.
//!
//! A launch is an argv array, never a shell line: the core comes from the
//! platform table and the ROM from the library's own scan. When the game exits,
//! the process `exec`s the library again, so the library's WebView is not left
//! running behind an emulator on a 4 GB machine.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::library::{Game, GameKind};
use crate::paths;

pub fn retroarch_bin() -> PathBuf {
    std::env::var_os("LAZYOS_RETROARCH")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("retroarch"))
}

/// The exact command line for one retro game. `--appendconfig` is the kiosk
/// overlay the package ships; the user's `~/.config/retroarch/retroarch.cfg`
/// is still read first, so their own settings survive.
pub fn retro_argv(game: &Game, libretro_dir: &Path) -> Vec<String> {
    let core = game.core.as_deref().unwrap_or_default();
    let rom = game
        .rom
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut args = vec!["--fullscreen".to_string()];
    let config = paths::retroarch_config();
    if config.is_file() {
        args.push("--appendconfig".into());
        args.push(config.display().to_string());
    }
    args.push("-L".into());
    args.push(
        libretro_dir
            .join(format!("{core}_libretro.so"))
            .display()
            .to_string(),
    );
    args.push(rom);
    args
}

pub fn run_retro(game: &Game, libretro_dir: &Path) -> io::Result<ExitStatus> {
    Command::new(retroarch_bin())
        .args(retro_argv(game, libretro_dir))
        .status()
}

/// The GTK host that plays one game. The TUI library and the desktop entries
/// start it by name; `LAZYOS_GAMES_HOST` is for a checkout.
pub fn host_bin() -> PathBuf {
    std::env::var_os("LAZYOS_GAMES_HOST")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("lazyos-games"))
}

/// Start the `--play` process detached from the library's process group, so
/// quitting the library does not take the game with it.
pub fn spawn_play(id: &str) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let mut command = Command::new(host_bin());
    command.arg("--play").arg(id);
    // The TUI's foot PTY is not the host's terminal: if the host inherited it,
    // returning would draw a second TUI into the old window. A host with no
    // terminal always comes back through `foot` (below).
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.process_group(0);
    command.spawn()?;
    Ok(())
}

/// The library page's URL for a web game. The host is the manifest slug.
pub fn web_uri(game: &Game) -> Option<String> {
    if game.kind != GameKind::Web {
        return None;
    }
    let host = game.id.strip_prefix("web:")?;
    let entry = game.entry.as_deref().unwrap_or("index.html");
    Some(format!("lazyos-game://{host}/{entry}"))
}

/// Hand the session back to the TUI library in a new `foot` window, with the
/// saved tab and position.
///
/// The host has no terminal of its own (it starts with null stdio), so running
/// the TUI here would leave it nowhere to draw. `LAZYOS_UI=1` tells the child
/// it *is* the UI, so it draws in this one window instead of asking the host
/// for a second one.
pub fn return_to_library() -> ! {
    if std::env::var_os("LAZYOS_GAMES_NO_RETURN").is_some() {
        std::process::exit(0);
    }
    use std::process::Stdio;
    let spawned = Command::new("foot")
        .args(["--app-id=lazyos-pick", "-e", "lazyos", "menu", "games"])
        .env("LAZYOS_UI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("lazyos-games: cannot return to the library: {error}");
            std::process::exit(1);
        }
    }
}

/// Open the folder a game came from in Files, where the person can decide what
/// to do with the file. The library never opens a ROM itself.
/// Open the folder a game came from in Files, in a window of its own: the
/// library keeps the shelf, Files gets the place where the person decides what
/// to do with the file. `LAZYOS_UI=1` keeps the child from opening a third
/// window of its own.
pub fn reveal(game: &Game) -> io::Result<()> {
    use std::process::Stdio;
    let folder = match (game.kind, game.rom.as_deref(), game.root.as_deref()) {
        (GameKind::Retro, Some(rom), _) => rom.parent().map(Path::to_path_buf),
        (GameKind::Web, _, Some(root)) => Some(root.to_path_buf()),
        _ => None,
    };
    let Some(folder) = folder else {
        return Ok(());
    };
    Command::new("foot")
        .args(["--app-id=lazyos-pick", "-e", "lazyos", "menu", "files"])
        .arg(folder)
        .env("LAZYOS_UI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::GameKind;

    fn game() -> Game {
        Game {
            id: "retro:snes:0000000000000000".into(),
            kind: GameKind::Retro,
            title: "Game".into(),
            platform: "snes".into(),
            platform_title: "Super Nintendo".into(),
            core: Some("snes9x".into()),
            rom: Some(PathBuf::from("/home/lazy/Games/snes/Game.sfc")),
            root: None,
            entry: None,
            art: None,
            description: None,
            author: None,
            save: false,
            favorite: false,
            last: 0,
            size: 0,
        }
    }

    #[test]
    fn a_launch_is_an_argv_array_not_a_shell_line() {
        let args = retro_argv(&game(), Path::new("/usr/lib/libretro"));
        assert_eq!(args[0], "--fullscreen");
        let core = args.iter().position(|arg| arg == "-L").expect("no -L");
        assert_eq!(args[core + 1], "/usr/lib/libretro/snes9x_libretro.so");
        assert_eq!(args.last().unwrap(), "/home/lazy/Games/snes/Game.sfc");
        assert!(!args.iter().any(|arg| arg == "-c" || arg == "sh"));
    }

    #[test]
    fn a_zipped_game_is_the_archive_path_itself() {
        // RetroArch opens the archive and plays the first ROM inside it, so the
        // argv carries the `.zip` and nothing else: no extraction of ours, and
        // no `#entry` syntax to depend on.
        let mut zipped = game();
        zipped.platform = "nes".into();
        zipped.core = Some("nestopia".into());
        zipped.rom = Some(PathBuf::from("/home/lazy/Games/nes/Mega Man 2.zip"));
        let args = retro_argv(&zipped, Path::new("/usr/lib/libretro"));
        assert_eq!(args.last().unwrap(), "/home/lazy/Games/nes/Mega Man 2.zip");
        assert!(!args.iter().any(|arg| arg.contains('#')));
        let core = args.iter().position(|arg| arg == "-L").expect("no -L");
        assert_eq!(args[core + 1], "/usr/lib/libretro/nestopia_libretro.so");
    }

    #[test]
    fn a_web_game_url_is_its_manifest_slug_and_entry() {
        let mut web = game();
        web.id = "web:snake".into();
        web.kind = GameKind::Web;
        web.entry = Some("index.html".into());
        web.rom = None;
        assert_eq!(
            web_uri(&web).as_deref(),
            Some("lazyos-game://snake/index.html")
        );
        assert_eq!(web_uri(&game()), None);
    }
}
