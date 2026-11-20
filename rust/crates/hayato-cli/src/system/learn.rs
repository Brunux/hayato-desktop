#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
/// Learn: the seven manual pages rendered in the window, plus `apropos` over man pages (`hayato-learn`).
///
/// The old Learn menu opened `glow` in a terminal for each page. This version runs
/// glow into a pipe and maps its ANSI back onto the theme's roles, through the
/// shared `system::manual` renderer; a subset renderer is the fallback when glow is
/// missing. The `apropos` search opens man pages in the same renderer.
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, Design, Glyph, Header, Input, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_card, draw_filter_bar,
    draw_scrollbar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use crate::system::manual::{DocSpan, parse_ansi};
use crate::system::{have, missing};

const DOCS_DIR: &str = "/usr/share/doc/hayato";
const PAGE_WRAP_WIDTH: usize = 88;
const ACTION_BASE: usize = 10_000;

/// A manual page entry.
#[derive(Clone, Debug)]
pub struct ManualPage {
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
}

/// The learn state.
pub struct LearnScreen {
    input: Input,
    pages: Vec<ManualPage>,
    filtered: Vec<ManualPage>,
    list: ListState,
    strip: ActionStrip,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    view_mode: LearnMode,
    page_title: String,
    content_lines: Vec<Vec<DocSpan>>,
    content_scroll: Cell<u16>,
    search_input: Input,
    search_results: Vec<SearchResult>,
    search_list: ListState,
    page_keyhints: Vec<KeyHint>,
    search_keyhints: Vec<KeyHint>,
}

/// Result from manual search or apropos.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub detail: String,
    pub action: SearchAction,
}

/// Action to take on search result.
#[derive(Clone, Debug)]
pub enum SearchAction {
    OpenPage(PathBuf),
    OpenManPage(String, String), // page, section
}

/// Mode of the learn screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnMode {
    List,
    Page,
    Search,
    Apropos,
}

