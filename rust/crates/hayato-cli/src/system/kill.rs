//! Process Manager & Killer (`hayato-kill`, `hayato menu kill`).
//!
//! Lists current user processes with PID, command name, and memory usage.
//! Separates the `Refresh` action into an [`ActionStrip`] above the filter bar
//! so the scrollable list holds strictly user processes. Filter as you type;
//! Enter sends SIGTERM, with in-window SIGKILL confirmation if the process
//! refuses to terminate.

use std::cell::Cell;
use std::process::Command;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Confirm, ConfirmAction, Design,
    Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_card,
    draw_filter_bar, draw_gauge,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::out;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub rss_kb: u64,
    pub cpu_pct: f32,
    pub comm: String,
}

/// Format KiB as a crisp human-readable memory string (`512 KB`, `248 MB`, `1.4 GB`).
pub fn format_mem_kb(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
    } else if kb >= 1024 {
        format!("{} MB", kb / 1024)
    } else {
        format!("{kb} KB")
    }
}

/// Parse `/proc/meminfo` into `(used_kb, total_kb)`.
pub fn parse_meminfo(raw: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok());
        }
    }
    let total = total?;
    let avail = available.unwrap_or(0).min(total);
    Some((total.saturating_sub(avail), total))
}

fn read_system_mem() -> Option<(u64, u64)> {
    let raw = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&raw)
}

/// Parse `ps -eo pid=,rss=,pcpu=,comm= --sort=-rss` (or legacy 3-col `pid=,rss=,comm=`) output.
pub fn parse_ps_table(raw: &str, self_pid: u32) -> Vec<ProcessRecord> {
    let mut procs = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        if pid <= 1 || pid == self_pid {
            continue;
        }
        let rss_kb = parts[1].parse::<u64>().unwrap_or(0);
        let (cpu_pct, comm) = if parts.len() >= 4 {
            if let Ok(cpu) = parts[2].parse::<f32>() {
                (cpu, parts[3..].join(" "))
            } else {
                (0.0, parts[2..].join(" "))
            }
        } else {
            (0.0, parts[2..].join(" "))
        };

        procs.push(ProcessRecord {
            pid,
            rss_kb,
            cpu_pct,
            comm,
        });
    }
    procs
}

enum KillMode {
    List,
    ConfirmKill {
        pid: u32,
        comm: String,
        confirm: Confirm,
    },
}

