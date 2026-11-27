//! Unified Download Deck (`hayato-download`, `hayato menu download`).
//!
//! Single-screen download studio: auto-fills a valid clipboard URL, lets you
//! toggle Video vs Audio-only (m4a) inline (`Tab` / `Ctrl+P`), shows recent
//! files in `~/Downloads`, and runs `yt-dlp` inside an embedded [`TaskView`].

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, KeyHint, Outcome, Screen, Task, TaskView,
    TextStyle, draw_card,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::pick::{Question, row};
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
    let dl = downloads_dir();
    if let Ok(entries) = fs::read_dir(&dl) {
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .count()
    } else {
        0
    }
}

fn downloads_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join("Downloads")
}

fn recent_downloads(limit: usize) -> Vec<(String, String)> {
    let dl = downloads_dir();
    let Ok(entries) = fs::read_dir(&dl) else {
        return Vec::new();
    };
    let mut items: Vec<(std::time::SystemTime, String, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            Some((modified, name, meta.len()))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items
        .into_iter()
        .take(limit)
        .map(|(_, name, bytes)| {
            let size = if bytes >= 1_073_741_824 {
                format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
            } else if bytes >= 1_048_576 {
                format!("{:.1} MB", bytes as f64 / 1_048_576.0)
            } else if bytes >= 1024 {
                format!("{} KB", bytes / 1024)
            } else {
                format!("{bytes} B")
            };
            (name, size)
        })
        .collect()
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

/// The one question of `hayato menu download`, built from data so a snapshot
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

enum DownloadMode {
    Compose,
    Downloading(Box<TaskView>),
}

/// Unified Download Deck screen.
pub struct DownloadScreen {
    input: Input,
    audio_only: bool,
    recent: Vec<(String, String)>,
    total_files: usize,
    active_bg: bool,
    mode: DownloadMode,
    banner: Option<(String, bool)>,
    pill_hits: Cell<[Rect; 2]>,
    header: Header,
    keybar: Vec<KeyHint>,
}

impl Default for DownloadScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadScreen {
    pub fn new() -> Self {
        let clip = clipboard_url();
        let input = match &clip {
            Some(url) => Input::with_text(url.clone()),
            None => Input::new(),
        };
        let total_files = downloads_count();
        let recent = recent_downloads(12);
        let active_bg = is_downloading();
        let banner = clip.map(|_| {
            (
                "Prefilled URL from clipboard · press Enter to start".into(),
                false,
            )
        });

        let mut hints = vec![
            KeyHint::new("Enter", "download"),
            KeyHint::new("Tab/^P", "video / audio"),
            KeyHint::new("Ctrl+O", "open folder"),
        ];
        if active_bg {
            hints.push(KeyHint::new("Ctrl+K", "stop bg"));
        }
        hints.push(KeyHint::new("Esc", "close"));

        Self {
            input,
            audio_only: false,
            recent,
            total_files,
            active_bg,
            mode: DownloadMode::Compose,
            banner,
            pill_hits: Cell::new([Rect::default(), Rect::default()]),
            header: Header::new("Hayato")
                .subtitle("Download Deck")
                .status(format!("{total_files} files in ~/Downloads")),
            keybar: hints,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 56, 52)
    }

    fn start_download(&mut self) {
        let url = self.input.text().trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.banner = Some((
                "Enter a valid URL starting with http:// or https://".into(),
                true,
            ));
            return;
        }
        let dl = downloads_dir();
        let _ = fs::create_dir_all(&dl);
        let argv = build_download_argv(&url, &dl, self.audio_only);
        let kind_label = if self.audio_only {
            "Audio (m4a)"
        } else {
            "Video"
        };
        let preview: String = url.chars().take(48).collect();
        let tag = format!("download-{}", std::process::id());
        let step_label = format!("yt-dlp {preview} → ~/Downloads");
        match Task::start_steps(
            &tag,
            format!("Download {kind_label}"),
            vec![step_label],
            true,
            vec![argv],
        ) {
            Ok(task) => {
                self.mode = DownloadMode::Downloading(Box::new(TaskView::new(task)));
                self.banner = None;
            }
            Err(e) => {
                self.banner = Some((format!("Could not start yt-dlp: {e}"), true));
            }
        }
    }
}

impl Screen for DownloadScreen {
    fn needs_tick(&self) -> Option<Duration> {
        match &self.mode {
            DownloadMode::Downloading(tv) => tv.needs_tick(),
            DownloadMode::Compose => None,
        }
    }

    fn tick(&mut self) -> Outcome {
        match &mut self.mode {
            DownloadMode::Downloading(tv) => tv.tick(),
            DownloadMode::Compose => Outcome::Continue,
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let DownloadMode::Downloading(tv) = &mut self.mode {
            tv.draw(ctx);
            return;
        }

        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("New Download"), true);
        let inner = Rect::new(
            left.x + 2,
            left.y + 1,
            left.width.saturating_sub(4),
            left.height.saturating_sub(2),
        );
        if inner.width >= 16 && inner.height >= 8 {
            let dim = ctx.design.style(TextStyle::Dim);
            let body = ctx.design.style(TextStyle::Body);
            let accent = Style::default()
                .fg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD);

            let mut y = inner.y;
            put_trunc(
                buf_ref(ctx),
                inner.x,
                y,
                "Media or Direct URL",
                inner.width,
                accent,
            );
            y += 1;
            let input_rect = Rect::new(inner.x, y, inner.width, 1);
            self.input.render(
                input_rect,
                ctx.buf,
                ctx.design,
                "https://youtube.com/watch?v=…",
            );
            y += 2;

            put_trunc(
                ctx.buf,
                inner.x,
                y,
                "Profile (Tab to toggle)",
                inner.width,
                accent,
            );
            y += 1;
            let (vid_st, aud_st) = if !self.audio_only {
                (accent, dim)
            } else {
                (dim, accent)
            };
            let vid_pill = if !self.audio_only {
                "[ ● Video + Audio ]"
            } else {
                "  ○ Video + Audio  "
            };
            let aud_pill = if self.audio_only {
                "[ ● Audio only (m4a) ]"
            } else {
                "  ○ Audio only (m4a)  "
            };
            let next_x = put(ctx.buf, inner.x, y, vid_pill, inner.width, vid_st);
            let aud_x = next_x + 2;
            let aud_w = unicode_width::UnicodeWidthStr::width(aud_pill) as u16;
            put_trunc(
                ctx.buf,
                aud_x,
                y,
                aud_pill,
                inner.right().saturating_sub(aud_x),
                aud_st,
            );
            let vid_w = next_x.saturating_sub(inner.x);
            self.pill_hits.set([
                Rect::new(inner.x, y, vid_w, 1),
                Rect::new(aud_x, y, aud_w.min(inner.right().saturating_sub(aud_x)), 1),
            ]);
            y += 2;

            put_trunc(
                ctx.buf,
                inner.x,
                y,
                "Destination   ~/Downloads (single item, no playlist)",
                inner.width,
                dim,
            );
            y += 2;

            if self.active_bg && y < inner.bottom().saturating_sub(2) {
                put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    "● Background yt-dlp running · Ctrl+K stops",
                    inner.width,
                    Style::default().fg(ctx.design.color(ColorRole::Warn)),
                );
                y += 2;
            }

            if let Some((msg, is_err)) = &self.banner
                && y < inner.bottom().saturating_sub(1)
            {
                let st = Style::default().fg(ctx.design.color(if *is_err {
                    ColorRole::Danger
                } else {
                    ColorRole::Ok
                }));
                put_trunc(ctx.buf, inner.x, y, msg, inner.width, st);
            }

            put_trunc(
                ctx.buf,
                inner.x,
                inner.bottom().saturating_sub(1),
                "Enter starts yt-dlp · Ctrl+O opens ~/Downloads",
                inner.width,
                body,
            );
        }

        if let Some(right_area) = right {
            draw_card(
                ctx.buf,
                right_area,
                ctx.design,
                Some("Recent Downloads"),
                false,
            );
            let r_in = Rect::new(
                right_area.x + 2,
                right_area.y + 1,
                right_area.width.saturating_sub(4),
                right_area.height.saturating_sub(2),
            );
            let dim = ctx.design.style(TextStyle::Dim);
            let body = ctx.design.style(TextStyle::Body);
            let mut y = r_in.y;
            if self.recent.is_empty() {
                put_trunc(
                    ctx.buf,
                    r_in.x,
                    y,
                    "No files in ~/Downloads yet",
                    r_in.width,
                    dim,
                );
            } else {
                for (name, size) in &self.recent {
                    if y >= r_in.bottom().saturating_sub(1) {
                        break;
                    }
                    let sw = size.chars().count() as u16;
                    let max_name = r_in.width.saturating_sub(sw + 4);
                    put_trunc(
                        ctx.buf,
                        r_in.x,
                        y,
                        &format!("{}  {name}", Glyph::File.cell()),
                        max_name,
                        body,
                    );
                    let sx = r_in.right().saturating_sub(sw);
                    if sx > r_in.x + 4 {
                        put_trunc(ctx.buf, sx, y, size, sw, dim);
                    }
                    y += 1;
                }
            }
            put_trunc(
                ctx.buf,
                r_in.x,
                r_in.bottom().saturating_sub(1),
                &format!("{} total files · ~/Downloads", self.total_files),
                r_in.width,
                dim,
            );
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let DownloadMode::Downloading(tv) = &mut self.mode {
            return match tv.key(key, design) {
                Outcome::Done(code) => {
                    self.mode = DownloadMode::Compose;
                    self.total_files = downloads_count();
                    self.recent = recent_downloads(12);
                    self.active_bg = is_downloading();
                    let msg = if code == hayato_ui::app::exit::OK {
                        "Download finished · check ~/Downloads"
                    } else {
                        "Download stopped · check ~/Downloads"
                    };
                    self.banner = Some((msg.into(), false));
                    Outcome::Consumed
                }
                other => other,
            };
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.audio_only = !self.audio_only;
                    return Outcome::Consumed;
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    let dl = downloads_dir().to_string_lossy().into_owned();
                    crate::system::spawn_detached_argv("hayato", &["menu", "files", &dl]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                KeyCode::Char('k') | KeyCode::Char('K') if self.active_bg => {
                    let _ = Command::new("pkill").args(["-f", "yt-dlp"]).status();
                    self.active_bg = false;
                    self.banner = Some(("Stopped background yt-dlp process".into(), false));
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.audio_only = !self.audio_only;
                Outcome::Consumed
            }
            KeyCode::Enter => {
                self.start_download();
                Outcome::Consumed
            }
            KeyCode::Esc => {
                if !self.input.text().is_empty() {
                    self.input.clear();
                    return Outcome::Consumed;
                }
                Outcome::Done(hayato_ui::app::exit::ESCAPE)
            }
            _ => {
                let _ = self.input.handle(key);
                Outcome::Consumed
            }
        }
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, _design: &Design) -> Option<usize> {
        if !matches!(self.mode, DownloadMode::Compose) {
            return None;
        }
        let pills = self.pill_hits.get();
        for (i, r) in pills.iter().enumerate() {
            if r.width > 0 && x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(i);
            }
        }
        None
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if !matches!(self.mode, DownloadMode::Compose) {
            return Outcome::Continue;
        }
        match row {
            Some(0) => {
                self.audio_only = false;
                Outcome::Consumed
            }
            Some(1) => {
                self.audio_only = true;
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match &self.mode {
            DownloadMode::Downloading(tv) => tv.keybar(),
            DownloadMode::Compose => Some(&self.keybar),
        }
    }

    fn header(&self) -> Option<&Header> {
        match &self.mode {
            DownloadMode::Downloading(tv) => tv.header(),
            DownloadMode::Compose => Some(&self.header),
        }
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            DownloadMode::Compose => "download.compose",
            DownloadMode::Downloading(_) => "download.running",
        })
    }
}

fn buf_ref<'a>(ctx: &'a mut BodyCtx) -> &'a mut ratatui::buffer::Buffer {
    ctx.buf
}

/// Interactive `hayato menu download`.
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
    let mut screen = DownloadScreen::new();
    crate::run_screen(terminal, design, "Download", &mut screen)
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

    #[test]
    fn download_deck_toggles_profile_inline_with_tab() {
        let design = Design::solarized();
        let mut screen = DownloadScreen::new();
        assert!(!screen.audio_only);
        let _ = screen.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &design);
        assert!(screen.audio_only);
    }
}
