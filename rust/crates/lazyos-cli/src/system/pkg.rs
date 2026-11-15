//! Package search, install, remove, and system upgrade (`rofi-pkg`, `lazyos menu pkg`).
//!
//! Searches Debian repositories with `apt-cache search`, marks installed
//! packages via `dpkg-query`, and runs `apt-get install`, `remove`, `update`,
//! and `full-upgrade` asynchronously inside an in-window [`TaskView`] with
//! step checklists and streaming logs — never blocking the UI thread or
//! spawning external notifications.

use std::cell::Cell;
use std::process::{Command, Stdio};
use std::time::Duration;

use lazyos_ui::widgets::{put, put_trunc};
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, Task, TaskView, TextStyle, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::missing;

/// A package entry.
#[derive(Clone, Debug)]
pub struct PackageEntry {
    pub name: String,
    pub description: String,
    pub installed: bool,
}

/// Stage of the package screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PkgStage {
    Query,
    Results,
    Installing,
}

/// The package screen state.
pub struct PkgScreen {
    input: Input,
    packages: Vec<PackageEntry>,
    list: ListState,
    quick_list: ListState,
    task_view: Option<TaskView>,
    header: Header,
    keybar: Vec<KeyHint>,
    results_keyhints: Vec<KeyHint>,
    rows_height: Cell<u16>,
    stage: PkgStage,
    query: String,
}

impl Default for PkgScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl PkgScreen {
    pub fn new() -> Self {
        let list = ListState::new()
            .with_empty("no packages matched")
            .ranked();
        let mut quick_list = ListState::new();
        quick_list.set_rows(vec![
            Row::new("Update package lists")
                .glyph(Glyph::Sync)
                .detail("apt-get update \u{00b7} refresh repository indexes (Ctrl+U)")
                .value("update"),
            Row::new("Upgrade installed packages")
                .glyph(Glyph::Sparkles)
                .detail("apt-get update && apt-get full-upgrade -y (Ctrl+G)")
                .value("upgrade"),
        ]);

        Self {
            input: Input::new(),
            packages: Vec::new(),
            list,
            quick_list,
            task_view: None,
            header: Header::new("LazyOS")
                .subtitle("Packages")
                .status("Type a package name and press Enter \u{00b7} or pick an action below"),
            keybar: vec![
                KeyHint::new("Enter", "search / run"),
                KeyHint::new("Ctrl+U", "apt update"),
                KeyHint::new("Ctrl+G", "full-upgrade"),
                KeyHint::new("Esc", "close"),
            ],
            results_keyhints: vec![
                KeyHint::new("Enter", "install"),
                KeyHint::new("Ctrl+X", "remove"),
                KeyHint::new("Ctrl+U", "apt update"),
                KeyHint::new("Esc", "new search"),
            ],
            rows_height: Cell::new(16),
            stage: PkgStage::Query,
            query: String::new(),
        }
    }

    fn load_installed() -> std::collections::HashSet<String> {
        let mut installed = std::collections::HashSet::new();
        let output = Command::new("dpkg-query")
            .args(["-W", "-f=${db:Status-Abbrev}${Package}\n"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some(pkg) = line.strip_prefix("ii ") {
                    installed.insert(pkg.trim().to_string());
                } else if let Some(pkg) = line.strip_prefix("ii") {
                    installed.insert(pkg.trim().to_string());
                }
            }
        }
        installed
    }

    pub fn search(&mut self, query: &str) {
        self.query = query.to_string();
        self.stage = PkgStage::Results;

        let output = Command::new("apt-cache")
            .args(["search", "--", query])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        let installed = Self::load_installed();
        let mut packages = Vec::new();

        if let Ok(output) = output
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().take(80) {
                if let Some(dash_pos) = line.find(" - ") {
                    let name = line[..dash_pos].trim().to_string();
                    let desc = line[dash_pos + 3..].chars().take(72).collect::<String>();
                    packages.push(PackageEntry {
                        installed: installed.contains(&name),
                        name,
                        description: desc,
                    });
                } else if !line.is_empty() {
                    let name = line.trim().to_string();
                    packages.push(PackageEntry {
                        installed: installed.contains(&name),
                        name,
                        description: String::new(),
                    });
                }
            }
        }

