//! Notes and documents (`hayato menu notes`, `hayato-notes`).
//!
//! One list for the three note shapes this image has, and a thin wrapper
//! around the tool that owns each:
//!
//! * **quick notes** are Markdown files in `~/Documents/Notes`, edited in
//!   editxr in a foot window, with the folder as its vault; after the editor
//!   exits the same window reopens this list;
//! * **ink notes** are `.xopp` files opened in xournalpp, which also annotates
//!   and signs PDFs; exported PDFs land in `~/Documents/Signed`;
//! * **books** (`.epub`) are not notes: Files and `hayato open` send them to
//!   foliate, because reading is not note-taking.
//!
//! The folder is the state. There is no index file and no daemon: the list is
//! a directory read, the README is the only file this menu writes in the folder,
//! and a note the person deletes elsewhere simply stops appearing. Outside it,
//! the one write is editxr's two theme keys, so the editor follows the session
//! palette.

use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use hayato_theme::store as theme_store;
use hayato_theme::{Theme, builtin};
use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_badge, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

use super::pick::{Picker, Question, row};

const README: &str = "\
# Notes

Quick notes are Markdown files in this folder. **Alt+Space -> Notes** lists
them; Enter opens one in editxr, and quitting the editor returns to the list.

Ink notes are `.xopp` files, edited in xournalpp. The same menu annotates and
signs a PDF and exports an ink note to `../Signed/`.

This file is the only one the menu writes in this folder.
";