impl LearnScreen {
    pub fn new() -> Self {
        let pages = Self::load_pages();
        let list = ListState::new()
            .with_empty("no manual pages found")
            .ranked();
        let strip = ActionStrip::new(vec![
            ActionItem::new("search", "^F", "Search manuals").glyph(Glyph::Search),
            ActionItem::new("apropos", "^M", "Man pages (apropos)").glyph(Glyph::Book),
        ]);

        let mut screen = Self {
            input: Input::new(),
            pages: pages.clone(),
            filtered: pages,
            list,
            strip,
            header: Header::new("Hayato").subtitle("Learn"),
            keybar: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("^F", "search manuals"),
                KeyHint::new("^M", "man pages"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Esc", "back/close"),
            ],
            rows_height: Cell::new(20),
            view_mode: LearnMode::List,
            page_title: "Learn".to_string(),
            content_lines: Vec::new(),
            content_scroll: Cell::new(0),
            search_input: Input::new(),
            search_results: Vec::new(),
            search_list: ListState::new()
                .with_empty("type a query above to search")
                .ranked(),
            page_keyhints: vec![
                KeyHint::new("\u{2191}\u{2193}", "scroll"),
                KeyHint::new("PgUp/PgDn", "page"),
                KeyHint::new("Home/End", "top/bottom"),
                KeyHint::new("Esc", "back"),
            ],
            search_keyhints: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("type", "query"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "back"),
            ],
        };
        screen.filter_pages();
        screen
    }

    fn load_pages() -> Vec<ManualPage> {
        let mut pages = Vec::new();
        if let Ok(entries) = fs::read_dir(DOCS_DIR) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let title = content
                            .lines()
                            .next()
                            .and_then(|l| l.strip_prefix("# "))
                            .unwrap_or_else(|| {
                                path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
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
                                    .take(80)
                                    .collect::<String>()
                            })
                            .unwrap_or_default();
                        pages.push(ManualPage {
                            path,
                            title,
                            summary,
                        });
                    }
                }
            }
        }
        pages.sort_by(|a, b| a.path.cmp(&b.path));
        pages
    }

    fn page_row(page: &ManualPage) -> Row {
        Row::new(&page.title)
            .glyph(Glyph::Book)
            .detail(&page.summary)
    }

    fn search_row(result: &SearchResult) -> Row {
        Row::new(&result.title)
            .glyph(Glyph::Search)
            .detail(&result.detail)
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
                    p.title.to_lowercase().contains(&query)
                        || p.summary.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }

        let rows: Vec<Row> = self.filtered.iter().map(Self::page_row).collect();
        self.list.set_rows(rows);
    }

    fn enter_search_manuals(&mut self) {
        let seed = self.input.text().trim().to_string();
        self.search_input.set(seed);
        self.view_mode = LearnMode::Search;
        self.refresh_search_manuals();
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
                                title: format!("{} · line {}", title, line_num),
                                detail: clean_text,
                                action: SearchAction::OpenPage(PathBuf::from(file)),
                            });
                        }
                    }
                }
            }
        }

        self.search_results = results.clone();
        self.search_list
            .set_rows(results.iter().map(Self::search_row).collect());
    }

    fn enter_apropos(&mut self) {
        let seed = self.input.text().trim().to_string();
        self.search_input.set(seed);
        self.view_mode = LearnMode::Apropos;
        self.refresh_apropos();
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
                                let section = left[paren + 1..left.len() - 1].trim();
                                results.push(SearchResult {
                                    title: format!("{} ({})", name, section),
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

        self.search_results = results.clone();
        self.search_list
            .set_rows(results.iter().map(Self::search_row).collect());
    }

    fn trigger_strip_action(&mut self, id: &str) {
        match id {
            "search" => self.enter_search_manuals(),
            "apropos" => self.enter_apropos(),
            _ => {}
        }
    }

    fn open_page(&mut self, path: &Path) {
        if let Ok(content) = fs::read_to_string(path) {
            self.page_title = content
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("# "))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "Learn".to_string());
            self.content_lines = Self::render_markdown_styled(Some(path), &content);
            self.content_scroll.set(0);
            self.view_mode = LearnMode::Page;
        }
    }

    fn open_man_page(&mut self, page: &str, section: &str) {
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
                self.page_title = format!("{page}({section})");
                self.content_lines = Self::render_man(&content)
                    .into_iter()
                    .map(|l| {
                        vec![DocSpan {
                            text: l,
                            style: TextStyle::Body,
                        }]
                    })
                    .collect();
                self.content_scroll.set(0);
                self.view_mode = LearnMode::Page;
            }
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

    fn render_markdown(content: &str) -> Vec<String> {
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
                lines.push(format!("  {}", &clean_trimmed[4..]));
            } else if clean_trimmed.starts_with("## ") {
                lines.push(format!(" {}", &clean_trimmed[3..]));
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
}

