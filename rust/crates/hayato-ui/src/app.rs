//! The shell: header, body and key bar, one window shape for every command.
//!
//! A command implements [`Screen`] and gets the frame, the keys, the mouse and
//! the motion without touching a `Frame` itself (the check in
//! `scripts/check-system.sh` greps `hayato-cli` for exactly that). Three
//! decisions live here:
//!
//! - **Event-driven**: the loop draws once per event, and only while an
//!   animation is running does it wake on a 10 fps tick. An idle window costs
//!   zero wakeups (`optimization.md` §13: 154 µs per frame is ratatui's floor,
//!   so the budget is kept by not drawing).
//! - **The screen answers first**: Escape means "cancel, exit 1" by default, but
//!   a screen that owns the key (a confirmation, the failure card) gets it
//!   first - otherwise the failure card would exit 1 instead of 3.
//! - **Motion is bounded and optional**: at most [`MOTION_MS`], skipped when
//!   `HAYATO_REDUCED_MOTION=1`, and it never loops.

use std::io;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::Style;
use tachyonfx::{Effect, EffectTimer, Interpolation, Motion as Sweep, fx};

use crate::design::{ColorRole, Design};
use crate::widgets::{self, Header, KeyHint, draw_failure, draw_header, draw_keybar};

/// The shell's exit codes, and every surface's contract
/// (`specs/001-core.md` §Design): a shell binding writes `|| exit 0` for Escape.
pub mod exit {
    pub const OK: i32 = 0;
    pub const ESCAPE: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const FAILED: i32 = 3;
}

/// What the loop does after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    /// The screen used the key itself, so the shell must not act on it too:
    /// Escape that steps a menu back from a confirm is not Escape that closes
    /// the window (spec 007).
    Consumed,
    Done(i32),
}

/// What a body receives: its area, the buffer, the design. Never a `Frame`, so
/// a command cannot draw the header, the key bar or the backdrop.
pub struct BodyCtx<'a> {
    pub area: Rect,
    pub buf: &'a mut Buffer,
    pub design: &'a Design,
}

/// One command's body: what it draws, and what it does with a key or a click.
pub trait Screen {
    fn draw(&mut self, ctx: &mut BodyCtx);

    /// The first look at a key. Returning [`Outcome::Continue`] hands it to the
    /// shell (Escape, Ctrl-C); [`Outcome::Consumed`] keeps it from the shell;
    /// [`Outcome::Done`] ends the window.
    ///
    /// The design is passed rather than remembered: the row stride and the bands
    /// come from the density, and a screen that cached its own copy is a hit
    /// region that disagrees with the row it was drawn on.
    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        let _ = (key, design);
        Outcome::Continue
    }

    /// A click on `row` (the index the screen's [`Screen::hit`] returned), or on
    /// empty space when `None`.
    fn click(&mut self, row: Option<usize>) -> Outcome {
        let _ = row;
        Outcome::Continue
    }

    /// Which row is under `(x, y)`; `None` for blank space.
    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let _ = (area, x, y, design);
        None
    }

    /// Keys shown on the bottom band. A screen with no key bar uses the one set
    /// on [`App`]; this lets a task surface change its actions as its state
    /// changes. Returning a borrowed slice keeps a frame from allocating just
    /// to ask what the keys are (`tests/no_alloc.rs`).
    fn keybar(&self) -> Option<&[KeyHint]> {
        None
    }

    /// The top band, for a screen whose subject changes while it is open: the
    /// root menu becomes the power list, then a question to an agent, and the
    /// header has to say which (spec 007 marks the Ask strip `AI` there). A
    /// screen with no header of its own keeps the one set on [`App`].
    fn header(&self) -> Option<&Header> {
        None
    }

    /// How often the loop should wake this screen while nothing else happens.
    /// `None` means event-driven only; a running task wakes at 10 fps for its
    /// spinner and its stop timeout.
    fn needs_tick(&self) -> Option<Duration> {
        None
    }

    /// Called when the poll timeout expires. The screen may advance a spinner,
    /// check on a child process, or decide the task has ended.
    fn tick(&mut self) -> Outcome {
        Outcome::Continue
    }

    /// Dynamic design override for screens that change their palette live (e.g. theme picker).
    /// If `Some(design)` is returned, the window shell's background, header, and keybar
    /// adapt to it dynamically.
    fn design(&self) -> Option<Design> {
        None
    }

    /// Optional target window pixel dimensions `(width_px, height_px)`.
    /// When `Some((w, h))` changes during the session (for example when switching
    /// from a network list to an inline Wi-Fi QR view, or expanding a summary list
    /// into a log/manual viewer or `TaskView`), `App::run_until` dynamically resizes
    /// and re-centers the floating `foot` window in Sway.
    fn desired_size(&self) -> Option<(u16, u16)> {
        None
    }

    /// Non-key, non-left-click terminal events (focus gained/lost, mouse scroll,
    /// paste, resize). Screens that do not need them keep the default no-op.
    fn event(&mut self, event: &Event, design: &Design) -> Outcome {
        let _ = (event, design);
        Outcome::Continue
    }

    /// A short, stable name for the screen's current phase, for tests, the
    /// opt-in state log and an agent reading the surface. `None` when the
    /// screen has no phase (a one-shot list or a card).
    ///
    /// The label is part of the surface's contract: keep it lowercase, stable
    /// and free of user-facing copy, so a log line can be matched by a test or
    /// an agent without parsing the screen.
    fn phase(&self) -> Option<&'static str> {
        None
    }
}

