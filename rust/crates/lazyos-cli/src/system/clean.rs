//! Disk Cleaner (`rofi-clean`, `lazyos menu clean`).
//!
//! Opens immediately and computes reclaimable cache sizes asynchronously via
//! [`ActionRunner`]: apt package cache, systemd journal logs, thumbnail cache,
//! and user trash. Displays a storage breakdown hero card with [`draw_gauge`],
//! per-category proportion bars, and cleans selected categories in-window
//! while updating freed space in-place. Also provides `--dry-run` output.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::time::Duration;

use lazyos_ui::widgets::{put, put_trunc};
use lazyos_ui::{
    ActionRunner, BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState,
    Outcome, Row, Screen, TextStyle, draw_badge, draw_card, draw_filter_bar, draw_gauge,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{out, sudo};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reclaimable {
    pub trash_bytes: u64,
    pub thumb_bytes: u64,
    pub apt_bytes: u64,
    pub journal_bytes: u64,
    pub cache_bytes: u64,
    pub free_str: String,
    pub total_str: String,
    pub used_ratio: u8,
}

impl Reclaimable {
    pub fn safe_reclaimable_bytes(&self) -> u64 {
        self.trash_bytes
            .saturating_add(self.thumb_bytes)
            .saturating_add(self.apt_bytes)
            .saturating_add(self.journal_bytes.saturating_sub(100 * 1024 * 1024))
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "0 MB".to_string()
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes / 1024).max(1))
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn mini_bar(bytes: u64, max_bytes: u64) -> String {
    let max_bytes = max_bytes.max(1);
    let filled = if bytes == 0 {
        0
    } else {
        (((bytes as f64 / max_bytes as f64) * 6.0).ceil() as usize).clamp(1, 6)
    };
    let mut s = String::with_capacity(18);
    for i in 0..6 {
        if i < filled {
            s.push('\u{2588}');
        } else {
            s.push('\u{2591}');
        }
    }
    s
}

/// Recursively sum size of files in a directory.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    total += dir_size(&entry.path());
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

/// Parse journalctl --disk-usage output.
pub fn parse_journal_usage(raw: &str) -> u64 {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    for (i, &word) in parts.iter().enumerate() {
        if word == "up" && i + 1 < parts.len() {
            let val_str = parts[i + 1];
            if let Some(ch) = val_str.chars().last() {
                let num_str = &val_str[..val_str.len() - 1];
                if let Ok(num) = num_str.parse::<f64>() {
                    let mult = match ch {
                        'K' | 'k' => 1024.0,
                        'M' | 'm' => 1024.0 * 1024.0,
                        'G' | 'g' => 1024.0 * 1024.0 * 1024.0,
                        _ => 1.0,
                    };
                    return (num * mult) as u64;
                }
            }
        }
    }
    0
}

/// Measure all reclaimable sizes safely.
pub fn calculate_reclaimable() -> Reclaimable {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let trash_path = Path::new(&home).join(".local/share/Trash");
    let thumb_path = Path::new(&home).join(".cache/thumbnails");
    let apt_path = Path::new("/var/cache/apt/archives");
    let cache_path = Path::new(&home).join(".cache");

    let trash_bytes = dir_size(&trash_path);
    let thumb_bytes = dir_size(&thumb_path);
    let apt_bytes = dir_size(apt_path);
    let cache_bytes = dir_size(&cache_path);

    let journal_raw = out(&["journalctl", "--disk-usage"])
        .or_else(|| sudo(&["journalctl", "--disk-usage"]).ok())
        .unwrap_or_default();
    let journal_bytes = parse_journal_usage(&journal_raw);

    let df_out = out(&["df", "-h", "--output=avail,size,pcent", "/"]).unwrap_or_default();
    let mut free_str = "?".to_string();
    let mut total_str = "?".to_string();
    let mut used_ratio = 0u8;
    if let Some(line) = df_out.lines().nth(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 2 {
            free_str = cols[0].to_string();
            total_str = cols[1].to_string();
        }
        if let Some(pct) = cols.get(2) {
            used_ratio = pct.trim_end_matches('%').parse::<u8>().unwrap_or(0);
        }
    }

    Reclaimable {
        trash_bytes,
        thumb_bytes,
        apt_bytes,
        journal_bytes,
        cache_bytes,
        free_str,
        total_str,
        used_ratio,
    }
}

/// Headless query mode: `lazyos menu clean --dry-run`.
pub fn dry_run() {
    let rec = calculate_reclaimable();
    println!("trash: {}", format_bytes(rec.trash_bytes));
    println!("thumbnails: {}", format_bytes(rec.thumb_bytes));
    println!("apt: {}", format_bytes(rec.apt_bytes));
    println!("journal: {}", format_bytes(rec.journal_bytes));
    println!("cache: {}", format_bytes(rec.cache_bytes));
    println!("free: {} of {}", rec.free_str, rec.total_str);
}

enum CleanOp {
    Measured(Reclaimable),
    Cleaned {
        message: String,
        ok: bool,
        rec: Reclaimable,
    },
}

#[derive(Clone, Copy, Debug)]
enum CleanTarget {
    Trash,
    Thumbnails,
    Apt,
    Journal,
    AllSafe,
    InspectCache,
    Rescan,
}

pub struct CleanScreen {
    rec: Option<Reclaimable>,
    targets: Vec<CleanTarget>,
    runner: ActionRunner<CleanOp>,
    banner: Option<(String, ColorRole)>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl Default for CleanScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl CleanScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            rec: None,
            targets: Vec::new(),
            runner: ActionRunner::new(),
            banner: None,
            list: ListState::new(),
            header: Header::new("Free space").subtitle("Disk cleaner"),
            keybar: vec![
                KeyHint::new("Enter", "clean / open"),
                KeyHint::new("r", "rescan"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(16),
        };
        screen.rebuild_rows(false);
        screen.start_scan();
        screen
    }

    fn start_scan(&mut self) {
        self.runner
            .spawn("Calculating reclaimable disk space\u{2026}", || {
                CleanOp::Measured(calculate_reclaimable())
            });
        self.sync_chrome();
    }

    fn start_clean(&mut self, target: CleanTarget) {
        let label = match target {
            CleanTarget::Trash => "Emptying trash\u{2026}",
            CleanTarget::Thumbnails => "Removing thumbnail cache\u{2026}",
            CleanTarget::Apt => "Cleaning APT package cache\u{2026}",
            CleanTarget::Journal => "Vacuuming systemd journal to 100 MB\u{2026}",
            CleanTarget::AllSafe => "Cleaning trash, thumbnails, APT & journal\u{2026}",
            CleanTarget::InspectCache | CleanTarget::Rescan => return,
        };
        let before = self
            .rec
            .as_ref()
            .map(|r| r.safe_reclaimable_bytes())
            .unwrap_or(0);
        self.runner.spawn(label, move || {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            let mut ok = true;
            let mut err_msg = String::new();
            match target {
                CleanTarget::Trash => {
                    let _ = fs::remove_dir_all(Path::new(&home).join(".local/share/Trash/files"));
                    let _ = fs::remove_dir_all(Path::new(&home).join(".local/share/Trash/info"));
                }
                CleanTarget::Thumbnails => {
                    let _ = fs::remove_dir_all(Path::new(&home).join(".cache/thumbnails"));
                }
                CleanTarget::Apt => {
                    if let Err(e) = sudo(&["apt-get", "clean"]) {
                        ok = false;
                        err_msg = e;
                    }
                }
                CleanTarget::Journal => {
                    if let Err(e) = sudo(&["journalctl", "--vacuum-size=100M"]) {
                        ok = false;
                        err_msg = e;
                    }
                }
                CleanTarget::AllSafe => {
                    let _ = fs::remove_dir_all(Path::new(&home).join(".local/share/Trash/files"));
                    let _ = fs::remove_dir_all(Path::new(&home).join(".local/share/Trash/info"));
                    let _ = fs::remove_dir_all(Path::new(&home).join(".cache/thumbnails"));
                    let _ = sudo(&["apt-get", "clean"]);
                    let _ = sudo(&["journalctl", "--vacuum-size=100M"]);
                }
                CleanTarget::InspectCache | CleanTarget::Rescan => {}
            }
            let rec = calculate_reclaimable();
            let after = rec.safe_reclaimable_bytes();
            let freed = before.saturating_sub(after);
            let message = if !ok {
                format!("Cleanup error: {err_msg}")
            } else {
                match target {
                    CleanTarget::Trash => format!("Trash emptied \u{00b7} freed {}", format_bytes(freed)),
                    CleanTarget::Thumbnails => {
                        format!("Thumbnails removed \u{00b7} freed {}", format_bytes(freed))
                    }
                    CleanTarget::Apt => {
                        format!("APT cache cleaned \u{00b7} freed {}", format_bytes(freed))
                    }
                    CleanTarget::Journal => "Journal vacuumed to \u{2264} 100 MB".to_string(),
                    CleanTarget::AllSafe => {
                        format!("Cleaned safe caches \u{00b7} freed {}", format_bytes(freed))
                    }
                    _ => "Done".to_string(),
                }
            };
            CleanOp::Cleaned { message, ok, rec }
        });
        self.sync_chrome();
    }

    fn rebuild_rows(&mut self, preserve: bool) {
        let mut rows = Vec::new();
        let mut targets = Vec::new();

        if let Some(rec) = &self.rec {
            let max_cat = [
                rec.trash_bytes,
                rec.thumb_bytes,
                rec.apt_bytes,
                rec.journal_bytes,
                rec.cache_bytes,
            ]
            .into_iter()
            .max()
            .unwrap_or(1)
            .max(1);

            let safe_total = rec.safe_reclaimable_bytes();
            rows.push(
                Row::new("Clean all safe caches")
                    .glyph(Glyph::Sparkles)
                    .detail("trash + thumbnails + apt archives + journal > 100 MB")
                    .value(format_bytes(safe_total)),
            );
            targets.push(CleanTarget::AllSafe);

            rows.push(
                Row::new("Empty the trash")
                    .glyph(Glyph::Trash)
                    .detail(format!(
                        "{}  ~/.local/share/Trash",
                        mini_bar(rec.trash_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.trash_bytes)),
            );
            targets.push(CleanTarget::Trash);

            rows.push(
                Row::new("Remove thumbnails")
                    .glyph(Glyph::Image)
                    .detail(format!(
                        "{}  ~/.cache/thumbnails",
                        mini_bar(rec.thumb_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.thumb_bytes)),
            );
            targets.push(CleanTarget::Thumbnails);

            rows.push(
                Row::new("Clean the package cache")
                    .glyph(Glyph::Box)
                    .detail(format!(
                        "{}  /var/cache/apt/archives",
                        mini_bar(rec.apt_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.apt_bytes)),
            );
            targets.push(CleanTarget::Apt);

            rows.push(
                Row::new("Shrink journal to 100 MB")
                    .glyph(Glyph::Book)
                    .detail(format!(
                        "{}  systemd journal logs",
                        mini_bar(rec.journal_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.journal_bytes)),
            );
            targets.push(CleanTarget::Journal);

            rows.push(
                Row::new("Your cache (~/.cache)")
                    .glyph(Glyph::Folder)
                    .detail(format!(
                        "{}  open in file browser to inspect",
                        mini_bar(rec.cache_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.cache_bytes)),
            );
            targets.push(CleanTarget::InspectCache);

            rows.push(
                Row::new("Rescan disk usage")
                    .glyph(Glyph::Sync)
                    .detail("recalculate directory sizes (r)"),
            );
            targets.push(CleanTarget::Rescan);
        } else {
            let calc = "calculating\u{2026}";
            rows.push(
                Row::new("Clean all safe caches")
                    .glyph(Glyph::Sparkles)
                    .detail("trash + thumbnails + apt archives + journal > 100 MB")
                    .value(calc),
            );
            targets.push(CleanTarget::AllSafe);
            rows.push(
                Row::new("Empty the trash")
                    .glyph(Glyph::Trash)
                    .detail("~/.local/share/Trash")
                    .value(calc),
            );
            targets.push(CleanTarget::Trash);
            rows.push(
                Row::new("Remove thumbnails")
                    .glyph(Glyph::Image)
                    .detail("~/.cache/thumbnails")
                    .value(calc),
            );
            targets.push(CleanTarget::Thumbnails);
            rows.push(
                Row::new("Clean the package cache")
                    .glyph(Glyph::Box)
                    .detail("/var/cache/apt/archives")
                    .value(calc),
            );
            targets.push(CleanTarget::Apt);
            rows.push(
                Row::new("Shrink journal to 100 MB")
                    .glyph(Glyph::Book)
                    .detail("systemd journal logs")
                    .value(calc),
            );
            targets.push(CleanTarget::Journal);
            rows.push(
                Row::new("Your cache (~/.cache)")
                    .glyph(Glyph::Folder)
                    .detail("open in file browser to inspect")
                    .value(calc),
            );
            targets.push(CleanTarget::InspectCache);
        }

        self.targets = targets;
        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
    }

    fn sync_chrome(&mut self) {
        let status = if self.runner.is_running() {
            format!(
                "{} {}",
                self.runner.spinner(),
                self.runner.label().unwrap_or("Scanning\u{2026}")
            )
        } else if let Some((msg, _)) = &self.banner {
            msg.clone()
        } else if let Some(rec) = &self.rec {
            format!(
                "{} free of {} \u{00b7} {} reclaimable",
                rec.free_str,
                rec.total_str,
                format_bytes(rec.safe_reclaimable_bytes())
            )
        } else {
            "Calculating reclaimable space\u{2026}".to_string()
        };
        self.header = Header::new("Free space")
            .subtitle("Disk cleaner")
            .status(status);
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let hero = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 2.min(inner.height),
        };
        let filter = Rect {
            x: inner.x,
            y: inner.y.saturating_add(2).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(2)),
        };
        let rows_y = inner.y.saturating_add(4).min(inner.bottom());
        let rows = Rect {
            x: inner.x,
            y: rows_y,
            width: inner.width,
            height: inner.bottom().saturating_sub(rows_y),
        };
        (hero, filter, rows)
    }

    fn draw_hero(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        if area.height == 0 || area.width < 20 {
            return;
        }
        let y = area.y;
        if let Some(rec) = &self.rec {
            let role = if rec.used_ratio >= 90 {
                ColorRole::Danger
            } else if rec.used_ratio >= 75 {
                ColorRole::Warn
            } else {
                ColorRole::Ok
            };
            let mut x = draw_badge(buf, area.x, y, design, "ROOT /", role);
            let summary = format!(
                "  {} free of {}  \u{00b7}  Reclaimable: {}",
                rec.free_str,
                rec.total_str,
                format_bytes(rec.safe_reclaimable_bytes())
            );
            let gauge_space = 22u16.min(area.width / 3);
            x = put_trunc(
                buf,
                x,
                y,
                &summary,
                area.right().saturating_sub(x + gauge_space + 2),
                design.style(TextStyle::Title),
            );
            let _ = x;
            if area.width >= 40 {
                let gx = area.right().saturating_sub(gauge_space);
                let gw = gauge_space.saturating_sub(6);
                let pct = format!(" {:>3}%", rec.used_ratio);
                draw_gauge(
                    buf,
                    Rect {
                        x: gx,
                        y,
                        width: gw,
                        height: 1,
                    },
                    design,
                    rec.used_ratio as f64 / 100.0,
                    &pct,
                );
            }
        } else {
            let x = draw_badge(buf, area.x, y, design, "SCANNING", ColorRole::Accent);
            let _ = put_trunc(
                buf,
                x,
                y,
                "  Measuring Trash, Thumbnails, APT & Journal usage\u{2026}",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        }

        if area.height >= 2 {
            let rule_y = area.y + 1;
            let rule_style = Style::default().fg(design.color(ColorRole::SurfaceAlt));
            for col in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((col, rule_y)) {
                    cell.set_symbol("\u{2500}").set_style(rule_style);
                }
            }
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        let Some(&target) = self.targets.get(index) else {
            return Outcome::Continue;
        };
        match target {
            CleanTarget::InspectCache => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                let cache_dir = Path::new(&home).join(".cache");
                crate::system::spawn_detached(&format!(
                    "exec lazyos menu files '{}'",
                    cache_dir.display()
                ));
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            CleanTarget::Rescan => {
                self.banner = None;
                self.start_scan();
                Outcome::Consumed
            }
            _ => {
                self.banner = None;
                self.start_clean(target);
                Outcome::Consumed
            }
        }
    }
}

impl Screen for CleanScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((820, 480))
    }

    fn needs_tick(&self) -> Option<Duration> {
        self.runner.tick_rate()
    }

    fn tick(&mut self) -> Outcome {
        self.runner.tick();
        if let Some(op) = self.runner.poll() {
            match op {
                CleanOp::Measured(rec) => {
                    self.rec = Some(rec);
                    self.rebuild_rows(true);
                }
                CleanOp::Cleaned { message, ok, rec } => {
                    self.rec = Some(rec);
                    self.rebuild_rows(true);
                    self.banner = Some((
                        message,
                        if ok { ColorRole::Ok } else { ColorRole::Danger },
                    ));
                }
            }
            self.sync_chrome();
            return Outcome::Continue;
        }
        if self.runner.is_running() {
            self.sync_chrome();
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Free space"), true);
        let (hero, filter, rows) = Self::layout(ctx.area);
        self.draw_hero(ctx.buf, hero, ctx.design);
        let hint = if self.runner.is_running() {
            self.runner.label().unwrap_or("Working\u{2026}")
        } else if let Some((msg, _)) = &self.banner {
            msg.as_str()
        } else {
            "Enter cleans selected cache \u{00b7} r rescans \u{00b7} Escape closes"
        };
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            hint,
            self.list.visible_len(),
            self.list.len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if key.code == KeyCode::Esc {
            if !self.list.filter().is_empty() {
                self.list.set_filter("");
                return Outcome::Consumed;
            }
            return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
        }
        if self.list.filter().is_empty() && key.code == KeyCode::Char('r') {
            if !self.runner.is_running() {
                self.banner = None;
                self.start_scan();
            }
            return Outcome::Consumed;
        }
        let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL);
        if typed {
            self.list.type_filter(key);
            return Outcome::Consumed;
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, _, rows) = Self::layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

/// Interactive `lazyos menu clean`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = CleanScreen::new();
    crate::run_screen(terminal, design, "Free space", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_appropriately() {
        assert_eq!(format_bytes(0), "0 MB");
        assert_eq!(format_bytes(500 * 1024), "500 KB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn parse_journal_usage_extracts_bytes() {
        let raw = "Archived and active journals take up 64.0M in the file system.";
        assert_eq!(parse_journal_usage(raw), 64 * 1024 * 1024);
        let raw_g = "Journals take up 1.5G in the file system.";
        assert_eq!(
            parse_journal_usage(raw_g),
            (1.5 * 1024.0 * 1024.0 * 1024.0) as u64
        );
    }
}
