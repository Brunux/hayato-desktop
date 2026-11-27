//! Wi-Fi & Network Manager (`bar-net`, `hayato menu net`).
//!
//! Renders cached Wi-Fi networks immediately on open while launching a
//! non-blocking background channel scan (`nmcli device wifi list --rescan yes`)
//! that updates the list in-place while preserving cursor and filter state.
//! Displays a connected-network hero card, 4-bar signal meters, in-window
//! masked WPA password entry, and inline Wi-Fi QR code rendering with dynamic
//! window resizing. Also provides headless `--status` JSON for waybar.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    ActionItem, ActionRunner, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Confirm,
    ConfirmAction, Design, Glyph, Header, Input, InputAction, KeyHint, ListAction, ListState,
    Outcome, QrCode, Row, Screen, TextStyle, draw_badge, draw_card, draw_empty, draw_filter_bar,
    draw_signal_bars, signal_bars,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{have, out};

/// One access point record parsed from nmcli.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiAp {
    pub in_use: bool,
    pub ssid: String,
    pub security: String,
    pub signal: u8,
}

/// Convert a 0..=100 signal percentage into a single-character step bar.
pub fn signal_bar(signal: u8) -> &'static str {
    match signal {
        0..=19 => " ",
        20..=39 => "\u{2582}",
        40..=59 => "\u{2583}",
        60..=79 => "\u{2584}",
        _ => "\u{2585}",
    }
}

/// Parse nmcli tabular output into structured, de-duplicated AP records.
///
/// Input format: `IN-USE:SSID:SECURITY:SIGNAL`
/// Where colons in SSID are escaped as `\:`.
/// Duplicate SSIDs are collapsed to the strongest signal; active networks sort first.
pub fn parse_nmcli_wifi(raw: &str) -> Vec<WifiAp> {
    let mut best: BTreeMap<String, WifiAp> = BTreeMap::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.peek() == Some(&':') {
                cur.push(':');
                chars.next();
            } else if ch == ':' {
                fields.push(cur);
                cur = String::new();
            } else {
                cur.push(ch);
            }
        }
        fields.push(cur);

        if fields.len() < 4 {
            continue;
        }

        let in_use = fields[0] == "*";
        let signal: u8 = fields.last().and_then(|s| s.parse().ok()).unwrap_or(0);
        let security = fields[fields.len() - 2].clone();
        let ssid = fields[1..fields.len() - 2].join(":");

        if ssid.is_empty() {
            continue;
        }

        let ap = WifiAp {
            in_use,
            ssid: ssid.clone(),
            security,
            signal,
        };

        match best.get(&ssid) {
            Some(existing) => {
                if ap.in_use || (ap.signal > existing.signal && !existing.in_use) {
                    best.insert(ssid, ap);
                }
            }
            None => {
                best.insert(ssid, ap);
            }
        }
    }

    let mut list: Vec<WifiAp> = best.into_values().collect();
    list.sort_by(|a, b| {
        b.in_use
            .cmp(&a.in_use)
            .then_with(|| b.signal.cmp(&a.signal))
            .then_with(|| a.ssid.cmp(&b.ssid))
    });
    list
}

fn signal_bars_4(signal: u8) -> &'static str {
    match signal {
        0 => "<span alpha='30%'>▂▄▆█</span>",
        1..=24 => "▂<span alpha='30%'>▄▆█</span>",
        25..=49 => "▂▄<span alpha='30%'>▆█</span>",
        50..=74 => "▂▄▆<span alpha='30%'>█</span>",
        _ => "▂▄▆█",
    }
}

fn format_status_json(radio_enabled: bool, active: Option<&WifiAp>) -> String {
    if !radio_enabled {
        return r#"{"text":"off","tooltip":"<span alpha='50%'>╭─</span> <b>NET // RADIO OFF</b>\n<span alpha='45%'>│</span> <span alpha='60%'>WIFI</span>  disabled\n<span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] Wi-Fi · [RMB] VPN</span>","class":"disabled","percentage":0}"#
            .to_string();
    }

    if let Some(active) = active {
        let ssid_pango = escape_pango(&active.ssid);
        let sec = if active.security.is_empty() {
            "open".to_string()
        } else {
            escape_pango(&active.security)
        };
        let bars = signal_bars_4(active.signal);
        format!(
            r#"{{"text":"{}","tooltip":"<span alpha='50%'>╭─</span> <b>NET // WI-FI</b>\n<span alpha='45%'>│</span> <span alpha='60%'>SSID</span>  {}\n<span alpha='45%'>│</span> <span alpha='60%'>SIG </span>  {}  {}%\n<span alpha='45%'>│</span> <span alpha='60%'>SEC </span>  {}\n<span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] Wi-Fi · [RMB] VPN</span>","class":"connected","percentage":{}}}"#,
            escape_json(&active.ssid),
            escape_json(&ssid_pango),
            bars,
            active.signal,
            escape_json(&sec),
            active.signal
        )
    } else {
        r#"{"text":"disconnected","tooltip":"<span alpha='50%'>╭─</span> <b>NET // OFFLINE</b>\n<span alpha='45%'>│</span> <span alpha='60%'>WIFI</span>  disconnected\n<span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] Wi-Fi · [RMB] VPN</span>","class":"disconnected","percentage":0}"#
            .to_string()
    }
}

