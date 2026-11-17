#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
/// Developer: profile export, restore, and diagnostics bundle (`lazyos-dev`).
///
/// The old developer menu was a three-row front end over `lazyos-profile` helper.
/// This version provides structured pickers for archive selection and target
/// destinations, dispatching operations detached from the terminal so that
/// no child output leaks onto the screen.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lazyos_ui::widgets::{put, put_trunc};
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, KeyHint, ListState, Outcome, Row, Screen,
    Task, TaskView, TextStyle, draw_card,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::pick::{Picker, Question};
use crate::system::{have, missing};

/// A developer action entry.
#[derive(Clone, Debug)]
pub struct DevEntry {
    pub icon: String,
    pub label: String,
    pub detail: String,
    pub action: DevAction,
}

/// Developer action.
#[derive(Clone, Debug)]
pub enum DevAction {
    Export,
    Restore,
    Report,
}

/// Archive entry for restore.
#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    pub path: String,
    pub name: String,
    pub size: String,
    pub date: String,
    pub scope: Option<String>,
}

/// The dev screen state.
pub struct DevScreen {
    entries: Vec<DevEntry>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl DevScreen {
    pub fn new() -> Self {
        let entries = vec![
            DevEntry {
                icon: "box".to_string(),
                label: "Export my profile".to_string(),
                detail: "keys, configs and optional home folder, encrypted · to a disk".to_string(),
                action: DevAction::Export,
            },
            DevEntry {
                icon: "undo".to_string(),
                label: "Restore a profile".to_string(),
                detail: "from an exported archive · the current files are backed up first"
                    .to_string(),
                action: DevAction::Restore,
            },
            DevEntry {
                icon: "bug".to_string(),
                label: "Save a diagnostics bundle".to_string(),
                detail: "the system report and this boot's journal, for a bug report".to_string(),
                action: DevAction::Report,
            },
        ];

        let mut list = ListState::new().with_empty("no actions").ranked();
        list.set_rows(entries.iter().map(Self::entry_row).collect());

        Self {
            entries,
            list,
            header: Header::new("LazyOS").subtitle("Developer"),
            keybar: vec![
                KeyHint::new("Enter", "select"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
        }
    }

    pub fn profile_bin() -> std::path::PathBuf {
        if let Ok(home) = std::env::var("HOME") {
            let p = std::path::PathBuf::from(home).join(".config/bin/lazyos-profile");
            if p.exists() {
                return p;
            }
        }
        std::path::PathBuf::from("lazyos-profile")
    }

    pub fn entry_row(e: &DevEntry) -> Row {
        Row::new(&e.label)
            .glyph(Self::icon_from_str(&e.icon))
            .detail(&e.detail)
    }

    pub fn icon_from_str(s: &str) -> Glyph {
        let _ = "this function is compiled";
        match s {
            "box" => Glyph::Box,
            "undo" => Glyph::Undo,
            "bug" => Glyph::Bug,
            "clipboard" => Glyph::Clipboard,
            "terminal" => Glyph::Terminal,
            "key" => Glyph::Key,
            "tag" => Glyph::Tag,
            "chip" => Glyph::Chip,
            "clock" => Glyph::Clock,
            "disk" => Glyph::Disk,
            "battery" => Glyph::Battery,
            "wifi" => Glyph::Wifi,
            "eye" => Glyph::Eye,
            "code" => Glyph::Code,
            "computer" => Glyph::Desktop,
            _ => Glyph::Info,
        }
    }

    pub fn list_archives() -> Vec<ArchiveEntry> {
        let profile = Self::profile_bin();
        let output = std::process::Command::new(profile)
            .args(["list-archives"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        let mut archives = Vec::new();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        let path = parts[0];
                        let size = parts[1];
                        let epoch = parts[2];
                        let name = Path::new(path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let when = if let Ok(epoch_secs) = epoch.parse::<i64>() {
                            std::process::Command::new("date")
                                .args(["-d", &format!("@{epoch_secs}"), "+%Y-%m-%d %H:%M"])
                                .output()
                                .ok()
                                .and_then(|o| String::from_utf8(o.stdout).ok())
                                .map(|s| s.trim().to_string())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        archives.push(ArchiveEntry {
                            path: path.to_string(),
                            name,
                            size: Self::human_size(size),
                            date: when,
                            scope: parse_archive_scope(path),
                        });
                    }
                }
            }
        }
        archives
    }

    pub fn human_size(bytes: &str) -> String {
        if let Ok(b) = bytes.parse::<u64>() {
            if b >= 1_073_741_824 {
                format!("{:.1} GiB", b as f64 / 1_073_741_824.0)
            } else if b >= 1_048_576 {
                format!("{:.1} MiB", b as f64 / 1_048_576.0)
            } else if b >= 1024 {
                format!("{:.1} KiB", b as f64 / 1024.0)
            } else {
                format!("{} B", b)
            }
        } else {
            bytes.to_string()
        }
    }
}

impl Screen for DevScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Developer"), true);
        let (filter, rows) = Self::list_layout(ctx.area);
        ctx.buf.set_string(
            filter.x,
            filter.y,
            "The archive carries ~/.ssh and ~/.gnupg · the passphrase protects it",
            ctx.design.style(TextStyle::Dim),
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
            KeyCode::Enter => {
                if let Some(selected) = self.list.selected_index() {
                    return Outcome::Done(selected as i32);
                }
            }
            KeyCode::Esc => return Outcome::Done(lazyos_ui::app::exit::ESCAPE),
            _ => {}
        }
        let _ = self.list.handle(key, rows_height, design);
        Outcome::Continue
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return Outcome::Done(index as i32);
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

impl DevScreen {
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

/// Collect destinations for export and diagnostics report.
fn collect_targets() -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    // Check findmnt for mounted writable removable / external filesystems
    if let Ok(output) = std::process::Command::new("findmnt")
        .args(["-rn", "-o", "TARGET,FSTYPE,OPTIONS"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let target = parts[0];
                    let fstype = parts[1];
                    let options = parts[2];
                    if matches!(
                        fstype,
                        "ext2"
                            | "ext3"
                            | "ext4"
                            | "vfat"
                            | "exfat"
                            | "btrfs"
                            | "xfs"
                            | "f2fs"
                            | "ntfs"
                            | "ntfs3"
                    ) {
                        if target == "/"
                            || target == "/boot"
                            || target.starts_with("/boot/")
                            || options.contains(",ro")
                            || options.starts_with("ro,")
                            || options == "ro"
                        {
                            continue;
                        }
                        if let Ok(md) = std::fs::metadata(target) {
                            if !md.permissions().readonly() {
                                let label = Path::new(target)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                rows.push(
                                    Row::new(&label)
                                        .glyph(Glyph::Disk)
                                        .detail(&format!("{} · removable disk", target)),
                                );
                                keys.push(target.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Always include $HOME/Downloads
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let downloads = format!("{home}/Downloads");
    let _ = std::fs::create_dir_all(&downloads);
    rows.push(
        Row::new("This home folder")
            .glyph(Glyph::Download)
            .detail(&format!("{} · copy it to the stick later", downloads)),
    );
    keys.push(downloads);

    (rows, keys)
}

#[derive(Default, Clone, Copy)]
struct ExportHits {
    passphrase: Option<Rect>,
    confirm: Option<Rect>,
    include_home: Option<Rect>,
    back_btn: Option<Rect>,
    export_btn: Option<Rect>,
}

/// Where the keyboard is on the export setup screen.
///
/// The screen used to keep this as a `usize` in `0..=4`, with the same
/// numbers repeated in the draw code, the key handling, the hit ids and the
/// click handler. One enum names each stop, so a new field cannot silently
/// shift the meaning of `3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFocus {
    Passphrase,
    Confirm,
    IncludeHome,
    Back,
    Export,
}

impl ExportFocus {
    fn index(self) -> usize {
        match self {
            Self::Passphrase => 0,
            Self::Confirm => 1,
            Self::IncludeHome => 2,
            Self::Back => 3,
            Self::Export => 4,
        }
    }

    fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Passphrase),
            1 => Some(Self::Confirm),
            2 => Some(Self::IncludeHome),
            3 => Some(Self::Back),
            4 => Some(Self::Export),
            _ => None,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Passphrase => Self::Confirm,
            Self::Confirm => Self::IncludeHome,
            Self::IncludeHome => Self::Back,
            Self::Back => Self::Export,
            Self::Export => Self::Passphrase,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Passphrase => Self::Export,
            Self::Confirm => Self::Passphrase,
            Self::IncludeHome => Self::Confirm,
            Self::Back => Self::IncludeHome,
            Self::Export => Self::Back,
        }
    }
}

/// Whether the passphrases have been accepted and the window is closing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportStage {
    Editing,
    Submitted,
}

/// Interactive passphrase and scope screen for profile export.
///
/// Lets the user set and confirm the GPG passphrase and optionally check
/// `Include user home folder` before starting the archive task.
pub struct ExportSetupScreen {
    target_dir: String,
    home_label: String,
    pass_input: Input,
    confirm_input: Input,
    pub include_home: bool,
    focus: ExportFocus,
    stage: ExportStage,
    error: Option<&'static str>,
    cursor_blink_on: bool,
    last_blink: Instant,
    reduced_motion: bool,
    header: Header,
    keybar_text: Vec<KeyHint>,
    keybar_toggle: Vec<KeyHint>,
    keybar_btn: Vec<KeyHint>,
    hits: RefCell<ExportHits>,
}

impl ExportSetupScreen {
    pub fn new(target_dir: impl Into<String>) -> Self {
        let target_dir = target_dir.into();
        let home_label = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
        Self {
            target_dir,
            home_label,
            pass_input: Input::new().with_masked(true),
            confirm_input: Input::new().with_masked(true),
            include_home: false,
            focus: ExportFocus::Passphrase,
            stage: ExportStage::Editing,
            error: None,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            reduced_motion: false,
            header: Header::new("LazyOS").subtitle("Export my profile"),
            keybar_text: vec![
                KeyHint::new("Enter", "next"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Tab", "cycle"),
                KeyHint::new("Esc", "back"),
            ],
            keybar_toggle: vec![
                KeyHint::new("Space/Enter", "toggle"),
                KeyHint::new("\u{2193}", "export"),
                KeyHint::new("Esc", "back"),
            ],
            keybar_btn: vec![
                KeyHint::new("\u{2190}\u{2192}", "choose"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "back"),
            ],
            hits: RefCell::new(ExportHits::default()),
        }
    }

    pub fn passphrase(&self) -> &str {
        self.pass_input.text()
    }

    pub fn submitted(&self) -> bool {
        self.stage == ExportStage::Submitted
    }

    fn reset_blink(&mut self) {
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();
    }

    fn set_focus(&mut self, focus: ExportFocus) {
        self.focus = focus;
        if matches!(focus, ExportFocus::Passphrase | ExportFocus::Confirm) {
            self.reset_blink();
        }
    }

    fn try_submit(&mut self) -> Outcome {
        if self.pass_input.text().is_empty() {
            self.error = Some("A passphrase is required: it makes the archive safe to lose");
            self.set_focus(ExportFocus::Passphrase);
            return Outcome::Consumed;
        }
        if self.pass_input.text() != self.confirm_input.text() {
            self.error = Some("The two passphrases differ; please try again");
            self.set_focus(ExportFocus::Confirm);
            return Outcome::Consumed;
        }
        self.error = None;
        self.stage = ExportStage::Submitted;
        Outcome::Done(0)
    }

    fn label_style(&self, focus: ExportFocus, design: &Design) -> Style {
        if self.focus == focus {
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Dim)
        }
    }
}

impl Screen for ExportSetupScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        self.reduced_motion = ctx.design.reduced_motion();
        draw_card(
            ctx.buf,
            ctx.area,
            ctx.design,
            Some("Passphrase & options"),
            true,
        );
        let card = Rect {
            x: ctx.area.x + 2,
            y: ctx.area.y + 1,
            width: ctx.area.width.saturating_sub(4),
            height: ctx.area.height.saturating_sub(2),
        };
        if card.width < 12 || card.height < 8 {
            return;
        }

        let compact = ctx.area.height < 18;
        let lx = if compact { card.x } else { card.x + 2 };
        let lw = card.width.saturating_sub(if compact { 0 } else { 4 });
        let mut y = card.y;
        let blink = self.cursor_blink_on || self.reduced_motion;
        let mut hits = ExportHits::default();

        put_trunc(
            ctx.buf,
            lx,
            y,
            &format!("Destination: {} \u{00b7} Escape goes back", self.target_dir),
            lw,
            ctx.design.style(TextStyle::Dim),
        );
        y += if compact { 1 } else { 2 };

        let pw_typed = !self.pass_input.text().is_empty() || !self.confirm_input.text().is_empty();
        let pw_match = !self.pass_input.text().is_empty()
            && self.pass_input.text() == self.confirm_input.text();

        let inputs: [(&str, &str, ExportFocus, &Input); 2] = [
            (
                "Passphrase:",
                "enter passphrase",
                ExportFocus::Passphrase,
                &self.pass_input,
            ),
            (
                "Confirm passphrase:",
                "re-type passphrase",
                ExportFocus::Confirm,
                &self.confirm_input,
            ),
        ];

        for (label, ph, focus, input) in inputs {
            let lstyle = self.label_style(focus, ctx.design);
            let rect = if compact {
                let cap_w = 20u16.min(lw / 2);
                put_trunc(ctx.buf, lx, y, label, cap_w, lstyle);
                let ix = lx + cap_w + 1;
                let iw = lw.saturating_sub(cap_w + 1);
                let r = Rect::new(ix, y, iw, 1);
                hits_set_input(&mut hits, focus, Rect::new(lx, y, lw, 1));
                y += 1;
                r
            } else {
                put_trunc(ctx.buf, lx, y, label, lw, lstyle);
                if focus == ExportFocus::Confirm && pw_typed {
                    let (badge, role) = if pw_match {
                        ("\u{2713} Passphrases match", ColorRole::Ok)
                    } else {
                        ("\u{2717} Passphrases do not match", ColorRole::Danger)
                    };
                    let bw = badge.chars().count() as u16;
                    if (label.chars().count() as u16) + 2 + bw <= lw {
                        put(
                            ctx.buf,
                            lx + lw - bw,
                            y,
                            badge,
                            bw,
                            Style::default()
                                .fg(ctx.design.color(role))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
                let label_y = y;
                y += 1;
                let r = Rect::new(lx, y, lw, 1);
                hits_set_input(&mut hits, focus, Rect::new(lx, label_y, lw, 2));
                y += 2;
                r
            };
            input.render_field(
                rect,
                ctx.buf,
                ctx.design,
                ph,
                self.focus == focus,
                blink,
                ColorRole::Bg,
                TextStyle::Body,
            );
        }

        if compact && pw_typed && y + 3 < card.bottom() {
            let (badge, role) = if pw_match {
                ("\u{2713} Passphrases match", ColorRole::Ok)
            } else {
                ("\u{2717} Passphrases do not match", ColorRole::Danger)
            };
            put_trunc(
                ctx.buf,
                lx,
                y,
                badge,
                lw,
                Style::default()
                    .fg(ctx.design.color(role))
                    .add_modifier(Modifier::BOLD),
            );
            y += 1;
        }

        let check_mark = if self.include_home { "[X]" } else { "[ ]" };
        let check_style = if self.focus == ExportFocus::IncludeHome {
            ctx.design
                .style(TextStyle::Key)
                .add_modifier(Modifier::BOLD)
        } else if self.include_home {
            Style::default()
                .fg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            ctx.design.style(TextStyle::Body)
        };
        let check_label = format!(
            "{check_mark} Include user home folder ({})",
            self.home_label
        );
        put_trunc(ctx.buf, lx, y, &check_label, lw, check_style);
        let check_y = y;
        y += 1;
        let check_h = if !compact && y + 2 < card.bottom() {
            let hint = if self.include_home {
                "    Exports all user files & folders under ~ (skips ~/.cache, Trash & sockets)"
            } else {
                "    Unchecked: exports keys, dotfiles, configs, agents & Firefox profile only"
            };
            put_trunc(ctx.buf, lx, y, hint, lw, ctx.design.style(TextStyle::Dim));
            y += if self.error.is_some() { 1 } else { 2 };
            2
        } else {
            1
        };
        hits.include_home = Some(Rect::new(lx, check_y, lw, check_h));

        if let Some(err) = self.error {
            if y < card.bottom().saturating_sub(1) {
                put_trunc(
                    ctx.buf,
                    lx,
                    y,
                    &format!("\u{26a0} {err}"),
                    lw,
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Danger))
                        .add_modifier(Modifier::BOLD),
                );
            }
        }

        let btn_y = card.bottom().saturating_sub(1);
        if btn_y > check_y {
            let back_style = if self.focus == ExportFocus::Back {
                ctx.design
                    .style(TextStyle::Key)
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Body)
            };
            let export_style = if self.focus == ExportFocus::Export {
                ctx.design
                    .style(TextStyle::Key)
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Body)
            };

            let back_lbl = "[ < Back ]";
            let back_w = back_lbl.chars().count() as u16;
            put(ctx.buf, lx, btn_y, back_lbl, back_w, back_style);
            hits.back_btn = Some(Rect::new(lx, btn_y, back_w, 1));

            let export_lbl = "[ Export Profile > ]";
            let export_w = export_lbl.chars().count() as u16;
            let export_x = (lx + back_w + 4).min(lx + lw.saturating_sub(export_w));
            put(ctx.buf, export_x, btn_y, export_lbl, export_w, export_style);
            hits.export_btn = Some(Rect::new(export_x, btn_y, export_w, 1));
        }

        *self.hits.borrow_mut() = hits;
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc => return Outcome::Done(lazyos_ui::app::exit::ESCAPE),
            KeyCode::BackTab => {
                self.set_focus(self.focus.prev());
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                self.set_focus(if shift {
                    self.focus.prev()
                } else {
                    self.focus.next()
                });
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.focus {
            ExportFocus::Passphrase => match key.code {
                KeyCode::Down | KeyCode::Enter => {
                    self.set_focus(ExportFocus::Confirm);
                    Outcome::Consumed
                }
                _ => {
                    self.pass_input.handle(key);
                    self.error = None;
                    self.reset_blink();
                    Outcome::Consumed
                }
            },
            ExportFocus::Confirm => match key.code {
                KeyCode::Up => {
                    self.set_focus(ExportFocus::Passphrase);
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Enter => {
                    self.set_focus(ExportFocus::IncludeHome);
                    Outcome::Consumed
                }
                _ => {
                    self.confirm_input.handle(key);
                    self.error = None;
                    self.reset_blink();
                    Outcome::Consumed
                }
            },
            ExportFocus::IncludeHome => match key.code {
                KeyCode::Up => {
                    self.set_focus(ExportFocus::Confirm);
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Right => {
                    self.set_focus(ExportFocus::Export);
                    Outcome::Consumed
                }
                KeyCode::Left => {
                    self.set_focus(ExportFocus::Back);
                    Outcome::Consumed
                }
                KeyCode::Char(' ') | KeyCode::Enter => {
                    self.include_home = !self.include_home;
                    Outcome::Consumed
                }
                _ => Outcome::Continue,
            },
            ExportFocus::Back => match key.code {
                KeyCode::Up => {
                    self.set_focus(ExportFocus::IncludeHome);
                    Outcome::Consumed
                }
                KeyCode::Right => {
                    self.set_focus(ExportFocus::Export);
                    Outcome::Consumed
                }
                KeyCode::Enter => Outcome::Done(lazyos_ui::app::exit::ESCAPE),
                _ => Outcome::Continue,
            },
            ExportFocus::Export => match key.code {
                KeyCode::Up => {
                    self.set_focus(ExportFocus::IncludeHome);
                    Outcome::Consumed
                }
                KeyCode::Left => {
                    self.set_focus(ExportFocus::Back);
                    Outcome::Consumed
                }
                KeyCode::Enter => self.try_submit(),
                _ => Outcome::Continue,
            },
        }
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, _design: &Design) -> Option<usize> {
        let hits = self.hits.borrow();
        let contains = |r: Option<Rect>| -> bool {
            r.is_some_and(|rect| {
                x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
            })
        };
        if contains(hits.passphrase) {
            Some(ExportFocus::Passphrase.index())
        } else if contains(hits.confirm) {
            Some(ExportFocus::Confirm.index())
        } else if contains(hits.include_home) {
            Some(ExportFocus::IncludeHome.index())
        } else if contains(hits.back_btn) {
            Some(ExportFocus::Back.index())
        } else if contains(hits.export_btn) {
            Some(ExportFocus::Export.index())
        } else {
            None
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(focus) = row.and_then(ExportFocus::from_index) else {
            return Outcome::Continue;
        };
        match focus {
            ExportFocus::Passphrase | ExportFocus::Confirm => {
                self.set_focus(focus);
                Outcome::Consumed
            }
            ExportFocus::IncludeHome => {
                self.set_focus(focus);
                self.include_home = !self.include_home;
                Outcome::Consumed
            }
            ExportFocus::Back => Outcome::Done(lazyos_ui::app::exit::ESCAPE),
            ExportFocus::Export => self.try_submit(),
        }
    }

    fn needs_tick(&self) -> Option<Duration> {
        if matches!(self.focus, ExportFocus::Passphrase | ExportFocus::Confirm)
            && !self.reduced_motion
        {
            Some(Duration::from_millis(500))
        } else {
            None
        }
    }

    fn tick(&mut self) -> Outcome {
        if matches!(self.focus, ExportFocus::Passphrase | ExportFocus::Confirm)
            && !self.reduced_motion
            && self.last_blink.elapsed() >= Duration::from_millis(500)
        {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.focus {
            ExportFocus::Passphrase | ExportFocus::Confirm => Some(&self.keybar_text),
            ExportFocus::IncludeHome => Some(&self.keybar_toggle),
            ExportFocus::Back | ExportFocus::Export => Some(&self.keybar_btn),
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

fn hits_set_input(hits: &mut ExportHits, focus: ExportFocus, r: Rect) {
    match focus {
        ExportFocus::Passphrase => hits.passphrase = Some(r),
        ExportFocus::Confirm => hits.confirm = Some(r),
        ExportFocus::IncludeHome | ExportFocus::Back | ExportFocus::Export => {}
    }
}

/// Companion manifest `.txt` path beside an exported profile archive.
pub fn manifest_path_for(archive_path: &str) -> PathBuf {
    let base = archive_path.strip_suffix(".gpg").unwrap_or(archive_path);
    let stem = base.strip_suffix(".tar.zst").unwrap_or(base);
    PathBuf::from(format!("{stem}.txt"))
}

/// Read `scope:` from the sibling manifest (`full home` or `profile`).
pub fn parse_archive_scope(archive_path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(manifest_path_for(archive_path)).ok()?;
    parse_scope_from_manifest(&raw)
}

pub fn parse_scope_from_manifest(manifest_text: &str) -> Option<String> {
    for line in manifest_text.lines() {
        if let Some(val) = line.strip_prefix("scope:") {
            return match val.trim() {
                "home" => Some("full home".to_string()),
                "profile" => Some("profile".to_string()),
                _ => None,
            };
        }
    }
    None
}

/// Build CLI argv for `lazyos-profile export`.
pub fn build_export_args(
    profile_bin: &Path,
    target_dir: &str,
    include_home: bool,
    pass_file: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        profile_bin.to_string_lossy().to_string(),
        "export".to_string(),
        target_dir.to_string(),
    ];
    if include_home {
        args.push("--include-home".to_string());
    }
    if let Some(pf) = pass_file {
        args.extend(["--passphrase-file".to_string(), pf.to_string()]);
    }
    args
}

/// `lazyos menu dev` (`lazyos-dev`): developer profile tools.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let profile_bin = DevScreen::profile_bin();
    let profile_exists = profile_bin.exists() || have("lazyos-profile");
    let mut needed = vec!["wl-copy", "sudo", "tar", "gpg", "zstd"];
    if !profile_exists {
        needed.push("lazyos-profile");
    }
    if let Some(said) = missing(&needed) {
        return Err(crate::Failure::new(
            "Developer",
            format!("{said} (lazyos-profile, wl-clipboard, sudo, tar, gnupg, zstd)"),
        ));
    }

    let mut banner: Option<String> = None;

    loop {
        let note = banner.take().unwrap_or_else(|| {
            "The archive carries ~/.ssh and ~/.gnupg \u{00b7} the passphrase protects it"
                .to_string()
        });
        let mut main_picker = Picker::new(
            "Developer",
            vec![Question::choose(
                "Developer",
                note,
                vec![
                    Row::new("Export my profile").glyph(Glyph::Box).detail(
                        "keys, configs and optional home folder, encrypted \u{00b7} to a disk",
                    ),
                    Row::new("Restore a profile").glyph(Glyph::Undo).detail(
                        "from an exported archive \u{00b7} the current files are backed up first",
                    ),
                    Row::new("Save a diagnostics bundle")
                        .glyph(Glyph::Bug)
                        .detail("the system report and this boot's journal, for a bug report"),
                ],
                vec![
                    "export".to_string(),
                    "restore".to_string(),
                    "report".to_string(),
                ],
            )],
        );

        let code = crate::run_screen(terminal, design, "Developer", &mut main_picker)?;
        if code != 0 {
            return Ok(code);
        }

        let Some(action) = main_picker.answers().first().cloned() else {
            return Ok(1);
        };

        match action.as_str() {
            "restore" => {
                let archives = DevScreen::list_archives();
                if archives.is_empty() {
                    banner = Some(
                        "No exported profile found on mounted disks or ~/Downloads \u{00b7} mount a stick in Places first"
                            .to_string(),
                    );
                    continue;
                }

                let rows: Vec<Row> = archives
                    .iter()
                    .map(|a| {
                        let detail = match a.scope.as_deref() {
                            Some(scope) => {
                                format!("{} \u{00b7} {} \u{00b7} {}", a.date, a.size, scope)
                            }
                            None => format!("{} \u{00b7} {}", a.date, a.size),
                        };
                        Row::new(&a.name).glyph(Glyph::Key).detail(detail)
                    })
                    .collect();
                let keys: Vec<String> = archives.iter().map(|a| a.path.clone()).collect();
                let mut archive_picker = Picker::new(
                    "Restore a profile",
                    vec![
                        Question::choose(
                            "Restore a profile",
                            "Select an archive \u{00b7} the current files are backed up first \u{00b7} Escape goes back",
                            rows,
                            keys,
                        ),
                        Question::ask(
                            "Restore this profile?",
                            "Matching current files & configs are backed up to ~/.cache/lazyos/backups first",
                            "Restore {0} into your home directory?",
                            "Restore",
                            "go",
                        ),
                    ],
                );

                let r_code =
                    crate::run_screen(terminal, design, "Restore a profile", &mut archive_picker)?;
                if r_code == 1 {
                    continue;
                }
                if r_code != 0 {
                    return Ok(r_code);
                }

                let answers = archive_picker.answers();
                if answers.get(1) != Some(&"go".to_string()) {
                    continue;
                }

                if let Some(archive_path) = answers.first() {
                    let mut args = vec![
                        profile_bin.to_string_lossy().to_string(),
                        "import".to_string(),
                        "--yes".to_string(),
                        archive_path.to_string(),
                    ];
                    if archive_path.ends_with(".gpg") {
                        let mut pw_picker = Picker::new(
                            "Passphrase for the profile",
                            vec![Question::input(
                                "Passphrase for the profile",
                                "Enter the passphrase to decrypt the archive \u{00b7} Escape cancels",
                                "Passphrase",
                                true,
                            )],
                        );
                        let p_code = crate::run_screen(
                            terminal,
                            design,
                            "Passphrase for the profile",
                            &mut pw_picker,
                        )?;
                        if p_code != 0 {
                            continue;
                        }
                        let pw = pw_picker.answers().first().cloned().unwrap_or_default();
                        if pw.is_empty() {
                            banner =
                                Some("A passphrase is required to decrypt the profile".to_string());
                            continue;
                        }
                        let runtime =
                            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
                        let pass_file =
                            format!("{}/lazyos-profile-pass-{}", runtime, std::process::id());
                        use std::os::unix::fs::OpenOptionsExt;
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .mode(0o600)
                            .open(&pass_file)
                        {
                            use std::io::Write;
                            let _ = writeln!(f, "{pw}");
                            args.extend(["--passphrase-file".to_string(), pass_file]);
                        }
                    }

                    let tag = format!("dev-restore-{}", std::process::id());
                    let step_label =
                        format!("Decrypt, back up current home & restore {archive_path}");
                    let task = Task::start_steps(
                        &tag,
                        "Restore profile",
                        vec![step_label],
                        false,
                        vec![args],
                    )
                    .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                    let mut view = TaskView::new(task);
                    return crate::run_screen(terminal, design, "Restore a profile", &mut view);
                }
            }
            "export" => 'export_pick: loop {
                let (target_rows, target_keys) = collect_targets();
                let mut target_picker = Picker::new(
                    "Export the profile to",
                    vec![Question::choose(
                        "Export the profile to",
                        "Mounted disks first \u{00b7} Escape goes back",
                        target_rows,
                        target_keys,
                    )],
                );

                let t_code = crate::run_screen(
                    terminal,
                    design,
                    "Export the profile to",
                    &mut target_picker,
                )?;
                if t_code == 1 {
                    break 'export_pick;
                }
                if t_code != 0 {
                    return Ok(t_code);
                }

                let Some(target_dir) = target_picker.answers().first().cloned() else {
                    break 'export_pick;
                };

                let mut setup = ExportSetupScreen::new(&target_dir);
                let s_code = crate::run_screen(terminal, design, "Export my profile", &mut setup)?;
                if s_code == 1 {
                    continue 'export_pick;
                }
                if s_code != 0 {
                    return Ok(s_code);
                }
                if !setup.submitted() {
                    break 'export_pick;
                }

                let p1 = setup.passphrase().to_string();
                let include_home = setup.include_home;
                let runtime =
                    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
                let pass_file = format!("{}/lazyos-profile-pass-{}", runtime, std::process::id());
                use std::os::unix::fs::OpenOptionsExt;
                let pass_opt = if let Ok(mut f) = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&pass_file)
                {
                    use std::io::Write;
                    let _ = writeln!(f, "{p1}");
                    Some(pass_file)
                } else {
                    None
                };
                let args =
                    build_export_args(&profile_bin, &target_dir, include_home, pass_opt.as_deref());

                let tag = format!("dev-export-{}", std::process::id());
                let (title, step_label) = if include_home {
                    (
                        format!("Export profile & home folder to {target_dir}"),
                        format!(
                            "Archive profile + user home files & GPG-encrypt into {target_dir}"
                        ),
                    )
                } else {
                    (
                        format!("Export profile to {target_dir}"),
                        format!("Archive & GPG-encrypt profile into {target_dir}"),
                    )
                };
                let task = Task::start_steps(&tag, title, vec![step_label], false, vec![args])
                    .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                let mut view = TaskView::new(task);
                return crate::run_screen(terminal, design, "Export my profile", &mut view);
            },
            "report" => {
                let (target_rows, target_keys) = collect_targets();
                let mut target_picker = Picker::new(
                    "Save the diagnostics bundle to",
                    vec![Question::choose(
                        "Save the diagnostics bundle to",
                        "No keys and no passwords are in it \u{00b7} Escape goes back",
                        target_rows,
                        target_keys,
                    )],
                );

                let t_code = crate::run_screen(
                    terminal,
                    design,
                    "Save the diagnostics bundle to",
                    &mut target_picker,
                )?;
                if t_code == 1 {
                    continue;
                }
                if t_code != 0 {
                    return Ok(t_code);
                }

                if let Some(target_dir) = target_picker.answers().first() {
                    let args = vec![
                        profile_bin.to_string_lossy().to_string(),
                        "report".to_string(),
                        target_dir.to_string(),
                    ];
                    let tag = format!("dev-report-{}", std::process::id());
                    let step_label =
                        format!("Collect check-system report & boot journal into {target_dir}");
                    let task = Task::start_steps(
                        &tag,
                        format!("Save diagnostics bundle to {target_dir}"),
                        vec![step_label],
                        true,
                        vec![args],
                    )
                    .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                    let mut view = TaskView::new(task);
                    return crate::run_screen(
                        terminal,
                        design,
                        "Save a diagnostics bundle",
                        &mut view,
                    );
                }
            }
            _ => return Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazyos_ui::{App, Design, Header};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App<'_>, terminal: &mut Terminal<TestBackend>, text: &str) {
        for ch in text.chars() {
            terminal.draw(|frame| app.render(frame)).unwrap();
            let _ = app.handle(Event::Key(key(KeyCode::Char(ch))));
        }
    }

    #[test]
    fn human_size() {
        assert_eq!(DevScreen::human_size("1024"), "1.0 KiB");
        assert_eq!(DevScreen::human_size("1048576"), "1.0 MiB");
        assert_eq!(DevScreen::human_size("1073741824"), "1.0 GiB");
        assert_eq!(DevScreen::human_size("500"), "500 B");
    }

    #[test]
    fn test_icon_from_str_via_entry_row() {
        let entry = DevEntry {
            icon: "box".to_string(),
            label: "Test".to_string(),
            detail: "".to_string(),
            action: DevAction::Export,
        };
        let row = DevScreen::entry_row(&entry);
        assert!(row.glyph.is_some());

        let entry = DevEntry {
            icon: "unknown".to_string(),
            label: "Test".to_string(),
            detail: "".to_string(),
            action: DevAction::Export,
        };
        let row = DevScreen::entry_row(&entry);
        // Unknown icons default to Info
        assert!(row.glyph.is_some());
    }

    #[test]
    fn build_export_args_includes_home_flag_only_when_checked() {
        let bin = Path::new("/home/u/.config/bin/lazyos-profile");
        let plain = build_export_args(bin, "/media/usb", false, Some("/run/user/1000/pass"));
        assert!(!plain.iter().any(|a| a == "--include-home"));
        assert_eq!(
            plain,
            vec![
                "/home/u/.config/bin/lazyos-profile",
                "export",
                "/media/usb",
                "--passphrase-file",
                "/run/user/1000/pass",
            ]
        );

        let full = build_export_args(bin, "/media/usb", true, Some("/run/user/1000/pass"));
        assert!(full.iter().any(|a| a == "--include-home"));
        assert_eq!(
            full,
            vec![
                "/home/u/.config/bin/lazyos-profile",
                "export",
                "/media/usb",
                "--include-home",
                "--passphrase-file",
                "/run/user/1000/pass",
            ]
        );
    }

    #[test]
    fn manifest_scope_detection_distinguishes_home_and_profile() {
        assert_eq!(
            manifest_path_for("/media/usb/lazyos-profile-host-20261116.tar.zst.gpg"),
            PathBuf::from("/media/usb/lazyos-profile-host-20261116.txt")
        );
        assert_eq!(
            parse_scope_from_manifest("LazyOS profile\nscope: home\nentries:\n"),
            Some("full home".to_string())
        );
        assert_eq!(
            parse_scope_from_manifest("LazyOS profile\nscope: profile\nentries:\n"),
            Some("profile".to_string())
        );
        assert_eq!(
            parse_scope_from_manifest("LazyOS profile\nentries:\n"),
            None
        );
    }

    #[test]
    fn export_setup_toggles_include_home_and_validates_passphrase() {
        let mut screen = ExportSetupScreen::new("/media/usb");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = App::new(Header::new("LazyOS"))
            .design(Design::solarized().with_reduced_motion(true))
            .screen(&mut screen);

        type_text(&mut app, &mut terminal, "secret1");
        let _ = app.handle(Event::Key(key(KeyCode::Enter))); // focus -> confirm
        type_text(&mut app, &mut terminal, "mismatch");
        let _ = app.handle(Event::Key(key(KeyCode::Enter))); // focus -> include_home
        let _ = app.handle(Event::Key(key(KeyCode::Char(' ')))); // toggle checked
        let _ = app.handle(Event::Key(key(KeyCode::Down))); // focus -> Export button
        let out = app.handle(Event::Key(key(KeyCode::Enter)));
        assert_eq!(out, Outcome::Continue);
        drop(app);
        assert!(screen.include_home);
        assert!(!screen.submitted());
        assert_eq!(
            screen.error,
            Some("The two passphrases differ; please try again")
        );

        let mut app = App::new(Header::new("LazyOS"))
            .design(Design::solarized().with_reduced_motion(true))
            .screen(&mut screen);
        // Backspace off "mismatch", type matching passphrase.
        for _ in 0..8 {
            let _ = app.handle(Event::Key(key(KeyCode::Backspace)));
        }
        type_text(&mut app, &mut terminal, "secret1");
        let _ = app.handle(Event::Key(key(KeyCode::Down))); // include_home (stays checked)
        let _ = app.handle(Event::Key(key(KeyCode::Down))); // Export button
        let out = app.handle(Event::Key(key(KeyCode::Enter)));
        assert_eq!(out, Outcome::Done(0));
        drop(app);
        assert!(screen.include_home);
        assert!(screen.submitted());
        assert_eq!(screen.passphrase(), "secret1");
    }

    #[test]
    fn export_setup_supports_mouse_click_toggle_and_compact_layout() {
        let mut screen = ExportSetupScreen::new("/home/u/Downloads");
        let mut terminal = Terminal::new(TestBackend::new(64, 14)).unwrap();
        let design = Design::solarized().with_reduced_motion(true);
        let mut app = App::new(Header::new("LazyOS"))
            .design(design.clone())
            .screen(&mut screen);
        terminal.draw(|frame| app.render(frame)).unwrap();
        drop(app);

        let rect = screen
            .hits
            .borrow()
            .include_home
            .expect("checkbox hit rect");
        assert_eq!(
            screen.hit(Rect::new(0, 0, 64, 14), rect.x, rect.y, &design),
            Some(2)
        );
        assert_eq!(screen.click(Some(2)), Outcome::Consumed);
        assert!(screen.include_home);
        assert_eq!(screen.click(Some(2)), Outcome::Consumed);
        assert!(!screen.include_home);
    }
}
