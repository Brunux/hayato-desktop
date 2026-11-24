//! Capture: screenshots, annotate, OCR, QR, recording (`hayato menu capture`,
//! also reachable as `hayato-capture`).
//!
//! The old script was a old list over tools the image already has. Every row is
//! one command that ends before the list closes: grimshot takes the picture
//! (screen, area, window, saved under ~/Pictures/Screenshots/<date> and named
//! in a notification), swappy annotates one, tesseract reads the text out of a
//! region and zbarimg reads a QR or barcode out of one. Both copy the result
//! to the clipboard and say so.
//!
//! Recording is the exception: wf-recorder keeps running after the menu closes,
//! so it is started here, announced on the bar and in a mako card, and stopped
//! by opening this same menu again. Its pid and file name live in
//! $XDG_RUNTIME_DIR/hayato-capture, so a session that dies takes the state with
//! it and a killed recorder is not remembered; the file it was writing stays.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use hayato_ui::widgets::{draw_badge, put_trunc};
use hayato_ui::{
    BodyCtx, Card, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::{missing, out};

const NEEDED: &[&str] = &[
    "grimshot",
    "swappy",
    "tesseract",
    "zbarimg",
    "wf-recorder",
    "slurp",
    "grim",
];

const HIT_AUDIO: usize = 20_000;
const HIT_WEBCAM: usize = 20_001;
const HIT_RECENT_BASE: usize = 21_000;

#[derive(Clone, Debug)]
struct RecentCapture {
    name: String,
    path: PathBuf,
    is_video: bool,
    size_str: String,
    age_str: String,
}

#[derive(Default, Clone)]
struct StudioHits {
    audio: Option<Rect>,
    webcam: Option<Rect>,
    recent: Vec<Rect>,
}

enum CaptureMode {
    Idle,
    Recording {
        pid: u32,
        file: Option<PathBuf>,
        started: Option<SystemTime>,
        elapsed: String,
        size: String,
    },
}

/// Capture & Recording Studio (`hayato menu capture`).
pub struct CaptureScreen {
    mode: CaptureMode,
    options: hayato_ui::recording::Options,
    recent: Vec<RecentCapture>,
    shot_dir: PathBuf,
    day_str: String,
    list: ListState,
    keys: Vec<Option<&'static str>>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    hits: RefCell<StudioHits>,
    open_qr: bool,
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_age(modified: SystemTime, now: SystemTime) -> String {
    let secs = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn scan_recent_captures(shot_dir: &Path, videos_dir: &Path) -> Vec<RecentCapture> {
    let now = SystemTime::now();
    let mut found: Vec<(SystemTime, RecentCapture)> = Vec::new();
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dirs_to_check = [
        shot_dir.join(&day),
        shot_dir.to_path_buf(),
        videos_dir.to_path_buf(),
    ];

    for dir in dirs_to_check {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            let is_video = matches!(ext.as_str(), "mp4" | "mkv" | "webm");
            let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp");
            if !is_video && !is_image {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if found.iter().any(|(_, r)| r.path == path) {
                continue;
            }
            found.push((
                modified,
                RecentCapture {
                    name,
                    path,
                    is_video,
                    size_str: format_bytes(meta.len()),
                    age_str: format_age(modified, now),
                },
            ));
        }
    }

    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().take(5).map(|(_, r)| r).collect()
}

impl CaptureScreen {
    pub fn new() -> Self {
        let shot_dir = dirs::picture_dir()
            .map(|p| p.join("Screenshots"))
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Pictures"));
        let videos_dir = dirs::video_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Videos"));
        let day_str = chrono::Local::now().format("%Y-%m-%d").to_string();
        let options = hayato_ui::recording::load_options();
        let recent = scan_recent_captures(&shot_dir, &videos_dir);

        let mut screen = Self {
            mode: CaptureMode::Idle,
            options,
            recent,
            shot_dir,
            day_str,
            list: ListState::new().with_empty("no matching capture actions"),
            keys: Vec::new(),
            header: Header::new("Hayato").subtitle("Screenshot"),
            keybar: vec![
                KeyHint::new("Enter", "capture"),
                KeyHint::new("^O", "folder"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            hits: RefCell::new(StudioHits::default()),
            open_qr: false,
        };
        screen.check_recording();
        screen.build_rows();
        screen.update_header();
        screen
    }

    fn check_recording(&mut self) {
        let state = hayato_ui::recording::state_dir();
        let recorder = hayato_ui::recording::read_in(&state);
        match recorder {
            hayato_ui::recording::Recorder::Recording { pid, file } => {
                let started = file
                    .as_deref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.created().ok().or_else(|| m.modified().ok()));
                self.mode = CaptureMode::Recording {
                    pid,
                    file,
                    started,
                    elapsed: "00:00:00".to_string(),
                    size: "0 B".to_string(),
                };
                self.refresh_recording_telemetry();
            }
            hayato_ui::recording::Recorder::Idle => {
                stop_camera(&state);
                hayato_ui::recording::clear_in(&state);
                self.mode = CaptureMode::Idle;
            }
        }
    }

    fn refresh_recording_telemetry(&mut self) {
        if let CaptureMode::Recording {
            file,
            started,
            elapsed,
            size,
            ..
        } = &mut self.mode
        {
            if let Some(start) = *started {
                let secs = SystemTime::now()
                    .duration_since(start)
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                *elapsed = format_elapsed(secs);
            }
            if let Some(path) = file.as_deref() {
                if let Ok(meta) = std::fs::metadata(path) {
                    *size = format_bytes(meta.len());
                }
            }
        }
    }

    fn update_header(&mut self) {
        let status = match &self.mode {
            CaptureMode::Idle => format!(
                "Ready \u{00b7} {}/{}",
                self.shot_dir.display(),
                self.day_str
            ),
            CaptureMode::Recording { elapsed, size, .. } => {
                format!("● REC {elapsed} \u{00b7} {size} \u{00b7} Enter stops")
            }
        };
        self.header = Header::new("Hayato").subtitle("Screenshot").status(status);
    }

    fn cycle_audio(&mut self) {
        self.options.audio = self.options.audio.cycle();
        let _ = hayato_ui::recording::save_options(self.options);
        self.build_rows();
        self.update_header();
    }

    fn toggle_webcam(&mut self) {
        self.options.webcam = !self.options.webcam;
        let _ = hayato_ui::recording::save_options(self.options);
        self.build_rows();
        self.update_header();
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys: Vec<Option<&'static str>> = Vec::new();

        if let CaptureMode::Recording { file, .. } = &self.mode {
            rows.push(Row::heading("Recording"));
            keys.push(None);
            rows.push(
                Row::new("Stop recording")
                    .glyph(Glyph::Stop)
                    .detail(
                        file.as_deref()
                            .and_then(|path| path.file_name())
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                    )
                    .value("● REC"),
            );
            keys.push(Some("stop"));
        } else {
            rows.push(Row::heading("Snapshot"));
            keys.push(None);
            rows.push(
                Row::new("Screenshot · Whole screen")
                    .glyph(Glyph::Camera)
                    .detail(format!(
                        "save to {}/{}",
                        self.shot_dir.display(),
                        self.day_str
                    )),
            );
            keys.push(Some("shot:save:screen"));
            rows.push(
                Row::new("Screenshot · Area (save)")
                    .glyph(Glyph::Crop)
                    .detail("drag a region with slurp"),
            );
            keys.push(Some("shot:save:area"));
            rows.push(
                Row::new("Screenshot · Area → clipboard")
                    .glyph(Glyph::Crop)
                    .detail("paste it anywhere"),
            );
            keys.push(Some("shot:copy:area"));
            rows.push(
                Row::new("Screenshot · Window")
                    .glyph(Glyph::Image)
                    .detail("click a window to capture"),
            );
            keys.push(Some("shot:save:window"));
            rows.push(
                Row::new("Annotate area (swappy)")
                    .glyph(Glyph::Camera)
                    .detail("arrows, text, crop"),
            );
            keys.push(Some("annotate"));
        }

        rows.push(Row::heading("Library"));
        keys.push(None);
        rows.push(
            Row::new("Open Screenshots folder")
                .glyph(Glyph::Folder)
                .detail(self.shot_dir.display().to_string()),
        );
        keys.push(Some("folder"));

        self.list.set_rows(rows);
        self.keys = keys;
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let left_w = ((area.width as u32 * 48) / 100).clamp(44, 62) as u16;
            let right_w = area.width.saturating_sub(left_w + 1);
            let left = Rect::new(area.x, area.y, left_w, area.height);
            let right = Rect::new(area.x + left_w + 1, area.y, right_w, area.height);
            (left, Some(right))
        } else {
            (area, None)
        }
    }

    fn left_layout(left: Rect, compact_strip: bool) -> (Rect, Rect, Rect) {
        let inner = Rect {
            x: left.x + 2,
            y: left.y + 1,
            width: left.width.saturating_sub(4),
            height: left.height.saturating_sub(2),
        };
        if compact_strip {
            let strip = Rect {
                height: 2.min(inner.height),
                ..inner
            };
            let filter = Rect {
                y: inner.y + 2,
                height: 1.min(inner.height.saturating_sub(2)),
                ..inner
            };
            let rows = Rect {
                y: inner.y + 4,
                height: inner.height.saturating_sub(4),
                ..inner
            };
            (strip, filter, rows)
        } else {
            let strip = Rect::new(inner.x, inner.y, 0, 0);
            let filter = Rect {
                height: 1.min(inner.height),
                ..inner
            };
            let rows = Rect {
                y: inner.y + 2,
                height: inner.height.saturating_sub(2),
                ..inner
            };
            (strip, filter, rows)
        }
    }

    fn draw_studio_deck(&self, ctx: &mut BodyCtx, right: Rect) -> StudioHits {
        let mut hits = StudioHits::default();
        draw_card(ctx.buf, right, ctx.design, Some("Screenshot Deck"), false);
        let inner = Rect {
            x: right.x + 2,
            y: right.y + 1,
            width: right.width.saturating_sub(4),
            height: right.height.saturating_sub(2),
        };
        if inner.width < 12 || inner.height < 6 {
            return hits;
        }
        let mut y = inner.y;

        // 1. Status & Target Folder
        let x = draw_badge(ctx.buf, inner.x, y, ctx.design, "● READY", ColorRole::Ok);
        let avail = inner.right().saturating_sub(x + 2);
        put_trunc(
            ctx.buf,
            x + 2,
            y,
            "Stills & Annotation",
            avail,
            ctx.design.style(TextStyle::Body),
        );
        y += 1;
        if y < inner.bottom() {
            put_trunc(
                ctx.buf,
                inner.x,
                y,
                &format!("Target: {}/{}", self.shot_dir.display(), self.day_str),
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
            y += 2;
        }
        if y + 2 <= inner.bottom() {
            put_trunc(
                ctx.buf,
                inner.x,
                y,
                "Video / Webcam → Studio (Alt+Ctrl+R) · OCR / QR → Lens (Alt+I)",
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
            y += 2;
        }

        // 2. Recent Captures
        if y + 2 <= inner.bottom() {
            put_trunc(
                ctx.buf,
                inner.x,
                y,
                "Recent Captures (click or ^O opens folder)",
                inner.width,
                ctx.design.style(TextStyle::Title),
            );
            y += 1;

            if self.recent.is_empty() {
                put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    "No screenshots or recordings yet",
                    inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
            } else {
                for item in &self.recent {
                    if y >= inner.bottom() {
                        break;
                    }
                    let icon = if item.is_video {
                        Glyph::Video.cell()
                    } else {
                        Glyph::Image.cell()
                    };
                    let meta = format!("{} \u{00b7} {}", item.size_str, item.age_str);
                    let meta_w = meta.chars().count() as u16;
                    let name_max = inner.width.saturating_sub(meta_w + 4);
                    let left_text = format!("{icon} {}", item.name);
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        y,
                        &left_text,
                        name_max,
                        ctx.design.style(TextStyle::Body),
                    );
                    let mx = inner.right().saturating_sub(meta_w);
                    if mx > inner.x + 6 {
                        put_trunc(
                            ctx.buf,
                            mx,
                            y,
                            &meta,
                            meta_w,
                            ctx.design.style(TextStyle::Dim),
                        );
                    }
                    hits.recent.push(Rect::new(inner.x, y, inner.width, 1));
                    y += 1;
                }
            }
        }

        hits
    }
}

impl Screen for CaptureScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        let compact = right.is_none();

        draw_card(ctx.buf, left, ctx.design, Some("Actions"), true);
        let (strip, filter, rows) = Self::left_layout(left, compact);

        let mut hits = StudioHits::default();
        if compact && strip.height >= 1 {
            match &self.mode {
                CaptureMode::Idle => {
                    let line1 = format!(
                        "● READY \u{00b7} ^A Audio: {} \u{00b7} ^W Cam: {}",
                        self.options.audio.label(),
                        if self.options.webcam { "On" } else { "Off" }
                    );
                    put_trunc(
                        ctx.buf,
                        strip.x,
                        strip.y,
                        &line1,
                        strip.width,
                        Style::default().fg(ctx.design.color(ColorRole::Accent)),
                    );
                }
                CaptureMode::Recording { elapsed, size, .. } => {
                    let line1 = format!("● REC {elapsed} \u{00b7} {size} \u{00b7} Enter stops");
                    put_trunc(
                        ctx.buf,
                        strip.x,
                        strip.y,
                        &line1,
                        strip.width,
                        Style::default()
                            .fg(ctx.design.color(ColorRole::Danger))
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }

        let total = self.keys.iter().filter(|k| k.is_some()).count();
        let visible = if self.list.filter().is_empty() {
            total
        } else {
            self.list.visible_len()
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            "type to filter \u{00b7} Enter runs",
            visible,
            total,
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);

        if let Some(right_rect) = right {
            hits = self.draw_studio_deck(ctx, right_rect);
        }
        *self.hits.borrow_mut() = hits;
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.cycle_audio();
                    return Outcome::Consumed;
                }
                KeyCode::Char('w') | KeyCode::Char('W') => {
                    self.toggle_webcam();
                    return Outcome::Consumed;
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    let dir = self.shot_dir.to_string_lossy();
                    crate::system::spawn_detached_argv("hayato-files", &["--", &dir]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                _ => {}
            }
        }
        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_index() {
                    return self.activate(index);
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') if ctrl => {
                if let Some(item) = self.recent.first() {
                    let _ = crate::system::share::spawn_detached(
                        &crate::system::share::SharePayload::from_path(item.path.clone()),
                    );
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                return Outcome::Consumed;
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !ctrl && !alt && self.list.type_filter(key) =>
            {
                return Outcome::Continue;
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(r) = row else {
            return Outcome::Continue;
        };
        if r == HIT_AUDIO {
            self.cycle_audio();
            return Outcome::Consumed;
        }
        if r == HIT_WEBCAM {
            self.toggle_webcam();
            return Outcome::Consumed;
        }
        if r >= HIT_RECENT_BASE {
            let idx = r - HIT_RECENT_BASE;
            if let Some(item) = self.recent.get(idx) {
                if let Some(parent) = item.path.parent() {
                    let dir = parent.to_string_lossy();
                    crate::system::spawn_detached_argv("hayato-files", &["--", &dir]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
            return Outcome::Consumed;
        }
        self.list.set_cursor(r);
        if let Some(index) = self.list.selected_index() {
            return self.activate(index);
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let hits = self.hits.borrow();
        if let Some(r) = hits.audio {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(HIT_AUDIO);
            }
        }
        if let Some(r) = hits.webcam {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(HIT_WEBCAM);
            }
        }
        for (idx, r) in hits.recent.iter().enumerate() {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(HIT_RECENT_BASE + idx);
            }
        }
        let (left, right) = Self::split_panes(area);
        let (_, _, rows) = Self::left_layout(left, right.is_none());
        self.list.hit_row(rows, x, y, design)
    }

    fn needs_tick(&self) -> Option<Duration> {
        match self.mode {
            CaptureMode::Recording { .. } => Some(Duration::from_secs(1)),
            CaptureMode::Idle => None,
        }
    }

    fn tick(&mut self) -> Outcome {
        self.refresh_recording_telemetry();
        self.update_header();
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.mode {
            CaptureMode::Idle => "idle",
            CaptureMode::Recording { .. } => "recording",
        })
    }
}

impl CaptureScreen {
    fn activate(&mut self, index: usize) -> Outcome {
        let Some(Some(key)) = self.keys.get(index).copied() else {
            return Outcome::Continue;
        };
        if key == "record:audio" {
            self.cycle_audio();
            Outcome::Continue
        } else if key == "record:webcam" {
            self.toggle_webcam();
            Outcome::Continue
        } else if let Some(rest) = key.strip_prefix("shot:") {
            let target = rest.split(':').nth(1).unwrap_or("");
            let act = rest.split(':').next().unwrap_or("");
            Self::screenshot(act, target);
            Outcome::Done(hayato_ui::app::exit::OK)
        } else {
            match key {
                "annotate" => {
                    Self::annotate();
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "ocr" => {
                    Self::ocr();
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "qr" => {
                    Self::scan_qr();
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "rec:none" => {
                    Self::start_recording(false);
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "rec:area" => {
                    Self::start_recording(true);
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "stop" => {
                    Self::stop_recording();
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "make-qr" => {
                    self.open_qr = true;
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                "folder" => {
                    let shotdir = dirs::picture_dir()
                        .map(|p| p.join("Screenshots"))
                        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Pictures"));
                    let shotdir_str = shotdir.to_string_lossy();
                    crate::system::spawn_detached_argv("xdg-open", &[&shotdir_str]);
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                _ => Outcome::Continue,
            }
        }
    }

    /// Build the argv to invoke `~/.config/bin/screenshot` so saved screenshots
    /// land in `~/Pictures/Screenshots/<date>/` instead of `XDG_PICTURES_DIR`.
    pub fn screenshot_argv(home: &str, action: &str, target: &str) -> Vec<String> {
        let mut argv = vec![
            format!("{home}/.config/bin/screenshot"),
            "-n".to_string(),
            action.to_string(),
        ];
        if !target.is_empty() {
            argv.push(target.to_string());
        }
        argv
    }

    fn screenshot(action: &str, target: &str) {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let argv = Self::screenshot_argv(&home, action, target);
        let mut sh_args: Vec<&str> = vec!["-c", "sleep 0.2; exec \"$@\"", "sh"];
        for a in &argv {
            sh_args.push(a.as_str());
        }
        crate::system::spawn_detached_argv("sh", &sh_args);
    }

    fn annotate() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let script = format!(
            r#"sleep 0.2
_path=$("{home}/.config/bin/screenshot" save area) || exit 0
[ -n "$_path" ] || exit 0
swappy -f "$_path" -o "$_path" || true
if [ -s "$_path" ]; then
    notify-send -a Hayato -i camera-web -h "string:x-hayato-open-path:$_path" "Capture" "Saved $(basename "$_path") · click to show"
else
    notify-send -a Hayato -i camera-web "Capture" "The annotation was not saved"
fi"#
        );
        crate::system::spawn_detached(&script);
    }

    fn ocr() {
        let script = r#"sleep 0.2
_geom=$(slurp -d) || exit 0
[ -n "$_geom" ] || exit 0
_tmp=$(mktemp "${TMPDIR:-/tmp}/ocr.XXXXXX.png") || exit 1
grim -g "$_geom" "$_tmp" 2>/dev/null || { rm -f "$_tmp"; exit 0; }
_text=$(tesseract "$_tmp" stdout -l eng 2>/dev/null | sed -e 's/[[:space:]]*$//' -e '/^$/d') || true
rm -f "$_tmp"
if [ -n "$_text" ]; then
    printf '%s' "$_text" | wl-copy
    _preview=$(printf '%s' "$_text" | head -1 | cut -c1-60)
    notify-send -a Hayato -i text-x-generic "Text from the screen" "Text copied · $_preview"
else
    notify-send -a Hayato -i text-x-generic "Text from the screen" "No text was found in that region"
fi"#;
        crate::system::spawn_detached(script);
    }

    fn scan_qr() {
        let script = r#"sleep 0.2
_geom=$(slurp -d) || exit 0
[ -n "$_geom" ] || exit 0
_tmp=$(mktemp "${TMPDIR:-/tmp}/qr.XXXXXX.png") || exit 1
grim -g "$_geom" "$_tmp" 2>/dev/null || { rm -f "$_tmp"; exit 0; }
_code=$(zbarimg -q --raw "$_tmp" 2>/dev/null | tr -d '\r\n') || true
rm -f "$_tmp"
if [ -n "$_code" ]; then
    printf '%s' "$_code" | wl-copy
    _preview=$(printf '%s' "$_code" | head -1 | cut -c1-60)
    notify-send -a Hayato -i qr "QR code from the screen" "Copied · $_preview"
else
    notify-send -a Hayato -i qr "QR code from the screen" "No code was found in that region"
fi"#;
        crate::system::spawn_detached(script);
    }

    fn start_recording(area: bool) {
        // The menu closes first; a detached copy of this binary does the work,
        // so the recorder never inherits the window's terminal or process
        // group and its own pid is the one saved.
        let mut args = vec![
            "menu".to_string(),
            "capture".to_string(),
            "--start".to_string(),
        ];
        if area {
            args.push("area".to_string());
        }
        spawn_self(&args);
    }

    fn stop_recording() {
        let state = hayato_ui::recording::state_dir();
        let Some(pid) = hayato_ui::recording::pid_in(&state) else {
            return;
        };
        // Only the recorder itself may be signalled: a stale file can name a
        // number that now belongs to something else.
        if !hayato_ui::recording::is_recorder(pid) {
            hayato_ui::recording::clear_in(&state);
            return;
        }
        let file = hayato_ui::recording::file_in(&state)
            .map(|file| file.to_string_lossy().to_string())
            .unwrap_or_default();
        // The waiter outlives this window: it finalises the file, clears the
        // bar and only then says the recording is saved.
        spawn_self(&[
            "menu".to_string(),
            "capture".to_string(),
            "--stop-wait".to_string(),
            pid.to_string(),
            file,
        ]);
    }
}

/// Re-run this binary detached. `current_exe` is the real file, so a
/// multi-call symlink (`hayato-capture`, `hayato`) still reaches the dispatcher.
fn spawn_self(args: &[String]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::system::spawn_detached_argv(exe, &args);
}

/// The recorder's argv: one program and its arguments, never a shell line.
///
/// `-g` sits before `-f` exactly as `wf-recorder` expects it. The caller owns
/// every word, so a path with a space stays one argument.
pub fn recorder_argv(file: &Path, geometry: Option<&str>, audio: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "wf-recorder".to_string(),
        "-c".to_string(),
        "libx264".to_string(),
        "-p".to_string(),
        "preset=veryfast".to_string(),
        "-p".to_string(),
        "crf=28".to_string(),
        "-r".to_string(),
        "30".to_string(),
    ];
    if let Some(audio) = audio {
        argv.push(format!("--audio={audio}"));
    }
    if let Some(geometry) = geometry {
        argv.push("-g".to_string());
        argv.push(geometry.to_string());
    }
    argv.push("-f".to_string());
    argv.push(file.to_string_lossy().to_string());
    argv
}

/// `$HOME/Videos/hayato-<date>-<time>.mp4`, the file the recorder writes.
pub fn videos_file() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    home.join("Videos").join(format!("hayato-{stamp}.mp4"))
}

fn slurp_geometry() -> Option<String> {
    let output = Command::new("slurp")
        .arg("-d")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let geometry = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!geometry.is_empty()).then_some(geometry)
}

/// Write the recorder's pid and file. Called only after `/proc/<pid>/comm`
/// proved the pid is `wf-recorder`, so Stop can trust what it reads.
fn write_state(state: &Path, pid: u32, file: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(state)?;
    std::fs::write(state.join("recording.pid"), format!("{pid}\n"))?;
    std::fs::write(
        state.join("recording.file"),
        format!("{}\n", file.display()),
    )?;
    Ok(())
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn last_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
        })
        .unwrap_or_default()
}

fn announce_recording(file: &Path, options: hayato_ui::recording::Options) {
    let body = format!(
        "Capturing to {} · {} · Alt+Ctrl+C stops it",
        basename(file),
        options.summary()
    );
    if crate::system::have_busy() {
        crate::system::busy("record", "Recording", &body, "media-record");
    } else {
        crate::system::say("Recording", body, Urgency::Normal, Glyph::Video);
    }
}

fn announce_start_failure(why: &str) {
    let body = if why.is_empty() {
        "Recording did not start".to_string()
    } else {
        format!("Recording did not start · {why}")
    };
    if crate::system::have_busy() {
        crate::system::busy_fail("record", "Capture", &body, "dialog-error", None);
    } else {
        crate::system::say("Capture", body, Urgency::Critical, Glyph::Warning);
    }
}

fn announce_stopped(file: &Path, forced: bool) {
    let base = basename(file);
    let nonempty = file.metadata().map(|meta| meta.len() > 0).unwrap_or(false);
    let (body, critical) = if forced {
        (
            format!("Recording was killed · {base} may be incomplete"),
            true,
        )
    } else if nonempty {
        (format!("Recording saved · {base}"), false)
    } else {
        (format!("Recording failed · {base} is empty"), true)
    };
    if crate::system::have_busy() {
        if critical {
            crate::system::busy_fail("record", "Capture", &body, "dialog-error", Some(file));
        } else {
            crate::system::busy_end("record", "Recording", &body, Some(file));
        }
    } else {
        let glyph = if critical {
            Glyph::Warning
        } else {
            Glyph::Video
        };
        let urgency = if critical {
            Urgency::Critical
        } else {
            Urgency::Normal
        };
        crate::system::say_open("Recording", body, urgency, glyph, file);
    }
}

fn signal(pid: u32, signal: libc::c_int) {
    // SAFETY: `kill(2)` takes a pid and a signal number; the pid came from the
    // state file and was verified to be `wf-recorder` before this process ran.
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

/// Wait up to `tenths` × 100 ms for the recorder to stop being the recorder.
fn wait_gone(pid: u32, tenths: u32) -> bool {
    for _ in 0..tenths {
        if !hayato_ui::recording::is_recorder(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !hayato_ui::recording::is_recorder(pid)
}

/// The PulseAudio/PipeWire source for the chosen mode.
fn audio_source(mode: hayato_ui::recording::AudioMode) -> Result<Option<String>, String> {
    use hayato_ui::recording::AudioMode;
    match mode {
        AudioMode::Off => Ok(None),
        AudioMode::Mic => {
            if let Some(source) = out(&["pactl", "get-default-source"])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return Ok(Some(source));
            }
            let source = out(&["pactl", "list", "short", "sources"]).and_then(|text| {
                text.lines().find_map(|line| {
                    let name = line.split_whitespace().nth(1)?;
                    (!name.ends_with(".monitor")).then(|| name.to_string())
                })
            });
            source
                .map(Some)
                .ok_or_else(|| "No microphone source is available (pactl found none)".to_string())
        }
        AudioMode::System => {
            if let Some(sink) = out(&["pactl", "get-default-sink"])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                return Ok(Some(format!("{sink}.monitor")));
            }
            let source = out(&["pactl", "list", "short", "sources"]).and_then(|text| {
                text.lines().find_map(|line| {
                    let name = line.split_whitespace().nth(1)?;
                    name.ends_with(".monitor").then(|| name.to_string())
                })
            });
            source.map(Some).ok_or_else(|| {
                "No system-audio monitor source is available (pactl found none)".to_string()
            })
        }
        AudioMode::Both => {
            // Capture uses a single source; Studio's PipeWire mixer handles Both.
            // Fall back to default microphone here if invoked via legacy capture.
            audio_source(AudioMode::Mic)
        }
    }
}

fn webcam_window_exists() -> bool {
    out(&["swaymsg", "-t", "get_tree", "-r"])
        .map(|tree| tree.contains("\"hayato-webcam\""))
        .unwrap_or(false)
}

fn geometry_rect(geometry: Option<&str>) -> (i64, i64, i64, i64) {
    if let Some(geometry) = geometry
        && let Some((pos, size)) = geometry.split_once(' ')
        && let (Some(x), Some(y)) = (
            pos.split(',').next().and_then(|v| v.trim().parse().ok()),
            pos.split(',').nth(1).and_then(|v| v.trim().parse().ok()),
        )
        && let (Some(w), Some(h)) = (
            size.split('x').next().and_then(|v| v.trim().parse().ok()),
            size.split('x').nth(1).and_then(|v| v.trim().parse().ok()),
        )
    {
        return (x, y, w, h);
    }
    let fallback = (0, 0, 1366, 768);
    let Some(text) = out(&["swaymsg", "-t", "get_outputs", "-r"]) else {
        return fallback;
    };
    let Ok(outputs) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return fallback;
    };
    let Some(output) = outputs
        .iter()
        .find(|output| output["focused"].as_bool().unwrap_or(false))
    else {
        return fallback;
    };
    let rect = &output["rect"];
    (
        rect["x"].as_i64().unwrap_or(0),
        rect["y"].as_i64().unwrap_or(0),
        rect["width"].as_i64().unwrap_or(1366),
        rect["height"].as_i64().unwrap_or(768),
    )
}

fn workspace_origin() -> (i64, i64) {
    let Some(text) = out(&["swaymsg", "-t", "get_workspaces", "-r"]) else {
        return (0, 0);
    };
    let Ok(workspaces) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return (0, 0);
    };
    workspaces
        .iter()
        .find(|workspace| workspace["focused"].as_bool().unwrap_or(false))
        .map(|workspace| {
            (
                workspace["rect"]["x"].as_i64().unwrap_or(0),
                workspace["rect"]["y"].as_i64().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0))
}

fn place_webcam(geometry: Option<&str>) {
    let (x, y, w, h) = geometry_rect(geometry);
    let (wx, wy) = workspace_origin();
    let px = (x + w - 320 - 20 - wx).max(10);
    let py = (y + h - 240 - 20 - wy).max(10);
    let command = format!(
        "[app_id=\"hayato-webcam\"] floating enable, border none, resize set 320 240, move position {px} {py}"
    );
    let _ = Command::new("swaymsg").arg(command).status();
}

/// Discover the V4L2 webcam capture node under `sys_dir` (normally
/// `/sys/class/video4linux`), never assuming `/dev/video0`.
///
/// On MediaTek SoCs (MT8192/MT8183), hardware codec nodes (`mtk-vcodec-enc` /
/// `mtk-vcodec-dec`) can probe before USB UVC and take `/dev/video0`, pushing
/// the webcam to `/dev/video1` (with `/dev/video2` as its UVC metadata node).
fn find_webcam_device_in(sys_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(sys_dir).ok()?;
    let mut nodes: Vec<(u32, String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let num: u32 = name.strip_prefix("video")?.parse().ok()?;
            Some((num, name, entry.path()))
        })
        .collect();
    nodes.sort_by_key(|(num, _, _)| *num);

    let mut fallback: Option<String> = None;
    for (_, name, path) in nodes {
        // UVC cameras expose index 0 for video capture and index 1 for metadata.
        let index = std::fs::read_to_string(path.join("index"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "0".to_string());
        if index != "0" {
            continue;
        }
        let driver = std::fs::canonicalize(path.join("device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if driver == "uvcvideo" {
            return Some(format!("/dev/{name}"));
        }
        let dev_name = std::fs::read_to_string(path.join("name"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let drv_low = driver.to_ascii_lowercase();
        let is_codec = [
            "vcodec", "codec", "m2m", "encoder", "decoder", "isp", "meta",
        ]
        .iter()
        .any(|needle| dev_name.contains(needle) || drv_low.contains(needle));
        if !is_codec && fallback.is_none() {
            fallback = Some(format!("/dev/{name}"));
        }
    }
    fallback
}

fn find_webcam_device() -> Result<String, String> {
    find_webcam_device_in(Path::new("/sys/class/video4linux"))
        .ok_or_else(|| "no webcam device found under /sys/class/video4linux".to_string())
}

/// Start the webcam preview window. It is a normal mpv window, placed as a
/// floating bottom-right overlay; wf-recorder records it as part of the screen.
fn start_webcam(geometry: Option<&str>) -> Result<u32, String> {
    if !crate::system::have("mpv") {
        return Err("mpv is not installed, so the webcam overlay cannot start".to_string());
    }
    let dev = find_webcam_device()?;
    let url = format!("av://v4l2:{dev}");
    let state = hayato_ui::recording::state_dir();
    let _ = std::fs::create_dir_all(&state);
    let log = state.join("camera.log");
    let log_out = File::create(&log).map_err(|error| format!("camera log: {error}"))?;
    let log_err = log_out
        .try_clone()
        .map_err(|error| format!("camera log: {error}"))?;
    let mut command = Command::new("mpv");
    command
        .args([
            "--no-config",
            "--msg-level=all=error",
            "--vo=wlshm",
            "--no-osc",
            "--no-input-default-bindings",
            "--no-border",
            "--ontop",
            "--wayland-app-id=hayato-webcam",
            "--title=Hayato Webcam",
            "--geometry=320x240",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start mpv: {error}"))?;
    let pid = child.id();
    for _ in 0..15 {
        std::thread::sleep(Duration::from_millis(100));
        if webcam_window_exists() {
            break;
        }
    }
    place_webcam(geometry);
    // mpv finishes sizing the surface just after the app id appears; a second
    // placement wins over that resize and leaves the 320x240 overlay.
    std::thread::sleep(Duration::from_millis(300));
    place_webcam(geometry);
    if let Ok(Some(status)) = child.try_wait() {
        let detail = last_line(&log);
        return Err(if detail.is_empty() {
            format!("the webcam overlay exited ({status})")
        } else {
            format!("the webcam overlay exited ({status}): {detail}")
        });
    }
    Ok(pid)
}

fn stop_camera_pid(pid: Option<u32>) {
    if let Some(pid) = pid {
        signal(pid, libc::SIGTERM);
    }
}

fn stop_camera(state: &Path) {
    let path = state.join("camera.pid");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let Ok(pid) = text.trim().parse::<u32>() else {
        return;
    };
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|value| value.trim().to_string())
        .ok();
    if comm.as_deref() == Some("mpv") {
        signal(pid, libc::SIGTERM);
    }
}

/// `hayato menu capture --start [area]`: run detached by the menu.
///
/// The recorder is spawned as its own argv, so the pid saved here is the
/// recorder's and not a wrapper shell's. Nothing reaches the state directory
/// until `/proc/<pid>/comm` says `wf-recorder`, which is what makes Stop's
/// signal trustworthy.
pub fn start_headless(area: bool) -> i32 {
    // Let the menu close before the screen is captured.
    std::thread::sleep(Duration::from_millis(200));
    let geometry = if area {
        match slurp_geometry() {
            Some(geometry) => Some(geometry),
            None => return 0,
        }
    } else {
        None
    };
    let options = hayato_ui::recording::load_options();
    let audio = match audio_source(options.audio) {
        Ok(source) => source,
        Err(error) => {
            announce_start_failure(&error);
            return 1;
        }
    };
    let camera = if options.webcam {
        match start_webcam(geometry.as_deref()) {
            Ok(pid) => Some(pid),
            Err(error) => {
                announce_start_failure(&error);
                return 1;
            }
        }
    } else {
        None
    };
    let state = hayato_ui::recording::state_dir();
    let file = videos_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::create_dir_all(&state);
    let log = state.join("recorder.log");
    let Ok(log_out) = File::create(&log) else {
        stop_camera_pid(camera);
        announce_start_failure("the recorder log could not be opened");
        return 1;
    };
    let Ok(log_err) = log_out.try_clone() else {
        stop_camera_pid(camera);
        announce_start_failure("the recorder log could not be opened");
        return 1;
    };
    let argv = recorder_argv(&file, geometry.as_deref(), audio.as_deref());
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    // The recorder gets its own session: the window may close, and so may the
    // process that started it, without the capture taking the hangup.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            stop_camera_pid(camera);
            hayato_ui::recording::clear_in(&state);
            announce_start_failure(&error.to_string());
            return 1;
        }
    };
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(300));
    if hayato_ui::recording::is_recorder(pid) {
        match write_state(&state, pid, &file) {
            Ok(()) => {
                if let Some(camera) = camera {
                    let _ = std::fs::write(state.join("camera.pid"), format!("{camera}\n"));
                }
                announce_recording(&file, options);
                0
            }
            Err(error) => {
                // A recorder with no saved pid cannot be stopped from the menu,
                // so end it now instead of leaving it running invisibly.
                signal(pid, libc::SIGINT);
                stop_camera_pid(camera);
                hayato_ui::recording::clear_in(&state);
                announce_start_failure(&format!("the recording state could not be saved: {error}"));
                1
            }
        }
    } else {
        stop_camera_pid(camera);
        hayato_ui::recording::clear_in(&state);
        announce_start_failure(&last_line(&log));
        1
    }
}

/// `hayato menu capture --stop-wait PID FILE`: run detached by Stop.
///
/// SIGINT is what finalises an `.mp4`; a recorder that ignores it gets SIGTERM,
/// and one that ignores both is killed and reported as incomplete rather than
/// leaving the session believing a capture is still running. The state is
/// cleared only after the recorder is gone, so the card that says the file was
/// saved is never itself recorded.
pub fn stop_wait(pid: u32, file: &str) -> i32 {
    let path = PathBuf::from(file);
    signal(pid, libc::SIGINT);
    let mut gone = wait_gone(pid, 50);
    if !gone {
        signal(pid, libc::SIGTERM);
        gone = wait_gone(pid, 20);
    }
    let forced = if gone {
        false
    } else {
        signal(pid, libc::SIGKILL);
        true
    };
    let state = hayato_ui::recording::state_dir();
    stop_camera(&state);
    hayato_ui::recording::clear_in(&state);
    announce_stopped(&path, forced);
    0
}

/// `hayato menu capture --status`: `idle`, or `recording<TAB><file>`.
///
/// Read-only, and it trusts the same `/proc/<pid>/comm` check as the menu, so a
/// stale state file never reports a capture.
pub fn status() -> String {
    let state = hayato_ui::recording::state_dir();
    if !hayato_ui::recording::is_recording_in(&state) {
        return "idle".to_string();
    }
    match hayato_ui::recording::file_in(&state) {
        Some(file) => format!("recording\t{}", file.display()),
        None => "recording".to_string(),
    }
}

/// `hayato menu capture` (`hayato-capture`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Capture",
            format!("{said} (grimshot, swappy, tesseract, zbarimg, wf-recorder, slurp, grim)"),
        ));
    }
    let mut screen = CaptureScreen::new();
    let code = crate::run_screen(terminal, design, "Capture", &mut screen)?;
    if screen.open_qr {
        return super::qr::menu(terminal, design);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_needed_tools() {
        assert!(NEEDED.contains(&"grimshot"));
        assert!(NEEDED.contains(&"wf-recorder"));
    }

    #[test]
    fn screenshot_uses_wrapper_in_config_bin() {
        let argv = CaptureScreen::screenshot_argv("/home/brunux", "save", "area");
        assert_eq!(
            argv,
            vec!["/home/brunux/.config/bin/screenshot", "-n", "save", "area"]
        );
    }

    #[test]
    fn the_recorder_is_started_as_its_own_argv() {
        let argv = recorder_argv(Path::new("/home/brunux/Videos/a.mp4"), None, None);
        assert_eq!(argv[0], "wf-recorder");
        assert!(
            !argv.iter().any(|arg| arg == "sh" || arg == "eval"),
            "the recorder is never wrapped in a shell"
        );
        assert!(argv.windows(2).any(|pair| pair == ["-c", "libx264"]));
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["-f", "/home/brunux/Videos/a.mp4"]),
            "the output file is its own argv word"
        );
        assert!(!argv.iter().any(|arg| arg == "-g"));
    }

    #[test]
    fn an_area_recording_passes_the_slurp_geometry_directly() {
        let argv = recorder_argv(Path::new("/tmp/a.mp4"), Some("10,20 300x200"), None);
        let geometry = argv.iter().position(|arg| arg == "-g").unwrap();
        assert_eq!(argv[geometry + 1], "10,20 300x200");
        let file = argv.iter().position(|arg| arg == "-f").unwrap();
        assert!(
            geometry < file,
            "-g is a recorder option, so it comes before the output file"
        );
    }

    #[test]
    fn an_audio_recording_names_the_source_as_one_argument() {
        let argv = recorder_argv(
            Path::new("/tmp/a.mp4"),
            None,
            Some("alsa_input.platform-sound.HiFi__Mic__source"),
        );
        assert!(
            argv.iter()
                .any(|arg| arg == "--audio=alsa_input.platform-sound.HiFi__Mic__source")
        );
        let audio = argv
            .iter()
            .position(|arg| arg.starts_with("--audio="))
            .unwrap();
        let file = argv.iter().position(|arg| arg == "-f").unwrap();
        assert!(audio < file, "audio is a recorder option");
    }

    #[test]
    fn a_path_with_a_space_or_quote_stays_one_argument() {
        let path = Path::new("/home/brunux/My Videos/it's.mp4");
        let argv = recorder_argv(path, None, None);
        assert!(argv.iter().any(|arg| arg == &path.to_string_lossy()));
        assert_eq!(argv.iter().filter(|arg| *arg == "-f").count(), 1);
    }

    #[test]
    fn a_videos_file_is_a_timestamped_mp4_under_home() {
        let file = videos_file();
        assert_eq!(
            file.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("Videos"))
        );
        let name = file.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("hayato-"));
        assert!(name.ends_with(".mp4"));
    }

    #[test]
    fn studio_format_helpers_are_human_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(2_500_000), "2.4 MB");
        assert_eq!(format_bytes(1_500_000_000), "1.4 GB");
        assert_eq!(format_elapsed(0), "00:00:00");
        assert_eq!(format_elapsed(65), "00:01:05");
        assert_eq!(format_elapsed(3661), "01:01:01");
    }

    #[test]
    fn studio_split_panes_adapts_to_terminal_width() {
        let wide = Rect::new(0, 0, 140, 40);
        let (left, right) = CaptureScreen::split_panes(wide);
        assert!(right.is_some());
        assert!(left.width >= 40);
        assert!(right.unwrap().width >= 44);

        let compact = Rect::new(0, 0, 78, 22);
        let (left_c, right_c) = CaptureScreen::split_panes(compact);
        assert_eq!(left_c, compact);
        assert!(right_c.is_none());
    }

    #[test]
    fn find_webcam_skips_vcodec_and_uvc_metadata_nodes() {
        let base = std::env::temp_dir().join(format!(
            "hayato-v4l2-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let drivers = base.join("drivers");
        for drv in ["mtk-vcodec-enc", "mtk-vcodec-dec", "uvcvideo"] {
            std::fs::create_dir_all(drivers.join(drv)).unwrap();
        }
        let sys = base.join("video4linux");
        for (node, name, idx, drv) in [
            ("video0", "mtk-vcodec-enc", "0", "mtk-vcodec-enc"),
            ("video1", "USB2.0 HD UVC WebCam", "0", "uvcvideo"),
            ("video2", "USB2.0 HD UVC WebCam", "1", "uvcvideo"),
            ("video3", "mtk-vcodec-dec", "0", "mtk-vcodec-dec"),
        ] {
            let dir = sys.join(node);
            std::fs::create_dir_all(dir.join("device")).unwrap();
            std::fs::write(dir.join("name"), format!("{name}\n")).unwrap();
            std::fs::write(dir.join("index"), format!("{idx}\n")).unwrap();
            std::os::unix::fs::symlink(drivers.join(drv), dir.join("device/driver")).unwrap();
        }

        assert_eq!(find_webcam_device_in(&sys).as_deref(), Some("/dev/video1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn studio_ctrl_a_and_ctrl_w_toggle_inline_recording_deck() {
        let design = Design::solarized();
        let mut screen = CaptureScreen::new();
        assert!(matches!(screen.phase(), Some("idle") | Some("recording")));
        let before_audio = screen.options.audio;
        let _ = screen.key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &design,
        );
        assert_ne!(screen.options.audio, before_audio);

        let before_cam = screen.options.webcam;
        let _ = screen.key(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            &design,
        );
        assert_ne!(screen.options.webcam, before_cam);
    }
}