/// Headless query for waybar `custom/net` module: emits JSON.
pub fn status_json() -> String {
    let radio = out(&["nmcli", "radio", "wifi"]).unwrap_or_default();
    if radio != "enabled" {
        return format_status_json(false, None);
    }

    let scan_raw = out(&[
        "nmcli",
        "--terse",
        "--fields",
        "IN-USE,SSID,SECURITY,SIGNAL",
        "device",
        "wifi",
        "list",
        "--rescan",
        "no",
    ])
    .unwrap_or_default();

    let aps = parse_nmcli_wifi(&scan_raw);
    let active = aps.iter().find(|ap| ap.in_use);
    format_status_json(true, active)
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_pango(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn has_saved_connection(ssid: &str) -> bool {
    let conns =
        out(&["nmcli", "--terse", "--fields", "NAME", "connection", "show"]).unwrap_or_default();
    conns.lines().any(|l| l.trim() == ssid)
}

fn query_ipv4() -> Option<String> {
    let raw = out(&["nmcli", "-t", "-f", "TYPE,STATE,DEVICE", "device", "status"])?;
    let dev = raw
        .lines()
        .find(|l| l.starts_with("wifi:connected:"))
        .and_then(|l| l.split(':').nth(2))?;
    let ip = out(&["nmcli", "-g", "IP4.ADDRESS", "device", "show", dev])?;
    let first = ip.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.split('/').next().unwrap_or(first).to_string())
    }
}

fn scan_wifi(rescan: bool) -> (bool, Vec<WifiAp>, Option<String>) {
    let radio = out(&["nmcli", "radio", "wifi"]).unwrap_or_default() == "enabled";
    if !radio {
        return (false, Vec::new(), None);
    }
    let arg = if rescan { "yes" } else { "no" };
    let scan_raw = out(&[
        "nmcli",
        "--terse",
        "--fields",
        "IN-USE,SSID,SECURITY,SIGNAL",
        "device",
        "wifi",
        "list",
        "--rescan",
        arg,
    ])
    .unwrap_or_default();
    let aps = parse_nmcli_wifi(&scan_raw);
    let ip = if aps.iter().any(|a| a.in_use) {
        query_ipv4()
    } else {
        None
    };
    (true, aps, ip)
}

fn connect_to(ssid: &str, password: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("nmcli");
    cmd.args(["device", "wifi", "connect", ssid]);
    if let Some(pw) = password {
        cmd.args(["password", pw]);
    }
    let res = cmd.output().map_err(|e| e.to_string())?;
    if res.status.success() {
        return Ok(String::from_utf8_lossy(&res.stdout).trim().to_string());
    }
    if password.is_none() && has_saved_connection(ssid) {
        let up = Command::new("nmcli")
            .args(["connection", "up", "id", ssid])
            .output()
            .map_err(|e| e.to_string())?;
        if up.status.success() {
            return Ok(String::from_utf8_lossy(&up.stdout).trim().to_string());
        }
    }
    let stderr = String::from_utf8_lossy(&res.stderr);
    let msg = stderr.lines().next().unwrap_or("connection failed").trim();
    Err(if msg.is_empty() {
        "connection refused or timed out".to_string()
    } else {
        msg.to_string()
    })
}

enum NetOp {
    Scanned {
        radio_on: bool,
        aps: Vec<WifiAp>,
        ip: Option<String>,
        portal: bool,
    },
    Connected {
        ssid: String,
        result: Result<(), String>,
        radio_on: bool,
        aps: Vec<WifiAp>,
        ip: Option<String>,
        portal: bool,
    },
    RadioToggled {
        radio_on: bool,
        aps: Vec<WifiAp>,
        ip: Option<String>,
        portal: bool,
    },
}