/// What a row does once it is chosen. The pickers that need the terminal are
/// run by `menu()` after the screen closes, then the list reopens.
enum Pending {
    Annotate,
    Export,
    Search,
    Trash(PathBuf),
    Share(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    List,
    NewTitle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteKind {
    Text,
    Ink,
}

struct Note {
    path: PathBuf,
    title: String,
    kind: NoteKind,
    modified: SystemTime,
    size: u64,
}

/// How one preview line is drawn. The page is Markdown-lite: enough structure
/// to read a note, not a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewStyle {
    Title,
    Body,
    Dim,
    Code,
    Quote,
    Bullet,
    Danger,
}

struct PreviewLine {
    text: String,
    style: PreviewStyle,
}

/// What the page beside the shelf shows: the selected note, or the help for the
/// selected action.
struct Preview {
    title: String,
    badge: &'static str,
    meta: String,
    excerpt: String,
    lines: Vec<PreviewLine>,
    empty: Option<String>,
}

impl Preview {
    fn empty(hint: &str) -> Self {
        Self {
            title: "No note selected".to_string(),
            badge: "—",
            meta: String::new(),
            excerpt: String::new(),
            lines: Vec::new(),
            empty: Some(hint.to_string()),
        }
    }
}

/// The notes screen.
pub struct NotesScreen {
    notes: Vec<Note>,
    list: ListState,
    keys: Vec<String>,
    mode: Mode,
    title_input: Input,
    header: Header,
    keybar: Vec<KeyHint>,
    keybar_title: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status: String,
    summary: String,
    preview: Preview,
    wrapped: Vec<(String, PreviewStyle)>,
    wrapped_width: u16,
    pending: Option<Pending>,
}

impl NotesScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            notes: Vec::new(),
            list: ListState::new().with_empty("no notes yet · Enter on New quick note"),
            keys: Vec::new(),
            mode: Mode::List,
            title_input: Input::new(),
            header: Header::new("Hayato").subtitle("Notes"),
            keybar: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("Ctrl+P", "read"),
                KeyHint::new("Del", "trash"),
                KeyHint::new("Esc", "close"),
            ],
            keybar_title: vec![KeyHint::new("Enter", "create"), KeyHint::new("Esc", "back")],
            rows_height: Cell::new(20),
            status: String::new(),
            summary: String::new(),
            preview: Preview::empty("Select a note to read it here"),
            wrapped: Vec::new(),
            wrapped_width: 0,
            pending: None,
        };
        screen.reload();
        screen
    }

    fn reload(&mut self) {
        self.notes = scan(&notes_dir());
        self.build_rows();
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for (index, note) in self.notes.iter().enumerate() {
            let glyph = match note.kind {
                NoteKind::Text => Glyph::File,
                NoteKind::Ink => Glyph::Brush,
            };
            rows.push(row(note.title.as_str(), detail(note), glyph).value(
                if note.kind == NoteKind::Ink {
                    "xopp"
                } else {
                    "md"
                },
            ));
            keys.push(format!("note:{index}"));
        }
        rows.push(row("New quick note", "name it, write it", Glyph::File));
        keys.push("new".to_string());
        rows.push(row(
            "Today's note",
            "open or create today's",
            Glyph::Calendar,
        ));
        keys.push("daily".to_string());
        rows.push(row(
            "New ink note",
            if have("xournalpp") {
                "save it with Ctrl+S"
            } else {
                "install xournalpp to write and draw"
            },
            Glyph::Brush,
        ));
        keys.push("ink".to_string());
        rows.push(row(
            "Sign or annotate a PDF",
            if have("xournalpp") {
                "xournalpp · export to Signed"
            } else {
                "install xournalpp (8.8 MB)"
            },
            Glyph::File,
        ));
        keys.push("annotate".to_string());
        rows.push(row(
            "Export ink to PDF",
            if have("xournalpp") {
                "into Documents/Signed"
            } else {
                "install xournalpp (8.8 MB)"
            },
            Glyph::Download,
        ));
        keys.push("export".to_string());
        rows.push(row("Search the notes", "rg over the folder", Glyph::Search));
        keys.push("search".to_string());
        rows.push(row("Open folder", home_short(&notes_dir()), Glyph::Folder));
        keys.push("folder".to_string());
        self.list.set_rows(rows);
        self.keys = keys;
        self.summary = summary_line(&self.notes);
        self.refresh_preview();
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let Some(key) = self.keys.get(index).cloned() else {
            return Outcome::Continue;
        };
        if let Some(note_index) = key.strip_prefix("note:") {
            let Some(note) = note_index
                .parse::<usize>()
                .ok()
                .and_then(|index| self.notes.get(index))
            else {
                return Outcome::Continue;
            };
            let path = note.path.clone();
            let kind = note.kind;
            return match kind {
                NoteKind::Text => match open_editor(&path) {
                    Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                    Err(reason) => {
                        self.status = reason;
                        Outcome::Continue
                    }
                },
                NoteKind::Ink => match open_ink(&path) {
                    Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                    Err(reason) => {
                        self.status = reason;
                        Outcome::Continue
                    }
                },
            };
        }
        match key.as_str() {
            "new" => {
                self.mode = Mode::NewTitle;
                self.title_input = Input::new();
                Outcome::Consumed
            }
            "daily" => {
                let path = daily_path(&notes_dir(), SystemTime::now());
                if !path.exists() {
                    let _ = fs::write(&path, daily_header(SystemTime::now()));
                }
                match open_editor(&path) {
                    Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                    Err(reason) => {
                        self.status = reason;
                        Outcome::Continue
                    }
                }
            }
            "ink" => match new_ink(&notes_dir()) {
                Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                Err(reason) => {
                    self.status = reason;
                    Outcome::Continue
                }
            },
            "annotate" => {
                self.pending = Some(Pending::Annotate);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            "export" => {
                self.pending = Some(Pending::Export);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            "search" => {
                self.pending = Some(Pending::Search);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            "folder" => {
                let dir = notes_dir();
                spawn_self(&[
                    "menu".into(),
                    "files".into(),
                    "--".into(),
                    dir.display().to_string(),
                ]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            _ => Outcome::Continue,
        }
    }

    fn selected_note(&self) -> Option<&Note> {
        self.list
            .selected_index()
            .and_then(|index| self.keys.get(index))
            .and_then(|key| key.strip_prefix("note:"))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.notes.get(index))
    }

    /// Read the selected note (or the selected action's help) into the page.
    /// Called when the selection changes, not per frame.
    fn refresh_preview(&mut self) {
        let selected = self
            .list
            .selected_index()
            .and_then(|index| self.keys.get(index));
        self.preview = match selected {
            Some(key) if key.starts_with("note:") => key[5..]
                .parse::<usize>()
                .ok()
                .and_then(|index| self.notes.get(index))
                .map(note_preview)
                .unwrap_or_else(|| Preview::empty("Select a note to read it here")),
            Some(key) => action_preview(key),
            None if !self.list.filter().is_empty() => {
                Preview::empty("Nothing matches · Backspace widens the filter")
            }
            None => Preview::empty("Select a note to read it here"),
        };
        self.wrapped_width = 0;
    }

    /// The page beside the shelf: title, kind badge, metadata, a margin rule
    /// and the wrapped body. It is not a nested card: the outer card is the
    /// desk, the rule is the notebook margin.
    fn draw_page(&mut self, area: Rect, ctx: &mut BodyCtx) {
        let design = ctx.design;
        let meta_style = design.style(TextStyle::Dim);
        if area.width < 12 || area.height < 3 {
            return;
        }
        if let Some(hint) = self.preview.empty.as_deref() {
            let mid = area.y + area.height / 2;
            centred(
                ctx.buf,
                area,
                mid.saturating_sub(1),
                "No note selected",
                design.style(TextStyle::Title),
            );
            centred(ctx.buf, area, mid + 1, hint, meta_style);
            return;
        }
        if area.height < 6 {
            return;
        }
        put_trunc(
            ctx.buf,
            area.x + 2,
            area.y,
            &self.preview.title,
            area.width
                .saturating_sub(self.preview.badge.len() as u16 + 6),
            design.style(TextStyle::Title),
        );
        let badge_x = area
            .right()
            .saturating_sub(self.preview.badge.len() as u16 + 3);
        draw_badge(
            ctx.buf,
            badge_x,
            area.y,
            design,
            self.preview.badge,
            ColorRole::Accent,
        );
        put_trunc(
            ctx.buf,
            area.x + 2,
            area.y + 1,
            &self.preview.meta,
            area.width.saturating_sub(4),
            meta_style,
        );
        for x in area.x + 2..area.right().saturating_sub(2) {
            ctx.buf.set_string(x, area.y + 2, "─", meta_style);
        }
        for y in area.y + 3..area.bottom().saturating_sub(1) {
            ctx.buf.set_string(area.x + 1, y, "│", meta_style);
        }
        let width = area.width.saturating_sub(5);
        if width < 8 {
            return;
        }
        if self.wrapped_width != width {
            self.wrapped = wrap(&self.preview.lines, width);
            self.wrapped_width = width;
        }
        let body_y = area.y + 3;
        let rows = area.height.saturating_sub(4) as usize;
        for (index, (text, style)) in self.wrapped.iter().take(rows).enumerate() {
            put_trunc(
                ctx.buf,
                area.x + 3,
                body_y + index as u16,
                text,
                width,
                preview_style(design, *style),
            );
        }
        if self.wrapped.len() > rows {
            put_trunc(
                ctx.buf,
                area.x + 3,
                body_y + rows.saturating_sub(1) as u16,
                "… more · Enter opens",
                width,
                meta_style,
            );
        }
    }

    /// The narrow layout's two-line page strip: the first body line, then the
    /// metadata. It keeps the preview's promise on an 80×24 terminal.
    fn draw_strip(&self, area: Rect, ctx: &mut BodyCtx) {
        let design = ctx.design;
        if self.preview.empty.is_some() {
            put_trunc(
                ctx.buf,
                area.x + 2,
                area.y,
                "Select a note to preview",
                area.width.saturating_sub(4),
                design.style(TextStyle::Body),
            );
            put_trunc(
                ctx.buf,
                area.x + 2,
                area.y + 1,
                "New quick note starts the first one",
                area.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
            return;
        }
        put_trunc(
            ctx.buf,
            area.x + 2,
            area.y,
            &self.preview.excerpt,
            area.width.saturating_sub(4),
            design.style(TextStyle::Body),
        );
        let meta = if self.status.is_empty() {
            &self.preview.meta
        } else {
            &self.status
        };
        put_trunc(
            ctx.buf,
            area.x + 2,
            area.y + 1,
            meta,
            area.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
    }
}

impl Screen for NotesScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("All notes"), true);
        let panes = panes(inner);
        self.rows_height.set(panes.list.height);
        match self.mode {
            Mode::List => {
                draw_filter_bar(
                    ctx.buf,
                    panes.filter,
                    ctx.design,
                    self.list.filter(),
                    "type to filter · Enter opens",
                    self.list.visible_len(),
                    self.list.len(),
                );
                self.list.render(panes.list, ctx.buf, ctx.design, true);
                if let Some(page) = panes.page {
                    self.draw_page(page, ctx);
                }
                if let Some(strip) = panes.strip {
                    self.draw_strip(strip, ctx);
                }
                if let Some(summary) = panes.summary {
                    let text = if self.status.is_empty() {
                        &self.summary
                    } else {
                        &self.status
                    };
                    put_trunc(
                        ctx.buf,
                        summary.x,
                        summary.y,
                        text,
                        summary.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                }
            }
            Mode::NewTitle => {
                let area = Rect {
                    x: inner.x,
                    y: inner.y,
                    width: panes.filter.width,
                    height: panes.list.height + 2,
                };
                put_trunc(
                    ctx.buf,
                    area.x,
                    area.y,
                    "Title for the new note (Enter creates it, Escape goes back)",
                    area.width,
                    ctx.design.style(TextStyle::Title),
                );
                let input_rect = Rect {
                    x: area.x,
                    y: area.y + 2,
                    width: area.width,
                    height: 1,
                };
                self.title_input
                    .render(input_rect, ctx.buf, ctx.design, "What is it about?");
                if let Some(page) = panes.page {
                    self.draw_page(page, ctx);
                }
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if self.mode == Mode::NewTitle {
            return match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::List;
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    let title = self.title_input.text().trim().to_string();
                    if title.is_empty() {
                        return Outcome::Consumed;
                    }
                    let path = new_note_path(&notes_dir(), &title, SystemTime::now());
                    if let Err(error) = write_note(&path, &title) {
                        self.status = format!("Could not create the note: {error}");
                        self.mode = Mode::List;
                        return Outcome::Consumed;
                    }
                    match open_editor(&path) {
                        Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                        Err(reason) => {
                            self.status = reason;
                            self.mode = Mode::List;
                            Outcome::Consumed
                        }
                    }
                }
                _ => {
                    let _ = self.title_input.handle(key);
                    Outcome::Consumed
                }
            };
        }
        if key.code == KeyCode::Esc && !self.list.filter().is_empty() {
            self.list.set_filter("");
            self.refresh_preview();
            return Outcome::Consumed;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(note) = self.selected_note() {
                let path = note.path.clone();
                if note.kind == NoteKind::Text {
                    return match preview(&path) {
                        Ok(()) => Outcome::Done(hayato_ui::app::exit::OK),
                        Err(reason) => {
                            self.status = reason;
                            Outcome::Continue
                        }
                    };
                }
            }
            return Outcome::Consumed;
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(note) = self.selected_note() {
                self.pending = Some(Pending::Share(note.path.clone()));
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
            return Outcome::Consumed;
        }
        if key.code == KeyCode::Delete {
            if let Some(note) = self.selected_note() {
                let path = note.path.clone();
                self.pending = Some(Pending::Trash(path));
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
            return Outcome::Consumed;
        }
        let rows_height = self.rows_height.get();
        let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if typed {
            if self.list.type_filter(key) {
                self.refresh_preview();
            }
            return Outcome::Consumed;
        }
        let before = self.list.selected_index();
        let action = self.list.handle(key, rows_height, design);
        if self.list.selected_index() != before {
            self.refresh_preview();
        }
        match action {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if self.mode != Mode::List {
            return Outcome::Continue;
        }
        if let Some(visible) = row {
            self.list.set_cursor(visible);
            self.refresh_preview();
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if self.mode != Mode::List {
            return None;
        }
        let inner = design.block(hayato_ui::Border::Rounded).inner(area);
        self.list.hit_row(panes(inner).list, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(match self.mode {
            Mode::List => &self.keybar,
            Mode::NewTitle => &self.keybar_title,
        })
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            Mode::List => "list",
            Mode::NewTitle => "new-title",
        })
    }
}

/// The two layouts: a shelf beside a page when there is room, and the shelf
/// with a two-line page strip on a small terminal. The thresholds are the
/// session's own windows (152×45 and 80×24), not arbitrary breakpoints.
struct Panes {
    filter: Rect,
    list: Rect,
    page: Option<Rect>,
    strip: Option<Rect>,
    summary: Option<Rect>,
}

fn panes(inner: Rect) -> Panes {
    if inner.width >= super::DUAL_PANE_MIN_WIDTH.saturating_sub(2) {
        let shelf = (inner.width * 44 / 100).clamp(38, 64);
        Panes {
            filter: Rect {
                x: inner.x,
                y: inner.y,
                width: shelf,
                height: 1,
            },
            list: Rect {
                x: inner.x,
                y: inner.y + 2,
                width: shelf,
                height: inner.height.saturating_sub(3),
            },
            page: Some(Rect {
                x: inner.x + shelf + 1,
                y: inner.y,
                width: inner.width.saturating_sub(shelf + 1),
                height: inner.height,
            }),
            strip: None,
            summary: Some(Rect {
                x: inner.x,
                y: inner.bottom().saturating_sub(1),
                width: shelf,
                height: 1,
            }),
        }
    } else {
        let strip_height = if inner.width >= 60 && inner.height >= 14 {
            2
        } else {
            0
        };
        Panes {
            filter: Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
            list: Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2 + strip_height),
            },
            page: None,
            strip: (strip_height > 0).then(|| Rect {
                x: inner.x,
                y: inner.bottom().saturating_sub(strip_height),
                width: inner.width,
                height: strip_height,
            }),
            summary: None,
        }
    }
}

fn centred(buf: &mut ratatui::buffer::Buffer, area: Rect, y: u16, text: &str, style: Style) {
    let width = UnicodeWidthStr::width(text) as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    put_trunc(buf, x, y, text, area.width, style);
}

fn summary_line(notes: &[Note]) -> String {
    let text = notes
        .iter()
        .filter(|note| note.kind == NoteKind::Text)
        .count();
    let ink = notes.len().saturating_sub(text);
    match notes.first() {
        Some(newest) => format!(
            "{text} notes · {ink} ink · edited {}",
            stamp_of(newest.modified)
        ),
        None => format!("{text} notes · {ink} ink · the folder is empty"),
    }
}

fn date_of(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| {
            chrono::DateTime::<chrono::Local>::from(UNIX_EPOCH + since)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string())
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// The page's Markdown-lite renderer: headings, quotes, bullets, fenced code,
/// and the inline markers removed. It is a preview, not a parser: a line that
/// is not one of those is body text, and the editor shows the rest.
fn markdown_lines(text: &str) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    let mut in_code = false;
    let mut blank = false;
    for raw in text.lines().take(600) {
        let raw = raw.trim_end();
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            lines.push(PreviewLine {
                text: raw.to_string(),
                style: PreviewStyle::Code,
            });
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            if !blank && !lines.is_empty() {
                lines.push(PreviewLine {
                    text: String::new(),
                    style: PreviewStyle::Dim,
                });
                blank = true;
            }
            continue;
        }
        blank = false;
        if trimmed.starts_with('#') {
            lines.push(PreviewLine {
                text: trimmed.trim_start_matches('#').trim().to_string(),
                style: PreviewStyle::Title,
            });
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            lines.push(PreviewLine {
                text: format!("│ {}", rest.trim()),
                style: PreviewStyle::Quote,
            });
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            lines.push(PreviewLine {
                text: format!("• {rest}"),
                style: PreviewStyle::Bullet,
            });
        } else {
            lines.push(PreviewLine {
                text: inline_markdown(trimmed),
                style: PreviewStyle::Body,
            });
        }
    }
    lines
}

fn inline_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(ch, '*' | '_' | '`') {
            while chars.peek() == Some(&ch) {
                chars.next();
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Wrap the preview at the page width. This runs when the selection or the
/// window changes, never per frame; the draw loop only copies cached strings.
fn wrap(lines: &[PreviewLine], width: u16) -> Vec<(String, PreviewStyle)> {
    let mut out = Vec::new();
    if width == 0 {
        return out;
    }
    for line in lines {
        if line.text.is_empty() {
            out.push((String::new(), line.style));
            continue;
        }
        let mut current = String::new();
        let mut used = 0u16;
        for word in line.text.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word) as u16;
            let needed = if current.is_empty() {
                word_width
            } else {
                word_width + 1
            };
            if !current.is_empty() && used + needed > width {
                out.push((std::mem::take(&mut current), line.style));
                used = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                used += 1;
            }
            current.push_str(word);
            used += word_width;
        }
        if !current.is_empty() {
            out.push((current, line.style));
        }
    }
    out
}

fn preview_style(design: &Design, style: PreviewStyle) -> Style {
    match style {
        PreviewStyle::Title => design.style(TextStyle::Title),
        PreviewStyle::Body | PreviewStyle::Bullet => design.style(TextStyle::Body),
        PreviewStyle::Dim | PreviewStyle::Quote => design.style(TextStyle::Dim),
        PreviewStyle::Code => design.style(TextStyle::Code),
        PreviewStyle::Danger => design.style(TextStyle::Danger),
    }
}

/// The page's header already names the note, so the first line that repeats the
/// title is dropped: a note's own `# Heading` (or its first line, which is what
/// names a note without a heading) would otherwise appear twice.
fn strip_title(lines: &mut Vec<PreviewLine>, title: &str) {
    let Some(index) = lines.iter().position(|line| !line.text.is_empty()) else {
        return;
    };
    if truncate(&lines[index].text, 72) != title {
        return;
    }
    lines.remove(index);
    while lines.first().is_some_and(|line| line.text.is_empty()) {
        lines.remove(0);
    }
}

fn note_preview(note: &Note) -> Preview {
    let edited = date_of(note.modified);
    match note.kind {
        NoteKind::Text => {
            let text = read_head(&note.path);
            let words = word_count(&text);
            let mut lines = markdown_lines(&text);
            if note.size > 0 && text.is_empty() {
                lines.push(PreviewLine {
                    text: "Cannot read this note.".to_string(),
                    style: PreviewStyle::Danger,
                });
            } else {
                strip_title(&mut lines, &note.title);
                if lines.is_empty() {
                    lines.push(PreviewLine {
                        text: "Empty note · Enter opens the editor".to_string(),
                        style: PreviewStyle::Dim,
                    });
                }
            }
            let excerpt = lines
                .iter()
                .find(|line| !line.text.is_empty() && line.style != PreviewStyle::Title)
                .or_else(|| lines.iter().find(|line| !line.text.is_empty()))
                .map(|line| line.text.clone())
                .unwrap_or_default();
            Preview {
                title: note.title.clone(),
                badge: "MD",
                meta: format!("{words} words · edited {edited}"),
                excerpt,
                lines,
                empty: None,
            }
        }
        NoteKind::Ink => {
            let body = "An ink note. Enter opens it in xournalpp; export the PDF from the list.";
            Preview {
                title: note.title.clone(),
                badge: "INK",
                meta: format!("{} · edited {edited}", human_size(note.size)),
                excerpt: body.to_string(),
                lines: vec![PreviewLine {
                    text: body.to_string(),
                    style: PreviewStyle::Body,
                }],
                empty: None,
            }
        }
    }
}

fn action_preview(key: &str) -> Preview {
    let (title, body) = match key {
        "new" => (
            "New quick note",
            "Name it, then write it in editxr. The file is <date>-<title>.md in this folder; quitting the editor returns here.",
        ),
        "daily" => (
            "Today's note",
            "Open or create the note for today, with the date as its heading. One file per day.",
        ),
        "ink" => (
            "New ink note",
            "A blank xournalpp document. Save it in this folder with Ctrl+S and it joins the shelf.",
        ),
        "annotate" => (
            "Sign or annotate a PDF",
            "Pick a PDF from Documents or Downloads. xournalpp opens it; save the .xopp and export the signed copy to Documents/Signed.",
        ),
        "export" => (
            "Export ink to PDF",
            "xournalpp --create-pdf writes the PDF into Documents/Signed.",
        ),
        "search" => (
            "Search the notes",
            "ripgrep over the folder. Enter opens the hit in editxr; Ctrl+F finds inside it.",
        ),
        "folder" => (
            "Open folder",
            "The notes folder in Files, where the rest of the file actions live.",
        ),
        _ => ("Notes", "Enter opens the selected row."),
    };
    Preview {
        title: title.to_string(),
        badge: "ACTION",
        meta: "Enter runs it".to_string(),
        excerpt: body.to_string(),
        lines: vec![PreviewLine {
            text: body.to_string(),
            style: PreviewStyle::Body,
        }],
        empty: None,
    }
}

/// The paths the menu works in. `Documents` comes from xdg-user-dirs, so a
/// localised session keeps its notes where the rest of its documents are.
pub fn notes_dir() -> PathBuf {
    let documents = dirs::document_dir().unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
            .join("Documents")
    });
    documents.join("Notes")
}

pub fn signed_dir() -> PathBuf {
    notes_dir()
        .parent()
        .map(|parent| parent.join("Signed"))
        .unwrap_or_else(|| PathBuf::from("/tmp/Signed"))
}

fn ensure_notes() -> std::io::Result<()> {
    let dir = notes_dir();
    fs::create_dir_all(&dir)?;
    let readme = dir.join("README.md");
    if !readme.exists() {
        fs::write(&readme, README)?;
    }
    fs::create_dir_all(signed_dir())?;
    Ok(())
}

fn have(program: &str) -> bool {
    crate::files::open::on_path(&std::env::var("PATH").unwrap_or_default(), program)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn read_head(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let mut buffer = vec![0u8; 8192];
    let Ok(read) = file.read(&mut buffer) else {
        return String::new();
    };
    String::from_utf8_lossy(&buffer[..read]).to_string()
}

/// A note's title is its first Markdown heading, else its first non-empty line,
/// else its file name. One rule for the list and the tests.
pub fn title_from(text: &str, name: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let title = line.trim_start_matches('#').trim();
        if !title.is_empty() {
            return truncate(title, 72);
        }
    }
    display_stem(name)
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// The file name without its extension, its leading `YYYY-MM-DD-` (the menu
/// writes one on every note it creates) and its separators. The list already
/// shows the date in the detail column, so repeating it in the title only pushes
/// the words off the shelf.
fn display_stem(name: &str) -> String {
    let stem = stem(Path::new(name));
    let stem = stem.trim();
    let stripped = match stem.char_indices().nth(10) {
        Some((index, '-')) if is_date_prefix(&stem[..index]) => &stem[index + 1..],
        _ => stem,
    };
    let stem = if stripped.is_empty() { stem } else { stripped };
    let spaced = stem.replace(['-', '_'], " ");
    let spaced = spaced.trim();
    if spaced.is_empty() {
        truncate(stem, 72)
    } else {
        truncate(spaced, 72)
    }
}

/// `2026-11-18` in the ten bytes before the separator.
fn is_date_prefix(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn ink_title(name: &str) -> String {
    display_stem(name)
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{} MB", bytes / 1_048_576),
    }
}

fn stamp_of(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| {
            chrono::DateTime::<chrono::Local>::from(UNIX_EPOCH + since)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn detail(note: &Note) -> String {
    format!("{} · {}", stamp_of(note.modified), human_size(note.size))
}

fn home_short(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.and_then(|home| {
        path.strip_prefix(home)
            .ok()
            .map(|rest| format!("~/{}", rest.display()))
    })
    .unwrap_or_else(|| path.display().to_string())
}

/// Every note in `dir`, newest first. `README.md` is the menu's own page, not
/// a note, and hidden files are the person's business.
fn scan(dir: &Path) -> Vec<Note> {
    let mut notes = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return notes;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(extension) = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_lowercase())
        else {
            continue;
        };
        if !matches!(extension.as_str(), "md" | "markdown" | "txt" | "xopp") {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') || name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        let kind = if extension == "xopp" {
            NoteKind::Ink
        } else {
            NoteKind::Text
        };
        let metadata = entry.metadata().ok();
        let modified = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        let size = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
        let title = match kind {
            NoteKind::Ink => ink_title(&name),
            NoteKind::Text => {
                let text = read_head(&path);
                title_from(&text, &name)
            }
        };
        notes.push(Note {
            path,
            title,
            kind,
            modified,
            size,
        });
    }
    notes.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    notes
}

/// `title` -> `title` as a file name: lower case, words joined by `-`, at most
/// 48 characters. Empty input is `note`, so a note always has a name.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
        if slug.chars().count() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

/// `YYYY-MM-DD-<slug>.md`, with `-2`, `-3`… when that name is taken.
pub fn new_note_path(dir: &Path, title: &str, now: SystemTime) -> PathBuf {
    let date = chrono::DateTime::<chrono::Local>::from(now)
        .format("%Y-%m-%d")
        .to_string();
    let base = format!("{date}-{}", slugify(title));
    unique_path(dir, &base, "md")
}

fn daily_path(dir: &Path, now: SystemTime) -> PathBuf {
    let date = chrono::DateTime::<chrono::Local>::from(now)
        .format("%Y-%m-%d")
        .to_string();
    dir.join(format!("{date}.md"))
}

fn daily_header(now: SystemTime) -> String {
    let date = chrono::DateTime::<chrono::Local>::from(now)
        .format("%A, %B %-d, %Y")
        .to_string();
    format!("# {date}\n\n")
}

fn unique_path(dir: &Path, base: &str, extension: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{base}.{extension}"));
    let mut suffix = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base}-{suffix}.{extension}"));
        suffix += 1;
    }
    candidate
}

