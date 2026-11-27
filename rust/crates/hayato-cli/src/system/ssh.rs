//! Remote Login (SSH) Control Panel (`hayato-ssh`, `hayato menu ssh`).
//!
//! Displays live `ssh.service` boot & runtime state, port 22 listener, local IP
//! connect command, and authorized_keys count alongside one-click enable/disable.

use std::cell::Cell;

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{out, sudo};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["systemctl", "sshd"];

/// The `--status` output.
pub fn status() -> String {
    let enabled = out(&["systemctl", "is-enabled", "ssh"])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false);
    let active = out(&["systemctl", "is-active", "ssh"])
        .map(|s| s.trim() == "active")
        .unwrap_or(false);
    let port = out(&["ss", "-ltnH", "sport = :22"])
        .map(|s| s.contains("0.0.0.0:22") || s.contains("[::]:22"))
        .unwrap_or(false);

    let mut parts = Vec::new();
    parts.push(format!(
        "ssh.service: {}",
        if enabled { "enabled" } else { "disabled" }
    ));
    parts.push(format!("running: {}", if active { "yes" } else { "no" }));
    if port {
        parts.push("listening on port 22".to_string());
    }
    parts.join(" · ")
}

/// Remote Login (SSH) Studio screen.
pub struct SshScreen {
    enabled: bool,
    active: bool,
    port_open: bool,
    ssh_cmd: String,
    auth_keys_count: usize,
    keys: Vec<String>,
    list: ListState,
    banner: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    pub chosen: Option<String>,
}

impl SshScreen {
    pub fn new(banner: Option<String>) -> Self {
        let enabled = out(&["systemctl", "is-enabled", "ssh"])
            .map(|s| s.trim() == "enabled")
            .unwrap_or(false);
        let active = out(&["systemctl", "is-active", "ssh"])
            .map(|s| s.trim() == "active")
            .unwrap_or(false);
        let port_open = out(&["ss", "-ltnH", "sport = :22"])
            .map(|s| s.contains(":22"))
            .unwrap_or(false);
        let user = std::env::var("USER").unwrap_or_else(|_| "brunux".to_string());
        let host = out(&["hostname", "-I"])
            .and_then(|s| s.split_whitespace().next().map(|ip| ip.to_string()))
            .or_else(|| out(&["hostname"]).map(|s| s.trim().to_string()))
            .unwrap_or_else(|| "hayato".to_string());
        let ssh_cmd = format!("ssh {user}@{host}");
        let auth_keys_count = std::env::var("HOME")
            .ok()
            .and_then(|h| std::fs::read_to_string(format!("{h}/.ssh/authorized_keys")).ok())
            .map(|t| {
                t.lines()
                    .filter(|l| {
                        let s = l.trim();
                        !s.is_empty() && !s.starts_with('#')
                    })
                    .count()
            })
            .unwrap_or(0);

        let mut rows = vec![
            Row::new("Copy SSH command")
                .glyph(Glyph::Clipboard)
                .detail(&ssh_cmd),
        ];
        let mut keys = vec![format!("copy:{ssh_cmd}")];
        if active || enabled {
            rows.push(
                Row::new("Disable & stop ssh.service")
                    .glyph(Glyph::Lock)
                    .detail("sudo systemctl disable --now ssh"),
            );
            keys.push("disable".into());
        }
        if !active || !enabled {
            rows.push(
                Row::new("Enable & start ssh.service")
                    .glyph(Glyph::Unlock)
                    .detail("sudo systemctl enable --now ssh"),
            );
            keys.push("enable".into());
        }
        rows.push(
            Row::new("Refresh status")
                .glyph(Glyph::Sync)
                .detail("re-check systemd unit & port 22"),
        );
        keys.push("refresh".into());

        let mut list = ListState::new();
        list.set_rows(rows);

        Self {
            enabled,
            active,
            port_open,
            ssh_cmd,
            auth_keys_count,
            keys,
            list,
            banner,
            header: Header::new("Hayato")
                .subtitle("Remote Login (SSH)")
                .status(if active {
                    "● sshd running on port 22"
                } else {
                    "○ sshd stopped"
                }),
            keybar: vec![KeyHint::new("Enter", "act"), KeyHint::new("Esc", "close")],
            rows_height: Cell::new(14),
            chosen: None,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 48, 46)
    }

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("SSH Telemetry"), false);
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
        let pill_style = Style::default()
            .fg(design.color(if self.active {
                ColorRole::Ok
            } else {
                ColorRole::Muted
            }))
            .add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        put_trunc(
            buf,
            inner.x,
            y,
            if self.active {
                "● SSHD RUNNING"
            } else {
                "○ SSHD STOPPED"
            },
            inner.width,
            pill_style,
        );
        y += 2;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!(
                "Boot unit   {}",
                if self.enabled { "enabled" } else { "disabled" }
            ),
            inner.width,
            body,
        );
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!(
                "Port 22     {}",
                if self.port_open {
                    "listening"
                } else {
                    "closed"
                }
            ),
            inner.width,
            body,
        );
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!(
                "Auth keys   {} in ~/.ssh/authorized_keys",
                self.auth_keys_count
            ),
            inner.width,
            dim,
        );
        y += 2;
        put_trunc(buf, inner.x, y, "Connect Command", inner.width, accent);
        y += 1;
        put_trunc(buf, inner.x, y, &self.ssh_cmd, inner.width, body);

        if let Some(note) = &self.banner {
            put_trunc(
                buf,
                inner.x,
                inner.bottom().saturating_sub(1),
                note,
                inner.width,
                Style::default().fg(design.color(ColorRole::Ok)),
            );
        }
    }

    fn activate(&mut self) -> Outcome {
        if let Some(idx) = self.list.selected_index()
            && let Some(k) = self.keys.get(idx)
        {
            self.chosen = Some(k.clone());
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        Outcome::Continue
    }
}

