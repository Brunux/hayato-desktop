//! Disk Cleaner (`hayato-clean`, `hayato menu clean`).
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

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    ActionItem, ActionRunner, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Design, Glyph,
    Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_badge, draw_card,
    draw_filter_bar, draw_gauge,
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

/// Fast directory size via `du -sb`, with recursive Rust fallback.
pub fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    if let Some(s) = path.to_str()
        && let Some(raw) = out(&["du", "-sb", s])
        && let Some(first) = raw.split_whitespace().next()
        && let Ok(bytes) = first.parse::<u64>()
    {
        return bytes;
    }
    dir_size_walk(path)
}

fn dir_size_walk(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    total += dir_size_walk(&entry.path());
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
                let num_str = &val_str[..val_str.len() - ch.len_utf8()];
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

    let journal_raw = out(&["journalctl", "--disk-usage"]).unwrap_or_default();
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

/// Headless query mode: `hayato menu clean --dry-run`.
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
}

pub struct CleanScreen {
    rec: Option<Reclaimable>,
    targets: Vec<CleanTarget>,
    strip: ActionStrip,
    runner: ActionRunner<CleanOp>,
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
            strip: ActionStrip::default(),
            runner: ActionRunner::new(),
            list: ListState::new(),
            header: Header::new("Hayato").subtitle("Free space"),
            keybar: vec![
                KeyHint::new("Enter", "clean item"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Ctrl+A", "clean all safe"),
                KeyHint::new("Ctrl+R", "rescan"),
                KeyHint::new("Esc", "back"),
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
    }

    fn start_clean(&mut self, target: CleanTarget) {
        let label = match target {
            CleanTarget::Trash => "Emptying trash\u{2026}",
            CleanTarget::Thumbnails => "Removing thumbnail cache\u{2026}",
            CleanTarget::Apt => "Cleaning APT package cache\u{2026}",
            CleanTarget::Journal => "Vacuuming systemd journal to 100 MB\u{2026}",
            CleanTarget::AllSafe => "Cleaning trash, thumbnails, APT & journal\u{2026}",
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
            }
            let rec = calculate_reclaimable();
            let after = rec.safe_reclaimable_bytes();
            let freed = before.saturating_sub(after);
            let message = if !ok {
                format!("Cleanup error: {err_msg}")
            } else {
                match target {
                    CleanTarget::Trash => {
                        format!("Trash emptied \u{00b7} freed {}", format_bytes(freed))
                    }
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
                }
            };
            CleanOp::Cleaned { message, ok, rec }
        });
    }

    fn rebuild_rows(&mut self, preserve: bool) {
        let safe_label = match &self.rec {
            Some(rec) => format!(
                "Clean all safe ({})",
                format_bytes(rec.safe_reclaimable_bytes())
            ),
            None => "Clean all safe".to_string(),
        };
        let cache_label = match &self.rec {
            Some(rec) => format!("Inspect ~/.cache ({})", format_bytes(rec.cache_bytes)),
            None => "Inspect ~/.cache".to_string(),
        };
        self.strip.set_items(vec![
            ActionItem::new("all", "^A", safe_label).glyph(Glyph::Sparkles),
            ActionItem::new("rescan", "^R", "Rescan").glyph(Glyph::Sync),
            ActionItem::new("inspect", "^O", cache_label).glyph(Glyph::Folder),
        ]);

        let mut rows = Vec::new();
        let mut targets = Vec::new();

        if let Some(rec) = &self.rec {
            let max_cat = [
                rec.trash_bytes,
                rec.thumb_bytes,
                rec.apt_bytes,
                rec.journal_bytes,
            ]
            .into_iter()
            .max()
            .unwrap_or(1)
            .max(1);

            rows.push(
                Row::new("Trash")
                    .glyph(Glyph::Trash)
                    .detail(format!(
                        "{}  ~/.local/share/Trash",
                        mini_bar(rec.trash_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.trash_bytes)),
            );
            targets.push(CleanTarget::Trash);

            rows.push(
                Row::new("Thumbnails")
                    .glyph(Glyph::Image)
                    .detail(format!(
                        "{}  ~/.cache/thumbnails",
                        mini_bar(rec.thumb_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.thumb_bytes)),
            );
            targets.push(CleanTarget::Thumbnails);

            rows.push(
                Row::new("APT package cache")
                    .glyph(Glyph::Box)
                    .detail(format!(
                        "{}  /var/cache/apt/archives",
                        mini_bar(rec.apt_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.apt_bytes)),
            );
            targets.push(CleanTarget::Apt);

            rows.push(
                Row::new("Systemd journal logs")
                    .glyph(Glyph::Book)
                    .detail(format!(
                        "{}  shrink to \u{2264} 100 MB",
                        mini_bar(rec.journal_bytes, max_cat)
                    ))
                    .value(format_bytes(rec.journal_bytes)),
            );
            targets.push(CleanTarget::Journal);
        } else {
            let calc = "calculating\u{2026}";
            rows.push(
                Row::new("Trash")
                    .glyph(Glyph::Trash)
                    .detail("~/.local/share/Trash")
                    .value(calc),
            );
            targets.push(CleanTarget::Trash);
            rows.push(
                Row::new("Thumbnails")
                    .glyph(Glyph::Image)
                    .detail("~/.cache/thumbnails")
                    .value(calc),
            );
            targets.push(CleanTarget::Thumbnails);
            rows.push(
                Row::new("APT package cache")
                    .glyph(Glyph::Box)
                    .detail("/var/cache/apt/archives")
                    .value(calc),
            );
            targets.push(CleanTarget::Apt);
            rows.push(
                Row::new("Systemd journal logs")
                    .glyph(Glyph::Book)
                    .detail("shrink to \u{2264} 100 MB")
                    .value(calc),
            );
            targets.push(CleanTarget::Journal);
        }

        self.targets = targets;
        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect, Rect) {
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
        let strip = Rect {
            x: inner.x,
            y: inner.y.saturating_add(2).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(2)),
        };
        let filter = Rect {
            x: inner.x,
            y: inner.y.saturating_add(4).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(4)),
        };
        let rows_y = inner.y.saturating_add(6).min(inner.bottom());
        let rows = Rect {
            x: inner.x,
            y: rows_y,
            width: inner.width,
            height: inner.bottom().saturating_sub(rows_y),
        };
        (hero, strip, filter, rows)
    }

    fn draw_hero(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        if area.height == 0 || area.width < 20 {
            return;
        }
        let y = area.y;
        if self.runner.is_running() {
            let badge = format!("{} WORKING", self.runner.spinner());
            let x = draw_badge(buf, area.x, y, design, &badge, ColorRole::Accent);
            let _ = put_trunc(
                buf,
                x + 2,
                y,
                self.runner.label().unwrap_or("Working\u{2026}"),
                area.right().saturating_sub(x + 2),
                design.style(TextStyle::Title),
            );
        } else if let Some(notice) = self.runner.notice() {
            let (badge_txt, role, style) = if notice.is_error {
                ("ERROR", ColorRole::Danger, design.style(TextStyle::Danger))
            } else {
                (
                    "\u{2714} CLEANED",
                    ColorRole::Ok,
                    design.style(TextStyle::Title),
                )
            };
            let x = draw_badge(buf, area.x, y, design, badge_txt, role);
            let _ = put_trunc(
                buf,
                x + 2,
                y,
                &notice.text,
                area.right().saturating_sub(x + 2),
                style,
            );
        } else if let Some(rec) = &self.rec {
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

    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        match id {
            "all" => {
                self.start_clean(CleanTarget::AllSafe);
                Outcome::Consumed
            }
            "rescan" => {
                self.start_scan();
                Outcome::Consumed
            }
            "inspect" => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
                let cache_dir = Path::new(&home).join(".cache");
                let cache_str = cache_dir.to_string_lossy().into_owned();
                crate::system::spawn_detached_argv("hayato", &["menu", "files", &cache_str]);
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            _ => Outcome::Consumed,
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        let Some(&target) = self.targets.get(index) else {
            return Outcome::Continue;
        };
        self.start_clean(target);
        Outcome::Consumed
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
        if let Some(op) = self.runner.poll() {
            match op {
                CleanOp::Measured(rec) => {
                    self.rec = Some(rec);
                    self.rebuild_rows(true);
                    // Routine measurement clears the scanning banner immediately.
                    self.runner.clear_status();
                }
                CleanOp::Cleaned { message, ok, rec } => {
                    self.rec = Some(rec);
                    self.rebuild_rows(true);
                    // Override auto-hide after cleanup so the user sees how much space was freed
                    // (or why cleanup failed) until the next action.
                    if ok {
                        self.runner.set_persistent_info(message);
                    } else {
                        self.runner.set_persistent_error(message);
                    }
                }
            }
            return Outcome::Continue;
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(
            ctx.buf,
            ctx.area,
            ctx.design,
            Some("What is using the disk"),
            true,
        );
        let (hero, strip, filter, rows) = Self::layout(ctx.area);
        self.draw_hero(ctx.buf, hero, ctx.design);
        self.strip.render(strip, ctx.buf, ctx.design);
        let hint = "Enter cleans selected category \u{00b7} Tab focuses actions \u{00b7} Ctrl+A cleans all safe";
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
        self.list
            .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match self.strip.handle(key) {
            ActionStripEvent::Activated(id) => return self.run_strip_action(id),
            ActionStripEvent::Consumed => return Outcome::Consumed,
            ActionStripEvent::Ignored => {}
        }
        if key.code == KeyCode::Esc {
            if !self.list.filter().is_empty() {
                self.list.set_filter("");
                return Outcome::Consumed;
            }
            return Outcome::Done(hayato_ui::app::exit::ESCAPE);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    return self.run_strip_action("all");
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    return self.run_strip_action("rescan");
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    return self.run_strip_action("inspect");
                }
                _ => {}
            }
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
            if row >= 10_000 {
                let strip_idx = row - 10_000;
                if let Some(item) = self.strip.items().get(strip_idx) {
                    return self.run_strip_action(item.id);
                }
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, strip, _, rows) = Self::layout(area);
        if let Some(idx) = self.strip.hit_index(strip, x, y) {
            return Some(10_000 + idx);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

/// Interactive `hayato menu clean`.
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
