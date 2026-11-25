//! Live-Recording Quick Control HUD (`LiveHudScreen`, `hayato studio --hud`).
//!
//! Triggered by left-clicking the minimal Waybar REC icon (or pressing
//! `Alt+Ctrl+R` while recording). Gives instant live controls without
//! interrupting the recording:
//! - Live PipeWire audio switching & mixing (`Off` / `Mic` / `System` / `Both`).
//! - Animated 60 FPS webcam PiP corner gliding (`TL` / `TR` / `BL` / `BR` / `CTR`)
//!   + size (`S`/`M`/`L`), Hide/Show, Sticky across workspaces, and Mirror toggle.
//! - Active fullscreen window warning + 1-key `[u] Unfullscreen`.
//! - `[Space] Stop Recording` or `[Esc] Hide HUD`.

use std::time::Duration;

use hayato_ui::recording::{self, AudioMode, PipCorner, PipSize, StudioSession};
use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Header, KeyHint, Outcome, Screen, TextStyle, draw_badge, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::digits::draw_pixel_digits;
use super::{format_bytes, format_elapsed, now_epoch, sway_ipc};

pub struct LiveHudScreen {
    session: StudioSession,
    elapsed_str: String,
    size_str: String,
    fullscreen_active: bool,
    status_note: String,
    header: Header,
    keybar: Vec<KeyHint>,
    pulse_on: bool,
}

impl LiveHudScreen {
    pub fn new(session: StudioSession) -> Self {
        let mut s = Self {
            session,
            elapsed_str: "00:00:00".to_string(),
            size_str: "0 B".to_string(),
            fullscreen_active: false,
            status_note: "Live controls · Esc hides HUD without stopping".to_string(),
            header: Header::new("Hayato").subtitle("REC LIVE"),
            keybar: vec![
                KeyHint::new("1-4", "audio"),
                KeyHint::new("v", "cam"),
                KeyHint::new("c", "corner"),
                KeyHint::new("s", "sticky"),
                KeyHint::new("Space", "stop"),
                KeyHint::new("Esc", "hide"),
            ],
            pulse_on: true,
        };
        s.refresh_telemetry();
        s
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        use hayato_ui::recording::QualityPreset;
        use std::path::PathBuf;
        Self::new(StudioSession {
            kind: "screen".to_string(),
            pid: 99999,
            comm: "wf-recorder".to_string(),
            file: PathBuf::from("/home/brunux/Videos/hayato-screen-test.mp4"),
            started_epoch: now_epoch().saturating_sub(252),
            quality: QualityPreset::Balanced,
            audio: AudioMode::Both,
            geometry: None,
            cam_pid: Some(99998),
            cam_visible: true,
            cam_sticky: true,
            cam_corner: PipCorner::BottomRight,
            cam_size: PipSize::Medium,
            cam_mirror: true,
        })
    }

    fn refresh_telemetry(&mut self) {
        if let Some(live) = recording::read_studio_session() {
            self.session = live;
        }
        let secs = now_epoch().saturating_sub(self.session.started_epoch);
        self.elapsed_str = format_elapsed(secs);
        self.size_str = self
            .session
            .file
            .metadata()
            .map(|m| format_bytes(m.len()))
            .unwrap_or_else(|_| "0 B".to_string());
        self.fullscreen_active = sway_ipc::has_active_fullscreen();
        self.header = Header::new("Hayato")
            .subtitle("REC LIVE")
            .status(format!("{} · {}", self.elapsed_str, self.size_str));
    }