enum NetMode {
    List,
    Password {
        ssid: String,
        input: Input,
    },
    WifiQr {
        qr: QrCode,
        title: String,
        subtitle: String,
        payload: String,
    },
    ConfirmForget {
        ssid: String,
        confirm: Confirm,
    },
}

/// The AP behind a visible row. The portal row takes the first slot, so every
/// network row moves down by one while it is present.
fn ap_index(portal: bool, row: usize) -> Option<usize> {
    if portal {
        row.checked_sub(1)
    } else {
        Some(row)
    }
}

pub struct NetScreen {
    radio_on: bool,
    aps: Vec<WifiAp>,
    active_ip: Option<String>,
    portal: bool,
    strip: ActionStrip,
    list: ListState,
    mode: NetMode,
    runner: ActionRunner<NetOp>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    open_vpn: bool,
}

impl NetScreen {
    pub fn new() -> Self {
        let (radio_on, aps, active_ip) = scan_wifi(false);
        let portal = radio_on && super::portal::needs_signin();
        let mut screen = Self {
            radio_on,
            aps,
            active_ip,
            portal,
            strip: ActionStrip::default(),
            list: ListState::new(),
            mode: NetMode::List,
            runner: ActionRunner::new(),
            header: Header::new("Hayato").subtitle("Wi-Fi"),
            keybar: Vec::new(),
            rows_height: Cell::new(16),
            open_vpn: false,
        };
        screen.rebuild_rows(false);
        if radio_on {
            screen.start_rescan();
        }
        screen.sync_chrome();
        screen
    }

    fn start_rescan(&mut self) {
        self.runner
            .spawn("Scanning Wi-Fi networks\u{2026}", move || {
                let (radio_on, aps, ip) = scan_wifi(true);
                let portal = radio_on && super::portal::needs_signin();
                NetOp::Scanned {
                    radio_on,
                    aps,
                    ip,
                    portal,
                }
            });
        self.sync_chrome();
    }

    fn start_toggle_radio(&mut self) {
        let target_on = !self.radio_on;
        let label = if target_on {
            "Turning Wi-Fi on\u{2026}"
        } else {
            "Turning Wi-Fi off\u{2026}"
        };
        self.runner.spawn(label, move || {
            let arg = if target_on { "on" } else { "off" };
            let _ = Command::new("nmcli").args(["radio", "wifi", arg]).status();
            if target_on {
                std::thread::sleep(Duration::from_millis(500));
                let (radio_on, aps, ip) = scan_wifi(true);
                let portal = radio_on && super::portal::needs_signin();
                NetOp::RadioToggled {
                    radio_on,
                    aps,
                    ip,
                    portal,
                }
            } else {
                NetOp::RadioToggled {
                    radio_on: false,
                    aps: Vec::new(),
                    ip: None,
                    portal: false,
                }
            }
        });
        self.sync_chrome();
    }

    fn start_connect(&mut self, ssid: String, password: Option<String>) {
        let label = format!("Connecting to {ssid}\u{2026}");
        self.runner.spawn(label, move || {
            let result = connect_to(&ssid, password.as_deref()).map(|_| ());
            let (radio_on, aps, ip) = scan_wifi(false);
            let portal = radio_on && super::portal::needs_signin();
            NetOp::Connected {
                ssid,
                result,
                radio_on,
                aps,
                ip,
                portal,
            }
        });
        self.sync_chrome();
    }

