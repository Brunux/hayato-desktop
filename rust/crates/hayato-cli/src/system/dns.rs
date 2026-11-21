//! DNS: point the active connection at a resolver, or back at DHCP (`hayato-dns`).
//!
//! NetworkManager keeps /etc/resolv.conf, so the switch is two nmcli
//! properties on the connection that is up — `ipv4.dns` and
//! `ipv4.ignore-auto-dns` — followed by bringing that connection up again
//! so the change is applied at once.
//!
//! That re-activation is why this says something before it acts: a Wi-Fi
//! connection that comes back up costs a second or two, and a menu that is
//! already closed while the network blinks is indistinguishable from a bug.
//! The `busy` card stays until the connection answers again and then becomes
//! the read-back from NetworkManager, not the value that was asked for.
//!
//! The card registers no stop command, and that is a decision, not an
//! omission. Killing `nmcli connection up` is safe in itself (NetworkManager
//! finishes or drops the activation on its own), and it is still the wrong
//! button: by the time the activation runs, `connection modify` has already
//! written the new servers to the profile, so a stop leaves the profile
//! saying Cloudflare while resolv.conf still holds whatever DHCP handed out
//! — a mismatch with nothing on screen to explain it.
//!
//! DNS is not the same as a VPN: the addresses below are public resolvers
//! reached in the clear unless the network itself is encrypted. Nothing here
//! touches IPv6 DNS (`ipv6.dns`), which the same nmcli line could set.

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

use super::pick::{Picker, Question};
use super::{out, sudo};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["nmcli"];