/// Resolve the active Sway IPC socket path, preferring `SWAYSOCK` when it exists
/// on disk and falling back to discovering `sway-ipc.*.sock` in `$XDG_RUNTIME_DIR`.
fn resolve_sway_socket() -> Option<std::path::PathBuf> {
    if let Ok(sock) = std::env::var("SWAYSOCK") {
        let p = std::path::PathBuf::from(&sock);
        if p.exists() {
            return Some(p);
        }
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let entries = std::fs::read_dir(runtime).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("sway-ipc.") && s.ends_with(".sock") {
            return Some(entry.path());
        }
    }
    None
}

/// Dynamically resize and re-center the hosting floating `foot` window in Sway
/// after the surface has already mapped.
///
/// Safe in tests: does nothing unless `HAYATO_UI=1` is set in the environment.
pub fn request_window_size(width_px: u16, height_px: u16) {
    if std::env::var_os("HAYATO_UI").is_none() {
        return;
    }
    let w = width_px.clamp(480, 1600);
    let h = height_px.clamp(280, 1050);

    if let Some(sock) = resolve_sway_socket() {
        let ppid = unsafe { libc::getppid() };
        let rule = format!(
            "[pid={ppid}] floating enable, resize set width {w} px height {h} px, move position center"
        );
        let _ = std::process::Command::new("swaymsg")
            .arg("-s")
            .arg(sock)
            .arg(rule)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[4;{h};{w}t");
        let _ = out.flush();
    }
}

/// How long an opening animation may run.
pub const MOTION_MS: u64 = 300;

/// The tick an animation runs at: 10 fps, which is what the plan budgets for.
pub const TICK: Duration = Duration::from_millis(100);

/// The opening animation, bounded and skippable.
pub struct Motion {
    effect: Option<Effect>,
    last: Instant,
    enabled: bool,
}

impl Motion {
    pub fn new(enabled: bool) -> Self {
        Self {
            effect: None,
            last: Instant::now(),
            enabled,
        }
    }

    pub fn start(&mut self, design: &Design) {
        if !self.enabled {
            return;
        }
        let faded = design.color(ColorRole::Bg);
        self.effect = Some(fx::sweep_in(
            Sweep::LeftToRight,
            12,
            0,
            faded,
            EffectTimer::from_ms(MOTION_MS as u32, Interpolation::QuadOut),
        ));
        self.last = Instant::now();
    }

    pub fn running(&self) -> bool {
        self.effect.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Advance by what has actually elapsed, not by the tick: a frame that was
    /// slow must not stretch the effect past its ceiling.
    pub fn apply(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(effect) = self.effect.as_mut() else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last).min(TICK);
        self.last = now;
        effect.process(elapsed.into(), buf, area);
        if effect.done() {
            self.effect = None;
        }
    }
}

/// Where `HAYATO_DEBUG_STATE=1` writes phase changes. A file, not stderr:
/// the window owns the terminal, and a debug line printed into it would paint
/// over the frame it is describing.
fn state_log_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("hayato").join("state.log")
}

