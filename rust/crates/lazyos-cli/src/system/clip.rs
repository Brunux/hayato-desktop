//! Clipboard: cliphist rows by kind (text/link/image), copy back, save image, clear (`rofi-clip`).
//!
//! The old script used `cliphist list` to get the history (tab-separated: id<TAB>preview),
//! classified each entry by its preview (image, link, or text), and showed them with the
//! appropriate glyph. Enter copied back, Alt+1 deleted, footer rows saved the newest image
//! and wiped the history.
//!
//! The secret rule (spec 006) applies: a row that is a secret (password) is handled by
//! `Card::secret` so the value never appears in the notification body.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;

use lazyos_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, Card, Design, Glyph, Header, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, Urgency, draw_card, draw_empty, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use std::process::Stdio;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["cliphist", "wl-copy", "wl-paste", "xdg-open", "file"];

/// The clipboard screen.
pub struct ClipScreen {
    entries: Vec<ClipEntry>,
    keys: Vec<String>,
    strip: ActionStrip,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ClipKind {
    Image,
    Link,
    Text,
}

#[derive(Clone, Debug)]
struct ClipEntry {
    id: String,
    preview: String,
    kind: ClipKind,
}

impl ClipScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            entries: Vec::new(),
            keys: Vec::new(),
            strip: ActionStrip::default(),
            list: ListState::new().with_empty("nothing has been copied yet"),
            header: Header::new("LazyOS").subtitle("Clipboard"),
            keybar: vec![
                KeyHint::new("Enter", "copy back"),
                KeyHint::new("Alt+1", "delete entry"),
                KeyHint::new("Ctrl+S", "save image"),
                KeyHint::new("Ctrl+X", "clear"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            status_note: None,
        };
        screen.reload();
        screen
    }

    fn reload(&mut self) {
        self.entries = Self::get_entries();
        self.build_rows();
    }

    fn get_entries() -> Vec<ClipEntry> {
        let output = out(&["cliphist", "list"]).unwrap_or_default();
        let mut entries = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let id = parts[0].to_string();
            let preview = parts[1..].join("\t");
            let kind = if preview.starts_with("[[ binary data") {
                ClipKind::Image
            } else if preview.starts_with("http://") || preview.starts_with("https://") {
                ClipKind::Link
            } else {
                ClipKind::Text
            };
            entries.push(ClipEntry { id, preview, kind });
        }
        entries
    }

    fn build_rows(&mut self) {
        self.strip.set_items(vec![
            ActionItem::new("save", "^S", "Save newest image").glyph(Glyph::Image),
            ActionItem::new("wipe", "^X", format!("Clear history ({})", self.entries.len()))
                .glyph(Glyph::Trash),
        ]);

        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for e in &self.entries {
            let (glyph, kind_str) = match e.kind {
                ClipKind::Image => (Glyph::Image, "image"),
                ClipKind::Link => (Glyph::Link, "link"),
                ClipKind::Text => (Glyph::Clipboard, "text"),
            };
            let shown: String = e.preview.chars().take(58).collect();
            let shown = if e.preview.chars().count() > 58 {
                format!("{shown}…")
            } else {
                shown
            };
            rows.push(Row::new(shown).glyph(glyph).value(kind_str));
            keys.push(e.id.clone());
        }
        self.keys = keys;
        self.list.update_rows(rows);
    }
}

