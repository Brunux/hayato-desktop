//! Pure-Rust navigation, sorting, filtering, and bookmark state for `hayato-filer`.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::FileEntry;
use crate::fs::ops::UndoAction;
use crate::prefs::FilerPrefs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    #[serde(alias = "kind")]
    Name,
    Modified,
    Size,
    Perms,
    Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub fn toggle(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TabState {
    pub cwd: PathBuf,
    pub in_trash: bool,
    pub history: Vec<PathBuf>,
    pub history_idx: usize,
    pub filter: String,
    pub entries: Vec<FileEntry>,
    pub visible: Vec<FileEntry>,
}

impl TabState {
    pub fn new(initial: PathBuf) -> Self {
        let in_trash = is_trash_files_dir(&initial);
        Self {
            cwd: initial.clone(),
            in_trash,
            history: vec![initial],
            history_idx: 0,
            filter: String::new(),
            entries: Vec::new(),
            visible: Vec::new(),
        }
    }

    pub fn title(&self) -> String {
        if self.in_trash {
            return "Trash".to_string();
        }
        if self.cwd == home_dir() {
            return "Home".to_string();
        }
        self.cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/".to_string())
    }

    pub fn icon_glyph(&self) -> &'static str {
        if self.in_trash {
            "\u{f1f8}"
        } else if self.cwd == home_dir() {
            "\u{f015}"
        } else {
            "\u{f07b}"
        }
    }

    pub fn can_back(&self) -> bool {
        self.history_idx > 0
    }

    pub fn can_forward(&self) -> bool {
        self.history_idx + 1 < self.history.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub path: PathBuf,
    pub icon: String,
}

/// One non-Home entry in the sidebar PLACES list (either a default XDG folder,
/// Trash, or a user-pinned bookmark) that can be freely reordered by drag-and-drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceItem {
    pub key: String,
    pub glyph: String,
    pub name: String,
    pub path: PathBuf,
    pub is_trash: bool,
    pub is_bookmark: bool,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub view_mode: ViewMode,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub sidebar_compact: bool,
    pub col_size_width: i32,
    pub col_mtime_width: i32,
    pub col_perms_width: i32,
    pub col_owner_width: i32,
    pub bookmarks: Vec<Bookmark>,
    /// Persisted order of non-Home PLACES entries (both defaults and pinned folders).
    pub place_order: Vec<String>,
    /// Session undo history, newest last. Finder-style: Ctrl+Z reverses the
    /// most recent rename, move, trash, copy or folder creation.
    pub undo: Vec<UndoAction>,
}

impl AppState {
    pub fn new(initial: PathBuf) -> Self {
        let bookmarks = load_bookmarks();
        let place_order = load_place_order();
        Self {
            tabs: vec![TabState::new(initial)],
            active_tab: 0,
            view_mode: ViewMode::Grid,
            sort_field: SortField::Name,
            sort_order: SortOrder::Asc,
            folders_first: true,
            show_hidden: false,
            sidebar_compact: false,
            col_size_width: crate::prefs::DEFAULT_COL_SIZE,
            col_mtime_width: crate::prefs::DEFAULT_COL_MTIME,
            col_perms_width: crate::prefs::DEFAULT_COL_PERMS,
            col_owner_width: crate::prefs::DEFAULT_COL_OWNER,
            bookmarks,
            place_order,
            undo: Vec::new(),
        }
    }

    /// Apply persisted window & view preferences onto a fresh state.
    pub fn apply_prefs(&mut self, prefs: &FilerPrefs) {
        self.view_mode = prefs.view_mode;
        self.sort_field = prefs.sort_field;
        self.sort_order = prefs.sort_order;
        self.folders_first = prefs.folders_first;
        self.show_hidden = prefs.show_hidden;
        self.sidebar_compact = prefs.sidebar_compact;
        self.col_size_width = prefs.col_size_width;
        self.col_mtime_width = prefs.col_mtime_width;
        self.col_perms_width = prefs.col_perms_width;
        self.col_owner_width = prefs.col_owner_width;
    }

    /// Snapshot the view choices into a persistable struct.
    pub fn to_prefs(&self, width: i32, height: i32) -> FilerPrefs {
        FilerPrefs {
            view_mode: self.view_mode,
            sort_field: self.sort_field,
            sort_order: self.sort_order,
            folders_first: self.folders_first,
            show_hidden: self.show_hidden,
            sidebar_compact: self.sidebar_compact,
            window_width: width,
            window_height: height,
            col_size_width: self.col_size_width,
            col_mtime_width: self.col_mtime_width,
            col_perms_width: self.col_perms_width,
            col_owner_width: self.col_owner_width,
        }
    }

