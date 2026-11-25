//! Filesystem model, directory listing, formatting helpers, and submodules.

pub mod devices;
pub mod ops;
pub mod thumbs;
pub mod trash;

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified_secs: i64,
    pub mode: u32,
    pub kind: String,
    pub icon_glyph: &'static str,
    pub icon_class: &'static str,
}

impl FileEntry {
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let sym_meta = fs::symlink_metadata(path).ok()?;
        let is_symlink = sym_meta.file_type().is_symlink();
        let meta = if is_symlink {
            fs::metadata(path).unwrap_or(sym_meta)
        } else {
            sym_meta
        };
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let modified_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mode = meta.mode();
        let (kind, icon_glyph, icon_class) = classify_entry(&name, is_dir, mode);
        Some(Self {
            path: path.to_path_buf(),
            name,
            is_dir,
            is_symlink,
            size,
            modified_secs,
            mode,
            kind,
            icon_glyph,
            icon_class,
        })
    }

    pub fn is_image(&self) -> bool {
        if self.is_dir {
            return false;
        }
        matches!(
            ext_lower(&self.name).as_deref(),
            Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp")
        )
    }

    pub fn is_audio(&self) -> bool {
        if self.is_dir {
            return false;
        }
        matches!(
            ext_lower(&self.name).as_deref(),
            Some("mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac")
        )
    }

    pub fn is_text_previewable(&self) -> bool {
        if self.is_dir {
            return false;
        }
        matches!(
            ext_lower(&self.name).as_deref(),
            Some(
                "txt"
                    | "md"
                    | "rs"
                    | "py"
                    | "sh"
                    | "toml"
                    | "json"
                    | "jsonc"
                    | "yaml"
                    | "yml"
                    | "css"
                    | "html"
                    | "js"
                    | "ts"
                    | "c"
                    | "h"
                    | "cpp"
                    | "conf"
                    | "ini"
                    | "log"
                    | "csv"
                    | "xml"
                    | "desktop"
            )
        )
    }
}

pub fn list_dir(dir: &Path) -> Result<Vec<FileEntry>, String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("Cannot open {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for item in rd.flatten() {
        if let Some(entry) = FileEntry::from_path(&item.path()) {
            out.push(entry);
        }
    }
    Ok(out)
}

pub fn ext_lower(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn classify_entry(name: &str, is_dir: bool, mode: u32) -> (String, &'static str, &'static str) {
    if is_dir {
        return ("Folder".to_string(), "\u{f07b}", "icon-folder");
    }
    match ext_lower(name).as_deref() {
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp") => {
            ("Image".to_string(), "\u{f1c5}", "icon-image")
        }
        Some("mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac") => {
            ("Audio".to_string(), "\u{f1c7}", "icon-audio")
        }
        Some("mp4" | "mkv" | "webm" | "mov" | "avi") => {
            ("Video".to_string(), "\u{f1c8}", "icon-video")
        }
        Some("pdf") => ("PDF".to_string(), "\u{f1c1}", "icon-pdf"),
        Some("zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" | "bz2") => {
            ("Archive".to_string(), "\u{f1c6}", "icon-archive")
        }
        Some(
            "rs" | "py" | "sh" | "js" | "ts" | "c" | "h" | "cpp" | "toml" | "json" | "jsonc"
            | "yaml" | "yml" | "css" | "html" | "xml",
        ) => ("Code".to_string(), "\u{f1c9}", "icon-code"),
        Some("md" | "txt" | "log" | "csv" | "conf" | "ini" | "desktop") => {
            ("Text".to_string(), "\u{f15c}", "icon-file")
        }
        Some("deb" | "rpm" | "appimage") => ("Package".to_string(), "\u{f1c6}", "icon-archive"),
        Some(ext) => {
            if mode & 0o111 != 0 {
                ("App".to_string(), "\u{f013}", "icon-exec")
            } else {
                (ext.to_ascii_uppercase(), "\u{f15b}", "icon-file")
            }
        }
        None => {
            if mode & 0o111 != 0 {
                ("App".to_string(), "\u{f013}", "icon-exec")
            } else {
                ("File".to_string(), "\u{f15b}", "icon-file")
            }
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx + 1 < UNITS.len() {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else if val >= 100.0 {
        format!("{val:.0} {}", UNITS[idx])
    } else {
        format!("{val:.1} {}", UNITS[idx])
    }
}

pub fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

pub fn format_mtime(secs: i64) -> String {
    if secs <= 0 {
        return "—".to_string();
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn free_space_bytes(path: &Path) -> Option<u64> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc == 0 {
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    } else {
        None
    }
}

pub fn fs_usage_bytes(path: &Path) -> Option<(u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc == 0 {
        let total = (st.f_blocks as u64).saturating_mul(st.f_frsize as u64);
        let avail = (st.f_bavail as u64).saturating_mul(st.f_frsize as u64);
        Some((total.saturating_sub(avail), total))
    } else {
        None
    }
}

pub fn now_iso8601_local() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_modes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o644), "rw-r--r--");
    }
}
