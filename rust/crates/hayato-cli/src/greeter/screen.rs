//! `GreeterScreen`: the native Ratatui login canvas for `hayato-greeter`.
//!
//! Draws through `hayato_ui::App` (`BodyCtx` only — never owns a `Frame`),
//! keeping zero steady-state allocations on draw and tick, and driving real
//! `greetd` IPC authentication cleanly on a background worker thread.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Local;
use hayato_theme::{Theme, builtin};
use hayato_ui::Glyph;
use hayato_ui::app::{BodyCtx, Outcome, Screen};
use hayato_ui::design::{Border, ColorRole, Density, Design, TextStyle};
use hayato_ui::widgets::{
    ActionItem, ActionRunner, ActionStrip, ActionStripEvent, Header, Input, InputAction, KeyHint,
    ListAction, ListState, Row, draw_card_with, draw_filter_bar, put, put_trunc,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::ipc::{AuthMessageType, AuthOutcome, Backend, SessionLaunch};
use super::{GreeterConfig, GreeterState, SessionEntry, UserEntry, load_theme_by_slug};

const MAX_USER_PILLS: usize = 6;
const ANIM_FRAME_MS: u64 = 60;

/// 5x8 pixel-art bitmaps matching the Plymouth boot splash (`H`, `a`, `y`, `a`, `t`, `o`).
/// Each byte encodes 5 columns in bits 4..=0 (bit 4 = leftmost pixel).
const PIXEL_LETTERS: [[u8; 8]; 6] = [
    // H
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001, 0b00000],
    // a
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0b00000],
    // y
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
    // a
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0b00000],
    // t
    [0b00000, 0b00100, 0b11111, 0b00100, 0b00100, 0b00100, 0b00011, 0b00000],
    // o
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000],
];

/// 5x8 pixel-art decode/glitch glyphs (`0`, `1`, `#`, `/`, `=`, `+`).
const PIXEL_GLITCHES: [[u8; 8]; 6] = [
    // 0
    [0b00000, 0b01110, 0b10011, 0b10101, 0b11001, 0b01110, 0b00000, 0b00000],
    // 1
    [0b00000, 0b00100, 0b01100, 0b00100, 0b00100, 0b01110, 0b00000, 0b00000],
    // #
    [0b00000, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b00000, 0b00000],
    // /
    [0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000, 0b00000],
    // =
    [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
    // +
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000, 0b00000],
];

const COMPACT_LETTERS: [&str; 6] = ["H", "a", "y", "a", "t", "o"];
const COMPACT_GLITCHES: [&str; 6] = ["0", "1", "#", "/", "=", "+"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GlyphState {
    Glitch(usize),
    Locked(usize),
}

fn hero_letter_state(idx: usize, tick: u32, reduced_motion: bool) -> GlyphState {
    if reduced_motion {
        return GlyphState::Locked(idx);
    }
    // Word is already formed on entry; keep a subtle periodic single-letter glitch pulse.
    let cycle = tick % 42;
    let target = ((tick / 42) as usize) % PIXEL_LETTERS.len();
    if tick >= 18 && idx == target && cycle < 2 {
        let g = ((tick as usize) + idx) % PIXEL_GLITCHES.len();
        GlyphState::Glitch(g)
    } else {
        GlyphState::Locked(idx)
    }
}

fn draw_pixel_glyph(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    bitmap: &[u8; 8],
    style: Style,
) {
    for row in 0..4u16 {
        let top_bits = bitmap[(row * 2) as usize];
        let bot_bits = bitmap[(row * 2 + 1) as usize];
        for col in 0..5u16 {
            let mask = 1u8 << (4 - col);
            let top = (top_bits & mask) != 0;
            let bot = (bot_bits & mask) != 0;
            let ch = match (top, bot) {
                (true, true) => "\u{2588}",  // █
                (true, false) => "\u{2580}", // ▀
                (false, true) => "\u{2584}", // ▄
                (false, false) => continue,
            };
            if let Some(cell) = buf.cell_mut((x + col, y + row)) {
                cell.set_symbol(ch).set_style(style);
            }
        }
    }
}

fn draw_pixel_hero(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    design: &Design,
    tick: u32,
    reduced_motion: bool,
) {
    // 6 letters * 5 cols + 5 gaps * 1 col + 1 gap + 4-col pixel underscore = 40 cols
    const TOTAL_W: u16 = 40;
    let base_x = area.x + area.width.saturating_sub(TOTAL_W) / 2;

    let locked_style = Style::default().fg(design.color(ColorRole::Text));
    let glitch_style = Style::default().fg(design.color(ColorRole::Accent));
    let cursor_style = Style::default().fg(design.color(ColorRole::Warn));
    let cursor_dim = Style::default().fg(design.color(ColorRole::Border));

    for i in 0..PIXEL_LETTERS.len() {
        let lx = base_x + (i as u16) * 6;
        match hero_letter_state(i, tick, reduced_motion) {
            GlyphState::Glitch(g) => {
                draw_pixel_glyph(buf, lx, y, &PIXEL_GLITCHES[g], glitch_style);
            }
            GlyphState::Locked(idx) => {
                draw_pixel_glyph(buf, lx, y, &PIXEL_LETTERS[idx], locked_style);
            }
        }
    }

    // Horizontal pixel underscore cursor (_) aligned on row 6 (top half of terminal row 3)
    let cursor_on = reduced_motion || (tick / 8).is_multiple_of(2);
    let cstyle = if cursor_on { cursor_style } else { cursor_dim };
    let cursor_x = base_x + 36;
    for dx in 0..4u16 {
        if let Some(cell) = buf.cell_mut((cursor_x + dx, y + 3)) {
            cell.set_symbol("\u{2580}").set_style(cstyle); // ▀
        }
    }
}

fn draw_compact_hero(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    design: &Design,
    tick: u32,
    reduced_motion: bool,
) {
    // "Hayato" (6 cols) + 1 space + underscore cursor (1 col) = 8 cols
    let bx = area.x + area.width.saturating_sub(8) / 2;
    let locked_style = Style::default()
        .fg(design.color(ColorRole::Text))
        .add_modifier(Modifier::BOLD);
    let glitch_style = Style::default()
        .fg(design.color(ColorRole::Accent))
        .add_modifier(Modifier::BOLD);
    let cursor_style = Style::default()
        .fg(design.color(ColorRole::Warn))
        .add_modifier(Modifier::BOLD);

    for i in 0..COMPACT_LETTERS.len() {
        let (sym, st) = match hero_letter_state(i, tick, reduced_motion) {
            GlyphState::Glitch(g) => (COMPACT_GLITCHES[g], glitch_style),
            GlyphState::Locked(idx) => (COMPACT_LETTERS[idx], locked_style),
        };
        put(buf, bx + i as u16, y, sym, 1, st);
    }

    let cursor_on = reduced_motion || (tick / 8).is_multiple_of(2);
    if cursor_on {
        put(buf, bx + 7, y, "_", 1, cursor_style);
    }
}

fn fill_rect(buf: &mut ratatui::buffer::Buffer, area: Rect, style: Style) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusKind {
    Info,
    Ok,
    Warn,
    Err,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Users,
    Username,
    Password,
    Strip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Reboot,
    Poweroff,
}

impl PowerAction {
    fn label(self) -> &'static str {
        match self {
            PowerAction::Sleep => "Sleep",
            PowerAction::Reboot => "Reboot",
            PowerAction::Poweroff => "Shut down",
        }
    }

    fn systemd_verb(self) -> &'static str {
        match self {
            PowerAction::Sleep => "suspend",
            PowerAction::Reboot => "reboot",
            PowerAction::Poweroff => "poweroff",
        }
    }
}