    fn rebuild_rows(&mut self, preserve: bool) {
        let mut actions = Vec::new();
        if self.radio_on {
            actions.push(ActionItem::new("rescan", "^R", "Rescan").glyph(Glyph::Sync));
            actions.push(ActionItem::new("toggle", "^T", "Turn off").glyph(Glyph::Power));
            if self.portal {
                actions.push(ActionItem::new("portal", "^P", "Sign in").glyph(Glyph::Globe));
            }
            if self.aps.iter().any(|a| a.in_use) {
                actions.push(ActionItem::new("qr", "^Q", "Share QR").glyph(Glyph::Qr));
            }
            actions.push(ActionItem::new("vpn", "^V", "VPN").glyph(Glyph::Shield));
        } else {
            actions.push(ActionItem::new("toggle", "^T", "Turn Wi-Fi on").glyph(Glyph::Wifi));
            actions.push(ActionItem::new("vpn", "^V", "VPN").glyph(Glyph::Shield));
        }
        self.strip.set_items(actions);

        let mut rows = Vec::new();
        if self.radio_on {
            if self.portal {
                rows.push(
                    Row::new("Sign in to this network")
                        .glyph(Glyph::Globe)
                        .detail("the network asks for a sign-in page")
                        .value("Enter opens"),
                );
            }
            for ap in &self.aps {
                let sec = if ap.security.is_empty() || ap.security == "--" {
                    "open".to_string()
                } else {
                    ap.security.clone()
                };
                let detail = if ap.in_use {
                    format!(
                        "{sec} \u{00b7} connected  ({} {:>3}%)",
                        signal_bars(ap.signal),
                        ap.signal
                    )
                } else {
                    format!("{sec}  ({} {:>3}%)", signal_bars(ap.signal), ap.signal)
                };
                let glyph = if ap.in_use { Glyph::Check } else { Glyph::Wifi };
                rows.push(
                    Row::new(ap.ssid.clone())
                        .glyph(glyph)
                        .detail(detail)
                        .value(ap.ssid.clone()),
                );
            }
        }

        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
    }

    fn sync_chrome(&mut self) {
        match &self.mode {
            NetMode::List => {
                let status = if self.runner.is_running() {
                    format!(
                        "{} {}",
                        self.runner.spinner(),
                        self.runner.label().unwrap_or("Working\u{2026}")
                    )
                } else if let Some(notice) = self.runner.notice() {
                    notice.text.clone()
                } else if self.portal {
                    "Sign in required \u{00b7} Enter opens the network's page".to_string()
                } else if !self.radio_on {
                    "Wi-Fi radio is off \u{00b7} Enter or Ctrl+T turns it on".to_string()
                } else {
                    format!(
                        "{} networks available \u{00b7} Ctrl+R rescans",
                        self.aps.len()
                    )
                };
                self.header = Header::new("Hayato").subtitle("Wi-Fi").status(status);
                self.keybar = vec![
                    KeyHint::new(
                        "Enter",
                        if self.portal {
                            "sign in"
                        } else if self.radio_on {
                            "connect"
                        } else {
                            "turn on"
                        },
                    ),
                    KeyHint::new("Tab", "actions"),
                    KeyHint::new("Ctrl+R", "rescan"),
                    KeyHint::new("Ctrl+T", "radio"),
                    KeyHint::new("Esc", "back"),
                ];
            }
            NetMode::Password { ssid, .. } => {
                self.header = Header::new("Hayato")
                    .subtitle("Wi-Fi")
                    .status("Enter connects \u{00b7} Escape returns to networks");
                self.keybar = vec![
                    KeyHint::new("Enter", "connect"),
                    KeyHint::new("Esc", "back"),
                ];
            }
            NetMode::WifiQr { title, .. } => {
                let status = self
                    .runner
                    .notice()
                    .map(|n| n.text.clone())
                    .unwrap_or_else(|| {
                        "s saves PNG \u{00b7} c copies payload \u{00b7} o opens folder".to_string()
                    });
                self.header = Header::new("Hayato").subtitle("Wi-Fi").status(status);
                self.keybar = vec![
                    KeyHint::new("s", "save PNG"),
                    KeyHint::new("c", "copy payload"),
                    KeyHint::new("o", "open"),
                    KeyHint::new("Esc", "back to networks"),
                ];
            }
            NetMode::ConfirmForget { .. } => {
                self.header = Header::new("Hayato")
                    .subtitle("Wi-Fi")
                    .status("Cancel is selected");
                self.keybar = vec![
                    KeyHint::new("\u{2190}/\u{2192}", "choose"),
                    KeyHint::new("Enter", "confirm"),
                    KeyHint::new("Esc", "cancel"),
                ];
            }
        }
    }

    fn prompt_forget_selected(&mut self) {
        if self.runner.is_running() || !self.radio_on {
            return;
        }
        let Some(row) = self.list.selected_index() else {
            return;
        };
        let Some(index) = ap_index(self.portal, row) else {
            return;
        };
        let Some(ap) = self.aps.get(index).cloned() else {
            return;
        };
        if !has_saved_connection(&ap.ssid) {
            self.runner
                .set_transient(format!("{} has no saved profile", ap.ssid));
            self.sync_chrome();
            return;
        }
        let confirm = Confirm::new(
            format!("Forget {}?", ap.ssid),
            format!(
                "Delete the saved Wi-Fi profile and passphrase for {}?",
                ap.ssid
            ),
        )
        .with_detail("You will need to enter its password again to reconnect.")
        .with_options(&["Cancel", "Forget"])
        .destructive(true);
        self.mode = NetMode::ConfirmForget {
            ssid: ap.ssid,
            confirm,
        };
        self.sync_chrome();
    }

