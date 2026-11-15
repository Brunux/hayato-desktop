#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
/// Learn: the seven manual pages rendered in the window, plus `apropos` over man pages (`rofi-learn`).
///
/// The old `rofi-learn` opened `glow` in a terminal for each page. This version renders
/// markdown directly in the TUI using a subset renderer (headings, lists, code fences,
/// bold, tables) with the theme's code role. The `apropos` search opens man pages in
/// the same renderer.
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use lazyos_ui::{
    BodyCtx, Card, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_card, draw_filter_bar, draw_scrollbar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::missing;

const DOCS_DIR: &str = "/usr/share/doc/lazyos";

/// A manual page entry.
#[derive(Clone, Debug)]
pub struct ManualPage {
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
}

#[derive(Clone, Debug)]
enum LearnRowAction {
    Page(PathBuf),
    SearchManuals,
    Apropos,
}

/// The learn state.
pub struct LearnScreen {
    input: Input,
    pages: Vec<ManualPage>,
    filtered: Vec<ManualPage>,
    actions: Vec<LearnRowAction>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    view_mode: LearnMode,
    content_lines: Vec<String>,
    content_scroll: Cell<u16>,
    search_query: String,
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

        let mut screen = Self {
            input: Input::new(),
            pages: pages.clone(),
            filtered: pages,
            actions: Vec::new(),
            list,
            header: Header::new("LazyOS").subtitle("Learn"),
            keybar: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("type", "filter"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "back/close"),
            ],
            rows_height: Cell::new(20),
            view_mode: LearnMode::List,
            content_lines: Vec::new(),
            content_scroll: Cell::new(0),
            search_query: String::new(),
            search_results: Vec::new(),
            search_list: ListState::new().ranked(),
            page_keyhints: vec![
                KeyHint::new("\u{2191}\u{2193}", "scroll"),
                KeyHint::new("PgUp/PgDn", "page"),
                KeyHint::new("Esc", "back"),
            ],
            search_keyhints: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("type", "filter"),
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
                            .map(|l| l.trim().chars().take(80).collect::<String>())
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
        pages.sort_by(|a, b| a.title.cmp(&b.title));
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

        let mut rows = Vec::new();
        let mut actions = Vec::new();

        for p in &self.filtered {
            rows.push(Self::page_row(p));
            actions.push(LearnRowAction::Page(p.path.clone()));
        }

        if !query.is_empty() {
            rows.push(
                Row::new(format!("Search manual pages for “{query}”"))
                    .glyph(Glyph::Search)
                    .detail("full-text ripgrep over /usr/share/doc/lazyos"),
            );
            actions.push(LearnRowAction::SearchManuals);

            rows.push(
                Row::new(format!("Search system man pages (apropos) for “{query}”"))
                    .glyph(Glyph::Book)
                    .detail("apropos over installed man pages"),
            );
            actions.push(LearnRowAction::Apropos);
        }

        self.actions = actions;
        self.list.set_rows(rows);
    }

    fn search_manuals(&mut self) {
        let query = self.input.text().trim();
        if query.is_empty() {
            return;
        }
        self.search_query = query.to_string();

        // Search with ripgrep
        let output = std::process::Command::new("rg")
            .args([
                "-n",
                "-i",
                "--no-heading",
                "--with-filename",
                "--max-count",
                "5",
                query,
                DOCS_DIR,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        let mut results = Vec::new();
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

        self.search_results = results.clone();
        self.search_list
            .set_rows(results.iter().map(Self::search_row).collect());
        self.view_mode = LearnMode::Search;
    }

    fn run_apropos(&mut self) {
        let query = self.input.text().trim();
        if query.is_empty() {
            return;
        }
        self.search_query = query.to_string();

        let output = std::process::Command::new("apropos")
            .args(["--", query])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        let mut results = Vec::new();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines().take(60) {
                    // Format: "name (section) - description"
                    if let Some(dash_pos) = line.find(" - ") {
                        let left = &line[..dash_pos];
                        let desc = &line[dash_pos + 3..];
                        if let Some(paren) = left.rfind('(') {
                            let name = &left[..paren].trim();
                            let section = &left[paren + 1..left.len() - 1].trim();
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

        self.search_results = results.clone();
        self.search_list
            .set_rows(results.iter().map(Self::search_row).collect());
        self.view_mode = LearnMode::Apropos;
    }

    fn open_page(&mut self, path: &Path) {
        if let Ok(content) = fs::read_to_string(path) {
            self.content_lines = Self::render_markdown(&content);
            self.content_scroll.set(0);
            self.view_mode = LearnMode::Page;
        }
    }

    fn open_man_page(&mut self, page: &str, section: &str) {
        let output = std::process::Command::new("man")
            .args([section, page])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout).to_string();
                self.content_lines = Self::render_man(&content);
                self.content_scroll.set(0);
                self.view_mode = LearnMode::Page;
            }
        }
    }

    fn render_markdown(content: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut in_code = false;
        let mut code_lang: String;

        for line in content.lines() {
            let trimmed = line.trim();

            // Code fences
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

            // Headings
            if trimmed.starts_with("### ") {
                lines.push(format!("  {}", &trimmed[4..]));
            } else if trimmed.starts_with("## ") {
                lines.push(format!(" {}", &trimmed[3..]));
            } else if trimmed.starts_with("# ") {
                lines.push(format!("{}", &trimmed[2..]));
            }
            // Bold
            else if trimmed.contains("**") {
                let mut l = line.to_string();
                // Simple bold rendering - just remove **
                l = l.replace("**", "");
                lines.push(l);
            }
            // Lists
            else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                lines.push(format!("  • {}", &trimmed[2..]));
            }
            // Numbered lists
            else if trimmed.len() > 2
                && trimmed.chars().nth(1) == Some('.')
                && trimmed.chars().next().unwrap().is_ascii_digit()
            {
                lines.push(format!("  {}", trimmed));
            } else if trimmed.starts_with('|') && trimmed.ends_with('|') || !trimmed.is_empty() {
                lines.push(line.to_string());
            } else {
                lines.push(String::new());
            }
        }

        lines
    }

    fn render_man(content: &str) -> Vec<String> {
        // Simple man page rendering - just wrap lines
        content.lines().map(|l| l.to_string()).collect()
    }
}

impl Screen for LearnScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Learn"), true);

        match self.view_mode {
            LearnMode::List => {
                let (filter, rows) = Self::list_layout(ctx.area);
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    self.input.text(),
                    "Enter opens manual page · type to filter or search",
                    self.list.len(),
                    self.actions.len(),
                );
                self.rows_height.set(rows.height);
                self.list.render(rows, ctx.buf, ctx.design, true);
            }
            LearnMode::Page => {
                let inner = Rect {
                    x: ctx.area.x + 2,
                    y: ctx.area.y + 1,
                    width: ctx.area.width.saturating_sub(4),
                    height: ctx.area.height.saturating_sub(2),
                };
                let scroll = self.content_scroll.get() as usize;
                for (i, line) in self.content_lines.iter().skip(scroll).enumerate() {
                    if i as u16 >= inner.height {
                        break;
                    }
                    ctx.buf.set_string(
                        inner.x,
                        inner.y + i as u16,
                        line,
                        ctx.design.style(TextStyle::Body),
                    );
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
                let (filter, rows) = Self::list_layout(ctx.area);
                let mode_name = if self.view_mode == LearnMode::Search {
                    "Manual search"
                } else {
                    "Apropos"
                };
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    "",
                    &format!("{mode_name} results for “{}” · Esc goes back", self.search_query),
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
                let rows_height = self.rows_height.get();
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    let _ = self.input.handle(key);
                    self.filter_pages();
                    return Outcome::Continue;
                }
                match key.code {
                    KeyCode::Enter => {
                        if let Some(selected) = self.list.selected_index() {
                            self.activate_list_row(selected);
                        }
                        return Outcome::Continue;
                    }
                    KeyCode::Esc => {
                        if !self.input.text().is_empty() {
                            self.input.set("");
                            self.filter_pages();
                            return Outcome::Continue;
                        }
                        return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
                    }
                    _ => {}
                }
                match self.list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => {
                        self.activate_list_row(index);
                        Outcome::Continue
                    }
                    _ => Outcome::Continue,
                }
            }
            LearnMode::Page => match key.code {
                KeyCode::Esc => {
                    self.view_mode = LearnMode::List;
                    Outcome::Continue
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let scroll = self.content_scroll.get();
                    if scroll > 0 {
                        self.content_scroll.set(scroll - 1);
                    }
                    Outcome::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let scroll = self.content_scroll.get() as usize;
                    let max = self.content_lines.len().saturating_sub(1);
                    if scroll < max {
                        self.content_scroll.set((scroll + 1) as u16);
                    }
                    Outcome::Continue
                }
                KeyCode::PageUp => {
                    let scroll = self.content_scroll.get() as usize;
                    self.content_scroll.set((scroll.saturating_sub(15)) as u16);
                    Outcome::Continue
                }
                KeyCode::PageDown => {
                    let scroll = self.content_scroll.get() as usize;
                    let max = self.content_lines.len().saturating_sub(1);
                    self.content_scroll.set(((scroll + 15).min(max)) as u16);
                    Outcome::Continue
                }
                KeyCode::Home => {
                    self.content_scroll.set(0);
                    Outcome::Continue
                }
                KeyCode::End => {
                    let max = self.content_lines.len().saturating_sub(1);
                    self.content_scroll.set(max as u16);
                    Outcome::Continue
                }
                _ => Outcome::Continue,
            },
            LearnMode::Search | LearnMode::Apropos => {
                let rows_height = self.rows_height.get();
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
                        return Outcome::Continue;
                    }
                    KeyCode::Esc => {
                        self.view_mode = LearnMode::List;
                        return Outcome::Continue;
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
                        Outcome::Continue
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
                    self.list.set_cursor(row);
                    if let Some(index) = self.list.selected_index() {
                        self.activate_list_row(index);
                    }
                }
                Outcome::Continue
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
                Outcome::Continue
            }
            LearnMode::Page => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        match self.view_mode {
            LearnMode::List => {
                let (_, rows) = Self::list_layout(area);
                self.list.hit_row(rows, x, y, design)
            }
            LearnMode::Search | LearnMode::Apropos => {
                let (_, rows) = Self::list_layout(area);
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

    fn desired_size(&self) -> Option<(u16, u16)> {
        match self.view_mode {
            LearnMode::Page => Some((1060, 740)),
            LearnMode::List | LearnMode::Search | LearnMode::Apropos => Some((860, 540)),
        }
    }
}

impl LearnScreen {
    fn activate_list_row(&mut self, index: usize) {
        if let Some(action) = self.actions.get(index).cloned() {
            match action {
                LearnRowAction::Page(path) => self.open_page(&path),
                LearnRowAction::SearchManuals => self.search_manuals(),
                LearnRowAction::Apropos => self.run_apropos(),
            }
        }
    }

    fn list_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let filter = Rect {
            height: 1.min(inner.height),
            ..inner
        };
        let rows = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        (filter, rows)
    }
}

/// `lazyos menu learn` (`rofi-learn`): the manual viewer.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["rg", "apropos", "man"]) {
        return Err(crate::Failure::new("Learn", format!("{said} (ripgrep, man-db)")));
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
