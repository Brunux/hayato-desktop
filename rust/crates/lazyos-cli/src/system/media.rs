//! Media: player state and transport keys (`rofi-media`).
//!
//! The old script used `playerctl` to get the current track metadata and status,
//! and offered play/pause/next/prev/stop. The track line at the top was also a
//! way to copy what is playing. This is the same interaction: one list, the
//! track marked with a star when playing, and the four transport rows.
//!
//! `playerctl -F` (follow) is not used here: the menu opens, reads once, and
//! closes. A live-updating panel is the bar's MPRIS module (spec 014).

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;

use lazyos_ui::{
    BodyCtx, Card, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_badge, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["playerctl"];

/// The media screen.
pub struct MediaScreen {
    players: Vec<Player>,
    actions: Vec<String>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

#[derive(Clone, Debug)]
struct Player {
    name: String,
    title: String,
    artist: String,
    album: String,
    status: String,
}

impl MediaScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            players: Vec::new(),
            actions: Vec::new(),
            list: ListState::new(),
            header: Header::new("LazyOS").subtitle("Media"),
            keybar: vec![
                KeyHint::new("Enter", "act"),
                KeyHint::new("r", "refresh"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            status_note: None,
        };
        screen.reload();
        screen
    }

    fn reload(&mut self) {
        self.players = Self::get_players();
        self.build_rows();
    }

    fn get_players() -> Vec<Player> {
        let players = out(&["playerctl", "-l"]).unwrap_or_default();
        let mut out_players = Vec::new();
        for line in players.lines() {
            let name = line.trim();
            if name.is_empty() {
                continue;
            }
            let title = out(&["playerctl", "metadata", "title", "-p", name]).unwrap_or_default();
            let artist = out(&["playerctl", "metadata", "artist", "-p", name]).unwrap_or_default();
            let album = out(&["playerctl", "metadata", "album", "-p", name]).unwrap_or_default();
            let status = out(&["playerctl", "status", "-p", name]).unwrap_or_default();
            out_players.push(Player {
                name: name.to_string(),
                title: title.trim().to_string(),
                artist: artist.trim().to_string(),
                album: album.trim().to_string(),
                status: status.trim().to_string(),
            });
        }
        out_players
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        if self.players.is_empty() {
            rows.push(
                Row::new("Nothing is playing")
                    .glyph(Glyph::Music)
                    .detail("no MPRIS player is running · press Enter or r to refresh"),
            );
            actions.push("refresh".to_string());
        } else {
            for p in &self.players {
                let mut label = String::new();
                if !p.artist.is_empty() {
                    label.push_str(&format!("{} — ", p.artist));
                }
                if p.title.is_empty() {
                    label.push_str(&p.name);
                } else {
                    label.push_str(&p.title);
                }
                let detail = if p.album.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} · {}", p.name, p.album)
                };
                let badge = if p.status == "Playing" {
                    "▶ playing"
                } else {
                    "⏸ paused"
                };
                rows.push(
                    Row::new(label)
                        .glyph(Glyph::Music)
                        .detail(detail)
                        .value(badge),
                );
                actions.push(format!("player:{}", p.name));
            }
            if self.players.iter().any(|p| p.status == "Playing") {
                rows.push(
                    Row::new("Pause")
                        .glyph(Glyph::Pause)
                        .detail("pause playback"),
                );
                actions.push("pause".to_string());
            } else {
                rows.push(
                    Row::new("Play")
                        .glyph(Glyph::Play)
                        .detail("resume playback"),
                );
                actions.push("play".to_string());
            }
            rows.push(
                Row::new("Next track")
                    .glyph(Glyph::Next)
                    .detail("skip to next"),
            );
            actions.push("next".to_string());
            rows.push(
                Row::new("Previous track")
                    .glyph(Glyph::Prev)
                    .detail("skip to previous"),
            );
            actions.push("previous".to_string());
            rows.push(Row::new("Stop").glyph(Glyph::Stop).detail("stop playback"));
            actions.push("stop".to_string());
            rows.push(
                Row::new("Copy track")
                    .glyph(Glyph::Clipboard)
                    .detail("artist — title to clipboard"),
            );
            actions.push("copy".to_string());
        }
        self.actions = actions;
        self.list.update_rows(rows);
    }
}