    fn forget_network(&mut self, ssid: String) {
        let label = format!("Forgetting {ssid}\u{2026}");
        self.runner.spawn(label, move || {
            let status = Command::new("nmcli")
                .args(["connection", "delete", "id", &ssid])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output();
            let result = match status {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
                Err(e) => Err(e.to_string()),
            };
            let (radio_on, aps, ip) = scan_wifi(false);
            let portal = radio_on && super::portal::needs_signin();
            NetOp::Connected {
                ssid: format!("Forgot {ssid}"),
                result,
                radio_on,
                aps,
                ip,
                portal,
            }
        });
        self.sync_chrome();
    }

    fn open_inline_qr(&mut self) {
        let Some((payload, title)) = super::qr::payload_wifi() else {
            self.runner
                .set_persistent_error("Not connected to a Wi-Fi network");
            self.sync_chrome();
            return;
        };
        if let Some(qr) = QrCode::encode(&payload) {
            self.runner.clear_status();
            self.mode = NetMode::WifiQr {
                qr,
                title,
                subtitle: "Scan with camera to join Wi-Fi".to_string(),
                payload,
            };
            self.sync_chrome();
        } else {
            self.runner
                .set_persistent_error("Could not encode QR: payload too large");
            self.sync_chrome();
        }
    }

    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        match id {
            "toggle" => {
                self.start_toggle_radio();
                Outcome::Consumed
            }
            "rescan" => {
                if self.radio_on {
                    self.start_rescan();
                }
                Outcome::Consumed
            }
            "portal" => {
                if self.portal {
                    self.open_portal();
                }
                Outcome::Consumed
            }
            "qr" => {
                self.open_inline_qr();
                Outcome::Consumed
            }
            "vpn" => {
                self.open_vpn = true;
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            _ => Outcome::Consumed,
        }
    }

    fn open_portal(&mut self) {
        match super::portal::open() {
            Ok(url) => self.runner.set_transient(format!("Opened {url}")),
            Err(reason) => self.runner.set_persistent_error(reason),
        }
        self.sync_chrome();
    }

