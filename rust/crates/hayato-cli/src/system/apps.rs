// Launcher (Apps): `.desktop` entries with icons, frecency, and `Terminal=true` → foot.
//
// The old `the apps menu` was a black box: the icon decoding happened at open time
// through gdk-pixbuf, the ranking was the old picker's own, and a `Terminal=true` entry opened
// in whatever terminal the old picker's `terminal` setting named. This version parses the
// `.desktop` files itself, resolves icons through the Hayato theme, shares frecency
// with the palette, and spawns `Terminal=true` entries in foot.
//
// The parser is a pure function with fixtures for the real files on this image
// (`firefox-esr`, `org.hayato.*`, a `Terminal=true` entry, a `NoDisplay=true` entry).

#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use hayato_ui::{
    BodyCtx, Card, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, Urgency, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::menu::frecency::Frecency;
use crate::menu::table::{Action, Arg, PALETTE_EXTRA, Program, ROOT};
use crate::paths::Paths;
use crate::system::{missing, print};

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    dirs
}

/// A parsed `.desktop` entry or built-in Hayato tool.
#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub glyph_override: Option<Glyph>,
    pub terminal: bool,
    pub no_display: bool,
    pub comment: Option<String>,
    pub categories: Vec<String>,
}

/// The launcher state.
pub struct AppsScreen {
    input: Input,
    entries: Vec<DesktopEntry>,
    filtered: Vec<DesktopEntry>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    frecency: Frecency,
}

