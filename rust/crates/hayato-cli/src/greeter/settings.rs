//! Interactive settings editor for `hayato-greeter` (`/etc/hayato/greeter.toml`).
//!
//! Lets users define boot greeter defaults (follow user theme vs fixed theme,
//! default theme across all 16 built-in themes, show/hide user pills,
//! remember last user/session, default session, 12h/24h clock format, power
//! actions visibility, and custom greeting text) with instant in-window feedback
//! and live launch into greeter preview.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hayato_theme::{Theme, builtin};
use hayato_ui::Glyph;
use hayato_ui::app::{BodyCtx, Outcome, Screen};
use hayato_ui::design::{ColorRole, Density, Design, TextStyle};
use hayato_ui::widgets::{
    ActionItem, ActionStrip, ActionStripEvent, Header, Input, InputAction, KeyHint, ListAction,
    ListState, Row, draw_card_with, draw_filter_bar, put, put_trunc,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::ipc::Backend;
use super::screen::{GreeterScreen, GreeterSetup};
use super::{
    DEFAULT_GREETING, GreeterConfig, GreeterState, SessionEntry, default_config_path,
    default_state_path, discover_sessions, discover_users, load_greeter_config, load_theme_by_slug,
    resolve_theme_and_greeting, resolve_user_home, save_greeter_config,
};
use crate::Failure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsMode {
    Main,
    ThemePicker,
    SessionPicker,
    EditGreeting,
    ConfirmReset { confirm_yes: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Strip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusKind {
    Info,
    Ok,
    Err,
}

#[derive(Clone, Copy, Debug, Default)]
struct HitRegions {
    main_list: Option<Rect>,
    picker_list: Option<Rect>,
    strip: Option<Rect>,
    confirm_cancel: Option<Rect>,
    confirm_yes: Option<Rect>,
}

pub struct GreeterSettingsScreen {
    config: GreeterConfig,
    config_path: PathBuf,
    sessions: Vec<SessionEntry>,
    theme: Theme,
    theme_slug: String,
    theme_slugs: Vec<&'static str>,

    main_list: ListState,
    theme_list: ListState,
    session_list: ListState,
    greeting_input: Input,
    strip: ActionStrip,

    mode: SettingsMode,
    focus: Focus,
    status: Option<(StatusKind, String)>,
    allow_preview_launch: bool,
    open_preview: bool,
    cursor_blink_on: bool,
    last_blink: Instant,

    density: Density,
    reduced_motion: bool,
    header: Header,
    keybar_main: Vec<KeyHint>,
    keybar_picker: Vec<KeyHint>,
    keybar_input: Vec<KeyHint>,
    keybar_confirm: Vec<KeyHint>,
    hits: RefCell<HitRegions>,
    list_height: Cell<u16>,
    picker_height: Cell<u16>,
}

impl GreeterSettingsScreen {
    pub fn new(
        config_path: PathBuf,
        sessions: Vec<SessionEntry>,
        density: Density,
        reduced_motion: bool,
        allow_preview_launch: bool,
    ) -> Self {
        let config = load_greeter_config(&config_path);
        let initial_slug = config
            .theme
            .as_deref()
            .filter(|s| load_theme_by_slug(s).is_some())
            .unwrap_or("tokyo-night")
            .to_string();
        let theme = load_theme_by_slug(&initial_slug)
            .or_else(|| load_theme_by_slug("tokyo-night"))
            .expect("tokyo-night built-in theme must exist");

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
        if let Some(pos) = theme_slugs.iter().position(|s| *s == initial_slug) {
            theme_list.set_cursor(pos);
        }

        let mut session_list = ListState::new().with_empty("no sessions");
        let mut s_rows = vec![
            Row::new("Auto (first available)")
                .glyph(Glyph::Window)
                .detail("recommended")
                .value(""),
        ];
        for s in &sessions {
            s_rows.push(
                Row::new(s.name.clone())
                    .glyph(Glyph::Window)
                    .detail(s.kind.label())
                    .value(s.name.clone()),
            );
        }
        session_list.set_rows(s_rows);

        let mut main_list = ListState::new().with_empty("no settings");
        main_list.set_rows(build_setting_rows(&config));

        let strip_preview_label = if allow_preview_launch {
            "Preview"
        } else {
            "Back to preview"
        };
        let strip_done_label = if allow_preview_launch {
            "Done"
        } else {
            "Apply & Back"
        };
        let strip = ActionStrip::new(vec![
            ActionItem::new("preview", "^P", strip_preview_label).glyph(Glyph::Eye),
            ActionItem::new("reset", "^R", "Reset defaults").glyph(Glyph::Undo),
            ActionItem::new("done", "Esc", strip_done_label).glyph(Glyph::Check),
        ]);

        let mut scr = Self {
            config,
            config_path,
            sessions,
            theme,
            theme_slug: initial_slug,
            theme_slugs,
            main_list,
            theme_list,
            session_list,
            greeting_input: Input::new(),
            strip,
            mode: SettingsMode::Main,
            focus: Focus::List,
            status: None,
            allow_preview_launch,
            open_preview: false,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            density,
            reduced_motion,
            header: Header::new("Hayato").subtitle("Greeter Settings"),
            keybar_main: vec![
                KeyHint::new("Enter", "toggle / edit"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Esc", "back"),
            ],
            keybar_picker: vec![
                KeyHint::new("Enter", "apply"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "cancel"),
            ],
            keybar_input: vec![KeyHint::new("Enter", "save"), KeyHint::new("Esc", "cancel")],
            keybar_confirm: vec![
                KeyHint::new("\u{2190}\u{2192}", "choose"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
            hits: RefCell::new(HitRegions::default()),
            list_height: Cell::new(10),
            picker_height: Cell::new(10),
        };
        scr.sync_session_cursor();
        scr
    }

    pub fn config(&self) -> &GreeterConfig {
        &self.config
    }

    pub fn take_open_preview(&mut self) -> bool {
        let was = self.open_preview;
        self.open_preview = false;
        was
    }

    fn reset_blink(&mut self) {
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();
    }

    fn rebuild_rows(&mut self) {
        let cursor = self.main_list.cursor();
        self.main_list.set_rows(build_setting_rows(&self.config));
        self.main_list.set_cursor(cursor);
    }

    fn persist(&mut self, summary: &str) {
        self.rebuild_rows();
        match save_greeter_config(&self.config_path, &self.config) {
            Ok(()) => {
                self.status = Some((
                    StatusKind::Ok,
                    format!("{summary} \u{00b7} saved {}", self.config_path.display()),
                ));
            }
            Err(err) => {
                self.status = Some((StatusKind::Err, err));
            }
        }
    }

    fn sync_theme_from_config(&mut self) {
        let slug = self
            .config
            .theme
            .as_deref()
            .filter(|s| load_theme_by_slug(s).is_some())
            .unwrap_or("tokyo-night");
        if let Some(t) = load_theme_by_slug(slug) {
            self.theme_slug = slug.to_string();
            self.theme = t;
        }
        if let Some(pos) = self.theme_slugs.iter().position(|s| *s == self.theme_slug) {
            self.theme_list.set_cursor(pos);
        }
    }

    fn preview_highlighted_theme(&mut self) {
        if let Some(slug) = self
            .theme_list
            .selected()
            .and_then(|r| r.value.as_deref())
            .map(str::to_string)
            && let Some(t) = load_theme_by_slug(&slug)
        {
            self.theme_slug = slug;
            self.theme = t;
        }
    }

    fn sync_session_cursor(&mut self) {
        let idx = match self.config.default_session.as_deref() {
            Some(name) => self
                .sessions
                .iter()
                .position(|s| s.name == name)
                .map_or(0, |pos| pos + 1),
            None => 0,
        };
        self.session_list.set_cursor(idx);
    }

    fn activate_selected_row(&mut self) -> Outcome {
        let Some(key) = self
            .main_list
            .selected()
            .and_then(|r| r.value.as_deref())
            .map(str::to_string)
        else {
            return Outcome::Continue;
        };
        match key.as_str() {
            "follow_user_theme" => {
                self.config.follow_user_theme = !self.config.follow_user_theme;
                let state = if self.config.follow_user_theme {
                    "Following last user's desktop theme"
                } else {
                    "Using fixed default greeter theme"
                };
                self.persist(state);
            }
            "theme" => {
                self.theme_list.set_filter("");
                self.sync_theme_from_config();
                self.mode = SettingsMode::ThemePicker;
            }
            "show_user_pills" => {
                self.config.show_user_pills = !self.config.show_user_pills;
                let msg = if self.config.show_user_pills {
                    "User pills enabled"
                } else {
                    "User pills hidden (manual login input)"
                };
                self.persist(msg);
            }
            "remember_user" => {
                self.config.remember_user = !self.config.remember_user;
                let msg = if self.config.remember_user {
                    "Remember last user enabled"
                } else {
                    "Remember last user disabled"
                };
                self.persist(msg);
            }
            "default_session" => {
                self.session_list.set_filter("");
                self.sync_session_cursor();
                self.mode = SettingsMode::SessionPicker;
            }
            "remember_session" => {
                self.config.remember_session = !self.config.remember_session;
                let msg = if self.config.remember_session {
                    "Remember last session enabled"
                } else {
                    "Remember last session disabled"
                };
                self.persist(msg);
            }
            "clock_24h" => {
                self.config.clock_24h = !self.config.clock_24h;
                let msg = if self.config.clock_24h {
                    "Clock set to 24-hour format"
                } else {
                    "Clock set to 12-hour format"
                };
                self.persist(msg);
            }
            "show_power_actions" => {
                self.config.show_power_actions = !self.config.show_power_actions;
                let msg = if self.config.show_power_actions {
                    "Power actions shown on greeter"
                } else {
                    "Power actions hidden on greeter"
                };
                self.persist(msg);
            }
            "greeting" => {
                self.greeting_input = Input::new();
                if let Some(cur) = &self.config.greeting {
                    self.greeting_input.set(cur);
                }
                self.reset_blink();
                self.mode = SettingsMode::EditGreeting;
            }
            _ => {}
        }
        Outcome::Consumed
    }

    fn trigger_strip_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "preview" => {
                if self.allow_preview_launch {
                    self.open_preview = true;
                }
                Outcome::Done(if self.allow_preview_launch { 0 } else { 1 })
            }
            "reset" => {
                self.mode = SettingsMode::ConfirmReset { confirm_yes: false };
                Outcome::Consumed
            }
            "done" => Outcome::Done(1),
            _ => Outcome::Continue,
        }
    }

    fn draw_summary_card(&self, area: Rect, ctx: &mut BodyCtx) {
        let inner = draw_card_with(
            ctx.buf,
            area,
            ctx.design,
            Some(" Greeter defaults "),
            ColorRole::Border,
            ctx.design.style(TextStyle::Title),
        );
        if inner.height == 0 || inner.width < 12 {
            return;
        }
        let theme_mode = if self.config.follow_user_theme {
            format!("Follow user (fallback: {})", self.theme.name)
        } else {
            format!("Fixed: {}", self.theme.name)
        };
        let label = format!("{}  {}", Glyph::Gear.cell(), theme_mode);
        put_trunc(
            ctx.buf,
            inner.x,
            inner.y,
            &label,
            inner.width,
            Style::default()
                .fg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
        );

        if inner.height >= 2 {
            let pills_txt = if self.config.show_user_pills {
                "pills on"
            } else {
                "pills off"
            };
            let clock_txt = if self.config.clock_24h { "24h" } else { "12h" };
            let summary = format!(
                "{pills_txt} \u{00b7} clock {clock_txt} \u{00b7} {}",
                self.config_path.display()
            );
            put_trunc(
                ctx.buf,
                inner.x,
                inner.y + 1,
                &summary,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
        }
    }

    fn draw_modal_picker(&self, area: Rect, ctx: &mut BodyCtx, is_theme: bool) -> Option<Rect> {
        let title = if is_theme {
            Some(" Default greeter theme (live preview) ")
        } else {
            Some(" Default desktop session ")
        };
        let inner = draw_card_with(
            ctx.buf,
            area,
            ctx.design,
            title,
            ColorRole::Accent,
            ctx.design.style(TextStyle::Title),
        );
        if inner.height < 4 {
            return None;
        }
        let list = if is_theme {
            &self.theme_list
        } else {
            &self.session_list
        };
        let filter_rect = Rect::new(inner.x, inner.y, inner.width, 1);
        draw_filter_bar(
            ctx.buf,
            filter_rect,
            ctx.design,
            list.filter(),
            "type to filter",
            list.visible_len(),
            list.rows().len(),
        );
        let list_rect = Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        );
        self.picker_height.set(list_rect.height);
        list.render(list_rect, ctx.buf, ctx.design, true);
        Some(list_rect)
    }

    fn draw_edit_greeting(&self, area: Rect, ctx: &mut BodyCtx) {
        let inner = draw_card_with(
            ctx.buf,
            area,
            ctx.design,
            Some(" Custom greeting banner "),
            ColorRole::Accent,
            ctx.design.style(TextStyle::Title),
        );
        if inner.height >= 2 {
            let hint = format!("Leave empty to restore default ({DEFAULT_GREETING})");
            put_trunc(
                ctx.buf,
                inner.x,
                inner.y,
                &hint,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
        }
        if inner.height >= 4 {
            let box_rect = Rect::new(inner.x, inner.y + 2, inner.width, 1);
            self.greeting_input.render_field(
                box_rect,
                ctx.buf,
                ctx.design,
                DEFAULT_GREETING,
                true,
                self.cursor_blink_on,
                ColorRole::Selection,
                TextStyle::Body,
            );
        }
    }

    fn draw_confirm_reset(&self, area: Rect, ctx: &mut BodyCtx, confirm_yes: bool) -> HitRegions {
        let mut hits = HitRegions::default();
        let inner = draw_card_with(
            ctx.buf,
            area,
            ctx.design,
            Some(" Reset all greeter settings to defaults? "),
            ColorRole::Warn,
            ctx.design.style(TextStyle::Title),
        );
        if inner.height >= 1 {
            put_trunc(
                ctx.buf,
                inner.x,
                inner.y,
                "Follow user theme, user pills, 24h clock, and power actions will all be re-enabled.",
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
        }
        if inner.height >= 3 {
            let y = inner.y + 2;
            let cancel_label = " [ Cancel ] ";
            let yes_label = " [ Reset defaults ] ";
            let cancel_style = if !confirm_yes {
                Style::default()
                    .fg(ctx.design.color(ColorRole::Bg))
                    .bg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Dim)
            };
            let yes_style = if confirm_yes {
                Style::default()
                    .fg(ctx.design.color(ColorRole::Bg))
                    .bg(ctx.design.color(ColorRole::Danger))
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Danger)
            };

            let cancel_w = (cancel_label.chars().count() as u16).min(inner.width);
            let c_rect = Rect::new(inner.x, y, cancel_w, 1);
            put_trunc(
                ctx.buf,
                c_rect.x,
                y,
                cancel_label,
                c_rect.width,
                cancel_style,
            );
            hits.confirm_cancel = Some(c_rect);

            let yes_x = c_rect.right() + 2;
            if yes_x + 6 <= inner.right() {
                let yes_w = (yes_label.chars().count() as u16).min(inner.right() - yes_x);
                let y_rect = Rect::new(yes_x, y, yes_w, 1);
                put_trunc(ctx.buf, y_rect.x, y, yes_label, y_rect.width, yes_style);
                hits.confirm_yes = Some(y_rect);
            }
        }
        hits
    }
}

impl Screen for GreeterSettingsScreen {
    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.mode {
            SettingsMode::Main => Some(&self.keybar_main),
            SettingsMode::ThemePicker | SettingsMode::SessionPicker => Some(&self.keybar_picker),
            SettingsMode::EditGreeting => Some(&self.keybar_input),
            SettingsMode::ConfirmReset { .. } => Some(&self.keybar_confirm),
        }
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            SettingsMode::Main => "main",
            SettingsMode::ThemePicker => "theme-picker",
            SettingsMode::SessionPicker => "session-picker",
            SettingsMode::EditGreeting => "edit-greeting",
            SettingsMode::ConfirmReset { .. } => "confirm-reset",
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
        let blink_active = matches!(self.mode, SettingsMode::EditGreeting)
            && !self.design().is_some_and(|d| d.reduced_motion());
        blink_active.then_some(Duration::from_millis(500))
    }

    fn tick(&mut self) -> Outcome {
        if matches!(self.mode, SettingsMode::EditGreeting)
            && !self.design().is_some_and(|d| d.reduced_motion())
            && self.last_blink.elapsed() >= Duration::from_millis(500)
        {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let mut hits = HitRegions::default();
        let area = ctx.area;
        if area.width < 24 || area.height < 10 {
            return;
        }

        match self.mode {
            SettingsMode::ThemePicker => {
                hits.picker_list = self.draw_modal_picker(area, ctx, true);
                *self.hits.borrow_mut() = hits;
                return;
            }
            SettingsMode::SessionPicker => {
                hits.picker_list = self.draw_modal_picker(area, ctx, false);
                *self.hits.borrow_mut() = hits;
                return;
            }
            SettingsMode::EditGreeting => {
                self.draw_edit_greeting(area, ctx);
                *self.hits.borrow_mut() = hits;
                return;
            }
            SettingsMode::ConfirmReset { confirm_yes } => {
                hits = self.draw_confirm_reset(area, ctx, confirm_yes);
                *self.hits.borrow_mut() = hits;
                return;
            }
            SettingsMode::Main => {}
        }

        let strip_rect = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
        self.strip.render(strip_rect, ctx.buf, ctx.design);
        hits.strip = Some(strip_rect);

        let summary_y = area.y + 2;
        let summary_h = if area.height >= 18 { 4 } else { 3 };
        let summary_area = Rect::new(area.x, summary_y, area.width, summary_h);
        self.draw_summary_card(summary_area, ctx);

        let body_y = summary_y + summary_h + 1;
        let footer_h = if self.status.is_some() { 2 } else { 0 };
        let list_h = area.bottom().saturating_sub(body_y + footer_h);
        if list_h >= 3 {
            let list_area = Rect::new(area.x, body_y, area.width, list_h);
            let inner = draw_card_with(
                ctx.buf,
                list_area,
                ctx.design,
                Some(" Settings "),
                ColorRole::Border,
                ctx.design.style(TextStyle::Title),
            );
            self.list_height.set(inner.height);
            hits.main_list = Some(inner);
            self.main_list
                .render(inner, ctx.buf, ctx.design, self.focus == Focus::List);
        }

        if let Some((kind, msg)) = &self.status
            && area.bottom() >= 2
        {
            let status_y = area.bottom().saturating_sub(1);
            let style = match kind {
                StatusKind::Ok => Style::default()
                    .fg(ctx.design.color(ColorRole::Ok))
                    .add_modifier(Modifier::BOLD),
                StatusKind::Err => ctx.design.style(TextStyle::Danger),
                StatusKind::Info => ctx.design.style(TextStyle::Dim),
            };
            put_trunc(
                ctx.buf,
                area.x + 1,
                status_y,
                msg,
                area.width.saturating_sub(2),
                style,
            );
        }

        *self.hits.borrow_mut() = hits;
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let hits = self.hits.borrow();
        if let Some(list_area) = hits.picker_list {
            let list = if self.mode == SettingsMode::ThemePicker {
                &self.theme_list
            } else {
                &self.session_list
            };
            return list.hit_row(list_area, x, y, design).map(|r| 1000 + r);
        }
        if let Some(r) = hits.confirm_cancel
            && r.contains((x, y).into())
        {
            return Some(200);
        }
        if let Some(r) = hits.confirm_yes
            && r.contains((x, y).into())
        {
            return Some(201);
        }
        if let Some(list_area) = hits.main_list
            && let Some(row) = self.main_list.hit_row(list_area, x, y, design)
        {
            return Some(100 + row);
        }
        if let Some(strip_area) = hits.strip
            && let Some(idx) = self.strip.hit_index(strip_area, x, y)
        {
            return Some(300 + idx);
        }
        None
    }

    fn click(&mut self, hit_id: Option<usize>) -> Outcome {
        let Some(id) = hit_id else {
            return Outcome::Continue;
        };
        if id >= 1000 {
            let row = id - 1000;
            match self.mode {
                SettingsMode::ThemePicker => {
                    self.theme_list.set_cursor(row);
                    if let Some(slug) = self
                        .theme_list
                        .selected()
                        .and_then(|r| r.value.as_deref())
                        .map(str::to_string)
                    {
                        self.config.theme = Some(slug);
                        self.sync_theme_from_config();
                        let label = self.theme.name.clone();
                        self.persist(&format!("Default theme set to {label}"));
                        self.mode = SettingsMode::Main;
                    }
                    return Outcome::Consumed;
                }
                SettingsMode::SessionPicker => {
                    self.session_list.set_cursor(row);
                    if let Some(picked) = self
                        .session_list
                        .selected()
                        .and_then(|r| r.value.as_deref())
                    {
                        self.config.default_session = if picked.is_empty() {
                            None
                        } else {
                            Some(picked.to_string())
                        };
                        let label = self
                            .config
                            .default_session
                            .clone()
                            .unwrap_or_else(|| "Auto (first available)".into());
                        self.persist(&format!("Default session set to {label}"));
                        self.mode = SettingsMode::Main;
                    }
                    return Outcome::Consumed;
                }
                _ => return Outcome::Continue,
            }
        }
        if (100..200).contains(&id) {
            let row = id - 100;
            self.main_list.set_cursor(row);
            self.focus = Focus::List;
            self.strip.set_focused(false);
            return self.activate_selected_row();
        }
        if (300..400).contains(&id) {
            let idx = id - 300;
            if let Some(item) = self.strip.items().get(idx) {
                return self.trigger_strip_action(item.id);
            }
            return Outcome::Continue;
        }
        match id {
            200 => {
                self.mode = SettingsMode::Main;
                Outcome::Consumed
            }
            201 => {
                self.config = GreeterConfig::default();
                self.sync_theme_from_config();
                self.sync_session_cursor();
                self.persist("Restored greeter defaults");
                self.mode = SettingsMode::Main;
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        match self.mode {
            SettingsMode::ThemePicker => {
                let h = self.picker_height.get().max(1);
                match key.code {
                    KeyCode::Esc => {
                        self.sync_theme_from_config();
                        self.mode = SettingsMode::Main;
                        Outcome::Consumed
                    }
                    KeyCode::Backspace | KeyCode::Char(_)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if self.theme_list.type_filter(key) {
                            self.preview_highlighted_theme();
                        }
                        Outcome::Consumed
                    }
                    _ => match self.theme_list.handle(key, h, design) {
                        ListAction::Moved => {
                            self.preview_highlighted_theme();
                            Outcome::Consumed
                        }
                        ListAction::Activated(_) => {
                            if let Some(slug) = self
                                .theme_list
                                .selected()
                                .and_then(|r| r.value.as_deref())
                                .map(str::to_string)
                            {
                                self.config.theme = Some(slug);
                                self.sync_theme_from_config();
                                let label = self.theme.name.clone();
                                self.persist(&format!("Default theme set to {label}"));
                            }
                            self.mode = SettingsMode::Main;
                            Outcome::Consumed
                        }
                        ListAction::Ignored => Outcome::Continue,
                    },
                }
            }
            SettingsMode::SessionPicker => {
                let h = self.picker_height.get().max(1);
                match key.code {
                    KeyCode::Esc => {
                        self.mode = SettingsMode::Main;
                        Outcome::Consumed
                    }
                    KeyCode::Backspace | KeyCode::Char(_)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        self.session_list.type_filter(key);
                        Outcome::Consumed
                    }
                    _ => match self.session_list.handle(key, h, design) {
                        ListAction::Moved => Outcome::Consumed,
                        ListAction::Activated(_) => {
                            if let Some(picked) = self
                                .session_list
                                .selected()
                                .and_then(|r| r.value.as_deref())
                            {
                                self.config.default_session = if picked.is_empty() {
                                    None
                                } else {
                                    Some(picked.to_string())
                                };
                                let label = self
                                    .config
                                    .default_session
                                    .clone()
                                    .unwrap_or_else(|| "Auto (first available)".into());
                                self.persist(&format!("Default session set to {label}"));
                            }
                            self.mode = SettingsMode::Main;
                            Outcome::Consumed
                        }
                        ListAction::Ignored => Outcome::Continue,
                    },
                }
            }
            SettingsMode::EditGreeting => match key.code {
                KeyCode::Esc => {
                    self.mode = SettingsMode::Main;
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    let text = self.greeting_input.text().trim().to_string();
                    self.config.greeting = (!text.is_empty()).then_some(text);
                    self.persist("Updated greeting text");
                    self.mode = SettingsMode::Main;
                    Outcome::Consumed
                }
                _ => match self.greeting_input.handle(key) {
                    InputAction::Changed | InputAction::Submitted => {
                        self.reset_blink();
                        Outcome::Consumed
                    }
                    InputAction::Cancelled => {
                        self.mode = SettingsMode::Main;
                        Outcome::Consumed
                    }
                    InputAction::Ignored => Outcome::Continue,
                },
            },
            SettingsMode::ConfirmReset { confirm_yes } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    self.mode = SettingsMode::ConfirmReset {
                        confirm_yes: !confirm_yes,
                    };
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    if confirm_yes {
                        self.config = GreeterConfig::default();
                        self.sync_theme_from_config();
                        self.sync_session_cursor();
                        self.persist("Restored greeter defaults");
                    }
                    self.mode = SettingsMode::Main;
                    Outcome::Consumed
                }
                KeyCode::Esc => {
                    self.mode = SettingsMode::Main;
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
            SettingsMode::Main => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('p') {
                    return self.trigger_strip_action("preview");
                }
                if ctrl && key.code == KeyCode::Char('r') {
                    return self.trigger_strip_action("reset");
                }
                if key.code == KeyCode::Esc {
                    return Outcome::Done(1);
                }
                if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                    self.focus = match self.focus {
                        Focus::List => {
                            self.strip.set_focused(true);
                            Focus::Strip
                        }
                        Focus::Strip => {
                            self.strip.set_focused(false);
                            Focus::List
                        }
                    };
                    return Outcome::Consumed;
                }
                if self.focus == Focus::Strip {
                    match self.strip.handle(key) {
                        ActionStripEvent::Activated(id) => {
                            self.focus = Focus::List;
                            self.strip.set_focused(false);
                            return self.trigger_strip_action(id);
                        }
                        ActionStripEvent::Consumed => {
                            if !self.strip.is_focused() {
                                self.focus = Focus::List;
                            }
                            return Outcome::Consumed;
                        }
                        ActionStripEvent::Ignored => {}
                    }
                }
                if self.focus == Focus::List && key.code == KeyCode::Char(' ') {
                    return self.activate_selected_row();
                }
                let h = self.list_height.get().max(1);
                match self.main_list.handle(key, h, design) {
                    ListAction::Moved => Outcome::Consumed,
                    ListAction::Activated(_) => self.activate_selected_row(),
                    ListAction::Ignored => Outcome::Continue,
                }
            }
        }
    }
}

fn build_setting_rows(cfg: &GreeterConfig) -> Vec<Row> {
    let theme_slug = cfg.theme.as_deref().unwrap_or("tokyo-night");
    let theme_name = load_theme_by_slug(theme_slug)
        .map(|t| format!("{} ({theme_slug})", t.name))
        .unwrap_or_else(|| theme_slug.to_string());

    vec![
        Row::new("Follow user theme")
            .glyph(Glyph::Sliders)
            .detail(if cfg.follow_user_theme {
                "on \u{00b7} syncs with last user's active theme"
            } else {
                "off \u{00b7} always uses Default theme below"
            })
            .value("follow_user_theme"),
        Row::new("Default theme")
            .glyph(Glyph::Theme)
            .detail(theme_name)
            .value("theme"),
        Row::new("User pills")
            .glyph(Glyph::People)
            .detail(if cfg.show_user_pills {
                "shown \u{00b7} clickable account pills"
            } else {
                "hidden \u{00b7} username text box only"
            })
            .value("show_user_pills"),
        Row::new("Clock format")
            .glyph(Glyph::Clock)
            .detail(if cfg.clock_24h {
                "24-hour (19:30)"
            } else {
                "12-hour (07:30 PM)"
            })
            .value("clock_24h"),
        Row::new("Power buttons")
            .glyph(Glyph::Power)
            .detail(if cfg.show_power_actions {
                "shown \u{00b7} Sleep, Reboot, Shut down"
            } else {
                "hidden"
            })
            .value("show_power_actions"),
    ]
}

/// Concise one-line status for `hayato menu settings` (`Settings`).
pub fn status_summary() -> String {
    let cfg = load_greeter_config(&default_config_path());
    let theme = if cfg.follow_user_theme {
        match cfg.theme.as_deref() {
            Some(slug) => format!("follow user ({slug})"),
            None => "follow user".into(),
        }
    } else {
        format!("fixed ({})", cfg.theme.as_deref().unwrap_or("tokyo-night"))
    };
    let pills = if cfg.show_user_pills {
        "pills on"
    } else {
        "pills off"
    };
    let clock = if cfg.clock_24h { "24h" } else { "12h" };
    format!("{theme} \u{00b7} {pills} \u{00b7} {clock}")
}

/// Open Greeter Settings cleanly from a standalone menu (`hayato menu greeter`
/// or `hayato menu settings`). Supports launching `Preview` in-window and
/// returning cleanly to Settings.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    menu_impl(terminal, design, true)
}

/// Open Greeter Settings when entered from inside `GreeterScreen --preview` so
/// returning exits cleanly back to the refreshed Preview without recursion.
pub(crate) fn menu_embedded(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    menu_impl(terminal, design, false)
}

fn menu_impl(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    allow_preview_launch: bool,
) -> Result<i32, Failure> {
    loop {
        let cfg_path = default_config_path();
        let sessions = discover_sessions();
        let mut scr = GreeterSettingsScreen::new(
            cfg_path.clone(),
            sessions.clone(),
            design.density(),
            design.reduced_motion(),
            allow_preview_launch,
        );
        let code = crate::run_screen(terminal, design, "Greeter Settings", &mut scr)?;
        if allow_preview_launch && scr.take_open_preview() {
            let state_path = default_state_path();
            let state = GreeterState::load(&state_path);
            let users = discover_users();
            let cfg = load_greeter_config(&cfg_path);
            let home = resolve_user_home(&users, &state);
            let (theme_slug, theme, greeting) =
                resolve_theme_and_greeting(None, home.as_deref(), &state, &cfg_path);
            let mut preview_scr = GreeterScreen::new(GreeterSetup {
                users,
                sessions,
                state,
                state_path: None,
                auth_marker: None,
                theme_slug,
                theme,
                greeting,
                backend: Backend::Preview,
                preview: true,
                density: design.density(),
                reduced_motion: design.reduced_motion(),
                config: cfg,
            });
            let _ = crate::run_screen(terminal, design, "Greeter", &mut preview_scr)?;
            continue;
        }
        return Ok(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::greeter::SessionKind;
    use hayato_ui::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn test_sessions() -> Vec<SessionEntry> {
        vec![SessionEntry {
            name: "Sway".into(),
            exec: vec!["sway".into()],
            kind: SessionKind::Wayland,
            desktop_names: vec!["sway".into()],
            comment: None,
        }]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn toggling_settings_rows_persists_greeter_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("greeter.toml");
        let design = Design::solarized().with_reduced_motion(true);
        let mut scr = GreeterSettingsScreen::new(
            cfg_path.clone(),
            test_sessions(),
            Density::Normal,
            true,
            true,
        );

        // Row 0: Follow user theme -> toggle off.
        scr.key(key(KeyCode::Enter), &design);
        assert!(!scr.config().follow_user_theme);

        // Move to Row 2: User pills -> toggle off.
        scr.key(key(KeyCode::Down), &design);
        scr.key(key(KeyCode::Down), &design);
        scr.key(key(KeyCode::Char(' ')), &design);
        assert!(!scr.config().show_user_pills);

        // Move to Row 3: Clock format -> toggle 12h.
        scr.key(key(KeyCode::Down), &design);
        scr.key(key(KeyCode::Enter), &design);
        assert!(!scr.config().clock_24h);

        let disk = load_greeter_config(&cfg_path);
        assert!(!disk.follow_user_theme);
        assert!(!disk.show_user_pills);
        assert!(!disk.clock_24h);
    }

    #[test]
    fn cancel_first_reset_requires_confirmation() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("greeter.toml");
        let custom = GreeterConfig {
            show_user_pills: false,
            follow_user_theme: false,
            ..Default::default()
        };
        save_greeter_config(&cfg_path, &custom).unwrap();

        let design = Design::solarized().with_reduced_motion(true);
        let mut scr = GreeterSettingsScreen::new(
            cfg_path.clone(),
            test_sessions(),
            Density::Normal,
            true,
            true,
        );

        scr.key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &design,
        );
        assert_eq!(scr.mode, SettingsMode::ConfirmReset { confirm_yes: false });

        // Pressing Enter first safely cancels.
        scr.key(key(KeyCode::Enter), &design);
        assert_eq!(scr.mode, SettingsMode::Main);
        assert!(!scr.config().show_user_pills);

        // Confirm reset -> restores defaults.
        scr.key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            &design,
        );
        scr.key(key(KeyCode::Right), &design);
        scr.key(key(KeyCode::Enter), &design);
        assert!(scr.config().show_user_pills);
        assert!(scr.config().follow_user_theme);
    }

    #[test]
    fn settings_screen_renders_cleanly_at_90x30() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("greeter.toml");
        let design = Design::solarized().with_reduced_motion(true);
        let mut scr =
            GreeterSettingsScreen::new(cfg_path, test_sessions(), Density::Normal, true, true);
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        let mut app = App::new(Header::new("Hayato"))
            .design(design)
            .screen(&mut scr);
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        assert!(out.contains("Follow user theme"));
        assert!(out.contains("User pills"));
        assert!(out.contains("Reset defaults"));
        assert!(!out.contains("Default session"));
        assert!(!out.contains("Remember last session"));
        let strip_rect = scr.hits.borrow().strip.expect("top action strip rect");
        let list_rect = scr.hits.borrow().main_list.expect("settings list rect");
        assert!(
            strip_rect.y < list_rect.y,
            "action strip should sit at the top above the settings card"
        );
    }
}
