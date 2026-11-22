//! Bookmarks & Disks Jump (`hayato-places`, `hayato menu places`).
//!
//! Lists user directories (`~/Downloads`, `~/Documents`, etc.), mounted
//! and mountable block storage devices with filesystem usage and mountpoints.
//! Also provides `--rows` headless output for witnesses.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card, draw_filter_bar, draw_gauge,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::pick::{Question, row};
use super::{have, out};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceEntry {
    pub key: String,
    pub label: String,
    pub target: String,
    pub glyph: Glyph,
}

/// A mounted removable drive (USB stick, SD card, external disk, or loop ISO).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedDrive {
    pub label: String,
    pub mountpoint: String,
    pub device: String,
    pub fstype: String,
}

/// Parse `findmnt -rn -o TARGET,SOURCE,FSTYPE` into mounted removable drives.
pub fn parse_mounted_drives(raw: &str) -> Vec<MountedDrive> {
    let mut drives = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let target = parts[0];
        let source = parts[1];
        let fstype = parts[2];
        match fstype {
            "ext2" | "ext3" | "ext4" | "vfat" | "exfat" | "btrfs" | "xfs" | "f2fs" | "ntfs"
            | "ntfs3" | "iso9660" | "udf" => {}
            _ => continue,
        }
        if !source.starts_with("/dev/") {
            continue;
        }
        if target == "/"
            || target.starts_with("/boot")
            || target.starts_with("/var")
            || target.starts_with("/usr")
            || target.starts_with("/etc")
        {
            continue;
        }
        let label = Path::new(target)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| target.to_string());
        drives.push(MountedDrive {
            label,
            mountpoint: target.to_string(),
            device: source.to_string(),
            fstype: fstype.to_string(),
        });
    }
    drives
}

/// Read all currently mounted removable drives from `findmnt`.
pub fn mounted_drives() -> Vec<MountedDrive> {
    let raw = out(&["findmnt", "-rn", "-o", "TARGET,SOURCE,FSTYPE"]).unwrap_or_default();
    parse_mounted_drives(&raw)
}

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Render the Waybar JSON payload for `custom/drives` (`hayato menu places --bar`).
pub fn bar_json_for(drives: &[MountedDrive]) -> String {
    if drives.is_empty() {
        return r#"{"text":"","class":"empty","tooltip":""}"#.to_string();
    }
    let text = if drives.len() == 1 {
        "<span font_desc='JetBrains Mono 13'>\u{f052}</span>".to_string()
    } else {
        format!(
            "<span font_desc='JetBrains Mono 13'>\u{f052}</span> {}",
            drives.len()
        )
    };
    let tooltip = if drives.len() == 1 {
        let d = &drives[0];
        format!(
            "Removable drive · {}\n{} ({} · {})\nClick: safely eject · Right-click: Places",
            d.label, d.mountpoint, d.device, d.fstype
        )
    } else {
        let mut lines = vec![format!("Removable drives · {} mounted", drives.len())];
        for d in drives {
            lines.push(format!("• {} ({})", d.label, d.mountpoint));
        }
        lines.push("Click: choose drive to eject · Right-click: Places".to_string());
        lines.join("\n")
    };
    format!(
        r#"{{"text":"{}","class":"mounted","tooltip":"{}"}}"#,
        json_escape(&text),
        json_escape(&tooltip)
    )
}

/// Live Waybar JSON payload (`hayato menu places --bar`).
pub fn bar_json() -> String {
    bar_json_for(&mounted_drives())
}

fn signal_bar() {
    let _ = out(&["pkill", "-RTMIN+8", "waybar"]);
}

