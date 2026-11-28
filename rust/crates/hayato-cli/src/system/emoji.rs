//! Emoji: the committed emojis.tsv table, fuzzy filter, insert through wtype (`hayato-emoji`).
//!
//! The table is generated on the build host by `scripts/make-emojis.sh` from Python's
//! Unicode database (~1300 rows: name, glyph, block), filtered against what the emoji
//! font draws. The font pin is checked at runtime.
//!
//! Insertion is Omarchy's trick: the glyph goes to the clipboard and a virtual
//! Shift+Insert is typed. Ctrl+Y copies only, for a surface that refuses a
//! virtual keyboard.

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;
use std::env;
use std::fs;

use hayato_ui::{
    BodyCtx, Card, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen,
    Urgency, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::missing;

const NEEDED: &[&str] = &["wl-copy", "wtype"];
const EMOJI_PATH: &str = "~/.config/emojis.tsv";

/// The emoji screen.
pub struct EmojiScreen {
    emojis: Vec<Emoji>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    font_ok: bool,
    status_note: Option<String>,
}

#[derive(Clone, Debug)]
struct Emoji {
    name: String,
    glyph: String,
    block: String,
}

impl EmojiScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            emojis: Vec::new(),
            list: ListState::new().with_empty("type to filter: a name, a block, or scroll"),
            header: Header::new("Hayato").subtitle("Emoji"),
            keybar: vec![
                KeyHint::new("Enter", "insert at cursor"),
                KeyHint::new("Ctrl+Y", "copy only"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            font_ok: true,
            status_note: None,
        };
        screen.load_emojis();
        screen
    }

    fn load_emojis(&mut self) {
        let path = env::home_dir()
            .map(|h| h.join(".config/emojis.tsv"))
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let content = fs::read_to_string(&path).unwrap_or_default();
        self.emojis = content
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() < 3 {
                    return None;
                }
                Some(Emoji {
                    name: parts[0].to_string(),
                    glyph: parts[1].to_string(),
                    block: parts[2].to_string(),
                })
            })
            .collect();
        // Check the font pin
        self.font_ok = Self::check_font_pin();
        if !self.font_ok {
            self.emojis.clear();
        }
        self.build_rows();
    }

    fn check_font_pin() -> bool {
        // Check if the emoji font is installed by checking fc-match
        if let Ok(output) = std::process::Command::new("fc-match")
            .args(["emoji"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let font_file = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Ok(output2) = std::process::Command::new("sha256sum")
                    .arg(&font_file)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output()
                {
                    if output2.status.success() {
                        let sha_output = String::from_utf8_lossy(&output2.stdout);
                        let sha = sha_output.split_whitespace().next().unwrap_or("");
                        // The pinned SHA from make-emojis.sh
                        const PINNED_SHA: &str =
                            "9536496476507f7621d05fb6f42dc53cd6e736267329c7b30e2772bab4281740";
                        return sha == PINNED_SHA;
                    }
                }
            }
        }
        false
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        if !self.font_ok {
            rows.push(
                Row::new("Emoji font not found or pin mismatch")
                    .glyph(Glyph::Warning)
                    .detail("Run scripts/make-emojis.sh on the build host"),
            );
        } else if self.emojis.is_empty() {
            rows.push(
                Row::new("No emojis loaded")
                    .glyph(Glyph::Warning)
                    .detail("The emoji table is missing or empty"),
            );
        } else {
            for e in &self.emojis {
                rows.push(
                    Row::new(format!("{}  {}", e.glyph, e.name))
                        .glyph(Glyph::Smile)
                        .detail(e.block.clone())
                        .value(e.glyph.clone()),
                );
            }
        }
        self.list.set_rows(rows);
    }
}

impl Screen for EmojiScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Pick one"), true);
        let (filter, rows) = super::pick::list_layout(ctx.area);
        let msg = if !self.font_ok {
            "The emoji font pin does not match · run make-emojis.sh"
        } else if let Some(note) = &self.status_note {
            note.as_str()
        } else {
            "Enter inserts at cursor · Ctrl+Y copies only · type to filter"
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.emojis.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let is_copy_only =
            key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL);
        let rows_height = self.rows_height.get();
        if is_copy_only {
            if let Some(index) = self.list.selected_index() {
                return self.insert_emoji(index, true);
            }
        }
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
                    return self.insert_emoji(index, false);
                }
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Continue;
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.insert_emoji(index, false),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            if let Some(index) = self.list.selected_index() {
                return self.insert_emoji(index, false);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, rows) = super::pick::list_layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl EmojiScreen {
    fn insert_emoji(&mut self, index: usize, copy_only: bool) -> Outcome {
        if index >= self.emojis.len() || !self.font_ok {
            return Outcome::Continue;
        }
        let glyph = self.emojis[index].glyph.clone();
        let name = self.emojis[index].name.clone();
        let copied = std::process::Command::new("wl-copy")
            .arg(&glyph)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !copied {
            self.status_note = Some("wl-copy is not available".to_string());
            return Outcome::Continue;
        }
        if copy_only {
            self.status_note = Some(format!("Copied {glyph} ({name}) to clipboard"));
            Outcome::Continue
        } else {
            crate::system::spawn_detached(
                "sleep 0.15; if command -v wtype >/dev/null 2>&1; then wtype -M shift -k Insert -m shift; fi",
            );
            Outcome::Done(hayato_ui::app::exit::OK)
        }
    }
}

/// `hayato menu emoji` (`hayato-emoji`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Emoji",
            format!("{said} (wl-clipboard, wtype)"),
        ));
    }
    let mut screen = EmojiScreen::new();
    let code = crate::run_screen(terminal, design, "Emoji", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_loading() {
        // Compile-time check
    }
}