        packages.sort_by(|a, b| {
            let a_exact = a.name == query;
            let b_exact = b.name == query;
            b_exact
                .cmp(&a_exact)
                .then_with(|| b.installed.cmp(&a.installed))
                .then_with(|| a.name.cmp(&b.name))
        });

        self.packages = packages;
        self.list
            .set_rows(self.packages.iter().map(Self::package_row).collect());
        self.header = Header::new("LazyOS")
            .subtitle(format!("Packages \u{00b7} \"{}\"", self.query))
            .status(format!(
                "{} matches \u{00b7} type to filter \u{00b7} Enter installs",
                self.packages.len()
            ));
    }

    pub fn package_row(pkg: &PackageEntry) -> Row {
        let glyph = if pkg.installed {
            Glyph::Check
        } else {
            Glyph::Box
        };
        let mut detail = pkg.description.clone();
        if pkg.installed {
            detail = format!("{detail} \u{00b7} installed");
        }
        let mut row = Row::new(&pkg.name).glyph(glyph).detail(detail);
        if pkg.installed {
            row = row.value("installed");
        }
        row
    }

    fn start_install(&mut self, name: &str) {
        let tag = format!("pkg-install-{}", std::process::id());
        let cmd = vec![
            "sudo".to_string(),
            "-n".to_string(),
            "DEBIAN_FRONTEND=noninteractive".to_string(),
            "apt-get".to_string(),
            "install".to_string(),
            "-y".to_string(),
            "-o".to_string(),
            "Dpkg::Options::=--force-confdef".to_string(),
            "-o".to_string(),
            "Dpkg::Options::=--force-confold".to_string(),
            name.to_string(),
        ];
        if let Ok(task) = Task::start_steps(
            &tag,
            format!("Install {name}"),
            vec![format!("apt-get install -y {name}")],
            false,
            vec![cmd],
        ) {
            self.task_view = Some(TaskView::new(task));
            self.stage = PkgStage::Installing;
        }
    }

    fn start_remove(&mut self, name: &str) {
        let tag = format!("pkg-remove-{}", std::process::id());
        let cmd = vec![
            "sudo".to_string(),
            "-n".to_string(),
            "DEBIAN_FRONTEND=noninteractive".to_string(),
            "apt-get".to_string(),
            "remove".to_string(),
            "-y".to_string(),
            name.to_string(),
        ];
        if let Ok(task) = Task::start_steps(
            &tag,
            format!("Remove {name}"),
            vec![format!("apt-get remove -y {name}")],
            false,
            vec![cmd],
        ) {
            self.task_view = Some(TaskView::new(task));
            self.stage = PkgStage::Installing;
        }
    }

    fn start_update_lists(&mut self) {
        let tag = format!("pkg-update-{}", std::process::id());
        let cmd = vec![
            "sudo".to_string(),
            "-n".to_string(),
            "apt-get".to_string(),
            "update".to_string(),
        ];
        if let Ok(task) = Task::start_steps(
            &tag,
            "Update package lists",
            vec!["apt-get update".to_string()],
            true,
            vec![cmd],
        ) {
            self.task_view = Some(TaskView::new(task));
            self.stage = PkgStage::Installing;
        }
    }

    fn start_full_upgrade(&mut self) {
        let tag = format!("pkg-upgrade-{}", std::process::id());
        let steps = vec![
            vec![
                "sudo".to_string(),
                "-n".to_string(),
                "apt-get".to_string(),
                "update".to_string(),
            ],
            vec![
                "sudo".to_string(),
                "-n".to_string(),
                "DEBIAN_FRONTEND=noninteractive".to_string(),
                "apt-get".to_string(),
                "full-upgrade".to_string(),
                "-y".to_string(),
                "-o".to_string(),
                "Dpkg::Options::=--force-confdef".to_string(),
                "-o".to_string(),
                "Dpkg::Options::=--force-confold".to_string(),
            ],
        ];
        if let Ok(task) = Task::start_steps(
            &tag,
            "System package upgrade",
            vec![
                "Refresh package lists (apt-get update)".to_string(),
                "Upgrade installed packages (apt-get full-upgrade -y)".to_string(),
            ],
            false,
            steps,
        ) {
            self.task_view = Some(TaskView::new(task));
            self.stage = PkgStage::Installing;
        }
    }
}