fn write_note(path: &Path, title: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("# {title}\n\n"))
}

/// The note editor: editxr when the image ships it, else the person's
/// `$EDITOR`, else nvim, else vi. Notes open in editxr even when `$EDITOR`
/// names nvim, because a note is a Markdown document that editxr renders as it
/// is typed; the shell keeps `$EDITOR` for `eff`, `git commit` and
/// `crontab -e`.
fn editor() -> Option<String> {
    if have("editxr") {
        return Some("editxr".to_string());
    }
    let from_env = std::env::var("EDITOR")
        .ok()
        .and_then(|value| value.split_whitespace().next().map(str::to_string))
        .filter(|value| !value.is_empty());
    from_env.or_else(|| {
        ["nvim", "vi"]
            .into_iter()
            .find(|program| have(program))
            .map(str::to_string)
    })
}

/// Edit in foot, then reopen this list in the same window. The path is one
/// argument to `sh -c`, never part of the script, so a space or quote in a note
/// name stays a name.
fn open_editor(path: &Path) -> Result<(), String> {
    open_editor_at(path, None)
}

/// How an editor takes a file, and whether it can start at a line. editxr is a
/// Markdown workspace: it gets the notes folder as its vault and no line
/// argument (it has `Ctrl+F` instead). nvim, vim, vi and micro take `+N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorKind {
    Editxr,
    LineAware,
    Plain,
}

