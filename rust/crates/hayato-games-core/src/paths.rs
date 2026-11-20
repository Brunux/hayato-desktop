//! Where everything lives: the version, the system data, the user's ROMs and
//! the files the library writes.
//!
//! Every path is either an XDG base directory or a documented Hayato one, and
//! every one can be overridden for a checkout (`cargo run`, the tests): the
//! installed binary and the development binary then read the same contract.

use std::path::{Path, PathBuf};

/// The desktop repository's VERSION, the same string `hayato --version` reports.
pub fn version() -> &'static str {
    include_str!("../../../../VERSION").trim()
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn xdg(name: &str, fallback: &str) -> PathBuf {
    env_path(name).unwrap_or_else(|| home().join(fallback))
}

pub fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn state_home() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

pub fn cache_home() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `~/.local/share/hayato/games`: the web games, saves and the WebKit profile.
pub fn data_dir() -> PathBuf {
    data_home().join("hayato/games")
}

/// `~/.local/state/hayato/games`: the library's own small state (recents).
pub fn state_dir() -> PathBuf {
    state_home().join("hayato/games")
}

/// `~/.cache/hayato/games`: WebKit's cache, disposable.
pub fn cache_dir() -> PathBuf {
    cache_home().join("hayato/games")
}

/// `~/.config/hayato/games.css`: the palette as CSS variables, written by
/// `hayato-theme` (surface `games`).
pub fn theme_css() -> PathBuf {
    config_home().join("hayato/games.css")
}

/// The installed data (`/usr/share/hayato/games`), or the crate's `data/` when
/// running from a checkout before anything is installed.
pub fn system_games() -> PathBuf {
    if let Some(path) = env_path("HAYATO_GAMES_DIR") {
        return path;
    }
    let installed = PathBuf::from("/usr/share/hayato/games");
    if installed.is_dir() {
        return installed;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hayato-games/data")
}

/// The folders scanned for ROMs: `~/Games` unless `HAYATO_GAMES_ROM_DIRS`
/// names a colon-separated list (the file manager's "Places" model is a
/// different contract; this is only the scan).
pub fn rom_roots() -> Vec<PathBuf> {
    if let Some(value) = std::env::var_os("HAYATO_GAMES_ROM_DIRS") {
        let roots: Vec<PathBuf> = std::env::split_paths(&value)
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    vec![home().join("Games")]
}

/// `/usr/lib/<multiarch>/libretro`, where the libretro cores land. The Debian
/// packages use the multiarch path; the last candidate is the non-multiarch
/// fallback a hand-built core would use.
pub fn libretro_dir() -> PathBuf {
    if let Some(path) = env_path("HAYATO_LIBRETRO_DIR") {
        return path;
    }
    for candidate in [
        "/usr/lib/aarch64-linux-gnu/libretro",
        "/usr/lib/x86_64-linux-gnu/libretro",
        "/usr/lib/libretro",
    ] {
        if Path::new(candidate).is_dir() {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from("/usr/lib/aarch64-linux-gnu/libretro")
}

/// The kiosk overrides the library passes to RetroArch. It is a package
/// conffile, not a user file: the user's own `retroarch.cfg` stays theirs.
pub fn retroarch_config() -> PathBuf {
    env_path("HAYATO_RETROARCH_CONFIG")
        .unwrap_or_else(|| PathBuf::from("/etc/hayato/retroarch.cfg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_repository_version() {
        let version = version();
        assert!(!version.is_empty(), "VERSION is empty");
        assert_eq!(version.lines().count(), 1, "VERSION must be one line");
        assert!(
            version.split('.').count() == 3,
            "VERSION {version:?} is not x.y.z"
        );
    }

    #[test]
    fn the_system_data_falls_back_to_the_crate_when_nothing_is_installed() {
        // The test process has no HAYATO_GAMES_DIR; if /usr/share/hayato/games
        // is absent (a checkout), the crate's data/ is the answer.
        let path = system_games();
        assert!(
            path.ends_with("hayato/games") || path.ends_with("data"),
            "unexpected system data directory: {}",
            path.display()
        );
    }
}
