//! The platforms the library knows, and the one core each one runs.
//!
//! This table is data, not a command line: a game's `id` selects a row, and the
//! row names the core file. Nothing here is assembled from a ROM's name or from
//! anything a web page said, which is why a file called `; rm -rf ~.sfc` is just
//! a title and an extension.

use std::path::Path;

/// One emulated platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    /// The short id the library and the UI use (`snes`, `psx`, `n64`).
    pub id: &'static str,
    /// The name the grid and the rail show.
    pub title: &'static str,
    /// The libretro core basename, without `_libretro.so`.
    pub core: &'static str,
    /// Lower-case extensions, without the dot.
    pub extensions: &'static [&'static str],
    /// The firmware the platform needs, if any (documentation, never shipped).
    pub bios: Option<&'static str>,
    /// When set, the platform is claimed only for a file inside a folder with
    /// one of these names. Neo Geo is the case: a `.zip` is a romset under
    /// `neogeo/`, but a zipped SNES ROM somewhere else must not become an
    /// arcade game.
    pub folders: Option<&'static [&'static str]>,
    /// The smallest file that can be a ROM. `.md` is a Mega Drive extension
    /// and also Markdown, and `.cue`/`.m3u` are text descriptors; the floor is
    /// what keeps a README out of the Genesis shelf.
    pub min_bytes: u64,
}

pub const PLATFORMS: &[Platform] = &[
    Platform {
        id: "nes",
        title: "Nintendo Entertainment System",
        core: "nestopia",
        extensions: &["nes", "fds"],
        bios: Some("disksys.rom, for Famicom Disk System games"),
        folders: None,
        min_bytes: 16 * 1024,
    },
    Platform {
        id: "snes",
        title: "Super Nintendo",
        core: "snes9x",
        extensions: &["sfc", "smc", "swc", "fig"],
        bios: None,
        folders: None,
        min_bytes: 32 * 1024,
    },
    Platform {
        id: "gb",
        title: "Game Boy",
        core: "gambatte",
        extensions: &["gb", "gbc"],
        bios: None,
        folders: None,
        min_bytes: 32 * 1024,
    },
    Platform {
        id: "gba",
        title: "Game Boy Advance",
        core: "mgba",
        extensions: &["gba"],
        bios: Some("gba_bios.bin, optional (mGBA runs without it)"),
        folders: None,
        min_bytes: 64 * 1024,
    },
    Platform {
        id: "genesis",
        title: "Genesis / Mega Drive",
        core: "genesis_plus_gx",
        extensions: &["md", "gen", "smd", "32x"],
        bios: None,
        folders: None,
        min_bytes: 64 * 1024,
    },
    Platform {
        id: "sms",
        title: "Master System / Game Gear",
        core: "genesis_plus_gx",
        extensions: &["sms", "gg"],
        bios: None,
        folders: None,
        min_bytes: 8 * 1024,
    },
    Platform {
        id: "psx",
        title: "PlayStation",
        core: "mednafen_psx",
        extensions: &["cue", "chd", "pbp", "m3u", "toc", "ccd"],
        bios: Some("scph5501.bin (US), scph5502.bin (EU), scph5500.bin (JP)"),
        folders: None,
        min_bytes: 0,
    },
    Platform {
        id: "pce",
        title: "PC Engine / TurboGrafx-16",
        core: "mednafen_pce_fast",
        extensions: &["pce", "sgx"],
        bios: Some("syscard3.pce, for CD games"),
        folders: None,
        min_bytes: 8 * 1024,
    },
    Platform {
        id: "vb",
        title: "Virtual Boy",
        core: "mednafen_vb",
        extensions: &["vb"],
        bios: None,
        folders: None,
        min_bytes: 64 * 1024,
    },
    Platform {
        id: "wswan",
        title: "WonderSwan",
        core: "mednafen_wswan",
        extensions: &["ws", "wsc"],
        bios: None,
        folders: None,
        min_bytes: 64 * 1024,
    },
    Platform {
        id: "n64",
        title: "Nintendo 64",
        core: "mupen64plus_next",
        extensions: &["n64", "z64", "v64"],
        bios: None,
        folders: None,
        min_bytes: 1024 * 1024,
    },
    Platform {
        id: "neogeo",
        title: "Neo Geo",
        core: "fbneo_neogeo",
        // A romset stays zipped: FBNeo reads the .zip itself.
        extensions: &["zip"],
        bios: Some("neogeo.zip, the MVS BIOS (also AES)"),
        folders: Some(&["neogeo", "mvs", "aes", "fbneo"]),
        min_bytes: 64 * 1024,
    },
];

/// The platform a file extension belongs to, if any. `bin` is deliberately
/// absent: a PS1 disc is selected by its `.cue`/`.chd`, and a stray `.bin` is
/// more often a BIOS or a patch than a Mega Drive ROM.
pub fn by_extension(extension: &str) -> Option<&'static Platform> {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    PLATFORMS.iter().find(|platform| {
        platform.folders.is_none() && platform.extensions.contains(&extension.as_str())
    })
}