fn format_power_failure(action: PowerAction, status: ExitStatus, stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let detail = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            status
                .code()
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "signaled".to_string())
        });
    format!("{} refused by systemd ({detail})", action.label())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Login,
    SessionPicker,
    ThemePicker,
    ConfirmPower {
        action: PowerAction,
        confirm_yes: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct HitRects {
    user_pills: [Option<(usize, Rect)>; MAX_USER_PILLS + 1],
    user_count: usize,
    username_box: Option<Rect>,
    password_box: Option<Rect>,
    strip_row: Option<Rect>,
    confirm_cancel: Option<Rect>,
    confirm_yes: Option<Rect>,
    picker_list: Option<Rect>,
}

impl Default for HitRects {
    fn default() -> Self {
        Self {
            user_pills: [None; MAX_USER_PILLS + 1],
            user_count: 0,
            username_box: None,
            password_box: None,
            strip_row: None,
            confirm_cancel: None,
            confirm_yes: None,
            picker_list: None,
        }
    }
}

pub struct GreeterSetup {
    pub users: Vec<UserEntry>,
    pub sessions: Vec<SessionEntry>,
    pub state: GreeterState,
    pub state_path: Option<PathBuf>,
    pub auth_marker: Option<PathBuf>,
    pub theme_slug: String,
    pub theme: Theme,
    pub greeting: String,
    pub backend: Backend,
    pub preview: bool,
    pub density: Density,
    pub reduced_motion: bool,
    pub config: GreeterConfig,
}

struct AuthAttemptResult {
    backend: Backend,
    outcome: AuthOutcome,
    username: String,
    session_name: String,
}

/// Which user the login form is for.
///
/// The old field used `users.len()` as the "Other…" sentinel, so an empty
/// `/etc/passwd` list and the extra pill were the same number. The variant
/// says which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UserChoice {
    Existing(usize),
    Other,
}

/// Which PAM exchange the password field is answering.
///
/// `challenge_open` was a bool beside a `prompt_label` string, so the prompt
/// and the field could disagree. `Challenge` owns the label; `Password` is the
/// first attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AuthPrompt {
    Password,
    Challenge { label: String },
}

impl AuthPrompt {
    fn is_challenge(&self) -> bool {
        matches!(self, Self::Challenge { .. })
    }

    fn label(&self) -> &str {
        match self {
            Self::Password => "Password",
            Self::Challenge { label } => label,
        }
    }
}

pub struct GreeterScreen {
    users: Vec<UserEntry>,
    selected_user: UserChoice,
    sessions: Vec<SessionEntry>,
    selected_session: usize,
    state: GreeterState,
    state_path: Option<PathBuf>,
    auth_marker: Option<PathBuf>,

    theme_slug: String,
    theme: Theme,
    theme_list: ListState,
    theme_slugs: Vec<&'static str>,
    session_list: ListState,

    greeting: String,
    hostname: String,
    clock_str: String,
    date_str: String,
    clock_fixed: bool,
    clock_24h: bool,
    last_clock_sec: u64,

    show_user_pills: bool,
    remember_user: bool,
    remember_session: bool,
    follow_user_theme: bool,
    show_power_actions: bool,
    open_settings: bool,

    username_input: Input,
    secret_input: Input,
    auth_prompt: AuthPrompt,
    caps_lock: bool,
    cursor_blink_on: bool,
    last_blink: Instant,
    anim_tick: u32,
    last_anim: Instant,

    focus: Focus,
    mode: Mode,
    strip: ActionStrip,

    backend: Option<Backend>,
    runner: ActionRunner<AuthAttemptResult>,
    status: Option<(StatusKind, String)>,
    preview: bool,
    density: Density,
    reduced_motion: bool,

    header: Header,
    keybar_main: Vec<KeyHint>,
    keybar_picker: Vec<KeyHint>,
    keybar_confirm: Vec<KeyHint>,
    keybar_busy: Vec<KeyHint>,
    hits: RefCell<HitRects>,
    picker_height: Cell<u16>,
}

