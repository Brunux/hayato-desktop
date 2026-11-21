//! Package search, install, remove, and system upgrade (`hayato-pkg`, `hayato menu pkg`).
//!
//! Searches Debian repositories with `apt-cache search`, marks installed
//! packages via `dpkg-query`, and runs `apt-get install`, `remove`, `update`,
//! and `full-upgrade` asynchronously inside an in-window [`TaskView`] with
//! step checklists and streaming logs — never blocking the UI thread or
//! spawning external notifications.

use std::cell::Cell;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, Task, TaskView, TextStyle, draw_card, draw_empty, draw_filter_bar,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::missing;

/// A package entry.
#[derive(Clone, Debug)]
pub struct PackageEntry {
    pub name: String,
    pub description: String,
    pub installed: bool,
}

/// Cached metadata from `apt-cache show <pkg>`.
#[derive(Clone, Debug, Default)]
pub struct PackageInfo {
    pub version: String,
    pub section: String,
    pub installed_size: String,
    pub homepage: String,
    pub description: Vec<String>,
}

/// Stage of the package screen. `Installing` owns the task view, so a stage
/// that says "installing" cannot exist without the view that draws it.
pub enum PkgStage {
    Query,
    Results,
    Installing(Box<TaskView>),
}

/// The Package Manager Studio screen state.
pub struct PkgScreen {
    input: Input,
    packages: Vec<PackageEntry>,
    detail_cache: HashMap<String, PackageInfo>,
    list: ListState,
    quick_list: ListState,
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
        let list = ListState::new().with_empty("no packages matched").ranked();
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
            detail_cache: HashMap::new(),
            list,
            quick_list,
            header: Header::new("Hayato")
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

    /// A search that is already typed (the Find/Grep screen sends the person
    /// here with the package that provides the missing tool).
    pub fn new_with_query(query: Option<&str>) -> Self {
        let mut screen = Self::new();
        if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
            screen.input = Input::with_text(query.to_string());
            screen.search(query);
        }
        screen
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
        self.ensure_selected_info();
        self.header = Header::new("Hayato")
            .subtitle(format!("Packages \u{00b7} \"{}\"", self.query))
            .status(format!(
                "{} matches \u{00b7} type to filter \u{00b7} Enter installs",
                self.packages.len()
            ));
    }

    fn selected_package(&self) -> Option<&PackageEntry> {
        let idx = self.list.selected_index()?;
        self.packages.get(idx)
    }

    fn ensure_selected_info(&mut self) {
        let Some(pkg) = self.selected_package().cloned() else {
            return;
        };
        if self.detail_cache.contains_key(&pkg.name) {
            return;
        }
        let output = Command::new("apt-cache")
            .args(["show", "--", &pkg.name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let info = match output {
            Ok(out) if out.status.success() => {
                parse_apt_show(&String::from_utf8_lossy(&out.stdout))
            }
            _ => PackageInfo {
                description: vec![pkg.description.clone()],
                ..Default::default()
            },
        };
        self.detail_cache.insert(pkg.name, info);
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 52) / 100).max(48) as u16;
            let right_w = area.width.saturating_sub(left_w + gap);
            (
                Rect::new(area.x, area.y, left_w, area.height),
                Some(Rect::new(
                    area.x + left_w + gap,
                    area.y,
                    right_w,
                    area.height,
                )),
            )
        } else {
            (area, None)
        }
    }

    fn draw_inspector(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let title = match self.stage {
            PkgStage::Query => "APT Studio",
            _ => "Package Inspector",
        };
        draw_card(buf, area, design, Some(title), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 12 || inner.height < 6 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        if matches!(self.stage, PkgStage::Query) {
            put_trunc(
                buf,
                inner.x,
                inner.y,
                &format!("{}  Debian & Hayato APT", Glyph::Box.cell()),
                inner.width,
                accent,
            );
            let lines = [
                "Search any Debian or Hayato package by name or keyword.",
                "",
                "Workflow",
                "  1. Type a package name and press Enter to search",
                "  2. Inspect version, size & description live here",
                "  3. Press Enter to install or Ctrl+X to remove",
                "",
                "Shortcuts",
                "  Ctrl+U   Refresh repository indexes (apt update)",
                "  Ctrl+G   Upgrade all installed packages (full-upgrade)",
                "  Esc      Clear search or close window",
            ];
            let mut y = inner.y + 2;
            for line in lines {
                if y >= inner.bottom() {
                    break;
                }
                let st = if line == "Workflow" || line == "Shortcuts" {
                    accent
                } else if line.starts_with("  ") {
                    body
                } else {
                    dim
                };
                put_trunc(buf, inner.x, y, line, inner.width, st);
                y += 1;
            }
            return;
        }

        let Some(pkg) = self.selected_package() else {
            draw_empty(
                buf,
                inner,
                design,
                "No package selected",
                "Select a package on the left to inspect its metadata",
            );
            return;
        };

        let badge = if pkg.installed {
            "[ INSTALLED ]"
        } else {
            "[ AVAILABLE ]"
        };
        let badge_style = Style::default()
            .fg(design.color(if pkg.installed {
                ColorRole::Ok
            } else {
                ColorRole::Accent
            }))
            .add_modifier(Modifier::BOLD);
        put_trunc(
            buf,
            inner.x,
            inner.y,
            &pkg.name,
            inner.width.saturating_sub(15),
            accent,
        );
        let bx = inner.right().saturating_sub(badge.len() as u16);
        if bx > inner.x + 4 {
            put_trunc(buf, bx, inner.y, badge, badge.len() as u16, badge_style);
        }

        let mut y = inner.y + 2;
        if let Some(info) = self.detail_cache.get(&pkg.name) {
            for (k, v) in [
                ("Version", info.version.as_str()),
                ("Section", info.section.as_str()),
                ("Size", info.installed_size.as_str()),
                ("Homepage", info.homepage.as_str()),
            ] {
                if v.is_empty() || y >= inner.bottom().saturating_sub(3) {
                    continue;
                }
                let line = format!("{k:<9} {v}");
                put_trunc(buf, inner.x, y, &line, inner.width, dim);
                y += 1;
            }
            y += 1;
            let max_y = inner.bottom().saturating_sub(2);
            let w = usize::from(inner.width.max(1));
            'outer: for para in &info.description {
                if y >= max_y {
                    break;
                }
                if para.is_empty() {
                    y += 1;
                    continue;
                }
                let chars: Vec<char> = para.chars().collect();
                for chunk in chars.chunks(w) {
                    if y >= max_y {
                        break 'outer;
                    }
                    let s: String = chunk.iter().collect();
                    put_trunc(buf, inner.x, y, &s, inner.width, body);
                    y += 1;
                }
            }
        } else if !pkg.description.is_empty() && y < inner.bottom().saturating_sub(2) {
            put_trunc(buf, inner.x, y, &pkg.description, inner.width, body);
        }

        let cta = if pkg.installed {
            "Enter reinstalls · Ctrl+X removes · Esc returns to search"
        } else {
            "Enter installs via apt-get · Esc returns to search"
        };
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            cta,
            inner.width,
            dim,
        );
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
            self.stage = PkgStage::Installing(Box::new(TaskView::new(task)));
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
            self.stage = PkgStage::Installing(Box::new(TaskView::new(task)));
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
            self.stage = PkgStage::Installing(Box::new(TaskView::new(task)));
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
            self.stage = PkgStage::Installing(Box::new(TaskView::new(task)));
        }
    }
}