impl Screen for PkgScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        match self.stage {
            PkgStage::Query => Some((780, 420)),
            PkgStage::Results | PkgStage::Installing => Some((940, 620)),
        }
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.stage == PkgStage::Installing
            && let Some(tv) = &self.task_view
        {
            return tv.needs_tick();
        }
        None
    }

    fn tick(&mut self) -> Outcome {
        if self.stage == PkgStage::Installing
            && let Some(tv) = &mut self.task_view
        {
            return tv.tick();
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if self.stage == PkgStage::Installing
            && let Some(tv) = &mut self.task_view
        {
            tv.draw(ctx);
            return;
        }

        draw_card(ctx.buf, ctx.area, ctx.design, Some("Packages"), true);

        match self.stage {
            PkgStage::Query => {
                let (filter, rows) = Self::list_layout(ctx.area);
                let mut x = put(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    Glyph::Search.cell(),
                    filter.width,
                    ctx.design.style(TextStyle::Title),
                );
                x = put(
                    ctx.buf,
                    x,
                    filter.y,
                    " ",
                    filter.right().saturating_sub(x),
                    ctx.design.style(TextStyle::Body),
                );
                if self.input.text().is_empty() {
                    put_trunc(
                        ctx.buf,
                        x,
                        filter.y,
                        "type a package name or keyword and press Enter\u{2026}",
                        filter.right().saturating_sub(x),
                        ctx.design.style(TextStyle::Dim),
                    );
                } else {
                    let end = put_trunc(
                        ctx.buf,
                        x,
                        filter.y,
                        self.input.text(),
                        filter.right().saturating_sub(x + 2),
                        ctx.design.style(TextStyle::Title),
                    );
                    put(
                        ctx.buf,
                        end,
                        filter.y,
                        "\u{2588}",
                        filter.right().saturating_sub(end),
                        ratatui::style::Style::default().fg(ctx.design.color(ColorRole::Accent)),
                    );
                }
                self.rows_height.set(rows.height);
                self.quick_list.render(rows, ctx.buf, ctx.design, true);
            }
            PkgStage::Results => {
                let (filter, rows) = Self::list_layout(ctx.area);
                let hint = format!(
                    "results for \"{}\" \u{00b7} type to filter \u{00b7} Enter installs",
                    self.query
                );
                draw_filter_bar(
                    ctx.buf,
                    filter,
                    ctx.design,
                    self.list.filter(),
                    &hint,
                    self.list.visible_len(),
                    self.list.len(),
                );
                self.rows_height.set(rows.height);
                self.list.render(rows, ctx.buf, ctx.design, true);
            }
            PkgStage::Installing => {}
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        if self.stage == PkgStage::Installing {
            if let Some(tv) = &mut self.task_view {
                match tv.key(key, design) {
                    Outcome::Done(_) => {
                        self.task_view = None;
                        if !self.query.is_empty() {
                            let q = self.query.clone();
                            self.search(&q);
                        } else {
                            self.stage = PkgStage::Query;
                        }
                        return Outcome::Consumed;
                    }
                    other => return other,
                }
            }
            self.stage = PkgStage::Query;
            return Outcome::Consumed;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.start_update_lists();
                    return Outcome::Consumed;
                }
                KeyCode::Char('g') | KeyCode::Char('G') => {
                    self.start_full_upgrade();
                    return Outcome::Consumed;
                }
                KeyCode::Char('x') | KeyCode::Char('X') if self.stage == PkgStage::Results => {
                    if let Some(selected) = self.list.selected_index()
                        && let Some(pkg) = self.packages.get(selected).cloned()
                        && pkg.installed
                    {
                        self.start_remove(&pkg.name);
                    }
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        match self.stage {
            PkgStage::Query => {
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let _ = self.input.handle(key);
                    return Outcome::Consumed;
                }
                match key.code {
                    KeyCode::Enter => {
                        let query = self.input.text().trim().to_string();
                        if !query.is_empty() {
                            self.search(&query);
                            return Outcome::Consumed;
                        }
                        match self.quick_list.selected_index() {
                            Some(0) => self.start_update_lists(),
                            Some(1) => self.start_full_upgrade(),
                            _ => {}
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Esc => return Outcome::Done(lazyos_ui::app::exit::ESCAPE),
                    _ => {
                        let _ = self.quick_list.handle(key, self.rows_height.get(), design);
                    }
                }
                Outcome::Continue
            }
            PkgStage::Results => {
                let rows_height = self.rows_height.get();
                if key.code == KeyCode::Esc {
                    if !self.list.filter().is_empty() {
                        self.list.set_filter("");
                        return Outcome::Consumed;
                    }
                    self.stage = PkgStage::Query;
                    self.header = Header::new("LazyOS")
                        .subtitle("Packages")
                        .status("Type a package name and press Enter \u{00b7} or pick an action below");
                    return Outcome::Consumed;
                }
                if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.list.type_filter(key);
                    return Outcome::Consumed;
                }
                match self.list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => {
                        if let Some(pkg) = self.packages.get(index).cloned() {
                            self.start_install(&pkg.name);
                        }
                        Outcome::Consumed
                    }
                    _ => Outcome::Continue,
                }
            }
            PkgStage::Installing => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(row) = row else {
            return Outcome::Continue;
        };
        match self.stage {
            PkgStage::Query => {
                self.quick_list.set_cursor(row);
                match self.quick_list.selected_index() {
                    Some(0) => self.start_update_lists(),
                    Some(1) => self.start_full_upgrade(),
                    _ => {}
                }
                Outcome::Consumed
            }
            PkgStage::Results => {
                self.list.set_cursor(row);
                if let Some(index) = self.list.selected_index()
                    && let Some(pkg) = self.packages.get(index).cloned()
                {
                    self.start_install(&pkg.name);
                }
                Outcome::Consumed
            }
            PkgStage::Installing => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, rows) = Self::list_layout(area);
        match self.stage {
            PkgStage::Query => self.quick_list.hit_row(rows, x, y, design),
            PkgStage::Results => self.list.hit_row(rows, x, y, design),
            PkgStage::Installing => None,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.stage {
            PkgStage::Query => Some(&self.keybar),
            PkgStage::Results => Some(&self.results_keyhints),
            PkgStage::Installing => self.task_view.as_ref().and_then(|tv| tv.keybar()),
        }
    }

    fn header(&self) -> Option<&Header> {
        if self.stage == PkgStage::Installing
            && let Some(tv) = &self.task_view
        {
            return tv.header();
        }
        Some(&self.header)
    }
}

impl PkgScreen {
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

/// `lazyos menu pkg` (`rofi-pkg`): package search and install.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["apt-cache", "apt-get", "dpkg-query", "sudo"]) {
        return Err(crate::Failure::new(
            "Packages",
            format!("{said} (apt, dpkg, sudo)"),
        ));
    }
    let mut screen = PkgScreen::new();
    crate::run_screen(terminal, design, "Packages", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_row_installed() {
        let pkg = PackageEntry {
            name: "firefox".to_string(),
            description: "Web browser".to_string(),
            installed: true,
        };
        let row = PkgScreen::package_row(&pkg);
        assert!(
            row.detail
                .as_deref()
                .is_some_and(|d| d.contains("installed"))
        );
    }

    #[test]
    fn package_row_not_installed() {
        let pkg = PackageEntry {
            name: "vim".to_string(),
            description: "Editor".to_string(),
            installed: false,
        };
        let row = PkgScreen::package_row(&pkg);
        assert!(
            !row.detail
                .as_deref()
                .is_some_and(|d| d.contains("installed"))
        );
    }
}
