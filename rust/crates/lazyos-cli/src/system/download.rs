//! Media Downloader (`lazyos-download`, `lazyos menu download`).
//!
//! Prompts for video/audio URL (auto-populating from clipboard if valid),
//! asks for download profile (video vs audio-only), and runs `yt-dlp` inside
//! an in-window [`TaskView`] with live streaming progress and stop support.

use std::fs;
use std::path::Path;
use std::process::Command;

use lazyos_ui::{Design, Glyph, Task, TaskView};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::pick::{Picker, Question, row};
use super::{have, out};

/// Get the first URL in the clipboard if valid.
pub fn clipboard_url() -> Option<String> {
    if !have("wl-paste") {
        return None;
    }
    let text = out(&["wl-paste", "-n"]).unwrap_or_default();
    let first = text.lines().next()?.trim();
    if first.starts_with("http://") || first.starts_with("https://") {
        Some(first.to_string())
    } else {
        None
    }
}

/// Check if yt-dlp is currently running.
pub fn is_downloading() -> bool {
    let procs = out(&["pgrep", "-f", "yt-dlp"]).unwrap_or_default();
    !procs.trim().is_empty()
}

/// Count files in ~/Downloads.
pub fn downloads_count() -> usize {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let dl = Path::new(&home).join("Downloads");
    if let Ok(entries) = fs::read_dir(&dl) {
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .count()
    } else {
        0
    }
}

/// Build the yt-dlp argv for `url` into `dl_dir`.
pub fn build_download_argv(url: &str, dl_dir: &Path, audio_only: bool) -> Vec<String> {
    let mut argv = vec![
        "yt-dlp".to_string(),
        "--newline".to_string(),
        "-P".to_string(),
        dl_dir.display().to_string(),
        "--no-playlist".to_string(),
        "--no-mtime".to_string(),
        "--no-simulate".to_string(),
    ];
    if audio_only {
        argv.extend([
            "-x".to_string(),
            "--audio-format".to_string(),
            "m4a".to_string(),
        ]);
    }
    argv.extend(["--".to_string(), url.to_string()]);
    argv
}

/// The one question of `lazyos menu download`, built from data so a snapshot
/// can draw it without a clipboard, a Downloads folder or a running yt-dlp.
pub fn questions(clip: Option<&str>, count: usize, downloading: bool, note: &str) -> Vec<Question> {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    if let Some(url) = clip {
        rows.push(row("The link in the clipboard", url, Glyph::Clipboard));
        keys.push(format!("clip:{url}"));
    }

    rows.push(row(
        "Paste or type a link",
        "YouTube, Vimeo, SoundCloud, or a direct file URL",
        Glyph::Cloud,
    ));
    keys.push("link".to_string());

    let s = if count == 1 { "" } else { "s" };
    rows.push(row(
        "The Downloads folder",
        format!("{count} file{s} \u{00b7} ~/Downloads"),
        Glyph::Folder,
    ));
    keys.push("folder".to_string());

    if downloading {
        rows.push(row(
            "Stop active background download",
            "stops yt-dlp; partial .part file stays in ~/Downloads",
            Glyph::Warning,
        ));
        keys.push("stop".to_string());
    }

    vec![Question::choose("Download", note, rows, keys)]
}

/// Interactive `lazyos menu download`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if !have("yt-dlp") {
        return Err(crate::Failure::new(
            "Download",
            "yt-dlp is not installed (package yt-dlp)",
        ));
    }

    let mut banner: Option<String> = None;

    loop {
        let clip = clipboard_url();
        let count = downloads_count();
        let downloading = is_downloading();

        let note = banner
            .take()
            .unwrap_or_else(|| "Enter downloads in this window \u{00b7} Escape closes".to_string());

        let mut picker = Picker::new(
            "Download",
            questions(clip.as_deref(), count, downloading, &note),
        );

        let code = crate::run_screen(terminal, design, "Download", &mut picker)?;
        if code != 0 {
            return Ok(code);
        }

        let Some(action) = picker.answers().first().cloned() else {
            return Ok(1);
        };

        match action.as_str() {
            "folder" => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                let dl = Path::new(&home).join("Downloads");
                crate::system::spawn_detached(&format!(
                    "exec lazyos menu files '{}'",
                    dl.display()
                ));
                return Ok(0);
            }
            "stop" => {
                let _ = Command::new("pkill").args(["-f", "yt-dlp"]).status();
                banner = Some(
                    "Stopped active yt-dlp process \u{00b7} partial .part file remains in ~/Downloads"
                        .to_string(),
                );
            }
            "link" => {
                let mut url_picker = Picker::new(
                    "Link",
                    vec![Question::input(
                        "Download URL",
                        "Enter URL starting with http:// or https:// \u{00b7} Escape goes back",
                        "https://...",
                        false,
                    )],
                );
                let uc = crate::run_screen(terminal, design, "Link", &mut url_picker)?;
                if uc != 0 {
                    continue;
                }
                let Some(raw_url) = url_picker.answers().first() else {
                    continue;
                };
                let url = raw_url.trim();
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    banner =
                        Some("Not a valid URL (must start with http:// or https://)".to_string());
                    continue;
                }
                let res = ask_kind_and_download(terminal, design, url)?;
                if res == lazyos_ui::app::exit::ESCAPE {
                    continue;
                }
                return Ok(res);
            }
            act if act.starts_with("clip:") => {
                let url = &act[5..];
                let res = ask_kind_and_download(terminal, design, url)?;
                if res == lazyos_ui::app::exit::ESCAPE {
                    continue;
                }
                return Ok(res);
            }
            _ => return Ok(0),
        }
    }
}

fn ask_kind_and_download(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    url: &str,
) -> Result<i32, crate::Failure> {
    let rows = vec![
        row(
            "Video",
            "best video + audio streams merged into ~/Downloads",
            Glyph::Video,
        ),
        row(
            "Audio only",
            "m4a extracted from the best audio stream",
            Glyph::Music,
        ),
    ];
    let keys = vec!["video".to_string(), "audio".to_string()];

    let mut picker = Picker::new(
        "Download profile",
        vec![Question::choose("Download profile", url, rows, keys)],
    );

    let code = crate::run_screen(terminal, design, "Download profile", &mut picker)?;
    if code != 0 {
        return Ok(code);
    }

    let audio_only = picker.answers().first().map(String::as_str) == Some("audio");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let dl = Path::new(&home).join("Downloads");
    let _ = fs::create_dir_all(&dl);

    let argv = build_download_argv(url, &dl, audio_only);
    let kind_label = if audio_only { "Audio (m4a)" } else { "Video" };
    let preview: String = url.chars().take(48).collect();
    let tag = format!("download-{}", std::process::id());
    let step_label = format!("yt-dlp {preview} \u{2192} ~/Downloads");
    let task = Task::start_steps(
        &tag,
        format!("Download {kind_label}"),
        vec![step_label],
        true,
        vec![argv],
    )
    .map_err(|e| crate::Failure::new("Download", e.to_string()))?;

    let mut view = TaskView::new(task);
    crate::run_screen(terminal, design, "Download", &mut view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_url_prefixes() {
        assert!("https://example.com/video".starts_with("https://"));
        assert!("http://example.com/audio".starts_with("http://"));
        assert!(!"ftp://example.com".starts_with("http://"));
    }

    #[test]
    fn build_download_argv_includes_audio_flags_when_requested() {
        let argv = build_download_argv("https://example.com/v", Path::new("/tmp"), true);
        assert!(argv.contains(&"-x".to_string()));
        assert!(argv.contains(&"m4a".to_string()));
    }
}