/// Safely unmount a drive via `udisksctl` and power off its parent USB disk when idle.
/// On busy refusal, queries `fuser` so the user sees which process holds the mount open.
pub fn unmount_drive(drive: &MountedDrive) -> Result<String, String> {
    let res = Command::new("udisksctl")
        .args(["unmount", "-b", &drive.device])
        .output()
        .map_err(|e| format!("Could not run udisksctl: {e}"))?;
    if !res.status.success() {
        let stderr = String::from_utf8_lossy(&res.stderr).trim().to_string();
        if let Some(holders) = busy_processes(&drive.mountpoint) {
            return Err(format!("{} is in use by: {holders}", drive.label));
        }
        return Err(if stderr.is_empty() {
            format!("Could not unmount {}", drive.label)
        } else {
            format!("Could not unmount {}: {stderr}", drive.label)
        });
    }

    // Power off the parent USB/removable disk if no other partitions on it remain mounted.
    if let Some(pkname) = out(&["lsblk", "-no", "PKNAME", &drive.device])
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let parent_dev = format!("/dev/{pkname}");
        let remaining = out(&["findmnt", "-rn", "-o", "SOURCE"]).unwrap_or_default();
        let still_mounted = remaining.lines().any(|s| s.trim().starts_with(&parent_dev));
        let is_usb = out(&["lsblk", "-dnro", "TRAN", &parent_dev])
            .map(|s| s.trim() == "usb")
            .unwrap_or(false);
        if !still_mounted && is_usb {
            let _ = Command::new("udisksctl")
                .args(["power-off", "-b", &parent_dev])
                .output();
        }
    }

    signal_bar();
    Ok(format!("Safely ejected {} — safe to unplug", drive.label))
}