fn editor_kind(editor: &str) -> EditorKind {
    match editor {
        "editxr" => EditorKind::Editxr,
        "micro" | "nvim" | "vim" | "vi" => EditorKind::LineAware,
        _ => EditorKind::Plain,
    }
}

/// The `sh -c` body. The path, line, vault and binary are positional
/// arguments, never part of the script, so a space or quote in a note name
/// stays a name.
fn editor_script(kind: EditorKind, at_line: bool) -> &'static str {
    match kind {
        EditorKind::Editxr => r#""$0" --vault "$1" "$2"; exec env HAYATO_UI=1 "$3" menu notes"#,
        EditorKind::LineAware if at_line => {
            r#""$0" +"$1" "$2"; exec env HAYATO_UI=1 "$3" menu notes"#
        }
        _ => r#""$0" "$1"; exec env HAYATO_UI=1 "$2" menu notes"#,
    }
}

/// editxr's themes are a fixed enum, not a palette file: the closest it can get
/// to a Hayato theme is the built-in theme of the same name. A theme with no
/// match (a custom `theme.toml`, everforest, ...) falls back to editxr's neutral
/// System palette, while the light/dark choice still follows the session's own
/// background so a light theme does not open a dark editor.
fn editxr_settings(slug: &str, theme: Option<&Theme>) -> (&'static str, &'static str) {
    let name = match slug {
        "solarized" => "solarized",
        "nord" => "nord",
        "gruvbox" => "gruvbox",
        "tokyo-night" => "tokyoNight",
        "catppuccin" | "catppuccin-latte" => "catppuccin",
        "dracula" => "dracula",
        "one-dark" => "oneDark",
        "github" => "github",
        "monokai" => "monokai",
        "mono" => "mono",
        "clay" => "clay",
        _ => "system",
    };
    let light = theme.is_some_and(|theme| {
        let bg = theme.palette().bg;
        (u32::from(bg.r) * 299 + u32::from(bg.g) * 587 + u32::from(bg.b) * 114) / 1000 > 128
    });
    (name, if light { "light" } else { "dark" })
}