pub struct KillScreen {
    self_pid: u32,
    procs: Vec<ProcessRecord>,
    sys_mem: Option<(u64, u64)>,
    strip: ActionStrip,
    list: ListState,
    mode: KillMode,
    status_note: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl KillScreen {
    pub fn new() -> Self {
        let self_pid = std::process::id();
        let strip = ActionStrip::new(vec![
            ActionItem::new("refresh", "^R", "Refresh processes").glyph(Glyph::Sync),
        ]);
        let mut screen = Self {
            self_pid,
            procs: Vec::new(),
            sys_mem: None,
            strip,
            list: ListState::new().with_empty("no matching processes"),
            mode: KillMode::List,
            status_note: None,
            header: Header::new("Hayato").subtitle("Processes"),
            keybar: vec![
                KeyHint::new("Enter", "send SIGTERM"),
                KeyHint::new("Ctrl+R", "refresh"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(16),
        };
        screen.reload(false);
        screen
    }

    fn reload(&mut self, preserve: bool) {
        let ps_out =
            out(&["ps", "-eo", "pid=,rss=,pcpu=,comm=", "--sort=-rss"]).unwrap_or_default();
        self.procs = parse_ps_table(&ps_out, self.self_pid);
        let sys_mem = read_system_mem();
        self.sys_mem = sys_mem;
        let total_mem_kb = sys_mem.map(|(_, t)| t).unwrap_or(0);
        let rows: Vec<Row> = self
            .procs
            .iter()
            .take(60)
            .map(|p| {
                let mem_str = format_mem_kb(p.rss_kb);
                let value = if total_mem_kb > 0 {
                    let pct = (p.rss_kb as f64 / total_mem_kb as f64) * 100.0;
                    if pct >= 0.1 {
                        format!("{mem_str} ({pct:.1}%)")
                    } else {
                        mem_str
                    }
                } else {
                    mem_str
                };
                let detail = if p.cpu_pct >= 0.1 {
                    format!("pid {} \u{00b7} {:.1}% CPU", p.pid, p.cpu_pct)
                } else {
                    format!("pid {}", p.pid)
                };
                Row::new(&p.comm)
                    .glyph(Glyph::Bug)
                    .detail(detail)
                    .value(value)
            })
            .collect();
        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
        let status = self.status_note.clone().unwrap_or_else(|| {
            if let Some((used_kb, total_kb)) = sys_mem {
                let pct = if total_kb > 0 {
                    (used_kb as f64 / total_kb as f64 * 100.0).round() as u64
                } else {
                    0
                };
                format!(
                    "RAM {} / {} ({pct}%) \u{00b7} {} processes",
                    format_mem_kb(used_kb),
                    format_mem_kb(total_kb),
                    self.list.len()
                )
            } else {
                format!(
                    "{} user processes \u{00b7} Enter sends SIGTERM",
                    self.list.len()
                )
            }
        });
        self.header = Header::new("Hayato").subtitle("Processes").status(status);
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 56, 52)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Process Inspector"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 8 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let mut y = inner.y;

        if let Some((used_kb, total_kb)) = self.sys_mem {
            let pct = if total_kb > 0 {
                (used_kb as f64 / total_kb as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            put_trunc(buf, inner.x, y, "SYSTEM MEMORY", inner.width, accent);
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("{} / {}", format_mem_kb(used_kb), format_mem_kb(total_kb)),
                inner.width,
                body,
            );
            y += 1;
            let pct_label = format!("{:.0}%", pct * 100.0);
            draw_gauge(
                buf,
                Rect::new(inner.x, y, inner.width.min(36), 1),
                design,
                pct,
                &pct_label,
            );
            y += 2;
        }

        if let Some(idx) = self.list.selected_index()
            && let Some(p) = self.procs.get(idx)
        {
            put_trunc(buf, inner.x, y, "SELECTED PROCESS", inner.width, accent);
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &p.comm,
                inner.width,
                design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("PID       {}", p.pid),
                inner.width,
                body,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Memory    {}", format_mem_kb(p.rss_kb)),
                inner.width,
                body,
            );
            y += 1;
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("CPU       {:.1}%", p.cpu_pct),
                inner.width,
                body,
            );
        }

        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "Enter sends SIGTERM · prompts for SIGKILL if stubborn",
            inner.width,
            dim,
        );
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let strip = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1.min(inner.height),
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
        (strip, filter, rows)
    }

    fn signal_process(&mut self, index: usize) -> Outcome {
        let Some(p) = self.procs.get(index).cloned() else {
            return Outcome::Continue;
        };
        let pid_str = p.pid.to_string();
        let term_res = Command::new("kill").args(["-TERM", &pid_str]).status();
        if term_res.map(|s| s.success()).unwrap_or(false) {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let alive = Command::new("kill")
                .args(["-0", &pid_str])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if alive {
                let confirm = Confirm::new(
                    format!("Still running: {} (pid {})", p.comm, p.pid),
                    "Force-kill this process with SIGKILL?",
                )
                .with_detail("Unsaved data in that process will be lost.")
                .with_options(&["Leave running", "Force kill (SIGKILL)"])
                .destructive(true);
                self.mode = KillMode::ConfirmKill {
                    pid: p.pid,
                    comm: p.comm,
                    confirm,
                };
            } else {
                self.status_note = Some(format!("Ended {} (pid {})", p.comm, p.pid));
                self.reload(true);
            }
        } else {
            self.status_note = Some(format!(
                "Cannot signal {} ({}): not owned by you or already exited",
                p.comm, p.pid
            ));
            self.reload(true);
        }
        Outcome::Consumed
    }
}