impl Screen for LearnScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let card_title = match self.view_mode {
            LearnMode::List => "Pages",
            LearnMode::Page => self.page_title.as_str(),
            LearnMode::Search => "Learn · Manual Search",
            LearnMode::Apropos => "Learn · System Man Pages (apropos)",
        };
        draw_card(ctx.buf, ctx.area, ctx.design, Some(card_title), true);

        match self.view_mode {
            LearnMode::List => {
                let (strip_rect, filter, rows) = Self::list_layout(ctx.area);
                if let Some(sr) = strip_rect {
                    self.strip.render(sr, ctx.buf, ctx.design);
                }
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    self.input.text(),
                    "Enter opens manual page · type to filter · ^F search manuals · ^M man pages",
                    self.list.len(),
                    self.pages.len(),
                );
                self.rows_height.set(rows.height);
                self.list
                    .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
            }
            LearnMode::Page => {
                let inner = Rect {
                    x: ctx.area.x + 2,
                    y: ctx.area.y + 1,
                    width: ctx.area.width.saturating_sub(4),
                    height: ctx.area.height.saturating_sub(2),
                };
                let max_x = inner.x.saturating_add(inner.width.saturating_sub(2));
                let scroll = self.content_scroll.get() as usize;
                for (i, spans) in self.content_lines.iter().skip(scroll).enumerate() {
                    if i as u16 >= inner.height {
                        break;
                    }
                    let y = inner.y + i as u16;
                    let mut x = inner.x;
                    for span in spans {
                        let style = ctx.design.style(span.style);
                        for ch in span.text.chars() {
                            let w = UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                            if x.saturating_add(w) > max_x {
                                break;
                            }
                            let mut buf = [0u8; 4];
                            ctx.buf.set_string(x, y, ch.encode_utf8(&mut buf), style);
                            x = x.saturating_add(w);
                        }
                    }
                }
                draw_scrollbar(
                    ctx.buf,
                    inner,
                    ctx.design,
                    scroll,
                    inner.height as usize,
                    self.content_lines.len(),
                    true,
                );
            }
            LearnMode::Search | LearnMode::Apropos => {
                let (_, filter, rows) = Self::list_layout(ctx.area);
                let hint = if self.view_mode == LearnMode::Search {
                    "Type to search /usr/share/doc/hayato with ripgrep · Enter opens · Esc back"
                } else {
                    "Type to search system man pages with apropos · Enter opens · Esc back"
                };
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    self.search_input.text(),
                    hint,
                    self.search_list.len(),
                    self.search_results.len(),
                );
                self.rows_height.set(rows.height);
                self.search_list.render(rows, ctx.buf, ctx.design, true);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        match self.view_mode {
            LearnMode::List => {
                if self.strip.is_focused() {
                    match self.strip.handle(key) {
                        ActionStripEvent::Activated(id) => {
                            self.strip.set_focused(false);
                            self.trigger_strip_action(id);
                            return Outcome::Consumed;
                        }
                        ActionStripEvent::Consumed => return Outcome::Consumed,
                        ActionStripEvent::Ignored => {}
                    }
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match key.code {
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            self.enter_search_manuals();
                            return Outcome::Consumed;
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            self.enter_apropos();
                            return Outcome::Consumed;
                        }
                        _ => {}
                    }
                }
                if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                    self.strip.set_focused(!self.strip.is_focused());
                    return Outcome::Consumed;
                }
                let rows_height = self.rows_height.get();
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    let _ = self.input.handle(key);
                    self.filter_pages();
                    return Outcome::Consumed;
                }
                match key.code {
                    KeyCode::Enter => {
                        if let Some(selected) = self.list.selected_index() {
                            self.activate_list_row(selected);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Esc => {
                        if !self.input.text().is_empty() {
                            self.input.set("");
                            self.filter_pages();
                            return Outcome::Consumed;
                        }
                        return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                    }
                    _ => {}
                }
                match self.list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => {
                        self.activate_list_row(index);
                        Outcome::Consumed
                    }
                    _ => Outcome::Continue,
                }
            }
            LearnMode::Page => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.view_mode = LearnMode::List;
                    Outcome::Consumed
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let scroll = self.content_scroll.get();
                    if scroll > 0 {
                        self.content_scroll.set(scroll - 1);
                    }
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let scroll = self.content_scroll.get() as usize;
                    let max = self.content_lines.len().saturating_sub(1);
                    if scroll < max {
                        self.content_scroll.set((scroll + 1) as u16);
                    }
                    Outcome::Consumed
                }
                KeyCode::PageUp => {
                    let scroll = self.content_scroll.get() as usize;
                    self.content_scroll.set((scroll.saturating_sub(15)) as u16);
                    Outcome::Consumed
                }
                KeyCode::PageDown => {
                    let scroll = self.content_scroll.get() as usize;
                    let max = self.content_lines.len().saturating_sub(1);
                    self.content_scroll.set(((scroll + 15).min(max)) as u16);
                    Outcome::Consumed
                }
                KeyCode::Home => {
                    self.content_scroll.set(0);
                    Outcome::Consumed
                }
                KeyCode::End => {
                    let max = self.content_lines.len().saturating_sub(1);
                    self.content_scroll.set(max as u16);
                    Outcome::Consumed
                }
                _ => Outcome::Continue,
            },
            LearnMode::Search | LearnMode::Apropos => {
                let rows_height = self.rows_height.get();
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    let _ = self.search_input.handle(key);
                    if self.view_mode == LearnMode::Search {
                        self.refresh_search_manuals();
                    } else {
                        self.refresh_apropos();
                    }
                    return Outcome::Consumed;
                }
                match key.code {
                    KeyCode::Enter => {
                        if let Some(selected) = self.search_list.selected_index() {
                            if let Some(result) = self.search_results.get(selected).cloned() {
                                match &result.action {
                                    SearchAction::OpenPage(path) => self.open_page(path),
                                    SearchAction::OpenManPage(page, section) => {
                                        self.open_man_page(page, section)
                                    }
                                }
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Esc => {
                        self.view_mode = LearnMode::List;
                        return Outcome::Consumed;
                    }
                    _ => {}
                }
                match self.search_list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => {
                        if let Some(result) = self.search_results.get(index).cloned() {
                            match &result.action {
                                SearchAction::OpenPage(path) => self.open_page(path),
                                SearchAction::OpenManPage(page, section) => {
                                    self.open_man_page(page, section)
                                }
                            }
                        }
                        Outcome::Consumed
                    }
                    _ => Outcome::Continue,
                }
            }
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        match self.view_mode {
            LearnMode::List => {
                if let Some(row) = row {
                    if row >= ACTION_BASE {
                        let id = match row - ACTION_BASE {
                            0 => "search",
                            1 => "apropos",
                            _ => "",
                        };
                        self.trigger_strip_action(id);
                        return Outcome::Consumed;
                    }
                    self.list.set_cursor(row);
                    if let Some(index) = self.list.selected_index() {
                        self.activate_list_row(index);
                    }
                }
                Outcome::Consumed
            }
            LearnMode::Search | LearnMode::Apropos => {
                if let Some(row) = row {
                    self.search_list.set_cursor(row);
                    if let Some(index) = self.search_list.selected_index() {
                        if let Some(result) = self.search_results.get(index).cloned() {
                            match &result.action {
                                SearchAction::OpenPage(path) => self.open_page(path),
                                SearchAction::OpenManPage(page, section) => {
                                    self.open_man_page(page, section)
                                }
                            }
                        }
                    }
                }
                Outcome::Consumed
            }
            LearnMode::Page => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        match self.view_mode {
            LearnMode::List => {
                let (strip_rect, _, rows) = Self::list_layout(area);
                if let Some(sr) = strip_rect {
                    if let Some(idx) = self.strip.hit_index(sr, x, y) {
                        return Some(ACTION_BASE + idx);
                    }
                }
                self.list.hit_row(rows, x, y, design)
            }
            LearnMode::Search | LearnMode::Apropos => {
                let (_, _, rows) = Self::list_layout(area);
                self.search_list.hit_row(rows, x, y, design)
            }
            LearnMode::Page => None,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.view_mode {
            LearnMode::List => Some(&self.keybar),
            LearnMode::Page => Some(&self.page_keyhints),
            LearnMode::Search | LearnMode::Apropos => Some(&self.search_keyhints),
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.view_mode {
            LearnMode::List => "list",
            LearnMode::Page => "page",
            LearnMode::Search => "search",
            LearnMode::Apropos => "apropos",
        })
    }

    fn desired_size(&self) -> Option<(u16, u16)> {
        match self.view_mode {
            LearnMode::Page => Some((1060, 740)),
            LearnMode::List | LearnMode::Search | LearnMode::Apropos => Some((860, 540)),
        }
    }
}

impl LearnScreen {
    fn activate_list_row(&mut self, index: usize) {
        if let Some(page) = self.filtered.get(index).cloned() {
            self.open_page(&page.path);
        }
    }

    fn list_layout(area: Rect) -> (Option<Rect>, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        if inner.height >= 10 {
            let strip = Rect { height: 1, ..inner };
            let filter = Rect {
                y: inner.y + 2,
                height: 1,
                ..inner
            };
            let rows = Rect {
                y: inner.y + 4,
                height: inner.height.saturating_sub(4),
                ..inner
            };
            (Some(strip), filter, rows)
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

/// `hayato menu learn` (`hayato-learn`): the manual viewer.
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
}