/// The shell: a header, the screen's body, and the keys that are not obvious.
pub struct App<'a> {
    header: Header,
    keybar: Vec<KeyHint>,
    screen: Option<&'a mut dyn Screen>,
    design: Design,
    motion: Motion,
    body_area: Rect,
    last_phase: Option<&'static str>,
    /// `HAYATO_DEBUG_STATE=1` appends phase changes here, away from the TUI.
    state_log: Option<std::path::PathBuf>,
}

impl<'a> App<'a> {
    pub fn new(header: Header) -> Self {
        let design = Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
        let motion = Motion::new(!design.reduced_motion());
        let state_log = std::env::var_os("HAYATO_DEBUG_STATE")
            .filter(|value| value != "0")
            .map(|_| state_log_path());
        Self {
            header,
            keybar: Vec::new(),
            screen: None,
            design,
            motion,
            body_area: Rect::default(),
            last_phase: None,
            state_log,
        }
    }

    /// Tests and the installer inject their own design (density, palette,
    /// reduced motion) instead of reading the session's.
    pub fn design(mut self, design: Design) -> Self {
        self.motion = Motion::new(!design.reduced_motion());
        self.design = design;
        self
    }

    pub fn keybar(mut self, hints: Vec<KeyHint>) -> Self {
        self.keybar = hints;
        self
    }

    pub fn screen(mut self, screen: &'a mut dyn Screen) -> Self {
        self.screen = Some(screen);
        self
    }

    pub fn design_ref(&self) -> &Design {
        &self.design
    }