fn busy_processes(mountpoint: &str) -> Option<String> {
    let output = Command::new("fuser")
        .args(["-m", mountpoint])
        .output()
        .ok()?;
    let pids = String::from_utf8_lossy(&output.stdout);
    let mut names: Vec<String> = Vec::new();
    for tok in pids.split_whitespace() {
        let digits: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(pid) = digits.parse::<u32>()
            && let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        {
            let name = comm.trim().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// Parse findmnt output into mounted filesystem mountpoints.
pub fn parse_findmnt(raw: &str) -> Vec<(String, String)> {
    let mut mounts = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let target = parts[0];
        let fstype = parts[1];
        match fstype {
            "ext2" | "ext3" | "ext4" | "vfat" | "exfat" | "btrfs" | "xfs" | "f2fs" | "ntfs"
            | "ntfs3" => {}
            _ => continue,
        }
        if target == "/" || target.starts_with("/boot") {
            continue;
        }
        let base = Path::new(target)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| target.to_string());
        mounts.push((base, target.to_string()));
    }
    mounts
}

/// Parse lsblk output into unmounted partitions.
pub fn parse_lsblk(raw: &str) -> Vec<(String, String)> {
    let mut devices = Vec::new();
    for line in raw.lines() {
        // NAME TYPE FSTYPE LABEL MOUNTPOINT
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let name = parts[0];
        let ptype = parts[1];
        let fstype = parts[2];

        if ptype != "part" && ptype != "disk" {
            continue;
        }
        if fstype.is_empty() || fstype == "swap" || fstype == "crypto_LUKS" {
            continue;
        }
        // If there are 5 parts, parts[4] is mountpoint; if 4 parts and fstype is set,
        // it may or may not have a mountpoint. Check if line ends with a mountpoint or if parts len indicates mounted.
        // In raw lsblk: if MOUNTPOINT is non-empty, it's mounted.
        // We can inspect lines where mountpoint column is empty.
        let is_mounted = parts.len() >= 5 || (parts.len() == 4 && parts[3].starts_with('/'));
        if is_mounted {
            continue;
        }
        let label = if parts.len() >= 4 && !parts[3].starts_with('/') {
            parts[3].replace("\\x20", " ")
        } else {
            name.to_string()
        };
        devices.push((label, format!("/dev/{name}")));
    }
    devices
}

/// Build the full places list.
pub fn collect_places() -> Vec<PlaceEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let mut list = Vec::new();

    // Standard user folders
    list.push(PlaceEntry {
        key: format!("open:{home}"),
        label: "Home".to_string(),
        target: "~".to_string(),
        glyph: Glyph::Home,
    });

    let dirs = [
        ("Documents", "Documents", Glyph::File),
        ("Downloads", "Downloads", Glyph::Folder),
        ("Pictures", "Pictures", Glyph::Image),
        ("Music", "Music", Glyph::Music),
        ("Videos", "Videos", Glyph::Video),
        ("Dev", "Dev", Glyph::Code),
    ];

    for (name, folder, glyph) in dirs {
        let p = Path::new(&home).join(folder);
        if p.is_dir() {
            list.push(PlaceEntry {
                key: format!("open:{}", p.display()),
                label: name.to_string(),
                target: format!("~/{folder}"),
                glyph,
            });
        }
    }

    if Path::new("/etc/sway").is_dir() {
        list.push(PlaceEntry {
            key: "open:/etc/sway".to_string(),
            label: "Sway configuration".to_string(),
            target: "/etc/sway".to_string(),
            glyph: Glyph::Gear,
        });
    }

    list.push(PlaceEntry {
        key: "open:/".to_string(),
        label: "Root filesystem".to_string(),
        target: "/".to_string(),
        glyph: Glyph::Globe,
    });

    // Mounted removable devices
    let mounted = mounted_drives();
    for d in &mounted {
        list.push(PlaceEntry {
            key: format!("open:{}", d.mountpoint),
            label: d.label.clone(),
            target: format!("{} · {} (Ctrl+E eject)", d.mountpoint, d.fstype),
            glyph: Glyph::Disk,
        });
    }
    if !mounted.is_empty() {
        list.push(PlaceEntry {
            key: "action:eject".to_string(),
            label: format!("Eject a drive ({} mounted)", mounted.len()),
            target: "safely unmount & power off removable drives".to_string(),
            glyph: Glyph::Undo,
        });
    }

    // Unmounted devices
    if have("udisksctl") {
        let lsblk_raw =
            out(&["lsblk", "-rno", "NAME,TYPE,FSTYPE,LABEL,MOUNTPOINT"]).unwrap_or_default();
        for (label, dev) in parse_lsblk(&lsblk_raw) {
            list.push(PlaceEntry {
                key: format!("mount:{dev}"),
                label: format!("Mount {label}"),
                target: dev,
                glyph: Glyph::Disk,
            });
        }
    }

    // Shell & Disk actions
    list.push(PlaceEntry {
        key: format!("term:{home}"),
        label: "Open a terminal".to_string(),
        target: "~".to_string(),
        glyph: Glyph::Terminal,
    });

    list.push(PlaceEntry {
        key: "action:swap".to_string(),
        label: "Auxiliary swap".to_string(),
        target: "a card as more memory, on every boot".to_string(),
        glyph: Glyph::Chip,
    });

    list.push(PlaceEntry {
        key: "action:format".to_string(),
        label: "Format a disk".to_string(),
        target: "a stick or a card, wiped clean".to_string(),
        glyph: Glyph::Warning,
    });

    list
}

/// Headless `--rows` output for witness check.
pub fn rows() -> Vec<(String, String, String)> {
    collect_places()
        .into_iter()
        .map(|p| (p.key, p.label, p.target))
        .collect()
}

/// The one question of `hayato menu places`, built from data so a snapshot can
/// draw it without a mounted disk. `menu` passes what `collect_places` found.
pub fn questions(places: &[PlaceEntry], note: &str) -> Vec<Question> {
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    for p in places {
        rows.push(row(&p.label, &p.target, p.glyph));
        keys.push(p.key.clone());
    }
    vec![Question::choose("Places", note, rows, keys)]
}

#[derive(Clone, Debug)]
struct DiskUsage {
    used: String,
    avail: String,
    total: String,
    pct: f64,
    fstype: String,
}