    fn activate_ap(&mut self, row: usize) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        if self.portal && row == 0 {
            self.open_portal();
            return Outcome::Consumed;
        }
        let Some(index) = ap_index(self.portal, row) else {
            return Outcome::Continue;
        };
        let Some(ap) = self.aps.get(index).cloned() else {
            return Outcome::Continue;
        };
        let is_open = ap.security.is_empty() || ap.security == "--";
        if is_open || has_saved_connection(&ap.ssid) {
            self.start_connect(ap.ssid, None);
        } else {
            self.mode = NetMode::Password {
                ssid: ap.ssid,
                input: Input::new().with_masked(true),
            };
            self.sync_chrome();
        }
        Outcome::Consumed
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
        if area.height == 0 || area.width < 16 {
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
                    "\u{2714} DONE",
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
        } else if !self.radio_on {
            let x = draw_badge(buf, area.x, y, design, "RADIO OFF", ColorRole::Warn);
            let _ = put(
                buf,
                x,
                y,
                "  Wireless radio is disabled \u{00b7} press Enter or Ctrl+T to turn on",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        } else if let Some(active) = self.aps.iter().find(|a| a.in_use) {
            let mut x = draw_badge(buf, area.x, y, design, "\u{25cf} CONNECTED", ColorRole::Ok);
            x = put(
                buf,
                x,
                y,
                "  ",
                area.right().saturating_sub(x),
                design.style(TextStyle::Body),
            );
            x = put_trunc(
                buf,
                x,
                y,
                &active.ssid,
                area.right().saturating_sub(x + 24),
                design.style(TextStyle::Title),
            );
            let sec = if active.security.is_empty() || active.security == "--" {
                "open"
            } else {
                &active.security
            };
            let meta = match &self.active_ip {
                Some(ip) => format!("  \u{00b7}  {sec}  \u{00b7}  {ip}"),
                None => format!("  \u{00b7}  {sec}"),
            };
            put_trunc(
                buf,
                x,
                y,
                &meta,
                area.right().saturating_sub(x + 10),
                design.style(TextStyle::Dim),
            );
            if area.width >= 30 {
                let meter_x = area.right().saturating_sub(9);
                let after = draw_signal_bars(buf, meter_x, y, design, active.signal);
                let pct = format!(" {:>3}%", active.signal);
                put(
                    buf,
                    after,
                    y,
                    &pct,
                    area.right().saturating_sub(after),
                    design.style(TextStyle::Code),
                );
            }
        } else {
            let x = draw_badge(buf, area.x, y, design, "DISCONNECTED", ColorRole::Warn);
            let _ = put_trunc(
                buf,
                x,
                y,
                "  Select a network below to connect",
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
}

impl Screen for NetScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        match &self.mode {
            NetMode::WifiQr { qr, .. } => Some(qr.recommended_window_size()),
            NetMode::Password { .. } => Some((680, 320)),
            NetMode::List | NetMode::ConfirmForget { .. } => Some((860, 560)),
        }
    }

    fn needs_tick(&self) -> Option<Duration> {
        self.runner.tick_rate()
    }

    fn tick(&mut self) -> Outcome {
        if let Some(op) = self.runner.poll() {
            match op {
                NetOp::Scanned {
                    radio_on,
                    aps,
                    ip,
                    portal,
                } => {
                    let portal_changed = self.portal != portal;
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.portal = portal;
                    self.rebuild_rows(!portal_changed);
                    // Routine discovery clears the status banner immediately on completion.
                    self.runner.clear_status();
                }
                NetOp::Connected {
                    ssid,
                    result,
                    radio_on,
                    aps,
                    ip,
                    portal,
                } => {
                    let portal_changed = self.portal != portal;
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.portal = portal;
                    self.rebuild_rows(!portal_changed);
                    match result {
                        Ok(()) => {
                            self.runner.set_transient(format!("Connected to {ssid}"));
                        }
                        Err(err) => {
                            // Connection failure overrides auto-hide so the user sees why it failed.
                            self.runner.set_persistent_error(format!(
                                "Could not connect to {ssid}: {err}"
                            ));
                        }
                    }
                }
                NetOp::RadioToggled {
                    radio_on,
                    aps,
                    ip,
                    portal,
                } => {
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.portal = portal;
                    self.rebuild_rows(false);
                    let msg = if radio_on {
                        "Wi-Fi turned on"
                    } else {
                        "Wi-Fi turned off"
                    };
                    self.runner.set_transient(msg);
                }
            }
            self.sync_chrome();
            return Outcome::Continue;
        }
        self.sync_chrome();
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        match &self.mode {
            NetMode::WifiQr {
                qr,
                title,
                subtitle,
                ..
            } => {
                draw_card(ctx.buf, ctx.area, ctx.design, Some(title), true);
                if ctx.area.height >= 4 && !subtitle.is_empty() {
                    put_trunc(
                        ctx.buf,
                        ctx.area.x + 2,
                        ctx.area.y + 1,
                        subtitle,
                        ctx.area.width.saturating_sub(4),
                        ctx.design.style(TextStyle::Dim),
                    );
                }
                let inner = Rect {
                    x: ctx.area.x + 2,
                    y: ctx.area.y + 2,
                    width: ctx.area.width.saturating_sub(4),
                    height: ctx.area.height.saturating_sub(3),
                };
                super::draw_qr_fit(
                    ctx.buf,
                    inner,
                    ctx.design,
                    qr,
                    "Window too small for Wi-Fi QR · Esc back",
                );
            }
            NetMode::ConfirmForget { confirm, .. } => {
                confirm.render(ctx.area, ctx.buf, ctx.design);
            }
            NetMode::Password { ssid, input } => {
                let card_title = format!("Password for {ssid}");
                draw_card(ctx.buf, ctx.area, ctx.design, Some(&card_title), true);
                let (filter, rows) = super::pick::list_layout(ctx.area);
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    "Enter WPA/WPA2/WPA3 passphrase \u{00b7} Escape returns to network list",
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
                );
                let input_rect = Rect {
                    x: rows.x,
                    y: rows.y,
                    width: rows.width,
                    height: 1.min(rows.height),
                };
                input.render(input_rect, ctx.buf, ctx.design, "WPA passphrase\u{2026}");
            }
            NetMode::List => {
                draw_card(ctx.buf, ctx.area, ctx.design, Some("Networks"), true);
                let (hero, strip, filter, rows) = Self::layout(ctx.area);
                self.draw_hero(ctx.buf, hero, ctx.design);
                self.strip.render(strip, ctx.buf, ctx.design);
                let hint = if !self.radio_on {
                    "Wi-Fi radio is off \u{00b7} press Enter or Ctrl+T to enable"
                } else if self.portal {
                    "Sign in to this network \u{00b7} Enter opens the page \u{00b7} Tab focuses actions"
                } else {
                    "type to filter networks \u{00b7} Enter connects \u{00b7} Tab focuses actions"
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
                if !self.radio_on {
                    draw_empty(
                        ctx.buf,
                        rows,
                        ctx.design,
                        "Wi-Fi radio is off",
                        "Press Enter or Ctrl+T to turn Wi-Fi on and scan for networks",
                    );
                } else if self.aps.is_empty() && !self.portal {
                    draw_empty(
                        ctx.buf,
                        rows,
                        ctx.design,
                        if self.runner.is_running() {
                            "Scanning Wi-Fi channels\u{2026}"
                        } else {
                            "No Wi-Fi networks found"
                        },
                        "Press Ctrl+R to rescan nearby channels",
                    );
                } else {
                    self.list
                        .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
                }
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match &mut self.mode {
            NetMode::WifiQr { payload, .. } => match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    match super::qr::save_qr_png(payload, "wifi-qr") {
                        Ok(path) => {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let day = path
                                .parent()
                                .and_then(|p| p.file_name())
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            self.runner
                                .set_persistent_info(format!("Saved Screenshots/{day}/{name}"));
                        }
                        Err(err) => {
                            self.runner.set_persistent_error(err);
                        }
                    }
                    self.sync_chrome();
                    Outcome::Consumed
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    let target = super::qr::save_qr_png(payload, "wifi-qr")
                        .unwrap_or_else(|_| super::qr::qr_target_dir());
                    crate::system::spawn_detached(&format!("exec xdg-open '{}'", target.display()));
                    self.runner
                        .set_transient(format!("Opened {}", target.display()));
                    self.sync_chrome();
                    Outcome::Consumed
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    if let Ok(mut child) = Command::new("wl-copy")
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        if let Some(mut stdin) = child.stdin.take() {
                            let _ = stdin.write_all(payload.as_bytes());
                        }
                        let _ = child.wait();
                        self.runner
                            .set_transient("Copied Wi-Fi QR payload to clipboard");
                    }
                    self.sync_chrome();
                    Outcome::Consumed
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.mode = NetMode::List;
                    self.runner.clear_status();
                    self.sync_chrome();
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
            NetMode::ConfirmForget { ssid, confirm } => {
                let target = ssid.clone();
                match confirm.handle(key) {
                    ConfirmAction::Chosen(1) => {
                        self.mode = NetMode::List;
                        self.forget_network(target);
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = NetMode::List;
                        self.sync_chrome();
                    }
                    ConfirmAction::None => {}
                }
                Outcome::Consumed
            }
            NetMode::Password { ssid, input } => match input.handle(key) {
                InputAction::Submitted => {
                    let pw = input.text().to_string();
                    let target = ssid.clone();
                    self.mode = NetMode::List;
                    if !pw.is_empty() {
                        self.start_connect(target, Some(pw));
                    } else {
                        self.sync_chrome();
                    }
                    Outcome::Consumed
                }
                InputAction::Cancelled => {
                    self.mode = NetMode::List;
                    self.sync_chrome();
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
            NetMode::List => {
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
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            return self.run_strip_action("rescan");
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            return self.run_strip_action("toggle");
                        }
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            return self.run_strip_action("qr");
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            return self.run_strip_action("portal");
                        }
                        KeyCode::Char('v') | KeyCode::Char('V') => {
                            return self.run_strip_action("vpn");
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            self.prompt_forget_selected();
                            return Outcome::Consumed;
                        }
                        _ => {}
                    }
                }
                if key.code == KeyCode::Delete {
                    self.prompt_forget_selected();
                    return Outcome::Consumed;
                }
                if !self.radio_on && key.code == KeyCode::Enter {
                    return self.run_strip_action("toggle");
                }
                let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT);
                if typed {
                    self.list.type_filter(key);
                    return Outcome::Consumed;
                }
                match self.list.handle(key, self.rows_height.get(), design) {
                    ListAction::Activated(index) => self.activate_ap(index),
                    _ => Outcome::Continue,
                }
            }
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let NetMode::ConfirmForget { ssid, confirm } = &mut self.mode {
            if let Some(idx) = row {
                let target = ssid.clone();
                match confirm.click(idx) {
                    ConfirmAction::Chosen(1) => {
                        self.mode = NetMode::List;
                        self.forget_network(target);
                    }
                    ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                        self.mode = NetMode::List;
                        self.sync_chrome();
                    }
                    ConfirmAction::None => {}
                }
            }
            return Outcome::Consumed;
        }
        if !matches!(self.mode, NetMode::List) {
            return Outcome::Continue;
        }
        let Some(row) = row else {
            return Outcome::Continue;
        };
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
            return self.activate_ap(index);
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if let NetMode::ConfirmForget { confirm, .. } = &self.mode {
            return confirm.hit(area, design, x, y);
        }
        if !matches!(self.mode, NetMode::List) {
            return None;
        }
        let (_, strip, _, rows) = Self::layout(area);
        if y == strip.y && x >= strip.x && x < strip.right() {
            let mut col = strip.x;
            for (idx, item) in self.strip.items().iter().enumerate() {
                let w = 4
                    + item.key.len() as u16
                    + item.label.len() as u16
                    + item.glyph.map(|_| 2).unwrap_or(0);
                if x >= col && x < col + w {
                    return Some(10_000 + idx);
                }
                col += w + 1;
            }
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
            NetMode::List => "list",
            NetMode::Password { .. } => "password",
            NetMode::WifiQr { .. } => "wifi-qr",
            NetMode::ConfirmForget { .. } => "confirm-forget",
        })
    }
}

