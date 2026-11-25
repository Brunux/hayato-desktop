//! Async file operations (copy, move, rename, mkdir, delete) with cross-FS detection & progress.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Delete,
    Trash,
}

#[derive(Clone, Debug)]
pub enum OpProgress {
    Update { label: String, fraction: f64 },
    Done { summary: String },
    Error { message: String },
}

pub fn same_filesystem(a: &Path, b_dir: &Path) -> bool {
    let Ok(ma) = fs::symlink_metadata(a) else {
        return false;
    };
    let Ok(mb) = fs::metadata(b_dir) else {
        return false;
    };
    ma.dev() == mb.dev()
}

pub fn unique_dest(target: &Path) -> PathBuf {
    if !target.exists() && fs::symlink_metadata(target).is_err() {
        return target.to_path_buf();
    }
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    let (stem, ext) = match file_name.rfind('.') {
        Some(idx) if idx > 0 => (&file_name[..idx], Some(&file_name[idx..])),
        _ => (file_name.as_str(), None),
    };
    for n in 2..10_000 {
        let candidate = match ext {
            Some(e) => parent.join(format!("{stem} ({n}){e}")),
            None => parent.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() && fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    target.to_path_buf()
}

pub fn move_item_simple(src: &Path, dst: &Path) -> Result<(), String> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_recursive(src, dst, &AtomicBool::new(false))?;
    remove_path(src)
}

pub fn remove_path(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn spawn_batch(
    kind: OpKind,
    sources: Vec<PathBuf>,
    dest_dir: Option<PathBuf>,
    tx: Sender<OpProgress>,
    cancel: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("hayato-filer-ops".into())
        .spawn(move || {
            let total = sources.len().max(1);
            let mut done = 0usize;
            for src in sources {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(OpProgress::Done {
                        summary: format!("Cancelled after {done} item(s)"),
                    });
                    return;
                }
                let name = src
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| src.display().to_string());
                let verb = match kind {
                    OpKind::Copy => "Copying",
                    OpKind::Move => "Moving",
                    OpKind::Delete => "Deleting",
                    OpKind::Trash => "Trashing",
                };
                let _ = tx.send(OpProgress::Update {
                    label: format!("{verb} {name}…"),
                    fraction: done as f64 / total as f64,
                });

                let res = match kind {
                    OpKind::Copy => {
                        let Some(dst_root) = &dest_dir else {
                            continue;
                        };
                        let dst = unique_dest(&dst_root.join(&name));
                        copy_recursive(&src, &dst, &cancel)
                    }
                    OpKind::Move => {
                        let Some(dst_root) = &dest_dir else {
                            continue;
                        };
                        if src.parent() == Some(dst_root.as_path()) {
                            Ok(())
                        } else {
                            let dst = unique_dest(&dst_root.join(&name));
                            if fs::rename(&src, &dst).is_ok() {
                                Ok(())
                            } else {
                                copy_recursive(&src, &dst, &cancel).and_then(|()| remove_path(&src))
                            }
                        }
                    }
                    OpKind::Delete => remove_path(&src),
                    OpKind::Trash => crate::fs::trash::move_to_trash(&src).map(|_| ()),
                };

                if let Err(e) = res {
                    let _ = tx.send(OpProgress::Error {
                        message: format!("{name}: {e}"),
                    });
                    return;
                }
                done += 1;
                let _ = tx.send(OpProgress::Update {
                    label: format!("{verb} ({done}/{total})"),
                    fraction: done as f64 / total as f64,
                });
            }
            let summary = match kind {
                OpKind::Copy => format!("Copied {done} item(s)"),
                OpKind::Move => format!("Moved {done} item(s)"),
                OpKind::Delete => format!("Deleted {done} item(s)"),
                OpKind::Trash => format!("Moved {done} item(s) to Trash"),
            };
            let _ = tx.send(OpProgress::Done { summary });
        });
}

pub fn copy_recursive(src: &Path, dst: &Path, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let meta = fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(target, dst).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if meta.is_dir() {
        // Prevent copying a directory into itself
        if dst.starts_with(src) {
            return Err("Cannot copy a folder into itself".into());
        }
        fs::create_dir_all(dst).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
            let child_dst = dst.join(entry.file_name());
            copy_recursive(&entry.path(), &child_dst, cancel)?;
        }
        return Ok(());
    }
    let mut reader = fs::File::open(src).map_err(|e| e.to_string())?;
    let mut writer = fs::File::create(dst).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = fs::remove_file(dst);
            return Err("Cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    let _ = fs::set_permissions(dst, meta.permissions());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_dest_adds_numeric_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("photo.png");
        fs::write(&a, b"x").unwrap();
        let b = unique_dest(&a);
        assert_eq!(b.file_name().unwrap(), "photo (2).png");
        fs::write(&b, b"y").unwrap();
        let c = unique_dest(&a);
        assert_eq!(c.file_name().unwrap(), "photo (3).png");
    }

    #[test]
    fn recursive_copy_copies_nested_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/a.txt"), b"abc").unwrap();
        let dst = dir.path().join("dst");
        copy_recursive(&src, &dst, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read_to_string(dst.join("sub/a.txt")).unwrap(), "abc");
    }
}