fn read_disk_usage(path: &str) -> Option<DiskUsage> {
    let raw = out(&["df", "-hT", path])?;
    let line = raw.lines().nth(1)?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let fstype = parts[1].to_string();
    let total = parts[2].to_string();
    let used = parts[3].to_string();
    let avail = parts[4].to_string();
    let pct_num: f64 = parts[5].trim_end_matches('%').parse().unwrap_or(0.0);
    Some(DiskUsage {
        used,
        avail,
        total,
        pct: (pct_num / 100.0).clamp(0.0, 1.0),
        fstype,
    })
}

/// Interactive Places & Storage Hub screen.
pub struct PlacesScreen {
    places: Vec<PlaceEntry>,
    usage_cache: HashMap<String, Option<DiskUsage>>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    banner: Option<String>,
    pub chosen: Option<String>,
}

impl PlacesScreen {
    pub fn new(places: Vec<PlaceEntry>, banner: Option<String>) -> Self {
        let rows: Vec<Row> = places
            .iter()
            .map(|p| Row::new(&p.label).glyph(p.glyph).detail(&p.target))
            .collect();
        let mut list = ListState::new().with_empty("no matching places");
        list.set_rows(rows);

        let mut screen = Self {
            places,
            usage_cache: HashMap::new(),
            list,
            header: Header::new("Hayato")
                .subtitle("Places & Storage")
                .status("Folders, disks, mounts & storage tools"),
            keybar: vec![
                KeyHint::new("Enter", "open / mount"),
                KeyHint::new("Ctrl+E", "eject drive"),
                KeyHint::new("Ctrl+T", "terminal here"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            banner,
            chosen: None,
        };
        screen.ensure_selected_usage();
        screen
    }

    fn selected_place(&self) -> Option<&PlaceEntry> {
        let idx = self.list.selected_index()?;
        self.places.get(idx)
    }

    fn ensure_selected_usage(&mut self) {
        let Some(place) = self.selected_place().cloned() else {
            return;
        };
        if let Some(path) = place.key.strip_prefix("open:")
            && !self.usage_cache.contains_key(path)
        {
            self.usage_cache
                .insert(path.to_string(), read_disk_usage(path));
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 52) / 100).max(46) as u16;
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

    fn left_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let filter = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
        );
        (filter, rows)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Storage Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 6 {
            return;
        }
        let Some(place) = self.selected_place() else {
            return;
        };
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let title_style = design.style(TextStyle::Title).add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        let kind = if place.key.starts_with("open:") {
            "DIRECTORY / MOUNT"
        } else if place.key.starts_with("mount:") {
            "UNMOUNTED VOLUME"
        } else if place.key.starts_with("term:") {
            "TERMINAL"
        } else {
            "STORAGE TOOL"
        };
        put_trunc(buf, inner.x, y, kind, inner.width, accent);
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("{}  {}", place.glyph.cell(), place.label),
            inner.width,
            title_style,
        );
        y += 2;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("Target    {}", place.target),
            inner.width,
            body,
        );
        y += 2;

        if let Some(path) = place.key.strip_prefix("open:")
            && let Some(Some(usage)) = self.usage_cache.get(path)
        {
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Filesystem  {}", usage.fstype),
                inner.width,
                dim,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!(
                    "Used        {} / {} ({} free)",
                    usage.used, usage.total, usage.avail
                ),
                inner.width,
                body,
            );
            y += 2;
            if y < inner.bottom().saturating_sub(2) {
                let gauge_rect = Rect::new(inner.x, y, inner.width.min(36), 1);
                let pct_label = format!("{:.0}%", usage.pct * 100.0);
                draw_gauge(buf, gauge_rect, design, usage.pct, &pct_label);
            }
        } else if place.key.starts_with("mount:") {
            put_trunc(
                buf,
                inner.x,
                y,
                "Press Enter to mount via udisksctl and open in Files.",
                inner.width,
                dim,
            );
        }

        let footer = if place.key.starts_with("open:") {
            "Enter opens in Files · Ctrl+T opens terminal here"
        } else if place.key.starts_with("mount:") {
            "Enter mounts volume and opens in Files"
        } else {
            "Enter runs selected storage tool"
        };
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            footer,
            inner.width,
            dim,
        );
    }

    fn activate_selected(&mut self) -> Outcome {
        if let Some(place) = self.selected_place() {
            self.chosen = Some(place.key.clone());
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        Outcome::Continue
    }
}