impl Screen for KillScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        match &self.mode {
            KillMode::ConfirmKill { confirm, .. } => {
                confirm.render(ctx.area, ctx.buf, ctx.design);
            }
            KillMode::List => {
                let (left, right) = Self::split_panes(ctx.area);
                draw_card(ctx.buf, left, ctx.design, Some("By memory"), true);
                let (strip, filter, rows) = Self::layout(left);
                self.strip.render(strip, ctx.buf, ctx.design);
                let hint = self.status_note.as_deref().unwrap_or(
                    "Enter sends SIGTERM \u{00b7} type to filter \u{00b7} Ctrl+R refreshes",
                );
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
                if let Some(right_area) = right {
                    self.draw_inspector(ctx.buf, right_area, ctx.design);
                }
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match &mut self.mode {
            KillMode::ConfirmKill { pid, comm, confirm } => match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    let pid_val = *pid;
                    let comm_val = comm.clone();
                    let _ = Command::new("kill")
                        .args(["-KILL", &pid_val.to_string()])
                        .status();
                    self.mode = KillMode::List;
                    self.status_note = Some(format!("Force-killed {comm_val} (pid {pid_val})"));
                    self.reload(true);
                    Outcome::Consumed
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    let pid_val = *pid;
                    let comm_val = comm.clone();
                    self.mode = KillMode::List;
                    self.status_note = Some(format!("Left {comm_val} (pid {pid_val}) running"));
                    self.reload(true);
                    Outcome::Consumed
                }
                ConfirmAction::None => Outcome::Consumed,
            },
            KillMode::List => {
                match self.strip.handle(key) {
                    ActionStripEvent::Activated("refresh") => {
                        self.status_note = None;
                        self.reload(true);
                        return Outcome::Consumed;
                    }
                    ActionStripEvent::Activated(_) | ActionStripEvent::Consumed => {
                        return Outcome::Consumed;
                    }
                    ActionStripEvent::Ignored => {}
                }
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
                {
                    self.status_note = None;
                    self.reload(true);
                    return Outcome::Consumed;
                }
                if key.code == KeyCode::Esc {
                    if !self.list.filter().is_empty() {
                        self.list.set_filter("");
                        return Outcome::Consumed;
                    }
                    return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                }
                let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT);
                if typed {
                    self.list.type_filter(key);
                    return Outcome::Consumed;
                }
                match self.list.handle(key, self.rows_height.get(), design) {
                    ListAction::Activated(index) => self.signal_process(index),
                    _ => Outcome::Continue,
                }
            }
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let KillMode::ConfirmKill { pid, comm, confirm } = &mut self.mode {
            if let Some(idx) = row {
                let pid_val = *pid;
                let comm_val = comm.clone();
                match confirm.click(idx) {
                    ConfirmAction::Chosen(1) => {
                        let _ = Command::new("kill")
                            .args(["-KILL", &pid_val.to_string()])
                            .status();
                        self.mode = KillMode::List;
                        self.status_note = Some(format!("Force-killed {comm_val} (pid {pid_val})"));
                        self.reload(true);
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = KillMode::List;
                        self.status_note = Some(format!("Left {comm_val} (pid {pid_val}) running"));
                        self.reload(true);
                    }
                    ConfirmAction::None => {}
                }
            }
            return Outcome::Consumed;
        }
        if let Some(row) = row {
            if row == 10_000 {
                self.status_note = None;
                self.reload(true);
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.signal_process(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if let KillMode::ConfirmKill { confirm, .. } = &self.mode {
            return confirm.hit(area, design, x, y);
        }
        let (left, _) = Self::split_panes(area);
        let (strip, _, rows) = Self::layout(left);
        if let Some(idx) = self.strip.hit_index(strip, x, y) {
            return Some(10_000 + idx);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match self.mode {
            KillMode::List => Some(&self.keybar),
            KillMode::ConfirmKill { .. } => None,
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.mode {
            KillMode::List => "list",
            KillMode::ConfirmKill { .. } => "confirm-kill",
        })
    }
}

/// Interactive `hayato menu kill`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = KillScreen::new();
    crate::run_screen(terminal, design, "End a process", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ps_table_filters_pid_1_and_self() {
        let raw = "\
    1  1024 systemd
  100 20480 foot
  200 40960 rustc
  300  5120 hayato
";
        let procs = parse_ps_table(raw, 300);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 100);
        assert_eq!(procs[0].comm, "foot");
        assert_eq!(procs[1].pid, 200);
        assert_eq!(procs[1].comm, "rustc");
    }

    #[test]
    fn format_mem_kb_and_meminfo_render_cleanly() {
        assert_eq!(format_mem_kb(512), "512 KB");
        assert_eq!(format_mem_kb(256 * 1024), "256 MB");
        assert_eq!(format_mem_kb(2 * 1024 * 1024), "2.0 GB");

        let meminfo = "MemTotal:        4000000 kB\nMemFree:          500000 kB\nMemAvailable:    1500000 kB\n";
        assert_eq!(parse_meminfo(meminfo), Some((2_500_000, 4_000_000)));
    }
}
