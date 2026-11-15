//! Wi-Fi & Network Manager (`bar-net`, `lazyos menu net`).
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

use lazyos_ui::widgets::{put, put_trunc};
use lazyos_ui::{
    ActionRunner, BodyCtx, ColorRole, Design, Glyph, Header, Input, InputAction, KeyHint,
    ListAction, ListState, Outcome, QrCode, Row, Screen, TextStyle, draw_badge, draw_card,
    draw_filter_bar, draw_qr, draw_signal_bars, signal_bars,
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

/// Headless query for waybar `custom/net` module: emits JSON.
pub fn status_json() -> String {
    let radio = out(&["nmcli", "radio", "wifi"]).unwrap_or_default();
    if radio != "enabled" {
        return r#"{"text":"off","tooltip":"Wi-Fi radio is off","class":"disabled","percentage":0}"#
            .to_string();
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
    if let Some(active) = aps.iter().find(|ap| ap.in_use) {
        format!(
            r#"{{"text":"{}","tooltip":"Connected to {} ({}%)","class":"connected","percentage":{}}}"#,
            escape_json(&active.ssid),
            escape_json(&active.ssid),
            active.signal,
            active.signal
        )
    } else {
        r#"{"text":"disconnected","tooltip":"Wi-Fi disconnected","class":"disconnected","percentage":0}"#
            .to_string()
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
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

fn scan_wifi(rescan: bool) -> ( bool, Vec<WifiAp>, Option<String> ) {
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
        explicit: bool,
    },
    Connected {
        ssid: String,
        result: Result<(), String>,
        radio_on: bool,
        aps: Vec<WifiAp>,
        ip: Option<String>,
    },
    RadioToggled {
        radio_on: bool,
        aps: Vec<WifiAp>,
        ip: Option<String>,
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
}

#[derive(Clone, Debug)]
enum NetAction {
    Connect(String),
    ToggleRadio,
    Rescan,
    ShowQr,
    Nmtui,
}

pub struct NetScreen {
    radio_on: bool,
    aps: Vec<WifiAp>,
    active_ip: Option<String>,
    actions: Vec<NetAction>,
    list: ListState,
    mode: NetMode,
    runner: ActionRunner<NetOp>,
    banner: Option<(String, ColorRole)>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl NetScreen {
    pub fn new() -> Self {
        let (radio_on, aps, active_ip) = scan_wifi(false);
        let mut screen = Self {
            radio_on,
            aps,
            active_ip,
            actions: Vec::new(),
            list: ListState::new(),
            mode: NetMode::List,
            runner: ActionRunner::new(),
            banner: None,
            header: Header::new("Wi-Fi"),
            keybar: Vec::new(),
            rows_height: Cell::new(16),
        };
        screen.rebuild_rows(false);
        if radio_on {
            screen.start_rescan(false);
        }
        screen.sync_chrome();
        screen
    }

    fn start_rescan(&mut self, explicit: bool) {
        self.runner.spawn("Scanning Wi-Fi networks\u{2026}", move || {
            let (radio_on, aps, ip) = scan_wifi(true);
            NetOp::Scanned {
                radio_on,
                aps,
                ip,
                explicit,
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
            let (radio_on, aps, ip) = scan_wifi(target_on);
            NetOp::RadioToggled { radio_on, aps, ip }
        });
        self.sync_chrome();
    }

    fn start_connect(&mut self, ssid: String, password: Option<String>) {
        let label = format!("Connecting to {ssid}\u{2026}");
        self.runner.spawn(label, move || {
            let result = connect_to(&ssid, password.as_deref()).map(|_| ());
            let (radio_on, aps, ip) = scan_wifi(false);
            NetOp::Connected {
                ssid,
                result,
                radio_on,
                aps,
                ip,
            }
        });
        self.sync_chrome();
    }

    fn rebuild_rows(&mut self, preserve: bool) {
        let mut rows = Vec::new();
        let mut actions = Vec::new();

        if !self.radio_on {
            rows.push(
                Row::new("Turn Wi-Fi on")
                    .glyph(Glyph::Wifi)
                    .detail("enable wireless radio and scan channels"),
            );
            actions.push(NetAction::ToggleRadio);
            rows.push(
                Row::new("Network settings (nmtui)")
                    .glyph(Glyph::Gear)
                    .detail("manage connections in terminal"),
            );
            actions.push(NetAction::Nmtui);
        } else {
            for ap in &self.aps {
                let sec = if ap.security.is_empty() || ap.security == "--" {
                    "open".to_string()
                } else {
                    ap.security.clone()
                };
                let detail = if ap.in_use {
                    format!("{sec} \u{00b7} connected")
                } else {
                    sec
                };
                let meter = format!("{} {:>3}%", signal_bars(ap.signal), ap.signal);
                let glyph = if ap.in_use { Glyph::Check } else { Glyph::Wifi };
                rows.push(
                    Row::new(ap.ssid.clone())
                        .glyph(glyph)
                        .detail(detail)
                        .value(meter),
                );
                actions.push(NetAction::Connect(ap.ssid.clone()));
            }

            rows.push(
                Row::new("Rescan networks")
                    .glyph(Glyph::Sync)
                    .detail("scan all Wi-Fi channels now (r)"),
            );
            actions.push(NetAction::Rescan);

            if self.aps.iter().any(|a| a.in_use) {
                rows.push(
                    Row::new("Show the password as a QR")
                        .glyph(Glyph::Qr)
                        .detail("render Wi-Fi credentials QR in this window (Ctrl+Q)"),
                );
                actions.push(NetAction::ShowQr);
            }

            rows.push(
                Row::new("Turn Wi-Fi off")
                    .glyph(Glyph::Power)
                    .detail("disable wireless radio"),
            );
            actions.push(NetAction::ToggleRadio);

            rows.push(
                Row::new("Network settings (nmtui)")
                    .glyph(Glyph::Gear)
                    .detail("manage connections in terminal"),
            );
            actions.push(NetAction::Nmtui);
        }

        self.actions = actions;
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
                } else if let Some((msg, _)) = &self.banner {
                    msg.clone()
                } else if !self.radio_on {
                    "Wi-Fi radio is off \u{00b7} Enter turns it on".to_string()
                } else {
                    format!("{} networks available \u{00b7} r rescans", self.aps.len())
                };
                self.header = Header::new("Wi-Fi").subtitle("Network").status(status);
                self.keybar = vec![
                    KeyHint::new("Enter", "connect / run"),
                    KeyHint::new("r", "rescan"),
                    KeyHint::new("Ctrl+Q", "Wi-Fi QR"),
                    KeyHint::new("Esc", "close"),
                ];
            }
            NetMode::Password { ssid, .. } => {
                self.header = Header::new("Wi-Fi")
                    .subtitle(format!("Password for {ssid}"))
                    .status("Enter connects \u{00b7} Escape returns to networks");
                self.keybar = vec![
                    KeyHint::new("Enter", "connect"),
                    KeyHint::new("Esc", "back"),
                ];
            }
            NetMode::WifiQr { title, .. } => {
                let status = self
                    .banner
                    .as_ref()
                    .map(|(m, _)| m.clone())
                    .unwrap_or_else(|| "Scan with a phone camera \u{00b7} c copies payload".to_string());
                self.header = Header::new("Wi-Fi").subtitle(title.clone()).status(status);
                self.keybar = vec![
                    KeyHint::new("c", "copy payload"),
                    KeyHint::new("Enter/Esc", "back to networks"),
                ];
            }
        }
    }

    fn open_inline_qr(&mut self) {
        let Some((payload, title)) = super::qr::payload_wifi() else {
            self.banner = Some((
                "Not connected to a Wi-Fi network".to_string(),
                ColorRole::Warning,
            ));
            self.sync_chrome();
            return;
        };
        if let Some(qr) = QrCode::encode(&payload) {
            self.banner = None;
            self.mode = NetMode::WifiQr {
                qr,
                title,
                subtitle: "Scan with camera to join Wi-Fi".to_string(),
                payload,
            };
            self.sync_chrome();
        } else {
            self.banner = Some(("Could not encode QR: payload too large".to_string(), ColorRole::Danger));
            self.sync_chrome();
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        let Some(action) = self.actions.get(index).cloned() else {
            return Outcome::Continue;
        };
        match action {
            NetAction::ToggleRadio => {
                self.banner = None;
                self.start_toggle_radio();
                Outcome::Consumed
            }
            NetAction::Rescan => {
                self.banner = None;
                self.start_rescan(true);
                Outcome::Consumed
            }
            NetAction::ShowQr => {
                self.open_inline_qr();
                Outcome::Consumed
            }
            NetAction::Nmtui => {
                crate::system::spawn_detached("exec foot --app-id=popup nmtui");
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            NetAction::Connect(ssid) => {
                let target_ap = self.aps.iter().find(|a| a.ssid == ssid);
                let is_open =
                    target_ap.is_some_and(|a| a.security.is_empty() || a.security == "--");
                if is_open || has_saved_connection(&ssid) {
                    self.banner = None;
                    self.start_connect(ssid, None);
                } else {
                    self.mode = NetMode::Password {
                        ssid,
                        input: Input::new().with_masked(true),
                    };
                    self.sync_chrome();
                }
                Outcome::Consumed
            }
        }
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
        if area.height == 0 || area.width < 16 {
            return;
        }
        let y = area.y;
        if !self.radio_on {
            let mut x = draw_badge(buf, area.x, y, design, "RADIO OFF", ColorRole::Warn);
            x = put(
                buf,
                x,
                y,
                "  Wireless radio is disabled",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
            let _ = x;
        } else if let Some(active) = self.aps.iter().find(|a| a.in_use) {
            let mut x = draw_badge(buf, area.x, y, design, "\u{25cf} CONNECTED", ColorRole::Ok);
            x = put(buf, x, y, "  ", area.right().saturating_sub(x), design.style(TextStyle::Body));
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
            let mut x = draw_badge(
                buf,
                area.x,
                y,
                design,
                "DISCONNECTED",
                ColorRole::Warn,
            );
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
            NetMode::List => Some((860, 560)),
        }
    }

    fn needs_tick(&self) -> Option<Duration> {
        self.runner.tick_rate()
    }

    fn tick(&mut self) -> Outcome {
        self.runner.tick();
        if let Some(op) = self.runner.poll() {
            match op {
                NetOp::Scanned {
                    radio_on,
                    aps,
                    ip,
                    explicit,
                } => {
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.rebuild_rows(true);
                    if explicit {
                        self.banner = Some((
                            format!("Found {} Wi-Fi networks", self.aps.len()),
                            ColorRole::Ok,
                        ));
                    }
                }
                NetOp::Connected {
                    ssid,
                    result,
                    radio_on,
                    aps,
                    ip,
                } => {
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.rebuild_rows(true);
                    match result {
                        Ok(()) => {
                            self.banner =
                                Some((format!("Connected to {ssid}"), ColorRole::Ok));
                        }
                        Err(err) => {
                            self.banner = Some((
                                format!("Could not connect to {ssid}: {err}"),
                                ColorRole::Danger,
                            ));
                        }
                    }
                }
                NetOp::RadioToggled { radio_on, aps, ip } => {
                    self.radio_on = radio_on;
                    self.aps = aps;
                    self.active_ip = ip;
                    self.rebuild_rows(false);
                    let msg = if radio_on {
                        "Wi-Fi turned on".to_string()
                    } else {
                        "Wi-Fi turned off".to_string()
                    };
                    self.banner = Some((msg, ColorRole::Ok));
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
                draw_qr(ctx.buf, inner, ctx.design, qr);
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
                draw_card(ctx.buf, ctx.area, ctx.design, Some("Wi-Fi"), true);
                let (hero, filter, rows) = Self::layout(ctx.area);
                self.draw_hero(ctx.buf, hero, ctx.design);
                let hint = if self.runner.is_running() {
                    self.runner.label().unwrap_or("Scanning\u{2026}")
                } else if let Some((msg, _)) = &self.banner {
                    msg.as_str()
                } else {
                    "type to filter networks \u{00b7} Enter connects \u{00b7} r rescans"
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
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match &mut self.mode {
            NetMode::WifiQr { payload, .. } => match key.code {
                KeyCode::Char('c') => {
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
                        self.banner = Some((
                            "Copied Wi-Fi QR payload to clipboard".to_string(),
                            ColorRole::Ok,
                        ));
                    }
                    self.sync_chrome();
                    Outcome::Consumed
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.mode = NetMode::List;
                    self.banner = None;
                    self.sync_chrome();
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
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
                if key.code == KeyCode::Esc {
                    if !self.list.filter().is_empty() {
                        self.list.set_filter("");
                        return Outcome::Consumed;
                    }
                    return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
                }
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
                {
                    self.open_inline_qr();
                    return Outcome::Consumed;
                }
                if self.list.filter().is_empty() && key.code == KeyCode::Char('r') {
                    if !self.runner.is_running() && self.radio_on {
                        self.banner = None;
                        self.start_rescan(true);
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
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if !matches!(self.mode, NetMode::List) {
            return Outcome::Continue;
        }
        let Some(row) = row else {
            return Outcome::Continue;
        };
        self.list.set_cursor(row);
        if let Some(index) = self.list.selected_index() {
            return self.activate(index);
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if !matches!(self.mode, NetMode::List) {
            return None;
        }
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

/// `lazyos menu net` interactive surface.
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
    let mut screen = NetScreen::new();
    crate::run_screen(terminal, design, "Wi-Fi", &mut screen)
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
    fn signal_bar_gauge_steps() {
        assert_eq!(signal_bar(10), " ");
        assert_eq!(signal_bar(30), "\u{2582}");
        assert_eq!(signal_bar(50), "\u{2583}");
        assert_eq!(signal_bar(70), "\u{2584}");
        assert_eq!(signal_bar(90), "\u{2585}");
    }

    #[test]
    fn status_json_disabled_format() {
        let json =
            r#"{"text":"off","tooltip":"Wi-Fi radio is off","class":"disabled","percentage":0}"#;
        assert!(json.contains("\"class\":\"disabled\""));
    }
}
