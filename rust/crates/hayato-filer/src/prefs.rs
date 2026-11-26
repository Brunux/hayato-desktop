//! Persisted window & view preferences for `hayato-filer`.
//!
//! Finder/elementary minimalism only works if the window comes back the way the
//! person left it: grid or list, the sort they chose, a collapsed sidebar and
//! the window size. That is one small JSON file, written on close and read on
//! launch; nothing else is remembered here (the folder itself is the argument
//! the launcher passes, never a second state file).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{SortField, SortOrder, ViewMode, home_dir};

/// Smallest window we will restore. A hand-edited or truncated file must never
/// produce a window too small to use.
const MIN_WIDTH: i32 = 640;
const MIN_HEIGHT: i32 = 420;
const DEFAULT_WIDTH: i32 = 980;
const DEFAULT_HEIGHT: i32 = 640;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilerPrefs {
    pub view_mode: ViewMode,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub sidebar_compact: bool,
    pub window_width: i32,
    pub window_height: i32,
}

impl Default for FilerPrefs {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::Grid,
            sort_field: SortField::Name,
            sort_order: SortOrder::Asc,
            folders_first: true,
            show_hidden: false,
            sidebar_compact: false,
            window_width: DEFAULT_WIDTH,
            window_height: DEFAULT_HEIGHT,
        }
    }
}

impl FilerPrefs {
    /// Bring a file read from disk back inside the usable range.
    pub fn sanitized(mut self) -> Self {
        self.window_width = self.window_width.max(MIN_WIDTH);
        self.window_height = self.window_height.max(MIN_HEIGHT);
        self
    }
}

pub fn prefs_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("hayato/filer-state.json");
    }
    home_dir().join(".config/hayato/filer-state.json")
}

pub fn load() -> FilerPrefs {
    load_from(&prefs_path())
}

pub fn load_from(path: &std::path::Path) -> FilerPrefs {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return FilerPrefs::default();
    };
    serde_json::from_str::<FilerPrefs>(&raw)
        .map(FilerPrefs::sanitized)
        .unwrap_or_default()
}

pub fn save(prefs: &FilerPrefs) {
    save_to(&prefs_path(), prefs);
}

pub fn save_to(path: &std::path::Path, prefs: &FilerPrefs) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filer-state.json");
        let prefs = FilerPrefs {
            view_mode: ViewMode::List,
            sort_field: SortField::Size,
            sort_order: SortOrder::Desc,
            folders_first: false,
            show_hidden: true,
            sidebar_compact: true,
            window_width: 1280,
            window_height: 800,
        };
        save_to(&path, &prefs);
        assert_eq!(load_from(&path), prefs);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filer-state.json");
        std::fs::write(&path, r#"{"view_mode":"list","show_hidden":true}"#).unwrap();
        let prefs = load_from(&path);
        assert_eq!(prefs.view_mode, ViewMode::List);
        assert!(prefs.show_hidden);
        // Everything the file did not mention keeps its default.
        assert_eq!(prefs.sort_field, SortField::Name);
        assert_eq!(prefs.sort_order, SortOrder::Asc);
        assert_eq!(prefs.window_width, DEFAULT_WIDTH);
        assert!(prefs.folders_first);
    }

    #[test]
    fn corrupt_file_and_missing_file_both_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filer-state.json");
        assert_eq!(load_from(&path), FilerPrefs::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), FilerPrefs::default());
    }

    #[test]
    fn a_tiny_window_is_lifted_to_the_usable_minimum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filer-state.json");
        std::fs::write(&path, r#"{"window_width":120,"window_height":80}"#).unwrap();
        let prefs = load_from(&path);
        assert_eq!(prefs.window_width, MIN_WIDTH);
        assert_eq!(prefs.window_height, MIN_HEIGHT);
    }
}
