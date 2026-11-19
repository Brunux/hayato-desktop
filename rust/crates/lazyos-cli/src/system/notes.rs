//! Notes and documents (`lazyos menu notes`, `lazyos-notes`).
//!
//! One list for the three note shapes this image has, and a thin wrapper
//! around the tool that owns each:
//!
//! * **quick notes** are Markdown files in `~/Documents/Notes`, edited in
//!   `$EDITOR` (nvim) in a foot window; after the editor exits the same window
//!   reopens this list;
//! * **ink notes** are `.xopp` files opened in xournalpp, which also annotates
//!   and signs PDFs; exported PDFs land in `~/Documents/Signed`;
//! * **books** (`.epub`) are not notes: Files and `lazyos open` send them to
//!   foliate, because reading is not note-taking.
//!
//! The folder is the state. There is no index file and no daemon: the list is
//! a directory read, the README is the only file this menu writes by itself,
//! and a note the person deletes elsewhere simply stops appearing.

use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use lazyos_ui::{
    BodyCtx, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row, Screen,
    TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use super::pick::{Picker, Question, row};

const README: &str = "\
# Notes

Quick notes are Markdown files in this folder. **Alt+Space -> Notes** lists
them; Enter edits one in nvim, and quitting the editor returns to the list.

Ink notes are `.xopp` files, edited in xournalpp. The same menu annotates and
signs a PDF and exports an ink note to `../Signed/`.

This file is the only one the menu writes by itself.
";

/// What a row does once it is chosen. The pickers that need the terminal are
/// run by `menu()` after the screen closes, then the list reopens.
enum Pending {
    Annotate,
    Export,
    Search,
    Trash(PathBuf),
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

/// The notes screen.
pub struct NotesScreen {
    notes: Vec<Note>,
    list: ListState,
    keys: Vec<String>,
    mode: Mode,
    title_input: Input,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status: String,
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
            header: Header::new("LazyOS").subtitle("Notes"),
            keybar: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("Ctrl+P", "preview"),
                KeyHint::new("Del", "trash"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            status: String::new(),
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
            let kind = match note.kind {
                NoteKind::Text => "note",
                NoteKind::Ink => "ink note",
            };
            rows.push(
                row(
                    note.title.as_str(),
                    format!("{kind} · {}", detail(note)),
                    glyph,
                )
                .value(if note.kind == NoteKind::Ink {
                    "xopp"
                } else {
                    "md"
                }),
            );
            keys.push(format!("note:{index}"));
        }
        rows.push(row(
            "New quick note",
            "name it, then edit it in nvim",
            Glyph::File,
        ));
        keys.push("new".to_string());
        rows.push(row(
            "Today's note",
            "open or create YYYY-MM-DD.md",
            Glyph::Calendar,
        ));
        keys.push("daily".to_string());
        rows.push(row(
            "New ink note",
            if have("xournalpp") {
                "xournalpp · save it here with Ctrl+S"
            } else {
                "install xournalpp to write and draw"
            },
            Glyph::Brush,
        ));
        keys.push("ink".to_string());
        rows.push(row(
            "Annotate or sign a PDF",
            if have("xournalpp") {
                "xournalpp · export the signed copy to Documents/Signed"
            } else {
                "install xournalpp (8.8 MB)"
            },
            Glyph::File,
        ));
        keys.push("annotate".to_string());
        rows.push(row(
            "Export an ink note to PDF",
            if have("xournalpp") {
                "xournalpp --create-pdf · into Documents/Signed"
            } else {
                "install xournalpp (8.8 MB)"
            },
            Glyph::Download,
        ));
        keys.push("export".to_string());
        rows.push(row(
            "Search inside the notes",
            "ripgrep over the folder · Enter opens at the line",
            Glyph::Search,
        ));
        keys.push("search".to_string());
        rows.push(row(
            "Open the notes folder",
            notes_dir().display().to_string(),
            Glyph::Folder,
        ));
        keys.push("folder".to_string());
        self.list.set_rows(rows);
        self.keys = keys;
        self.status = format!(
            "{} note(s) · {} · Enter opens",
            self.notes.len(),
            notes_dir().display()
        );
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
                    Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
                    Err(reason) => {
                        self.status = reason;
                        Outcome::Continue
                    }
                },
                NoteKind::Ink => match open_ink(&path) {
                    Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
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
                    Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
                    Err(reason) => {
                        self.status = reason;
                        Outcome::Continue
                    }
                }
            }
            "ink" => match new_ink(&notes_dir()) {
                Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
                Err(reason) => {
                    self.status = reason;
                    Outcome::Continue
                }
            },
            "annotate" => {
                self.pending = Some(Pending::Annotate);
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            "export" => {
                self.pending = Some(Pending::Export);
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            "search" => {
                self.pending = Some(Pending::Search);
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            "folder" => {
                let dir = notes_dir();
                spawn_self(&[
                    "menu".into(),
                    "files".into(),
                    "--".into(),
                    dir.display().to_string(),
                ]);
                Outcome::Done(lazyos_ui::app::exit::OK)
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
}

impl Screen for NotesScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Notes"), true);
        let (filter, rows) = super::pick::list_layout(ctx.area);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            "type to filter · Enter opens · Escape closes",
            self.list.len(),
            self.keys.len(),
        );
        self.rows_height.set(rows.height);
        match self.mode {
            Mode::List => {
                self.list.render(rows, ctx.buf, ctx.design, true);
                ctx.buf.set_string(
                    rows.x,
                    rows.y + rows.height.saturating_sub(1),
                    &self.status,
                    ctx.design.style(TextStyle::Dim),
                );
            }
            Mode::NewTitle => {
                ctx.buf.set_string(
                    rows.x,
                    rows.y,
                    "Title for the new note (Enter creates it, Escape goes back)",
                    ctx.design.style(TextStyle::Title),
                );
                let input_rect = Rect {
                    x: rows.x,
                    y: rows.y + 2,
                    width: rows.width,
                    height: 1,
                };
                self.title_input
                    .render(input_rect, ctx.buf, ctx.design, "What is it about?");
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
                        Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
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
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(note) = self.selected_note() {
                let path = note.path.clone();
                if note.kind == NoteKind::Text {
                    return match preview(&path) {
                        Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
                        Err(reason) => {
                            self.status = reason;
                            Outcome::Continue
                        }
                    };
                }
            }
            return Outcome::Consumed;
        }
        if key.code == KeyCode::Delete {
            if let Some(note) = self.selected_note() {
                let path = note.path.clone();
                self.pending = Some(Pending::Trash(path));
                return Outcome::Done(lazyos_ui::app::exit::OK);
            }
            return Outcome::Consumed;
        }
        let rows_height = self.rows_height.get();
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
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
    truncate(&stem(Path::new(name)), 72)
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn ink_title(name: &str) -> String {
    let stem = stem(Path::new(name));
    let spaced = stem.replace(['-', '_'], " ");
    let spaced = spaced.trim();
    if spaced.is_empty() {
        stem
    } else {
        spaced.to_string()
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{} MB", bytes / 1_048_576),
    }
}

fn detail(note: &Note) -> String {
    let stamp = note
        .modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| {
            chrono::DateTime::<chrono::Local>::from(UNIX_EPOCH + since)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string());
    format!("{stamp} · {}", human_size(note.size))
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

/// The editor from `$EDITOR`, else nvim, else vi. Only the program is used;
/// flags belong in the person's editor configuration, not in a menu.
fn editor() -> Option<String> {
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

/// Edit in foot and reopen the list afterwards. A search result passes its
/// line, which nvim and vim take as `+N` before the path; an editor without
/// that convention opens the note at the start instead.
fn open_editor_at(path: &Path, line: Option<u32>) -> Result<(), String> {
    let editor = editor().ok_or("No editor found: install nvim or set EDITOR")?;
    if !have("foot") {
        return Err("foot is missing, so the editor has no window".to_string());
    }
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "lazyos".to_string());
    let title = format!(
        "--title={}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Note".to_string())
    );
    let line = line.filter(|_| matches!(editor.as_str(), "nvim" | "vim" | "vi"));
    let line_arg = line.map(|line| line.to_string());
    let path_arg = path.display().to_string();
    let script = if line_arg.is_some() {
        r#""$0" +"$1" "$2"; exec env LAZYOS_UI=1 "$3" menu notes"#
    } else {
        r#""$0" "$1"; exec env LAZYOS_UI=1 "$2" menu notes"#
    };
    let mut args = vec![
        "--app-id=popup",
        title.as_str(),
        "-e",
        "sh",
        "-c",
        script,
        editor.as_str(),
    ];
    if let Some(line_arg) = &line_arg {
        args.push(line_arg.as_str());
    }
    args.push(path_arg.as_str());
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
        .unwrap_or_else(|_| "lazyos".to_string());
    let title = format!(
        "--title={}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Note".to_string())
    );
    let script = r#"glow -p "$0"; exec env LAZYOS_UI=1 "$1" menu notes"#;
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
            "Annotate or sign a PDF",
            "xournalpp is not installed (apt install xournalpp)",
            Urgency::Critical,
            Glyph::Warning,
        );
        return Ok(());
    }
    let files = pdfs();
    if files.is_empty() {
        crate::system::say(
            "Annotate or sign a PDF",
            "No PDF found in Documents or Downloads",
            Urgency::Normal,
            Glyph::File,
        );
        return Ok(());
    }
    let Some(path) = pick_file(
        terminal,
        design,
        "Annotate or sign a PDF",
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
        "Export an ink note to PDF",
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

/// Search the notes' contents with ripgrep, then open the chosen hit at its
/// line. The title filter on the list stays a title filter: a full-text search
/// is one row, so typing never walks every note.
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
            format!(
                "{} match(es) · Enter opens the note at the line",
                hits.len()
            ),
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

/// The rows `lazyos menu notes --rows` prints for the checks: the same actions
/// the window draws, without opening it.
pub fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("new", "New quick note", "name it, then edit in nvim"),
        ("daily", "Today's note", "open or create YYYY-MM-DD.md"),
        (
            "ink",
            "New ink note",
            "xournalpp · .xopp in Documents/Notes",
        ),
        (
            "annotate",
            "Annotate or sign a PDF",
            "xournalpp · export to Documents/Signed",
        ),
        (
            "export",
            "Export an ink note to PDF",
            "xournalpp --create-pdf · Documents/Signed",
        ),
        (
            "search",
            "Search inside the notes",
            "ripgrep · open at the line",
        ),
        ("folder", "Open the notes folder", "Documents/Notes"),
    ]
}

/// `lazyos menu notes` (`lazyos-notes`): the window.
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
            None => return Ok(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lazyos-notes-{name}-{}", std::process::id()));
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
        assert_eq!(title_from("", "2026-11-19-note.md"), "2026-11-19-note");
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
        assert!(notes.iter().any(|note| note.title == "2026 11 18 ink"));
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