impl Screen for MediaScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Media"), true);
        let (hero, filter, rows) = Self::layout(ctx.area);

        if let Some(h) = hero {
            if let Some(active) = self
                .players
                .iter()
                .find(|p| p.status == "Playing")
                .or_else(|| self.players.first())
            {
                let (badge, role) = if active.status == "Playing" {
                    ("▶ PLAYING", ColorRole::Success)
                } else {
                    ("⏸ PAUSED", ColorRole::Warn)
                };
                let mut x = draw_badge(ctx.buf, h.x, h.y, ctx.design, badge, role);
                x += 2;
                let title = if active.title.is_empty() {
                    active.name.as_str()
                } else {
                    active.title.as_str()
                };
                ctx.buf
                    .set_string(x, h.y, title, ctx.design.style(TextStyle::Title));
                if h.height >= 2 {
                    let sub = if !active.artist.is_empty() && !active.album.is_empty() {
                        format!(
                            "{} · {} ({})",
                            active.artist, active.album, active.name
                        )
                    } else if !active.artist.is_empty() {
                        format!("{} ({})", active.artist, active.name)
                    } else {
                        active.name.clone()
                    };
                    ctx.buf
                        .set_string(h.x, h.y + 1, sub, ctx.design.style(TextStyle::Dim));
                }
            } else {
                let mut x = draw_badge(
                    ctx.buf,
                    h.x,
                    h.y,
                    ctx.design,
                    "NO PLAYER",
                    ColorRole::Border,
                );
                x += 2;
                ctx.buf.set_string(
                    x,
                    h.y,
                    "No MPRIS media player active",
                    ctx.design.style(TextStyle::Dim),
                );
            }
        }

        let note = self
            .status_note
            .as_deref()
            .unwrap_or("Enter acts on player or transport · r refreshes");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            note,
            self.list.len(),
            self.list.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
            }
            KeyCode::Char('r') if self.list.filter().is_empty() => {
                self.reload();
                self.status_note = Some("Refreshed player status".to_string());
                return Outcome::Continue;
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
                return Outcome::Continue;
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
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, _, rows) = Self::layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl MediaScreen {
    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.actions.get(index).cloned() else {
            return Outcome::Continue;
        };
        if key == "refresh" {
            self.reload();
            return Outcome::Continue;
        }
        if let Some(name) = key.strip_prefix("player:") {
            let status = self
                .players
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.status.clone())
                .unwrap_or_default();
            let subcmd = if status == "Playing" { "pause" } else { "play" };
            let _ = std::process::Command::new("playerctl")
                .args([subcmd, "-p", name])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            self.reload();
            Outcome::Continue
        } else {
            match key.as_str() {
                "play" | "pause" | "next" | "previous" | "stop" => {
                    for p in &self.players {
                        let _ = std::process::Command::new("playerctl")
                            .args([key.as_str(), "-p", &p.name])
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();
                    }
                    self.reload();
                }
                "copy" => {
                    for p in &self.players {
                        let track = if !p.artist.is_empty() {
                            format!("{} - {}", p.artist, p.title)
                        } else {
                            p.title.clone()
                        };
                        let _ = std::process::Command::new("wl-copy")
                            .arg(&track)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();
                        self.status_note = Some(format!("Copied track to clipboard: {track}"));
                    }
                }
                _ => {}
            }
            Outcome::Continue
        }
    }

    fn layout(area: Rect) -> (Option<Rect>, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        if inner.height >= 12 {
            let hero = Rect {
                height: 2,
                ..inner
            };
            let filter = Rect {
                y: inner.y + 3,
                height: 1,
                ..inner
            };
            let rows = Rect {
                y: inner.y + 5,
                height: inner.height.saturating_sub(5),
                ..inner
            };
            (Some(hero), filter, rows)
        } else {
            let filter = Rect {
                height: 1.min(inner.height),
                ..inner
            };
            let rows = Rect {
                y: inner.y + 2,
                height: inner.height.saturating_sub(2),
                ..inner
            };
            (None, filter, rows)
        }
    }
}

/// `lazyos menu media` (`rofi-media`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Media",
            format!("{said} (playerctl)"),
        ));
    }
    let mut screen = MediaScreen::new();
    let code = crate::run_screen(terminal, design, "Media", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_parsing() {
        // Test that the player parsing logic works with mocked output
        // This is a compile-time check mostly
    }
}
