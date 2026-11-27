#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
/// Developer: profile export, restore, and diagnostics bundle (`hayato-dev`).
///
/// The old developer menu was a three-row front end over `hayato-profile` helper.
/// This version provides structured pickers for archive selection and target
/// destinations, dispatching operations detached from the terminal so that
/// no child output leaks onto the screen.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevAction {
    HealthCheck,
    Logs,
    Mcp,
    Report,
    AptRepo,
    SshGit,
    GreeterPreview,
    ReloadDesktop,
    OpenConfig,
    Export,
    Restore,
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
    /// Parallel to rows in `list`: `Some(action)` for selectable items, `None` for headings.
    row_actions: Vec<Option<DevAction>>,
    pub selected: Option<DevAction>,
    list: ListState,
    banner: String,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl DevScreen {
    pub fn new() -> Self {
        Self::with_banner(
            "Type to filter · health checks, logs, APT, MCP, desktop tools & profile backups",
        )
    }

    pub fn with_banner(banner: impl Into<String>) -> Self {
        let items: Vec<(Option<&str>, Option<DevEntry>)> = vec![
            (Some("Health & Debugging"), None),
            (
                None,
                Some(DevEntry {
                    icon: "chip".to_string(),
                    label: "Run system health check".to_string(),
                    detail: "check-system.sh · 10-core, 20-device, 30-desktop witnesses"
                        .to_string(),
                    action: DevAction::HealthCheck,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "terminal".to_string(),
                    label: "Inspect boot & session logs".to_string(),
                    detail: "journalctl · this boot errors, previous boot, kernel, Sway"
                        .to_string(),
                    action: DevAction::Logs,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "code".to_string(),
                    label: "Inspect MCP tools & status".to_string(),
                    detail: "hayato mcp --tools · 18 agent tools & OpenCode registration"
                        .to_string(),
                    action: DevAction::Mcp,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "bug".to_string(),
                    label: "Save a diagnostics bundle".to_string(),
                    detail: "the system report and this boot's journal, for a bug report"
                        .to_string(),
                    action: DevAction::Report,
                }),
            ),
            (Some("Packages & Identity"), None),
            (
                None,
                Some(DevEntry {
                    icon: "download".to_string(),
                    label: "Hayato APT repository".to_string(),
                    detail: "Cloudsmith debian/forky · status, enable, disable, apt update"
                        .to_string(),
                    action: DevAction::AptRepo,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "key".to_string(),
                    label: "SSH keys & Git identity".to_string(),
                    detail: "copy ~/.ssh/*.pub to clipboard · view git user & email".to_string(),
                    action: DevAction::SshGit,
                }),
            ),
            (Some("Desktop & UI Dev"), None),
            (
                None,
                Some(DevEntry {
                    icon: "eye".to_string(),
                    label: "Preview login greeter".to_string(),
                    detail: "hayato greeter --preview · test themes & layout without rebooting"
                        .to_string(),
                    action: DevAction::GreeterPreview,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "computer".to_string(),
                    label: "Reload desktop services".to_string(),
                    detail: "swaymsg reload · restart waybar and hayato-notify".to_string(),
                    action: DevAction::ReloadDesktop,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "box".to_string(),
                    label: "Open config & repo folders".to_string(),
                    detail: "~/.config/hayato, ~/.config/sway, /etc/hayato, ~/Dev/Hayato"
                        .to_string(),
                    action: DevAction::OpenConfig,
                }),
            ),
            (Some("Profile Backup"), None),
            (
                None,
                Some(DevEntry {
                    icon: "box".to_string(),
                    label: "Export my profile".to_string(),
                    detail: "keys, configs and optional home folder, encrypted · to a disk"
                        .to_string(),
                    action: DevAction::Export,
                }),
            ),
            (
                None,
                Some(DevEntry {
                    icon: "undo".to_string(),
                    label: "Restore a profile".to_string(),
                    detail: "from an exported archive · the current files are backed up first"
                        .to_string(),
                    action: DevAction::Restore,
                }),
            ),
        ];

        let mut rows = Vec::with_capacity(items.len());
        let mut row_actions = Vec::with_capacity(items.len());
        for (heading, entry) in items {
            if let Some(title) = heading {
                rows.push(Row::heading(title));
                row_actions.push(None);
            } else if let Some(e) = entry {
                rows.push(Self::entry_row(&e));
                row_actions.push(Some(e.action));
            }
        }

        let mut list = ListState::new().with_empty("no matching developer actions");
        list.set_rows(rows);

        Self {
            row_actions,
            selected: None,
            list,
            banner: banner.into(),
            header: Header::new("Hayato").subtitle("Developer"),
            keybar: vec![
                KeyHint::new("Enter", "open"),
                KeyHint::new("type", "filter"),
                KeyHint::new("\u{2191}\u{2193}", "move"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
        }
    }

    pub fn action_at(&self, index: usize) -> Option<DevAction> {
        self.row_actions.get(index).copied().flatten()
    }

    pub fn profile_bin() -> std::path::PathBuf {
        if let Ok(home) = std::env::var("HOME") {
            let p = std::path::PathBuf::from(home).join(".config/bin/hayato-profile");
            if p.exists() {
                return p;
            }
        }
        std::path::PathBuf::from("hayato-profile")
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
            "download" => Glyph::Download,
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
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Developer tools"), true);
        let (filter, rows) = Self::list_layout(left);
        let total = self.row_actions.iter().filter(|a| a.is_some()).count();
        let visible = if self.list.filter().is_empty() {
            total
        } else {
            self.list.visible_len()
        };
        hayato_ui::widgets::draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            &self.banner,
            visible,
            total,
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);

        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Enter => {
                if let Some(action) = self
                    .list
                    .selected_index()
                    .and_then(|idx| self.action_at(idx))
                {
                    self.selected = Some(action);
                    return Outcome::Done(0);
                }
            }
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                self.selected = None;
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            _ => {}
        }
        if !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && self.list.type_filter(key)
        {
            return Outcome::Consumed;
        }
        let _ = self.list.handle(key, rows_height, design);
        Outcome::Continue
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(action) = self
                .list
                .selected_index()
                .and_then(|idx| self.action_at(idx))
            {
                self.selected = Some(action);
                return Outcome::Done(0);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::list_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

struct DevInspectorInfo {
    category: &'static str,
    title: &'static str,
    command: &'static str,
    summary: &'static str,
    highlights: &'static [&'static str],
    paths: &'static [&'static str],
    safety: &'static str,
}

fn inspector_for(action: DevAction) -> DevInspectorInfo {
    match action {
        DevAction::HealthCheck => DevInspectorInfo {
            category: "HEALTH & DEBUGGING",
            title: "Run system health check",
            command: "check-system.sh",
            summary: "Executes the full 3-layer Hayato runtime witness suite in an in-window TaskView.",
            highlights: &[
                "10-core.sh   Rootfs, boot backend, APT & systemd services",
                "20-device.sh Kernel, firmware, audio, display & board quirks",
                "30-desktop.sh SwayFX, Waybar, theme files, glyphs & hayato CLI",
            ],
            paths: &["/usr/libexec/hayato/check/", "/usr/sbin/check-system.sh"],
            safety: "Read-only diagnostic run · safe to run at any time",
        },
        DevAction::Logs => DevInspectorInfo {
            category: "HEALTH & DEBUGGING",
            title: "Inspect boot & session logs",
            command: "journalctl -b -p err / --user",
            summary: "Quick-filter systemd journal streams without memorizing journalctl flags.",
            highlights: &[
                "This boot errors & warnings (priority err..emerg)",
                "Previous boot crash trail (-b -1)",
                "Kernel dmesg ring buffer & Wayland/Sway user session",
            ],
            paths: &["/run/log/journal", "/var/log/journal"],
            safety: "Read-only · opens inside in-window log viewer",
        },
        DevAction::Mcp => DevInspectorInfo {
            category: "HEALTH & DEBUGGING",
            title: "Inspect MCP tools & status",
            command: "hayato mcp --tools",
            summary: "Audits the read/propose-only Model Context Protocol server and agent registration.",
            highlights: &[
                "Lists all 18 built-in Hayato MCP tools & schemas",
                "Checks OpenCode & local agent MCP server registration",
                "Verifies read-only / propose-only safety boundary",
            ],
            paths: &["~/.config/opencode/opencode.json", "hayato mcp"],
            safety: "Read-only · MCP server never mutates host state directly",
        },
        DevAction::Report => DevInspectorInfo {
            category: "HEALTH & DEBUGGING",
            title: "Save a diagnostics bundle",
            command: "hayato-profile report",
            summary: "Packages hardware facts, system report, and this boot's journal into a shareable archive.",
            highlights: &[
                "Captures check-system.sh output & hardware summary",
                "Includes current boot journal & Sway output topology",
                "Writes cleanly to removable media or ~/Downloads",
            ],
            paths: &["~/Downloads/", "/run/media/"],
            safety: "Sanitized bundle · review before sharing publicly",
        },
        DevAction::AptRepo => DevInspectorInfo {
            category: "PACKAGES & IDENTITY",
            title: "Hayato APT repository",
            command: "sudo hayato-apt status | enable | disable",
            summary: "Manages the signed Cloudsmith debian/forky APT repository for Hayato updates.",
            highlights: &[
                "Inspects enabled status, suite, URI & GPG keyring",
                "One-click enable / disable with polkit/sudo elevation",
                "Runs apt-get update inside an in-window TaskView",
            ],
            paths: &[
                "/etc/apt/sources.list.d/hayato.list",
                "/usr/share/keyrings/hayato-archive-keyring.gpg",
            ],
            safety: "Signed APT metadata · requires sudo to toggle",
        },
        DevAction::SshGit => DevInspectorInfo {
            category: "PACKAGES & IDENTITY",
            title: "SSH keys & Git identity",
            command: "ls ~/.ssh/*.pub · git config --global",
            summary: "Inspects your local SSH public keys and global Git author configuration.",
            highlights: &[
                "Lists ~/.ssh/*.pub keys with algorithm & comment",
                "One-click copy any public key to Wayland clipboard",
                "Displays active git user.name, user.email & signing",
            ],
            paths: &["~/.ssh/", "~/.gitconfig"],
            safety: "Public keys only · private keys are never read or copied",
        },
        DevAction::GreeterPreview => DevInspectorInfo {
            category: "DESKTOP & UI DEV",
            title: "Preview login greeter",
            command: "hayato greeter --preview",
            summary: "Launches an interactive sandboxed preview of the Hayato login greeter.",
            highlights: &[
                "Test pixel-hacker wordmark, theme palettes & clock layout",
                "Verify power strip & user pills without logging out",
                "Runs against mock Preview backend (zero greetd IPC)",
            ],
            paths: &["~/.config/hayato/greeter.conf", "/etc/hayato/greeter.conf"],
            safety: "Sandboxed preview · cannot lock or sign out your session",
        },
        DevAction::ReloadDesktop => DevInspectorInfo {
            category: "DESKTOP & UI DEV",
            title: "Reload desktop services",
            command: "swaymsg reload · waybar · hayato-notify",
            summary: "Hot-reloads Sway configuration and restarts Waybar and notification cards cleanly.",
            highlights: &[
                "Applies updated Sway keybindings, gaps & window rules",
                "Restarts Waybar with refreshed CSS & module configs",
                "Restarts hayato-notify daemon with active theme",
            ],
            paths: &["~/.config/sway/", "~/.config/waybar/"],
            safety: "Non-destructive · open windows & terminals stay running",
        },
        DevAction::OpenConfig => DevInspectorInfo {
            category: "DESKTOP & UI DEV",
            title: "Open config & repo folders",
            command: "hayato menu files <dir>",
            summary: "Jump straight into Hayato configuration directories or local source checkouts.",
            highlights: &[
                "~/.config/hayato  Theme, greeter, notify & app configs",
                "~/.config/sway    Compositor rules & display overrides",
                "/etc/hayato       System defaults & APT configuration",
                "~/Dev/Hayato      Local umbrella repository checkout",
            ],
            paths: &["~/.config/hayato", "~/.config/sway", "/etc/hayato"],
            safety: "Opens in Hayato Files browser",
        },
        DevAction::Export => DevInspectorInfo {
            category: "PROFILE BACKUP",
            title: "Export my profile",
            command: "hayato-profile export",
            summary: "Creates an encrypted portable backup of your keys, configs, and optional home files.",
            highlights: &[
                "Interactive scope picker (configs, SSH/GPG keys, home)",
                "Passphrase-encrypted archive before writing to disk",
                "Targets USB drives or ~/Downloads with progress TaskView",
            ],
            paths: &["~/.config/", "~/.ssh/", "~/Downloads/"],
            safety: "Encrypted archive · store your passphrase safely",
        },
        DevAction::Restore => DevInspectorInfo {
            category: "PROFILE BACKUP",
            title: "Restore a profile",
            command: "hayato-profile restore",
            summary: "Restores a previously exported encrypted Hayato profile archive.",
            highlights: &[
                "Auto-discovers archives on USB drives and ~/Downloads",
                "Shows archive size, timestamp & included scope before restore",
                "Backs up current files automatically before overwriting",
            ],
            paths: &["~/Downloads/", "/run/media/"],
            safety: "Backs up existing configs first · asks passphrase & confirmation",
        },
    }
}

impl DevScreen {
    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 50, 48)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Workbench Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 8 {
            return;
        }
        let action = self
            .list
            .selected_index()
            .and_then(|idx| self.action_at(idx))
            .unwrap_or(DevAction::HealthCheck);
        let info = inspector_for(action);

        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let title_style = design.style(TextStyle::Title).add_modifier(Modifier::BOLD);
        let ok_style = Style::default().fg(design.color(ColorRole::Ok));

        let mut y = inner.y;
        put_trunc(buf, inner.x, y, info.category, inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, info.title, inner.width, title_style);
        y += 2;

        // Command pill
        let cmd_line = format!("$ {}", info.command);
        put_trunc(buf, inner.x, y, &cmd_line, inner.width, ok_style);
        y += 2;

        // Summary (word-wrapped)
        let w = usize::from(inner.width.max(1));
        let chars: Vec<char> = info.summary.chars().collect();
        for chunk in chars.chunks(w) {
            if y >= inner.bottom().saturating_sub(4) {
                break;
            }
            let s: String = chunk.iter().collect();
            put_trunc(buf, inner.x, y, &s, inner.width, body);
            y += 1;
        }
        y += 1;

        // Highlights
        if y < inner.bottom().saturating_sub(4) {
            put_trunc(buf, inner.x, y, "What it does", inner.width, accent);
            y += 1;
            for item in info.highlights {
                if y >= inner.bottom().saturating_sub(4) {
                    break;
                }
                let line = format!("• {item}");
                put_trunc(buf, inner.x, y, &line, inner.width, body);
                y += 1;
            }
            y += 1;
        }

        // Paths
        if y < inner.bottom().saturating_sub(3) {
            put_trunc(buf, inner.x, y, "Paths", inner.width, accent);
            y += 1;
            for p in info.paths {
                if y >= inner.bottom().saturating_sub(2) {
                    break;
                }
                put_trunc(buf, inner.x, y, p, inner.width, dim);
                y += 1;
            }
        }

        // Safety footer
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            info.safety,
            inner.width,
            dim,
        );
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
            header: Header::new("Hayato").subtitle("Export my profile"),
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
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
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
                KeyCode::Enter => Outcome::Done(hayato_ui::app::exit::ESCAPE),
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
            ExportFocus::Back => Outcome::Done(hayato_ui::app::exit::ESCAPE),
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

    fn phase(&self) -> Option<&'static str> {
        Some(match self.stage {
            ExportStage::Editing => "editing",
            ExportStage::Submitted => "submitted",
        })
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

