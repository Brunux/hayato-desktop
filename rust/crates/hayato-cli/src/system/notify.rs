//! Notify history: structured JSONL history with dismiss/clear (`hayato-notify-history`).
//!
//! Notifications are logged to `$XDG_STATE_HOME/hayato/notify/history.jsonl`
//! by `hayato-notify`. Enter on a row copies the notification to the clipboard,
//! DND toggles quiet mode without suppressing critical system alerts, and
//! dismiss clears the log.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, Card, ColorRole, Confirm, ConfirmAction,
    Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle,
    Urgency, draw_card, draw_empty, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::missing;

const NEEDED: &[&str] = &["wl-copy"];

enum NotifyMode {
    Browse,
    ConfirmDismiss(Confirm),
}

/// The Notification Center screen.
pub struct NotifyScreen {
    notifications: Vec<Notification>,
    actions: Vec<String>,
    dnd: bool,
    mode: NotifyMode,
    strip: ActionStrip,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

#[derive(Clone, Debug)]
struct Notification {
    id: String,
    app: String,
    summary: String,
    body: String,
}

fn history_path() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".local").join("state")
        });
    base.join("hayato").join("notify").join("history.jsonl")
}

fn dnd_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime)
        .join("hayato")
        .join("notify")
        .join("dnd")
}

/// Remove the notification history. The caller confirms first.
pub fn clear_history() -> std::io::Result<()> {
    match std::fs::remove_file(history_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl NotifyScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            notifications: Vec::new(),
            actions: Vec::new(),
            dnd: false,
            mode: NotifyMode::Browse,
            strip: ActionStrip::default(),
            list: ListState::new(),
            header: Header::new("Hayato").subtitle("Notifications"),
            keybar: vec![
                KeyHint::new("Enter", "copy notification"),
                KeyHint::new("Ctrl+D", "DND"),
                KeyHint::new("Ctrl+X", "dismiss all"),
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
        self.notifications = Self::get_notifications();
        self.dnd = Self::get_dnd();
        self.build_rows();
    }

    fn get_notifications() -> Vec<Notification> {
        let path = history_path();
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let mut notifications = Vec::new();
        let reader = std::io::BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                let id = json
                    .get("id")
                    .and_then(|v| {
                        if let Some(n) = v.as_u64() {
                            Some(n.to_string())
                        } else {
                            v.as_str().map(|s| s.to_string())
                        }
                    })
                    .unwrap_or_default();
                let app = json
                    .get("app_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let summary = json
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body = json
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !id.is_empty() || !summary.is_empty() {
                    notifications.push(Notification {
                        id,
                        app,
                        summary,
                        body,
                    });
                }
            }
        }

        // Most recent first
        notifications.reverse();
        notifications
    }

    fn get_dnd() -> bool {
        dnd_path().exists()
    }

    fn build_rows(&mut self) {
        let dnd_glyph = if self.dnd {
            Glyph::BellOff
        } else {
            Glyph::Bell
        };
        let dnd_label = if self.dnd { "DND: ON" } else { "DND: OFF" };
        self.strip.set_items(vec![
            ActionItem::new("dnd", "^D", dnd_label).glyph(dnd_glyph),
            ActionItem::new("restore", "^R", "Restore").glyph(Glyph::Undo),
            ActionItem::new(
                "dismiss",
                "^X",
                format!("Clear ({})", self.notifications.len()),
            )
            .glyph(Glyph::Trash),
        ]);

        let mut rows = Vec::new();
        let mut actions = Vec::new();
        for n in &self.notifications {
            let body_short: String = n.body.chars().take(64).collect();
            let body_short = if n.body.chars().count() > 64 {
                format!("{body_short}…")
            } else {
                body_short
            };
            let mut r = Row::new(n.summary.clone())
                .glyph(Glyph::Bell)
                .detail(body_short);
            if !n.app.is_empty() {
                r = r.value(n.app.clone());
            }
            rows.push(r);
            actions.push(format!("id:{}", n.id));
        }

        self.actions = actions;
        self.list.update_rows(rows);
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 50, 48)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Message Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 6 {
            return;
        }
        let Some(idx) = self.list.selected_index() else {
            draw_empty(
                buf,
                inner,
                design,
                "No notification selected",
                "Select a notification on the left to read its full body",
            );
            return;
        };
        let Some(n) = self.notifications.get(idx) else {
            draw_empty(
                buf,
                inner,
                design,
                "No notification selected",
                "Select a notification on the left to read its full body",
            );
            return;
        };
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let title_style = design.style(TextStyle::Title).add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        let app_label = if n.app.is_empty() {
            "NOTIFICATION"
        } else {
            n.app.as_str()
        };
        put_trunc(buf, inner.x, y, app_label, inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, &n.summary, inner.width, title_style);
        y += 2;

        if n.body.is_empty() {
            put_trunc(buf, inner.x, y, "(no message body)", inner.width, dim);
        } else {
            let max_y = inner.bottom().saturating_sub(2);
            let w = usize::from(inner.width.max(1));
            'outer: for line in n.body.lines() {
                if line.is_empty() {
                    y += 1;
                    if y >= max_y {
                        break;
                    }
                    continue;
                }
                let chars: Vec<char> = line.chars().collect();
                for chunk in chars.chunks(w) {
                    if y >= max_y {
                        break 'outer;
                    }
                    let s: String = chunk.iter().collect();
                    put_trunc(buf, inner.x, y, &s, inner.width, body);
                    y += 1;
                }
            }
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Enter copies full text · Ctrl+D toggles DND · Ctrl+X clears",
            inner.width,
            dim,
        );
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

