//! One-click archive extraction and creation, on the system `tar`/`zip`.
//!
//! Nothing is linked in for this: the image already ships `tar`, `xz`, `zip`
//! and `unzip`, so a right-click is one child process and no extra RAM. Both
//! directions refuse to clobber - extraction keeps existing files, and a new
//! archive lands under a free name from `ops::unique_dest`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use crate::fs::ops::{self, OpProgress, UndoAction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    TarXz,
}

impl ArchiveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
        }
    }
}

/// The archive format a path names, by extension (case-insensitive).
pub fn format_of(path: &Path) -> Option<ArchiveFormat> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Some(ArchiveFormat::TarXz)
    } else if name.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else {
        None
    }
}

pub fn is_archive(path: &Path) -> bool {
    format_of(path).is_some()
}

/// The base name a new archive takes: the folder's own name, or a file's name
/// without its extension; several files become `Archive`.
pub fn archive_stem(sources: &[PathBuf]) -> String {
    if sources.len() != 1 {
        return "Archive".to_string();
    }
    let p = &sources[0];
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Archive".to_string());
    if p.is_dir() {
        return name;
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name,
    }
}

/// Extract an archive beside itself, never overwriting what is already there.
pub fn extract_here(archive: &Path) -> Result<String, String> {
    let format = format_of(archive)
        .ok_or_else(|| format!("{} is not an archive", display(archive)))?;
    let dir = archive
        .parent()
        .ok_or_else(|| "Archive has no folder".to_string())?;
    let mut cmd = match format {
        ArchiveFormat::Zip => {
            let mut c = Command::new("unzip");
            c.arg("-n").arg("-q").arg(archive).arg("-d").arg(dir);
            c
        }
        ArchiveFormat::TarGz => {
            let mut c = Command::new("tar");
            c.arg("-x").arg("--skip-old-files").arg("-z").arg("-f").arg(archive).arg("-C").arg(dir);
            c
        }
        ArchiveFormat::TarXz => {
            let mut c = Command::new("tar");
            c.arg("-x").arg("--skip-old-files").arg("-J").arg("-f").arg(archive).arg("-C").arg(dir);
            c
        }
    };
    run(&mut cmd)?;
    Ok(format!("Extracted {}", display(archive)))
}

/// Create an archive of `sources` inside `dest_dir`; returns the new path.
pub fn compress(
    sources: &[PathBuf],
    format: ArchiveFormat,
    dest_dir: &Path,
) -> Result<PathBuf, String> {
    if sources.is_empty() {
        return Err("Nothing selected".to_string());
    }
    let base = sources[0]
        .parent()
        .ok_or_else(|| "Selected item has no folder".to_string())?;
    let stem = archive_stem(sources);
    let dest = ops::unique_dest(&dest_dir.join(format!("{stem}.{}", format.extension())));
    let names: Vec<OsString> = sources
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_os_string()))
        .collect();
    if names.is_empty() {
        return Err("Selected items have no names".to_string());
    }

    let mut cmd = match format {
        ArchiveFormat::Zip => {
            let mut c = Command::new("zip");
            c.arg("-r").arg("-q").arg(&dest);
            c
        }
        ArchiveFormat::TarGz => {
            let mut c = Command::new("tar");
            c.arg("-c").arg("-z").arg("-f").arg(&dest);
            c
        }
        ArchiveFormat::TarXz => {
            let mut c = Command::new("tar");
            c.arg("-c").arg("-J").arg("-f").arg(&dest);
            c
        }
    };
    cmd.args(&names).current_dir(base);
    if let Err(e) = run(&mut cmd) {
        // A half-written archive is worse than none.
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(dest)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{} could not start: {e}", cmd.get_program().to_string_lossy()))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let msg = err.lines().last().unwrap_or("").trim();
    Err(if msg.is_empty() {
        format!("{} failed", cmd.get_program().to_string_lossy())
    } else {
        msg.to_string()
    })
}

