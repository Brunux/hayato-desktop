//! Pure-Rust navigation, sorting, filtering, and bookmark state for `hayato-filer`.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::fs::FileEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
pub struct AppState {
    pub cwd: PathBuf,
    pub in_trash: bool,
    pub history: Vec<PathBuf>,
    pub history_idx: usize,
    pub view_mode: ViewMode,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub filter: String,
    pub entries: Vec<FileEntry>,
    pub visible: Vec<FileEntry>,
    pub bookmarks: Vec<PathBuf>,
}

impl AppState {
    pub fn new(initial: PathBuf) -> Self {
        let bookmarks = load_bookmarks();
        Self {
            cwd: initial.clone(),
            in_trash: false,
            history: vec![initial],
            history_idx: 0,
            view_mode: ViewMode::Grid,
            sort_field: SortField::Name,
            sort_order: SortOrder::Asc,
            folders_first: true,
            show_hidden: false,
            filter: String::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            bookmarks,
        }
    }

    pub fn navigate(&mut self, target: PathBuf, in_trash: bool) {
        if self.cwd == target && self.in_trash == in_trash {
            return;
        }
        self.in_trash = in_trash;
        self.cwd = target.clone();
        self.filter.clear();
        if self.history_idx + 1 < self.history.len() {
            self.history.truncate(self.history_idx + 1);
        }
        self.history.push(target);
        self.history_idx = self.history.len() - 1;
    }

    pub fn can_back(&self) -> bool {
        self.history_idx > 0
    }

    pub fn can_forward(&self) -> bool {
        self.history_idx + 1 < self.history.len()
    }

    pub fn go_back(&mut self) -> Option<PathBuf> {
        if !self.can_back() {
            return None;
        }
        self.history_idx -= 1;
        let next = self.history[self.history_idx].clone();
        self.in_trash = is_trash_files_dir(&next);
        self.cwd = next.clone();
        self.filter.clear();
        Some(next)
    }

    pub fn go_forward(&mut self) -> Option<PathBuf> {
        if !self.can_forward() {
            return None;
        }
        self.history_idx += 1;
        let next = self.history[self.history_idx].clone();
        self.in_trash = is_trash_files_dir(&next);
        self.cwd = next.clone();
        self.filter.clear();
        Some(next)
    }

    pub fn go_parent(&mut self) -> Option<PathBuf> {
        if self.in_trash {
            let home = home_dir();
            self.navigate(home.clone(), false);
            return Some(home);
        }
        let parent = self.cwd.parent()?.to_path_buf();
        if parent == self.cwd {
            return None;
        }
        self.navigate(parent.clone(), false);
        Some(parent)
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.recompute_visible();
    }

    pub fn recompute_visible(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        let mut out: Vec<FileEntry> = self
            .entries
            .iter()
            .filter(|e| {
                if !self.show_hidden && !self.in_trash && e.name.starts_with('.') {
                    return false;
                }
                if !needle.is_empty() && !e.name.to_lowercase().contains(&needle) {
                    return false;
                }
                true
            })
            .cloned()
            .collect();

        let field = self.sort_field;
        let order = self.sort_order;
        let folders_first = self.folders_first;

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
                SortField::Kind => a.kind.cmp(&b.kind),
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

        self.visible = out;
    }

    pub fn pin_bookmark(&mut self, dir: PathBuf) -> bool {
        if !dir.is_dir() || self.bookmarks.contains(&dir) {
            return false;
        }
        self.bookmarks.push(dir);
        save_bookmarks(&self.bookmarks);
        true
    }
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

fn load_bookmarks() -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read_to_string(bookmarks_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<PathBuf>>(&raw).unwrap_or_default()
}

fn save_bookmarks(bookmarks: &[PathBuf]) {
    let path = bookmarks_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(bookmarks) {
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
            kind: if is_dir { "Folder" } else { "File" }.to_string(),
            icon_name: if is_dir { "folder" } else { "text-x-generic" }.to_string(),
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
        assert_eq!(st.cwd, PathBuf::from("/home/u/Music"));
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
            st.visible
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
            st.visible
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha_dir", "zeta.txt", "beta.txt", ".secret"]
        );
    }
}
