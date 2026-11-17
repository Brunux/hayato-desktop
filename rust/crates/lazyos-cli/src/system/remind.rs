//! Reminders: write it now, be told later (`lazyos-remind`).
//!
//! The old script was a multi-step flow: title → note → when (delays, date, repeats)
//! → confirm. The reminders are transient systemd user timers (`systemd-run --user
//! --collect --on-active/--on-calendar`) that fire `notify-send`. The list reads
//! from systemd itself, so there is no second store to drift.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::new_without_default)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;

use lazyos_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, Card, ColorRole, Design, Glyph, Header,
    Input, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle, Urgency, draw_badge,
    draw_card, draw_empty, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::{missing, out};

const NEEDED: &[&str] = &[
    "systemd-run",
    "systemctl",
    "systemd-analyze",
    "date",
    "wl-copy",
    "jq",
];
const PREFIX: &str = "lazyos-reminder-";
const TAB: &str = "\t";

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemindMode {
    List,
    NewTitle,
    NewWhen(String),
}

/// The remind screen.
pub struct RemindScreen {
    reminders: Vec<Reminder>,
    actions: Vec<String>,
    strip: ActionStrip,
    list: ListState,
    when_list: ListState,
    when_keys: Vec<(&'static str, &'static str)>,
    title_input: Input,
    mode: RemindMode,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

#[derive(Clone, Debug)]
struct Reminder {
    unit: String,
    when: u64,
    title: String,
    note: String,
    calendar: Option<String>,
}

impl RemindScreen {
    pub fn new() -> Self {
        let presets: Vec<(&'static str, &'static str, &'static str)> = vec![
            ("In 5 minutes", "one-shot timer (+5m)", "active:5m"),
            ("In 15 minutes", "one-shot timer (+15m)", "active:15m"),
            ("In 30 minutes", "one-shot timer (+30m)", "active:30m"),
            ("In 1 hour", "one-shot timer (+1h)", "active:1h"),
            ("In 3 hours", "one-shot timer (+3h)", "active:3h"),
            (
                "Every day at 09:00",
                "repeating daily timer",
                "cal:*-*-* 09:00:00",
            ),
            (
                "Every weekday at 09:00",
                "repeating Mon..Fri timer",
                "cal:Mon..Fri *-*-* 09:00:00",
            ),
        ];
        let mut when_list = ListState::new();
        when_list.set_rows(
            presets
                .iter()
                .map(|(l, d, _)| Row::new(*l).glyph(Glyph::Clock).detail(*d))
                .collect(),
        );
        let when_keys = presets.into_iter().map(|(l, _, k)| (l, k)).collect();

        let strip = ActionStrip::new(vec![
            ActionItem::new("new", "^N", "New reminder").glyph(Glyph::Bell),
        ]);
        let mut screen = Self {
            reminders: Vec::new(),
            actions: Vec::new(),
            strip,
            list: ListState::new().with_empty("no reminders waiting"),
            when_list,
            when_keys,
            title_input: Input::new(),
            mode: RemindMode::List,
            header: Header::new("LazyOS").subtitle("Reminders"),
            keybar: vec![
                KeyHint::new("Enter", "new / cancel"),
                KeyHint::new("Ctrl+N", "new reminder"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            status_note: None,
        };
        screen.reload();
        screen
    }

    fn reload(&mut self) {
        self.reminders = Self::get_reminders();
        self.build_rows();
    }

    fn get_reminders() -> Vec<Reminder> {
        let output = out(&[
            "systemctl",
            "--user",
            "list-timers",
            "--all",
            "--output=json",
            &format!("{}*", PREFIX),
        ])
        .unwrap_or_default();
        let mut reminders = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&output) {
            if let Some(arr) = json.as_array() {
                for item in arr {
                    let unit = item
                        .get("unit")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let next = item.get("next").and_then(|v| v.as_u64()).unwrap_or(0) / 1_000_000;
                    if next == 0 || next <= now {
                        continue;
                    }
                    let props = out(&[
                        "systemctl",
                        "--user",
                        "show",
                        "-p",
                        "Description",
                        "-p",
                        "TimersCalendar",
                        &unit,
                    ])
                    .unwrap_or_default();
                    let mut desc = String::new();
                    let mut cal = String::new();
                    for line in props.lines() {
                        if line.starts_with("Description=") {
                            desc = line["Description=".len()..].to_string();
                        } else if line.starts_with("TimersCalendar=") {
                            cal = line["TimersCalendar={ OnCalendar=".len()..]
                                .trim_end_matches('}')
                                .to_string();
                        }
                    }
                    let title = desc
                        .split(TAB)
                        .next()
                        .unwrap_or("(untitled reminder)")
                        .to_string();
                    let note = desc.split(TAB).nth(1).unwrap_or("").to_string();
                    reminders.push(Reminder {
                        unit,
                        when: next,
                        title,
                        note,
                        calendar: if cal.is_empty() { None } else { Some(cal) },
                    });
                }
            }
        }
        reminders
    }

    fn left(secs: u64) -> String {
        if secs < 60 {
            "in less than a minute".to_string()
        } else if secs < 3600 {
            format!("in {} min", secs / 60)
        } else if secs < 86400 {
            let h = secs / 3600;
            let m = (secs % 3600) / 60;
            if m > 0 {
                format!("in {} h {} min", h, m)
            } else {
                format!("in {} h", h)
            }
        } else {
            format!("in {} days", secs / 86400)
        }
    }

    fn weekday(d: &str) -> &str {
        match d {
            "Mon" => "Monday",
            "Tue" => "Tuesday",
            "Wed" => "Wednesday",
            "Thu" => "Thursday",
            "Fri" => "Friday",
            "Sat" => "Saturday",
            "Sun" => "Sunday",
            _ => d,
        }
    }

    fn repeat_words(cal: &str) -> Option<String> {
        let time = cal.split_whitespace().last()?;
        let hm = time.strip_suffix(':').unwrap_or(time);
        let day_part = cal.split_whitespace().next()?;
        let words = match day_part {
            "*-*-*" => Some(format!("every day at {}", hm)),
            "Mon..Fri" | "Mon-Fri" => Some(format!("every weekday at {}", hm)),
            "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun" => {
                Some(format!("every {} at {}", Self::weekday(day_part), hm))
            }
            d if d.starts_with("*-*-") => {
                let day = d["*-*-".len()..].trim_start_matches('0');
                let day = if day.is_empty() { "0" } else { day };
                Some(format!("every month on day {} at {}", day, hm))
            }
            d if d.contains('-') && d.chars().next().unwrap().is_ascii_digit() => {
                // One-shot date
                let _ = chrono::NaiveDateTime::parse_from_str(
                    &format!("{} {}", d, hm),
                    "%Y-%m-%d %H:%M",
                );
                None
            }
            _ => Some("repeating".to_string()),
        };
        words
    }

    fn trim(s: &str, max: usize) -> String {
        if s.len() <= max {
            s.to_string()
        } else {
            format!("{}…", &s[..max])
        }
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for r in &self.reminders {
            let words = r
                .calendar
                .as_ref()
                .and_then(|c| Self::repeat_words(c))
                .unwrap_or_else(|| Self::left(r.when.saturating_sub(now)));
            let time_str = chrono::DateTime::from_timestamp(r.when as i64, 0)
                .map(|d| d.format("%a %d %b %H:%M").to_string())
                .unwrap_or_default();
            let mut detail = format!("{words} · Enter cancels");
            if !r.note.is_empty() {
                detail = format!("{} · {}", Self::trim(&r.note, 40), detail);
            }
            rows.push(
                Row::new(Self::trim(&r.title, 72))
                    .glyph(Glyph::Clock)
                    .detail(detail)
                    .value(time_str),
            );
            actions.push(format!("cancel:{}", r.unit));
        }
        self.actions = actions;
        self.list.update_rows(rows);
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let strip = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1.min(inner.height),
        };
        let filter = Rect {
            x: inner.x,
            y: inner.y.saturating_add(2).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(2)),
        };
        let rows_y = inner.y.saturating_add(4).min(inner.bottom());
        let rows = Rect {
            x: inner.x,
            y: rows_y,
            width: inner.width,
            height: inner.bottom().saturating_sub(rows_y),
        };
        (strip, filter, rows)
    }
}