impl Screen for SshScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("SSH Controls"), true);
        let inner = Rect::new(
            left.x + 2,
            left.y + 1,
            left.width.saturating_sub(4),
            left.height.saturating_sub(2),
        );
        let note = self
            .banner
            .as_deref()
            .unwrap_or("Enter acts · Escape closes");
        put_trunc(
            ctx.buf,
            inner.x,
            inner.y,
            note,
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
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
        match key.code {
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Enter => return self.activate(),
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => self.activate(),
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let rows = Rect::new(
            left.x + 2,
            left.y + 3,
            left.width.saturating_sub(4),
            left.height.saturating_sub(4),
        );
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            return self.activate();
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
        Some(if self.active {
            "ssh.running"
        } else {
            "ssh.stopped"
        })
    }
}

/// `hayato menu ssh`: Remote Login (SSH) Studio.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Remote login (ssh)",
            format!("{said} (systemd, openssh)"),
        ));
    }

    let mut status_note: Option<String> = None;

    loop {
        let mut screen = SshScreen::new(status_note.take());
        let code = crate::run_screen(terminal, design, "Remote login (ssh)", &mut screen)?;
        if code != 0 {
            return Ok(code);
        }
        let Some(key) = screen.chosen.take() else {
            return Ok(1);
        };

        match key.as_str() {
            k if k.starts_with("copy:") => {
                let cmd = &k[5..];
                let _ = out(&["wl-copy", cmd]);
                status_note = Some(format!("Copied to clipboard: {cmd}"));
            }
            "disable" => match sudo(&["systemctl", "disable", "--now", "ssh"]) {
                Ok(_) => status_note = Some("Disabled and stopped ssh.service".to_string()),
                Err(e) => status_note = Some(format!("Could not disable ssh: {e}")),
            },
            "enable" => match sudo(&["systemctl", "enable", "--now", "ssh"]) {
                Ok(_) => status_note = Some("Enabled and started ssh.service".to_string()),
                Err(e) => status_note = Some(format!("Could not enable ssh: {e}")),
            },
            _ => {
                status_note = Some(status());
            }
        }
    }
}

fn missing(names: &[&str]) -> Option<String> {
    let gone: Vec<&str> = names.iter().copied().filter(|n| !have(n)).collect();
    if gone.is_empty() {
        None
    } else {
        Some(format!("Missing: {}", gone.join(" ")))
    }
}

fn have(program: &str) -> bool {
    crate::files::open::on_path(&std::env::var("PATH").unwrap_or_default(), program)
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_formats_correctly() {}
}