    pub fn push_undo(&mut self, action: UndoAction) {
        self.undo.push(action);
    }

    pub fn take_undo(&mut self) -> Option<UndoAction> {
        self.undo.pop()
    }

    pub fn tab(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }

    pub fn tab_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }

    pub fn cwd(&self) -> &Path {
        &self.tab().cwd
    }

    pub fn in_trash(&self) -> bool {
        self.tab().in_trash
    }

    pub fn new_tab(&mut self, dir: PathBuf) -> usize {
        self.tabs.push(TabState::new(dir));
        self.active_tab = self.tabs.len() - 1;
        self.active_tab
    }

    /// Closes tab at `idx`. Returns `false` if it was the only remaining tab.
    pub fn close_tab(&mut self, idx: usize) -> bool {
        if self.tabs.len() <= 1 || idx >= self.tabs.len() {
            return false;
        }
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if idx < self.active_tab {
            self.active_tab -= 1;
        }
        true
    }

    pub fn switch_tab(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() || idx == self.active_tab {
            return false;
        }
        self.active_tab = idx;
        true
    }

    pub fn next_tab(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
        true
    }

    pub fn prev_tab(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        true
    }

    pub fn navigate(&mut self, target: PathBuf, in_trash: bool) {
        let tab = self.tab_mut();
        if tab.cwd == target && tab.in_trash == in_trash {
            return;
        }
        tab.in_trash = in_trash;
        tab.cwd = target.clone();
        tab.filter.clear();
        if tab.history_idx + 1 < tab.history.len() {
            tab.history.truncate(tab.history_idx + 1);
        }
        tab.history.push(target);
        tab.history_idx = tab.history.len() - 1;
    }

    pub fn can_back(&self) -> bool {
        self.tab().can_back()
    }

    pub fn can_forward(&self) -> bool {
        self.tab().can_forward()
    }

    pub fn go_back(&mut self) -> Option<PathBuf> {
        let tab = self.tab_mut();
        if !tab.can_back() {
            return None;
        }
        tab.history_idx -= 1;
        let next = tab.history[tab.history_idx].clone();
        tab.in_trash = is_trash_files_dir(&next);
        tab.cwd = next.clone();
        tab.filter.clear();
        Some(next)
    }

    pub fn go_forward(&mut self) -> Option<PathBuf> {
        let tab = self.tab_mut();
        if !tab.can_forward() {
            return None;
        }
        tab.history_idx += 1;
        let next = tab.history[tab.history_idx].clone();
        tab.in_trash = is_trash_files_dir(&next);
        tab.cwd = next.clone();
        tab.filter.clear();
        Some(next)
    }

    pub fn go_parent(&mut self) -> Option<PathBuf> {
        if self.in_trash() {
            let home = home_dir();
            self.navigate(home.clone(), false);
            return Some(home);
        }
        let parent = self.cwd().parent()?.to_path_buf();
        if parent == self.cwd() {
            return None;
        }
        self.navigate(parent.clone(), false);
        Some(parent)
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.tab_mut().entries = entries;
        self.recompute_visible();
    }

    pub fn recompute_visible(&mut self) {
        let field = self.sort_field;
        let order = self.sort_order;
        let folders_first = self.folders_first;
        let show_hidden = self.show_hidden;

        let tab = self.tab_mut();
        let needle = tab.filter.trim().to_lowercase();
        let mut out: Vec<FileEntry> = tab
            .entries
            .iter()
            .filter(|e| {
                if !show_hidden && !tab.in_trash && e.name.starts_with('.') {
                    return false;
                }
                if !needle.is_empty() && !e.name.to_lowercase().contains(&needle) {
                    return false;
                }
                true
            })
            .cloned()
            .collect();

        out.sort_by(|a, b| {
            if folders_first && a.is_dir != b.is_dir {
                return if a.is_dir {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            let cmp = match field {
                SortField::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortField::Modified => a.modified_secs.cmp(&b.modified_secs),
                SortField::Size => a.size.cmp(&b.size),
                SortField::Perms => {
                    crate::fs::format_mode(a.mode).cmp(&crate::fs::format_mode(b.mode))
                }
                SortField::Owner => a.owner.to_lowercase().cmp(&b.owner.to_lowercase()),
            };
            let cmp = if cmp == Ordering::Equal {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            } else {
                cmp
            };
            match order {
                SortOrder::Asc => cmp,
                SortOrder::Desc => cmp.reverse(),
            }
        });

        tab.visible = out;
    }

    pub fn pin_bookmark(&mut self, dir: PathBuf) -> bool {
        if !dir.is_dir() || self.is_bookmarked(&dir) {
            return false;
        }
        let used: Vec<&str> = self.bookmarks.iter().map(|b| b.icon.as_str()).collect();
        let icon = pick_bookmark_icon(&dir, &used).to_string();
        self.bookmarks.push(Bookmark { path: dir, icon });
        save_bookmarks(&self.bookmarks);
        true
    }

    /// Remove a pinned folder from PLACES. Returns `false` when it was not a
    /// bookmark, so a caller never announces an unpin that did not happen.
    pub fn unpin_bookmark(&mut self, dir: &Path) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.path != dir);
        if self.bookmarks.len() == before {
            return false;
        }
        let pin_key = format!("pin:{}", dir.display());
        self.place_order.retain(|k| k != &pin_key);
        save_bookmarks(&self.bookmarks);
        save_place_order(&self.place_order);
        true
    }

    pub fn is_bookmarked(&self, dir: &Path) -> bool {
        self.bookmarks.iter().any(|b| b.path == dir)
    }

    /// Returns all non-Home PLACES entries (defaults + Trash + pinned bookmarks)
    /// in the user's persisted order. `Home` is always static and rendered first.
    pub fn ordered_places(&self) -> Vec<PlaceItem> {
        let home = home_dir();
        let trash_dir = trash_files_dir();
        let mut items: Vec<PlaceItem> = Vec::new();

        for (key, glyph, name, path, is_trash) in [
            (
                "desktop",
                "\u{f108}",
                "Desktop",
                home.join("Desktop"),
                false,
            ),
            (
                "documents",
                "\u{f15c}",
                "Documents",
                home.join("Documents"),
                false,
            ),
            (
                "downloads",
                "\u{f019}",
                "Downloads",
                home.join("Downloads"),
                false,
            ),
            (
                "pictures",
                "\u{f03e}",
                "Pictures",
                home.join("Pictures"),
                false,
            ),
            ("music", "\u{f001}", "Music", home.join("Music"), false),
            ("videos", "\u{f008}", "Videos", home.join("Videos"), false),
            ("trash", "\u{f1f8}", "Trash", trash_dir, true),
        ] {
            if !is_trash && !path.is_dir() {
                continue;
            }
            items.push(PlaceItem {
                key: key.to_string(),
                glyph: glyph.to_string(),
                name: name.to_string(),
                path,
                is_trash,
                is_bookmark: false,
            });
        }

        for bm in &self.bookmarks {
            let name = bm
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| bm.path.display().to_string());
            items.push(PlaceItem {
                key: format!("pin:{}", bm.path.display()),
                glyph: bm.icon.clone(),
                name,
                path: bm.path.clone(),
                is_trash: false,
                is_bookmark: true,
            });
        }

        order_place_items(items, &self.place_order)
    }

    /// Reorder a non-Home PLACES item (`from_key`) to the position of `to_key`.
    pub fn reorder_place(&mut self, from_key: &str, to_key: &str) -> bool {
        if from_key == to_key || from_key == "home" || to_key == "home" {
            return false;
        }
        let mut keys: Vec<String> = self
            .ordered_places()
            .into_iter()
            .map(|item| item.key)
            .collect();
        let Some(from_idx) = keys.iter().position(|k| k == from_key) else {
            return false;
        };
        let Some(to_idx) = keys.iter().position(|k| k == to_key) else {
            return false;
        };
        let moved = keys.remove(from_idx);
        keys.insert(to_idx, moved);
        self.place_order = keys;
        save_place_order(&self.place_order);
        true
    }
}