    /// The active screen's phase label, for tests and the opt-in state log.
    pub fn current_phase(&self) -> Option<&'static str> {
        self.screen.as_ref().and_then(|screen| screen.phase())
    }

    fn log_phase(&mut self) {
        let Some(path) = &self.state_log else {
            return;
        };
        let phase = self.current_phase();
        if phase == self.last_phase {
            return;
        }
        self.last_phase = phase;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write as _;
            let label = phase.unwrap_or("-");
            let _ = writeln!(file, "{} phase {label}", std::process::id());
        }
    }

    /// Draw one frame. Everything a command draws goes through `BodyCtx`, so
    /// the three bands are the same in every window.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let design = self
            .screen
            .as_ref()
            .and_then(|s| s.design())
            .unwrap_or_else(|| self.design.clone());

        frame.render_widget(
            ratatui::widgets::Block::default().style(
                Style::default()
                    .bg(design.color(ColorRole::Bg))
                    .fg(design.color(ColorRole::Text)),
            ),
            area,
        );
        // Three bands, computed rather than solved: a `Layout` allocation per
        // frame is exactly the kind of quiet cost the budget test watches.
        let header_height = design.header_height().min(area.height);
        let keybar_height = 1.min(area.height.saturating_sub(header_height));
        let body_height = area
            .height
            .saturating_sub(header_height)
            .saturating_sub(keybar_height);
        let header = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: header_height,
        };
        let body = Rect {
            x: area.x,
            y: area.y + header_height,
            width: area.width,
            height: body_height,
        };
        let keybar = Rect {
            x: area.x,
            y: body.bottom(),
            width: area.width,
            height: keybar_height,
        };
        let band = self
            .screen
            .as_ref()
            .and_then(|s| s.header())
            .unwrap_or(&self.header);
        draw_header(frame.buffer_mut(), header, &design, band);
        self.body_area = body;
        if let Some(screen) = self.screen.as_mut() {
            let mut ctx = BodyCtx {
                area: body,
                buf: frame.buffer_mut(),
                design: &design,
            };
            screen.draw(&mut ctx);
        }
        let hints = self
            .screen
            .as_ref()
            .and_then(|s| s.keybar())
            .filter(|h| !h.is_empty())
            .unwrap_or(&self.keybar);
        draw_keybar(frame.buffer_mut(), keybar, &design, hints);
        self.motion.apply(frame.buffer_mut(), body);
    }

    /// The screen sees the key first; Escape and Ctrl-C are the shell's only
    /// when it does not use them.
    pub fn handle(&mut self, event: Event) -> Outcome {
        let design = self
            .screen
            .as_ref()
            .and_then(|s| s.design())
            .unwrap_or_else(|| self.design.clone());

        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(screen) = self.screen.as_mut() {
                    match screen.key(key, &design) {
                        Outcome::Continue => {}
                        Outcome::Consumed => {
                            let current = Design::for_session(
                                std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
                            );
                            if current.palette() != self.design.palette() {
                                self.design = current;
                            }
                            return Outcome::Continue;
                        }
                        done => return done,
                    }
                }
                match key.code {
                    KeyCode::Esc => Outcome::Done(exit::ESCAPE),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        Outcome::Done(exit::ESCAPE)
                    }
                    _ => Outcome::Continue,
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let row = self.screen.as_ref().and_then(|screen| {
                    screen.hit(self.body_area, mouse.column, mouse.row, &design)
                });
                let outcome = match self.screen.as_mut() {
                    Some(screen) => screen.click(row),
                    None => Outcome::Continue,
                };
                if outcome == Outcome::Consumed {
                    let current =
                        Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
                    if current.palette() != self.design.palette() {
                        self.design = current;
                    }
                    return Outcome::Continue;
                }
                outcome
            }
            other => match self.screen.as_mut() {
                Some(screen) => screen.event(&other, &design),
                None => Outcome::Continue,
            },
        }
    }

    /// The event loop: draw on every event, wake on a tick only while the
    /// opening animation runs.
    pub fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> io::Result<i32> {
        self.run_until(terminal, None)
    }

    /// The same loop with an idle ceiling: a card nobody answers closes itself
    /// after `limit`, because a broken input must not be able to hang the
    /// session. `None` waits for ever, which is what a menu wants.
    pub fn run_until(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        limit: Option<Duration>,
    ) -> io::Result<i32> {
        self.motion.start(&self.design);
        let mut last_event = Instant::now();
        let mut applied_size = self.screen.as_ref().and_then(|s| s.desired_size());
        let mut first_frame = true;
        loop {
            self.log_phase();
            terminal.draw(|frame| self.render(frame))?;
            if first_frame {
                first_frame = false;
                if let Some((w, h)) = applied_size
                    && std::env::var_os("HAYATO_UI").is_some()
                {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(45));
                        request_window_size(w, h);
                    });
                }
            } else {
                let current_size = self.screen.as_ref().and_then(|s| s.desired_size());
                if let Some((w, h)) = current_size
                    && applied_size != Some((w, h))
                {
                    applied_size = Some((w, h));
                    request_window_size(w, h);
                }
            }
            let screen_tick = self
                .screen
                .as_ref()
                .and_then(|s| s.needs_tick())
                .unwrap_or(Duration::MAX);
            let timeout = if self.motion.running() {
                TICK.min(screen_tick)
            } else {
                let base = match limit {
                    Some(limit) => limit
                        .saturating_sub(last_event.elapsed())
                        .max(Duration::from_millis(1)),
                    None => Duration::from_secs(3600),
                };
                base.min(screen_tick)
            };
            let outcome = if event::poll(timeout)? {
                last_event = Instant::now();
                let ev = event::read()?;
                let needs_full_clear = matches!(ev, Event::FocusGained | Event::Resize(_, _))
                    || matches!(
                        &ev,
                        Event::Key(k)
                            if k.kind == KeyEventKind::Press
                                && k.modifiers == KeyModifiers::CONTROL
                                && matches!(k.code, KeyCode::Char('l' | 'L'))
                    );
                let out = self.handle(ev);
                if needs_full_clear && !matches!(out, Outcome::Done(_)) {
                    let _ = terminal.clear();
                }
                out
            } else {
                if let Some(limit) = limit
                    && last_event.elapsed() >= limit
                    && !self.motion.running()
                {
                    return Ok(exit::FAILED);
                }
                if let Some(screen) = self.screen.as_mut() {
                    screen.tick()
                } else {
                    Outcome::Continue
                }
            };
            if let Outcome::Done(code) = outcome {
                return Ok(code);
            }
        }
    }
}

