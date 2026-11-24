//! Contextual File Actions (`hayato-file-actions`, `hayato menu file-actions`).
//!
//! Secondary actions for the file under the cursor: disk usage, compress,
//! extract, mount disk image, convert media, copy path, terminal.

use std::path::Path;
use std::process::Command;

use hayato_ui::task::{Task, TaskView};
use hayato_ui::{Card, Design, Glyph, Urgency};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::out;
use super::pick::{Picker, Question, row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Directory,
    ArchiveZip,
    ArchiveTar,
    DiskImage,
    Video,
    Audio,
    Pdf,
    Epub,
    Ink,
    Text,
    Other,
}

pub fn detect_kind(path: &Path) -> FileKind {
    if path.is_dir() {
        return FileKind::Directory;
    }
    let lower = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if lower.ends_with(".zip") {
        FileKind::ArchiveZip
    } else if lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.zst")
        || lower.ends_with(".tgz")
        || lower.ends_with(".txz")
        || lower.ends_with(".tbz")
        || lower.ends_with(".tbz2")
        || lower.ends_with(".tzst")
    {
        FileKind::ArchiveTar
    } else if lower.ends_with(".iso") || lower.ends_with(".img") {
        FileKind::DiskImage
    } else if lower.ends_with(".mp4")
        || lower.ends_with(".mkv")
        || lower.ends_with(".webm")
        || lower.ends_with(".mov")
        || lower.ends_with(".avi")
        || lower.ends_with(".m4v")
    {
        FileKind::Video
    } else if lower.ends_with(".mp3")
        || lower.ends_with(".flac")
        || lower.ends_with(".ogg")
        || lower.ends_with(".wav")
        || lower.ends_with(".m4a")
        || lower.ends_with(".aac")
    {
        FileKind::Audio
    } else if lower.ends_with(".pdf") {
        FileKind::Pdf
    } else if lower.ends_with(".epub") {
        FileKind::Epub
    } else if lower.ends_with(".xopp") {
        FileKind::Ink
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt") {
        FileKind::Text
    } else {
        FileKind::Other
    }
}

/// The question of `hayato menu file-actions`, built from data so a snapshot
/// can draw it without a file on disk.
pub fn questions(
    base: &str,
    target: &Path,
    parent: &Path,
    kind: FileKind,
    cached_du: Option<&str>,
    note: &str,
) -> Vec<Question> {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    if kind == FileKind::Directory {
        let du_detail = cached_du
            .as_ref()
            .map(|s| format!("calculated size: {s}"))
            .unwrap_or_else(|| "du -sh: calculate directory size".to_string());
        let mut r = row("Disk usage", du_detail, Glyph::Disk);
        if let Some(s) = cached_du {
            r = r.value(s.to_string());
        }
        rows.push(r);
        keys.push("du".to_string());
    }

    rows.push(row("Compress", format!("→ {base}.tar.xz"), Glyph::Box));
    keys.push("pack".to_string());

    if kind != FileKind::Directory {
        rows.push(row(
            "Share nearby (Hayato Drop)",
            "send over Wi-Fi via scannable QR code",
            Glyph::Qr,
        ));
        keys.push("drop".to_string());
    }

    if matches!(kind, FileKind::ArchiveZip | FileKind::ArchiveTar) {
        rows.push(row(
            "Extract here",
            "→ folder named after archive",
            Glyph::Box,
        ));
        keys.push("unpack".to_string());
    }

    if kind == FileKind::DiskImage {
        rows.push(row(
            "Mount the image",
            "udisksctl loop-setup then open",
            Glyph::Disk,
        ));
        keys.push("mount".to_string());
    }

    if matches!(kind, FileKind::Video | FileKind::Audio) {
        rows.push(row(
            "Convert",
            "ffmpeg: MP4 video or MP3 audio",
            Glyph::Video,
        ));
        keys.push("convert".to_string());
    }

    match kind {
        FileKind::Pdf => {
            rows.push(row(
                "Annotate or sign (xournalpp)",
                "export the signed copy beside the original",
                Glyph::Brush,
            ));
            keys.push("annotate".to_string());
        }
        FileKind::Epub => {
            rows.push(row("Read (foliate)", "EPUB reader", Glyph::Book));
            keys.push("read-epub".to_string());
        }
        FileKind::Ink => {
            rows.push(row("Open in xournalpp", "ink note", Glyph::Brush));
            keys.push("open-ink".to_string());
            rows.push(row(
                "Export to PDF (xournalpp)",
                "writes <name>-signed.pdf beside it",
                Glyph::Download,
            ));
            keys.push("export-ink".to_string());
        }
        FileKind::Text => {
            rows.push(row(
                "Edit (nvim)",
                "opens in a foot window",
                Glyph::Terminal,
            ));
            keys.push("edit-text".to_string());
        }
        _ => {}
    }

    rows.push(row(
        "Copy the path",
        target.display().to_string(),
        Glyph::Clipboard,
    ));
    keys.push("copy".to_string());

    rows.push(row(
        "A terminal here",
        parent.display().to_string(),
        Glyph::Terminal,
    ));
    keys.push("term".to_string());

    let title = format!("File actions · {base}");
    vec![Question::choose(title, note, rows, keys)]
}

