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

/// Start the `--play` process detached from the library's process group, so
/// quitting the library does not take the game with it.
pub fn spawn_play(id: &str) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.arg("--play").arg(id);
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

/// Hand the session back to the library. This is the other half of the
/// `--play` process: it exits, and the library starts fresh with the state the
/// page saved (filter, position, recents).
pub fn return_to_library() -> ! {
    if std::env::var_os("LAZYOS_GAMES_NO_RETURN").is_some() {
        std::process::exit(0);
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lazyos-games"));
    use std::os::unix::process::CommandExt;
    let error = Command::new(exe).arg("--library").exec();
    eprintln!("lazyos-games: cannot return to the library: {error}");
    std::process::exit(1);
}

/// Open the folder a game came from in Files, where the person can decide what
/// to do with the file. The library never opens a ROM itself.
pub fn reveal(game: &Game) -> io::Result<()> {
    let folder = match (game.kind, game.rom.as_deref(), game.root.as_deref()) {
        (GameKind::Retro, Some(rom), _) => rom.parent().map(Path::to_path_buf),
        (GameKind::Web, _, Some(root)) => Some(root.to_path_buf()),
        _ => None,
    };
    let Some(folder) = folder else {
        return Ok(());
    };
    Command::new("lazyos-files").arg(folder).spawn()?;
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
