//! The library: what is on disk, and the small state the library remembers.
//!
//! Two kinds of game live here. A **retro** game is a ROM file found under one
//! of the scan roots; its id is a hash of the path and its platform table row
//! names the core. A **web** game is a directory with a `game.toml` and an
//! `index.html`; its id is the manifest's slug. The public JSON the library
//! page sees carries no filesystem paths at all - the page asks for an id, and
//! the process that can actually launch something resolves it again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameKind {
    Retro,
    Web,
}

/// One game as the library holds it. `rom` and `root` never leave this process.
#[derive(Clone, Debug)]
pub struct Game {
    pub id: String,
    pub kind: GameKind,
    pub title: String,
    pub platform: String,
    pub platform_title: String,
    pub core: Option<String>,
    pub rom: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub entry: Option<String>,
    pub art: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub save: bool,
    pub favorite: bool,
    pub last: i64,
}

/// What one scan needs. Every directory is passed in so the tests can build a
/// small tree in a temp dir instead of touching the real home.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    pub system_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub libretro_dir: PathBuf,
}

impl ScanOptions {
    pub fn from_env() -> Self {
        Self {
            roots: paths::rom_roots(),
            system_dir: paths::system_games(),
            data_dir: paths::data_dir(),
            state_dir: paths::state_dir(),
            libretro_dir: paths::libretro_dir(),
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
}

/// What the library remembers between runs. It is state, not configuration:
/// deleting it only loses the Recent list and the favourites.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub favorites: BTreeSet<String>,
    /// Id -> Unix seconds of the last launch.
    #[serde(default)]
    pub last: BTreeMap<String, i64>,
    /// The last rail filter and grid position, restored on return from a game.
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub index: usize,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn load_state(path: &Path) -> State {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_state(path: &Path, state: &State) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    fs::rename(temporary, path)
}

#[derive(Clone, Debug)]
pub struct Library {
    pub roots: Vec<PathBuf>,
    pub games: Vec<Game>,
    pub retroarch: bool,
    pub libretro_dir: PathBuf,
}

impl Library {
    pub fn find(&self, id: &str) -> Option<&Game> {
        self.games.iter().find(|game| game.id == id)
    }

    pub fn mark_played(&mut self, id: &str, at: i64) {
        if let Some(game) = self.games.iter_mut().find(|game| game.id == id) {
            game.last = at;
        }
    }

    pub fn set_favorite(&mut self, id: &str, on: bool) {
        if let Some(game) = self.games.iter_mut().find(|game| game.id == id) {
            game.favorite = on;
        }
    }

    pub fn core_present(&self, core: &str) -> bool {
        self.libretro_dir
            .join(format!("{core}_libretro.so"))
            .is_file()
    }

    /// The JSON the library page reads. No path, no command line, no core
    /// filename: an id, what to draw, and whether the machine can run it.
    pub fn public(&self) -> PublicLibrary {
        let mut platforms = Vec::new();
        platforms.push(PublicPlatform {
            id: "all".into(),
            title: "All games".into(),
            count: self.games.len(),
        });
        let recent = self.games.iter().filter(|game| game.last > 0).count();
        platforms.push(PublicPlatform {
            id: "recent".into(),
            title: "Recent".into(),
            count: recent,
        });
        platforms.push(PublicPlatform {
            id: "favorites".into(),
            title: "Favourites".into(),
            count: self.games.iter().filter(|game| game.favorite).count(),
        });
        for platform in platform::PLATFORMS {
            let count = self
                .games
                .iter()
                .filter(|game| game.platform == platform.id)
                .count();
            if count > 0 {
                platforms.push(PublicPlatform {
                    id: platform.id.into(),
                    title: platform.title.into(),
                    count,
                });
            }
        }
        let web = self
            .games
            .iter()
            .filter(|game| game.kind == GameKind::Web)
            .count();
        if web > 0 {
            platforms.push(PublicPlatform {
                id: "web".into(),
                title: "LazyOS games".into(),
                count: web,
            });
        }

        PublicLibrary {
            version: 1,
            roots: self
                .roots
                .iter()
                .map(|root| root.display().to_string())
                .collect(),
            retroarch: self.retroarch,
            platforms,
            games: self
                .games
                .iter()
                .map(|game| PublicGame {
                    id: game.id.clone(),
                    kind: game.kind,
                    title: game.title.clone(),
                    platform: game.platform.clone(),
                    platform_title: game.platform_title.clone(),
                    core: game.core.clone(),
                    core_present: game.kind == GameKind::Web
                        || game
                            .core
                            .as_deref()
                            .map(|core| self.core_present(core))
                            .unwrap_or(false),
                    art: game.art.as_ref().and_then(|art| {
                        game.id.strip_prefix("web:").map(|host| {
                            format!("lazyos-game://{host}/{}", art.trim_start_matches('/'))
                        })
                    }),
                    save: game.save,
                    favorite: game.favorite,
                    last: game.last,
                    description: game.description.clone(),
                    author: game.author.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct PublicLibrary {
    pub version: u32,
    pub roots: Vec<String>,
    pub retroarch: bool,
    pub platforms: Vec<PublicPlatform>,
    pub games: Vec<PublicGame>,
}

#[derive(Serialize)]
pub struct PublicPlatform {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct PublicGame {
    pub id: String,
    pub kind: GameKind,
    pub title: String,
    pub platform: String,
    pub platform_title: String,
    pub core: Option<String>,
    pub core_present: bool,
    pub art: Option<String>,
    pub save: bool,
    pub favorite: bool,
    pub last: i64,
    pub description: Option<String>,
    pub author: Option<String>,
}

pub fn scan(options: &ScanOptions, state: &State) -> Library {
    let mut games = Vec::new();
    let mut seen = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in &options.roots {
        walk_roms(root, options, state, &mut games, &mut seen, &mut visited);
    }
    scan_web(&options.system_dir.join("web"), state, &mut games);
    scan_web(&options.data_dir.join("web"), state, &mut games);
    games.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Library {
        roots: options.roots.clone(),
        games,
        retroarch: which("retroarch"),
        libretro_dir: options.libretro_dir.clone(),
    }
}

fn walk_roms(
    dir: &Path,
    options: &ScanOptions,
    state: &State,
    games: &mut Vec<Game>,
    seen: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) {
    // A symlink to a parent would otherwise recurse forever; the canonical
    // directory is the identity, not the path that reached it.
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical_dir) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "art" | "bios" | "covers" | "saves" | "states" | "system" | "web"
            ) {
                continue;
            }
            walk_roms(&path, options, state, games, seen, visited);
            continue;
        }
        // `detect`, not `by_extension`: a `.zip` is a Neo Geo romset only
        // inside `neogeo/`, `mvs/`, `aes/` or `fbneo/`.
        let Some(platform) = platform::detect(&path) else {
            continue;
        };
        // `.md` is a Mega Drive extension and a Markdown one; a README is not a
        // ROM. The same floor keeps a `.cue` descriptor (which is allowed to be
        // tiny) on a platform whose floor is zero.
        if path
            .metadata()
            .map(|metadata| metadata.len() < platform.min_bytes)
            .unwrap_or(false)
        {
            continue;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let id = retro_id(platform.id, &canonical);
        games.push(Game {
            title: platform::pretty_title(stem),
            id: id.clone(),
            kind: GameKind::Retro,
            platform: platform.id.into(),
            platform_title: platform.title.into(),
            core: Some(platform.core.into()),
            save: has_save(options, platform.core, stem),
            favorite: state.favorites.contains(&id),
            last: state.last.get(&id).copied().unwrap_or(0),
            rom: Some(canonical),
            root: None,
            entry: None,
            art: None,
            description: None,
            author: None,
        });
    }
}

/// A save is a file the emulator wrote next to the content's name: `.srm` for
/// battery-backed RAM, `.state*` for a save state. Both are looked for in the
/// flat and per-core layouts RetroArch uses.
fn has_save(options: &ScanOptions, core: &str, stem: &str) -> bool {
    for directory in [
        options.data_dir.join("saves").join(core),
        options.data_dir.join("saves"),
        options.state_dir.join("states").join(core),
        options.state_dir.join("states"),
    ] {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(stem) && (name.ends_with(".srm") || name.contains(".state")) {
                return true;
            }
        }
    }
    false
}

#[derive(Deserialize)]
struct WebManifest {
    id: String,
    title: String,
    entry: String,
    #[serde(default)]
    art: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

fn scan_web(dir: &Path, state: &State, games: &mut Vec<Game>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(root.join("game.toml")) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<WebManifest>(&text) else {
            continue;
        };
        let Some(slug) = valid_slug(&manifest.id) else {
            continue;
        };
        let Some(entry) = safe_relative(&manifest.entry) else {
            continue;
        };
        if !root.join(&entry).is_file() {
            continue;
        }
        let art = manifest
            .art
            .as_deref()
            .and_then(safe_relative)
            .filter(|art| root.join(art).is_file());
        let root = root.canonicalize().unwrap_or(root);
        let id = format!("web:{slug}");
        games.retain(|game| game.id != id);
        games.push(Game {
            id: id.clone(),
            kind: GameKind::Web,
            title: manifest.title,
            platform: "web".into(),
            platform_title: "LazyOS".into(),
            core: None,
            rom: None,
            root: Some(root),
            entry: Some(entry),
            art,
            description: manifest.description,
            author: manifest.author,
            save: false,
            favorite: state.favorites.contains(&id),
            last: state.last.get(&id).copied().unwrap_or(0),
        });
    }
}

/// A manifest id is a slug because it becomes the custom-scheme host
/// (`lazyos-game://<id>/…`), and a host cannot hold a slash or a dot.
fn valid_slug(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.len() > 64 {
        return None;
    }
    id.chars()
        .all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        .then(|| id.to_string())
}

/// A path from a manifest must stay inside the game directory. `Path::join`
/// alone would happily accept `../../.config`, so the components are checked
/// before anything is joined.
pub fn safe_relative(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return None;
    }
    let parsed = Path::new(path);
    if parsed
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Some(path.to_string())
    } else {
        None
    }
}

pub fn retro_id(platform: &str, path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!(
        "retro:{platform}:{:016x}",
        fnv1a64(path.as_os_str().as_bytes())
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn which(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|directory| {
                let candidate = directory.join(program);
                candidate.is_file()
                    && candidate
                        .metadata()
                        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn options(root: &Path) -> ScanOptions {
        let libretro = root.join("libretro");
        fs::create_dir_all(&libretro).unwrap();
        fs::write(libretro.join("snes9x_libretro.so"), b"").unwrap();
        ScanOptions {
            roots: vec![root.join("roms")],
            system_dir: root.join("system"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            libretro_dir: libretro,
        }
    }

    #[test]
    fn scan_finds_roms_and_skips_the_folders_that_are_not_games() {
        let temp = tempfile::tempdir().unwrap();
        let options = options(temp.path());
        fs::create_dir_all(options.roots[0].join("snes")).unwrap();
        fs::create_dir_all(options.roots[0].join("bios")).unwrap();
        fs::create_dir_all(options.roots[0].join("art")).unwrap();
        fs::write(
            options.roots[0].join("snes/Super Mario World (USA).sfc"),
            vec![0u8; 64 * 1024],
        )
        .unwrap();
        // `.md` is a Mega Drive extension and Markdown's; the size floor is
        // what keeps a README out of the Genesis shelf.
        fs::write(options.roots[0].join("README.md"), b"# not a ROM\n").unwrap();
        fs::write(options.roots[0].join("bios/scph5501.bin"), b"bios").unwrap();
        fs::write(options.roots[0].join("art/cover.png"), b"png").unwrap();

        let web = options.system_dir.join("web/snake");
        fs::create_dir_all(&web).unwrap();
        fs::write(
            web.join("game.toml"),
            "id = \"snake\"\ntitle = \"Snake\"\nentry = \"index.html\"\nart = \"art.svg\"\n",
        )
        .unwrap();
        fs::write(web.join("index.html"), b"<html></html>").unwrap();
        fs::write(web.join("art.svg"), b"<svg/>").unwrap();

        let library = scan(&options, &State::default());
        assert_eq!(library.games.len(), 2, "{:#?}", library.games);
        let retro = library
            .games
            .iter()
            .find(|game| game.kind == GameKind::Retro)
            .unwrap();
        assert_eq!(retro.title, "Super Mario World");
        assert_eq!(retro.platform, "snes");
        assert_eq!(retro.core.as_deref(), Some("snes9x"));
        assert_eq!(
            retro.rom.as_deref(),
            Some(
                options.roots[0]
                    .join("snes/Super Mario World (USA).sfc")
                    .canonicalize()
                    .unwrap()
                    .as_path()
            )
        );
        let web = library
            .games
            .iter()
            .find(|game| game.kind == GameKind::Web)
            .unwrap();
        assert_eq!(web.id, "web:snake");
        assert_eq!(web.entry.as_deref(), Some("index.html"));
        assert_eq!(web.art.as_deref(), Some("art.svg"));

        let public = library.public();
        assert!(public.games.iter().any(|game| game.core_present));
        let json = serde_json::to_string(&public).unwrap();
        assert!(!json.contains("\"rom\""), "the page must not see ROM paths");
        assert!(!json.contains("Super Mario World (USA).sfc"));
        assert!(json.contains("lazyos-game://snake/art.svg"));
    }

    #[test]
    fn a_manifest_cannot_escape_its_directory() {
        let temp = tempfile::tempdir().unwrap();
        let options = options(temp.path());
        let web = options.system_dir.join("web/evil");
        fs::create_dir_all(&web).unwrap();
        fs::write(
            web.join("game.toml"),
            "id = \"evil\"\ntitle = \"Evil\"\nentry = \"../../secret.html\"\n",
        )
        .unwrap();
        let library = scan(&options, &State::default());
        assert!(library.games.is_empty(), "{:#?}", library.games);
        assert_eq!(safe_relative("../../secret.html"), None);
        assert_eq!(safe_relative("/etc/passwd"), None);
        assert_eq!(safe_relative("game/../secret"), None);
        assert_eq!(
            safe_relative("game/index.html").as_deref(),
            Some("game/index.html")
        );
    }

    #[test]
    fn state_round_trips_and_survives_a_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        assert_eq!(load_state(&path).last.len(), 0);
        let mut state = State::default();
        state.favorites.insert("web:snake".into());
        state.last.insert("web:snake".into(), 42);
        state.filter = "web".into();
        state.index = 3;
        save_state(&path, &state).unwrap();
        let back = load_state(&path);
        assert_eq!(back.favorites, state.favorites);
        assert_eq!(back.last, state.last);
        assert_eq!(back.filter, "web");
        assert_eq!(back.index, 3);
    }

    #[test]
    fn a_game_id_is_stable_across_scans() {
        let path = Path::new("/home/lazy/Games/snes/Game.sfc");
        assert_eq!(retro_id("snes", path), retro_id("snes", path));
        assert_ne!(
            retro_id("snes", path),
            retro_id("snes", Path::new("/other.sfc"))
        );
        assert!(retro_id("snes", path).starts_with("retro:snes:"));
    }
}