fn order_place_items(mut items: Vec<PlaceItem>, order: &[String]) -> Vec<PlaceItem> {
    if order.is_empty() {
        return items;
    }
    let mut out = Vec::with_capacity(items.len());
    for key in order {
        if let Some(pos) = items.iter().position(|it| &it.key == key) {
            out.push(items.remove(pos));
        }
    }
    out.extend(items);
    out
}

/// Curated fallback pool of distinct Font Awesome glyphs (verified in FA 4.7 & 6),
/// avoiding the default Places/Devices icons so every pinned folder stands out.
const BOOKMARK_FALLBACK_ICONS: &[&str] = &[
    "\u{f005}", // star
    "\u{f02e}", // bookmark
    "\u{f02b}", // tag
    "\u{f0e7}", // bolt
    "\u{f135}", // rocket
    "\u{f06c}", // leaf
    "\u{f1b2}", // cube
    "\u{f004}", // heart
    "\u{f06d}", // fire
    "\u{f0a3}", // certificate
    "\u{f14e}", // compass
    "\u{f0c3}", // flask
    "\u{f02d}", // book
    "\u{f0b1}", // briefcase
    "\u{f121}", // code
    "\u{f030}", // camera
    "\u{f025}", // headphones
    "\u{f11b}", // gamepad
    "\u{f0c2}", // cloud
    "\u{f187}", // archive
];