/// Interactive `hayato menu file-actions <path>`.
pub fn menu_for_path(
    path_str: &str,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let target = Path::new(path_str);
    if !target.exists() && !target.is_symlink() {
        return Err(crate::Failure::new(
            "File actions",
            format!("{} is not there any more", target.display()),
        ));
    }

    let base = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| target.display().to_string());
    let parent = target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let kind = detect_kind(target);
    let mut cached_du: Option<String> = None;
    let mut status_note: Option<String> = None;

    loop {
        let note = status_note
            .clone()
            .unwrap_or_else(|| target.display().to_string());

        let mut picker = Picker::new(
            &format!("File actions · {base}"),
            questions(&base, target, &parent, kind, cached_du.as_deref(), &note),
        );

        let code = crate::run_screen(
            terminal,
            design,
            &format!("File actions · {base}"),
            &mut picker,
        )?;
        if code != 0 {
            return Ok(code);
        }

        let answers = picker.answers();
        let Some(action) = answers.first() else {
            return Ok(1);
        };

        match action.as_str() {
            "drop" => {
                return super::drop::menu(
                    &[target.to_string_lossy().to_string()],
                    terminal,
                    design,
                );
            }
            "copy" => {
                let _ = Command::new("wl-copy")
                    .arg(target.to_str().unwrap_or(""))
                    .status();
                status_note = Some(format!("Copied path to clipboard: {}", target.display()));
            }
            "term" => {
                let dir = if kind == FileKind::Directory {
                    target
                } else {
                    &parent
                };
                let dir_str = dir.to_string_lossy();
                crate::system::spawn_detached_argv(
                    "foot",
                    &[&format!("--working-directory={dir_str}")],
                );
                return Ok(0);
            }
            "du" => {
                let out_str =
                    out(&["du", "-sh", target.to_str().unwrap_or("")]).unwrap_or_default();
                let size = out_str.split_whitespace().next().unwrap_or("?").to_string();
                cached_du = Some(size.clone());
                status_note = Some(format!("Disk usage for {base}: {size}"));
            }
            "pack" => {
                let out_archive = parent.join(format!("{base}.tar.xz"));
                let cmd = vec![
                    "tar".to_string(),
                    "-caf".to_string(),
                    out_archive.to_string_lossy().to_string(),
                    "-C".to_string(),
                    parent.to_string_lossy().to_string(),
                    "--".to_string(),
                    base.clone(),
                ];
                let title = format!("Compress {base}");
                let tag = format!("pack-{}", std::process::id());
                let task = Task::start_steps(
                    &tag,
                    title.clone(),
                    vec![format!("Pack {base}.tar.xz")],
                    true,
                    vec![cmd],
                )
                .map_err(|e| {
                    crate::Failure::new("Compress", format!("Could not start tar: {e}"))
                })?;
                let mut view = TaskView::new(task);
                return crate::run_screen(terminal, design, &title, &mut view);
            }
            "unpack" => {
                let stem = archive_stem(&base);
                let dest = parent.join(stem);
                let _ = std::fs::create_dir_all(&dest);
                let cmd = if kind == FileKind::ArchiveZip {
                    vec![
                        "unzip".to_string(),
                        "-o".to_string(),
                        target.to_string_lossy().to_string(),
                        "-d".to_string(),
                        dest.to_string_lossy().to_string(),
                    ]
                } else {
                    vec![
                        "tar".to_string(),
                        "-xvf".to_string(),
                        target.to_string_lossy().to_string(),
                        "-C".to_string(),
                        dest.to_string_lossy().to_string(),
                    ]
                };
                let title = format!("Extract {base}");
                let tag = format!("unpack-{}", std::process::id());
                let task = Task::start_steps(
                    &tag,
                    title.clone(),
                    vec![format!("Unpack into {stem}/")],
                    true,
                    vec![cmd],
                )
                .map_err(|e| {
                    crate::Failure::new("Extract", format!("Could not start extractor: {e}"))
                })?;
                let mut view = TaskView::new(task);
                return crate::run_screen(terminal, design, &title, &mut view);
            }
            "mount" => {
                let res = Command::new("udisksctl")
                    .args(["loop-setup", "-f", target.to_str().unwrap_or("")])
                    .output();
                if let Ok(o) = res {
                    let text = String::from_utf8_lossy(&o.stdout);
                    if let Some(pos) = text.find("/dev/loop") {
                        let loop_dev = text[pos..].split_whitespace().next().unwrap_or("");
                        let mount_res = Command::new("udisksctl")
                            .args(["mount", "-b", loop_dev])
                            .output();
                        if let Ok(mo) = mount_res {
                            let mtext = String::from_utf8_lossy(&mo.stdout);
                            if let Some(at) = mtext.rfind(" at ") {
                                let mp = mtext[at + 4..].trim().trim_end_matches('.');
                                let _ = Command::new("hayato").args(["menu", "files", mp]).status();
                                return Ok(0);
                            }
                        }
                    }
                }
                status_note = Some(format!("Could not mount disk image {base}"));
            }
            "annotate" => {
                if crate::system::have("xournalpp") {
                    crate::system::doc_tools::spawn("xournalpp", &[&target.display().to_string()]);
                } else {
                    crate::system::say(
                        "Annotate or sign",
                        "xournalpp is not installed (apt install xournalpp)",
                        Urgency::Critical,
                        Glyph::Warning,
                    );
                }
                return Ok(0);
            }
            "read-epub" => {
                if crate::system::have("foliate") {
                    crate::system::doc_tools::spawn("foliate", &[&target.display().to_string()]);
                } else {
                    crate::system::say(
                        "Read an e-book",
                        "foliate is not installed (apt install foliate)",
                        Urgency::Critical,
                        Glyph::Warning,
                    );
                }
                return Ok(0);
            }
            "open-ink" => {
                if crate::system::have("xournalpp") {
                    crate::system::doc_tools::spawn("xournalpp", &[&target.display().to_string()]);
                } else {
                    crate::system::say(
                        "Open an ink note",
                        "xournalpp is not installed (apt install xournalpp)",
                        Urgency::Critical,
                        Glyph::Warning,
                    );
                }
                return Ok(0);
            }
            "export-ink" => {
                let stem = target
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| base.clone());
                let out = parent.join(format!("{stem}-signed.pdf"));
                if crate::system::have("xournalpp") {
                    match Command::new("xournalpp")
                        .arg(format!("--create-pdf={}", out.display()))
                        .arg(&target)
                        .status()
                    {
                        Ok(status) if status.success() && out.exists() => {
                            crate::system::say_open(
                                "Signed PDF",
                                format!("Exported {}", out.display()),
                                Urgency::Normal,
                                Glyph::Pdf,
                                &out,
                            );
                        }
                        Ok(status) => {
                            status_note = Some(format!(
                                "xournalpp exited {status} without writing {}",
                                out.display()
                            ));
                        }
                        Err(error) => {
                            status_note = Some(format!("Cannot run xournalpp: {error}"));
                        }
                    }
                } else {
                    status_note =
                        Some("xournalpp is not installed (apt install xournalpp)".to_string());
                }
            }
            "edit-text" => {
                let editor = std::env::var("EDITOR")
                    .ok()
                    .and_then(|value| value.split_whitespace().next().map(str::to_string))
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        ["nvim", "vi"]
                            .into_iter()
                            .find(|program| crate::system::have(program))
                            .map(str::to_string)
                    });
                match editor {
                    Some(editor) => {
                        crate::system::spawn_detached_argv(
                            "foot",
                            &[
                                "--app-id=popup",
                                "-e",
                                &editor,
                                &target.display().to_string(),
                            ],
                        );
                        return Ok(0);
                    }
                    None => {
                        status_note =
                            Some("No editor found: install nvim or set EDITOR".to_string());
                    }
                }
            }
            "convert" => {
                let (_out_file, cmd, step_label) = convert_spec(kind, target, &parent, &base);
                let title = format!("Convert {base}");
                let tag = format!("convert-{}", std::process::id());
                let task =
                    Task::start_steps(&tag, title.clone(), vec![step_label], true, vec![cmd])
                        .map_err(|e| {
                            crate::Failure::new("Convert", format!("Could not start ffmpeg: {e}"))
                        })?;
                let mut view = TaskView::new(task);
                return crate::run_screen(terminal, design, &title, &mut view);
            }
            _ => return Ok(0),
        }
    }
}

