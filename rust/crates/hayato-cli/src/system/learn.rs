#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
/// Learn: Interactive Field Manual & Terminal Encyclopedia (`hayato-learn`).
///
/// Renders the Hayato manual chapters (`/usr/share/doc/hayato/*.md`) alongside
/// `ripgrep` full-text search and `apropos` / `man` system pages inside a
/// responsive dual-pane Ratatui workspace (`width >= 84`) with single-pane
/// focus toggle on narrower terminals (`80x24`).
use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, Border, ColorRole, Design, Glyph, Header, Input, KeyHint, ListAction, ListState,
    Outcome, Row, Screen, TextStyle, draw_card_with, draw_filter_bar, draw_gauge_with,
    draw_scrollbar,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::system::manual::{DocSpan, parse_ansi};
use crate::system::{have, missing};

const DOCS_DIR: &str = "/usr/share/doc/hayato";
const PAGE_WRAP_WIDTH: usize = 88;
const DUAL_PANE_MIN_WIDTH: u16 = super::DUAL_PANE_MIN_WIDTH;

const HIT_TAB_BASE: usize = 10_000;
const HIT_READER_PANE: usize = 20_000;
const HIT_READER_PREV_SEC: usize = 20_001;
const HIT_READER_NEXT_SEC: usize = 20_002;

/// A manual chapter entry discovered in `/usr/share/doc/hayato`.
#[derive(Clone, Debug)]
pub struct ManualPage {
    pub path: PathBuf,
    pub chapter: String,
    pub title: String,
    pub summary: String,
    pub read_minutes: u16,
    pub word_count: usize,
}

/// A navigable section heading inside a rendered document (`##` in Markdown or
/// top-level ALL-CAPS sections in `man` pages).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocSection {
    pub line: usize,
    pub title: String,
}

/// A rendered document cached by path (or synthetic key for man pages) so
/// scrolling and cursor hovering never re-invoke `glow` or re-parse Markdown.
#[derive(Clone, Debug)]
pub struct CachedDoc {
    pub title: String,
    pub chapter: Option<String>,
    pub read_minutes: Option<u16>,
    pub lines: Vec<Vec<DocSpan>>,
    pub sections: Vec<DocSection>,
}

/// Result from manual search (`rg`) or system manual lookup (`apropos`).
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub detail: String,
    pub action: SearchAction,
}

/// Action to take when activating a search result.
#[derive(Clone, Debug)]
pub enum SearchAction {
    OpenPage(PathBuf),
    OpenManPage(String, String),
}

/// Legacy mode enum kept for public API compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnMode {
    List,
    Page,
    Search,
    Apropos,
}

/// Active sidebar tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarTab {
    Manual,
    Search,
    Man,
}

impl SidebarTab {
    fn index(self) -> usize {
        match self {
            SidebarTab::Manual => 0,
            SidebarTab::Search => 1,
            SidebarTab::Man => 2,
        }
    }

    fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(SidebarTab::Manual),
            1 => Some(SidebarTab::Search),
            2 => Some(SidebarTab::Man),
            _ => None,
        }
    }
}

/// Active pane focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPane {
    Sidebar,
    Reader,
}

struct LayoutRects {
    dual_pane: bool,
    sidebar: Option<Rect>,
    reader: Option<Rect>,
}

struct SidebarRects {
    tabs: [Rect; 3],
    divider_y: Option<u16>,
    filter: Rect,
    rows: Rect,
}

struct ReaderRects {
    banner_title: Option<Rect>,
    banner_progress: Option<Rect>,
    divider_y: Option<u16>,
    prev_btn: Option<Rect>,
    next_btn: Option<Rect>,
    content: Rect,
}

/// Interactive Field Manual & Terminal Encyclopedia screen.
pub struct LearnScreen {
    tab: SidebarTab,
    focus: FocusPane,
    filtering: bool,
    input: Input,
    pages: Vec<ManualPage>,
    filtered: Vec<ManualPage>,
    list: ListState,
    header: Header,
    rows_height: Cell<u16>,
    reader_height: Cell<u16>,
    last_dual_pane: Cell<bool>,
    reduced_motion: Cell<bool>,
    pulse_phase: u8,
    doc_cache: HashMap<PathBuf, CachedDoc>,
    active_doc: Option<CachedDoc>,
    active_path: Option<PathBuf>,
    content_scroll: Cell<u16>,
    search_input: Input,
    search_results: Vec<SearchResult>,
    search_list: ListState,
    sidebar_keyhints: Vec<KeyHint>,
    filter_keyhints: Vec<KeyHint>,
    search_keyhints: Vec<KeyHint>,
    reader_keyhints: Vec<KeyHint>,
}

impl Default for LearnScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl LearnScreen {
    pub fn new() -> Self {
        let pages = Self::load_pages();
        let mut screen = Self::with_pages(pages);
        screen.preview_selected_chapter();
        screen
    }

    /// Construct a `LearnScreen` from pre-loaded chapters (used by `new()` and tests).
    pub fn with_pages(pages: Vec<ManualPage>) -> Self {
        let list = ListState::new()
            .with_empty("no manual pages found")
            .ranked();
        let mut screen = Self {
            tab: SidebarTab::Manual,
            focus: FocusPane::Sidebar,
            filtering: false,
            input: Input::new(),
            pages: pages.clone(),
            filtered: pages,
            list,
            header: Header::new("Hayato").subtitle("Learn · Field Manual"),
            rows_height: Cell::new(20),
            reader_height: Cell::new(20),
            last_dual_pane: Cell::new(false),
            reduced_motion: Cell::new(false),
            pulse_phase: 0,
            doc_cache: HashMap::new(),
            active_doc: None,
            active_path: None,
            content_scroll: Cell::new(0),
            search_input: Input::new(),
            search_results: Vec::new(),
            search_list: ListState::new()
                .with_empty("type a query above to search")
                .ranked(),
            sidebar_keyhints: vec![
                KeyHint::new("1/2/3", "mode"),
                KeyHint::new("/", "filter"),
                KeyHint::new("Enter/Tab", "read"),
                KeyHint::new("[]", "section"),
                KeyHint::new("Esc", "close"),
            ],
            filter_keyhints: vec![
                KeyHint::new("type", "filter"),
                KeyHint::new("\u{2191}\u{2193}", "select"),
                KeyHint::new("Enter", "read"),
                KeyHint::new("Esc", "clear"),
            ],
            search_keyhints: vec![
                KeyHint::new("type", "query"),
                KeyHint::new("\u{2191}\u{2193}", "select"),
                KeyHint::new("Enter", "open"),
                KeyHint::new("Tab", "reader"),
                KeyHint::new("Esc", "manual"),
            ],
            reader_keyhints: vec![
                KeyHint::new("j/k", "scroll"),
                KeyHint::new("Space", "page"),
                KeyHint::new("[]", "section"),
                KeyHint::new("Tab", "sidebar"),
                KeyHint::new("Esc", "back"),
            ],
        };
        screen.filter_pages();
        screen
    }