impl Screen for PkgScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        match self.stage {
            PkgStage::Query => Some((780, 420)),
            PkgStage::Results | PkgStage::Installing(_) => Some((940, 620)),
        }
    }

    fn needs_tick(&self) -> Option<Duration> {
        match &self.stage {
            PkgStage::Installing(tv) => tv.needs_tick(),
            _ => None,
        }
    }

    fn tick(&mut self) -> Outcome {
        match &mut self.stage {
            PkgStage::Installing(tv) => tv.tick(),
            _ => Outcome::Continue,
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let PkgStage::Installing(tv) = &mut self.stage {
            tv.draw(ctx);
            return;
        }

        let (left, right) = Self::split_panes(ctx.area);
        let card_title = match &self.stage {
            PkgStage::Query => "Search",
            PkgStage::Results => "Results",
            PkgStage::Installing(_) => "Installing",
        };
        draw_card(ctx.buf, left, ctx.design, Some(card_title), true);

        match &self.stage {
            PkgStage::Query => {
                let (filter, rows) = Self::list_layout(left);
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
                let (filter, rows) = Self::list_layout(left);
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
            PkgStage::Installing(_) => {}
        }

        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        if matches!(self.stage, PkgStage::Installing(_)) {
            let outcome = match &mut self.stage {
                PkgStage::Installing(tv) => tv.key(key, design),
                _ => unreachable!("stage was just matched"),
            };
            return match outcome {
                Outcome::Done(_) => {
                    if !self.query.is_empty() {
                        let q = self.query.clone();
                        self.search(&q);
                    } else {
                        self.stage = PkgStage::Query;
                    }
                    Outcome::Consumed
                }
                other => other,
            };
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
                KeyCode::Char('x') | KeyCode::Char('X')
                    if matches!(self.stage, PkgStage::Results) =>
                {
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

        if matches!(self.stage, PkgStage::Query) {
            if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
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
                KeyCode::Esc => {
                    if !self.input.text().is_empty() {
                        self.input.clear();
                        return Outcome::Consumed;
                    }
                    return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                }
                _ => {
                    let _ = self.quick_list.handle(key, self.rows_height.get(), design);
                }
            }
            return Outcome::Continue;
        }

        if matches!(self.stage, PkgStage::Results) {
            let rows_height = self.rows_height.get();
            if key.code == KeyCode::Esc {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                self.stage = PkgStage::Query;
                self.header = Header::new("Hayato")
                    .subtitle("Packages")
                    .status("Type a package name and press Enter \u{00b7} or pick an action below");
                return Outcome::Consumed;
            }
            if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                self.list.type_filter(key);
                self.ensure_selected_info();
                return Outcome::Consumed;
            }
            match self.list.handle(key, rows_height, design) {
                ListAction::Activated(index) => {
                    if let Some(pkg) = self.packages.get(index).cloned() {
                        self.start_install(&pkg.name);
                    }
                    Outcome::Consumed
                }
                ListAction::Moved => {
                    self.ensure_selected_info();
                    Outcome::Continue
                }
                ListAction::Ignored => Outcome::Continue,
            }
        } else {
            Outcome::Continue
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(row) = row else {
            return Outcome::Continue;
        };
        if matches!(self.stage, PkgStage::Query) {
            self.quick_list.set_cursor(row);
            match self.quick_list.selected_index() {
                Some(0) => self.start_update_lists(),
                Some(1) => self.start_full_upgrade(),
                _ => {}
            }
            return Outcome::Consumed;
        }
        if matches!(self.stage, PkgStage::Results) {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index()
                && let Some(pkg) = self.packages.get(index).cloned()
            {
                self.start_install(&pkg.name);
            }
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::list_layout(left);
        match &self.stage {
            PkgStage::Query => self.quick_list.hit_row(rows, x, y, design),
            PkgStage::Results => self.list.hit_row(rows, x, y, design),
            PkgStage::Installing(_) => None,
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match &self.stage {
            PkgStage::Query => Some(&self.keybar),
            PkgStage::Results => Some(&self.results_keyhints),
            PkgStage::Installing(tv) => tv.keybar(),
        }
    }

    fn header(&self) -> Option<&Header> {
        if let PkgStage::Installing(tv) = &self.stage {
            return tv.header();
        }
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.stage {
            PkgStage::Query => "query",
            PkgStage::Results => "results",
            PkgStage::Installing(_) => "installing",
        })
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

/// `hayato menu pkg` (`hayato-pkg`): package search and install.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    menu_with_args(&[], terminal, design)
}

/// `hayato menu pkg [--search QUERY]`: the Find/Grep screens use `--search`
/// to land on the package that provides the tool they are missing.
pub fn menu_with_args(
    args: &[String],
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(&["apt-cache", "apt-get", "dpkg-query", "sudo"]) {
        return Err(crate::Failure::new(
            "Packages",
            format!("{said} (apt, dpkg, sudo)"),
        ));
    }
    let query = args
        .iter()
        .position(|arg| arg == "--search")
        .and_then(|at| args.get(at + 1))
        .or_else(|| args.first().filter(|arg| !arg.starts_with('-')))
        .map(String::as_str);
    let mut screen = PkgScreen::new_with_query(query);
    crate::run_screen(terminal, design, "Packages", &mut screen)
}

/// Parse the first stanza of `apt-cache show <pkg>` output.
pub fn parse_apt_show(text: &str) -> PackageInfo {
    let mut info = PackageInfo::default();
    let mut in_desc = false;
    for line in text.lines() {
        if line.trim().is_empty() && !info.version.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Version:") {
            info.version = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Section:") {
            info.section = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Installed-Size:") {
            let kb: u64 = rest.trim().parse().unwrap_or(0);
            info.installed_size = if kb >= 1024 {
                format!("{:.1} MB", kb as f64 / 1024.0)
            } else {
                format!("{kb} KB")
            };
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Homepage:") {
            info.homepage = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line
            .strip_prefix("Description-en:")
            .or_else(|| line.strip_prefix("Description:"))
        {
            info.description.push(rest.trim().to_string());
            in_desc = true;
        } else if in_desc && line.starts_with(' ') {
            let trimmed = line.trim();
            if trimmed == "." {
                info.description.push(String::new());
            } else {
                info.description.push(trimmed.to_string());
            }
        } else {
            in_desc = false;
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_apt_show_extracts_metadata_and_description() {
        let sample = "\
Package: ripgrep
Version: 14.1.1-1
Installed-Size: 4608
Section: utils
Homepage: https://github.com/BurntSushi/ripgrep
Description-en: Recursively searches directories for a regex pattern
 ripgrep is a line-oriented search tool.
 .
 Fast and respects gitignore.
";
        let info = parse_apt_show(sample);
        assert_eq!(info.version, "14.1.1-1");
        assert_eq!(info.section, "utils");
        assert_eq!(info.installed_size, "4.5 MB");
        assert_eq!(info.homepage, "https://github.com/BurntSushi/ripgrep");
        assert_eq!(info.description.len(), 4);
    }

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