/// Choose a distinct Font Awesome icon for a pinned folder based on its name,
/// skipping any icon already in `used` and advancing to the next suggestion.
pub fn pick_bookmark_icon(path: &Path, used: &[&str]) -> &'static str {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    const RULES: &[(&[&str], &[&str])] = &[
        (
            &[
                "dev",
                "code",
                "src",
                "git",
                "repo",
                "proj",
                "rust",
                "workspace",
                "hack",
                "app",
            ],
            &["\u{f121}", "\u{f120}", "\u{f126}", "\u{f013}", "\u{f188}"],
        ),
        (
            &["work", "job", "office", "client", "biz"],
            &["\u{f0b1}", "\u{f0f2}", "\u{f02d}", "\u{f073}"],
        ),
        (
            &["game", "rom", "steam", "emu", "play"],
            &["\u{f11b}", "\u{f091}", "\u{f135}", "\u{f0e7}"],
        ),
        (
            &[
                "book", "read", "epub", "pdf", "doc", "study", "learn", "note", "wiki", "obsidian",
            ],
            &["\u{f02d}", "\u{f044}", "\u{f19d}", "\u{f02e}"],
        ),
        (
            &[
                "photo", "cam", "img", "art", "design", "draw", "wall", "screen", "shot",
            ],
            &["\u{f030}", "\u{f1fc}", "\u{f06e}", "\u{f004}"],
        ),
        (
            &["audio", "sound", "pod", "rec", "studio", "mix", "song"],
            &["\u{f025}", "\u{f130}", "\u{f028}", "\u{f1de}"],
        ),
        (
            &[
                "share", "drop", "sync", "cloud", "drive", "net", "web", "site", "pub",
            ],
            &["\u{f0c2}", "\u{f0ac}", "\u{f1e0}", "\u{f0ee}"],
        ),
        (
            &[
                "archive", "backup", "bak", "old", "vault", "safe", "sec", "key", "ssh", "gpg",
            ],
            &["\u{f187}", "\u{f023}", "\u{f084}", "\u{f132}"],
        ),
        (
            &["tmp", "temp", "scratch", "lab", "test", "exp", "sandbox"],
            &["\u{f0c3}", "\u{f188}", "\u{f0ad}", "\u{f0e7}"],
        ),
        (
            &["config", "dot", "etc", "bin", "script", "tool", "sys"],
            &["\u{f013}", "\u{f0ad}", "\u{f120}", "\u{f1de}"],
        ),
        (
            &["mail", "inbox", "msg", "chat"],
            &["\u{f0e0}", "\u{f01c}", "\u{f075}", "\u{f1d8}"],
        ),
        (
            &["fin", "tax", "bank", "money", "bill", "invoice"],
            &["\u{f09d}", "\u{f1ec}", "\u{f080}", "\u{f0d6}"],
        ),
        (
            &["travel", "trip", "map", "nav"],
            &["\u{f072}", "\u{f14e}", "\u{f041}", "\u{f0ac}"],
        ),
    ];

    for (keywords, glyphs) in RULES {
        if keywords.iter().any(|kw| name.contains(kw)) {
            for &glyph in *glyphs {
                if !used.contains(&glyph) {
                    return glyph;
                }
            }
        }
    }

    for &glyph in BOOKMARK_FALLBACK_ICONS {
        if !used.contains(&glyph) {
            return glyph;
        }
    }

    BOOKMARK_FALLBACK_ICONS[used.len() % BOOKMARK_FALLBACK_ICONS.len()]
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn trash_files_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("Trash/files");
    }
    home_dir().join(".local/share/Trash/files")
}