impl Screen for RemindScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Reminders"), true);

        match &self.mode {
            RemindMode::List => {
                let (strip, filter, rows) = Self::layout(ctx.area);
                self.strip.render(strip, ctx.buf, ctx.design);
                let default_msg = if self.reminders.is_empty() {
                    "Press Enter or Ctrl+N to schedule a reminder · Esc backs out".to_string()
                } else {
                    format!(
                        "{} waiting · Enter cancels · Ctrl+N schedules new",
                        self.reminders.len()
                    )
                };
                let msg = self.status_note.as_deref().unwrap_or(&default_msg);
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    self.list.filter(),
                    msg,
                    self.list.visible_len(),
                    self.reminders.len(),
                );
                self.rows_height.set(rows.height);
                if self.reminders.is_empty() {
                    draw_empty(
                        ctx.buf,
                        rows,
                        ctx.design,
                        "No reminders waiting",
                        "Press Enter or Ctrl+N to schedule a new reminder",
                    );
                } else {
                    self.list
                        .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
                }
            }
            RemindMode::NewTitle => {
                let (filter, rows) = super::pick::list_layout(ctx.area);
                let mut x = draw_badge(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    ctx.design,
                    "WHAT",
                    ColorRole::Accent,
                );
                x += 2;
                ctx.buf.set_string(
                    x,
                    filter.y,
                    "Type reminder message and press Enter (Esc returns to list)",
                    ctx.design.style(TextStyle::Title),
                );
                let input_rect = Rect {
                    x: rows.x,
                    y: rows.y + 1,
                    width: rows.width,
                    height: 1,
                };
                self.title_input
                    .render(input_rect, ctx.buf, ctx.design, "Remind me to…");
            }
            RemindMode::NewWhen(title) => {
                let (filter, rows) = super::pick::list_layout(ctx.area);
                let mut x = draw_badge(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    ctx.design,
                    "WHEN",
                    ColorRole::Accent,
                );
                x += 2;
                ctx.buf.set_string(
                    x,
                    filter.y,
                    format!("“{title}” · pick when it fires (Esc edits message)"),
                    ctx.design.style(TextStyle::Title),
                );
                self.rows_height.set(rows.height);
                self.when_list.render(rows, ctx.buf, ctx.design, true);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();

        match &self.mode {
            RemindMode::NewTitle => match key.code {
                KeyCode::Esc => {
                    self.mode = RemindMode::List;
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    let t = self.title_input.text().trim().to_string();
                    if !t.is_empty() {
                        self.mode = RemindMode::NewWhen(t);
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    let _ = self.title_input.handle(key);
                    return Outcome::Consumed;
                }
            },
            RemindMode::NewWhen(title) => {
                let title = title.clone();
                match key.code {
                    KeyCode::Esc => {
                        self.mode = RemindMode::NewTitle;
                        return Outcome::Consumed;
                    }
                    KeyCode::Enter => {
                        if let Some(idx) = self.when_list.selected_index() {
                            self.schedule_reminder(&title, idx);
                        }
                        return Outcome::Consumed;
                    }
                    _ => {
                        if let ListAction::Activated(idx) =
                            self.when_list.handle(key, rows_height, design)
                        {
                            self.schedule_reminder(&title, idx);
                        }
                        return Outcome::Consumed;
                    }
                }
            }
            RemindMode::List => {}
        }

        match self.strip.handle(key) {
            ActionStripEvent::Activated("new") => {
                self.title_input.set("");
                self.mode = RemindMode::NewTitle;
                return Outcome::Consumed;
            }
            ActionStripEvent::Activated(_) | ActionStripEvent::Consumed => {
                return Outcome::Consumed;
            }
            ActionStripEvent::Ignored => {}
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N'))
        {
            self.title_input.set("");
            self.mode = RemindMode::NewTitle;
            return Outcome::Consumed;
        }

        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => {
                if self.reminders.is_empty() {
                    self.title_input.set("");
                    self.mode = RemindMode::NewTitle;
                    return Outcome::Consumed;
                }
                if let Some(index) = self.list.selected_index() {
                    return self.activate(index);
                }
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            match &self.mode {
                RemindMode::List => {
                    if row == 10_000 {
                        self.title_input.set("");
                        self.mode = RemindMode::NewTitle;
                        return Outcome::Consumed;
                    }
                    self.strip.set_focused(false);
                    self.list.set_cursor(row);
                    if let Some(index) = self.list.selected_index() {
                        return self.activate(index);
                    }
                }
                RemindMode::NewWhen(title) => {
                    let title = title.clone();
                    self.when_list.set_cursor(row);
                    if let Some(idx) = self.when_list.selected_index() {
                        self.schedule_reminder(&title, idx);
                    }
                }
                RemindMode::NewTitle => {}
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        match self.mode {
            RemindMode::List => {
                let (strip, _, rows) = Self::layout(area);
                if y == strip.y && x >= strip.x && x < strip.right() {
                    return Some(10_000);
                }
                self.list.hit_row(rows, x, y, design)
            }
            RemindMode::NewWhen(_) => {
                let (_, rows) = super::pick::list_layout(area);
                self.when_list.hit_row(rows, x, y, design)
            }
            RemindMode::NewTitle => None,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl RemindScreen {
    fn schedule_reminder(&mut self, title: &str, when_idx: usize) {
        let Some((when_label, spec)) = self.when_keys.get(when_idx).copied() else {
            self.mode = RemindMode::List;
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let unit = format!("{PREFIX}{ts}");
        let timer_flag = if let Some(active) = spec.strip_prefix("active:") {
            format!("--on-active={active}")
        } else if let Some(cal) = spec.strip_prefix("cal:") {
            format!("--on-calendar={cal}")
        } else {
            "--on-active=15m".to_string()
        };

        let status = std::process::Command::new("systemd-run")
            .args([
                "--user",
                "--collect",
                &format!("--unit={unit}"),
                &format!("--description={title}"),
                &timer_flag,
                "notify-send",
                "-a",
                "LazyOS",
                "-i",
                "clock",
                "Reminder",
                title,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        self.mode = RemindMode::List;
        self.title_input.set("");
        if status.map(|s| s.success()).unwrap_or(false) {
            self.status_note = Some(format!("Scheduled “{title}” ({when_label})"));
        } else {
            self.status_note = Some(format!("Could not schedule “{title}”"));
        }
        self.reload();
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.actions.get(index).cloned() else {
            return Outcome::Continue;
        };
        if key == "new" {
            self.title_input.set("");
            self.mode = RemindMode::NewTitle;
            Outcome::Continue
        } else if let Some(unit) = key.strip_prefix("cancel:") {
            if let Some(r) = self.reminders.iter().find(|r| r.unit == unit) {
                let name = if r.title.is_empty() {
                    "The reminder"
                } else {
                    &r.title
                };
                let said = format!("“{name}”");
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", unit])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                if output.map(|s| s.success()).unwrap_or(false) {
                    self.status_note = Some(format!("Cancelled {said} — it will not fire"));
                } else {
                    self.status_note = Some(format!("{said} was already gone"));
                }
                self.reload();
            }
            Outcome::Continue
        } else {
            Outcome::Continue
        }
    }
}

/// `lazyos menu remind` (`lazyos-remind`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Reminders",
            format!("{said} (systemd, date, jq, wl-clipboard)"),
        ));
    }
    let mut screen = RemindScreen::new();
    let code = crate::run_screen(terminal, design, "Reminders", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remind_needed_tools() {
        assert!(NEEDED.contains(&"systemd-run"));
        assert!(NEEDED.contains(&"systemd-analyze"));
    }

    #[test]
    fn left_formats_seconds() {
        assert_eq!(RemindScreen::left(30), "in less than a minute");
        assert_eq!(RemindScreen::left(90), "in 1 min");
        assert_eq!(RemindScreen::left(3600), "in 1 h");
        assert_eq!(RemindScreen::left(7200), "in 2 h");
        assert_eq!(RemindScreen::left(86400), "in 1 days");
    }
}