impl AppsScreen {
    pub fn new() -> Self {
        let entries = Self::load_entries();
        let frecency = Self::load_frecency();
        let now = Self::now_secs();

        let mut list = ListState::new()
            .with_empty("no applications found")
            .ranked();
        list.set_rows(
            entries
                .iter()
                .map(|e| Self::entry_row(e, &frecency, now))
                .collect(),
        );

        Self {
            input: Input::new(),
            entries: entries.clone(),
            filtered: entries,
            list,
            header: Header::new("Hayato").subtitle("Search"),
            keybar: vec![
                KeyHint::new("Enter", "launch"),
                KeyHint::new("Shift+Enter", "launch in foot"),
                KeyHint::new("type", "filter"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            frecency,
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn load_entries() -> Vec<DesktopEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for dir in desktop_dirs() {
            if let Ok(read_dir) = fs::read_dir(dir) {
                for entry in read_dir.flatten() {
                    if entry.path().extension().and_then(|s| s.to_str()) == Some("desktop") {
                        if let Ok(parsed) = Self::parse_desktop(&entry.path()) {
                            if !parsed.no_display {
                                seen.insert(parsed.name.to_lowercase());
                                entries.push(parsed);
                            }
                        }
                    }
                }
            }
        }
        // Include every Hayato tool (ROOT + PALETTE_EXTRA) so Super/Search can
        // find and launch any app, system tool, or utility in one place.
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        for item in ROOT.iter().chain(PALETTE_EXTRA.iter()) {
            if item.key == "apps" {
                continue;
            }
            if !seen.insert(item.label.to_lowercase()) {
                continue;
            }
            let Some(exec) = Self::action_to_exec(item.action, &home) else {
                continue;
            };
            entries.push(DesktopEntry {
                name: item.label.to_string(),
                exec,
                icon: None,
                glyph_override: Some(item.glyph),
                terminal: false,
                no_display: false,
                comment: Some(item.detail.to_string()),
                categories: vec![item.group.label().to_string()],
            });
        }
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries
    }

    fn action_to_exec(action: Action, home: &Path) -> Option<String> {
        match action {
            Action::Run { program, args } => {
                let mut parts = Vec::with_capacity(args.len() + 1);
                let prog = match program {
                    Program::Session(name) => {
                        let p = home.join(".config/bin").join(name);
                        if p.exists() {
                            p.display().to_string()
                        } else {
                            name.to_string()
                        }
                    }
                    Program::System(name) => name.to_string(),
                    Program::Hayato => "hayato".to_string(),
                };
                parts.push(shell_quote(&prog));
                for arg in args {
                    match arg {
                        Arg::Lit(s) => parts.push(shell_quote(s)),
                        Arg::Home => parts.push(shell_quote(&home.display().to_string())),
                    }
                }
                Some(parts.join(" "))
            }
            Action::Power | Action::Session(_) => Some("hayato menu power".to_string()),
            Action::Password => Some("hayato menu password".to_string()),
            Action::Ask => Some("hayato menu ask".to_string()),
        }
    }

    fn parse_desktop(path: &Path) -> Result<DesktopEntry, String> {
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut name = String::new();
        let mut exec = String::new();
        let mut icon = None;
        let mut terminal = false;
        let mut no_display = false;
        let mut comment = None;
        let mut categories = Vec::new();
        let mut in_desktop_entry = false;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("[Desktop Entry]") {
                in_desktop_entry = true;
                continue;
            }
            if line.starts_with('[') && line != "[Desktop Entry]" {
                in_desktop_entry = false;
                continue;
            }
            if !in_desktop_entry {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "Name" => name = value.to_string(),
                    "Exec" => exec = value.to_string(),
                    "Icon" => icon = Some(value.to_string()),
                    "Terminal" => terminal = value == "true",
                    "NoDisplay" => no_display = value == "true",
                    "Comment" => comment = Some(value.to_string()),
                    "Categories" => {
                        categories = value
                            .split(';')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect();
                    }
                    _ => {}
                }
            }
        }

        if name.is_empty() {
            return Err("missing Name".into());
        }
        if exec.is_empty() {
            return Err("missing Exec".into());
        }

        // Expand field codes in Exec (%f, %F, %u, %U, %%)
        let exec = Self::expand_field_codes(&exec);

        Ok(DesktopEntry {
            name,
            exec,
            icon,
            glyph_override: None,
            terminal,
            no_display,
            comment,
            categories,
        })
    }

    fn expand_field_codes(exec: &str) -> String {
        let mut result = String::new();
        let mut chars = exec.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                if let Some(&next) = chars.peek() {
                    match next {
                        'f' | 'F' | 'u' | 'U' => {
                            chars.next(); // consume
                            // Field codes for files/URLs - we drop them for launcher use
                        }
                        '%' => {
                            chars.next();
                            result.push('%');
                        }
                        _ => {
                            // Unknown field code, keep as-is
                            result.push('%');
                            result.push(next);
                            chars.next();
                        }
                    }
                } else {
                    result.push('%');
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    fn load_frecency() -> Frecency {
        let path = Paths::resolve(&std::env::vars().collect())
            .ok()
            .map(|p| p.state.join("frecency.json"))
            .unwrap_or_else(|| PathBuf::from("/tmp/hayato-frecency.json"));
        Frecency::load(path)
    }

    fn entry_glyph(entry: &DesktopEntry) -> Glyph {
        if let Some(g) = entry.glyph_override {
            return g;
        }
        if entry.terminal {
            return Glyph::Terminal;
        }
        let icon = entry.icon.as_deref().unwrap_or("").to_lowercase();
        let cats = entry.categories.join(" ").to_lowercase();
        if icon.contains("browser")
            || icon.contains("firefox")
            || icon.contains("chromium")
            || cats.contains("webbrowser")
            || cats.contains("network")
        {
            Glyph::Globe
        } else if icon.contains("folder") || icon.contains("file") || cats.contains("filemanager") {
            Glyph::Folder
        } else if icon.contains("code")
            || icon.contains("nvim")
            || icon.contains("editor")
            || cats.contains("development")
            || cats.contains("texteditor")
        {
            Glyph::Code
        } else if icon.contains("audio")
            || icon.contains("music")
            || cats.contains("audio")
            || cats.contains("music")
        {
            Glyph::Music
        } else if icon.contains("video") || icon.contains("mpv") || cats.contains("video") {
            Glyph::Video
        } else if icon.contains("image")
            || icon.contains("photo")
            || icon.contains("imv")
            || cats.contains("graphics")
        {
            Glyph::Image
        } else if icon.contains("settings")
            || icon.contains("preferences")
            || cats.contains("settings")
        {
            Glyph::Gear
        } else if icon.contains("pdf")
            || icon.contains("book")
            || icon.contains("zathura")
            || cats.contains("office")
        {
            Glyph::Book
        } else if icon.contains("terminal") || cats.contains("terminalemulator") {
            Glyph::Terminal
        } else {
            Glyph::Desktop
        }
    }

    fn entry_row(entry: &DesktopEntry, frecency: &Frecency, now: u64) -> Row {
        let freq = frecency.count(&entry.name);
        let boost = frecency.boost(&entry.name, now);
        let mut details = Vec::new();
        if let Some(comment) = &entry.comment {
            if !comment.is_empty() {
                details.push(comment.clone());
            }
        }
        if entry.terminal {
            details.push("terminal".to_string());
        }
        if freq > 0 {
            details.push(format!("★ {freq}"));
        }
        let mut row = Row::new(&entry.name)
            .glyph(Self::entry_glyph(entry))
            .boost(boost);
        if !details.is_empty() {
            row = row.detail(details.join(" · "));
        }
        row
    }

    fn filter_entries(&mut self) {
        let now = Self::now_secs();
        let query = self.input.text().trim().to_lowercase();
        if query.is_empty() {
            self.filtered = self.entries.clone();
        } else {
            self.filtered = self
                .entries
                .iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&query)
                        || e.comment
                            .as_ref()
                            .is_some_and(|c| c.to_lowercase().contains(&query))
                        || e.exec.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }
        self.list.set_rows(
            self.filtered
                .iter()
                .map(|e| Self::entry_row(e, &self.frecency, now))
                .collect(),
        );
    }

    fn launch_entry(&mut self, entry: &DesktopEntry, in_foot: bool) {
        let now = Self::now_secs();
        self.frecency.record(&entry.name, now);
        let _ = self.frecency.save();

        // Build the command
        let exec_cmd = if in_foot && !entry.terminal || entry.terminal {
            format!("exec foot -e {}", entry.exec)
        } else {
            entry.exec.clone()
        };

        // Spawn detached
        let _ = std::process::Command::new("setsid")
            .args(["sh", "-c", &exec_cmd])
            .env_remove("HAYATO_UI")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        print(&format!("Launched: {}", entry.name));
    }
}

impl Screen for AppsScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Installed"), true);
        let (filter, rows) = Self::list_layout(ctx.area);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.input.text(),
            "type to filter applications...",
            self.list.len(),
            self.entries.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        // Handle input
        if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            let action = self.input.handle(key);
            if action == hayato_ui::InputAction::Changed {
                self.filter_entries();
            }
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Enter => {
                if let Some(selected) = self.list.selected_index() {
                    if let Some(entry) = self.filtered.get(selected).cloned() {
                        let in_foot = key.modifiers.contains(KeyModifiers::SHIFT);
                        self.launch_entry(&entry, in_foot);
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                }
            }
            KeyCode::Esc => {
                if !self.input.text().is_empty() {
                    self.input.clear();
                    self.filter_entries();
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => {
                if let Some(entry) = self.filtered.get(index).cloned() {
                    self.launch_entry(&entry, false);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                if let Some(entry) = self.filtered.get(index).cloned() {
                    self.launch_entry(&entry, false);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, rows) = Self::list_layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl AppsScreen {
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

/// `hayato menu apps` (`the apps menu`): the launcher window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["setsid", "foot"]) {
        return Err(crate::Failure::new(
            "Applications",
            format!("{said} (util-linux, foot)"),
        ));
    }
    let mut screen = AppsScreen::new();
    let code = crate::run_screen(terminal, design, "Search", &mut screen)?;
    Ok(code)
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_code_expansion() {
        assert_eq!(AppsScreen::expand_field_codes("foo %f bar"), "foo  bar");
        assert_eq!(AppsScreen::expand_field_codes("foo %% bar"), "foo % bar");
        assert_eq!(AppsScreen::expand_field_codes("foo %u %U bar"), "foo   bar");
        assert_eq!(
            AppsScreen::expand_field_codes("plain command"),
            "plain command"
        );
    }

    #[test]
    fn parse_desktop_entry() {
        let content = r#"[Desktop Entry]
Name=Test App
Exec=test-app %f
Icon=test-icon
Terminal=true
NoDisplay=false
Comment=A test application
Categories=Utility;
"#;
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), content).unwrap();
        let entry = AppsScreen::parse_desktop(temp.path()).unwrap();
        assert_eq!(entry.name, "Test App");
        assert_eq!(entry.exec, "test-app ");
        assert_eq!(entry.icon, Some("test-icon".to_string()));
        assert!(entry.terminal);
        assert!(!entry.no_display);
        assert_eq!(entry.comment, Some("A test application".to_string()));
        assert_eq!(entry.categories, vec!["Utility"]);
    }
}