/// editxr's config also holds the person's own settings, so this changes only
/// the two theme keys and leaves the rest of the JSON alone. `None` means the
/// file already says it and must not be touched: editxr rewrites the file on
/// exit, and a needless write would race that.
fn merged_editxr_config(existing: &str, theme: &str, appearance: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(existing)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = value.as_object_mut()?;
    let mut changed = false;
    // editxr 1.9.3's non-optional keys. The rest are optional and the editor
    // fills them; a config without these three does not decode at all, so a
    // first-run file has to carry them. Bumping the pin means re-reading
    // `Models/Config.swift` for a new one.
    for (key, default) in [
        ("wordWrap", json!(true)),
        ("renderMarkdown", json!(true)),
        ("llmProvider", json!("lmStudio")),
    ] {
        if !object.contains_key(key) {
            object.insert(key.to_string(), default);
            changed = true;
        }
    }
    if object.get("theme").and_then(Value::as_str) != Some(theme) {
        object.insert("theme".to_string(), Value::String(theme.to_string()));
        changed = true;
    }
    if object.get("appearance").and_then(Value::as_str) != Some(appearance) {
        object.insert(
            "appearance".to_string(),
            Value::String(appearance.to_string()),
        );
        changed = true;
    }
    if !changed {
        return None;
    }
    serde_json::to_string_pretty(&value).ok()
}

/// Point editxr at the session's theme before it opens. Best effort: a config
/// that cannot be written must not keep the note shut, and editxr still has its
/// own palette picker.
fn sync_editxr_theme() -> Result<(), String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    sync_editxr_theme_at(&home, &theme_store::Paths::from_env())
}

fn sync_editxr_theme_at(home: &Path, paths: &theme_store::Paths) -> Result<(), String> {
    let slug = theme_store::active(paths).unwrap_or_else(|| "tokyo-night".to_string());
    let theme = builtin::get(&slug).and_then(|text| Theme::parse(text).ok());
    let (name, appearance) = editxr_settings(&slug, theme.as_ref());
    let path = home.join(".config/editxr/config.json");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let Some(body) = merged_editxr_config(&existing, name, appearance) else {
        return Ok(());
    };
    theme_store::write_atomic(&path, &body)
}

/// Edit in foot and reopen the list afterwards. A search result passes its
/// line, which micro, nvim and vim take as `+N` before the path; editxr and an
/// editor without that convention open the note at the start instead.
fn open_editor_at(path: &Path, line: Option<u32>) -> Result<(), String> {
    let editor = editor().ok_or("No editor found: install editxr or set EDITOR")?;
    if !have("foot") {
        return Err("foot is missing, so the editor has no window".to_string());
    }
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "hayato".to_string());
    let title = format!(
        "--title={}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Note".to_string())
    );
    let kind = editor_kind(&editor);
    if kind == EditorKind::Editxr {
        // The editor reads its theme once at launch; a failed write is not a
        // reason to keep the note shut.
        let _ = sync_editxr_theme();
    }
    let line = line.filter(|_| kind == EditorKind::LineAware);
    let line_arg = line.map(|line| line.to_string());
    let path_arg = path.display().to_string();
    let vault_arg = notes_dir().display().to_string();
    let script = editor_script(kind, line_arg.is_some());
    let mut args = vec![
        "--app-id=popup",
        title.as_str(),
        "-e",
        "sh",
        "-c",
        script,
        editor.as_str(),
    ];
    match kind {
        EditorKind::Editxr => {
            args.push(vault_arg.as_str());
            args.push(path_arg.as_str());
        }
        _ => {
            if let Some(line_arg) = &line_arg {
                args.push(line_arg.as_str());
            }
            args.push(path_arg.as_str());
        }
    }
    args.push(exe.as_str());
    crate::system::spawn_detached_argv("foot", &args);
    Ok(())
}