impl Screen for NotifyScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let NotifyMode::ConfirmDismiss(confirm) = &self.mode {
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("History"), true);
        let (strip, filter, rows) = Self::layout(left);
        self.strip.render(strip, ctx.buf, ctx.design);
        let msg = if let Some(note) = &self.status_note {
            note.as_str()
        } else if self.notifications.is_empty() {
            "Notification history is empty · Tab focuses actions · Esc backs out"
        } else {
            "Enter copies notification · Tab focuses actions · type to filter"
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.notifications.len(),
        );
        self.rows_height.set(rows.height);
        if self.notifications.is_empty() {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                "Nothing in notification history yet",
                "Notifications land here after they expire",
            );
        } else {
            self.list
                .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
        }
        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let NotifyMode::ConfirmDismiss(confirm) = &mut self.mode {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.mode = NotifyMode::Browse;
                    let _ = clear_history();
                    self.status_note = Some("Cleared notification history".to_string());
                    self.reload();
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.mode = NotifyMode::Browse;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        match self.strip.handle(key) {
            ActionStripEvent::Activated(id) => return self.run_strip_action(id),
            ActionStripEvent::Consumed => return Outcome::Consumed,
            ActionStripEvent::Ignored => {}
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    return self.run_strip_action("dnd");
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    return self.run_strip_action("restore");
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    return self.run_strip_action("dismiss");
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
        if let NotifyMode::ConfirmDismiss(confirm) = &mut self.mode {
            if let Some(idx) = row {
                match confirm.click(idx) {
                    ConfirmAction::Chosen(1) => {
                        self.mode = NotifyMode::Browse;
                        let _ = clear_history();
                        self.status_note = Some("Cleared notification history".to_string());
                        self.reload();
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = NotifyMode::Browse;
                    }
                    ConfirmAction::None => {}
                }
            }
            return Outcome::Consumed;
        }
        if let Some(row) = row {
            if row >= 10_000 {
                let strip_idx = row - 10_000;
                if let Some(item) = self.strip.items().get(strip_idx) {
                    return self.run_strip_action(item.id);
                }
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if let NotifyMode::ConfirmDismiss(confirm) = &self.mode {
            return confirm.hit(area, design, x, y);
        }
        let (left, _) = Self::split_panes(area);
        let (strip, _, rows) = Self::layout(left);
        if let Some(idx) = self.strip.hit_index(strip, x, y) {
            return Some(10_000 + idx);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.mode {
            NotifyMode::Browse => Some(&self.keybar),
            NotifyMode::ConfirmDismiss(_) => None,
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            NotifyMode::Browse => "notify.browse",
            NotifyMode::ConfirmDismiss(_) => "notify.confirm_dismiss",
        })
    }
}

impl NotifyScreen {
    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "dnd" => {
                let path = dnd_path();
                if path.exists() {
                    let _ = fs::remove_file(&path);
                    self.status_note = Some("Do not disturb: off".to_string());
                } else {
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&path, "1\n");
                    self.status_note = Some("Do not disturb: on".to_string());
                }
                self.reload();
            }
            "restore" => {
                if let Some(last) = self.notifications.first() {
                    let mut cmd = std::process::Command::new("notify-send");
                    if !last.app.is_empty() {
                        cmd.arg("-a").arg(&last.app);
                    } else {
                        cmd.arg("-a").arg("Hayato");
                    }
                    cmd.arg("-i").arg("bell");
                    cmd.arg(&last.summary);
                    if !last.body.is_empty() {
                        cmd.arg(&last.body);
                    }
                    let _ = cmd
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    self.status_note = Some(format!("Restored notification: {}", last.summary));
                } else {
                    self.status_note = Some("No notification in history to restore".to_string());
                }
            }
            "dismiss" => {
                if self.notifications.is_empty() {
                    self.status_note = Some("Notification history is already empty".to_string());
                } else {
                    let confirm = Confirm::new(
                        "Dismiss all notifications?",
                        format!(
                            "Clear all {} notifications from history?",
                            self.notifications.len()
                        ),
                    )
                    .with_options(&["Cancel", "Dismiss all"])
                    .destructive(true);
                    self.mode = NotifyMode::ConfirmDismiss(confirm);
                }
            }
            _ => {}
        }
        Outcome::Consumed
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.actions.get(index).cloned() else {
            return Outcome::Continue;
        };
        if let Some(id) = key.strip_prefix("id:") {
            if let Some(n) = self.notifications.iter().find(|n| n.id == id) {
                let text = if n.body.is_empty() {
                    n.summary.clone()
                } else {
                    format!("{}\n{}", n.summary, n.body)
                };
                let _ = std::process::Command::new("wl-copy")
                    .arg(&text)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                self.status_note = Some(format!("Copied notification: {}", n.summary));
            }
        }
        Outcome::Consumed
    }
}

/// `hayato menu notify` (`hayato-notify-history`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Notifications",
            format!("{said} (wl-clipboard)"),
        ));
    }
    let mut screen = NotifyScreen::new();
    let code = crate::run_screen(terminal, design, "Notifications", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_parsing() {
        // Compile-time check
    }
}