/// Build CLI argv for `hayato-profile export`.
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

fn resolve_system_helper(name: &str, libexec_name: &str) -> Option<PathBuf> {
    for cand in [
        format!("/usr/sbin/{name}"),
        format!("/usr/local/sbin/{name}"),
        format!("/usr/libexec/hayato/{libexec_name}"),
    ] {
        let p = PathBuf::from(cand);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AptStatusSummary {
    enabled: bool,
    suite: String,
    keyring: String,
}

fn parse_apt_status(output: &str) -> AptStatusSummary {
    let mut enabled = false;
    let mut suite = "forky".to_string();
    let mut keyring = "unknown".to_string();
    for line in output.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "enabled" => enabled = matches!(v.trim(), "yes" | "true" | "1"),
                "suite" => suite = v.trim().to_string(),
                "keyring" => keyring = v.trim().to_string(),
                _ => {}
            }
        }
    }
    AptStatusSummary {
        enabled,
        suite,
        keyring,
    }
}

fn collect_ssh_pubkeys(home: &Path) -> Vec<(String, String, PathBuf)> {
    let ssh_dir = home.join(".ssh");
    let mut keys = Vec::new();
    let Ok(entries) = std::fs::read_dir(&ssh_dir) else {
        return keys;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let detail = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.lines().next().map(str::to_string))
            .map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                match parts.as_slice() {
                    [kind, _, comment, ..] => format!("{kind} · {comment}"),
                    [kind, ..] => (*kind).to_string(),
                    _ => "public key".to_string(),
                }
            })
            .unwrap_or_else(|| "public key".to_string());
        keys.push((name, detail, path));
    }
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    keys
}

