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
    let findmnt_raw = out(&["findmnt", "-rn", "-o", "TARGET,FSTYPE"]).unwrap_or_default();
    for (label, target) in parse_findmnt(&findmnt_raw) {
        list.push(PlaceEntry {
            key: format!("open:{target}"),
            label,
            target,
            glyph: Glyph::Disk,
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
}
