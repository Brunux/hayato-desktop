//! Process Manager & Killer (`hayato-kill`, `hayato menu kill`).
//!
//! Lists current user processes with PID, command name, and memory usage.
//! Separates the `Refresh` action into an [`ActionStrip`] above the filter bar
//! so the scrollable list holds strictly user processes. Filter as you type;
//! Enter sends SIGTERM, with in-window SIGKILL confirmation if the process
//! refuses to terminate.

use std::cell::Cell;
use std::process::Command;

use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, Confirm, ConfirmAction, Design, Glyph,
    Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use super::out;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub rss_kb: u64,
    pub comm: String,
}

/// Parse `ps -eo pid=,rss=,comm= --sort=-rss` output.
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
        let comm = parts[2..].join(" ");

        procs.push(ProcessRecord { pid, rss_kb, comm });
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
        let ps_out = out(&["ps", "-eo", "pid=,rss=,comm=", "--sort=-rss"]).unwrap_or_default();
        self.procs = parse_ps_table(&ps_out, self.self_pid);
        let rows: Vec<Row> = self
            .procs
            .iter()
            .take(60)
            .map(|p| {
                let mb = p.rss_kb / 1024;
                Row::new(&p.comm)
                    .glyph(Glyph::Bug)
                    .detail(format!("pid {}", p.pid))
                    .value(format!("{mb} MB"))
            })
            .collect();
        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
        let status = self.status_note.clone().unwrap_or_else(|| {
            format!(
                "{} user processes \u{00b7} Enter sends SIGTERM",
                self.list.len()
            )
        });
        self.header = Header::new("Hayato").subtitle("Processes").status(status);
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
                    "Force-kill with SIGKILL? Unsaved data in that process will be lost.",
                )
                .with_options(&["Leave running", "Force kill (SIGKILL)"]);
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
                draw_card(ctx.buf, ctx.area, ctx.design, Some("By memory"), true);
                let (strip, filter, rows) = Self::layout(ctx.area);
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
                    && !key.modifiers.contains(KeyModifiers::CONTROL);
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
        if !matches!(self.mode, KillMode::List) {
            return Outcome::Continue;
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
        if !matches!(self.mode, KillMode::List) {
            return None;
        }
        let (strip, _, rows) = Self::layout(area);
        if y == strip.y && x >= strip.x && x < strip.right() {
            return Some(10_000);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
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
}