/// Get the active non-loopback device.
fn active_device() -> Option<String> {
    out(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE", "device", "status"])
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            let parts: Vec<&str> = l.split(':').collect();
            if parts.len() >= 3 && parts[2] == "connected" && parts[1] != "loopback" {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
}

/// Get the active connection name for a device.
fn active_connection(dev: &str) -> Option<String> {
    out(&["nmcli", "-g", "GENERAL.CONNECTION", "device", "show", dev])
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .filter(|s| !s.is_empty() && s != "--")
}

/// The `--status` output.
pub fn status() -> String {
    let Some(dev) = active_device() else {
        return "No connection is up".to_string();
    };

    let Some(con) = active_connection(&dev) else {
        return format!("{} has no active connection", dev);
    };

    let current = out(&["nmcli", "-g", "IP4.DNS", "device", "show", &dev]).unwrap_or_default();
    let current = current.lines().collect::<Vec<_>>().join(", ");
    let auto = out(&[
        "nmcli",
        "-g",
        "ipv4.ignore-auto-dns",
        "connection",
        "show",
        &con,
    ])
    .unwrap_or_default();
    let auto = if auto == "no" { "no" } else { "yes" };

    format!(
        "On {} ({}) · now: {} · ignore-auto-dns: {}",
        con,
        dev,
        if current.is_empty() {
            "automatic".to_string()
        } else {
            current
        },
        auto
    )
}

const RESOLVERS: &[(&str, &str, &str)] = &[
    (
        "Automatic (DHCP)",
        "",
        "Uses the DNS servers handed out by your router or ISP.",
    ),
    (
        "Cloudflare",
        "1.1.1.1,1.0.0.1",
        "Fast global anycast resolver focused on speed and privacy.",
    ),
    (
        "Google",
        "8.8.8.8,8.8.4.4",
        "Reliable high-availability public DNS with DNSSEC validation.",
    ),
    (
        "Quad9",
        "9.9.9.9,149.112.112.112",
        "Non-profit resolver blocking known phishing and malware domains.",
    ),
    (
        "AdGuard",
        "94.140.14.14,94.140.15.15",
        "Blocks ads, trackers, and analytics domains at the DNS layer.",
    ),
];

/// DNS Resolver Studio screen.
pub struct DnsScreen {
    con: String,
    dev: String,
    current: String,
    ignore_auto: bool,
    keys: Vec<String>,
    list: ListState,
    banner: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    pub chosen: Option<String>,
}

impl DnsScreen {
    pub fn new(
        con: String,
        dev: String,
        current: String,
        ignore_auto: bool,
        banner: Option<String>,
    ) -> Self {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for (name, servers, _) in RESOLVERS {
            let in_use = if servers.is_empty() {
                !ignore_auto
            } else {
                ignore_auto && current == servers.replace(',', " ")
            };
            let detail = if servers.is_empty() {
                if current.is_empty() {
                    "network DHCP default".to_string()
                } else {
                    current.clone()
                }
            } else {
                (*servers).to_string()
            };
            let mut r = Row::new(*name).glyph(Glyph::Cloud).detail(detail);
            if in_use {
                r = r.value("● in use");
            }
            rows.push(r);
            keys.push(if servers.is_empty() {
                "automatic".into()
            } else {
                format!("dns:{servers}")
            });
        }
        rows.push(
            Row::new("Custom IPv4 servers…")
                .glyph(Glyph::Search)
                .detail("comma or space separated IPv4 addresses"),
        );
        keys.push("custom".into());

        let mut list = ListState::new();
        list.set_rows(rows);

        Self {
            header: Header::new("Hayato")
                .subtitle("DNS Studio")
                .status(format!("{con} ({dev})")),
            con,
            dev,
            current,
            ignore_auto,
            keys,
            list,
            banner,
            keybar: vec![KeyHint::new("Enter", "apply"), KeyHint::new("Esc", "close")],
            rows_height: Cell::new(14),
            chosen: None,
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 50) / 100).max(46) as u16;
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

    fn draw_inspector(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Resolver Inspector"), false);
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
        put_trunc(buf, inner.x, y, "ACTIVE CONNECTION", inner.width, accent);
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!("{} ({})", self.con, self.dev),
            inner.width,
            design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
        );
        y += 1;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!(
                "Current   {}",
                if self.current.is_empty() {
                    "automatic (DHCP)"
                } else {
                    self.current.as_str()
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
                "Mode      {}",
                if self.ignore_auto {
                    "Static (ignore DHCP DNS)"
                } else {
                    "Automatic (DHCP)"
                }
            ),
            inner.width,
            dim,
        );
        y += 2;

        if let Some(idx) = self.list.selected_index() {
            put_trunc(buf, inner.x, y, "Selected Preset", inner.width, accent);
            y += 1;
            if let Some((name, servers, desc)) = RESOLVERS.get(idx) {
                put_trunc(buf, inner.x, y, name, inner.width, body);
                y += 1;
                if !servers.is_empty() {
                    put_trunc(buf, inner.x, y, servers, inner.width, dim);
                    y += 1;
                }
                put_trunc(buf, inner.x, y + 1, desc, inner.width, dim);
            } else {
                put_trunc(
                    buf,
                    inner.x,
                    y,
                    "Enter custom comma or space separated IPv4 resolvers.",
                    inner.width,
                    dim,
                );
            }
        }

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

impl Screen for DnsScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Resolvers"), true);
        let inner = Rect::new(
            left.x + 2,
            left.y + 1,
            left.width.saturating_sub(4),
            left.height.saturating_sub(2),
        );
        let default_note = format!("On {} ({}) · Enter applies", self.con, self.dev);
        let note = self.banner.as_deref().unwrap_or(&default_note);
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
        Some("dns.studio")
    }
}

/// `hayato menu dns` (`hayato-dns`): DNS Resolver Studio.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "DNS",
            format!("{said} (NetworkManager)"),
        ));
    }

    let mut status_note: Option<String> = None;

    loop {
        let dev = match active_device() {
            Some(d) => d,
            None => {
                return Err(crate::Failure::new(
                    "DNS",
                    "No connection is up, so there is nothing to change",
                ));
            }
        };

        let con = match active_connection(&dev) {
            Some(c) => c,
            None => {
                return Err(crate::Failure::new(
                    "DNS",
                    format!("{dev} has no active connection"),
                ));
            }
        };

        let current = out(&["nmcli", "-g", "IP4.DNS", "device", "show", &dev]).unwrap_or_default();
        let current = current.lines().collect::<Vec<_>>().join(" ");
        let auto = out(&[
            "nmcli",
            "-g",
            "ipv4.ignore-auto-dns",
            "connection",
            "show",
            &con,
        ])
        .unwrap_or_default();
        let ignore_auto = auto.trim() != "no";

        let mut screen = DnsScreen::new(
            con.clone(),
            dev.clone(),
            current,
            ignore_auto,
            status_note.take(),
        );
        let code = crate::run_screen(terminal, design, "DNS", &mut screen)?;
        if code != 0 {
            return Ok(code);
        }
        let Some(key) = screen.chosen.take() else {
            return Ok(1);
        };

        let servers = match key.as_str() {
            "automatic" => String::new(),
            k if k.starts_with("dns:") => k[4..].to_string(),
            "custom" => {
                let mut ask_picker = Picker::new(
                    "DNS",
                    vec![Question::input(
                        "DNS servers",
                        "IPv4 addresses, comma or space separated (e.g. 1.1.1.1, 9.9.9.9)",
                        "1.1.1.1, 9.9.9.9",
                        false,
                    )],
                );
                let ask_code = crate::run_screen(terminal, design, "DNS", &mut ask_picker)?;
                if ask_code != 0 {
                    continue;
                }
                let input = ask_picker
                    .answers()
                    .first()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if input.is_empty() {
                    continue;
                }
                input
                    .replace([' ', ','], "\n")
                    .lines()
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(",")
            }
            _ => continue,
        };

        // Check for root/sudo
        if !sudo_available() {
            status_note = Some(
                "Changing a connection needs root and sudo asks for a password here".to_string(),
            );
            continue;
        }

        let result = if servers.is_empty() {
            sudo(&[
                "nmcli",
                "connection",
                "modify",
                &con,
                "ipv4.ignore-auto-dns",
                "no",
                "ipv4.dns",
                "",
            ])
            .and_then(|_| sudo(&["nmcli", "connection", "up", &con]))
        } else {
            sudo(&[
                "nmcli",
                "connection",
                "modify",
                &con,
                "ipv4.dns",
                &servers,
                "ipv4.ignore-auto-dns",
                "yes",
            ])
            .and_then(|_| sudo(&["nmcli", "connection", "up", &con]))
        };

        match result {
            Ok(_) => {
                std::thread::sleep(std::time::Duration::from_millis(800));
                let now =
                    out(&["nmcli", "-g", "IP4.DNS", "device", "show", &dev]).unwrap_or_default();
                let now = now.lines().collect::<Vec<_>>().join(" ");
                if now.is_empty() {
                    status_note = Some("Back on the network's automatic DHCP servers".to_string());
                } else {
                    status_note = Some(format!("DNS is now {now}"));
                }
            }
            Err(e) => {
                status_note = Some(format!("Could not apply DNS: {e}"));
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

fn sudo_available() -> bool {
    sudo(&["true"]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_available_works() {
        // Runs without panicking regardless of container sandbox privileges.
        let _ = sudo_available();
    }
}
