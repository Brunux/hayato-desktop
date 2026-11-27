//! On-demand local folder diff & sync for Hayato Connect (`[2] Transfer`).
//!
//! Zero `inotify` daemons: manifests are scanned explicitly only when opened
//! or requested, constrained strictly within the chosen folder via path
//! canonicalization and traversal checks.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

const MAX_MANIFEST_FILES: usize = 2000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub rel_path: String,
    pub size: u64,
    pub mtime_s: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncDiffPlan {
    pub push_to_phone: Vec<FileEntry>,
    pub pull_from_phone: Vec<String>,
    pub up_to_date: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncConfig {
    folder: String,
}

pub fn default_sync_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("hayato/connect/sync.json")
}

pub fn default_sync_folder() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join("Documents")
        })
        .join("Notes")
}

pub fn load_sync_folder(config_path: &Path) -> PathBuf {
    if let Ok(raw) = fs::read_to_string(config_path)
        && let Ok(cfg) = serde_json::from_str::<SyncConfig>(&raw)
        && !cfg.folder.trim().is_empty()
    {
        return PathBuf::from(cfg.folder);
    }
    default_sync_folder()
}

pub fn save_sync_folder(config_path: &Path, folder: &Path) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let body = serde_json::to_string_pretty(&SyncConfig {
        folder: folder.display().to_string(),
    })
    .map_err(|e| format!("encode sync config: {e}"))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(config_path)
        .map_err(|e| format!("write sync config: {e}"))?;
    use std::io::Write;
    f.write_all(body.as_bytes())
        .map_err(|e| format!("write sync config: {e}"))
}

/// Resolve `rel` strictly inside `root`, rejecting absolute components, `..`,
/// NUL bytes, or symlinks pointing outside `root`.
pub fn safe_rel_path(root: &Path, rel: &str, must_exist: bool) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') {
        return None;
    }
    let clean = rel.replace('\\', "/");
    let rel_p = Path::new(&clean);
    let mut normalized = PathBuf::new();
    for comp in rel_p.components() {
        match comp {
            Component::Normal(c) => {
                let s = c.to_string_lossy();
                if s.starts_with('.') {
                    return None;
                }
                normalized.push(c);
            }
            _ => return None,
        }
    }
    if normalized.as_os_str().is_empty() {
        return None;
    }
    let canon_root = fs::canonicalize(root).ok()?;
    let target = canon_root.join(&normalized);
    if must_exist {
        let canon_target = fs::canonicalize(&target).ok()?;
        if !canon_target.starts_with(&canon_root) || !canon_target.is_file() {
            return None;
        }
        return Some(canon_target);
    }
    if let Some(parent) = target.parent() {
        if parent.exists() {
            let canon_parent = fs::canonicalize(parent).ok()?;
            if !canon_parent.starts_with(&canon_root) {
                return None;
            }
        }
    }
    Some(target)
}

/// Walk `root` on demand and return a deterministic file manifest.
pub fn build_manifest(root: &Path) -> Result<Vec<FileEntry>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let canon_root = fs::canonicalize(root).map_err(|e| format!("open sync root: {e}"))?;
    let mut out = Vec::new();
    collect_dir(&canon_root, &canon_root, &mut out);
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn collect_dir(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) {
    if out.len() >= MAX_MANIFEST_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_MANIFEST_FILES {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_dir(root, &entry.path(), out);
        } else if meta.is_file()
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            let mtime_s = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push(FileEntry {
                rel_path: rel.to_string_lossy().replace('\\', "/"),
                size: meta.len(),
                mtime_s,
            });
        }
    }
}

/// Compute a conservative two-way diff plan (2-second mtime tolerance).
pub fn compute_diff_plan(local: &[FileEntry], phone: &[FileEntry]) -> SyncDiffPlan {
    let mut phone_map: HashMap<&str, &FileEntry> = HashMap::with_capacity(phone.len());
    for p in phone {
        phone_map.insert(p.rel_path.as_str(), p);
    }
    let mut local_map: HashMap<&str, &FileEntry> = HashMap::with_capacity(local.len());
    let mut plan = SyncDiffPlan::default();

    for l in local {
        local_map.insert(l.rel_path.as_str(), l);
        match phone_map.get(l.rel_path.as_str()) {
            None => plan.push_to_phone.push(l.clone()),
            Some(p) => {
                if l.size == p.size && (l.mtime_s - p.mtime_s).abs() <= 2 {
                    plan.up_to_date += 1;
                } else if l.mtime_s > p.mtime_s + 2 {
                    plan.push_to_phone.push(l.clone());
                } else if p.mtime_s > l.mtime_s + 2 {
                    plan.pull_from_phone.push(p.rel_path.clone());
                } else if l.size != p.size {
                    plan.push_to_phone.push(l.clone());
                } else {
                    plan.up_to_date += 1;
                }
            }
        }
    }

    for p in phone {
        if !local_map.contains_key(p.rel_path.as_str()) {
            plan.pull_from_phone.push(p.rel_path.clone());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_guards_and_diff_plan() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("hello.md");
        fs::write(&notes, "hi").unwrap();

        assert!(safe_rel_path(dir.path(), "../etc/passwd", false).is_none());
        assert!(safe_rel_path(dir.path(), "/etc/passwd", false).is_none());
        assert!(safe_rel_path(dir.path(), ".secret/key", false).is_none());
        assert!(safe_rel_path(dir.path(), "hello.md", true).is_some());
        assert!(safe_rel_path(dir.path(), "sub/new.md", false).is_some());

        let local = build_manifest(dir.path()).unwrap();
        assert_eq!(local.len(), 1);
        let phone = vec![FileEntry {
            rel_path: "from_phone.txt".into(),
            size: 10,
            mtime_s: local[0].mtime_s,
        }];
        let plan = compute_diff_plan(&local, &phone);
        assert_eq!(plan.push_to_phone.len(), 1);
        assert_eq!(plan.pull_from_phone, vec!["from_phone.txt"]);
    }
}