fn preview(path: &Path) -> Result<(), String> {
    if !have("glow") {
        return Err("glow is not installed, so the note has no preview".to_string());
    }
    if !have("foot") {
        return Err("foot is missing, so the preview has no window".to_string());
    }
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "hayato".to_string());
    let title = format!(
        "--title={}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Note".to_string())
    );
    let script = r#"glow -p "$0"; exec env HAYATO_UI=1 "$1" menu notes"#;
    crate::system::spawn_detached_argv(
        "foot",
        &[
            "--app-id=popup",
            &title,
            "-e",
            "sh",
            "-c",
            script,
            &path.display().to_string(),
            &exe,
        ],
    );
    Ok(())
}

fn open_ink(path: &Path) -> Result<(), String> {
    if !have("xournalpp") {
        return Err("xournalpp is not installed (package xournalpp)".to_string());
    }
    super::doc_tools::spawn("xournalpp", &[&path.display().to_string()]);
    Ok(())
}

fn new_ink(dir: &Path) -> Result<(), String> {
    if !have("xournalpp") {
        return Err("xournalpp is not installed (package xournalpp)".to_string());
    }
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    // A new ink note is xournalpp's own unsaved document. Creating the `.xopp`
    // here would mean writing a format xournalpp owns; instead its save dialog
    // starts in the notes folder, and the list picks the file up once saved.
    super::doc_tools::spawn_in(Some(dir), "xournalpp", &[]);
    crate::system::say(
        "New ink note",
        format!(
            "Save it in {} (Ctrl+S); it then appears in Notes",
            dir.display()
        ),
        Urgency::Normal,
        Glyph::Brush,
    );
    Ok(())
}

fn spawn_self(args: &[String]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::system::spawn_detached_argv(exe, &args);
}

/// PDFs under Documents and Downloads, newest name first. `fd` keeps the walk
/// short and the list is capped, so a folder full of scans cannot stall the
/// window.
fn pdfs() -> Vec<PathBuf> {
    if !have("fd") {
        return Vec::new();
    }
    let mut roots = vec![
        notes_dir()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(notes_dir),
    ];
    if let Some(downloads) = dirs::download_dir() {
        roots.push(downloads);
    }
    let mut args = vec![
        "-e".to_string(),
        "pdf".to_string(),
        "--type".to_string(),
        "f".to_string(),
        "--max-results".to_string(),
        "200".to_string(),
        ".".to_string(),
    ];
    for root in &roots {
        if root.is_dir() {
            args.push(root.display().to_string());
        }
    }
    if args.len() <= 8 {
        return Vec::new();
    }
    let mut argv: Vec<&str> = vec!["fd"];
    argv.extend(args.iter().map(String::as_str));
    let Some(text) = super::out(&argv) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    files.sort();
    files
}

fn ink_notes() -> Vec<PathBuf> {
    scan(&notes_dir())
        .into_iter()
        .filter(|note| note.kind == NoteKind::Ink)
        .map(|note| note.path)
        .collect()
}

fn pick_file(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    title: &str,
    note: &str,
    files: &[PathBuf],
) -> Result<Option<PathBuf>, crate::Failure> {
    let rows: Vec<Row> = files
        .iter()
        .map(|path| {
            row(
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string()),
                path.parent()
                    .map(|parent| parent.display().to_string())
                    .unwrap_or_default(),
                Glyph::File,
            )
        })
        .collect();
    let keys: Vec<String> = files
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    let mut picker = Picker::new(title, vec![Question::choose(title, note, rows, keys)]);
    let code = crate::run_screen(terminal, design, title, &mut picker)?;
    if code != 0 {
        return Ok(None);
    }
    Ok(picker
        .answers()
        .first()
        .map(PathBuf::from)
        .filter(|path| path.exists()))
}

fn annotate_pdf(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<(), crate::Failure> {
    if !have("xournalpp") {
        crate::system::say(
            "Sign or annotate a PDF",
            "xournalpp is not installed (apt install xournalpp)",
            Urgency::Critical,
            Glyph::Warning,
        );
        return Ok(());
    }
    let files = pdfs();
    if files.is_empty() {
        crate::system::say(
            "Sign or annotate a PDF",
            "No PDF found in Documents or Downloads",
            Urgency::Normal,
            Glyph::File,
        );
        return Ok(());
    }
    let Some(path) = pick_file(
        terminal,
        design,
        "Sign or annotate a PDF",
        "xournalpp opens it · save the .xopp, then export the PDF to Documents/Signed",
        &files,
    )?
    else {
        return Ok(());
    };
    super::doc_tools::spawn("xournalpp", &[&path.display().to_string()]);
    Ok(())
}

fn export_ink(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<(), crate::Failure> {
    if !have("xournalpp") {
        crate::system::say(
            "Export an ink note",
            "xournalpp is not installed (apt install xournalpp)",
            Urgency::Critical,
            Glyph::Warning,
        );
        return Ok(());
    }
    let files = ink_notes();
    if files.is_empty() {
        crate::system::say(
            "Export an ink note",
            "No .xopp note in Documents/Notes yet",
            Urgency::Normal,
            Glyph::File,
        );
        return Ok(());
    }
    let Some(path) = pick_file(
        terminal,
        design,
        "Export ink to PDF",
        "xournalpp --create-pdf writes into Documents/Signed",
        &files,
    )?
    else {
        return Ok(());
    };
    let out_dir = signed_dir();
    if let Err(error) = fs::create_dir_all(&out_dir) {
        crate::system::say(
            "Export an ink note",
            format!("Cannot create {}: {error}", out_dir.display()),
            Urgency::Critical,
            Glyph::Warning,
        );
        return Ok(());
    }
    let out = unique_path(&out_dir, &stem(&path), "pdf");
    let status = Command::new("xournalpp")
        .arg(format!("--create-pdf={}", out.display()))
        .arg(&path)
        .status();
    match status {
        Ok(status) if status.success() && out.exists() => {
            crate::system::say_open(
                "Signed PDF",
                format!("Exported {}", out.display()),
                Urgency::Normal,
                Glyph::File,
                &out,
            );
        }
        Ok(status) => {
            crate::system::say(
                "Export an ink note",
                format!(
                    "xournalpp exited {status} without writing {}",
                    out.display()
                ),
                Urgency::Critical,
                Glyph::Warning,
            );
        }
        Err(error) => {
            crate::system::say(
                "Export an ink note",
                format!("Cannot run xournalpp: {error}"),
                Urgency::Critical,
                Glyph::Warning,
            );
        }
    }
    Ok(())
}

fn trash_note(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
    path: &Path,
) -> Result<(), crate::Failure> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let mut picker = Picker::new(
        "Move to trash",
        vec![Question::choose(
            "Move to trash",
            "The note goes to the trash, where Files can restore it",
            vec![
                row("Keep the note", "nothing is deleted", Glyph::ArrowLeft),
                row(
                    format!("Move “{name}” to trash"),
                    "recoverable from Files",
                    Glyph::Trash,
                ),
            ],
            vec!["keep".to_string(), "trash".to_string()],
        )],
    );
    let code = crate::run_screen(terminal, design, "Move to trash", &mut picker)?;
    if code != 0 || picker.answers().first().map(String::as_str) != Some("trash") {
        return Ok(());
    }
    let result: Result<(), String> = if have("gio") {
        Command::new("gio")
            .arg("trash")
            .arg(path)
            .status()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("gio exited {status}"))
                }
            })
    } else {
        fs::remove_file(path).map_err(|error| error.to_string())
    };
    if let Err(reason) = result {
        crate::system::say(
            "Notes",
            format!("Could not move the note to trash: {reason}"),
            Urgency::Critical,
            Glyph::Warning,
        );
    }
    Ok(())
}