impl GreeterScreen {
    pub fn new(setup: GreeterSetup) -> Self {
        let cfg = setup.config;
        let show_user_pills = cfg.show_user_pills;
        let remember_user = cfg.remember_user;
        let remember_session = cfg.remember_session;
        let clock_24h = cfg.clock_24h;
        let follow_user_theme = cfg.follow_user_theme;
        let show_power_actions = cfg.show_power_actions;

        let mut selected_user = if setup.users.is_empty() {
            UserChoice::Other
        } else {
            UserChoice::Existing(0)
        };
        let mut username_input = Input::new();
        if remember_user && let Some(last) = setup.state.last_user.as_deref() {
            if show_user_pills
                && let Some(idx) = setup.users.iter().position(|u| u.username == last)
            {
                selected_user = UserChoice::Existing(idx);
            } else if !show_user_pills || setup.users.is_empty() {
                username_input.set(last);
            }
        }

        // When pills are hidden or no users were found in /etc/passwd, use the manual username field.
        let focus = if !show_user_pills || setup.users.is_empty() {
            selected_user = UserChoice::Other;
            if username_input.text().is_empty() {
                Focus::Username
            } else {
                Focus::Password
            }
        } else {
            Focus::Password
        };

        let mut selected_session = 0;
        if remember_session
            && let Some(last) = setup.state.last_session.as_deref()
            && let Some(idx) = setup.sessions.iter().position(|s| s.name == last)
        {
            selected_session = idx;
        } else if let Some(def) = cfg.default_session.as_deref()
            && let Some(idx) = setup.sessions.iter().position(|s| s.name == def)
        {
            selected_session = idx;
        }

        let theme_slugs: Vec<&'static str> = builtin::slugs().collect();
        let mut theme_list = ListState::new().with_empty("no themes");
        let theme_rows: Vec<Row> = theme_slugs
            .iter()
            .map(|slug| {
                let name = load_theme_by_slug(slug)
                    .map(|t| t.name)
                    .unwrap_or_else(|| (*slug).to_string());
                Row::new(name)
                    .glyph(Glyph::Theme)
                    .value(*slug)
                    .detail(*slug)
            })
            .collect();
        theme_list.set_rows(theme_rows);
        if let Some(pos) = theme_slugs.iter().position(|s| *s == setup.theme_slug) {
            theme_list.set_cursor(pos);
        }

        let mut session_list = ListState::new().with_empty("no sessions");
        let session_rows: Vec<Row> = setup
            .sessions
            .iter()
            .map(|s| {
                Row::new(s.name.clone())
                    .glyph(Glyph::Window)
                    .detail(s.kind.label())
                    .value(s.name.clone())
            })
            .collect();
        session_list.set_rows(session_rows);
        session_list.set_cursor(selected_session);

        let hostname = read_hostname();
        let (clock_str, date_str, now_sec) = current_clock(clock_24h);

        let strip = Self::build_strip(show_power_actions);

        let keybar_main = Vec::new();

        Self {
            users: setup.users,
            selected_user,
            sessions: setup.sessions,
            selected_session,
            state: setup.state,
            state_path: setup.state_path,
            auth_marker: setup.auth_marker,
            theme_slug: setup.theme_slug,
            theme: setup.theme,
            theme_list,
            theme_slugs,
            session_list,
            greeting: setup.greeting,
            hostname,
            clock_str,
            date_str,
            clock_fixed: false,
            clock_24h,
            last_clock_sec: now_sec,
            show_user_pills,
            remember_user,
            remember_session,
            follow_user_theme,
            show_power_actions,
            open_settings: false,
            username_input,
            secret_input: Input::new().with_masked(true),
            auth_prompt: AuthPrompt::Password,
            caps_lock: false,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            anim_tick: 0,
            last_anim: Instant::now(),
            focus,
            mode: Mode::Login,
            strip,
            backend: Some(setup.backend),
            runner: ActionRunner::new(),
            status: None,
            preview: setup.preview,
            density: setup.density,
            reduced_motion: setup.reduced_motion,
            header: Header::default(),
            keybar_main,
            keybar_picker: vec![
                KeyHint::new("Enter", "select"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "back"),
            ],
            keybar_confirm: vec![
                KeyHint::new("\u{2190}\u{2192}", "choose"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
            keybar_busy: vec![KeyHint::new("Esc", "cancel auth")],
            hits: RefCell::new(HitRects::default()),
            picker_height: Cell::new(10),
        }
    }

    pub fn take_open_settings(&mut self) -> bool {
        let was = self.open_settings;
        self.open_settings = false;
        was
    }

    /// Override displayed clock/date/hostname deterministically (for snapshot tests).
    pub fn set_clock_for_test(&mut self, clock: &str, date: &str, hostname: &str) {
        self.clock_str = clock.into();
        self.date_str = date.into();
        self.hostname = hostname.into();
        self.clock_fixed = true;
    }

    fn build_strip(show_power_actions: bool) -> ActionStrip {
        let mut items = vec![ActionItem::new("theme", "^T", "Theme").glyph(Glyph::Theme)];
        if show_power_actions {
            items.push(ActionItem::new("sleep", "^Z", "Sleep").glyph(Glyph::Moon));
            items.push(ActionItem::new("reboot", "^R", "Reboot").glyph(Glyph::Reboot));
            items.push(ActionItem::new("poweroff", "^P", "Shut down").glyph(Glyph::Power));
        }
        ActionStrip::new(items)
    }

    fn refresh_strip(&mut self) {
        self.strip = Self::build_strip(self.show_power_actions);
    }

    fn pills_visible(&self) -> bool {
        self.show_user_pills && !self.users.is_empty()
    }

    fn using_other_user(&self) -> bool {
        !self.show_user_pills || matches!(self.selected_user, UserChoice::Other)
    }

    fn active_username(&self) -> String {
        match self.selected_user {
            UserChoice::Existing(idx) => self
                .users
                .get(idx)
                .map(|user| user.username.clone())
                .unwrap_or_default(),
            UserChoice::Other => self.username_input.text().trim().to_string(),
        }
    }

    fn active_session(&self) -> SessionEntry {
        self.sessions
            .get(self.selected_session)
            .cloned()
            .unwrap_or(SessionEntry {
                name: "Sway".into(),
                exec: vec!["sway".into()],
                kind: super::SessionKind::Wayland,
                desktop_names: vec!["sway".into()],
                comment: None,
            })
    }

    fn apply_theme_slug(&mut self, slug: &str) {
        let Some(new_theme) = load_theme_by_slug(slug) else {
            return;
        };
        if !self.preview {
            if let Some(path) = &self.state_path {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::write(parent.join("theme"), format!("{slug}\n"));
                }
            }
        }
        // Only emit OSC sequences to stdout when running in an interactive terminal.
        #[cfg(not(test))]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            use std::io::Write as _;
            let mut out = std::io::stdout();
            let _ = out.write_all(new_theme.osc_sequences().as_bytes());
            let _ = out.flush();
        }
        self.theme_slug = slug.to_string();
        self.theme = new_theme;
        self.refresh_strip();
    }

    fn cycle_theme(&mut self) {
        if self.theme_slugs.is_empty() {
            return;
        }
        let pos = self
            .theme_slugs
            .iter()
            .position(|s| *s == self.theme_slug)
            .unwrap_or(0);
        let next_slug = self.theme_slugs[(pos + 1) % self.theme_slugs.len()];
        self.apply_theme_slug(next_slug);
        if let Some(next_pos) = self.theme_slugs.iter().position(|s| *s == self.theme_slug) {
            self.theme_list.set_cursor(next_pos);
        }
    }

    fn reset_blink(&mut self) {
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();
    }