/// A body that says "nothing here yet" - spec 001's menu, and the shell every
/// later surface is built on.
pub struct EmptyScreen {
    pub title: String,
    pub hint: String,
}

impl EmptyScreen {
    pub fn new(title: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            hint: hint.into(),
        }
    }
}

impl Screen for EmptyScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        widgets::draw_empty(ctx.buf, ctx.area, ctx.design, &self.title, &self.hint);
    }
}

/// The body of the error card: every key closes it, and the exit code stays 3.
pub struct FailureScreen {
    pub summary: String,
    pub detail: String,
}

impl FailureScreen {
    pub fn new(summary: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            detail: detail.into(),
        }
    }
}

impl Screen for FailureScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_failure(ctx.buf, ctx.area, ctx.design, &self.summary, &self.detail);
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind == KeyEventKind::Press {
            Outcome::Done(exit::FAILED)
        } else {
            Outcome::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::Density;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn shell<'a>(screen: &'a mut dyn Screen) -> App<'a> {
        App::new(Header::new("Hayato").subtitle("the session menu"))
            .design(Design::solarized().with_reduced_motion(true))
            .keybar(vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("Esc", "close"),
            ])
            .screen(screen)
    }

    #[test]
    fn escape_returns_one_and_ctrl_c_too() {
        let mut screen = EmptyScreen::new("menu", "later");
        let mut app = shell(&mut screen);
        assert_eq!(app.handle(key(KeyCode::Esc)), Outcome::Done(1));
        let mut screen = EmptyScreen::new("menu", "later");
        let mut app = shell(&mut screen);
        assert_eq!(
            app.handle(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL
            ))),
            Outcome::Done(1)
        );
    }

    #[test]
    fn an_ordinary_key_does_nothing() {
        let mut screen = EmptyScreen::new("menu", "later");
        let mut app = shell(&mut screen);
        assert_eq!(app.handle(key(KeyCode::Char('x'))), Outcome::Continue);
    }

    #[test]
    fn the_failure_card_owns_the_key_so_the_code_stays_three() {
        let mut screen = FailureScreen::new("it broke", "because");
        let mut app = shell(&mut screen);
        // Escape would be exit 1 from the shell; the card answers first.
        assert_eq!(app.handle(key(KeyCode::Esc)), Outcome::Done(3));
    }

    #[test]
    fn the_shell_draws_header_body_and_key_bar() {
        let mut screen = EmptyScreen::new("the session menu", "arrives in spec 007");
        let mut app = shell(&mut screen);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = crate::screen(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("Hayato"), "the header is the first row");
        assert!(
            lines[23].contains("[Esc]"),
            "the key bar is the last row: {:?}",
            lines[23]
        );
        assert!(text.contains("the session menu"));
    }

    #[test]
    fn reduced_motion_renders_the_final_frame_without_a_tick() {
        let mut screen = EmptyScreen::new("menu", "later");
        let mut app = shell(&mut screen);
        let mut with_motion = Terminal::new(TestBackend::new(80, 24)).unwrap();
        with_motion.draw(|frame| app.render(frame)).unwrap();
        // No animation was started and none is applied: the frame is the final
        // one, which is what "reduced motion" means for a screenshot or a person.
        assert!(!app.motion.enabled());

        let design = Design::solarized();
        let mut plain = Buffer::empty(Rect::new(0, 0, 80, 24));
        widgets::draw_header(
            &mut plain,
            Rect::new(0, 0, 80, design.header_height()),
            &design,
            &Header::new("Hayato").subtitle("the session menu"),
        );
        let mut motion = Motion::new(true);
        motion.start(&design);
        assert!(motion.running(), "the animation exists when it is allowed");
        let ceiling = Duration::from_millis(300);
        assert!(Duration::from_millis(MOTION_MS) <= ceiling);
    }

    #[test]
    fn the_density_decides_the_row_stride_the_list_and_the_hit_test_share() {
        let design = Design::solarized().with_density(Density::Tablet);
        assert_eq!(design.row_stride(), 2);
        assert_eq!(design.header_height(), 3);
    }
}