/// One `rg` hit: the note, the line and the line's text.
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    path: PathBuf,
    line: u32,
    text: String,
}

/// `rg --null --line-number` writes `path\0line:text`, one match per line.
/// The NUL is what keeps a path with a colon in it a path.
fn parse_matches(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for record in text.lines() {
        let Some((path, rest)) = record.split_once('\0') else {
            continue;
        };
        let Some((line, text)) = rest.split_once(':') else {
            continue;
        };
        let Ok(line) = line.parse::<u32>() else {
            continue;
        };
        hits.push(Hit {
            path: PathBuf::from(path),
            line,
            text: text.trim_end().to_string(),
        });
    }
    hits
}

/// Search the notes' contents with ripgrep, then open the chosen hit in
/// editxr. The title filter on the list stays a title filter: a full-text
/// search is one row, so typing never walks every note.
fn search_notes(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<bool, crate::Failure> {
    let mut picker = Picker::new(
        "Search the notes",
        vec![Question::input(
            "Search the notes",
            "ripgrep over ~/Documents/Notes · Enter searches",
            "a word or phrase",
            false,
        )],
    );
    let code = crate::run_screen(terminal, design, "Search the notes", &mut picker)?;
    if code != 0 {
        return Ok(false);
    }
    let Some(query) = picker
        .answers()
        .first()
        .map(|answer| answer.trim().to_string())
        .filter(|query| !query.is_empty())
    else {
        return Ok(false);
    };
    let dir = notes_dir();
    let dir_text = dir.display().to_string();
    let argv = [
        "rg",
        "--line-number",
        "--no-heading",
        "--color",
        "never",
        "--with-filename",
        "--null",
        "--max-count",
        "20",
        "--glob",
        "!README.md",
        "--",
        query.as_str(),
        dir_text.as_str(),
    ];
    let hits = super::out(&argv)
        .map(|text| parse_matches(&text))
        .unwrap_or_default();
    if hits.is_empty() {
        crate::system::say(
            "Notes",
            format!("No note contains “{query}”"),
            Urgency::Normal,
            Glyph::Search,
        );
        return Ok(false);
    }
    let rows: Vec<Row> = hits
        .iter()
        .map(|hit| {
            let name = hit
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| hit.path.display().to_string());
            row(
                format!("{name}:{}", hit.line),
                hit.text.as_str(),
                Glyph::File,
            )
        })
        .collect();
    let keys: Vec<String> = (0..hits.len()).map(|index| index.to_string()).collect();
    let mut picker = Picker::new(
        "Matches",
        vec![Question::choose(
            "Matches",
            format!("{} match(es) · Enter opens it in editxr", hits.len()),
            rows,
            keys,
        )],
    );
    let code = crate::run_screen(terminal, design, "Matches", &mut picker)?;
    if code != 0 {
        return Ok(false);
    }
    let Some(index) = picker
        .answers()
        .first()
        .and_then(|answer| answer.parse::<usize>().ok())
        .filter(|index| *index < hits.len())
    else {
        return Ok(false);
    };
    let hit = &hits[index];
    open_editor_at(&hit.path, Some(hit.line))
        .map(|()| true)
        .map_err(|reason| crate::Failure::new("Notes", reason))
}

/// The rows `hayato menu notes --rows` prints for the checks: the same actions
/// the window draws, without opening it.
pub fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("new", "New quick note", "name it, then write it"),
        ("daily", "Today's note", "open or create today's"),
        (
            "ink",
            "New ink note",
            "xournalpp · .xopp in Documents/Notes",
        ),
        (
            "annotate",
            "Sign or annotate a PDF",
            "xournalpp · export to Documents/Signed",
        ),
        (
            "export",
            "Export ink to PDF",
            "xournalpp --create-pdf · Documents/Signed",
        ),
        ("search", "Search the notes", "ripgrep · open in editxr"),
        ("folder", "Open folder", "Documents/Notes"),
    ]
}

