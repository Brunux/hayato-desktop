//! Lightweight on-disk thumbnail cache (~/.cache/hayato/filer-thumbs) for images.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::state::home_dir;

pub fn cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("hayato/filer-thumbs");
    }
    home_dir().join(".cache/hayato/filer-thumbs")
}

/// Return path to a 128x128 PNG thumbnail for `image_path`, creating it if missing.
pub fn ensure_thumbnail(image_path: &Path, mtime: i64) -> Option<PathBuf> {
    let ext = image_path
        .extension()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
        return None;
    }
    let dir = cache_dir();
    let mut hasher = Sha256::new();
    hasher.update(image_path.to_string_lossy().as_bytes());
    hasher.update(mtime.to_le_bytes());
    let key = format!("{:x}", hasher.finalize());
    let out = dir.join(format!("{}.png", &key[..24]));
    if out.is_file() {
        return Some(out);
    }
    // Skip very large files (>24 MB) to protect 4 GiB Chromebook RAM
    if fs::metadata(image_path).ok()?.len() > 24 * 1024 * 1024 {
        return None;
    }
    let img = image::open(image_path).ok()?;
    let thumb = img.thumbnail(128, 128);
    let _ = fs::create_dir_all(&dir);
    thumb.save(&out).ok()?;
    Some(out)
}