fn archive_stem(base: &str) -> &str {
    for suffix in [
        ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".txz", ".tbz2", ".tbz", ".tzst",
        ".zip", ".tar",
    ] {
        if let Some(stem) = base.strip_suffix(suffix)
            && !stem.is_empty()
        {
            return stem;
        }
    }
    base
}

fn convert_spec(
    kind: FileKind,
    target: &Path,
    parent: &Path,
    base: &str,
) -> (std::path::PathBuf, Vec<String>, String) {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(base);
    if kind == FileKind::Audio {
        let out_file = parent.join(format!("{stem}-converted.mp3"));
        let cmd = vec![
            "ffmpeg".to_string(),
            "-hide_banner".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            target.to_string_lossy().to_string(),
            "-vn".to_string(),
            "-c:a".to_string(),
            "libmp3lame".to_string(),
            "-q:a".to_string(),
            "2".to_string(),
            out_file.to_string_lossy().to_string(),
        ];
        let step = format!("Transcode to {stem}-converted.mp3");
        (out_file, cmd, step)
    } else {
        let out_file = parent.join(format!("{stem}-converted.mp4"));
        let cmd = vec![
            "ffmpeg".to_string(),
            "-hide_banner".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            target.to_string_lossy().to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-crf".to_string(),
            "23".to_string(),
            "-preset".to_string(),
            "veryfast".to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "160k".to_string(),
            out_file.to_string_lossy().to_string(),
        ];
        let step = format!("Transcode to {stem}-converted.mp4");
        (out_file, cmd, step)
    }
}

pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let path = args.first().cloned().unwrap_or_else(|| ".".to_string());
    menu_for_path(&path, terminal, design)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_kind_extension_matching() {
        assert_eq!(detect_kind(Path::new("archive.zip")), FileKind::ArchiveZip);
        assert_eq!(
            detect_kind(Path::new("bundle.tar.gz")),
            FileKind::ArchiveTar
        );
        assert_eq!(detect_kind(Path::new("os.iso")), FileKind::DiskImage);
        assert_eq!(detect_kind(Path::new("song.mp3")), FileKind::Audio);
        assert_eq!(detect_kind(Path::new("clip.mp4")), FileKind::Video);
        assert_eq!(detect_kind(Path::new("doc.pdf")), FileKind::Pdf);
        assert_eq!(detect_kind(Path::new("book.epub")), FileKind::Epub);
        assert_eq!(detect_kind(Path::new("take.xopp")), FileKind::Ink);
        assert_eq!(detect_kind(Path::new("notes.md")), FileKind::Text);
        assert_eq!(detect_kind(Path::new("README")), FileKind::Other);
    }

    #[test]
    fn archive_stem_strips_short_and_long_tar_suffixes() {
        assert_eq!(archive_stem("backup.tar.gz"), "backup");
        assert_eq!(archive_stem("backup.tgz"), "backup");
        assert_eq!(archive_stem("backup.txz"), "backup");
        assert_eq!(archive_stem("backup.tbz2"), "backup");
        assert_eq!(archive_stem("backup.tzst"), "backup");
        assert_eq!(archive_stem("photos.zip"), "photos");
    }

    #[test]
    fn convert_spec_transcodes_audio_to_mp3_and_video_to_mp4() {
        let (audio_out, audio_cmd, _) = convert_spec(
            FileKind::Audio,
            Path::new("/tmp/track.flac"),
            Path::new("/tmp"),
            "track.flac",
        );
        assert_eq!(
            audio_out,
            std::path::PathBuf::from("/tmp/track-converted.mp3")
        );
        assert!(audio_cmd.contains(&"libmp3lame".to_string()));
        assert!(!audio_cmd.contains(&"libx264".to_string()));

        let (video_out, video_cmd, _) = convert_spec(
            FileKind::Video,
            Path::new("/tmp/clip.mkv"),
            Path::new("/tmp"),
            "clip.mkv",
        );
        assert_eq!(
            video_out,
            std::path::PathBuf::from("/tmp/clip-converted.mp4")
        );
        assert!(video_cmd.contains(&"libx264".to_string()));
    }
}