    fn draw_mini_monitor(&self, ctx: &mut BodyCtx, x: u16, y: u16) {
        let border_st = ctx.design.style(TextStyle::Dim);
        let cam_st = if self.session.cam_visible {
            Style::default()
                .fg(ctx.design.color(ColorRole::Bg))
                .bg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            ctx.design.style(TextStyle::Dim)
        };
        let _ = put(ctx.buf, x, y, "╭────────────────╮", 18, border_st);
        for row in 1..=4 {
            let _ = put(ctx.buf, x, y + row, "│                │", 18, border_st);
        }
        let _ = put(ctx.buf, x, y + 5, "╰────────────────╯", 18, border_st);

        let (cx, cy) = match self.session.cam_corner {
            PipCorner::TopLeft => (x + 2, y + 1),
            PipCorner::TopRight => (x + 11, y + 1),
            PipCorner::BottomLeft => (x + 2, y + 4),
            PipCorner::BottomRight => (x + 11, y + 4),
            PipCorner::Center => (x + 6, y + 2),
        };
        let label = if self.session.cam_visible {
            " CAM "
        } else {
            " OFF "
        };
        let _ = put(ctx.buf, cx, cy, label, 5, cam_st);
    }
}

impl Screen for LiveHudScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((580, 390))
    }

    fn phase(&self) -> Option<&'static str> {
        Some("studio.hud")
    }

    fn needs_tick(&self) -> Option<Duration> {
        Some(Duration::from_millis(500))
    }

    fn tick(&mut self) -> Outcome {
        self.pulse_on = !self.pulse_on;
        self.refresh_telemetry();
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("REC // LIVE HUD"), true);
        if inner.width < 30 || inner.height < 12 {
            return;
        }

        // Row 0..2: Pulsing REC badge + big 3-row pixel timer + mode/size
        let dot_role = if self.pulse_on || ctx.design.reduced_motion() {
            ColorRole::Danger
        } else {
            ColorRole::Warn
        };
        let bx = draw_badge(ctx.buf, inner.x, inner.y, ctx.design, "● REC", dot_role);
        let digit_style = Style::default()
            .fg(ctx.design.color(ColorRole::Danger))
            .add_modifier(Modifier::BOLD);
        draw_pixel_digits(
            ctx.buf,
            bx + 2,
            inner.y,
            inner.width.saturating_sub(bx - inner.x + 2),
            &self.elapsed_str,
            digit_style,
        );

        let meta_y = inner.y + 3;
        let mode_str = format!(
            "{} ({}) · {}",
            if self.session.kind == "webcam" {
                "Webcam"
            } else {
                "Screen"
            },
            self.session.quality.as_str(),
            self.size_str
        );
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            meta_y,
            &mode_str,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );

        // Audio mixer section
        let aud_y = meta_y + 2;
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            aud_y,
            "AUDIO MIX (live · zero restart)",
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
        let mut ax = inner.x;
        for (key_lbl, mode, name) in [
            ("1", AudioMode::Off, "Off"),
            ("2", AudioMode::Mic, "Mic"),
            ("3", AudioMode::System, "System"),
            ("4", AudioMode::Both, "Both"),
        ] {
            let active = self.session.audio == mode;
            let pill = format!("[{key_lbl} {name}]");
            let st = if active {
                Style::default()
                    .fg(ctx.design.color(ColorRole::Bg))
                    .bg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Body)
            };
            let w = pill.len() as u16;
            if ax + w <= inner.right() {
                let _ = put(ctx.buf, ax, aud_y + 1, &pill, w, st);
                ax += w + 1;
            }
        }

        // Webcam PiP controls + mini-monitor preview
        let cam_y = aud_y + 3;
        if cam_y + 5 < inner.bottom() {
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                cam_y,
                "WEBCAM PiP (animated glide)",
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
            let vis = if self.session.cam_visible {
                "● SHOWN"
            } else {
                "○ HIDDEN"
            };
            let sticky = if self.session.cam_sticky {
                "● ALL WS"
            } else {
                "○ THIS WS"
            };
            let mir = if self.session.cam_mirror {
                "● ON"
            } else {
                "○ OFF"
            };
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                cam_y + 1,
                &format!("[v] Cam: {vis}  [s] Sticky: {sticky}"),
                inner.width.saturating_sub(20),
                ctx.design.style(TextStyle::Body),
            );
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                cam_y + 2,
                &format!(
                    "[m] Mirror: {mir}  [z] Size: {}",
                    self.session.cam_size.as_str().to_uppercase()
                ),
                inner.width.saturating_sub(20),
                ctx.design.style(TextStyle::Body),
            );
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                cam_y + 3,
                &format!(
                    "[c] Corner: {} (TL/TR/BL/BR/CTR)",
                    self.session.cam_corner.short()
                ),
                inner.width.saturating_sub(20),
                ctx.design.style(TextStyle::Code),
            );

            if inner.width >= 48 {
                self.draw_mini_monitor(ctx, inner.right().saturating_sub(18), cam_y);
            }
        }

        let footer_y = inner.bottom().saturating_sub(1);
        if self.fullscreen_active && footer_y > cam_y + 5 {
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                footer_y - 1,
                "▲ Fullscreen window covers PiP · [u] Unfullscreen",
                inner.width,
                Style::default()
                    .fg(ctx.design.color(ColorRole::Warn))
                    .add_modifier(Modifier::BOLD),
            );
        }
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            footer_y,
            &self.status_note,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let reduced = design.reduced_motion();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Char(' ') | KeyCode::Enter => {
                super::trigger_stop();
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            KeyCode::Char('1') => {
                super::set_audio_live(AudioMode::Off);
                self.session.audio = AudioMode::Off;
                self.status_note = "Audio muted (live)".to_string();
                Outcome::Consumed
            }
            KeyCode::Char('2') => {
                super::set_audio_live(AudioMode::Mic);
                self.session.audio = AudioMode::Mic;
                self.status_note = "Audio switched to Mic (live)".to_string();
                Outcome::Consumed
            }
            KeyCode::Char('3') => {
                super::set_audio_live(AudioMode::System);
                self.session.audio = AudioMode::System;
                self.status_note = "Audio switched to System line-out (live)".to_string();
                Outcome::Consumed
            }
            KeyCode::Char('4') => {
                super::set_audio_live(AudioMode::Both);
                self.session.audio = AudioMode::Both;
                self.status_note = "Audio mixed: Mic + System (live)".to_string();
                Outcome::Consumed
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                super::toggle_pip_visible(reduced);
                self.refresh_telemetry();
                self.status_note = if self.session.cam_visible {
                    "Webcam PiP shown".to_string()
                } else {
                    "Webcam PiP hidden (stream kept warm)".to_string()
                };
                Outcome::Consumed
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                super::toggle_pip_sticky();
                self.refresh_telemetry();
                self.status_note = if self.session.cam_sticky {
                    "Webcam pinned across all workspaces".to_string()
                } else {
                    "Webcam kept on current workspace".to_string()
                };
                Outcome::Consumed
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                super::toggle_pip_mirror();
                self.refresh_telemetry();
                self.status_note = "Toggled webcam mirror".to_string();
                Outcome::Consumed
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                let next = self.session.cam_corner.cycle();
                self.session.cam_corner = next;
                super::move_pip_live(next, self.session.cam_size, reduced);
                self.status_note = format!("Glided webcam to {}", next.short());
                Outcome::Consumed
            }
            KeyCode::Char('z') | KeyCode::Char('Z') => {
                let next = self.session.cam_size.cycle();
                self.session.cam_size = next;
                super::move_pip_live(self.session.cam_corner, next, reduced);
                self.status_note = format!("Resized webcam to {}", next.as_str().to_uppercase());
                Outcome::Consumed
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                sway_ipc::unfullscreen_active();
                self.fullscreen_active = false;
                self.status_note = "Exited fullscreen so PiP stays on top".to_string();
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayato_ui::{App, Header};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn live_hud_renders_cleanly() {
        let mut screen = LiveHudScreen::for_test();
        let mut app = App::new(Header::new("Hayato"))
            .design(Design::solarized().with_reduced_motion(true))
            .screen(&mut screen);
        let mut term = Terminal::new(TestBackend::new(66, 20)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let out = hayato_ui::screen(term.backend().buffer());
        assert!(out.contains("REC // LIVE HUD"));
        assert!(out.contains("AUDIO MIX"));
        assert!(out.contains("WEBCAM PiP"));
    }
}