    fn reset_pam_state(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            backend.cancel();
        }
        self.auth_prompt = AuthPrompt::Password;
        self.secret_input = Input::new().with_masked(true);
    }

    fn select_user(&mut self, idx: usize) {
        self.selected_user = if idx >= self.users.len() {
            UserChoice::Other
        } else {
            UserChoice::Existing(idx)
        };
        self.reset_pam_state();
        self.status = None;
        self.focus = if self.using_other_user() {
            Focus::Username
        } else {
            Focus::Password
        };
    }

    fn submit_auth(&mut self) {
        if self.runner.is_running() {
            return;
        }
        let username = self.active_username();
        if username.is_empty() {
            self.status = Some((
                StatusKind::Warn,
                "Enter a username before signing in".into(),
            ));
            self.focus = Focus::Username;
            return;
        }
        let Some(mut backend) = self.backend.take() else {
            return;
        };

        let session = self.active_session();
        let launch = SessionLaunch {
            username: username.clone(),
            cmd: session.exec.clone(),
            env: session.env_vars(),
            session_name: session.name.clone(),
        };
        let secret = self.secret_input.text().to_string();
        let reply_only = self.auth_prompt.is_challenge();

        self.status = None;
        self.runner.spawn("Authenticating", move || {
            let outcome = if reply_only {
                backend.reply(&launch, &secret)
            } else {
                backend.begin(&launch, &secret)
            };
            AuthAttemptResult {
                backend,
                outcome,
                username,
                session_name: launch.session_name,
            }
        });
    }

    fn trigger_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "session" => {
                self.session_list.set_filter("");
                self.session_list.set_cursor(self.selected_session);
                self.mode = Mode::SessionPicker;
                Outcome::Consumed
            }
            "theme" => {
                self.theme_list.set_filter("");
                if let Some(pos) = self.theme_slugs.iter().position(|s| *s == self.theme_slug) {
                    self.theme_list.set_cursor(pos);
                }
                self.mode = Mode::ThemePicker;
                Outcome::Consumed
            }
            "settings" => {
                if self.preview {
                    self.open_settings = true;
                    Outcome::Done(0)
                } else {
                    Outcome::Consumed
                }
            }
            "sleep" => {
                self.mode = Mode::ConfirmPower {
                    action: PowerAction::Sleep,
                    confirm_yes: false,
                };
                Outcome::Consumed
            }
            "reboot" => {
                self.mode = Mode::ConfirmPower {
                    action: PowerAction::Reboot,
                    confirm_yes: false,
                };
                Outcome::Consumed
            }
            "poweroff" => {
                self.mode = Mode::ConfirmPower {
                    action: PowerAction::Poweroff,
                    confirm_yes: false,
                };
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn execute_power(&mut self, action: PowerAction) -> Outcome {
        self.mode = Mode::Login;
        if self.preview {
            self.status = Some((
                StatusKind::Ok,
                format!(
                    "Preview: would execute `systemctl {} -i`",
                    action.systemd_verb()
                ),
            ));
            return Outcome::Consumed;
        }
        // Redirect stdio so systemd warnings/inhibitors never paint over the raw
        // Ratatui terminal, and pass `-i` so an idle lock or background TTY does
        // not block a deliberate console power button press.
        match Command::new("systemctl")
            .arg(action.systemd_verb())
            .arg("-i")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(out) if out.status.success() => {
                self.status = Some((StatusKind::Info, format!("Executing {}…", action.label())));
                Outcome::Consumed
            }
            Ok(out) => {
                self.status = Some((
                    StatusKind::Err,
                    format_power_failure(action, out.status, &out.stderr),
                ));
                Outcome::Consumed
            }
            Err(e) => {
                self.status = Some((
                    StatusKind::Err,
                    format!("cannot run systemctl {}: {e}", action.systemd_verb()),
                ));
                Outcome::Consumed
            }
        }
    }

    fn record_login_state(&mut self, username: &str, session_name: &str) {
        self.state.last_user = self.remember_user.then(|| username.to_string());
        self.state.last_session = self.remember_session.then(|| session_name.to_string());
        if self.follow_user_theme {
            self.state.theme = Some(self.theme_slug.clone());
        }
        if let Some(path) = &self.state_path {
            let _ = self.state.save(path);
        }
        if let Some(marker) = &self.auth_marker {
            let _ = std::fs::write(marker, b"ok\n");
        }
    }

    fn draw_confirm_modal(
        &self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        action: PowerAction,
        confirm_yes: bool,
        hits: &mut HitRects,
    ) {
        let mut msg = String::new();
        msg.push_str(action.label());
        msg.push_str(" this device now?");
        put_trunc(
            buf,
            area.x,
            area.y,
            &msg,
            area.width,
            design.style(TextStyle::Title),
        );

        let y = area.y + 2;
        if y >= area.bottom() {
            return;
        }
        let cancel_label = "  Cancel (Esc)  ";
        let yes_label = "  Confirm (Enter)  ";
        let cancel_w = cancel_label.len() as u16;
        let yes_w = yes_label.len() as u16;

        let cancel_style = if !confirm_yes {
            design.style(TextStyle::Key)
        } else {
            Style::default()
                .fg(design.color(ColorRole::Text))
                .bg(design.color(ColorRole::SurfaceAlt))
        };
        let yes_style = if confirm_yes {
            Style::default()
                .fg(design.color(ColorRole::Bg))
                .bg(design.color(ColorRole::Danger))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(design.color(ColorRole::Danger))
                .bg(design.color(ColorRole::SurfaceAlt))
        };

        let cancel_rect = Rect::new(area.x, y, cancel_w.min(area.width), 1);
        put_trunc(
            buf,
            cancel_rect.x,
            y,
            cancel_label,
            cancel_rect.width,
            cancel_style,
        );
        hits.confirm_cancel = Some(cancel_rect);

        let yes_x = cancel_rect.right() + 2;
        if yes_x + 4 <= area.right() {
            let yes_rect = Rect::new(yes_x, y, yes_w.min(area.right() - yes_x), 1);
            put_trunc(buf, yes_rect.x, y, yes_label, yes_rect.width, yes_style);
            hits.confirm_yes = Some(yes_rect);
        }
    }

    fn draw_picker_modal(
        &self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        is_theme: bool,
        hits: &mut HitRects,
    ) {
        let title = if is_theme {
            "Select greeter theme (live preview)"
        } else {
            "Select desktop session"
        };
        put_trunc(
            buf,
            area.x,
            area.y,
            title,
            area.width,
            design.style(TextStyle::Title),
        );
        let filter_y = area.y + 1;
        if filter_y + 2 >= area.bottom() {
            return;
        }
        let list = if is_theme {
            &self.theme_list
        } else {
            &self.session_list
        };
        let filter_rect = Rect::new(area.x, filter_y, area.width, 1);
        draw_filter_bar(
            buf,
            filter_rect,
            design,
            list.filter(),
            "type to filter themes or sessions",
            list.visible_len(),
            list.rows().len(),
        );
        let list_rect = Rect::new(
            area.x,
            filter_y + 2,
            area.width,
            area.bottom().saturating_sub(filter_y + 2),
        );
        self.picker_height.set(list_rect.height);
        hits.picker_list = Some(list_rect);
        list.render(list_rect, buf, design, true);
    }
}

