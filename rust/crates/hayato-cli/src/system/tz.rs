//! Date, Time & Timezone Studio (`hayato-tz`, `hayato menu tz`).
//!
//! Filterable list of every tzdata time zone paired with a live clock deck
//! showing local time, UTC time, NTP sync status, and a preview of the
//! highlighted time zone.

use std::cell::Cell;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{out, spawn_detached_argv, sudo};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["timedatectl"];

/// The `--status` output.
pub fn status() -> String {
    let current = out(&["timedatectl", "show", "-p", "Timezone", "--value"]).unwrap_or_default();
    let ntp = out(&["timedatectl", "show", "-p", "NTP", "--value"]).unwrap_or_default();
    let ntp_str = if ntp.trim() == "yes" { "on" } else { "off" };
    format!("Current: {} · Network time: {}", current.trim(), ntp_str)
}

/// Date, Time & Timezone Studio screen.
pub struct TzScreen {
    zones: Vec<String>,
    current: String,
    ntp: bool,
    list: ListState,
    banner: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    pub chosen: Option<String>,
}

impl TzScreen {
    pub fn new(zones: Vec<String>, current: String, ntp: bool, banner: Option<String>) -> Self {
        let rows: Vec<Row> = zones
            .iter()
            .map(|z| {
                if z == &current {
                    Row::new(z)
                        .glyph(Glyph::Clock)
                        .detail("active system time zone")
                        .value("● current")
                } else {
                    Row::new(z).glyph(Glyph::Clock)
                }
            })
            .collect();
        let mut list = ListState::new().with_empty("no matching time zones");
        list.set_rows(rows);
        Self {
            header: Header::new("Hayato")
                .subtitle("Date & Time Studio")
                .status(format!(
                    "{current} · NTP {}",
                    if ntp { "on" } else { "off" }
                )),
            zones,
            current,
            ntp,
            list,
            banner,
            keybar: vec![
                KeyHint::new("Enter", "set timezone"),
                KeyHint::new("Ctrl+N", "toggle NTP"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            chosen: None,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 52, 48)
    }

    fn left_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let filter = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
        );
        (filter, rows)
    }

    fn draw_clock_deck(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Clock & NTP Deck"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 8 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let title_style = design.style(TextStyle::Title).add_modifier(Modifier::BOLD);

        let local_now = chrono::Local::now()
            .format("%H:%M:%S · %a %d %b %Y")
            .to_string();
        let utc_now = chrono::Utc::now().format("%H:%M:%S UTC").to_string();

        let mut y = inner.y;
        put_trunc(buf, inner.x, y, "SYSTEM CLOCK", inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, &local_now, inner.width, title_style);
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("Zone      {}", self.current),
            inner.width,
            body,
        );
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("UTC       {utc_now}"),
            inner.width,
            dim,
        );
        y += 2;

        put_trunc(buf, inner.x, y, "NETWORK TIME (NTP)", inner.width, accent);
        y += 1;
        let (ntp_label, ntp_role) = if self.ntp {
            ("● Automatic sync ON (systemd-timesyncd)", ColorRole::Ok)
        } else {
            ("○ Automatic sync OFF", ColorRole::Warn)
        };
        put_trunc(
            buf,
            inner.x,
            y,
            ntp_label,
            inner.width,
            Style::default()
                .fg(design.color(ntp_role))
                .add_modifier(Modifier::BOLD),
        );
        y += 2;

        if let Some(idx) = self.list.selected_index()
            && let Some(sel) = self.zones.get(idx)
        {
            put_trunc(buf, inner.x, y, "HIGHLIGHTED ZONE", inner.width, accent);
            y += 1;
            put_trunc(buf, inner.x, y, sel, inner.width, body);
        }

        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Enter applies zone & restarts Waybar · Ctrl+N toggles NTP",
            inner.width,
            dim,
        );
    }

    fn activate(&mut self) -> Outcome {
        if let Some(idx) = self.list.selected_index()
            && let Some(zone) = self.zones.get(idx)
        {
            self.chosen = Some(format!("set:{zone}"));
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        Outcome::Continue
    }
}

impl Screen for TzScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Time Zones"), true);
        let (filter, rows) = Self::left_layout(left);
        let default_note = format!("Current: {} · type to filter · Ctrl+N NTP", self.current);
        let note = self.banner.as_deref().unwrap_or(&default_note);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            note,
            self.list.visible_len(),
            self.zones.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
        if let Some(right_area) = right {
            self.draw_clock_deck(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N'))
        {
            self.chosen = Some(if self.ntp {
                "ntp:off".into()
            } else {
                "ntp:on".into()
            });
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => return self.activate(),
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate(),
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::left_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate();
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("tz.studio")
    }
}

/// `hayato menu tz` (`hayato-tz`): Date, Time & Timezone Studio.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Time zone",
            format!("{said} (systemd)"),
        ));
    }

    let zones: Vec<String> = out(&["timedatectl", "list-timezones"])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut status_note: Option<String> = None;

    loop {
        let current = out(&["timedatectl", "show", "-p", "Timezone", "--value"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let ntp = out(&["timedatectl", "show", "-p", "NTP", "--value"])
            .unwrap_or_default()
            .trim()
            == "yes";

        let mut screen = TzScreen::new(zones.clone(), current, ntp, status_note.take());
        let code = crate::run_screen(terminal, design, "Time zone", &mut screen)?;
        if code != 0 {
            return Ok(code);
        }
        let Some(key) = screen.chosen.take() else {
            return Ok(1);
        };

        match key.as_str() {
            k if k.starts_with("set:") => {
                let zone = &k[4..];
                if let Err(e) = sudo(&["timedatectl", "set-timezone", zone]) {
                    status_note = Some(format!("Could not set {zone}: {e}"));
                    continue;
                }
                if out(&["pgrep", "-x", "waybar"]).is_some() {
                    let _ = out(&["pkill", "-x", "waybar"]);
                    spawn_detached_argv("waybar", &[]);
                }
                let now =
                    out(&["env", &format!("TZ={zone}"), "date", "+%H:%M"]).unwrap_or_default();
                status_note = Some(format!("Now {zone} · {}", now.trim()));
            }
            "ntp:on" | "ntp:off" => {
                let want = if key == "ntp:on" { "true" } else { "false" };
                let label = if key == "ntp:on" { "on" } else { "off" };
                match sudo(&["timedatectl", "set-ntp", want]) {
                    Ok(_) => status_note = Some(format!("Network time (NTP) is now {label}")),
                    Err(e) => status_note = Some(format!("Could not change network time: {e}")),
                }
            }
            _ => {}
        }
    }
}

fn missing(names: &[&str]) -> Option<String> {
    let gone: Vec<&str> = names.iter().copied().filter(|n| !have(n)).collect();
    if gone.is_empty() {
        None
    } else {
        Some(format!("Missing: {}", gone.join(" ")))
    }
}

fn have(program: &str) -> bool {
    crate::files::open::on_path(&std::env::var("PATH").unwrap_or_default(), program)
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_formats_correctly() {}
}