pub fn is_trash_files_dir(p: &Path) -> bool {
    p == trash_files_dir()
}

fn bookmarks_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("hayato/filer-bookmarks.json");
    }
    home_dir().join(".config/hayato/filer-bookmarks.json")
}

fn parse_bookmarks_json(raw: &str) -> Vec<Bookmark> {
    if let Ok(bms) = serde_json::from_str::<Vec<Bookmark>>(raw) {
        return bms;
    }
    // Backwards-compatible upgrade from legacy `Vec<PathBuf>` bookmarks file.
    if let Ok(paths) = serde_json::from_str::<Vec<PathBuf>>(raw) {
        let mut out: Vec<Bookmark> = Vec::with_capacity(paths.len());
        for path in paths {
            let used: Vec<&str> = out.iter().map(|b| b.icon.as_str()).collect();
            let icon = pick_bookmark_icon(&path, &used).to_string();
            out.push(Bookmark { path, icon });
        }
        return out;
    }
    Vec::new()
}

fn load_bookmarks() -> Vec<Bookmark> {
    let Ok(raw) = std::fs::read_to_string(bookmarks_path()) else {
        return Vec::new();
    };
    parse_bookmarks_json(&raw)
}

fn save_bookmarks(bookmarks: &[Bookmark]) {
    let path = bookmarks_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(bookmarks) {
        let _ = std::fs::write(path, json);
    }
}

fn place_order_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("hayato/filer-places-order.json");
    }
    home_dir().join(".config/hayato/filer-places-order.json")
}

fn load_place_order() -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(place_order_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
}