impl Screen for GreeterScreen {
    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        if self.runner.is_running() {
            Some(&self.keybar_busy)
        } else {
            match self.mode {
                Mode::Login => Some(&self.keybar_main),
                Mode::SessionPicker | Mode::ThemePicker => Some(&self.keybar_picker),
                Mode::ConfirmPower { .. } => Some(&self.keybar_confirm),
            }
        }
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.mode {
            Mode::Login => {
                if self.runner.is_running() {
                    "login-authenticating"
                } else if self.auth_prompt.is_challenge() {
                    "login-challenge"
                } else {
                    "login"
                }
            }
            Mode::SessionPicker => "session-picker",
            Mode::ThemePicker => "theme-picker",
            Mode::ConfirmPower { .. } => "confirm-power",
        })
    }

    fn design(&self) -> Option<Design> {
        Some(Design::new(
            self.theme.palette(),
            self.density,
            self.reduced_motion || self.theme.motion != hayato_theme::Motion::Full,
        ))
    }

    fn needs_tick(&self) -> Option<Duration> {
        let reduced = self.design().is_some_and(|d| d.reduced_motion());
        let anim_active = self.mode == Mode::Login && !reduced && !self.clock_fixed;
        let blink_active = self.mode == Mode::Login
            && matches!(self.focus, Focus::Username | Focus::Password)
            && !reduced;
        if self.runner.is_running() {
            Some(Duration::from_millis(100))
        } else if anim_active {
            Some(Duration::from_millis(ANIM_FRAME_MS))
        } else if blink_active {
            Some(Duration::from_millis(500))
        } else if !self.clock_fixed {
            Some(Duration::from_secs(1))
        } else {
            None
        }
    }

    fn tick(&mut self) -> Outcome {
        if !self.clock_fixed {
            let now_sec = epoch_secs();
            if now_sec != self.last_clock_sec {
                let (c, d, s) = current_clock(self.clock_24h);
                self.clock_str = c;
                self.date_str = d;
                self.last_clock_sec = s;
            }
        }
        let reduced = self.design().is_some_and(|d| d.reduced_motion());
        if self.mode == Mode::Login
            && !reduced
            && !self.clock_fixed
            && self.last_anim.elapsed() >= Duration::from_millis(ANIM_FRAME_MS)
        {
            self.anim_tick = self.anim_tick.wrapping_add(1);
            self.last_anim = Instant::now();
        }
        if self.mode == Mode::Login
            && matches!(self.focus, Focus::Username | Focus::Password)
            && !reduced
            && self.last_blink.elapsed() >= Duration::from_millis(500)
        {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
        }

        if let Some(res) = self.runner.poll() {
            self.backend = Some(res.backend);
            match res.outcome {
                AuthOutcome::Started { preview_note } => {
                    self.auth_prompt = AuthPrompt::Password;
                    self.secret_input = Input::new().with_masked(true);
                    if let Some(note) = preview_note {
                        self.status = Some((StatusKind::Ok, note));
                        return Outcome::Consumed;
                    }
                    self.record_login_state(&res.username, &res.session_name);
                    return Outcome::Done(0);
                }
                AuthOutcome::Challenge {
                    prompt_type,
                    prompt,
                    notice,
                } => {
                    let cleaned = prompt.trim_end_matches(':').trim().to_string();
                    self.auth_prompt = AuthPrompt::Challenge {
                        label: if cleaned.is_empty() {
                            "Response".into()
                        } else {
                            cleaned
                        },
                    };
                    self.secret_input =
                        Input::new().with_masked(prompt_type == AuthMessageType::Secret);
                    self.focus = Focus::Password;
                    self.status = notice.map(|n| (StatusKind::Info, n));
                }
                AuthOutcome::Failed(err) => {
                    self.reset_pam_state();
                    self.status = Some((StatusKind::Err, err));
                    self.focus = Focus::Password;
                }
            }
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let mut hits = HitRects::default();
        let area = ctx.area;
        if area.width < 32 || area.height < 12 {
            return;
        }

        let in_modal = !matches!(self.mode, Mode::Login);
        let use_pixel_hero = area.height >= 26 && area.width >= 44;
        let hero_h: u16 = if use_pixel_hero { 8 } else { 5 };
        let has_status = self.runner.is_running() || self.status.is_some();

        // Compute tight card geometry fitted to content (or expanded for modals).
        let (card_w, card_h) = if in_modal {
            (
                area.width.saturating_sub(4).clamp(32, 72),
                area.height.saturating_sub(hero_h + 4).clamp(8, 16),
            )
        } else {
            let mut rows: u16 = 2; // top + bottom inner breathing room
            if self.pills_visible() {
                rows += 2;
            }
            if self.using_other_user() {
                rows += 2;
            }
            if self.auth_prompt.is_challenge() {
                rows += 1;
            }
            rows += 1; // password input
            if has_status {
                rows += 2;
            }
            let h = (rows + 2).min(area.height.saturating_sub(hero_h + 3));
            (area.width.saturating_sub(4).clamp(32, 46), h.max(5))
        };

        let strip_block_h: u16 = if in_modal { 0 } else { 2 };
        let total_h = hero_h + card_h + strip_block_h;
        let top_y = area.y + area.height.saturating_sub(total_h) / 2;
        let card_x = area.x + (area.width - card_w) / 2;

        // 1. Minimalist Hero: clock, date, single centered Hayato wordmark
        {
            let y0 = top_y;
            let clock_w = (self.clock_str.chars().count() as u16).min(area.width);
            let cx = area.x + area.width.saturating_sub(clock_w) / 2;
            put_trunc(
                ctx.buf,
                cx,
                y0,
                &self.clock_str,
                clock_w,
                Style::default()
                    .fg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD),
            );

            let date_w = (self.date_str.chars().count() as u16).min(area.width);
            let dx = area.x + area.width.saturating_sub(date_w) / 2;
            put_trunc(
                ctx.buf,
                dx,
                y0 + 1,
                &self.date_str,
                date_w,
                ctx.design.style(TextStyle::Dim),
            );

            let hero_reduced = ctx.design.reduced_motion() || self.clock_fixed;
            if use_pixel_hero {
                draw_pixel_hero(ctx.buf, area, y0 + 3, ctx.design, self.anim_tick, hero_reduced);
            } else {
                draw_compact_hero(ctx.buf, area, y0 + 3, ctx.design, self.anim_tick, hero_reduced);
            }
        }

        // 2. Compact Login Card (border only, no title)
        let card_area = Rect::new(card_x, top_y + hero_h, card_w, card_h);
        let inner = draw_card_with(
            ctx.buf,
            card_area,
            ctx.design,
            None,
            ColorRole::Border,
            ctx.design.style(TextStyle::Title),
        );
        if inner.width < 16 || inner.height < 3 {
            *self.hits.borrow_mut() = hits;
            return;
        }
        fill_rect(
            ctx.buf,
            inner,
            Style::default().bg(ctx.design.color(ColorRole::Surface)),
        );

        match &self.mode {
            Mode::SessionPicker => {
                self.draw_picker_modal(inner, ctx.buf, ctx.design, false, &mut hits);
                *self.hits.borrow_mut() = hits;
                return;
            }
            Mode::ThemePicker => {
                self.draw_picker_modal(inner, ctx.buf, ctx.design, true, &mut hits);
                *self.hits.borrow_mut() = hits;
                return;
            }
            Mode::ConfirmPower {
                action,
                confirm_yes,
            } => {
                self.draw_confirm_modal(
                    inner,
                    ctx.buf,
                    ctx.design,
                    *action,
                    *confirm_yes,
                    &mut hits,
                );
                *self.hits.borrow_mut() = hits;
                return;
            }
            Mode::Login => {}
        }

        let pad_x: u16 = if inner.width >= 24 { 2 } else { 1 };
        let field_x = inner.x + pad_x;
        let field_w = inner.width.saturating_sub(pad_x * 2);
        let mut y = inner.y + if inner.height >= 5 { 1 } else { 0 };
        let bg_surface = ctx.design.color(ColorRole::Surface);

        // User pills row (hidden when show_user_pills = false)
        if self.pills_visible() && y < inner.bottom() {
            let mut px = field_x;
            let pill_limit = self.users.len().min(MAX_USER_PILLS);
            let mut pill_idx = 0usize;
            for idx in 0..=pill_limit {
                let is_other = idx == pill_limit;
                let label = if is_other {
                    "+ Other"
                } else {
                    self.users[idx].username.as_str()
                };
                let w = (label.chars().count() as u16) + 4;
                if px + w > field_x + field_w {
                    break;
                }
                let selected = if is_other {
                    self.using_other_user()
                } else {
                    self.selected_user == UserChoice::Existing(idx)
                };
                let focused = self.focus == Focus::Users && selected;
                let pill_style = if focused {
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Bg))
                        .bg(ctx.design.color(ColorRole::Accent))
                        .add_modifier(Modifier::BOLD)
                } else if selected {
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Accent))
                        .bg(ctx.design.color(ColorRole::Selection))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Text))
                        .bg(ctx.design.color(ColorRole::Bg))
                };
                let rect = Rect::new(px, y, w, 1);
                fill_rect(ctx.buf, rect, pill_style);
                put_trunc(ctx.buf, px + 2, y, label, w - 4, pill_style);
                let target_user_idx = if is_other { self.users.len() } else { idx };
                hits.user_pills[pill_idx] = Some((target_user_idx, rect));
                pill_idx += 1;
                px += w + 1;
            }
            hits.user_count = pill_idx;
            y += 2;
        }

        // Optional inline Username box when "+ Other" is active or pills are hidden
        if self.using_other_user() && y < inner.bottom() {
            let user_focused = self.focus == Focus::Username;
            let box_rect = Rect::new(field_x, y, field_w, 1);
            self.username_input.render_field(
                box_rect,
                ctx.buf,
                ctx.design,
                "",
                user_focused,
                self.cursor_blink_on,
                ColorRole::Bg,
                TextStyle::Body,
            );
            hits.username_box = Some(box_rect);
            y += 2;
        }

        // Password / PAM prompt input box
        if y < inner.bottom() {
            let pass_focused = self.focus == Focus::Password;
            if self.auth_prompt.is_challenge() && y + 1 < inner.bottom() {
                put_trunc(
                    ctx.buf,
                    field_x,
                    y,
                    self.auth_prompt.label(),
                    field_w,
                    ctx.design.style(TextStyle::Dim).bg(bg_surface),
                );
                y += 1;
            }

            let caps_reserve = if self.caps_lock { 8 } else { 0 };
            let input_w = field_w.saturating_sub(caps_reserve);
            if input_w >= 8 {
                let pass_rect = Rect::new(field_x, y, input_w, 1);
                self.secret_input.render_field(
                    pass_rect,
                    ctx.buf,
                    ctx.design,
                    "",
                    pass_focused,
                    self.cursor_blink_on,
                    ColorRole::Bg,
                    TextStyle::Body,
                );
                hits.password_box = Some(pass_rect);

                if self.caps_lock && pass_rect.right() + 7 <= field_x + field_w {
                    put(
                        ctx.buf,
                        pass_rect.right() + 1,
                        y,
                        "[CAPS]",
                        6,
                        Style::default()
                            .fg(ctx.design.color(ColorRole::Bg))
                            .bg(ctx.design.color(ColorRole::Warn))
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
            y += 2;
        }

        // Status / Spinner / PAM error (only drawn when active)
        if has_status && y < inner.bottom() {
            if self.runner.is_running() {
                let spin = self.runner.spinner();
                put(
                    ctx.buf,
                    field_x,
                    y,
                    spin,
                    2,
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Accent))
                        .bg(bg_surface),
                );
                put_trunc(
                    ctx.buf,
                    field_x + 3,
                    y,
                    "Authenticating\u{2026}",
                    field_w.saturating_sub(3),
                    ctx.design.style(TextStyle::Title).bg(bg_surface),
                );
            } else if let Some((kind, text)) = &self.status {
                let (glyph, role) = match kind {
                    StatusKind::Err => (Glyph::Warning.cell(), ColorRole::Danger),
                    StatusKind::Warn => (Glyph::Warning.cell(), ColorRole::Warn),
                    StatusKind::Ok => (Glyph::Check.cell(), ColorRole::Ok),
                    StatusKind::Info => (Glyph::Info.cell(), ColorRole::Accent),
                };
                put(
                    ctx.buf,
                    field_x,
                    y,
                    glyph,
                    2,
                    Style::default()
                        .fg(ctx.design.color(role))
                        .bg(bg_surface)
                        .add_modifier(Modifier::BOLD),
                );
                put_trunc(
                    ctx.buf,
                    field_x + 3,
                    y,
                    text,
                    field_w.saturating_sub(3),
                    Style::default().fg(ctx.design.color(role)).bg(bg_surface),
                );
            }
        }

        // 3. Single quiet ActionStrip centered below the card
        let strip_y = card_area.bottom() + 1;
        if strip_y < area.bottom() {
            let strip_w = if self.show_power_actions { 76 } else { 30 };
            let w = strip_w.min(area.width.saturating_sub(2));
            let sx = area.x + area.width.saturating_sub(w) / 2;
            let strip_area = Rect::new(sx, strip_y, w, 1);
            self.strip
                .render_on(strip_area, ctx.buf, ctx.design, ColorRole::Bg);
            hits.strip_row = Some(strip_area);
        }

        *self.hits.borrow_mut() = hits;
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, _design: &Design) -> Option<usize> {
        let hits = self.hits.borrow();
        if let Some(list_area) = hits.picker_list {
            let list = if self.mode == Mode::ThemePicker {
                &self.theme_list
            } else {
                &self.session_list
            };
            if let Some(row) = list.hit_row(list_area, x, y, _design) {
                return Some(1000 + row);
            }
            return None;
        }
        if let Some(r) = hits.confirm_cancel {
            if r.contains((x, y).into()) {
                return Some(200);
            }
        }
        if let Some(r) = hits.confirm_yes {
            if r.contains((x, y).into()) {
                return Some(201);
            }
        }
        for &(idx, r) in hits.user_pills.iter().take(hits.user_count).flatten() {
            if r.contains((x, y).into()) {
                return Some(10 + idx);
            }
        }
        if let Some(r) = hits.username_box {
            if r.contains((x, y).into()) {
                return Some(20);
            }
        }
        if let Some(r) = hits.password_box {
            if r.contains((x, y).into()) {
                return Some(21);
            }
        }
        if let Some(strip_area) = hits.strip_row {
            if let Some(idx) = self.strip.hit_index(strip_area, x, y) {
                return Some(300 + idx);
            }
        }
        None
    }

    fn click(&mut self, hit_id: Option<usize>) -> Outcome {
        let Some(id) = hit_id else {
            return Outcome::Continue;
        };
        self.reset_blink();
        if id >= 1000 {
            let row = id - 1000;
            match self.mode {
                Mode::SessionPicker => {
                    self.session_list.set_cursor(row);
                    if let Some(picked) = self.session_list.selected() {
                        if let Some(pos) = self
                            .sessions
                            .iter()
                            .position(|s| Some(s.name.as_str()) == picked.value.as_deref())
                        {
                            self.selected_session = pos;
                            self.refresh_strip();
                        }
                        self.mode = Mode::Login;
                        return Outcome::Consumed;
                    }
                }
                Mode::ThemePicker => {
                    self.theme_list.set_cursor(row);
                    if let Some(slug) = self
                        .theme_list
                        .selected()
                        .and_then(|r| r.value.as_deref())
                        .map(str::to_string)
                    {
                        self.apply_theme_slug(&slug);
                        self.mode = Mode::Login;
                        return Outcome::Consumed;
                    }
                }
                _ => {}
            }
            return Outcome::Consumed;
        }
        if (300..400).contains(&id) {
            let idx = id - 300;
            if let Some(item) = self.strip.items().get(idx) {
                return self.trigger_action(item.id);
            }
            return Outcome::Continue;
        }
        match id {
            200 => {
                self.mode = Mode::Login;
                Outcome::Consumed
            }
            201 => {
                if let Mode::ConfirmPower { action, .. } = self.mode {
                    return self.execute_power(action);
                }
                Outcome::Consumed
            }
            20 => {
                self.focus = Focus::Username;
                self.strip.set_focused(false);
                Outcome::Consumed
            }
            21 => {
                self.focus = Focus::Password;
                self.strip.set_focused(false);
                Outcome::Consumed
            }
            x if (10..100).contains(&x) => {
                self.select_user(x - 10);
                self.strip.set_focused(false);
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        self.reset_blink();
        self.caps_lock = key.state.contains(KeyEventState::CAPS_LOCK);

        // If authentication is currently running, only allow Esc to cancel locally.
        if self.runner.is_running() {
            if key.code == KeyCode::Esc {
                self.status = Some((StatusKind::Warn, "Authentication cancelled".into()));
                return Outcome::Consumed;
            }
            return Outcome::Consumed;
        }

        // Modal overlays handle keys exclusively first.
        match self.mode {
            Mode::ConfirmPower {
                action,
                mut confirm_yes,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        self.mode = Mode::Login;
                    }
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                        confirm_yes = !confirm_yes;
                        self.mode = Mode::ConfirmPower {
                            action,
                            confirm_yes,
                        };
                    }
                    KeyCode::Enter => {
                        if confirm_yes {
                            return self.execute_power(action);
                        } else {
                            self.mode = Mode::Login;
                        }
                    }
                    _ => {}
                }
                return Outcome::Consumed;
            }
            Mode::SessionPicker => {
                let h = self.picker_height.get();
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Login;
                        return Outcome::Consumed;
                    }
                    KeyCode::Backspace | KeyCode::Char(_)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.session_list.type_filter(key);
                        return Outcome::Consumed;
                    }
                    _ => {
                        if let ListAction::Activated(idx) = self.session_list.handle(key, h, design)
                        {
                            if let Some(row) = self.session_list.rows().get(idx) {
                                if let Some(pos) = self
                                    .sessions
                                    .iter()
                                    .position(|s| Some(s.name.as_str()) == row.value.as_deref())
                                {
                                    self.selected_session = pos;
                                    self.refresh_strip();
                                }
                            }
                            self.mode = Mode::Login;
                        }
                        return Outcome::Consumed;
                    }
                }
            }
            Mode::ThemePicker => {
                let h = self.picker_height.get();
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Login;
                        return Outcome::Consumed;
                    }
                    KeyCode::Backspace | KeyCode::Char(_)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if self.theme_list.type_filter(key) {
                            if let Some(slug) = self
                                .theme_list
                                .selected()
                                .and_then(|r| r.value.as_deref())
                                .map(str::to_string)
                            {
                                self.apply_theme_slug(&slug);
                            }
                        }
                        return Outcome::Consumed;
                    }
                    _ => {
                        match self.theme_list.handle(key, h, design) {
                            ListAction::Moved => {
                                if let Some(slug) = self
                                    .theme_list
                                    .selected()
                                    .and_then(|r| r.value.as_deref())
                                    .map(str::to_string)
                                {
                                    self.apply_theme_slug(&slug);
                                }
                            }
                            ListAction::Activated(_) => {
                                if let Some(slug) = self
                                    .theme_list
                                    .selected()
                                    .and_then(|r| r.value.as_deref())
                                    .map(str::to_string)
                                {
                                    self.apply_theme_slug(&slug);
                                }
                                self.mode = Mode::Login;
                            }
                            ListAction::Ignored => {}
                        }
                        return Outcome::Consumed;
                    }
                }
            }
            Mode::Login => {}
        }

        // Global greeter shortcuts active from any field.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (ctrl, key.code) {
            (false, KeyCode::F(2)) | (true, KeyCode::Char('s')) => {
                return self.trigger_action("session");
            }
            (false, KeyCode::F(4)) => {
                self.cycle_theme();
                return Outcome::Consumed;
            }
            (true, KeyCode::Char('t')) => {
                return self.trigger_action("theme");
            }
            (true, KeyCode::Char('z')) if self.show_power_actions => {
                return self.trigger_action("sleep");
            }
            (true, KeyCode::Char('r')) if self.show_power_actions => {
                return self.trigger_action("reboot");
            }
            (true, KeyCode::Char('p')) | (false, KeyCode::F(12)) if self.show_power_actions => {
                return self.trigger_action("poweroff");
            }
            _ => {}
        }

        // ActionStrip keyboard mode when Strip is focused.
        if self.focus == Focus::Strip {
            match self.strip.handle(key) {
                ActionStripEvent::Activated(id) => {
                    self.focus = Focus::Password;
                    return self.trigger_action(id);
                }
                ActionStripEvent::Consumed => {
                    if !self.strip.is_focused() {
                        self.focus = Focus::Password;
                    }
                    return Outcome::Consumed;
                }
                ActionStripEvent::Ignored => {}
            }
        }

        // Escape inside real greetd never closes the VT7 greeter; only clears state.
        if key.code == KeyCode::Esc {
            if !self.secret_input.text().is_empty()
                || self.auth_prompt.is_challenge()
                || self.status.is_some()
            {
                self.reset_pam_state();
                self.status = None;
                self.focus = Focus::Password;
                return Outcome::Consumed;
            }
            if !self.preview {
                return Outcome::Consumed;
            }
            return Outcome::Continue;
        }

        // Field cycle with Tab / BackTab
        let pills = self.pills_visible();
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            let reverse = key.code == KeyCode::BackTab;
            self.focus = match (self.focus, reverse, pills, self.using_other_user()) {
                (Focus::Password, false, _, _) => {
                    self.strip.set_focused(true);
                    Focus::Strip
                }
                (Focus::Strip, false, true, _) => {
                    self.strip.set_focused(false);
                    Focus::Users
                }
                (Focus::Strip, false, false, _) => {
                    self.strip.set_focused(false);
                    Focus::Username
                }
                (Focus::Users, false, _, true) => Focus::Username,
                (Focus::Users, false, _, false) => Focus::Password,
                (Focus::Username, false, _, _) => Focus::Password,
                (Focus::Password, true, _, true) => Focus::Username,
                (Focus::Password, true, true, false) => Focus::Users,
                (Focus::Password, true, false, false) => Focus::Username,
                (Focus::Username, true, true, _) => Focus::Users,
                (Focus::Username, true, false, _) => {
                    self.strip.set_focused(true);
                    Focus::Strip
                }
                (Focus::Users, true, _, _) => {
                    self.strip.set_focused(true);
                    Focus::Strip
                }
                (Focus::Strip, true, _, _) => {
                    self.strip.set_focused(false);
                    Focus::Password
                }
            };
            return Outcome::Consumed;
        }

        match self.focus {
            Focus::Users => match key.code {
                KeyCode::Left => {
                    let max = self.users.len();
                    let current = match self.selected_user {
                        UserChoice::Existing(idx) => idx,
                        UserChoice::Other => max,
                    };
                    let next = if current == 0 { max } else { current - 1 };
                    self.select_user(next);
                    self.focus = Focus::Users;
                    Outcome::Consumed
                }
                KeyCode::Right => {
                    let max = self.users.len();
                    let current = match self.selected_user {
                        UserChoice::Existing(idx) => idx,
                        UserChoice::Other => max,
                    };
                    let next = if current >= max { 0 } else { current + 1 };
                    self.select_user(next);
                    self.focus = Focus::Users;
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Enter => {
                    self.focus = if self.using_other_user() {
                        Focus::Username
                    } else {
                        Focus::Password
                    };
                    Outcome::Consumed
                }
                _ => Outcome::Continue,
            },
            Focus::Username => match key.code {
                KeyCode::Enter | KeyCode::Down => {
                    self.focus = Focus::Password;
                    Outcome::Consumed
                }
                KeyCode::Up => {
                    if pills {
                        self.focus = Focus::Users;
                    }
                    Outcome::Consumed
                }
                _ => match self.username_input.handle(key) {
                    InputAction::Changed | InputAction::Submitted => Outcome::Consumed,
                    InputAction::Ignored | InputAction::Cancelled => Outcome::Continue,
                },
            },
            Focus::Password => match key.code {
                KeyCode::Up => {
                    self.focus = if self.using_other_user() || !pills {
                        Focus::Username
                    } else {
                        Focus::Users
                    };
                    Outcome::Consumed
                }
                KeyCode::Down => {
                    self.focus = Focus::Strip;
                    self.strip.set_focused(true);
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    self.submit_auth();
                    Outcome::Consumed
                }
                _ => match self.secret_input.handle(key) {
                    InputAction::Changed => {
                        self.status = None;
                        Outcome::Consumed
                    }
                    InputAction::Submitted => {
                        self.submit_auth();
                        Outcome::Consumed
                    }
                    InputAction::Ignored | InputAction::Cancelled => Outcome::Continue,
                },
            },
            Focus::Strip => Outcome::Continue,
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_clock(clock_24h: bool) -> (String, String, u64) {
    let now = Local::now();
    let time = if clock_24h {
        now.format("%H:%M").to_string()
    } else {
        now.format("%I:%M %p").to_string()
    };
    (time, now.format("%a, %d %b %Y").to_string(), epoch_secs())
}

fn read_hostname() -> String {
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "hayato".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::greeter::SessionKind;
    use hayato_ui::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn user_choice_and_auth_prompt_name_their_states() {
        assert_ne!(UserChoice::Existing(0), UserChoice::Other);
        assert_eq!(UserChoice::Other, UserChoice::Other);

        assert!(!AuthPrompt::Password.is_challenge());
        assert_eq!(AuthPrompt::Password.label(), "Password");

        let challenge = AuthPrompt::Challenge {
            label: "One-time code".into(),
        };
        assert!(challenge.is_challenge());
        assert_eq!(challenge.label(), "One-time code");
    }

    fn test_screen_with_config(preview: bool, config: GreeterConfig) -> GreeterScreen {
        let theme = load_theme_by_slug("solarized").unwrap();
        let setup = GreeterSetup {
            users: vec![UserEntry {
                username: "brunux".into(),
                display_name: "Bruno".into(),
                home: "/home/brunux".into(),
            }],
            sessions: vec![
                SessionEntry {
                    name: "Sway".into(),
                    exec: vec!["sway".into()],
                    kind: SessionKind::Wayland,
                    desktop_names: vec!["sway".into()],
                    comment: None,
                },
                SessionEntry {
                    name: "Bash".into(),
                    exec: vec!["/bin/bash".into()],
                    kind: SessionKind::X11,
                    desktop_names: vec![],
                    comment: None,
                },
            ],
            state: GreeterState {
                last_user: Some("brunux".into()),
                ..Default::default()
            },
            state_path: None,
            auth_marker: None,
            theme_slug: "solarized".into(),
            theme,
            greeting: "light. fast. fun.".into(),
            backend: Backend::Preview,
            preview,
            density: Density::Normal,
            reduced_motion: true,
            config,
        };
        let mut scr = GreeterScreen::new(setup);
        scr.set_clock_for_test("18:30", "Sun, 15 Nov 2026", "hayato");
        scr
    }

    fn test_screen(preview: bool) -> GreeterScreen {
        test_screen_with_config(preview, GreeterConfig::default())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render_text(scr: &mut GreeterScreen, w: u16, h: u16) -> String {
        let design = Design::solarized().with_reduced_motion(true);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut app = App::new(Header::new("Hayato")).design(design).screen(scr);
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn power_action_requires_cancel_first_confirmation() {
        let design = Design::solarized().with_reduced_motion(true);
        let mut scr = test_screen(false);

        // Ctrl+R enters ConfirmPower with Cancel selected by default.
        let o = scr.key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &design,
        );
        assert_eq!(o, Outcome::Consumed);
        assert_eq!(
            scr.mode,
            Mode::ConfirmPower {
                action: PowerAction::Reboot,
                confirm_yes: false
            }
        );

        // Pressing Enter immediately cancels safely instead of rebooting.
        scr.key(key(KeyCode::Enter), &design);
        assert_eq!(scr.mode, Mode::Login);

        // Re-enter and press Right -> confirm_yes = true.
        scr.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &design,
        );
        scr.key(key(KeyCode::Right), &design);
        assert_eq!(
            scr.mode,
            Mode::ConfirmPower {
                action: PowerAction::Poweroff,
                confirm_yes: true
            }
        );
        scr.key(key(KeyCode::Esc), &design);
        assert_eq!(scr.mode, Mode::Login);
    }

    #[test]
    fn real_greeter_consumes_escape_instead_of_closing() {
        let design = Design::solarized().with_reduced_motion(true);
        let mut scr = test_screen(false);
        assert_eq!(scr.key(key(KeyCode::Esc), &design), Outcome::Consumed);

        let mut preview_scr = test_screen(true);
        assert_eq!(
            preview_scr.key(key(KeyCode::Esc), &design),
            Outcome::Continue
        );
    }

    #[test]
    fn f4_cycles_theme_and_updates_dynamic_design() {
        let design = Design::solarized().with_reduced_motion(true);
        let mut scr = test_screen(true);
        let first_palette = *scr.design().unwrap().palette();
        scr.key(key(KeyCode::F(4)), &design);
        let next_palette = *scr.design().unwrap().palette();
        assert_ne!(first_palette, next_palette);
    }

    #[test]
    fn renders_cleanly_at_80x24_and_152x45() {
        let mut scr = test_screen(true);
        let small = render_text(&mut scr, 80, 24);
        assert!(small.contains("Hayato"));
        assert!(small.contains("18:30"));
        assert!(small.contains("brunux"));
        assert!(small.contains("Theme"));
        assert!(!small.contains("z_"));
        assert!(!small.contains("Hayato_"));
        assert!(!small.contains("Sway (Wayland)"));
        assert!(!small.contains("Settings"));
        assert!(!small.contains("^G"));

        let large = render_text(&mut scr, 152, 45);
        assert!(large.contains("18:30"));
        assert!(large.contains("\u{2588}"));
        assert!(large.contains("Shut down"));
        assert!(!large.contains("Settings"));
    }

    #[test]
    fn pixel_hero_animation_phases_and_reduced_motion() {
        for idx in 0..PIXEL_LETTERS.len() {
            assert_eq!(hero_letter_state(idx, 0, true), GlyphState::Locked(idx));
            assert_eq!(hero_letter_state(idx, 42, true), GlyphState::Locked(idx));
            // Word starts already formed on entry even when motion is enabled.
            assert_eq!(hero_letter_state(idx, 0, false), GlyphState::Locked(idx));
        }
        // Periodic hacker glitch pulse triggers at tick 42 for letter 1, settles by tick 44.
        assert!(matches!(hero_letter_state(1, 42, false), GlyphState::Glitch(_)));
        assert_eq!(hero_letter_state(1, 44, false), GlyphState::Locked(1));
    }

    #[test]
    fn disabled_user_pills_and_power_actions_render_cleanly() {
        let cfg = GreeterConfig {
            show_user_pills: false,
            remember_user: false,
            show_power_actions: false,
            ..Default::default()
        };
        let mut scr = test_screen_with_config(true, cfg);
        assert_eq!(scr.focus, Focus::Username);
        let out = render_text(&mut scr, 100, 30);
        assert!(!out.contains("+ Other"));
        assert!(out.contains("Theme"));
        assert!(!out.contains("Shut down"));
        assert!(!out.contains("Settings"));
    }

    #[test]
    fn password_cursor_blinks_and_other_pill_remains_legible() {
        let design = Design::solarized();
        let mut scr = test_screen(true);
        scr.reduced_motion = false;
        scr.theme.motion = hayato_theme::Motion::Full;
        assert!(scr.needs_tick().is_some());
        assert!(scr.cursor_blink_on);

        scr.last_blink -= Duration::from_millis(600);
        scr.tick();
        assert!(!scr.cursor_blink_on);

        // Typing resets the blink phase immediately to solid-on.
        scr.key(key(KeyCode::Char('a')), &design);
        assert!(scr.cursor_blink_on);

        // Verify the rendered buffer for focused input contrast and "+ Other" pill legibility.
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        let mut app = App::new(Header::new("Hayato"))
            .design(design.clone())
            .screen(&mut scr);
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer();

        let pass_rect = scr
            .hits
            .borrow()
            .password_box
            .expect("password box hit rect");
        assert_eq!(
            buf.cell((pass_rect.x, pass_rect.y)).unwrap().symbol(),
            "\u{258d}",
            "focused password box renders left accent focus bar"
        );
        assert_eq!(
            buf.cell((pass_rect.x + 2, pass_rect.y)).unwrap().bg,
            design.color(ColorRole::Accent),
            "focused password box highlights active cursor cell"
        );

        let other_pill = scr.hits.borrow().user_pills[1]
            .expect("+ Other pill rect")
            .1;
        let other_cell = buf.cell((other_pill.x + 2, other_pill.y)).unwrap();
        assert_ne!(
            other_cell.fg, other_cell.bg,
            "unselected + Other pill text must contrast cleanly against pill fill"
        );
    }

    #[test]
    fn power_failure_formats_concise_status_without_duplicate_exit_prefix() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            let status = ExitStatus::from_raw(1 << 8);
            let empty = format_power_failure(PowerAction::Reboot, status, b"");
            assert_eq!(empty, "Reboot refused by systemd (exit 1)");
            let with_err = format_power_failure(
                PowerAction::Poweroff,
                status,
                b"User brunux is logged in on tty7.\nAccess denied\n",
            );
            assert_eq!(with_err, "Shut down refused by systemd (Access denied)");
        }
    }
}