impl Screen for PlacesScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Places"), true);
        let (filter, rows) = Self::left_layout(left);
        let msg = self
            .banner
            .as_deref()
            .unwrap_or("Enter opens · Ctrl+T terminal here · type to filter");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.places.len(),
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
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        {
            if let Some(place) = self.selected_place()
                && let Some(path) = place.key.strip_prefix("open:")
            {
                self.chosen = Some(format!("term:{path}"));
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
        }
        let is_eject = (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E')))
            || (key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('1'))
            || key.code == KeyCode::Delete;
        if is_eject {
            if let Some(place) = self.selected_place()
                && let Some(path) = place.key.strip_prefix("open:")
            {
                self.chosen = Some(format!("eject:{path}"));
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    self.ensure_selected_usage();
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => return self.activate_selected(),
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                self.ensure_selected_usage();
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate_selected(),
            ListAction::Moved => {
                self.ensure_selected_usage();
                Outcome::Continue
            }
            ListAction::Ignored => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let (_, rows) = Self::left_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate_selected();
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("places.hub")
    }
}

/// Dedicated Safely Eject Drives Studio (`hayato menu places --eject`).
pub struct EjectScreen {
    drives: Vec<MountedDrive>,
    usage_cache: HashMap<String, Option<DiskUsage>>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    banner: Option<(String, bool)>,
}

impl Default for EjectScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl EjectScreen {
    pub fn new() -> Self {
        Self::from_drives(mounted_drives())
    }

    pub fn from_drives(drives: Vec<MountedDrive>) -> Self {
        let mut screen = Self {
            drives,
            usage_cache: HashMap::new(),
            list: ListState::new().with_empty("no removable drives are mounted"),
            header: Header::new("Hayato").subtitle("Eject Drives"),
            keybar: vec![
                KeyHint::new("Enter", "safely eject"),
                KeyHint::new("Ctrl+E", "eject all"),
                KeyHint::new("Ctrl+O", "open folder"),
                KeyHint::new("Ctrl+R", "refresh"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(16),
            banner: None,
        };
        screen.rebuild_rows();
        screen
    }

    fn reload(&mut self) {
        self.drives = mounted_drives();
        self.usage_cache.clear();
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let mut rows: Vec<Row> = self
            .drives
            .iter()
            .map(|d| {
                Row::new(&d.label)
                    .glyph(Glyph::Disk)
                    .detail(format!("{} · {} ({})", d.mountpoint, d.device, d.fstype))
                    .value("eject")
            })
            .collect();
        if self.drives.len() > 1 {
            rows.push(
                Row::new(format!("Safely eject all ({} drives)", self.drives.len()))
                    .glyph(Glyph::Undo)
                    .detail("unmount & power off every mounted removable drive")
                    .value("^E"),
            );
        }
        self.list.set_rows(rows);
        self.header =
            Header::new("Hayato")
                .subtitle("Eject Drives")
                .status(if self.drives.is_empty() {
                    "No removable drives mounted · safe to unplug".into()
                } else {
                    format!(
                        "{} mounted removable drive{}",
                        self.drives.len(),
                        if self.drives.len() == 1 { "" } else { "s" }
                    )
                });
        self.ensure_selected_usage();
    }

    fn selected_drive(&self) -> Option<&MountedDrive> {
        let idx = self.list.selected_index()?;
        self.drives.get(idx)
    }

    fn ensure_selected_usage(&mut self) {
        let Some(d) = self.selected_drive().cloned() else {
            return;
        };
        if !self.usage_cache.contains_key(&d.mountpoint) {
            self.usage_cache
                .insert(d.mountpoint.clone(), read_disk_usage(&d.mountpoint));
        }
    }

    fn eject_all(&mut self) {
        if self.drives.is_empty() {
            return;
        }
        let targets = self.drives.clone();
        let mut ok_count = 0usize;
        let mut first_err: Option<String> = None;
        for d in &targets {
            match unmount_drive(d) {
                Ok(_) => ok_count += 1,
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        self.reload();
        if let Some(err) = first_err {
            self.banner = Some((
                format!("Ejected {ok_count}/{} · {err}", targets.len()),
                true,
            ));
        } else {
            self.banner = Some((
                format!("Safely ejected all {ok_count} drives — safe to unplug"),
                false,
            ));
        }
    }

    fn activate_selected(&mut self) -> Outcome {
        let Some(idx) = self.list.selected_index() else {
            return Outcome::Continue;
        };
        if idx == self.drives.len() && self.drives.len() > 1 {
            self.eject_all();
            return Outcome::Consumed;
        }
        if let Some(drive) = self.drives.get(idx).cloned() {
            match unmount_drive(&drive) {
                Ok(msg) => {
                    self.reload();
                    self.banner = Some((msg, false));
                }
                Err(err) => {
                    self.banner = Some((err, true));
                }
            }
        }
        Outcome::Consumed
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Drive Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 6 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        if let Some(d) = self.selected_drive() {
            let mut y = inner.y;
            put_trunc(
                buf,
                inner.x,
                y,
                "MOUNTED REMOVABLE DRIVE",
                inner.width,
                accent,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("{}  {}", Glyph::Disk.cell(), d.label),
                inner.width,
                design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
            );
            y += 2;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Device      {}", d.device),
                inner.width,
                body,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Mountpoint  {}", d.mountpoint),
                inner.width,
                body,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Filesystem  {}", d.fstype),
                inner.width,
                dim,
            );
            y += 2;
            if let Some(Some(u)) = self.usage_cache.get(&d.mountpoint)
                && y < inner.bottom().saturating_sub(3)
            {
                put_trunc(
                    buf,
                    inner.x,
                    y,
                    &format!("Used        {} / {} ({} free)", u.used, u.total, u.avail),
                    inner.width,
                    body,
                );
                y += 1;
                let pct_label = format!("{:.0}%", u.pct * 100.0);
                draw_gauge(
                    buf,
                    Rect::new(inner.x, y, inner.width.min(36), 1),
                    design,
                    u.pct,
                    &pct_label,
                );
            }
        } else if self.drives.is_empty() {
            put_trunc(buf, inner.x, inner.y, "ALL CLEAR", inner.width, accent);
            put_trunc(
                buf,
                inner.x,
                inner.y + 2,
                "No removable drives are currently mounted.",
                inner.width,
                body,
            );
            put_trunc(
                buf,
                inner.x,
                inner.y + 3,
                "It is safe to unplug your USB sticks and SD cards.",
                inner.width,
                dim,
            );
        } else {
            put_trunc(
                buf,
                inner.x,
                inner.y,
                "EJECT ALL DRIVES",
                inner.width,
                accent,
            );
            put_trunc(
                buf,
                inner.x,
                inner.y + 2,
                "Safely unmounts and powers off every listed drive.",
                inner.width,
                body,
            );
        }

        if let Some((msg, is_err)) = &self.banner {
            let st = Style::default().fg(design.color(if *is_err {
                ColorRole::Danger
            } else {
                ColorRole::Ok
            }));
            put_trunc(
                buf,
                inner.x,
                inner.bottom().saturating_sub(2),
                msg,
                inner.width,
                st,
            );
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Enter ejects · Ctrl+O opens in Files · Ctrl+E ejects all",
            inner.width,
            dim,
        );
    }
}

impl Screen for EjectScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = PlacesScreen::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Mounted Drives"), true);
        let (filter, rows) = PlacesScreen::left_layout(left);
        let default_msg = if self.drives.is_empty() {
            "No removable drives mounted · Esc closes"
        } else {
            "Enter safely ejects · Ctrl+O opens · Ctrl+E ejects all"
        };
        let msg = self
            .banner
            .as_ref()
            .map(|(m, _)| m.as_str())
            .unwrap_or(default_msg);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            self.list.len(),
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    self.eject_all();
                    return Outcome::Consumed;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.reload();
                    self.banner = Some(("Refreshed mounted drives".into(), false));
                    return Outcome::Consumed;
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    if let Some(d) = self.selected_drive() {
                        let escaped = d.mountpoint.replace('\'', "'\\''");
                        crate::system::spawn_detached(&format!(
                            "exec hayato menu files '{escaped}'"
                        ));
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    self.ensure_selected_usage();
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => return self.activate_selected(),
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                self.ensure_selected_usage();
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate_selected(),
            ListAction::Moved => {
                self.ensure_selected_usage();
                Outcome::Continue
            }
            ListAction::Ignored => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = PlacesScreen::split_panes(area);
        let (_, rows) = PlacesScreen::left_layout(left);
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate_selected();
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("places.eject")
    }
}

/// Interactive `hayato menu places [--eject]`.
pub fn menu_with_args(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if args.iter().any(|a| a == "--eject" || a == "eject") {
        let mut screen = EjectScreen::new();
        return crate::run_screen(terminal, design, "Eject Drives", &mut screen);
    }
    menu(terminal, design)
}

/// Interactive `hayato menu places`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut status_note: Option<String> = None;

    loop {
        let places = collect_places();
        let mut screen = PlacesScreen::new(places, status_note.take());

        let code = crate::run_screen(terminal, design, "Places", &mut screen)?;
        if code != 0 {
            return Ok(code);
        }

        let Some(key) = screen.chosen.take() else {
            return Ok(1);
        };

        match key.as_str() {
            "action:eject" => {
                let mut eject = EjectScreen::new();
                let res = crate::run_screen(terminal, design, "Eject Drives", &mut eject)?;
                if res == hayato_ui::app::exit::ESCAPE {
                    continue;
                }
                continue;
            }
            k if k.starts_with("eject:") => {
                let mp = &k[6..];
                if let Some(drive) = mounted_drives().into_iter().find(|d| d.mountpoint == mp) {
                    match unmount_drive(&drive) {
                        Ok(msg) => status_note = Some(msg),
                        Err(err) => status_note = Some(err),
                    }
                } else {
                    status_note = Some(format!("{mp} is not a removable drive"));
                }
            }
            "action:swap" => {
                let res = super::swap::menu(terminal, design)?;
                if res == hayato_ui::app::exit::ESCAPE {
                    continue;
                }
                return Ok(res);
            }
            "action:format" => {
                let res = super::format::menu(terminal, design)?;
                if res == hayato_ui::app::exit::ESCAPE {
                    continue;
                }
                return Ok(res);
            }
            k if k.starts_with("term:") => {
                let path = &k[5..];
                let escaped = path.replace('\'', "'\\''");
                crate::system::spawn_detached(&format!(
                    "exec foot --working-directory '{escaped}'"
                ));
                return Ok(0);
            }
            k if k.starts_with("open:") => {
                let path = &k[5..];
                let escaped = path.replace('\'', "'\\''");
                crate::system::spawn_detached(&format!("exec hayato menu files '{escaped}'"));
                return Ok(0);
            }
            k if k.starts_with("mount:") => {
                let dev = &k[6..];
                let out = Command::new("udisksctl")
                    .args(["mount", "-b", dev])
                    .output();
                match out {
                    Ok(res) if res.status.success() => {
                        signal_bar();
                        let text = String::from_utf8_lossy(&res.stdout);
                        if let Some(pos) = text.rfind(" at ") {
                            let mp = text[pos + 4..].trim().trim_end_matches('.');
                            let escaped = mp.replace('\'', "'\\''");
                            crate::system::spawn_detached(&format!(
                                "exec hayato menu files '{escaped}'"
                            ));
                            return Ok(0);
                        } else {
                            status_note = Some(format!("Mounted {dev}"));
                        }
                    }
                    Ok(res) => {
                        let err = String::from_utf8_lossy(&res.stderr);
                        status_note = Some(format!("Could not mount {dev}: {}", err.trim()));
                    }
                    Err(e) => {
                        status_note = Some(format!("Could not mount {dev}: {e}"));
                    }
                }
            }
            _ => return Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_findmnt_filters_root_and_boot() {
        let raw = "\
/ ext4
/boot ext4
/run/media/brunux/STICK vfat
/home/user/other btrfs
";
        let res = parse_findmnt(raw);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, "STICK");
        assert_eq!(res[0].1, "/run/media/brunux/STICK");
        assert_eq!(res[1].0, "other");
    }

    #[test]
    fn parse_lsblk_extracts_unmounted_partitions() {
        let raw = "\
sda disk
sda1 part vfat STICK_LABEL
mmcblk0p1 part ext4 ROOT /
";
        let res = parse_lsblk(raw);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, "STICK_LABEL");
        assert_eq!(res[0].1, "/dev/sda1");
    }

    #[test]
    fn rows_includes_swap_and_format_actions() {
        let r = rows();
        assert!(
            r.iter()
                .any(|(k, l, _)| k == "action:swap" && l == "Auxiliary swap")
        );
        assert!(
            r.iter()
                .any(|(k, l, _)| k == "action:format" && l == "Format a disk")
        );
    }

    #[test]
    fn parse_mounted_drives_extracts_removable_mounts_only() {
        let raw = "\
/ /dev/mmcblk0p3 ext4
/boot /dev/mmcblk0p2 ext4
/run/media/brunux/NURA-BACKUP /dev/sda1 exfat
/run/media/brunux/PHOTOS /dev/sdb1 vfat
/home/brunux/drive rclone fuse.rclone
";
        let drives = parse_mounted_drives(raw);
        assert_eq!(drives.len(), 2);
        assert_eq!(drives[0].label, "NURA-BACKUP");
        assert_eq!(drives[0].device, "/dev/sda1");
        assert_eq!(drives[0].fstype, "exfat");
        assert_eq!(drives[1].label, "PHOTOS");
    }

    #[test]
    fn bar_json_hides_when_empty_and_formats_single_and_multi_drives() {
        let empty = bar_json_for(&[]);
        assert!(empty.contains(r#""class":"empty""#));

        let d1 = MountedDrive {
            label: "NURA-BACKUP".into(),
            mountpoint: "/run/media/brunux/NURA-BACKUP".into(),
            device: "/dev/sda1".into(),
            fstype: "exfat".into(),
        };
        let one = bar_json_for(std::slice::from_ref(&d1));
        assert!(one.contains(r#""class":"mounted""#));
        assert!(one.contains("NURA-BACKUP"));
        assert!(one.contains("Click: safely eject"));

        let d2 = MountedDrive {
            label: "PHOTOS".into(),
            mountpoint: "/run/media/brunux/PHOTOS".into(),
            device: "/dev/sdb1".into(),
            fstype: "vfat".into(),
        };
        let two = bar_json_for(&[d1, d2]);
        assert!(two.contains(r#""class":"mounted""#));
        assert!(two.contains("2 mounted"));
        assert!(two.contains("PHOTOS"));
    }

    #[test]
    fn eject_screen_adds_eject_all_row_when_multiple_drives_mounted() {
        let d1 = MountedDrive {
            label: "USB1".into(),
            mountpoint: "/run/media/brunux/USB1".into(),
            device: "/dev/sda1".into(),
            fstype: "exfat".into(),
        };
        let d2 = MountedDrive {
            label: "USB2".into(),
            mountpoint: "/run/media/brunux/USB2".into(),
            device: "/dev/sdb1".into(),
            fstype: "vfat".into(),
        };
        let screen = EjectScreen::from_drives(vec![d1, d2]);
        assert_eq!(screen.phase(), Some("places.eject"));
        // 2 drive rows + 1 "Safely eject all" row
        assert_eq!(screen.list.len(), 3);
    }
}
