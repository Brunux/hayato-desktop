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
    Update {
        label: String,
        fraction: f64,
    },
    Done {
        summary: String,
        /// How to reverse this operation, when it can be reversed. Finder's
        /// Ctrl+Z: a rename goes back, a move comes home, a copy is removed,
        /// a trashed file is restored. A permanent delete carries `None`.
        undo: Option<UndoAction>,
    },
    Error {
        message: String,
    },
}

/// A completed, reversible operation. The window keeps a stack of these; the
/// paths are the ones actually used, so a collision suffix (`photo (2).png`)
/// is undone exactly as it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// Rename `to` back to `from`.
    Rename { from: PathBuf, to: PathBuf },
    /// Paths this operation created (a copy, a new folder): remove them.
    Created(Vec<PathBuf>),
    /// Move each `to` back to `from` (the pair is `(original, landed)`).
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Trashed paths; restoring reads each `.trashinfo`.
    Trashed(Vec<PathBuf>),
}

impl UndoAction {
    /// Short verb for the banner ("Undid move of 3 item(s)").
    pub fn label(&self) -> String {
        match self {
            UndoAction::Rename { to, .. } => {
                let name = to
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("rename of {name}")
            }
            UndoAction::Created(paths) => format!("creation of {} item(s)", paths.len()),
            UndoAction::Moved(pairs) => format!("move of {} item(s)", pairs.len()),
            UndoAction::Trashed(paths) => format!("trash of {} item(s)", paths.len()),
        }
    }
}

/// Reverse a completed operation. Best-effort across a batch: it stops at the
/// first failure and reports what could not be put back.
pub fn undo(action: &UndoAction) -> Result<String, String> {
    match action {
        UndoAction::Rename { from, to } => {
            if !to.exists() {
                return Err(format!("{} is no longer there", to.display()));
            }
            if from.exists() {
                return Err(format!("{} already exists again", from.display()));
            }
            fs::rename(to, from).map_err(|e| e.to_string())?;
            Ok(format!("Undid rename to {}", display_name(from)))
        }
        UndoAction::Created(paths) => {
            for p in paths {
                if p.exists() || fs::symlink_metadata(p).is_ok() {
                    remove_path(p)?;
                }
            }
            Ok(format!("Undid creation of {} item(s)", paths.len()))
        }
        UndoAction::Moved(pairs) => {
            let mut done = 0usize;
            for (from, to) in pairs {
                if !to.exists() && fs::symlink_metadata(to).is_err() {
                    continue;
                }
                if let Some(parent) = from.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let target = unique_dest(from);
                move_item_simple(to, &target)?;
                done += 1;
            }
            Ok(format!("Undid move of {done} item(s)"))
        }
        UndoAction::Trashed(paths) => {
            let mut done = 0usize;
            for p in paths {
                if crate::fs::trash::restore_from_trash(p).is_ok() {
                    done += 1;
                }
            }
            if done == 0 {
                return Err("Nothing could be restored from Trash".into());
            }
            Ok(format!("Restored {done} item(s) from Trash"))
        }
    }
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
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
            let mut created: Vec<PathBuf> = Vec::new();
            let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut trashed: Vec<PathBuf> = Vec::new();
            for src in sources {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(OpProgress::Done {
                        summary: format!("Cancelled after {done} item(s)"),
                        undo: None,
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
                        copy_recursive(&src, &dst, &cancel).inspect(|_| created.push(dst))
                    }
                    OpKind::Move => {
                        let Some(dst_root) = &dest_dir else {
                            continue;
                        };
                        if src.parent() == Some(dst_root.as_path()) {
                            Ok(())
                        } else {
                            let dst = unique_dest(&dst_root.join(&name));
                            let moved_res = if fs::rename(&src, &dst).is_ok() {
                                Ok(())
                            } else {
                                copy_recursive(&src, &dst, &cancel).and_then(|()| remove_path(&src))
                            };
                            moved_res.inspect(|_| moved.push((src.clone(), dst)))
                        }
                    }
                    OpKind::Delete => remove_path(&src),
                    OpKind::Trash => crate::fs::trash::move_to_trash(&src)
                        .inspect(|dst| trashed.push(dst.clone()))
                        .map(|_| ()),
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
            let (summary, undo) = match kind {
                OpKind::Copy => (
                    format!("Copied {done} item(s)"),
                    (!created.is_empty()).then_some(UndoAction::Created(created)),
                ),
                OpKind::Move => (
                    format!("Moved {done} item(s)"),
                    (!moved.is_empty()).then_some(UndoAction::Moved(moved)),
                ),
                OpKind::Delete => (format!("Deleted {done} item(s)"), None),
                OpKind::Trash => (
                    format!("Moved {done} item(s) to Trash"),
                    (!trashed.is_empty()).then_some(UndoAction::Trashed(trashed)),
                ),
            };
            let _ = tx.send(OpProgress::Done { summary, undo });
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
    fn undo_rename_moves_the_name_back() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        fs::write(&to, b"x").unwrap();
        let action = UndoAction::Rename {
            from: from.clone(),
            to: to.clone(),
        };
        assert!(undo(&action).is_ok());
        assert!(from.exists());
        assert!(!to.exists());
    }

    #[test]
    fn undo_created_removes_the_copies() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("one.txt");
        let b = dir.path().join("two.txt");
        fs::write(&a, b"1").unwrap();
        fs::write(&b, b"2").unwrap();
        let action = UndoAction::Created(vec![a.clone(), b.clone()]);
        assert!(undo(&action).is_ok());
        assert!(!a.exists());
        assert!(!b.exists());
    }

    #[test]
    fn undo_moved_brings_files_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let away = dir.path().join("away");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&away).unwrap();
        let original = home.join("note.md");
        let landed = away.join("note.md");
        fs::write(&landed, b"hi").unwrap();
        let action = UndoAction::Moved(vec![(original.clone(), landed.clone())]);
        assert!(undo(&action).is_ok());
        assert_eq!(fs::read_to_string(&original).unwrap(), "hi");
        assert!(!landed.exists());
    }

    #[test]
    fn undo_labels_name_the_action() {
        let action = UndoAction::Rename {
            from: PathBuf::from("/a/old.txt"),
            to: PathBuf::from("/a/new.txt"),
        };
        assert_eq!(action.label(), "rename of new.txt");
        assert_eq!(
            UndoAction::Trashed(vec![PathBuf::from("/t/x")]).label(),
            "trash of 1 item(s)"
        );
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