/// `hayato menu net` interactive surface.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if !have("nmcli") {
        return Err(crate::Failure::new(
            "Wi-Fi",
            "nmcli is not installed (package network-manager)",
        ));
    }
    loop {
        let mut screen = NetScreen::new();
        let code = crate::run_screen(terminal, design, "Wi-Fi", &mut screen)?;
        if screen.open_vpn {
            let _ = super::vpn::menu(terminal, design)?;
            continue;
        }
        return Ok(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nmcli_wifi_parses_records_and_collapses_duplicates() {
        let raw = "\
*:MyHome:WPA2:80
 :MyHome:WPA2:65
 :CoffeeShop:--:50
 :Mesh\\:Network:WPA3:90
";
        let aps = parse_nmcli_wifi(raw);
        assert_eq!(aps.len(), 3);
        assert_eq!(aps[0].ssid, "MyHome");
        assert!(aps[0].in_use);
        assert_eq!(aps[0].signal, 80);

        assert_eq!(aps[1].ssid, "Mesh:Network");
        assert_eq!(aps[1].signal, 90);
        assert_eq!(aps[1].security, "WPA3");

        assert_eq!(aps[2].ssid, "CoffeeShop");
        assert_eq!(aps[2].security, "--");
    }

    #[test]
    fn the_portal_row_does_not_shift_a_network_onto_it() {
        assert_eq!(ap_index(false, 0), Some(0));
        assert_eq!(ap_index(false, 2), Some(2));
        assert_eq!(ap_index(true, 0), None);
        assert_eq!(ap_index(true, 1), Some(0));
        assert_eq!(ap_index(true, 3), Some(2));
    }

    #[test]
    fn signal_bar_gauge_steps() {
        assert_eq!(signal_bar(10), " ");
        assert_eq!(signal_bar(30), "\u{2582}");
        assert_eq!(signal_bar(50), "\u{2583}");
        assert_eq!(signal_bar(70), "\u{2584}");
        assert_eq!(signal_bar(90), "\u{2585}");
    }

    #[test]
    fn status_json_disabled_format() {
        let disabled = format_status_json(false, None);
        assert!(disabled.contains("\"class\":\"disabled\""));
        assert!(disabled.contains("NET // RADIO OFF"));

        let disconnected = format_status_json(true, None);
        assert!(disconnected.contains("\"class\":\"disconnected\""));
        assert!(disconnected.contains("NET // OFFLINE"));

        let ap = WifiAp {
            in_use: true,
            ssid: "Lab & Ops".into(),
            security: "WPA2".into(),
            signal: 82,
        };
        let connected = format_status_json(true, Some(&ap));
        assert!(connected.contains("\"class\":\"connected\""));
        assert!(connected.contains("Lab &amp; Ops"));
        assert!(connected.contains("▂▄▆█  82%"));
        assert!(connected.contains("WPA2"));
    }
}