fn display(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Extract each archive on a worker thread, reporting through the shared
/// progress channel (no undo: the extracted tree is the answer).
pub fn spawn_extract(archives: Vec<PathBuf>, tx: Sender<OpProgress>, cancel: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new()
        .name("hayato-filer-archive".into())
        .spawn(move || {
            let total = archives.len().max(1);
            let mut done = 0usize;
            for archive in archives {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(OpProgress::Done {
                        summary: format!("Cancelled after {done} archive(s)"),
                        undo: None,
                    });
                    return;
                }
                let _ = tx.send(OpProgress::Update {
                    label: format!("Extracting {}…", display(&archive)),
                    fraction: done as f64 / total as f64,
                });
                if let Err(e) = extract_here(&archive) {
                    let _ = tx.send(OpProgress::Error {
                        message: format!("{}: {e}", display(&archive)),
                    });
                    return;
                }
                done += 1;
            }
            let _ = tx.send(OpProgress::Done {
                summary: format!("Extracted {done} archive(s)"),
                undo: None,
            });
        });
}

/// Create one archive from the selection; the new file can be undone.
pub fn spawn_compress(
    sources: Vec<PathBuf>,
    format: ArchiveFormat,
    dest_dir: PathBuf,
    tx: Sender<OpProgress>,
    cancel: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("hayato-filer-archive".into())
        .spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                let _ = tx.send(OpProgress::Done {
                    summary: "Cancelled".to_string(),
                    undo: None,
                });
                return;
            }
            let _ = tx.send(OpProgress::Update {
                label: format!("Compressing to .{}…", format.extension()),
                fraction: 0.4,
            });
            match compress(&sources, format, &dest_dir) {
                Ok(dest) => {
                    let name = display(&dest);
                    let _ = tx.send(OpProgress::Done {
                        summary: format!("Created {name}"),
                        undo: Some(UndoAction::Created(vec![dest])),
                    });
                }
                Err(e) => {
                    let _ = tx.send(OpProgress::Error { message: e });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats_by_extension_case_insensitively() {
        assert_eq!(format_of(Path::new("/a/notes.zip")), Some(ArchiveFormat::Zip));
        assert_eq!(format_of(Path::new("/a/NOTES.ZIP")), Some(ArchiveFormat::Zip));
        assert_eq!(format_of(Path::new("/a/pack.tar.gz")), Some(ArchiveFormat::TarGz));
        assert_eq!(format_of(Path::new("/a/pack.tgz")), Some(ArchiveFormat::TarGz));
        assert_eq!(format_of(Path::new("/a/pack.tar.xz")), Some(ArchiveFormat::TarXz));
        assert_eq!(format_of(Path::new("/a/pack.txz")), Some(ArchiveFormat::TarXz));
        assert_eq!(format_of(Path::new("/a/photo.png")), None);
        assert_eq!(format_of(Path::new("/a/plain")), None);
    }

    #[test]
    fn archive_stem_names_a_folder_or_a_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Holiday");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(archive_stem(&[folder]), "Holiday");

        let file = dir.path().join("report.tar.gz");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(archive_stem(&[file]), "report.tar");

        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        assert_eq!(archive_stem(&[a, b]), "Archive");
    }

    #[test]
    fn tar_gz_round_trips_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("project");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/note.md"), b"hello").unwrap();

        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let archive = compress(
            std::slice::from_ref(&src),
            ArchiveFormat::TarGz,
            &dest_dir,
        )
        .unwrap();
        assert_eq!(archive.file_name().unwrap(), "project.tar.gz");
        assert!(archive.exists());

        // Extract into a fresh folder and confirm the tree survived.
        let extract_dir = dir.path().join("restore");
        std::fs::create_dir_all(&extract_dir).unwrap();
        let moved = extract_dir.join("project.tar.gz");
        std::fs::rename(&archive, &moved).unwrap();
        extract_here(&moved).unwrap();
        assert_eq!(
            std::fs::read_to_string(extract_dir.join("project/sub/note.md")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn a_second_archive_gets_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, b"x").unwrap();
        let first = compress(std::slice::from_ref(&src), ArchiveFormat::Zip, dir.path()).unwrap();
        let second = compress(std::slice::from_ref(&src), ArchiveFormat::Zip, dir.path()).unwrap();
        assert_eq!(first.file_name().unwrap(), "notes.zip");
        assert_eq!(second.file_name().unwrap(), "notes (2).zip");
    }
}
