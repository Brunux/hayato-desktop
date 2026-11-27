//! On-disk metadata cache (`$XDG_CACHE_HOME/hayato/software/meta/<pkg>.json`)
//! keyed by `/var/lib/apt/lists` mtime so Software opens quickly with zero daemons.

use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::apt::PackageMeta;

#[derive(Serialize, Deserialize)]
struct CachedMeta {
    lists_stamp: u64,
    meta: PackageMeta,
}

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".cache")
        });
    base.join("hayato/software/meta")
}

/// Modification timestamp (seconds) of `/var/lib/apt/lists`.
pub fn apt_lists_stamp() -> u64 {
    fs::metadata("/var/lib/apt/lists")
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn safe_pkg_filename(pkg: &str) -> String {
    pkg.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '+' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn read(pkg: &str, current_stamp: u64) -> Option<PackageMeta> {
    let path = cache_dir().join(format!("{}.json", safe_pkg_filename(pkg)));
    let text = fs::read_to_string(path).ok()?;
    let entry: CachedMeta = serde_json::from_str(&text).ok()?;
    (entry.lists_stamp == current_stamp).then_some(entry.meta)
}

pub fn write(pkg: &str, current_stamp: u64, meta: &PackageMeta) {
    let dir = cache_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", safe_pkg_filename(pkg)));
    let entry = CachedMeta {
        lists_stamp: current_stamp,
        meta: meta.clone(),
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        let _ = fs::write(path, json);
    }
}
