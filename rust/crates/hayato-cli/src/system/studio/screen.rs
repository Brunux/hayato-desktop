//! Main Hayato Studio Deck (`StudioScreen`, `hayato-studio` / `hayato-camera`).
//!
//! 3 clean creative modes with smart defaults:
//! - `[1] Screen Record` (Whole screen / Area, Compact/Balanced/Crisp, live audio mix, PiP)
//! - `[2] Webcam Video` (direct UVC capture + live audio mix + Mirror)
//! - `[3] Photo Snap` (0s/3s/5s pixel-hacker countdown, Mirror, live `mpv.sock` viewfinder,
//!   1-key Copy & Share via Hayato Share, Cancel-first Delete confirm)

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hayato_ui::recording::{self, Options};
use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Header, KeyHint, Outcome, Screen,
    TextStyle, draw_badge, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::digits::{draw_pixel_digits, pixel_width};
use super::{camera_photos_dir, format_bytes, v4l2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudioMode {
    Screen,
    Webcam,
    Photo,
}

enum StudioPhase {
    Idle,
    Countdown { remaining: u8, started: Instant },
    ConfirmDelete { path: PathBuf, confirm: Confirm },
}

#[derive(Clone, Debug)]
struct RecentItem {
    name: String,
    path: PathBuf,
    size: String,
    is_photo: bool,
}

pub struct StudioScreen {
    mode: StudioMode,
    phase: StudioPhase,
    options: Options,
    area: bool,
    webcam_label: String,
    viewfinder_pid: Option<u32>,
    last_snap: Option<PathBuf>,
    recent: Vec<RecentItem>,
    status_line: String,
    header: Header,
    keybar: Vec<KeyHint>,
}

impl Drop for StudioScreen {
    fn drop(&mut self) {
        if let Some(pid) = self.viewfinder_pid.take() {
            super::stop_pid_if_comm(pid, &["mpv"]);
        }
    }
}

impl StudioScreen {
    pub fn new(mode: StudioMode) -> Self {
        let options = recording::load_options();
        let webcam_label = v4l2::find_webcam()
            .map(|w| format!("{} ({})", w.name, w.device))
            .unwrap_or_else(|| "No webcam detected".to_string());
        let recent = scan_recent();
        let mut s = Self {
            mode,
            phase: StudioPhase::Idle,
            options,
            area: false,
            webcam_label,
            viewfinder_pid: None,
            last_snap: None,
            recent,
            status_line: "Ready · Enter starts · [1-3] switch mode".to_string(),
            header: Header::new("Hayato").subtitle("Studio"),
            keybar: Vec::new(),
        };
        s.sync_chrome();
        s
    }

    #[cfg(test)]
    pub fn for_test(mode: StudioMode) -> Self {
        let mut s = Self {
            mode,
            phase: StudioPhase::Idle,
            options: Options::default(),
            area: false,
            webcam_label: "USB2.0 HD UVC (/dev/video1)".to_string(),
            viewfinder_pid: None,
            last_snap: None,
            recent: Vec::new(),
            status_line: "Ready".to_string(),
            header: Header::new("Hayato").subtitle("Studio"),
            keybar: Vec::new(),
        };
        s.sync_chrome();
        s
    }

    fn save_opts(&self) {
        let _ = recording::save_options(self.options);
    }

    fn sync_chrome(&mut self) {
        self.header = Header::new("Hayato")
            .subtitle("Studio")
            .status(format!("● READY · {}", self.webcam_label));
        self.keybar = match self.mode {
            StudioMode::Screen => vec![
                KeyHint::new("1-3", "mode"),
                KeyHint::new("Enter", "record"),
                KeyHint::new("t", "target"),
                KeyHint::new("q", "quality"),
                KeyHint::new("a", "audio"),
                KeyHint::new("w", "cam"),
                KeyHint::new("c", "corner"),
                KeyHint::new("^S", "share"),
                KeyHint::new("Esc", "close"),
            ],
            StudioMode::Webcam => vec![
                KeyHint::new("1-3", "mode"),
                KeyHint::new("Enter", "record"),
                KeyHint::new("q", "quality"),
                KeyHint::new("a", "audio"),
                KeyHint::new("m", "mirror"),
                KeyHint::new("^S", "share"),
                KeyHint::new("Esc", "close"),
            ],
            StudioMode::Photo => vec![
                KeyHint::new("1-3", "mode"),
                KeyHint::new("Space", "snap"),
                KeyHint::new("t", "timer"),
                KeyHint::new("m", "mirror"),
                KeyHint::new("v", "viewfinder"),
                KeyHint::new("^S", "share"),
                KeyHint::new("d", "delete"),
                KeyHint::new("Esc", "close"),
            ],
        };
    }

    fn do_snap(&mut self) {
        match super::snap_photo(self.options.mirror) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.last_snap = Some(path);
                self.recent = scan_recent();
                self.status_line = format!("Snapped {name} · Ctrl+S shares · Enter opens");
            }
            Err(e) => {
                self.status_line = format!("Snap failed: {e}");
            }
        }
    }

    fn toggle_viewfinder(&mut self, reduced_motion: bool) {
        if let Some(pid) = self.viewfinder_pid.take() {
            super::stop_pid_if_comm(pid, &["mpv"]);
            self.status_line = "Closed live viewfinder".to_string();
        } else {
            match super::start_webcam_overlay(
                self.options.corner,
                self.options.pip_size,
                self.options.sticky,
                self.options.mirror,
                reduced_motion,
            ) {
                Ok(pid) => {
                    self.viewfinder_pid = Some(pid);
                    self.status_line = "Live viewfinder active · Space snaps in <30ms".to_string();
                }
                Err(e) => self.status_line = format!("Viewfinder failed: {e}"),
            }
        }
    }

    fn draw_tabs(&self, ctx: &mut BodyCtx, x: u16, y: u16, max_w: u16) {
        let mut cx = x;
        for (num, mode, label) in [
            ("1", StudioMode::Screen, "Screen Record"),
            ("2", StudioMode::Webcam, "Webcam Video"),
            ("3", StudioMode::Photo, "Photo Snap"),
        ] {
            let active = self.mode == mode;
            let text = format!(" [{num}] {label} ");
            let st = if active {
                Style::default()
                    .fg(ctx.design.color(ColorRole::Bg))
                    .bg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Dim)
            };
            let w = text.len() as u16;
            if cx + w <= x + max_w {
                let _ = put(ctx.buf, cx, y, &text, w, st);
                cx += w + 1;
            }
        }
    }
}