/// The platform a file belongs to, folder rule included: `detect` is what the
/// scan uses, because a `.zip` is only a Neo Geo romset inside one of its
/// folders. `by_extension` stays for callers that have an extension only.
pub fn detect(path: &Path) -> Option<&'static Platform> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let folder = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_ascii_lowercase());
    PLATFORMS.iter().find(|platform| {
        if !platform.extensions.contains(&extension.as_str()) {
            return false;
        }
        match platform.folders {
            None => true,
            Some(folders) => folder
                .as_deref()
                .is_some_and(|folder| folders.contains(&folder)),
        }
    })
}

#[cfg(test)]
pub fn by_id(id: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|platform| platform.id == id)
}

/// A file stem as a title: underscores become spaces, trailing dump tags such
/// as `(USA)`, `(Rev 1)` or `[!]` are removed, and the spaces are collapsed.
///
/// A group is only removed at the end, and a title that is nothing but a group
/// keeps it. The heuristic is the one every frontend uses: `(World)` is a
/// region tag, not a parenthesis the game's name needs.
pub fn pretty_title(stem: &str) -> String {
    let mut title = stem.replace('_', " ");
    loop {
        let trimmed = title.trim_end();
        let open = match (trimmed.rfind('('), trimmed.rfind('[')) {
            (Some(paren), Some(bracket)) => Some(paren.max(bracket)),
            (Some(paren), None) => Some(paren),
            (None, Some(bracket)) => Some(bracket),
            (None, None) => None,
        };
        let Some(open) = open else {
            break;
        };
        let close = if trimmed.as_bytes()[open] == b'(' {
            ')'
        } else {
            ']'
        };
        if !trimmed.ends_with(close) {
            break;
        }
        let rest = trimmed[..open].trim_end();
        if rest.is_empty() {
            break;
        }
        title = rest.to_string();
    }
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_extension_belongs_to_exactly_one_platform() {
        let mut seen = std::collections::BTreeSet::new();
        for platform in PLATFORMS {
            for extension in platform.extensions {
                assert!(
                    seen.insert(*extension),
                    "{extension} is claimed by more than one platform"
                );
                match platform.folders {
                    None => assert_eq!(
                        by_extension(extension).map(|p| p.id),
                        Some(platform.id),
                        "{extension} does not resolve to {}",
                        platform.id
                    ),
                    // A folder-scoped platform must not be claimed by the
                    // extension alone, and must be claimed inside its folder.
                    Some(folders) => {
                        assert_eq!(
                            by_extension(extension),
                            None,
                            "{extension} must not be claimed by extension alone"
                        );
                        let inside = Path::new(folders[0]).join(format!("game.{extension}"));
                        assert_eq!(
                            detect(&inside).map(|p| p.id),
                            Some(platform.id),
                            "{extension} inside {}/ does not resolve to {}",
                            folders[0],
                            platform.id
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_zip_is_neogeo_only_inside_its_folders() {
        for folder in ["neogeo", "mvs", "aes", "fbneo", "NeoGeo"] {
            let rom = Path::new(folder).join("mslug.zip");
            assert_eq!(detect(&rom).map(|p| p.id), Some("neogeo"), "{folder}");
        }
        assert_eq!(detect(Path::new("snes/zipped.zip")), None);
        assert_eq!(detect(Path::new("zipped.zip")), None);
        assert_eq!(
            detect(Path::new("snes/Super Mario World.sfc")).map(|p| p.id),
            Some("snes")
        );
        assert_eq!(detect(Path::new("n64/game.z64")).map(|p| p.id), Some("n64"));
    }

    #[test]
    fn extensions_are_case_insensitive_and_dots_are_optional() {
        assert_eq!(by_extension("SFC").map(|p| p.id), Some("snes"));
        assert_eq!(by_extension(".N64").map(|p| p.id), Some("n64"));
        assert_eq!(by_extension("bin"), None);
        assert_eq!(by_extension("exe"), None);
    }

    #[test]
    fn dump_tags_are_removed_from_the_end_only() {
        assert_eq!(pretty_title("Super_Mario_World_(USA)"), "Super Mario World");
        assert_eq!(pretty_title("Zelda (USA) (Rev 1) [!]"), "Zelda");
        assert_eq!(pretty_title("Super Mario (World)"), "Super Mario");
        assert_eq!(pretty_title("  spaced__out  "), "spaced out");
        assert_eq!(pretty_title("(USA)"), "(USA)");
    }

    #[test]
    fn the_platform_table_has_the_cores_the_image_ships() {
        for (id, core) in [
            ("nes", "nestopia"),
            ("snes", "snes9x"),
            ("gb", "gambatte"),
            ("gba", "mgba"),
            ("genesis", "genesis_plus_gx"),
            ("psx", "mednafen_psx"),
            ("n64", "mupen64plus_next"),
            ("neogeo", "fbneo_neogeo"),
        ] {
            assert_eq!(by_id(id).map(|p| p.core), Some(core));
        }
    }
}