    fn load_pages() -> Vec<ManualPage> {
        let mut raw_paths = Vec::new();
        if let Ok(entries) = fs::read_dir(DOCS_DIR) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    raw_paths.push(path);
                }
            }
        }
        raw_paths.sort();

        let mut pages = Vec::new();
        for (idx, path) in raw_paths.into_iter().enumerate() {
            if let Ok(content) = fs::read_to_string(&path) {
                let chapter = extract_chapter_number(&path, idx + 1);
                let title = content
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("# "))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Manual")
                    })
                    .to_string();
                let summary = content
                    .lines()
                    .skip(1)
                    .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                    .map(|l| {
                        l.trim()
                            .replace("**", "")
                            .replace('`', "")
                            .chars()
                            .take(72)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let word_count = content.split_whitespace().count();
                let read_minutes = estimate_read_minutes(&content);
                pages.push(ManualPage {
                    path,
                    chapter,
                    title,
                    summary,
                    read_minutes,
                    word_count,
                });
            }
        }
        pages
    }

    fn page_row(page: &ManualPage) -> Row {
        let label = format!("{} · {}", page.chapter, page.title);
        let read_time = format!("~{}m", page.read_minutes.max(1));
        let mut row = Row::new(label).glyph(Glyph::Book).value(read_time);
        if !page.summary.is_empty() {
            row = row.detail(&page.summary);
        }
        row
    }

    fn search_row(result: &SearchResult, glyph: Glyph) -> Row {
        Row::new(&result.title).glyph(glyph).detail(&result.detail)
    }

    fn filter_pages(&mut self) {
        let query = self.input.text().trim().to_lowercase();
        if query.is_empty() {
            self.filtered = self.pages.clone();
        } else {
            self.filtered = self
                .pages
                .iter()
                .filter(|p| {
                    p.chapter.to_lowercase().contains(&query)
                        || p.title.to_lowercase().contains(&query)
                        || p.summary.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }

        let rows: Vec<Row> = self.filtered.iter().map(Self::page_row).collect();
        self.list.set_rows(rows);
    }

    fn switch_tab(&mut self, tab: SidebarTab) {
        if self.tab == tab {
            self.focus = FocusPane::Sidebar;
            return;
        }
        let prev = self.tab;
        self.tab = tab;
        self.focus = FocusPane::Sidebar;
        self.filtering = false;
        match tab {
            SidebarTab::Manual => {
                self.preview_selected_chapter();
            }
            SidebarTab::Search => {
                if prev == SidebarTab::Manual && self.search_input.text().is_empty() {
                    let seed = self.input.text().trim().to_string();
                    if !seed.is_empty() {
                        self.search_input.set(seed);
                    }
                }
                self.refresh_search_manuals();
            }
            SidebarTab::Man => {
                if prev == SidebarTab::Manual && self.search_input.text().is_empty() {
                    let seed = self.input.text().trim().to_string();
                    if !seed.is_empty() {
                        self.search_input.set(seed);
                    }
                }
                self.refresh_apropos();
            }
        }
    }

    fn preview_selected_chapter(&mut self) {
        if self.tab != SidebarTab::Manual {
            return;
        }
        let Some(index) = self.list.selected_index() else {
            return;
        };
        let Some(page) = self.filtered.get(index).cloned() else {
            return;
        };
        if self.active_path.as_ref() == Some(&page.path) && self.active_doc.is_some() {
            return;
        }
        if let Some(doc) = self.load_or_cache_page(&page.path, Some(&page)) {
            self.active_path = Some(page.path);
            self.active_doc = Some(doc);
            self.content_scroll.set(0);
        }
    }

    fn load_or_cache_page(&mut self, path: &Path, meta: Option<&ManualPage>) -> Option<CachedDoc> {
        if let Some(cached) = self.doc_cache.get(path) {
            return Some(cached.clone());
        }
        let content = fs::read_to_string(path).ok()?;
        let doc = Self::build_markdown_doc(Some(path), &content, meta);
        self.doc_cache.insert(path.to_path_buf(), doc.clone());
        Some(doc)
    }

    fn build_markdown_doc(
        path: Option<&Path>,
        content: &str,
        meta: Option<&ManualPage>,
    ) -> CachedDoc {
        let title = meta
            .map(|m| m.title.clone())
            .or_else(|| {
                content
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("# "))
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_else(|| "Learn".to_string());
        let chapter = meta.map(|m| m.chapter.clone());
        let read_minutes = Some(
            meta.map(|m| m.read_minutes)
                .unwrap_or_else(|| estimate_read_minutes(content))
                .max(1),
        );
        let lines = Self::render_markdown_styled(path, content);
        let raw_headings = extract_markdown_sections(content);
        let sections = map_sections_to_lines(&raw_headings, &lines);
        CachedDoc {
            title,
            chapter,
            read_minutes,
            lines,
            sections,
        }
    }

    fn open_page(&mut self, path: &Path) {
        let meta = self.pages.iter().find(|p| p.path == path).cloned();
        if let Some(doc) = self.load_or_cache_page(path, meta.as_ref()) {
            self.active_path = Some(path.to_path_buf());
            self.active_doc = Some(doc);
            self.content_scroll.set(0);
            self.focus = FocusPane::Reader;
        }
    }

    fn open_man_page(&mut self, page: &str, section: &str) {
        let cache_key = PathBuf::from(format!("man://{page}({section})"));
        if let Some(cached) = self.doc_cache.get(&cache_key).cloned() {
            self.active_path = Some(cache_key);
            self.active_doc = Some(cached);
            self.content_scroll.set(0);
            self.focus = FocusPane::Reader;
            return;
        }

        let output = std::process::Command::new("man")
            .env("MANWIDTH", PAGE_WRAP_WIDTH.to_string())
            .env("MAN_KEEP_FORMATTING", "0")
            .args([section, page])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout).to_string();
                let rendered = Self::render_man(&content);
                let sections = extract_man_sections(&rendered);
                let lines = rendered
                    .into_iter()
                    .map(|l| {
                        let style = if is_man_section_heading(&l) {
                            TextStyle::Title
                        } else {
                            TextStyle::Body
                        };
                        vec![DocSpan { text: l, style }]
                    })
                    .collect();
                let doc = CachedDoc {
                    title: format!("{page}({section})"),
                    chapter: Some(format!("man({section})")),
                    read_minutes: Some(estimate_read_minutes(&content).max(1)),
                    lines,
                    sections,
                };
                self.doc_cache.insert(cache_key.clone(), doc.clone());
                self.active_path = Some(cache_key);
                self.active_doc = Some(doc);
                self.content_scroll.set(0);
                self.focus = FocusPane::Reader;
            }
        }
    }

    fn refresh_search_manuals(&mut self) {
        let query = self.search_input.text().trim();
        let mut results = Vec::new();
        if !query.is_empty() {
            let output = std::process::Command::new("rg")
                .args([
                    "-n",
                    "-i",
                    "--no-heading",
                    "--with-filename",
                    "--max-count",
                    "8",
                    query,
                    DOCS_DIR,
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();

            if let Ok(output) = output {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let parts: Vec<&str> = line.splitn(3, ':').collect();
                        if parts.len() == 3 {
                            let file = parts[0];
                            let line_num = parts[1];
                            let text = parts[2].trim();
                            let title = Path::new(file)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or(file);
                            let clean_text = text
                                .chars()
                                .filter(|c| !matches!(c, '*' | '_' | '`' | '#'))
                                .take(70)
                                .collect::<String>();
                            results.push(SearchResult {
                                title: format!("{title} · L{line_num}"),
                                detail: clean_text,
                                action: SearchAction::OpenPage(PathBuf::from(file)),
                            });
                        }
                    }
                }
            }
        }

        self.search_results = results;
        let rows = self
            .search_results
            .iter()
            .map(|r| Self::search_row(r, Glyph::Search))
            .collect();
        self.search_list.set_rows(rows);
    }

    fn refresh_apropos(&mut self) {
        let query = self.search_input.text().trim();
        let mut results = Vec::new();
        if !query.is_empty() {
            let output = std::process::Command::new("apropos")
                .args(["--", query])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();

            if let Ok(output) = output {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines().take(60) {
                        if let Some(dash_pos) = line.find(" - ") {
                            let left = &line[..dash_pos];
                            let desc = &line[dash_pos + 3..];
                            if let Some(paren) = left.rfind('(') {
                                let name = left[..paren].trim();
                                let section = left[paren + 1..].trim_end_matches(')').trim();
                                if !name.is_empty() && !section.is_empty() {
                                    results.push(SearchResult {
                                        title: format!("{name}({section})"),
                                        detail: desc.to_string(),
                                        action: SearchAction::OpenManPage(
                                            name.to_string(),
                                            section.to_string(),
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        self.search_results = results;
        let rows = self
            .search_results
            .iter()
            .map(|r| Self::search_row(r, Glyph::Terminal))
            .collect();
        self.search_list.set_rows(rows);
    }

    fn activate_list_row(&mut self, index: usize) {
        if let Some(page) = self.filtered.get(index).cloned() {
            self.open_page(&page.path);
        }
    }

    fn activate_search_row(&mut self, index: usize) {
        if let Some(result) = self.search_results.get(index).cloned() {
            match &result.action {
                SearchAction::OpenPage(path) => self.open_page(path),
                SearchAction::OpenManPage(page, section) => self.open_man_page(page, section),
            }
        }
    }

    fn max_scroll(&self) -> usize {
        let total = self.active_doc.as_ref().map_or(0, |d| d.lines.len());
        let viewport = usize::from(self.reader_height.get().max(1));
        total.saturating_sub(viewport)
    }

    fn jump_next_section(&mut self) {
        let Some(doc) = &self.active_doc else {
            return;
        };
        let scroll = usize::from(self.content_scroll.get());
        if let Some(target) = next_section_line(&doc.sections, scroll) {
            let clamped = target.min(self.max_scroll());
            self.content_scroll.set(clamped as u16);
        }
    }

    fn jump_prev_section(&mut self) {
        let Some(doc) = &self.active_doc else {
            return;
        };
        let scroll = usize::from(self.content_scroll.get());
        if let Some(target) = prev_section_line(&doc.sections, scroll) {
            let clamped = target.min(self.max_scroll());
            self.content_scroll.set(clamped as u16);
        }
    }

    fn render_markdown_styled(path: Option<&Path>, content: &str) -> Vec<Vec<DocSpan>> {
        if let Some(p) = path {
            if have("glow") {
                if let Ok(output) = std::process::Command::new("glow")
                    .args(["-s", "dark", "-w", &PAGE_WRAP_WIDTH.to_string()])
                    .arg(p)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output()
                {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let parsed = parse_ansi(&stdout);
                        if !parsed.is_empty() {
                            return parsed;
                        }
                    }
                }
            }
        }
        Self::render_markdown(content)
            .into_iter()
            .map(|l| {
                let trimmed = l.trim_start();
                let style =
                    if trimmed.starts_with('│') || trimmed.starts_with("┌─") || trimmed == "```"
                    {
                        TextStyle::Code
                    } else if trimmed.starts_with("§ ") || trimmed.starts_with("▸ ") {
                        TextStyle::Title
                    } else {
                        TextStyle::Body
                    };
                vec![DocSpan { text: l, style }]
            })
            .collect()
    }

    fn wrap_plain_line(line: &str, max_width: usize) -> Vec<String> {
        if line.is_empty() || line.chars().count() <= max_width {
            return vec![line.to_string()];
        }
        let indent_len = line.chars().take_while(|c| *c == ' ').count().min(8);
        let indent = " ".repeat(indent_len);
        let mut out = Vec::new();
        let mut current = String::new();
        for word in line.split_whitespace() {
            if current.is_empty() {
                current.push_str(&indent);
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(std::mem::take(&mut current));
                current.push_str(&indent);
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }

    pub fn render_markdown(content: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut in_code = false;
        let mut code_lang: String;

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with("```") {
                if in_code {
                    in_code = false;
                    lines.push("```".to_string());
                } else {
                    in_code = true;
                    code_lang = trimmed[3..].trim().to_string();
                    lines.push(format!("┌─ {} ────────────────────────", code_lang));
                }
                continue;
            }

            if in_code {
                lines.push(format!("│ {}", line));
                continue;
            }

            let clean = line.replace("**", "").replace('`', "");
            let clean_trimmed = clean.trim();

            if clean_trimmed.starts_with("### ") {
                lines.push(format!("  ▸ {}", &clean_trimmed[4..]));
            } else if clean_trimmed.starts_with("## ") {
                lines.push(format!("§ {}", &clean_trimmed[3..]));
            } else if clean_trimmed.starts_with("# ") {
                lines.push(clean_trimmed[2..].to_string());
            } else if clean_trimmed.starts_with("- ") || clean_trimmed.starts_with("* ") {
                for wrapped in
                    Self::wrap_plain_line(&format!("  • {}", &clean_trimmed[2..]), PAGE_WRAP_WIDTH)
                {
                    lines.push(wrapped);
                }
            } else if clean_trimmed.len() > 2
                && clean_trimmed.chars().nth(1) == Some('.')
                && clean_trimmed.chars().next().unwrap().is_ascii_digit()
            {
                for wrapped in
                    Self::wrap_plain_line(&format!("  {}", clean_trimmed), PAGE_WRAP_WIDTH)
                {
                    lines.push(wrapped);
                }
            } else if clean_trimmed.starts_with('|') && clean_trimmed.ends_with('|') {
                lines.push(clean);
            } else if !clean_trimmed.is_empty() {
                for wrapped in Self::wrap_plain_line(&clean, PAGE_WRAP_WIDTH) {
                    lines.push(wrapped);
                }
            } else {
                lines.push(String::new());
            }
        }

        lines
    }

    fn strip_overstrike(line: &str) -> String {
        let mut out: Vec<char> = Vec::new();
        for ch in line.chars() {
            if ch == '\x08' {
                out.pop();
            } else {
                out.push(ch);
            }
        }
        out.into_iter().collect()
    }

    fn render_man(content: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in content.lines() {
            let clean = Self::strip_overstrike(line);
            for wrapped in Self::wrap_plain_line(&clean, PAGE_WRAP_WIDTH) {
                out.push(wrapped);
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Layout & Drawing
    // -----------------------------------------------------------------------

    fn compute_layout(&self, area: Rect) -> LayoutRects {
        if area.width >= DUAL_PANE_MIN_WIDTH {
            let sidebar_w = ((u32::from(area.width) * 34) / 100) as u16;
            let sidebar_w = sidebar_w.clamp(30, 44).min(area.width.saturating_sub(24));
            let gap = 1u16;
            let sidebar = Rect {
                x: area.x,
                y: area.y,
                width: sidebar_w,
                height: area.height,
            };
            let reader_x = area.x.saturating_add(sidebar_w).saturating_add(gap);
            let reader = Rect {
                x: reader_x,
                y: area.y,
                width: area.right().saturating_sub(reader_x),
                height: area.height,
            };
            LayoutRects {
                dual_pane: true,
                sidebar: Some(sidebar),
                reader: Some(reader),
            }
        } else {
            match self.focus {
                FocusPane::Sidebar => LayoutRects {
                    dual_pane: false,
                    sidebar: Some(area),
                    reader: None,
                },
                FocusPane::Reader => LayoutRects {
                    dual_pane: false,
                    sidebar: None,
                    reader: Some(area),
                },
            }
        }
    }

    fn card_inner(area: Rect, design: &Design) -> Rect {
        design.block(Border::Rounded).inner(area)
    }

    fn tab_labels(width: u16) -> [&'static str; 3] {
        if width >= 35 {
            ["[1] Manual", "[2] Search", "[3] Man"]
        } else if width >= 27 {
            ["[1]Manual", "[2]Search", "[3]Man"]
        } else {
            ["[1]Man", "[2]Rg", "[3]Doc"]
        }
    }

    fn sidebar_layout(card_area: Rect, design: &Design) -> SidebarRects {
        let inner = Self::card_inner(card_area, design);
        let labels = Self::tab_labels(inner.width);
        let mut x = inner.x;
        let mut tabs = [Rect::default(); 3];
        for (i, label) in labels.iter().enumerate() {
            let w =
                (UnicodeWidthStr::width(*label) as u16 + 2).min(inner.right().saturating_sub(x));
            tabs[i] = Rect {
                x,
                y: inner.y,
                width: w,
                height: u16::from(inner.height >= 1),
            };
            x = x.saturating_add(w).saturating_add(1);
        }

        if inner.height >= 7 {
            let divider_y = Some(inner.y + 1);
            let filter = Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: 1,
            };
            let rows = Rect {
                x: inner.x,
                y: inner.y + 4,
                width: inner.width,
                height: inner.height.saturating_sub(4),
            };
            SidebarRects {
                tabs,
                divider_y,
                filter,
                rows,
            }
        } else if inner.height >= 4 {
            let filter = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            let rows = Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: inner.height.saturating_sub(2),
            };
            SidebarRects {
                tabs,
                divider_y: None,
                filter,
                rows,
            }
        } else {
            let filter = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 0,
            };
            let rows = Rect {
                x: inner.x,
                y: inner.y.saturating_add(1),
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };
            SidebarRects {
                tabs,
                divider_y: None,
                filter,
                rows,
            }
        }
    }

    fn reader_layout(card_area: Rect, design: &Design) -> ReaderRects {
        let inner = Self::card_inner(card_area, design);
        if inner.height >= 6 && inner.width >= 20 {
            let banner_title = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            let banner_progress = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            let divider_y = inner.y + 2;
            let content = Rect {
                x: inner.x,
                y: inner.y + 3,
                width: inner.width,
                height: inner.height.saturating_sub(3),
            };
            let next_btn = if inner.width >= 28 {
                Some(Rect {
                    x: inner.right().saturating_sub(3),
                    y: inner.y,
                    width: 3,
                    height: 1,
                })
            } else {
                None
            };
            let prev_btn = if inner.width >= 28 {
                Some(Rect {
                    x: inner.right().saturating_sub(7),
                    y: inner.y,
                    width: 3,
                    height: 1,
                })
            } else {
                None
            };
            ReaderRects {
                banner_title: Some(banner_title),
                banner_progress: Some(banner_progress),
                divider_y: Some(divider_y),
                prev_btn,
                next_btn,
                content,
            }
        } else {
            ReaderRects {
                banner_title: None,
                banner_progress: None,
                divider_y: None,
                prev_btn: None,
                next_btn: None,
                content: inner,
            }
        }
    }

    fn pulse_on(&self, design: &Design) -> bool {
        !design.reduced_motion() && (self.pulse_phase % 2 == 1)
    }

    fn active_border_role(&self, focused: bool, design: &Design) -> ColorRole {
        if !focused {
            ColorRole::Border
        } else if self.pulse_on(design) {
            ColorRole::AccentAlt
        } else {
            ColorRole::Accent
        }
    }

    fn draw_rule(buf: &mut Buffer, x: u16, y: u16, width: u16, design: &Design) {
        let style = Style::default().fg(design.color(ColorRole::Border));
        for col in x..x.saturating_add(width) {
            if let Some(cell) = buf.cell_mut((col, y)) {
                cell.set_symbol("\u{2500}");
                cell.set_style(style);
            }
        }
    }

    fn draw_sidebar(&mut self, buf: &mut Buffer, area: Rect, design: &Design) {
        let focused = self.focus == FocusPane::Sidebar;
        let border_role = self.active_border_role(focused, design);
        let title_style = if focused {
            design.style(TextStyle::Title)
        } else {
            design.style(TextStyle::Dim)
        };
        let card_title = if focused && self.pulse_on(design) {
            "\u{25c7} Field Manual"
        } else {
            "\u{25c8} Field Manual"
        };
        draw_card_with(
            buf,
            area,
            design,
            Some(card_title),
            border_role,
            title_style,
        );

        let rects = Self::sidebar_layout(area, design);
        let labels = Self::tab_labels(Self::card_inner(area, design).width);

        // Mode switcher tabs
        for (i, (rect, label)) in rects.tabs.iter().zip(labels.iter()).enumerate() {
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            let active = self.tab.index() == i;
            if active {
                let pill_style = Style::default()
                    .fg(design.color(ColorRole::KeyText))
                    .bg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD);
                let mut x = put(buf, rect.x, rect.y, " ", rect.width, pill_style);
                x = put_trunc(
                    buf,
                    x,
                    rect.y,
                    label,
                    rect.right().saturating_sub(x).saturating_sub(1),
                    pill_style,
                );
                put(
                    buf,
                    x,
                    rect.y,
                    " ",
                    rect.right().saturating_sub(x),
                    pill_style,
                );
            } else {
                let dim_style = design.style(TextStyle::Dim);
                let x = put(buf, rect.x, rect.y, " ", rect.width, dim_style);
                put_trunc(
                    buf,
                    x,
                    rect.y,
                    label,
                    rect.right().saturating_sub(x),
                    dim_style,
                );
            }
        }

        if let Some(y) = rects.divider_y {
            let inner = Self::card_inner(area, design);
            Self::draw_rule(buf, inner.x, y, inner.width, design);
        }

        // Filter / Query bar & list
        self.rows_height.set(rects.rows.height);
        match self.tab {
            SidebarTab::Manual => {
                let hint = if self.filtering {
                    "type to filter chapters · Esc clear"
                } else {
                    "/ filter chapters · Enter read"
                };
                draw_filter_bar(
                    buf,
                    rects.filter,
                    design,
                    self.input.text(),
                    hint,
                    self.list.len(),
                    self.pages.len(),
                );
                self.list.render(rects.rows, buf, design, focused);
            }
            SidebarTab::Search => {
                draw_filter_bar(
                    buf,
                    rects.filter,
                    design,
                    self.search_input.text(),
                    "rg /usr/share/doc/hayato · Enter open",
                    self.search_list.len(),
                    self.search_results.len(),
                );
                self.search_list.render(rects.rows, buf, design, focused);
            }
            SidebarTab::Man => {
                draw_filter_bar(
                    buf,
                    rects.filter,
                    design,
                    self.search_input.text(),
                    "apropos man pages · Enter open",
                    self.search_list.len(),
                    self.search_results.len(),
                );
                self.search_list.render(rects.rows, buf, design, focused);
            }
        }
    }

    fn draw_reader(&mut self, buf: &mut Buffer, area: Rect, design: &Design) {
        let focused = self.focus == FocusPane::Reader;
        let border_role = self.active_border_role(focused, design);
        let title_style = if focused {
            design.style(TextStyle::Title)
        } else {
            design.style(TextStyle::Dim)
        };

        let card_title = match &self.active_doc {
            Some(doc) => {
                if focused && self.pulse_on(design) {
                    format!("\u{25b9} {}", doc.title)
                } else {
                    format!("\u{25b8} {}", doc.title)
                }
            }
            None => "\u{25b8} Reader".to_string(),
        };
        draw_card_with(
            buf,
            area,
            design,
            Some(&card_title),
            border_role,
            title_style,
        );

        let rects = Self::reader_layout(area, design);
        self.reader_height.set(rects.content.height);

        let Some(doc) = &self.active_doc else {
            self.draw_reader_empty(buf, rects.content, design);
            return;
        };

        let total_lines = doc.lines.len();
        let viewport = usize::from(rects.content.height.max(1));
        let max_scroll = total_lines.saturating_sub(viewport);
        let scroll = usize::from(self.content_scroll.get()).min(max_scroll);
        self.content_scroll.set(scroll as u16);

        let active_sec = current_section(&doc.sections, scroll);

        // Banner Row 0: Chapter badge + current ## section + [ ] section jump controls
        if let Some(row0) = rects.banner_title {
            let mut x = row0.x;
            if let Some(ch) = &doc.chapter {
                let badge_style = Style::default()
                    .fg(design.color(ColorRole::KeyText))
                    .bg(design.color(ColorRole::AccentAlt))
                    .add_modifier(Modifier::BOLD);
                x = put(buf, x, row0.y, " ", 1, badge_style);
                x = put_trunc(buf, x, row0.y, ch, 10, badge_style);
                x = put(buf, x, row0.y, " ", 1, badge_style);
                x = put(buf, x, row0.y, " ", 1, design.style(TextStyle::Body));
            }
            let right_reserve = if rects.prev_btn.is_some() && !doc.sections.is_empty() {
                8u16
            } else {
                0u16
            };
            let budget = row0.right().saturating_sub(x).saturating_sub(right_reserve);
            if let Some((_, sec)) = active_sec {
                x = put(
                    buf,
                    x,
                    row0.y,
                    "\u{00a7} ",
                    budget.min(2),
                    design.style(TextStyle::Code),
                );
                put_trunc(
                    buf,
                    x,
                    row0.y,
                    &sec.title,
                    row0.right().saturating_sub(x).saturating_sub(right_reserve),
                    design.style(TextStyle::Title),
                );
            } else {
                put_trunc(
                    buf,
                    x,
                    row0.y,
                    &doc.title,
                    budget,
                    design.style(TextStyle::Title),
                );
            }

            if !doc.sections.is_empty() {
                if let Some(prev) = rects.prev_btn {
                    put(
                        buf,
                        prev.x,
                        prev.y,
                        "[<]",
                        prev.width,
                        design.style(TextStyle::Code),
                    );
                }
                if let Some(next) = rects.next_btn {
                    put(
                        buf,
                        next.x,
                        next.y,
                        "[>]",
                        next.width,
                        design.style(TextStyle::Code),
                    );
                }
            }
        }

        // Banner Row 1: Reading progress gauge + line counter + section counter
        if let Some(row1) = rects.banner_progress {
            let progress = if total_lines == 0 {
                0.0
            } else if max_scroll == 0 {
                1.0
            } else {
                (scroll as f64 / max_scroll as f64).clamp(0.0, 1.0)
            };

            let mut stats_buf = [0u8; 40];
            let stats = format_reader_stats(
                &mut stats_buf,
                scroll + 1,
                total_lines.max(1),
                active_sec.map(|(i, _)| i),
                doc.sections.len(),
                doc.read_minutes,
            );
            let stats_w = UnicodeWidthStr::width(stats) as u16;
            let gauge_w = row1.width.saturating_sub(stats_w + 2);
            if gauge_w >= 12 {
                let gauge_rect = Rect {
                    x: row1.x,
                    y: row1.y,
                    width: gauge_w,
                    height: 1,
                };
                let gauge_role = if focused {
                    ColorRole::Accent
                } else {
                    ColorRole::AccentAlt
                };
                draw_gauge_with(buf, gauge_rect, design, progress, "", gauge_role);
            }
            if stats_w > 0 && row1.width > stats_w {
                let stats_x = row1.right().saturating_sub(stats_w);
                put(
                    buf,
                    stats_x,
                    row1.y,
                    stats,
                    stats_w,
                    design.style(TextStyle::Dim),
                );
            }
        }

        if let Some(y) = rects.divider_y {
            let inner = Self::card_inner(area, design);
            Self::draw_rule(buf, inner.x, y, inner.width, design);
        }

        // Document lines
        let content = rects.content;
        let max_x = content.x.saturating_add(content.width.saturating_sub(2));
        for (i, spans) in doc.lines.iter().skip(scroll).enumerate() {
            if i as u16 >= content.height {
                break;
            }
            let y = content.y + i as u16;
            let mut x = content.x;
            for span in spans {
                let style = design.style(span.style);
                for ch in span.text.chars() {
                    let w = UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                    if x.saturating_add(w) > max_x {
                        break;
                    }
                    let mut ch_buf = [0u8; 4];
                    buf.set_string(x, y, ch.encode_utf8(&mut ch_buf), style);
                    x = x.saturating_add(w);
                }
            }
        }

        draw_scrollbar(
            buf,
            content,
            design,
            scroll,
            content.height as usize,
            total_lines,
            focused,
        );
    }

    fn draw_reader_empty(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.height < 4 || area.width < 16 {
            return;
        }
        let y0 = area.y + area.height / 4;
        let lines: [(&str, TextStyle); 6] = [
            ("HAYATO // FIELD MANUAL", TextStyle::Title),
            ("", TextStyle::Dim),
            (
                "  [1] Manual   — browse chapters 01..08 with live preview",
                TextStyle::Body,
            ),
            (
                "  [2] Search   — full-text ripgrep across /usr/share/doc/hayato",
                TextStyle::Body,
            ),
            (
                "  [3] Man      — instant apropos & system man pages",
                TextStyle::Body,
            ),
            (
                "  [ ] / Tab    — jump ## sections · switch pane focus",
                TextStyle::Code,
            ),
        ];
        for (idx, (text, style)) in lines.iter().enumerate() {
            let y = y0.saturating_add(idx as u16);
            if y >= area.bottom() {
                break;
            }
            put_trunc(
                buf,
                area.x + 1,
                y,
                text,
                area.width.saturating_sub(2),
                design.style(*style),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Screen trait implementation
// ---------------------------------------------------------------------------

impl Screen for LearnScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        self.reduced_motion.set(ctx.design.reduced_motion());
        let layout = self.compute_layout(ctx.area);
        self.last_dual_pane.set(layout.dual_pane);

        if let Some(sidebar) = layout.sidebar {
            self.draw_sidebar(ctx.buf, sidebar, ctx.design);
        }
        if let Some(reader) = layout.reader {
            self.draw_reader(ctx.buf, reader, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        // Global shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    self.switch_tab(SidebarTab::Search);
                    return Outcome::Consumed;
                }
                KeyCode::Char('m') | KeyCode::Char('M') => {
                    self.switch_tab(SidebarTab::Man);
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.filtering = false;
            self.focus = match self.focus {
                FocusPane::Sidebar => {
                    if self.active_doc.is_none() && self.tab == SidebarTab::Manual {
                        self.preview_selected_chapter();
                    }
                    if self.active_doc.is_some() || self.last_dual_pane.get() {
                        FocusPane::Reader
                    } else {
                        FocusPane::Sidebar
                    }
                }
                FocusPane::Reader => FocusPane::Sidebar,
            };
            return Outcome::Consumed;
        }

        match self.focus {
            FocusPane::Sidebar => self.handle_sidebar_key(key, design),
            FocusPane::Reader => self.handle_reader_key(key),
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(hit) = row else {
            return Outcome::Continue;
        };
        if hit == HIT_READER_PANE {
            self.filtering = false;
            self.focus = FocusPane::Reader;
            return Outcome::Consumed;
        }
        if hit == HIT_READER_PREV_SEC {
            self.focus = FocusPane::Reader;
            self.jump_prev_section();
            return Outcome::Consumed;
        }
        if hit == HIT_READER_NEXT_SEC {
            self.focus = FocusPane::Reader;
            self.jump_next_section();
            return Outcome::Consumed;
        }
        if (HIT_TAB_BASE..HIT_TAB_BASE + 3).contains(&hit) {
            if let Some(tab) = SidebarTab::from_index(hit - HIT_TAB_BASE) {
                self.switch_tab(tab);
            }
            return Outcome::Consumed;
        }

        // Clicked a row inside the sidebar list
        self.focus = FocusPane::Sidebar;
        match self.tab {
            SidebarTab::Manual => {
                let already_selected = self.list.selected_index() == Some(hit);
                self.list.set_cursor(hit);
                self.preview_selected_chapter();
                if already_selected || !self.last_dual_pane.get() {
                    if let Some(index) = self.list.selected_index() {
                        self.activate_list_row(index);
                    }
                }
                Outcome::Consumed
            }
            SidebarTab::Search | SidebarTab::Man => {
                self.search_list.set_cursor(hit);
                if let Some(index) = self.search_list.selected_index() {
                    self.activate_search_row(index);
                }
                Outcome::Consumed
            }
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let layout = self.compute_layout(area);
        if let Some(sidebar) = layout.sidebar {
            if rect_contains(sidebar, x, y) {
                let rects = Self::sidebar_layout(sidebar, design);
                for (i, tab_rect) in rects.tabs.iter().enumerate() {
                    if rect_contains(*tab_rect, x, y) {
                        return Some(HIT_TAB_BASE + i);
                    }
                }
                return match self.tab {
                    SidebarTab::Manual => self.list.hit_row(rects.rows, x, y, design),
                    SidebarTab::Search | SidebarTab::Man => {
                        self.search_list.hit_row(rects.rows, x, y, design)
                    }
                };
            }
        }
        if let Some(reader) = layout.reader {
            if rect_contains(reader, x, y) {
                let rects = Self::reader_layout(reader, design);
                if let Some(prev) = rects.prev_btn {
                    if rect_contains(prev, x, y) {
                        return Some(HIT_READER_PREV_SEC);
                    }
                }
                if let Some(next) = rects.next_btn {
                    if rect_contains(next, x, y) {
                        return Some(HIT_READER_NEXT_SEC);
                    }
                }
                return Some(HIT_READER_PANE);
            }
        }
        None
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.focus {
            FocusPane::Reader => Some(&self.reader_keyhints),
            FocusPane::Sidebar => match self.tab {
                SidebarTab::Manual if self.filtering => Some(&self.filter_keyhints),
                SidebarTab::Manual => Some(&self.sidebar_keyhints),
                SidebarTab::Search | SidebarTab::Man => Some(&self.search_keyhints),
            },
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match (self.tab, self.focus) {
            (SidebarTab::Manual, FocusPane::Sidebar) => "manual-sidebar",
            (SidebarTab::Manual, FocusPane::Reader) => "manual-reader",
            (SidebarTab::Search, FocusPane::Sidebar) => "search-sidebar",
            (SidebarTab::Search, FocusPane::Reader) => "search-reader",
            (SidebarTab::Man, FocusPane::Sidebar) => "man-sidebar",
            (SidebarTab::Man, FocusPane::Reader) => "man-reader",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.reduced_motion.get() {
            None
        } else {
            Some(Duration::from_millis(650))
        }
    }

    fn tick(&mut self) -> Outcome {
        self.pulse_phase = self.pulse_phase.wrapping_add(1);
        Outcome::Continue
    }

    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((1120, 740))
    }
}

impl LearnScreen {
    fn handle_sidebar_key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        let rows_height = self.rows_height.get();
        match self.tab {
            SidebarTab::Manual => {
                if self.filtering {
                    match key.code {
                        KeyCode::Esc => {
                            self.filtering = false;
                            if !self.input.text().is_empty() {
                                self.input.set("");
                                self.filter_pages();
                                self.preview_selected_chapter();
                            }
                            return Outcome::Consumed;
                        }
                        KeyCode::Enter => {
                            self.filtering = false;
                            if let Some(selected) = self.list.selected_index() {
                                self.activate_list_row(selected);
                            }
                            return Outcome::Consumed;
                        }
                        KeyCode::Char(_) | KeyCode::Backspace
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            let _ = self.input.handle(key);
                            self.filter_pages();
                            self.preview_selected_chapter();
                            return Outcome::Consumed;
                        }
                        _ => {}
                    }
                    let outcome = self.list.handle(key, rows_height, design);
                    self.preview_selected_chapter();
                    return match outcome {
                        ListAction::Activated(index) => {
                            self.filtering = false;
                            self.activate_list_row(index);
                            Outcome::Consumed
                        }
                        ListAction::Moved => Outcome::Consumed,
                        _ => Outcome::Continue,
                    };
                }

                // Non-filtering Manual sidebar navigation
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    match key.code {
                        KeyCode::Char('1') => {
                            self.switch_tab(SidebarTab::Manual);
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('2') => {
                            self.switch_tab(SidebarTab::Search);
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('3') => {
                            self.switch_tab(SidebarTab::Man);
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('/') => {
                            self.filtering = true;
                            return Outcome::Consumed;
                        }
                        KeyCode::Char(']') => {
                            self.jump_next_section();
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('[') => {
                            self.jump_prev_section();
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('j') => {
                            let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
                            let _ = self.list.handle(down, rows_height, design);
                            self.preview_selected_chapter();
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('k') => {
                            let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
                            let _ = self.list.handle(up, rows_height, design);
                            self.preview_selected_chapter();
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                            if let Some(selected) = self.list.selected_index() {
                                self.activate_list_row(selected);
                            }
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('q') => {
                            return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                        }
                        KeyCode::Esc => {
                            if !self.input.text().is_empty() {
                                self.input.set("");
                                self.filter_pages();
                                self.preview_selected_chapter();
                                return Outcome::Consumed;
                            }
                            return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                        }
                        _ => {}
                    }
                }

                let action = self.list.handle(key, rows_height, design);
                self.preview_selected_chapter();
                match action {
                    ListAction::Activated(index) => {
                        self.activate_list_row(index);
                        Outcome::Consumed
                    }
                    ListAction::Moved => Outcome::Consumed,
                    _ => Outcome::Continue,
                }
            }
            SidebarTab::Search | SidebarTab::Man => {
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    let _ = self.search_input.handle(key);
                    if self.tab == SidebarTab::Search {
                        self.refresh_search_manuals();
                    } else {
                        self.refresh_apropos();
                    }
                    return Outcome::Consumed;
                }
                match key.code {
                    KeyCode::Enter => {
                        if let Some(selected) = self.search_list.selected_index() {
                            self.activate_search_row(selected);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Esc => {
                        if !self.search_input.text().is_empty() {
                            self.search_input.set("");
                            if self.tab == SidebarTab::Search {
                                self.refresh_search_manuals();
                            } else {
                                self.refresh_apropos();
                            }
                        } else {
                            self.switch_tab(SidebarTab::Manual);
                        }
                        return Outcome::Consumed;
                    }
                    _ => {}
                }
                match self.search_list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => {
                        self.activate_search_row(index);
                        Outcome::Consumed
                    }
                    ListAction::Moved => Outcome::Consumed,
                    _ => Outcome::Continue,
                }
            }
        }
    }

    fn handle_reader_key(&mut self, key: KeyEvent) -> Outcome {
        let page_step = usize::from(self.reader_height.get().saturating_sub(2).max(5));
        let max = self.max_scroll();
        let scroll = usize::from(self.content_scroll.get());

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Left | KeyCode::Char('h') => {
                self.focus = FocusPane::Sidebar;
                Outcome::Consumed
            }
            KeyCode::Char('1') => {
                self.switch_tab(SidebarTab::Manual);
                Outcome::Consumed
            }
            KeyCode::Char('2') => {
                self.switch_tab(SidebarTab::Search);
                Outcome::Consumed
            }
            KeyCode::Char('3') => {
                self.switch_tab(SidebarTab::Man);
                Outcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.content_scroll.set(scroll.saturating_sub(1) as u16);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.content_scroll.set((scroll + 1).min(max) as u16);
                Outcome::Consumed
            }
            KeyCode::PageUp | KeyCode::Char('u') | KeyCode::Char('b') => {
                self.content_scroll
                    .set(scroll.saturating_sub(page_step) as u16);
                Outcome::Consumed
            }
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('d') => {
                self.content_scroll
                    .set((scroll + page_step).min(max) as u16);
                Outcome::Consumed
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.content_scroll.set(0);
                Outcome::Consumed
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.content_scroll.set(max as u16);
                Outcome::Consumed
            }
            KeyCode::Char(']') => {
                self.jump_next_section();
                Outcome::Consumed
            }
            KeyCode::Char('[') => {
                self.jump_prev_section();
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure Helpers (Chapter Numbering, Reading Time, Section Extraction, Formatting)
// ---------------------------------------------------------------------------

fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && x >= rect.x
        && x < rect.right()
        && y >= rect.y
        && y < rect.bottom()
}

/// Extract a 2-digit chapter prefix (`01`..`08`) from a manual filename,
/// falling back to the 1-based ordinal index.
pub fn extract_chapter_number(path: &Path, fallback_index: usize) -> String {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return if digits.len() == 1 {
                format!("0{digits}")
            } else {
                digits
            };
        }
    }
    format!("{fallback_index:02}")
}

/// Estimate reading time in minutes at ~200 words per minute (0 for empty text,
/// minimum 1 minute for any non-empty document).
pub fn estimate_read_minutes(content: &str) -> u16 {
    let words = content.split_whitespace().count();
    if words == 0 {
        return 0;
    }
    (words.saturating_add(199) / 200).max(1) as u16
}

/// Extract ordered `##` section headings from raw Markdown (ignoring code fences).
pub fn extract_markdown_sections(content: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut in_code = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            let clean = rest.trim().replace("**", "").replace('`', "");
            if !clean.is_empty() {
                sections.push(clean);
            }
        }
    }
    sections
}

/// Map extracted section titles onto rendered `DocSpan` line indices in order.
pub fn map_sections_to_lines(headings: &[String], lines: &[Vec<DocSpan>]) -> Vec<DocSection> {
    let mut mapped = Vec::new();
    let mut search_from = 0usize;
    for heading in headings {
        let needle = heading.to_lowercase();
        let mut found = None;
        for (idx, spans) in lines.iter().enumerate().skip(search_from) {
            let text: String = spans.iter().map(|s| s.text.as_str()).collect();
            let normalized = text
                .trim()
                .trim_start_matches('§')
                .trim_start_matches('#')
                .trim()
                .to_lowercase();
            if !normalized.is_empty() && (normalized == needle || normalized.contains(&needle)) {
                found = Some(idx);
                break;
            }
        }
        if let Some(line_idx) = found {
            mapped.push(DocSection {
                line: line_idx,
                title: heading.clone(),
            });
            search_from = line_idx + 1;
        }
    }
    mapped
}

fn is_man_section_heading(line: &str) -> bool {
    if line.is_empty() || line.starts_with(' ') || line.starts_with('\t') {
        return false;
    }
    let trimmed = line.trim();
    trimmed.len() >= 3
        && trimmed.len() <= 40
        && trimmed.chars().any(|c| c.is_ascii_uppercase())
        && trimmed
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, ' ' | '-' | '/'))
}

fn extract_man_sections(lines: &[String]) -> Vec<DocSection> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_man_section_heading(line))
        .map(|(idx, line)| DocSection {
            line: idx,
            title: line.trim().to_string(),
        })
        .collect()
}

/// Return the 1-based section index and active section at `scroll`.
pub fn current_section(sections: &[DocSection], scroll: usize) -> Option<(usize, &DocSection)> {
    let mut active = None;
    for (idx, sec) in sections.iter().enumerate() {
        if sec.line <= scroll {
            active = Some((idx + 1, sec));
        } else {
            break;
        }
    }
    active
}

/// Return the target line for `]` (next `##` heading strictly after `scroll`).
pub fn next_section_line(sections: &[DocSection], scroll: usize) -> Option<usize> {
    sections.iter().find(|s| s.line > scroll).map(|s| s.line)
}

/// Return the target line for `[` (previous `##` heading strictly before `scroll`,
/// or `0` if already before the first heading and scrolled past line 0).
pub fn prev_section_line(sections: &[DocSection], scroll: usize) -> Option<usize> {
    if let Some(prev) = sections.iter().rev().find(|s| s.line < scroll) {
        Some(prev.line)
    } else if scroll > 0 {
        Some(0)
    } else {
        None
    }
}

fn push_bytes(buf: &mut [u8], len: &mut usize, bytes: &[u8]) {
    let room = buf.len().saturating_sub(*len);
    let n = bytes.len().min(room);
    buf[*len..*len + n].copy_from_slice(&bytes[..n]);
    *len += n;
}

fn push_usize(buf: &mut [u8], len: &mut usize, mut value: usize) {
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    if value == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while value > 0 && count < digits.len() {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
        digits[..count].reverse();
    }
    push_bytes(buf, len, &digits[..count]);
}

fn format_reader_stats(
    buf: &mut [u8; 40],
    line: usize,
    total_lines: usize,
    sec_index: Option<usize>,
    sec_total: usize,
    read_minutes: Option<u16>,
) -> &str {
    let mut len = 0usize;
    push_bytes(buf, &mut len, b"L ");
    push_usize(buf, &mut len, line);
    push_bytes(buf, &mut len, b"/");
    push_usize(buf, &mut len, total_lines);
    if sec_total > 0 {
        push_bytes(buf, &mut len, b" \xc2\xb7 \xc2\xa7");
        push_usize(buf, &mut len, sec_index.unwrap_or(0));
        push_bytes(buf, &mut len, b"/");
        push_usize(buf, &mut len, sec_total);
    }
    if let Some(mins) = read_minutes {
        push_bytes(buf, &mut len, b" \xc2\xb7 ~");
        push_usize(buf, &mut len, usize::from(mins));
        push_bytes(buf, &mut len, b"m");
    }
    std::str::from_utf8(&buf[..len]).unwrap_or("")
}

/// `hayato menu learn` (`hayato-learn`): the interactive field manual viewer.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["rg", "apropos", "man"]) {
        return Err(crate::Failure::new(
            "Learn",
            format!("{said} (ripgrep, man-db)"),
        ));
    }
    let mut screen = LearnScreen::new();
    let code = crate::run_screen(terminal, design, "Learn", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayato_ui::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_screen(screen: &mut LearnScreen, width: u16, height: u16) -> String {
        let mut app = App::new(Header::new("Hayato"))
            .design(Design::solarized().with_reduced_motion(true))
            .screen(screen);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        hayato_ui::screen(terminal.backend().buffer())
    }

    fn sample_doc_md() -> &'static str {
        "# Getting Started\n\n\
         Welcome to Hayato Linux on ARM64.\n\n\
         ## First Boot\n\n\
         Press Alt+Space to open the launcher.\n\n\
         ```bash\n\
         ## not a heading inside code fence\n\
         hayato menu learn\n\
         ```\n\n\
         ## Keybindings\n\n\
         - Alt+Space: Launcher\n\
         - Alt+Return: Terminal\n\n\
         ## Recovery\n\n\
         Run hayato check to verify the session.\n"
    }

    #[test]
    fn markdown_headings() {
        let content = "# H1\n## H2\n### H3\n";
        let lines = LearnScreen::render_markdown(content);
        assert!(lines[0].contains("H1"));
        assert!(lines[1].contains("H2"));
        assert!(lines[2].contains("H3"));
    }

    #[test]
    fn markdown_code_fence() {
        let content = "```rust\nfn main() {}\n```\n";
        let lines = LearnScreen::render_markdown(content);
        assert!(lines[0].contains("rust"));
        assert!(lines[1].contains("fn main"));
        assert!(lines[2].contains("```"));
    }

    #[test]
    fn markdown_list() {
        let content = "- item 1\n- item 2\n";
        let lines = LearnScreen::render_markdown(content);
        assert!(lines[0].contains("• item 1"));
        assert!(lines[1].contains("• item 2"));
    }

    #[test]
    fn reading_time_calculation() {
        assert_eq!(estimate_read_minutes(""), 0);
        assert_eq!(estimate_read_minutes("hello world"), 1);
        let words_200 = "word ".repeat(200);
        assert_eq!(estimate_read_minutes(&words_200), 1);
        let words_201 = "word ".repeat(201);
        assert_eq!(estimate_read_minutes(&words_201), 2);
        let words_650 = "word ".repeat(650);
        assert_eq!(estimate_read_minutes(&words_650), 4);
    }

    #[test]
    fn section_heading_extraction_and_jumps() {
        let md = sample_doc_md();
        let headings = extract_markdown_sections(md);
        assert_eq!(headings, vec!["First Boot", "Keybindings", "Recovery"]);

        let rendered: Vec<Vec<DocSpan>> = LearnScreen::render_markdown(md)
            .into_iter()
            .map(|text| {
                vec![DocSpan {
                    text,
                    style: TextStyle::Body,
                }]
            })
            .collect();
        let sections = map_sections_to_lines(&headings, &rendered);
        assert_eq!(sections.len(), 3);
        assert!(sections[0].line < sections[1].line);
        assert!(sections[1].line < sections[2].line);

        // Jump forward with `]`
        let first = next_section_line(&sections, 0).unwrap();
        assert_eq!(first, sections[0].line);
        let second = next_section_line(&sections, first).unwrap();
        assert_eq!(second, sections[1].line);
        let third = next_section_line(&sections, second).unwrap();
        assert_eq!(third, sections[2].line);
        assert_eq!(next_section_line(&sections, third), None);

        // Current section tracking
        assert_eq!(
            current_section(&sections, second).map(|(i, s)| (i, s.title.as_str())),
            Some((2, "Keybindings"))
        );

        // Jump backward with `[`
        assert_eq!(prev_section_line(&sections, third), Some(second));
        assert_eq!(prev_section_line(&sections, second), Some(first));
        assert_eq!(prev_section_line(&sections, first), Some(0));
        assert_eq!(prev_section_line(&sections, 0), None);
    }

    #[test]
    fn dual_pane_and_narrow_render_cleanly() {
        let page = ManualPage {
            path: PathBuf::from("/virtual/01-start.md"),
            chapter: "01".to_string(),
            title: "Getting Started".to_string(),
            summary: "Welcome to Hayato Linux on ARM64.".to_string(),
            read_minutes: 2,
            word_count: 350,
        };
        let doc = LearnScreen::build_markdown_doc(None, sample_doc_md(), Some(&page));
        let mut screen = LearnScreen::with_pages(vec![page.clone()]);
        screen.doc_cache.insert(page.path.clone(), doc.clone());
        screen.active_path = Some(page.path);
        screen.active_doc = Some(doc);

        // Wide 152x45: dual-pane shows both Field Manual sidebar tabs and Reader pane
        let wide = render_screen(&mut screen, 152, 45);
        assert!(wide.contains("Hayato"), "header present");
        assert!(wide.contains("Field Manual"), "sidebar card present");
        assert!(wide.contains("[1] Manual"), "Manual tab present");
        assert!(wide.contains("[2] Search"), "Search tab present");
        assert!(wide.contains("[3] Man"), "Man tab present");
        assert!(wide.contains("01 · Getting Started"), "chapter row present");
        assert!(wide.contains("~2m"), "reading time estimate present");
        assert!(wide.contains("First Boot"), "live reader preview present");

        // Narrow 80x24: sidebar mode shows tabs & chapters cleanly
        let narrow_sidebar = render_screen(&mut screen, 80, 24);
        assert!(narrow_sidebar.contains("Field Manual"));
        assert!(narrow_sidebar.contains("01 · Getting Started"));

        // Toggle focus to Reader via Tab and render at 80x24
        let design = Design::solarized().with_reduced_motion(true);
        let outcome = screen.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &design);
        assert_eq!(outcome, Outcome::Consumed);
        assert_eq!(screen.phase(), Some("manual-reader"));

        let narrow_reader = render_screen(&mut screen, 80, 24);
        assert!(narrow_reader.contains("Getting Started"));
        assert!(narrow_reader.contains("First Boot"));
    }
}