impl Screen for StudioScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((1060, 620))
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.phase {
            StudioPhase::Idle => match self.mode {
                StudioMode::Screen => "studio.screen",
                StudioMode::Webcam => "studio.webcam",
                StudioMode::Photo => "studio.photo",
            },
            StudioPhase::Countdown { .. } => "studio.countdown",
            StudioPhase::ConfirmDelete { .. } => "studio.confirm_delete",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        match self.phase {
            StudioPhase::Countdown { .. } => Some(Duration::from_millis(100)),
            _ => None,
        }
    }

    fn tick(&mut self) -> Outcome {
        if let StudioPhase::Countdown { remaining, started } = self.phase {
            let elapsed = started.elapsed().as_secs() as u8;
            if elapsed >= remaining {
                self.phase = StudioPhase::Idle;
                self.do_snap();
            }
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let StudioPhase::ConfirmDelete { confirm, .. } = &mut self.phase {
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }

        if let StudioPhase::Countdown { remaining, started } = self.phase {
            let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("Photo Snap"), true);
            let left = remaining.saturating_sub(started.elapsed().as_secs() as u8).max(1);
            let txt = left.to_string();
            let pw = pixel_width(&txt);
            let cx = inner.x + inner.width.saturating_sub(pw) / 2;
            let cy = inner.y + inner.height.saturating_sub(5) / 2;
            draw_pixel_digits(
                ctx.buf,
                cx,
                cy,
                pw,
                &txt,
                Style::default()
                    .fg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD),
            );
            let msg = format!("Capturing in {left}\u{2026} (Esc cancels)");
            let mx = inner.x + inner.width.saturating_sub(msg.len() as u16) / 2;
            let _ = put_trunc(
                ctx.buf,
                mx,
                cy + 4,
                &msg,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
            return;
        }

        if ctx.area.height < 10 || ctx.area.width < 40 {
            return;
        }
        self.draw_tabs(ctx, ctx.area.x, ctx.area.y, ctx.area.width);

        let body = Rect::new(
            ctx.area.x,
            ctx.area.y + 2,
            ctx.area.width,
            ctx.area.height.saturating_sub(2),
        );
        let wide = body.width >= 96;
        let (left_area, right_area) = if wide {
            let lw = (body.width * 54) / 100;
            (
                Rect::new(body.x, body.y, lw, body.height),
                Some(Rect::new(
                    body.x + lw + 1,
                    body.y,
                    body.width.saturating_sub(lw + 1),
                    body.height,
                )),
            )
        } else {
            (body, None)
        };

        let title = match self.mode {
            StudioMode::Screen => "Screen Record Deck",
            StudioMode::Webcam => "Webcam Video Deck",
            StudioMode::Photo => "Photo Snap Deck",
        };
        let inner = draw_card(ctx.buf, left_area, ctx.design, Some(title), true);
        let mut y = inner.y;

        match self.mode {
            StudioMode::Screen => {
                let target_line = if self.area {
                    "[t] Target  : ○ Whole screen   ● Area (drag slurp)"
                } else {
                    "[t] Target  : ● Whole screen   ○ Area (drag slurp)"
                };
                let _ = put_trunc(ctx.buf, inner.x, y, target_line, inner.width, ctx.design.style(TextStyle::Body));
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[q] Quality : {}", self.options.quality.label()),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[a] Audio   : {} (live-switchable)", self.options.audio.short_label()),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let cam_st = if self.options.webcam { "ON" } else { "OFF" };
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!(
                        "[w] Webcam  : [{cam_st}] · [c] Corner: {} · [z] Size: {}",
                        self.options.corner.short(),
                        self.options.pip_size.as_str().to_uppercase()
                    ),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 1;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!(
                        "[s] Sticky  : {} · [m] Mirror: {}",
                        if self.options.sticky { "ALL WS" } else { "CURRENT WS" },
                        if self.options.mirror { "ON" } else { "OFF" }
                    ),
                    inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
                y += 2;
                if y < inner.bottom().saturating_sub(1) {
                    let _ = draw_badge(ctx.buf, inner.x, y, ctx.design, "  [ Enter ]  ● START SCREEN RECORDING  ", ColorRole::Danger);
                }
            }
            StudioMode::Webcam => {
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("Camera      : {}", self.webcam_label),
                    inner.width,
                    ctx.design.style(TextStyle::Code),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[q] Quality : {}", self.options.quality.label()),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[a] Audio   : {} (live-switchable)", self.options.audio.short_label()),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[m] Mirror  : {}", if self.options.mirror { "ON" } else { "OFF" }),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                if y < inner.bottom().saturating_sub(1) {
                    let _ = draw_badge(ctx.buf, inner.x, y, ctx.design, "  [ Enter ]  ● START WEBCAM RECORDING  ", ColorRole::Danger);
                }
            }
            StudioMode::Photo => {
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("Camera      : {}", self.webcam_label),
                    inner.width,
                    ctx.design.style(TextStyle::Code),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[t] Timer   : {}s (0s / 3s / 5s)", self.options.timer_secs),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!("[m] Mirror  : {}", if self.options.mirror { "ON" } else { "OFF" }),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    y,
                    &format!(
                        "[v] View    : {}",
                        if self.viewfinder_pid.is_some() {
                            "LIVE VIEWFINDER ON"
                        } else {
                            "Off (headless snap)"
                        }
                    ),
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
                y += 2;
                if y < inner.bottom().saturating_sub(1) {
                    let _ = draw_badge(ctx.buf, inner.x, y, ctx.design, "  [ Space ]  SNAP PHOTO  ", ColorRole::Accent);
                }
            }
        }

        let footer_y = inner.bottom().saturating_sub(1);
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            footer_y,
            &self.status_line,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );

        if let Some(right) = right_area {
            let r_in = draw_card(ctx.buf, right, ctx.design, Some("Recent Captures"), false);
            let mut ry = r_in.y;
            if self.recent.is_empty() {
                let _ = put_trunc(
                    ctx.buf,
                    r_in.x,
                    ry,
                    "No recordings or photos yet.",
                    r_in.width,
                    ctx.design.style(TextStyle::Dim),
                );
            } else {
                for item in self.recent.iter().take(8) {
                    if ry + 1 >= r_in.bottom() {
                        break;
                    }
                    let tag = if item.is_photo { "[IMG]" } else { "[VID]" };
                    let line = format!("{tag} {} ({})", item.name, item.size);
                    let _ = put_trunc(ctx.buf, r_in.x, ry, &line, r_in.width, ctx.design.style(TextStyle::Body));
                    ry += 1;
                }
            }
            if r_in.height >= 4 {
                let _ = put_trunc(
                    ctx.buf,
                    r_in.x,
                    r_in.bottom().saturating_sub(1),
                    "Ctrl+S shares latest · Ctrl+O opens folder",
                    r_in.width,
                    ctx.design.style(TextStyle::Dim),
                );
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let StudioPhase::ConfirmDelete { path, confirm } = &mut self.phase {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    let p = path.clone();
                    let _ = std::fs::remove_file(&p);
                    self.last_snap = None;
                    self.recent = scan_recent();
                    self.status_line = "Deleted last photo".to_string();
                    self.phase = StudioPhase::Idle;
                    return Outcome::Consumed;
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = StudioPhase::Idle;
                    return Outcome::Consumed;
                }
                ConfirmAction::None => return Outcome::Consumed,
            }
        }
        if matches!(self.phase, StudioPhase::Countdown { .. }) {
            if key.code == KeyCode::Esc {
                self.phase = StudioPhase::Idle;
                self.status_line = "Countdown cancelled".to_string();
            }
            return Outcome::Consumed;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    let target = self
                        .last_snap
                        .clone()
                        .or_else(|| self.recent.first().map(|r| r.path.clone()));
                    if let Some(p) = target {
                        let _ = crate::system::share::spawn_detached(
                            &crate::system::share::SharePayload::from_path(p),
                        );
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    self.status_line = "No capture to share yet".to_string();
                    return Outcome::Consumed;
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    let dir = if self.mode == StudioMode::Photo {
                        camera_photos_dir()
                    } else {
                        dirs::video_dir().unwrap_or_else(camera_photos_dir)
                    };
                    let _ = std::fs::create_dir_all(&dir);
                    let s = dir.to_string_lossy().into_owned();
                    crate::system::spawn_detached_argv("hayato-files", &["--", &s]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Char('1') => {
                self.mode = StudioMode::Screen;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('2') => {
                self.mode = StudioMode::Webcam;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('3') => {
                self.mode = StudioMode::Photo;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('q') => {
                self.options.quality = self.options.quality.cycle();
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('a') => {
                self.options.audio = self.options.audio.cycle();
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('w') => {
                self.options.webcam = !self.options.webcam;
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('c') => {
                self.options.corner = self.options.corner.cycle();
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('z') => {
                self.options.pip_size = self.options.pip_size.cycle();
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('s') => {
                self.options.sticky = !self.options.sticky;
                self.save_opts();
                return Outcome::Consumed;
            }
            KeyCode::Char('m') => {
                self.options.mirror = !self.options.mirror;
                self.save_opts();
                if self.viewfinder_pid.is_some() {
                    let _ = super::mpv_toggle_mirror();
                }
                return Outcome::Consumed;
            }
            KeyCode::Char('t') => {
                if self.mode == StudioMode::Photo {
                    self.options.timer_secs = match self.options.timer_secs {
                        0 => 3,
                        3 => 5,
                        _ => 0,
                    };
                    self.save_opts();
                } else if self.mode == StudioMode::Screen {
                    self.area = !self.area;
                }
                return Outcome::Consumed;
            }
            KeyCode::Char('v') if self.mode == StudioMode::Photo => {
                self.toggle_viewfinder(design.reduced_motion());
                return Outcome::Consumed;
            }
            KeyCode::Char('d') if self.mode == StudioMode::Photo => {
                if let Some(p) = self.last_snap.clone() {
                    self.phase = StudioPhase::ConfirmDelete {
                        path: p,
                        confirm: Confirm::new("Delete last photo?", "Delete")
                            .with_detail("This permanently removes the photo from ~/Pictures/Camera.")
                            .destructive(true),
                    };
                }
                return Outcome::Consumed;
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.mode {
                StudioMode::Screen => {
                    self.save_opts();
                    if let Some(pid) = self.viewfinder_pid.take() {
                        super::stop_pid_if_comm(pid, &["mpv"]);
                    }
                    super::trigger_start("screen", self.area);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                StudioMode::Webcam => {
                    self.save_opts();
                    if let Some(pid) = self.viewfinder_pid.take() {
                        super::stop_pid_if_comm(pid, &["mpv"]);
                    }
                    super::trigger_start("webcam", false);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                StudioMode::Photo => {
                    if self.options.timer_secs > 0 {
                        self.phase = StudioPhase::Countdown {
                            remaining: self.options.timer_secs,
                            started: Instant::now(),
                        };
                    } else {
                        self.do_snap();
                    }
                    return Outcome::Consumed;
                }
            },
            _ => {}
        }
        Outcome::Continue
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }
}

fn scan_recent() -> Vec<RecentItem> {
    let mut found = Vec::new();
    let dirs = [
        dirs::video_dir().unwrap_or_default(),
        camera_photos_dir(),
    ];
    for dir in dirs {
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
            let is_photo = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp");
            let is_video = matches!(ext.as_str(), "mp4" | "mkv" | "webm");
            if !is_photo && !is_video {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.push((
                modified,
                RecentItem {
                    name,
                    path,
                    size: format_bytes(meta.len()),
                    is_photo,
                },
            ));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().take(8).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayato_ui::{App, Header};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn studio_screen_renders_all_modes_at_compact_and_wide() {
        for mode in [StudioMode::Screen, StudioMode::Webcam, StudioMode::Photo] {
            for (w, h) in [(80, 24), (152, 45)] {
                let mut screen = StudioScreen::for_test(mode);
                let mut app = App::new(Header::new("Hayato"))
                    .design(Design::solarized().with_reduced_motion(true))
                    .screen(&mut screen);
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| app.render(f)).unwrap();
                let rendered = hayato_ui::screen(term.backend().buffer());
                assert!(rendered.contains("Screen Record"), "{mode:?} {w}x{h}");
            }
        }
    }
}