/// `hayato menu notes` (`hayato-notes`): the window.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    ensure_notes().map_err(|error| {
        crate::Failure::new(
            "Notes",
            format!("Cannot use {}: {error}", notes_dir().display()),
        )
    })?;
    loop {
        let mut screen = NotesScreen::new();
        let code = crate::run_screen(terminal, design, "Notes", &mut screen)?;
        match screen.pending.take() {
            Some(Pending::Annotate) => annotate_pdf(terminal, design)?,
            Some(Pending::Export) => export_ink(terminal, design)?,
            Some(Pending::Search) => {
                // A hit opens the editor, whose own chain reopens the list in
                // its window; reopening it here too would leave two menus.
                if search_notes(terminal, design)? {
                    return Ok(0);
                }
            }
            Some(Pending::Trash(path)) => trash_note(terminal, design, &path)?,
            Some(Pending::Share(path)) => {
                let _ = crate::system::share::open(
                    crate::system::share::SharePayload::from_path(path),
                    terminal,
                    design,
                )?;
            }
            None => return Ok(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hayato-notes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(text: &str) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(text.parse().unwrap())
    }

    #[test]
    fn the_title_is_the_first_heading_then_the_first_line() {
        assert_eq!(title_from("# Groceries\n\nmilk", "a.md"), "Groceries");
        assert_eq!(
            title_from("\n\nCall the plumber\n", "a.md"),
            "Call the plumber"
        );
        assert_eq!(title_from("", "2026-11-19-note.md"), "note");
    }

    #[test]
    fn a_date_prefix_and_separators_stay_out_of_the_title() {
        assert_eq!(ink_title("2026-11-18-ink.xopp"), "ink");
        assert_eq!(ink_title("morning_pages.xopp"), "morning pages");
        assert_eq!(ink_title("sketch.xopp"), "sketch");
        assert_eq!(display_stem("2026-11-18.md"), "2026 11 18");
        assert_eq!(display_stem("2026-11-18-.md"), "2026 11 18");
    }

    #[test]
    fn the_page_does_not_repeat_the_note_title() {
        let mut lines = markdown_lines("# Morning pages\n\nA short note.\n");
        strip_title(&mut lines, "Morning pages");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "A short note.");
        let mut body = markdown_lines("Call the plumber\n\nTomorrow\n");
        strip_title(&mut body, "Call the plumber");
        assert_eq!(body[0].text, "Tomorrow");
    }

    #[test]
    fn a_slug_is_lower_case_words_joined_by_dashes() {
        assert_eq!(slugify("My Great Note!"), "my-great-note");
        assert_eq!(slugify("  spaces  "), "spaces");
        assert_eq!(slugify("!!!"), "note");
        assert_eq!(slugify("Über café"), "über-café");
    }

    #[test]
    fn a_new_note_gets_the_date_and_a_free_name() {
        let dir = temp_dir("new");
        let path = new_note_path(&dir, "Ideas", at("1795000000"));
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "2026-11-18-ideas.md"
        );
        fs::write(&path, "# Ideas\n").unwrap();
        let second = new_note_path(&dir, "Ideas", at("1795000000"));
        assert_eq!(
            second.file_name().unwrap().to_string_lossy(),
            "2026-11-18-ideas-2.md"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_daily_note_is_the_date() {
        let dir = temp_dir("daily");
        let path = daily_path(&dir, at("1795000000"));
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "2026-11-18.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_skips_the_readme_and_reads_both_kinds() {
        let dir = temp_dir("scan");
        fs::write(dir.join("README.md"), "# Notes\n").unwrap();
        fs::write(dir.join("2026-11-18-ideas.md"), "# Ideas\n\nbody\n").unwrap();
        fs::write(dir.join("2026-11-18-ink.xopp"), "not really xopp\n").unwrap();
        fs::write(dir.join("photo.png"), "not a note\n").unwrap();
        let notes = scan(&dir);
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().any(|note| note.title == "Ideas"));
        assert!(notes.iter().any(|note| note.title == "ink"));
        assert!(notes.iter().all(|note| !note.title.contains("README")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(3 * 1_048_576), "3 MB");
    }

    #[test]
    fn the_preview_reads_markdown_without_a_parser() {
        let lines = markdown_lines(
            "# Title\n\nSome **bold** text\n\n- one\n- two\n\n> quote\n\n```\ncode\n```\n",
        );
        assert_eq!(lines[0].style, PreviewStyle::Title);
        assert_eq!(lines[0].text, "Title");
        assert!(
            lines
                .iter()
                .any(|line| line.text == "Some bold text" && line.style == PreviewStyle::Body)
        );
        assert!(
            lines
                .iter()
                .any(|line| line.text == "• one" && line.style == PreviewStyle::Bullet)
        );
        assert!(
            lines
                .iter()
                .any(|line| line.text == "│ quote" && line.style == PreviewStyle::Quote)
        );
        assert!(
            lines
                .iter()
                .any(|line| line.text == "code" && line.style == PreviewStyle::Code)
        );
        assert!(
            !lines
                .windows(2)
                .any(|pair| pair[0].text.is_empty() && pair[1].text.is_empty())
        );
    }

    #[test]
    fn the_preview_wraps_at_the_page_width() {
        let lines = vec![
            PreviewLine {
                text: "one two three four".to_string(),
                style: PreviewStyle::Body,
            },
            PreviewLine {
                text: String::new(),
                style: PreviewStyle::Dim,
            },
            PreviewLine {
                text: "tail".to_string(),
                style: PreviewStyle::Title,
            },
        ];
        let wrapped = wrap(&lines, 7);
        assert!(wrapped.len() >= 3);
        assert!(
            wrapped
                .iter()
                .all(|(text, _)| UnicodeWidthStr::width(text.as_str()) <= 7)
        );
        assert!(
            wrapped
                .iter()
                .any(|(text, style)| text.is_empty() && *style == PreviewStyle::Dim)
        );
        assert_eq!(wrapped.last().unwrap().0, "tail");
    }

    #[test]
    fn the_page_only_appears_when_there_is_room() {
        let wide = panes(Rect::new(0, 0, 152, 45));
        assert!(wide.page.is_some());
        assert!(wide.strip.is_none());
        let narrow = panes(Rect::new(0, 0, 80, 24));
        assert!(narrow.page.is_none());
        assert!(narrow.strip.is_some());
        let tiny = panes(Rect::new(0, 0, 50, 20));
        assert!(tiny.page.is_none());
        assert!(tiny.strip.is_none());
    }

    #[test]
    fn the_note_editor_gets_its_own_invocation() {
        assert_eq!(editor_kind("editxr"), EditorKind::Editxr);
        assert_eq!(editor_kind("nvim"), EditorKind::LineAware);
        assert_eq!(editor_kind("hx"), EditorKind::Plain);
        assert!(editor_script(EditorKind::Editxr, true).contains("--vault"));
        assert!(!editor_script(EditorKind::Editxr, true).contains('+'));
        assert!(editor_script(EditorKind::LineAware, true).contains(r#""$0" +"$1""#));
        assert_eq!(
            editor_script(EditorKind::Plain, true),
            editor_script(EditorKind::Plain, false)
        );
    }

    #[test]
    fn editxr_follows_the_session_theme() {
        let solarized = Theme::parse(builtin::SOLARIZED).unwrap();
        assert_eq!(
            editxr_settings("solarized", Some(&solarized)),
            ("solarized", "dark")
        );
        let latte = Theme::parse(builtin::CATPPUCCIN_LATTE).unwrap();
        assert_eq!(
            editxr_settings("catppuccin-latte", Some(&latte)),
            ("catppuccin", "light")
        );
        assert_eq!(editxr_settings("everforest", None), ("system", "dark"));
    }

    #[test]
    fn the_editxr_merge_keeps_the_persons_settings() {
        let first = merged_editxr_config("", "solarized", "dark").unwrap();
        let value: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(value["wordWrap"], serde_json::json!(true));
        assert_eq!(value["renderMarkdown"], serde_json::json!(true));
        assert_eq!(value["llmProvider"], serde_json::json!("lmStudio"));

        let before =
            r#"{"wordWrap":false,"llmProvider":"openai","theme":"clay","appearance":"auto"}"#;
        let after = merged_editxr_config(before, "solarized", "dark").unwrap();
        let value: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(value["wordWrap"], serde_json::json!(false));
        assert_eq!(value["llmProvider"], serde_json::json!("openai"));
        assert_eq!(value["theme"], serde_json::json!("solarized"));
        assert_eq!(value["appearance"], serde_json::json!("dark"));
        assert!(merged_editxr_config(&after, "solarized", "dark").is_none());
    }

    #[test]
    fn the_editxr_theme_file_is_written_once() {
        let home = temp_dir("editxr");
        let state = home.join(".local/state/hayato/theme");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("theme"), "catppuccin-latte\n").unwrap();
        let paths = theme_store::Paths::new(&home, &state);
        sync_editxr_theme_at(&home, &paths).unwrap();
        let path = home.join(".config/editxr/config.json");
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["theme"], serde_json::json!("catppuccin"));
        assert_eq!(value["appearance"], serde_json::json!("light"));
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        sync_editxr_theme_at(&home, &paths).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "a second open must not rewrite the config"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn words_and_action_pages_are_counted_and_named() {
        assert_eq!(word_count("one two\nthree"), 3);
        let search = action_preview("search");
        assert_eq!(search.title, "Search the notes");
        assert_eq!(search.badge, "ACTION");
        assert!(!search.lines.is_empty());
    }

    #[test]
    fn rg_hits_keep_the_path_line_and_text() {
        let text = "2026-11-18-ideas.md\x0012:first line\n2026-11-18-ink.xopp\x003:ink\n";
        let hits = parse_matches(text);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, PathBuf::from("2026-11-18-ideas.md"));
        assert_eq!(hits[0].line, 12);
        assert_eq!(hits[0].text, "first line");
        assert_eq!(hits[1].line, 3);
        assert!(parse_matches("not a record").is_empty());
    }

    #[test]
    fn the_check_rows_name_the_document_tools() {
        let keys: Vec<&str> = rows().iter().map(|(key, _, _)| *key).collect();
        assert!(keys.contains(&"new"));
        assert!(keys.contains(&"ink"));
        assert!(keys.contains(&"annotate"));
        assert!(keys.contains(&"export"));
    }
}
