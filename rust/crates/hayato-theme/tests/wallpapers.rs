//! Every built-in theme cycles exactly three wallpapers, and the three files
//! are in the tree.
//!
//! The picker and `Alt+Shift+W` walk the list a theme declares; a theme with one
//! picture has nowhere to go, a theme with four has a fourth nobody asked for,
//! and a name that no file matches is a blank screen. This is the witness for
//! `specs/026-hayato-rebrand.md`.

use std::collections::BTreeSet;
use std::path::Path;

use hayato_theme::{Theme, builtin};

fn theme(slug: &str, text: &str) -> Theme {
    Theme::parse(text).unwrap_or_else(|error| panic!("{slug}: {error}"))
}

#[test]
fn every_theme_cycles_three_distinct_wallpapers() {
    for (slug, text) in builtin::ALL {
        let theme = theme(slug, text);
        assert_eq!(
            theme.wallpapers.len(),
            3,
            "{slug}: {} wallpapers, want 3",
            theme.wallpapers.len()
        );
        let unique: BTreeSet<&String> = theme.wallpapers.iter().collect();
        assert_eq!(unique.len(), 3, "{slug}: a wallpaper is listed twice");
        for name in &theme.wallpapers {
            assert!(
                name.ends_with(".jpg") || name.ends_with(".png"),
                "{slug}: {name} is not a picture"
            );
            assert!(
                !name.to_lowercase().contains("omarchy"),
                "{slug}: {name} still carries the old brand name"
            );
        }
    }
}

#[test]
fn every_wallpaper_file_is_in_the_tree() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../uis/sway/files/home/Pictures/Wallpapers");
    let dir = dir
        .canonicalize()
        .unwrap_or_else(|error| panic!("wallpaper directory {}: {error}", dir.display()));
    for (slug, text) in builtin::ALL {
        for name in theme(slug, text).wallpapers {
            assert!(
                dir.join(&name).is_file(),
                "{slug}: {name} is declared but not in {}",
                dir.display()
            );
        }
    }
}
