//! FreeDesktop.org Trash specification (~/.local/share/Trash/{files,info}).

use std::fs;
use std::path::{Path, PathBuf};

use crate::fs::{now_iso8601_local, ops};
use crate::state::home_dir;

pub fn trash_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("Trash");
    }
    home_dir().join(".local/share/Trash")
}

pub fn move_to_trash(path: &Path) -> Result<PathBuf, String> {
    move_to_trash_in(path, &trash_root())
}

pub fn move_to_trash_in(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let files_dir = root.join("files");
    let info_dir = root.join("info");
    fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&info_dir).map_err(|e| e.to_string())?;

    let orig_name = path
        .file_name()
        .ok_or_else(|| "Invalid path".to_string())?
        .to_string_lossy()
        .into_owned();
    let dst_file = ops::unique_dest(&files_dir.join(&orig_name));
    let trash_name = dst_file
        .file_name()
        .ok_or_else(|| "Invalid trash name".to_string())?
        .to_string_lossy()
        .into_owned();
    let info_path = info_dir.join(format!("{trash_name}.trashinfo"));

    let encoded_orig = percent_encode_path(&path.to_string_lossy());
    let info_body = format!(
        "[Trash Info]\nPath={encoded_orig}\nDeletionDate={}\n",
        now_iso8601_local()
    );
    fs::write(&info_path, info_body).map_err(|e| format!("Write trashinfo failed: {e}"))?;

    if let Err(err) = ops::move_item_simple(path, &dst_file) {
        let _ = fs::remove_file(&info_path);
        return Err(err);
    }
    Ok(dst_file)
}

pub fn restore_from_trash(trashed_file: &Path) -> Result<PathBuf, String> {
    restore_from_trash_in(trashed_file, &trash_root())
}

pub fn restore_from_trash_in(trashed_file: &Path, root: &Path) -> Result<PathBuf, String> {
    let name = trashed_file
        .file_name()
        .ok_or_else(|| "Invalid trashed path".to_string())?
        .to_string_lossy()
        .into_owned();
    let info_path = root.join("info").join(format!("{name}.trashinfo"));
    let orig = fs::read_to_string(&info_path)
        .ok()
        .and_then(|body| parse_trashinfo_path(&body))
        .unwrap_or_else(|| home_dir().join(&name));
    if let Some(parent) = orig.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let dst = ops::unique_dest(&orig);
    ops::move_item_simple(trashed_file, &dst)?;
    let _ = fs::remove_file(&info_path);
    Ok(dst)
}

pub fn empty_trash() -> Result<(), String> {
    empty_trash_in(&trash_root())
}

pub fn empty_trash_in(root: &Path) -> Result<(), String> {
    for sub in ["files", "info"] {
        let dir = root.join(sub);
        if let Ok(rd) = fs::read_dir(&dir) {
            for item in rd.flatten() {
                let p = item.path();
                if p.is_dir() && !p.is_symlink() {
                    fs::remove_dir_all(&p).map_err(|e| e.to_string())?;
                } else {
                    fs::remove_file(&p).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

pub fn trash_count() -> usize {
    fs::read_dir(trash_root().join("files"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0)
}

fn parse_trashinfo_path(body: &str) -> Option<PathBuf> {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("Path=") {
            return Some(PathBuf::from(percent_decode(rest.trim())));
        }
    }
    None
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_roundtrip_preserves_spaces_and_restores() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("my file 1.txt");
        let root = dir.path().join("Trash");
        fs::write(&src, b"hello").unwrap();

        let trashed = move_to_trash_in(&src, &root).unwrap();
        assert!(!src.exists());
        assert!(trashed.exists());

        let restored = restore_from_trash_in(&trashed, &root).unwrap();
        assert_eq!(restored, src);
        assert_eq!(fs::read_to_string(&restored).unwrap(), "hello");
    }
}
