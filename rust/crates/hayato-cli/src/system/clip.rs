//! Clipboard Inspector Studio: kind-filtered history, cached full preview, copy back,
//! save image, and guarded wipe (`hayato-clip`).
//!
//! Reads `cliphist list` (tab-separated `id<TAB>preview`), classifies each entry
//! (image, link, or text), and caches full decoded previews per entry ID so moving
//! through history never re-decodes or allocates per steady-state frame.
//!
//! Destructive wipe (`Ctrl+X`) replaces the body with a `Confirm` dialog instead of
//! wiping immediately.

#![allow(clippy::collapsible_if)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Stdio;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Confirm, ConfirmAction, Design,
    Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_card,
    draw_empty, draw_filter_bar,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::{missing, out};

const NEEDED: &[&str] = &["cliphist", "wl-copy", "wl-paste", "xdg-open", "file"];

const HIT_STRIP_BASE: usize = 10_000;
const HIT_KIND_BASE: usize = 20_000;

/// Strip common tracking query parameters (`utm_*`, `fbclid`, `gclid`, etc.)
/// from a URL while preserving all functional query parameters.
pub fn clean_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some((base, query_and_frag)) = trimmed.split_once('?') else {
        return trimmed.to_string();
    };
    let (query, frag) = match query_and_frag.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query_and_frag, None),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
            !key.is_empty()
                && !key.starts_with("utm_")
                && !matches!(
                    key.as_str(),
                    "fbclid"
                        | "gclid"
                        | "dclid"
                        | "mc_cid"
                        | "mc_eid"
                        | "igshid"
                        | "si"
                        | "ref_src"
                )
        })
        .collect();
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Remove ANSI CSI escape sequences from terminal text.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Collapse multi-line whitespace into a single clean line.
pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Automatically pick the most helpful 0 ms pure-Rust transform for `text`.
pub fn smart_transform(text: &str) -> (&'static str, String) {
    let trimmed = text.trim();
    if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.contains(char::is_whitespace)
    {
        let cleaned = clean_url(trimmed);
        return ("Cleaned URL tracking params", cleaned);
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(pretty) = serde_json::to_string_pretty(&val)
    {
        return ("Formatted JSON", pretty);
    }
    if text.contains('\x1b') {
        return ("Stripped ANSI escape codes", strip_ansi(text));
    }
    ("Collapsed to single line", single_line(text))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipKind {
    Image,
    Link,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KindFilter {
    All,
    Text,
    Link,
    Image,
}

impl KindFilter {
    const ALL: [Self; 4] = [Self::All, Self::Text, Self::Link, Self::Image];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Text => "Text",
            Self::Link => "Links",
            Self::Image => "Images",
        }
    }

    fn matches(self, kind: ClipKind) -> bool {
        match self {
            Self::All => true,
            Self::Text => kind == ClipKind::Text,
            Self::Link => kind == ClipKind::Link,
            Self::Image => kind == ClipKind::Image,
        }
    }
}

#[derive(Clone, Debug)]
struct ClipEntry {
    id: String,
    preview: String,
    kind: ClipKind,
}

#[derive(Clone, Debug)]
enum DecodedPreview {
    Text {
        lines: Vec<String>,
        line_count: usize,
        char_count: usize,
    },
    Link {
        url: String,
        host: String,
    },
    Image {
        meta: String,
    },
}

enum ClipMode {
    Browse,
    ConfirmWipe(Confirm),
}

/// The Clipboard Inspector Studio screen.
pub struct ClipScreen {
    entries: Vec<ClipEntry>,
    visible_indices: Vec<usize>,
    kind_filter: KindFilter,
    mode: ClipMode,
    preview_cache: HashMap<String, DecodedPreview>,
    strip: ActionStrip,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    kind_hits: RefCell<Vec<Rect>>,
    status_note: Option<String>,
    static_mode: bool,
}

impl Default for ClipScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            entries: Vec::new(),
            visible_indices: Vec::new(),
            kind_filter: KindFilter::All,
            mode: ClipMode::Browse,
            preview_cache: HashMap::new(),
            strip: ActionStrip::default(),
            list: ListState::new().with_empty("nothing has been copied yet"),
            header: Header::new("Hayato").subtitle("Clipboard"),
            keybar: vec![
                KeyHint::new("Enter", "copy back"),
                KeyHint::new("Tab", "filter kind"),
                KeyHint::new("Del", "delete"),
                KeyHint::new("Ctrl+Y", "share"),
                KeyHint::new("Ctrl+S", "save img"),
                KeyHint::new("Ctrl+X", "clear"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            kind_hits: RefCell::new(Vec::new()),
            status_note: None,
            static_mode: false,
        };
        screen.reload();
        screen
    }

    /// Deterministic constructor for unit tests and snapshots.
    #[cfg(test)]
    fn from_entries_for_test(entries: Vec<(&str, &str)>) -> Self {
        let mut screen = Self {
            entries: entries
                .into_iter()
                .map(|(id, preview)| ClipEntry {
                    id: id.to_string(),
                    kind: classify_preview(preview),
                    preview: preview.to_string(),
                })
                .collect(),
            visible_indices: Vec::new(),
            kind_filter: KindFilter::All,
            mode: ClipMode::Browse,
            preview_cache: HashMap::new(),
            strip: ActionStrip::default(),
            list: ListState::new().with_empty("nothing has been copied yet"),
            header: Header::new("Hayato").subtitle("Clipboard"),
            keybar: vec![
                KeyHint::new("Enter", "copy back"),
                KeyHint::new("Tab", "filter kind"),
                KeyHint::new("Del", "delete"),
                KeyHint::new("Ctrl+Y", "share"),
                KeyHint::new("Ctrl+S", "save img"),
                KeyHint::new("Ctrl+X", "clear"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            kind_hits: RefCell::new(Vec::new()),
            status_note: None,
            static_mode: true,
        };
        screen.build_rows();
        screen
    }

    fn reload(&mut self) {
        if !self.static_mode {
            self.entries = Self::get_entries();
        }
        self.preview_cache.clear();
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
            let kind = classify_preview(&preview);
            entries.push(ClipEntry { id, preview, kind });
        }
        entries
    }

    fn build_rows(&mut self) {
        self.strip.set_items(vec![
            ActionItem::new("share", "^Y", "Share").glyph(Glyph::Qr),
            ActionItem::new("save", "^S", "Save img").glyph(Glyph::Image),
            ActionItem::new("wipe", "^X", format!("Clear ({})", self.entries.len()))
                .glyph(Glyph::Trash),
        ]);

        let mut rows = Vec::new();
        let mut visible_indices = Vec::new();
        for (idx, e) in self.entries.iter().enumerate() {
            if !self.kind_filter.matches(e.kind) {
                continue;
            }
            let (glyph, kind_str) = match e.kind {
                ClipKind::Image => (Glyph::Image, "image"),
                ClipKind::Link => (Glyph::Link, "link"),
                ClipKind::Text => (Glyph::Clipboard, "text"),
            };
            let shown: String = e.preview.chars().take(54).collect();
            let shown = if e.preview.chars().count() > 54 {
                format!("{shown}…")
            } else {
                shown
            };
            rows.push(Row::new(shown).glyph(glyph).value(kind_str));
            visible_indices.push(idx);
        }
        self.visible_indices = visible_indices;
        self.list.update_rows(rows);
        self.ensure_selected_preview();
    }

    fn selected_entry(&self) -> Option<&ClipEntry> {
        let row_idx = self.list.selected_index()?;
        let entry_idx = *self.visible_indices.get(row_idx)?;
        self.entries.get(entry_idx)
    }

    fn ensure_selected_preview(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if self.preview_cache.contains_key(&entry.id) {
            return;
        }
        let decoded = match entry.kind {
            ClipKind::Image => DecodedPreview::Image {
                meta: entry
                    .preview
                    .trim_start_matches("[[ binary data")
                    .trim_end_matches("]]")
                    .trim()
                    .to_string(),
            },
            ClipKind::Link => {
                let full = if self.static_mode {
                    entry.preview.clone()
                } else {
                    decode_text(&entry.id).unwrap_or_else(|| entry.preview.clone())
                };
                let url = full.trim().to_string();
                let host = url
                    .strip_prefix("https://")
                    .or_else(|| url.strip_prefix("http://"))
                    .unwrap_or(&url)
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
                DecodedPreview::Link { url, host }
            }
            ClipKind::Text => {
                let full = if self.static_mode {
                    entry.preview.clone()
                } else {
                    decode_text(&entry.id).unwrap_or_else(|| entry.preview.clone())
                };
                let char_count = full.chars().count();
                let all_lines: Vec<String> = full.lines().map(String::from).collect();
                let line_count = all_lines.len().max(1);
                let lines = all_lines.into_iter().take(48).collect();
                DecodedPreview::Text {
                    lines,
                    line_count,
                    char_count,
                }
            }
        };
        self.preview_cache.insert(entry.id, decoded);
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 48, 46)
    }

    fn left_layout(area: Rect) -> (Rect, Rect, Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let tabs = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let strip_y = inner.y.saturating_add(1).min(inner.bottom());
        let strip = Rect::new(
            inner.x,
            strip_y,
            inner.width,
            1.min(inner.bottom().saturating_sub(strip_y)),
        );
        let filter_y = inner.y.saturating_add(3).min(inner.bottom());
        let filter = Rect::new(
            inner.x,
            filter_y,
            inner.width,
            1.min(inner.bottom().saturating_sub(filter_y)),
        );
        let rows_y = inner.y.saturating_add(5).min(inner.bottom());
        let rows = Rect::new(
            inner.x,
            rows_y,
            inner.width,
            inner.bottom().saturating_sub(rows_y),
        );
        (tabs, strip, filter, rows)
    }

    fn draw_kind_tabs(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let mut hits = self.kind_hits.borrow_mut();
        hits.clear();
        if area.height == 0 {
            return;
        }
        let mut x = area.x;
        for kind in KindFilter::ALL {
            let count = self.entries.iter().filter(|e| kind.matches(e.kind)).count();
            let active = kind == self.kind_filter;
            let text = if active {
                format!("[{} {count}]", kind.label())
            } else {
                format!(" {} {count} ", kind.label())
            };
            let w = text.chars().count() as u16;
            if x + w > area.right() {
                break;
            }
            let style = if active {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Dim)
            };
            put_trunc(buf, x, area.y, &text, w, style);
            hits.push(Rect::new(x, area.y, w, 1));
            x += w + 1;
        }
    }

    fn draw_inspector(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 12 || inner.height < 4 {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            draw_empty(
                buf,
                inner,
                design,
                "No entry selected",
                "Select a clipboard item on the left to inspect its full content",
            );
            return;
        };
        let Some(preview) = self.preview_cache.get(&entry.id) else {
            return;
        };

        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        match preview {
            DecodedPreview::Text {
                lines,
                line_count,
                char_count,
            } => {
                let badge = format!(
                    "{}  TEXT · {} line{} · {} chars",
                    Glyph::Clipboard.cell(),
                    line_count,
                    if *line_count == 1 { "" } else { "s" },
                    char_count
                );
                put_trunc(buf, inner.x, inner.y, &badge, inner.width, accent);
                let mut y = inner.y + 2;
                let max_y = inner.bottom().saturating_sub(2);
                let w = usize::from(inner.width.max(1));
                'outer: for line in lines {
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
                put_trunc(
                    buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    "Enter copies back to clipboard · Del removes entry",
                    inner.width,
                    dim,
                );
            }
            DecodedPreview::Link { url, host } => {
                let badge = format!("{}  LINK · {host}", Glyph::Link.cell());
                put_trunc(buf, inner.x, inner.y, &badge, inner.width, accent);
                let max_y = inner.bottom().saturating_sub(2);
                let w = usize::from(inner.width.max(1));
                let chars: Vec<char> = url.chars().collect();
                for (y, chunk) in (inner.y + 2..).zip(chars.chunks(w)) {
                    if y >= max_y {
                        break;
                    }
                    let s: String = chunk.iter().collect();
                    put_trunc(buf, inner.x, y, &s, inner.width, body);
                }
                put_trunc(
                    buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    "Enter copies URL · Ctrl+O opens in browser · Del removes",
                    inner.width,
                    dim,
                );
            }
            DecodedPreview::Image { meta } => {
                let badge = format!("{}  IMAGE", Glyph::Image.cell());
                put_trunc(buf, inner.x, inner.y, &badge, inner.width, accent);
                let frame_w = inner.width.clamp(20, 36);
                let frame_h = 7u16.min(inner.height.saturating_sub(5));
                if frame_h >= 5 {
                    let fx = inner.x + inner.width.saturating_sub(frame_w) / 2;
                    let fy = inner.y + 2;
                    let b_style = Style::default().fg(design.color(ColorRole::Border));
                    put(buf, fx, fy, "╭", 1, b_style);
                    put(buf, fx + frame_w - 1, fy, "╮", 1, b_style);
                    put(buf, fx, fy + frame_h - 1, "╰", 1, b_style);
                    put(buf, fx + frame_w - 1, fy + frame_h - 1, "╯", 1, b_style);
                    for x in (fx + 1)..(fx + frame_w - 1) {
                        put(buf, x, fy, "─", 1, b_style);
                        put(buf, x, fy + frame_h - 1, "─", 1, b_style);
                    }
                    for y in (fy + 1)..(fy + frame_h - 1) {
                        put(buf, fx, y, "│", 1, b_style);
                        put(buf, fx + frame_w - 1, y, "│", 1, b_style);
                    }
                    let label = if meta.is_empty() {
                        "Binary image payload"
                    } else {
                        meta.as_str()
                    };
                    put_trunc(
                        buf,
                        fx + 2,
                        fy + 2,
                        &format!("{}  Image payload", Glyph::Image.cell()),
                        frame_w.saturating_sub(4),
                        body,
                    );
                    put_trunc(buf, fx + 2, fy + 3, label, frame_w.saturating_sub(4), dim);
                }
                put_trunc(
                    buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    "Enter copies image · Ctrl+S saves to ~/Pictures/clipboard",
                    inner.width,
                    dim,
                );
            }
        }
    }

    fn cycle_kind_filter(&mut self, delta: i32) {
        let cur = KindFilter::ALL
            .iter()
            .position(|&k| k == self.kind_filter)
            .unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(KindFilter::ALL.len() as i32) as usize;
        self.kind_filter = KindFilter::ALL[next];
        self.build_rows();
    }

    fn transform_selected(&mut self) -> Outcome {
        let Some(entry) = self.selected_entry() else {
            return Outcome::Consumed;
        };
        if entry.kind == ClipKind::Image {
            self.status_note = Some("Transform applies to text and link entries".into());
            return Outcome::Consumed;
        }
        let raw = match self.preview_cache.get(&entry.id) {
            Some(DecodedPreview::Text { lines, .. }) => lines.join("\n"),
            Some(DecodedPreview::Link { url, .. }) => url.clone(),
            _ => entry.preview.clone(),
        };
        let (label, transformed) = smart_transform(&raw);
        let ok = std::process::Command::new("wl-copy")
            .arg(&transformed)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        self.status_note = Some(if ok {
            format!("{label} → copied to clipboard")
        } else {
            format!("{label} (wl-copy failed)")
        });
        Outcome::Consumed
    }

    fn save_selected_to_note(&mut self) -> Outcome {
        let Some(entry) = self.selected_entry() else {
            return Outcome::Consumed;
        };
        if entry.kind == ClipKind::Image {
            self.status_note = Some("Use Ctrl+S to save images to ~/Pictures/clipboard".into());
            return Outcome::Consumed;
        }
        let raw = match self.preview_cache.get(&entry.id) {
            Some(DecodedPreview::Text { lines, .. }) => lines.join("\n"),
            Some(DecodedPreview::Link { url, .. }) => url.clone(),
            _ => entry.preview.clone(),
        };
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let dir = std::path::PathBuf::from(home).join("Documents/Notes");
        let _ = std::fs::create_dir_all(&dir);
        let inbox = dir.join("Inbox.md");
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let block = format!("\n## {ts}\n\n{raw}\n");
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inbox)
            .and_then(|mut f| std::io::Write::write_all(&mut f, block.as_bytes()));
        self.status_note = Some(match res {
            Ok(()) => "Saved clip to ~/Documents/Notes/Inbox.md".into(),
            Err(e) => format!("Could not save note: {e}"),
        });
        Outcome::Consumed
    }

    fn open_selected_link(&mut self) -> Outcome {
        let Some(entry) = self.selected_entry() else {
            return Outcome::Consumed;
        };
        if entry.kind != ClipKind::Link {
            self.status_note = Some("Selected entry is not a URL".into());
            return Outcome::Consumed;
        }
        let url = match self.preview_cache.get(&entry.id) {
            Some(DecodedPreview::Link { url, .. }) => url.clone(),
            _ => entry.preview.clone(),
        };
        crate::system::spawn_detached_argv("xdg-open", &[&url]);
        Outcome::Done(hayato_ui::app::exit::OK)
    }

    fn perform_wipe(&mut self) {
        let count = self.entries.len();
        if !self.static_mode {
            let _ = std::process::Command::new("cliphist")
                .arg("wipe")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        } else {
            self.entries.clear();
        }
        self.status_note = Some(format!("Cleared clipboard history ({count} entries)"));
        self.reload();
    }

    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "share" => {
                if let Some(row) = self.list.selected_index()
                    && let Some(&entry_idx) = self.visible_indices.get(row)
                    && let Some(entry) = self.entries.get(entry_idx)
                {
                    let payload = if entry.kind == ClipKind::Image {
                        let runtime = std::env::var("XDG_RUNTIME_DIR")
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(|_| std::env::temp_dir());
                        let tmp =
                            runtime.join(format!("hayato-clip-share-{}.png", std::process::id()));
                        let ok = std::process::Command::new("cliphist")
                            .args(["decode", &entry.id])
                            .output()
                            .ok()
                            .filter(|o| o.status.success() && !o.stdout.is_empty())
                            .and_then(|o| std::fs::write(&tmp, &o.stdout).ok())
                            .is_some();
                        if ok {
                            Some(crate::system::share::SharePayload::Image {
                                path: tmp,
                                ephemeral: true,
                                source: crate::system::share::ShareSource::Clipboard,
                            })
                        } else {
                            None
                        }
                    } else {
                        std::process::Command::new("cliphist")
                            .args(["decode", &entry.id])
                            .output()
                            .ok()
                            .filter(|o| o.status.success())
                            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                            .filter(|s| !s.trim().is_empty())
                            .map(|text| {
                                crate::system::share::SharePayload::from_text_or_url(
                                    text,
                                    Some("Clipboard".to_string()),
                                    crate::system::share::ShareSource::Clipboard,
                                )
                            })
                    };
                    if let Some(p) = payload
                        && crate::system::share::spawn_detached(&p).is_ok()
                    {
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    self.status_note = Some("Could not share selected entry".to_string());
                    return Outcome::Consumed;
                }
                self.status_note = Some("Select a clipboard entry to share".to_string());
                Outcome::Consumed
            }
            "save" => {
                let img_entry = self.entries.iter().find(|e| e.kind == ClipKind::Image);
                if let Some(entry) = img_entry {
                    let script = "tmp=$(mktemp \"${XDG_RUNTIME_DIR:-/tmp}/clipimg.XXXXXX\"); \
                        cliphist decode \"$1\" > \"$tmp\" 2>/dev/null; \
                        ext=$(file --brief --mime-type \"$tmp\" 2>/dev/null | sed -e 's|.*/||' -e 's|jpeg|jpg|' -e 's|[^a-z0-9]||g'); \
                        [ -n \"$ext\" ] || ext=png; \
                        dir=\"$HOME/Pictures/clipboard\"; \
                        mkdir -p \"$dir\"; \
                        dest=\"$dir/clip-$(date +%Y%m%d-%H%M%S).$ext\"; \
                        mv -f \"$tmp\" \"$dest\"; \
                        exec xdg-open \"$dest\"";
                    crate::system::spawn_detached_argv("sh", &["-c", script, "sh", &entry.id]);
                    Outcome::Done(hayato_ui::app::exit::OK)
                } else {
                    self.status_note = Some("No image in clipboard history".to_string());
                    Outcome::Consumed
                }
            }
            "wipe" => {
                if self.entries.is_empty() {
                    self.status_note = Some("Clipboard history is already empty".to_string());
                    return Outcome::Consumed;
                }
                let confirm = Confirm::new(
                    "Clear clipboard history?",
                    format!("Remove all {} saved clipboard entries?", self.entries.len()),
                )
                .with_detail("This cannot be undone.")
                .with_options(&["Cancel", "Clear all"])
                .destructive(true);
                self.mode = ClipMode::ConfirmWipe(confirm);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    fn activate(&mut self, row_index: usize) -> Outcome {
        let Some(&entry_idx) = self.visible_indices.get(row_index) else {
            return Outcome::Continue;
        };
        let Some(entry) = self.entries.get(entry_idx) else {
            return Outcome::Continue;
        };
        let copied = std::process::Command::new("sh")
            .args(["-c", "cliphist decode \"$1\" | wl-copy", "--", &entry.id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if copied {
            Outcome::Done(hayato_ui::app::exit::OK)
        } else {
            self.status_note = Some("Could not copy that entry back".to_string());
            Outcome::Consumed
        }
    }
}

fn classify_preview(preview: &str) -> ClipKind {
    if preview.starts_with("[[ binary data") {
        ClipKind::Image
    } else if preview.starts_with("http://") || preview.starts_with("https://") {
        ClipKind::Link
    } else {
        ClipKind::Text
    }
}

fn decode_text(id: &str) -> Option<String> {
    out(&["cliphist", "decode", id])
}

impl Screen for ClipScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let ClipMode::ConfirmWipe(confirm) = &self.mode {
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }

        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("History"), true);
        let (tabs, strip, filter, rows) = Self::left_layout(left);
        self.draw_kind_tabs(ctx.buf, tabs, ctx.design);
        self.strip.render(strip, ctx.buf, ctx.design);
        let msg = if let Some(note) = &self.status_note {
            note.as_str()
        } else if self.entries.is_empty() {
            "Clipboard history is empty · Esc backs out"
        } else {
            "Enter copies back · Del deletes · Tab filters kind"
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.visible_indices.len(),
        );
        self.rows_height.set(rows.height);
        if self.entries.is_empty() {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                "Nothing has been copied yet",
                "Copied text and images appear here",
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

        if let ClipMode::ConfirmWipe(confirm) = &mut self.mode {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.mode = ClipMode::Browse;
                    self.perform_wipe();
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.mode = ClipMode::Browse;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }

        if key.code == KeyCode::Tab {
            self.cycle_kind_filter(1);
            return Outcome::Consumed;
        }
        if key.code == KeyCode::BackTab {
            self.cycle_kind_filter(-1);
            return Outcome::Consumed;
        }

        if key.code == KeyCode::Right && self.list.filter().is_empty() {
            self.cycle_kind_filter(1);
            return Outcome::Consumed;
        }
        if key.code == KeyCode::Left && self.list.filter().is_empty() {
            self.cycle_kind_filter(-1);
            return Outcome::Consumed;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    return self.run_strip_action("share");
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    return self.run_strip_action("save");
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    return self.run_strip_action("wipe");
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    return self.open_selected_link();
                }
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    return self.transform_selected();
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    return self.save_selected_to_note();
                }
                _ => {}
            }
        }
        let rows_height = self.rows_height.get();
        let is_delete = key.code == KeyCode::Delete
            || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL));
        if is_delete {
            if let Some(row_idx) = self.list.selected_index()
                && let Some(&entry_idx) = self.visible_indices.get(row_idx)
                && entry_idx < self.entries.len()
            {
                let deleted = std::process::Command::new("sh")
                    .args([
                        "-c",
                        "printf '%s\\n' \"$1\" | cliphist delete",
                        "--",
                        &self.entries[entry_idx].id,
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if deleted {
                    self.status_note = Some("Entry deleted from clipboard history".to_string());
                    self.reload();
                } else {
                    self.status_note = Some("Could not delete that entry".to_string());
                }
                return Outcome::Consumed;
            }
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    self.ensure_selected_preview();
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
                self.ensure_selected_preview();
                return Outcome::Consumed;
            }
            _ => {}
        }
        let action = self.list.handle(key, rows_height, design);
        match action {
            ListAction::Activated(index) => self.activate(index),
            ListAction::Moved => {
                self.ensure_selected_preview();
                Outcome::Continue
            }
            ListAction::Ignored => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let ClipMode::ConfirmWipe(confirm) = &mut self.mode {
            if let Some(idx) = row {
                match confirm.click(idx) {
                    ConfirmAction::Chosen(1) => {
                        self.mode = ClipMode::Browse;
                        self.perform_wipe();
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = ClipMode::Browse;
                    }
                    ConfirmAction::None => {}
                }
            }
            return Outcome::Consumed;
        }
        if let Some(r) = row {
            if r >= HIT_KIND_BASE {
                let idx = r - HIT_KIND_BASE;
                if let Some(&kind) = KindFilter::ALL.get(idx) {
                    self.kind_filter = kind;
                    self.build_rows();
                }
                return Outcome::Consumed;
            }
            if r >= HIT_STRIP_BASE {
                let strip_idx = r - HIT_STRIP_BASE;
                if let Some(item) = self.strip.items().get(strip_idx) {
                    return self.run_strip_action(item.id);
                }
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(r);
            self.ensure_selected_preview();
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if let ClipMode::ConfirmWipe(confirm) = &self.mode {
            return confirm.hit(area, design, x, y);
        }
        let hits = self.kind_hits.borrow();
        for (idx, r) in hits.iter().enumerate() {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(HIT_KIND_BASE + idx);
            }
        }
        let (left, _) = Self::split_panes(area);
        let (_, strip, _, rows) = Self::left_layout(left);
        if let Some(idx) = self.strip.hit_index(strip, x, y) {
            return Some(HIT_STRIP_BASE + idx);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.mode {
            ClipMode::Browse => Some(&self.keybar),
            ClipMode::ConfirmWipe(_) => None,
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            ClipMode::Browse => "clip.browse",
            ClipMode::ConfirmWipe(_) => "clip.confirm_wipe",
        })
    }
}

/// `hayato menu clip` (`hayato-clip`): Clipboard Inspector Studio.
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
            classify_preview("[[ binary data 1234 png 640x480 ]]"),
            ClipKind::Image
        );
        assert_eq!(classify_preview("http://example.com"), ClipKind::Link);
        assert_eq!(classify_preview("https://hayato.dev"), ClipKind::Link);
        assert_eq!(classify_preview("plain text"), ClipKind::Text);
    }

    #[test]
    fn clip_studio_filters_by_kind_and_guards_wipe_with_confirm() {
        let design = Design::solarized();
        let mut screen = ClipScreen::from_entries_for_test(vec![
            ("1", "hello world\nline two"),
            ("2", "https://hayato.dev/docs"),
            ("3", "[[ binary data 24 KiB png 800x600 ]]"),
        ]);
        assert_eq!(screen.phase(), Some("clip.browse"));
        assert_eq!(screen.visible_indices.len(), 3);
        assert!(screen.preview_cache.contains_key("1"));

        // Tab cycles All -> Text (1 entry).
        let _ = screen.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &design);
        assert_eq!(screen.kind_filter, KindFilter::Text);
        assert_eq!(screen.visible_indices.len(), 1);

        // Tab cycles Text -> Link (1 entry).
        let _ = screen.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &design);
        assert_eq!(screen.kind_filter, KindFilter::Link);
        assert_eq!(screen.visible_indices.len(), 1);

        // Ctrl+X opens body-replacing ConfirmWipe modal instead of wiping immediately.
        let _ = screen.key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &design,
        );
        assert_eq!(screen.phase(), Some("clip.confirm_wipe"));
        assert_eq!(screen.entries.len(), 3);

        // Esc cancels wipe without deleting entries.
        let _ = screen.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &design);
        assert_eq!(screen.phase(), Some("clip.browse"));
        assert_eq!(screen.entries.len(), 3);
    }

    #[test]
    fn smart_clipboard_transforms_clean_urls_json_and_ansi() {
        assert_eq!(
            clean_url("https://hayato.dev/page?utm_source=x&id=42&fbclid=abc#top"),
            "https://hayato.dev/page?id=42#top"
        );
        assert_eq!(
            clean_url("https://hayato.dev/page?utm_medium=social"),
            "https://hayato.dev/page"
        );
        assert_eq!(strip_ansi("\x1b[31merror\x1b[0m: failed"), "error: failed");
        let (lbl, pretty) = smart_transform("{\"a\":1,\"b\":[2]}");
        assert_eq!(lbl, "Formatted JSON");
        assert!(pretty.contains('\n'));
    }
}