fn read_git_identity() -> String {
    let read = |key: &str| {
        std::process::Command::new("git")
            .args(["config", "--global", "--get", key])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match (read("user.name"), read("user.email")) {
        (Some(n), Some(e)) => format!("Git: {n} <{e}>"),
        (Some(n), None) => format!("Git: {n} (no email set)"),
        (None, Some(e)) => format!("Git: <{e}> (no name set)"),
        (None, None) => "Git: user.name / user.email not configured".to_string(),
    }
}

fn collect_dev_folders(home: &Path) -> Vec<(String, String, PathBuf)> {
    let candidates = [
        ("Hayato config", home.join(".config/hayato")),
        ("Sway config", home.join(".config/sway")),
        ("Waybar config", home.join(".config/waybar")),
        ("Session helpers", home.join(".config/bin")),
        ("System /etc/hayato", PathBuf::from("/etc/hayato")),
        ("Hayato source repo", home.join("Dev/Hayato")),
    ];
    candidates
        .into_iter()
        .filter(|(_, p)| p.is_dir())
        .map(|(label, p)| (label.to_string(), p.display().to_string(), p))
        .collect()
}

fn check_profile_deps(profile_bin: &Path) -> Result<(), String> {
    let profile_exists = profile_bin.exists() || have("hayato-profile");
    let mut needed = vec!["wl-copy", "sudo", "tar", "gpg", "zstd"];
    if !profile_exists {
        needed.push("hayato-profile");
    }
    if let Some(said) = missing(&needed) {
        return Err(format!(
            "{said} (hayato-profile, wl-clipboard, sudo, tar, gnupg, zstd)"
        ));
    }
    Ok(())
}

/// `hayato menu dev` (`hayato-dev`): Hayato developer workbench.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let profile_bin = DevScreen::profile_bin();
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"));
    let mut banner: Option<String> = None;

    loop {
        let note = banner.take().unwrap_or_else(|| {
            "Type to filter · health checks, logs, APT, MCP, desktop tools & profile backups"
                .to_string()
        });
        let mut dev_screen = DevScreen::with_banner(note);
        let code = crate::run_screen(terminal, design, "Developer", &mut dev_screen)?;
        if code != 0 {
            return Ok(code);
        }
        let Some(action) = dev_screen.selected else {
            return Ok(0);
        };

        match action {
            DevAction::HealthCheck => {
                let Some(bin) = resolve_system_helper("check-system.sh", "check-system.sh") else {
                    banner = Some(
                        "check-system.sh not found in /usr/sbin or /usr/libexec/hayato".to_string(),
                    );
                    continue;
                };
                let tag = format!("dev-check-{}", std::process::id());
                let task = Task::start_steps(
                    &tag,
                    "System health check",
                    vec!["Run check-system.sh (10-core, 20-device, 30-desktop)".to_string()],
                    true,
                    vec![vec![bin.to_string_lossy().to_string()]],
                )
                .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                let mut view = TaskView::new(task);
                let _ = crate::run_screen(terminal, design, "System health check", &mut view)?;
            }
            DevAction::Logs => {
                let mut log_picker = Picker::new(
                    "Inspect logs",
                    vec![Question::choose(
                        "Inspect logs",
                        "journalctl output opens in-window · Escape goes back",
                        vec![
                            Row::new("This boot errors")
                                .glyph(Glyph::Bug)
                                .detail("journalctl -b -p err..emerg -n 200"),
                            Row::new("Previous boot errors")
                                .glyph(Glyph::Clock)
                                .detail("journalctl -b -1 -p err..emerg -n 200"),
                            Row::new("Kernel messages (dmesg)")
                                .glyph(Glyph::Chip)
                                .detail("journalctl -b -k -n 200"),
                            Row::new("User session & Sway logs")
                                .glyph(Glyph::Desktop)
                                .detail("journalctl --user -b -n 200"),
                        ],
                        vec![
                            "boot-err".to_string(),
                            "prev-err".to_string(),
                            "kernel".to_string(),
                            "user".to_string(),
                        ],
                    )],
                );
                let l_code = crate::run_screen(terminal, design, "Inspect logs", &mut log_picker)?;
                if l_code != 0 {
                    continue;
                }
                let Some(which) = log_picker.answers().first().cloned() else {
                    continue;
                };
                let (title, argv): (&str, Vec<String>) = match which.as_str() {
                    "boot-err" => (
                        "This boot errors",
                        vec![
                            "journalctl".into(),
                            "-b".into(),
                            "-p".into(),
                            "err..emerg".into(),
                            "--no-pager".into(),
                            "-n".into(),
                            "200".into(),
                        ],
                    ),
                    "prev-err" => (
                        "Previous boot errors",
                        vec![
                            "journalctl".into(),
                            "-b".into(),
                            "-1".into(),
                            "-p".into(),
                            "err..emerg".into(),
                            "--no-pager".into(),
                            "-n".into(),
                            "200".into(),
                        ],
                    ),
                    "kernel" => (
                        "Kernel messages",
                        vec![
                            "journalctl".into(),
                            "-b".into(),
                            "-k".into(),
                            "--no-pager".into(),
                            "-n".into(),
                            "200".into(),
                        ],
                    ),
                    _ => (
                        "User session logs",
                        vec![
                            "journalctl".into(),
                            "--user".into(),
                            "-b".into(),
                            "--no-pager".into(),
                            "-n".into(),
                            "200".into(),
                        ],
                    ),
                };
                let tag = format!("dev-logs-{}", std::process::id());
                let task =
                    Task::start_steps(&tag, title, vec![format!("Read {title}")], true, vec![argv])
                        .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                let mut view = TaskView::new(task);
                let _ = crate::run_screen(terminal, design, title, &mut view)?;
            }
            DevAction::Mcp => {
                let opencode_cfg = home.join(".config/opencode/opencode.json");
                let mut steps = vec!["List hayato mcp --tools manifest".to_string()];
                let mut commands = vec![vec!["hayato".into(), "mcp".into(), "--tools".into()]];
                if opencode_cfg.exists() {
                    steps.push("Inspect OpenCode MCP registration".to_string());
                    commands.push(vec![
                        "cat".into(),
                        opencode_cfg.to_string_lossy().to_string(),
                    ]);
                }
                let tag = format!("dev-mcp-{}", std::process::id());
                let task = Task::start_steps(&tag, "Hayato MCP manifest", steps, true, commands)
                    .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                let mut view = TaskView::new(task);
                let _ = crate::run_screen(terminal, design, "Hayato MCP manifest", &mut view)?;
            }
            DevAction::AptRepo => {
                let Some(apt_bin) = resolve_system_helper("hayato-apt", "hayato-apt.sh") else {
                    banner = Some(
                        "hayato-apt not found in /usr/sbin or /usr/libexec/hayato".to_string(),
                    );
                    continue;
                };
                let status_out = std::process::Command::new(&apt_bin)
                    .arg("status")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                let st = parse_apt_status(&status_out);
                let status_line = format!(
                    "Cloudsmith APT: {} · suite: {} · keyring: {}",
                    if st.enabled { "enabled" } else { "disabled" },
                    st.suite,
                    st.keyring
                );
                let mut apt_picker = Picker::new(
                    "Hayato APT repository",
                    vec![Question::choose(
                        "Hayato APT repository",
                        status_line,
                        vec![
                            Row::new("Enable Cloudsmith APT")
                                .glyph(Glyph::Download)
                                .detail(
                                    "sudo hayato-apt enable · enables signed debian/forky repo",
                                ),
                            Row::new("Disable Cloudsmith APT")
                                .glyph(Glyph::Undo)
                                .detail("sudo hayato-apt disable · keeps local keyring intact"),
                            Row::new("Refresh package lists (apt update)")
                                .glyph(Glyph::Terminal)
                                .detail("sudo apt update"),
                        ],
                        vec![
                            "enable".to_string(),
                            "disable".to_string(),
                            "update".to_string(),
                        ],
                    )],
                );
                let a_code =
                    crate::run_screen(terminal, design, "Hayato APT repository", &mut apt_picker)?;
                if a_code != 0 {
                    continue;
                }
                let Some(choice) = apt_picker.answers().first().cloned() else {
                    continue;
                };
                let (title, argv): (&str, Vec<String>) = match choice.as_str() {
                    "enable" => (
                        "Enable Cloudsmith APT",
                        vec![
                            "sudo".into(),
                            apt_bin.to_string_lossy().to_string(),
                            "enable".into(),
                        ],
                    ),
                    "disable" => (
                        "Disable Cloudsmith APT",
                        vec![
                            "sudo".into(),
                            apt_bin.to_string_lossy().to_string(),
                            "disable".into(),
                        ],
                    ),
                    _ => (
                        "Update APT lists",
                        vec!["sudo".into(), "apt".into(), "update".into()],
                    ),
                };
                let tag = format!("dev-apt-{}", std::process::id());
                let task =
                    Task::start_steps(&tag, title, vec![title.to_string()], false, vec![argv])
                        .map_err(|e| crate::Failure::new("Developer", e.to_string()))?;
                let mut view = TaskView::new(task);
                let _ = crate::run_screen(terminal, design, title, &mut view)?;
            }
            DevAction::SshGit => {
                let git_line = read_git_identity();
                let pubkeys = collect_ssh_pubkeys(&home);
                if pubkeys.is_empty() {
                    banner = Some(format!("{git_line} · no ~/.ssh/*.pub keys found"));
                    continue;
                }
                let rows: Vec<Row> = pubkeys
                    .iter()
                    .map(|(name, detail, _)| Row::new(name).glyph(Glyph::Key).detail(detail))
                    .collect();
                let keys: Vec<String> = pubkeys
                    .iter()
                    .map(|(_, _, p)| p.to_string_lossy().to_string())
                    .collect();
                let mut key_picker = Picker::new(
                    "SSH keys & Git identity",
                    vec![Question::choose(
                        "SSH keys & Git identity",
                        format!("{git_line} · Enter copies the public key to clipboard"),
                        rows,
                        keys,
                    )],
                );
                let k_code = crate::run_screen(
                    terminal,
                    design,
                    "SSH keys & Git identity",
                    &mut key_picker,
                )?;
                if k_code != 0 {
                    continue;
                }
                if let Some(path_str) = key_picker.answers().first() {
                    if let Ok(contents) = std::fs::read_to_string(path_str) {
                        if let Ok(mut child) = std::process::Command::new("wl-copy")
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()
                        {
                            if let Some(mut stdin) = child.stdin.take() {
                                use std::io::Write;
                                let _ = stdin.write_all(contents.as_bytes());
                            }
                            let _ = child.wait();
                            let fname = Path::new(path_str)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path_str.clone());
                            banner = Some(format!("Copied {fname} to clipboard"));
                        }
                    }
                }
            }
            DevAction::GreeterPreview => {
                let preview_args = vec!["--preview".to_string()];
                let _ = crate::greeter::menu(&preview_args, terminal, design)?;
                banner = Some("Returned from greeter preview".to_string());
            }
            DevAction::ReloadDesktop => {
                let _ = std::process::Command::new("swaymsg")
                    .arg("reload")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let _ = std::process::Command::new("pkill")
                    .args(["-x", "waybar"])
                    .status();
                let _ = std::process::Command::new("pkill")
                    .args(["-x", "hayato-notify"])
                    .status();
                let _ = std::process::Command::new("swaymsg")
                    .args(["exec", "waybar"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let _ = std::process::Command::new("swaymsg")
                    .args(["exec", "hayato-notify"])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                banner = Some("Reloaded Sway, Waybar & hayato-notify".to_string());
            }
            DevAction::OpenConfig => {
                let folders = collect_dev_folders(&home);
                if folders.is_empty() {
                    banner = Some("No Hayato config or repo folders found".to_string());
                    continue;
                }
                let rows: Vec<Row> = folders
                    .iter()
                    .map(|(label, detail, _)| Row::new(label).glyph(Glyph::Box).detail(detail))
                    .collect();
                let keys: Vec<String> = folders
                    .iter()
                    .map(|(_, _, p)| p.to_string_lossy().to_string())
                    .collect();
                let mut dir_picker = Picker::new(
                    "Open config & repo folders",
                    vec![Question::choose(
                        "Open config & repo folders",
                        "Opens the folder in Files · Escape goes back",
                        rows,
                        keys,
                    )],
                );
                let d_code = crate::run_screen(
                    terminal,
                    design,
                    "Open config & repo folders",
                    &mut dir_picker,
                )?;
                if d_code != 0 {
                    continue;
                }
                if let Some(dir) = dir_picker.answers().first() {
                    let _ = std::process::Command::new("hayato-files")
                        .args(["--", dir])
                        .env_remove("HAYATO_UI")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    return Ok(0);
                }
            }
            DevAction::Restore => {
                if let Err(msg) = check_profile_deps(&profile_bin) {
                    banner = Some(msg);
                    continue;
                }
                let mut archives = DevScreen::list_archives();
                if archives.is_empty() {
                    // A stick that is plugged in but not mounted holds the
                    // archive just as well, and a fresh boot leaves it
                    // unmounted: offer the same disks Places offers, mount the
                    // chosen one, and look again. `list-archives` can only see
                    // mounted filesystems.
                    let mountable: Vec<super::places::PlaceEntry> = super::places::collect_places()
                        .into_iter()
                        .filter(|place| place.key.starts_with("mount:"))
                        .collect();
                    if mountable.is_empty() {
                        banner = Some(
                            "No exported profile found on mounted disks or ~/Downloads \u{00b7} plug the stick in and open Places"
                                .to_string(),
                        );
                        continue;
                    }
                    let rows: Vec<Row> = mountable
                        .iter()
                        .map(|place| {
                            Row::new(&place.label)
                                .glyph(Glyph::Disk)
                                .detail("mount it, then look for a profile archive on it")
                        })
                        .collect();
                    let keys: Vec<String> =
                        mountable.iter().map(|place| place.key.clone()).collect();
                    let mut mount_picker = Picker::new(
                        "Restore a profile",
                        vec![Question::choose(
                            "No archive on a mounted disk",
                            "A stick that is plugged in but not mounted can hold it \u{00b7} pick one to mount",
                            rows,
                            keys,
                        )],
                    );
                    let m_code = crate::run_screen(
                        terminal,
                        design,
                        "Restore a profile",
                        &mut mount_picker,
                    )?;
                    if m_code != 0 {
                        continue;
                    }
                    let Some(key) = mount_picker.answers().first().cloned() else {
                        continue;
                    };
                    let dev = key.trim_start_matches("mount:").to_string();
                    match std::process::Command::new("udisksctl")
                        .args(["mount", "-b", &dev])
                        .output()
                    {
                        Ok(res) if res.status.success() => {
                            archives = DevScreen::list_archives();
                        }
                        Ok(res) => {
                            banner = Some(format!(
                                "Could not mount {dev}: {}",
                                String::from_utf8_lossy(&res.stderr).trim()
                            ));
                            continue;
                        }
                        Err(error) => {
                            banner = Some(format!("Could not mount {dev}: {error}"));
                            continue;
                        }
                    }
                    if archives.is_empty() {
                        banner = Some(format!(
                            "No exported profile found on {dev} after mounting it"
                        ));
                        continue;
                    }
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
                            "Matching current files & configs are backed up to ~/.cache/hayato/backups first",
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
                            format!("{}/hayato-profile-pass-{}", runtime, std::process::id());
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
            DevAction::Export => 'export_pick: loop {
                if let Err(msg) = check_profile_deps(&profile_bin) {
                    banner = Some(msg);
                    break 'export_pick;
                }
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
                let pass_file = format!("{}/hayato-profile-pass-{}", runtime, std::process::id());
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
            DevAction::Report => {
                if let Err(msg) = check_profile_deps(&profile_bin) {
                    banner = Some(msg);
                    continue;
                }
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayato_ui::{App, Design, Header};
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
        let bin = Path::new("/home/u/.config/bin/hayato-profile");
        let plain = build_export_args(bin, "/media/usb", false, Some("/run/user/1000/pass"));
        assert!(!plain.iter().any(|a| a == "--include-home"));
        assert_eq!(
            plain,
            vec![
                "/home/u/.config/bin/hayato-profile",
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
                "/home/u/.config/bin/hayato-profile",
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
            manifest_path_for("/media/usb/hayato-profile-host-20261116.tar.zst.gpg"),
            PathBuf::from("/media/usb/hayato-profile-host-20261116.txt")
        );
        assert_eq!(
            parse_scope_from_manifest("Hayato profile\nscope: home\nentries:\n"),
            Some("full home".to_string())
        );
        assert_eq!(
            parse_scope_from_manifest("Hayato profile\nscope: profile\nentries:\n"),
            Some("profile".to_string())
        );
        assert_eq!(
            parse_scope_from_manifest("Hayato profile\nentries:\n"),
            None
        );
    }

    #[test]
    fn export_setup_toggles_include_home_and_validates_passphrase() {
        let mut screen = ExportSetupScreen::new("/media/usb");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = App::new(Header::new("Hayato"))
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

        let mut app = App::new(Header::new("Hayato"))
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
        let mut app = App::new(Header::new("Hayato"))
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

    #[test]
    fn dev_screen_escape_does_not_trigger_health_check() {
        let mut screen = DevScreen::new();
        let design = Design::solarized().with_reduced_motion(true);
        let out = screen.key(key(KeyCode::Esc), &design);
        assert_eq!(out, Outcome::Done(hayato_ui::app::exit::ESCAPE));
        assert_eq!(screen.selected, None);

        let out = screen.key(key(KeyCode::Enter), &design);
        assert_eq!(out, Outcome::Done(0));
        assert_eq!(screen.selected, Some(DevAction::HealthCheck));
    }

    #[test]
    fn dev_screen_maps_grouped_actions_and_skips_headings() {
        let screen = DevScreen::new();
        // Index 0 is the "Health & Debugging" heading, index 1 is HealthCheck.
        assert_eq!(screen.action_at(0), None);
        assert_eq!(screen.action_at(1), Some(DevAction::HealthCheck));
        assert_eq!(screen.action_at(2), Some(DevAction::Logs));
        assert_eq!(screen.action_at(3), Some(DevAction::Mcp));
        assert_eq!(screen.action_at(4), Some(DevAction::Report));
        // Index 5 is "Packages & Identity" heading.
        assert_eq!(screen.action_at(5), None);
        assert_eq!(screen.action_at(6), Some(DevAction::AptRepo));
        assert_eq!(screen.action_at(7), Some(DevAction::SshGit));
    }

    #[test]
    fn apt_status_parses_enabled_suite_and_keyring() {
        let st = parse_apt_status(
            "enabled=yes\nuri=https://dl.cloudsmith.io/public/hayato/hayato/deb/debian\nsuite=forky\nkeyring=present\n",
        );
        assert!(st.enabled);
        assert_eq!(st.suite, "forky");
        assert_eq!(st.keyring, "present");

        let off = parse_apt_status("enabled=no\nsuite=forky\nkeyring=missing\n");
        assert!(!off.enabled);
        assert_eq!(off.keyring, "missing");
    }

    #[test]
    fn collect_ssh_pubkeys_reads_pub_files_only() {
        let tmp = std::env::temp_dir().join(format!("hayato-dev-ssh-{}", std::process::id()));
        let ssh = tmp.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("id_ed25519"), "PRIVATE").unwrap();
        std::fs::write(
            ssh.join("id_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest bruno@hayato\n",
        )
        .unwrap();
        let keys = collect_ssh_pubkeys(&tmp);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].0, "id_ed25519.pub");
        assert_eq!(keys[0].1, "ssh-ed25519 · bruno@hayato");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