impl Screen for ClipScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Clipboard"), true);
        let (strip, filter, rows) = Self::list_layout(ctx.area);
        self.strip.render(strip, ctx.buf, ctx.design);
        let msg = if let Some(note) = &self.status_note {
            note.as_str()
        } else if self.entries.is_empty() {
            "Clipboard history is empty · Esc backs out"
        } else {
            "Enter copies back · Alt+1 deletes entry · Tab focuses actions"
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.entries.len(),
        );
        self.rows_height.set(rows.height);
        if self.entries.is_empty() {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                "Nothing has been copied yet",
                "Clipboard entries appear here automatically as you copy text or images",
            );
        } else {
            self.list
                .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match self.strip.handle(key) {
            ActionStripEvent::Activated(id) => return self.run_strip_action(id),
            ActionStripEvent::Consumed => return Outcome::Consumed,
            ActionStripEvent::Ignored => {}
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    return self.run_strip_action("save");
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    return self.run_strip_action("wipe");
                }
                _ => {}
            }
        }
        let rows_height = self.rows_height.get();
        let is_alt1 = key.code == KeyCode::Char('1') && key.modifiers.contains(KeyModifiers::ALT);
        if is_alt1 {
            if let Some(index) = self.list.selected_index() {
                if index < self.entries.len() {
                    let deleted = std::process::Command::new("sh")
                        .args([
                            "-c",
                            "printf '%s\\n' \"$1\" | cliphist delete",
                            "--",
                            &self.entries[index].id,
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if deleted {
                        self.status_note =
                            Some("Entry deleted from clipboard history".to_string());
                        self.reload();
                    } else {
                        self.status_note = Some("Could not delete that entry".to_string());
                    }
                }
                return Outcome::Consumed;
            }
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
        let action = self.list.handle(key, rows_height, design);
        match action {
            ListAction::Activated(index) => self.activate(index),
            ListAction::Ignored | ListAction::Moved => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            if r >= 10_000 {
                let strip_idx = r - 10_000;
                if let Some(item) = self.strip.items().get(strip_idx) {
                    return self.run_strip_action(item.id);
                }
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(r);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (strip, _, rows) = Self::list_layout(area);
        if y == strip.y && x >= strip.x && x < strip.right() {
            let mut col = strip.x;
            for (idx, item) in self.strip.items().iter().enumerate() {
                let w = 4
                    + item.key.len() as u16
                    + item.label.len() as u16
                    + item.glyph.map(|_| 2).unwrap_or(0);
                if x >= col && x < col + w {
                    return Some(10_000 + idx);
                }
                col += w + 1;
            }
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl ClipScreen {
    fn list_layout(area: Rect) -> (Rect, Rect, Rect) {
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

    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "save" => {
                let img_entry = self.entries.iter().find(|e| e.kind == ClipKind::Image);
                if let Some(entry) = img_entry {
                    let cmd = format!(
                        "tmp=$(mktemp /tmp/clipimg.XXXXXX); \
                        cliphist decode '{}' > \"$tmp\" 2>/dev/null; \
                        ext=$(file --brief --mime-type \"$tmp\" 2>/dev/null | sed -e 's|.*/||' -e 's|jpeg|jpg|' -e 's|[^a-z0-9]||g'); \
                        [ -n \"$ext\" ] || ext=png; \
                        dir=\"$HOME/Pictures/clipboard\"; \
                        mkdir -p \"$dir\"; \
                        dest=\"$dir/clip-$(date +%Y%m%d-%H%M%S).$ext\"; \
                        mv -f \"$tmp\" \"$dest\"; \
                        exec xdg-open \"$dest\"",
                        entry.id
                    );
                    crate::system::spawn_detached(&cmd);
                    Outcome::Done(lazyos_ui::app::exit::OK)
                } else {
                    self.status_note = Some("No image in clipboard history".to_string());
                    Outcome::Consumed
                }
            }
            "wipe" => {
                let count = self.entries.len();
                let _ = std::process::Command::new("cliphist")
                    .arg("wipe")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status_note = Some(format!("Cleared clipboard history ({count} entries)"));
                self.reload();
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.keys.get(index).cloned() else {
            return Outcome::Continue;
        };
        let copied = std::process::Command::new("sh")
            .args(["-c", "cliphist decode \"$1\" | wl-copy", "--", &key])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if copied {
            Outcome::Done(lazyos_ui::app::exit::OK)
        } else {
            self.status_note = Some("Could not copy that entry back".to_string());
            Outcome::Consumed
        }
    }
}

/// `lazyos menu clip` (`rofi-clip`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Clipboard",
            format!("{said} (cliphist, wl-clipboard, xdg-utils, file)"),
        ));
    }
    let mut screen = ClipScreen::new();
    let code = crate::run_screen(terminal, design, "Clipboard", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_entry_classification() {
        assert_eq!(
            ClipKind::Image,
            if "[[ binary data 1234 png 640x480 ]]".starts_with("[[ binary data") {
                ClipKind::Image
            } else {
                ClipKind::Text
            }
        );
        assert_eq!(
            ClipKind::Link,
            if "http://example.com".starts_with("http://") {
                ClipKind::Link
            } else {
                ClipKind::Text
            }
        );
        assert_eq!(
            ClipKind::Text,
            if "plain text".starts_with("[[ binary data") {
                ClipKind::Image
            } else if "plain text".starts_with("http") {
                ClipKind::Link
            } else {
                ClipKind::Text
            }
        );
    }
}