fn save_place_order(order: &[String]) {
    let path = place_order_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(order) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, is_dir: bool, size: u64, mtime: i64) -> FileEntry {
        FileEntry {
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            is_dir,
            is_symlink: false,
            size,
            modified_secs: mtime,
            mode: if is_dir { 0o755 } else { 0o644 },
            owner: "brunux".to_string(),
            kind: if is_dir { "Folder" } else { "File" }.to_string(),
            icon_glyph: if is_dir { "\u{f07b}" } else { "\u{f15b}" },
            icon_class: if is_dir { "icon-folder" } else { "icon-file" },
        }
    }

    #[test]
    fn history_back_forward_and_branch_truncate() {
        let mut st = AppState::new(PathBuf::from("/home/u"));
        st.navigate(PathBuf::from("/home/u/Docs"), false);
        st.navigate(PathBuf::from("/home/u/Pics"), false);
        assert!(st.can_back());
        assert!(!st.can_forward());
        assert_eq!(st.go_back(), Some(PathBuf::from("/home/u/Docs")));
        assert!(st.can_forward());
        st.navigate(PathBuf::from("/home/u/Music"), false);
        assert!(!st.can_forward());
        assert_eq!(st.cwd(), Path::new("/home/u/Music"));
    }

    #[test]
    fn tabs_open_switch_cycle_and_close() {
        let mut st = AppState::new(PathBuf::from("/home/u"));
        assert_eq!(st.tabs.len(), 1);
        assert!(!st.close_tab(0));

        let idx = st.new_tab(PathBuf::from("/tmp"));
        assert_eq!(idx, 1);
        assert_eq!(st.active_tab, 1);
        assert_eq!(st.cwd(), Path::new("/tmp"));

        st.new_tab(PathBuf::from("/var"));
        assert_eq!(st.tabs.len(), 3);
        assert_eq!(st.active_tab, 2);

        assert!(st.next_tab());
        assert_eq!(st.active_tab, 0);
        assert!(st.prev_tab());
        assert_eq!(st.active_tab, 2);

        // Closing a tab before active shifts active_tab down cleanly
        assert!(st.close_tab(0));
        assert_eq!(st.tabs.len(), 2);
        assert_eq!(st.active_tab, 1);
        assert_eq!(st.cwd(), Path::new("/var"));
    }

    #[test]
    fn prefs_round_trip_through_app_state() {
        let mut st = AppState::new(PathBuf::from("/home/u"));
        let prefs = FilerPrefs {
            view_mode: ViewMode::List,
            sort_field: SortField::Modified,
            sort_order: SortOrder::Desc,
            folders_first: false,
            show_hidden: true,
            sidebar_compact: true,
            window_width: 1200,
            window_height: 700,
            col_size_width: 112,
            col_mtime_width: 210,
            col_perms_width: 104,
            col_owner_width: 92,
        };
        st.apply_prefs(&prefs);
        assert_eq!(st.view_mode, ViewMode::List);
        assert_eq!(st.sort_field, SortField::Modified);
        assert_eq!(st.sort_order, SortOrder::Desc);
        assert!(!st.folders_first);
        assert!(st.show_hidden);
        assert!(st.sidebar_compact);
        assert_eq!(st.to_prefs(1200, 700), prefs);
    }

    #[test]
    fn undo_history_is_last_in_first_out() {
        let mut st = AppState::new(PathBuf::from("/home/u"));
        assert!(st.take_undo().is_none());
        st.push_undo(UndoAction::Rename {
            from: PathBuf::from("/tmp/a"),
            to: PathBuf::from("/tmp/b"),
        });
        st.push_undo(UndoAction::Created(vec![PathBuf::from("/tmp/new")]));
        assert_eq!(st.undo.len(), 2);
        assert_eq!(
            st.take_undo(),
            Some(UndoAction::Created(vec![PathBuf::from("/tmp/new")]))
        );
        assert_eq!(
            st.take_undo(),
            Some(UndoAction::Rename {
                from: PathBuf::from("/tmp/a"),
                to: PathBuf::from("/tmp/b"),
            })
        );
        assert!(st.undo.is_empty());
    }

    #[test]
    fn hidden_filter_and_folders_first_sorting() {
        let mut st = AppState::new(PathBuf::from("/tmp"));
        st.set_entries(vec![
            sample(".secret", false, 10, 100),
            sample("zeta.txt", false, 50, 200),
            sample("alpha_dir", true, 0, 50),
            sample("beta.txt", false, 20, 300),
        ]);
        assert_eq!(
            st.tab()
                .visible
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha_dir", "beta.txt", "zeta.txt"]
        );
        st.show_hidden = true;
        st.sort_field = SortField::Size;
        st.sort_order = SortOrder::Desc;
        st.recompute_visible();
        assert_eq!(
            st.tab()
                .visible
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha_dir", "zeta.txt", "beta.txt", ".secret"]
        );
    }

    #[test]
    fn bookmark_icons_match_folder_name_and_avoid_duplicates() {
        let dev = pick_bookmark_icon(Path::new("/home/u/Dev"), &[]);
        assert_eq!(dev, "\u{f121}");
        // Second dev-like folder picks the next suggestion instead of duplicating.
        let projects = pick_bookmark_icon(Path::new("/home/u/Projects"), &[dev]);
        assert_eq!(projects, "\u{f120}");
        assert_ne!(dev, projects);

        let games = pick_bookmark_icon(Path::new("/home/u/Games"), &[dev, projects]);
        assert_eq!(games, "\u{f11b}");

        // Legacy `Vec<PathBuf>` JSON upgrades seamlessly with distinct icons.
        let legacy = r#"["/home/u/Dev","/home/u/Projects","/home/u/Misc"]"#;
        let upgraded = parse_bookmarks_json(legacy);
        assert_eq!(upgraded.len(), 3);
        assert_eq!(upgraded[0].icon, "\u{f121}");
        assert_eq!(upgraded[1].icon, "\u{f120}");
        assert_ne!(upgraded[2].icon, upgraded[0].icon);
        assert_ne!(upgraded[2].icon, upgraded[1].icon);
    }

    #[test]
    fn places_order_respects_saved_keys_and_refuses_home() {
        let mk = |k: &str| PlaceItem {
            key: k.to_string(),
            glyph: "\u{f07b}".to_string(),
            name: k.to_string(),
            path: PathBuf::from(format!("/{k}")),
            is_trash: k == "trash",
            is_bookmark: k.starts_with("pin:"),
        };
        let items = vec![
            mk("desktop"),
            mk("downloads"),
            mk("trash"),
            mk("pin:/home/u/Dev"),
        ];
        let ordered = order_place_items(
            items,
            &["pin:/home/u/Dev".to_string(), "downloads".to_string()],
        );
        let keys: Vec<&str> = ordered.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["pin:/home/u/Dev", "downloads", "desktop", "trash"]
        );

        let mut st = AppState::new(PathBuf::from("/tmp"));
        assert!(!st.reorder_place("home", "trash"));
        assert!(!st.reorder_place("trash", "home"));
    }
}
