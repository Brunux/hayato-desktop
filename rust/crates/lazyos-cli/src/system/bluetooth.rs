//! Bluetooth picker (`bar-bt`, `lazyos menu bluetooth`): scan, connect, disconnect, bluetoothctl.
//!
//! Renders known devices immediately on open while launching a non-blocking
//! background discovery scan (`bluetoothctl --timeout 3 scan on`) that updates
//! the device list in-place. Power-on, power-off, pairing, and connection run
//! asynchronously via [`ActionRunner`] with an in-window spinner and controller
//! hero banner, never blocking the UI thread or firing external toasts.

use std::cell::Cell;
use std::process::{Command, Stdio};
use std::time::Duration;

use lazyos_ui::widgets::{put, put_trunc};
use lazyos_ui::{
    ActionItem, ActionRunner, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Design, Glyph,
    Header, KeyHint, ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_badge, draw_card,
    draw_empty, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["bluetoothctl", "foot"];

/// Waybar's `custom/bluetooth` module polls every 30 s (`interval` in
/// `config.jsonc`). A power or connection change made here has to reach the
/// bar now instead of on the next poll, so this takes the wait out of it:
/// `signal: 8` in the module is RTMIN+8, the same signal the vnc module sends.
pub fn signal_bar() {
    let _ = out(&["pkill", "-RTMIN+8", "waybar"]);
}

/// Returns true if the device name is just a formatted MAC address (e.g. "44-F9-B4-7F-6C-CA").
fn is_raw_mac(name: &str) -> bool {
    let clean = name.replace([':', '-'], "");
    clean.len() == 12 && clean.chars().all(|c| c.is_ascii_hexdigit())
}

fn do_scan(secs: u32) {
    let _ = Command::new("bluetoothctl")
        .args(["--timeout", &secs.to_string(), "scan", "on"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Pair, trust and connect, in that order.
///
/// A bare `connect` comes first in the naive flow, and it can bring up the ACL
/// link without a bond: BlueZ then refuses the HID profile
/// (`hidp_add_connection() Rejected connection from !bonded device`) and a
/// DualShock 4 sits there "Paired: yes, Bonded: no" with no input device. The
/// explicit agent matters for the same reason a non-interactive `bluetoothctl`
/// has none: a "just works" controller can pair without a key being stored.
///
/// An unbonded entry is not a pairing, and `pair` refuses it ("remove it
/// first"), so the empty entry is dropped and paired again. The bond is read
/// back from `info`: success is "Bonded: yes", not the word "successful".
///
/// A discovery runs before the request. Without one, BlueZ pairs against the
/// cached entry and can answer "Pairing successful" while storing no key at
/// all, because the pad only accepts the request in its own pairing mode and a
/// cached entry looks reachable enough to try. The scan is also what the
/// person's "hold PS + Share" produces: a pad that is not advertising is not
/// in pairing mode, and saying so is better than a pairing that did not
/// happen.
fn pair_and_connect(mac: &str, name: &str) -> (bool, String) {
    let info = out(&["bluetoothctl", "info", mac]).unwrap_or_default();
    if info.lines().any(|line| line.trim() == "Bonded: yes") {
        trust(mac);
        return if connect(mac) {
            (true, format!("Connected to {name}"))
        } else {
            (false, format!("Could not connect to {name}"))
        };
    }

    if info.contains("Paired: yes") {
        let _ = Command::new("bluetoothctl")
            .args(["remove", mac])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    if !discover(mac) {
        return (
            false,
            format!("{name} is not in pairing mode: put it in pairing mode and try again"),
        );
    }

    let pair = Command::new("bluetoothctl")
        .args(["--agent", "NoInputNoOutput", "pair", mac])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let text = pair
        .as_ref()
        .map(|output| String::from_utf8_lossy(&output.stdout))
        .unwrap_or_default();
    if !text.contains("Pairing successful") {
        return (
            false,
            format!("Could not pair with {name}: put it in pairing mode first"),
        );
    }

    trust(mac);
    let _ = connect(mac);
    let info = out(&["bluetoothctl", "info", mac]).unwrap_or_default();
    if info.lines().any(|line| line.trim() == "Bonded: yes") {
        (true, format!("Paired and connected to {name}"))
    } else {
        (
            false,
            format!("{name} paired but not bonded: put it in pairing mode and try again"),
        )
    }
}

/// Whether the address is in BlueZ's device list.
fn listed(mac: &str) -> bool {
    out(&["bluetoothctl", "devices"]).is_some_and(|list| list.contains(mac))
}

/// Discover for a few seconds, and say whether the address showed up.
///
/// A scan already running (the picker starts one when it opens) makes `scan on`
/// return at once with "In Progress", which is not a failure: the list is
/// checked again after a short wait either way.
fn discover(mac: &str) -> bool {
    for _ in 0..3 {
        if listed(mac) {
            return true;
        }
        let _ = Command::new("bluetoothctl")
            .args(["--timeout", "4", "scan", "on"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_secs(1));
        if listed(mac) {
            return true;
        }
    }
    false
}

fn trust(mac: &str) {
    let _ = Command::new("bluetoothctl")
        .args(["trust", mac])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn connect(mac: &str) -> bool {
    Command::new("bluetoothctl")
        .args(["connect", mac])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("Connection successful"))
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
struct Device {
    mac: String,
    name: String,
    connected: bool,
}

enum BtOp {
    Scanned {
        powered: bool,
        devices: Vec<Device>,
    },
    PoweredOn {
        powered: bool,
        devices: Vec<Device>,
    },
    PoweredOff,
    DeviceResult {
        message: String,
        ok: bool,
        powered: bool,
        devices: Vec<Device>,
    },
}

/// The bluetooth screen, sharing the same visual structure and action-strip
/// separation as `NetScreen` (`menu net`).
pub struct BluetoothScreen {
    devices: Vec<Device>,
    powered: bool,
    strip: ActionStrip,
    runner: ActionRunner<BtOp>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl Default for BluetoothScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl BluetoothScreen {
    pub fn new() -> Self {
        let powered = Self::get_powered();
        let devices = if powered {
            Self::get_devices()
        } else {
            Vec::new()
        };
        let mut screen = Self {
            devices,
            powered,
            strip: ActionStrip::default(),
            runner: ActionRunner::new(),
            list: ListState::new(),
            header: Header::new("LazyOS").subtitle("Bluetooth"),
            keybar: Vec::new(),
            rows_height: Cell::new(16),
        };
        screen.build_rows(false);
        if powered {
            screen.start_scan(3);
        }
        screen.sync_chrome();
        screen
    }

    fn start_scan(&mut self, secs: u32) {
        if self.runner.is_running() {
            return;
        }
        self.runner
            .spawn("Scanning for nearby devices\u{2026}", move || {
                do_scan(secs);
                let powered = BluetoothScreen::get_powered();
                let devices = if powered {
                    BluetoothScreen::get_devices()
                } else {
                    Vec::new()
                };
                BtOp::Scanned { powered, devices }
            });
        self.sync_chrome();
    }

    fn start_power_on(&mut self) {
        self.runner
            .spawn("Turning Bluetooth on & scanning\u{2026}", move || {
                let _ = Command::new("bluetoothctl")
                    .args(["power", "on"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                signal_bar();
                do_scan(3);
                let powered = BluetoothScreen::get_powered();
                let devices = if powered {
                    BluetoothScreen::get_devices()
                } else {
                    Vec::new()
                };
                BtOp::PoweredOn { powered, devices }
            });
        self.sync_chrome();
    }

    fn start_power_off(&mut self) {
        self.runner.spawn("Turning Bluetooth off\u{2026}", move || {
            let _ = Command::new("bluetoothctl")
                .args(["power", "off"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            signal_bar();
            BtOp::PoweredOff
        });
        self.sync_chrome();
    }

    fn start_toggle_device(&mut self, d: Device) {
        let label = if d.connected {
            format!("Disconnecting {}\u{2026}", d.name)
        } else {
            format!("Pairing with {}\u{2026}", d.name)
        };
        self.runner.spawn(label, move || {
            let (ok, message) = if d.connected {
                let out = Command::new("bluetoothctl")
                    .args(["disconnect", &d.mac])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output();
                let text = out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout))
                    .unwrap_or_default();
                if text.contains("Successful disconnected")
                    || out.as_ref().is_ok_and(|o| o.status.success())
                {
                    (true, format!("Disconnected {}", d.name))
                } else {
                    (false, format!("Could not disconnect {}", d.name))
                }
            } else {
                pair_and_connect(&d.mac, &d.name)
            };
            signal_bar();
            let powered = BluetoothScreen::get_powered();
            let devices = if powered {
                BluetoothScreen::get_devices()
            } else {
                Vec::new()
            };
            BtOp::DeviceResult {
                message,
                ok,
                powered,
                devices,
            }
        });
        self.sync_chrome();
    }

    fn get_powered() -> bool {
        out(&["bluetoothctl", "show"])
            .map(|s| s.contains("Powered: yes"))
            .unwrap_or(false)
    }

    fn get_devices() -> Vec<Device> {
        let connected = out(&["bluetoothctl", "devices", "Connected"]).unwrap_or_default();
        let mut connected_macs = std::collections::HashSet::new();
        for line in connected.lines() {
            if let Some(mac) = line
                .strip_prefix("Device ")
                .and_then(|s| s.split_whitespace().next())
            {
                connected_macs.insert(mac.to_string());
            }
        }

        let known = out(&["bluetoothctl", "devices"]).unwrap_or_default();
        let mut devices = Vec::new();
        for line in known.lines() {
            if let Some(rest) = line.strip_prefix("Device ") {
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let mac = parts[0].to_string();
                    let name = parts[1].to_string();
                    devices.push(Device {
                        mac: mac.clone(),
                        name,
                        connected: connected_macs.contains(&mac),
                    });
                }
            }
        }

        devices.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| is_raw_mac(&a.name).cmp(&is_raw_mac(&b.name)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        devices
    }

    fn build_rows(&mut self, preserve: bool) {
        let mut actions = Vec::new();
        if self.powered {
            actions.push(ActionItem::new("scan", "^R", "Rescan").glyph(Glyph::Sync));
            actions.push(ActionItem::new("toggle", "^T", "Turn off").glyph(Glyph::Power));
            actions.push(ActionItem::new("ctl", "^B", "bluetoothctl").glyph(Glyph::Terminal));
        } else {
            actions
                .push(ActionItem::new("toggle", "^T", "Turn Bluetooth on").glyph(Glyph::Bluetooth));
            actions.push(ActionItem::new("ctl", "^B", "bluetoothctl").glyph(Glyph::Terminal));
        }
        self.strip.set_items(actions);

        let mut rows = Vec::new();
        if self.powered {
            for d in &self.devices {
                let glyph = if d.connected {
                    Glyph::Check
                } else {
                    Glyph::Bluetooth
                };
                let detail = if d.connected {
                    format!("{} \u{00b7} connected", d.mac)
                } else {
                    format!("{} \u{00b7} available", d.mac)
                };
                rows.push(
                    Row::new(d.name.clone())
                        .glyph(glyph)
                        .detail(detail)
                        .value(d.mac.clone()),
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
        let status = if self.runner.is_running() {
            format!(
                "{} {}",
                self.runner.spinner(),
                self.runner.label().unwrap_or("Working\u{2026}")
            )
        } else if let Some(notice) = self.runner.notice() {
            notice.text.clone()
        } else if !self.powered {
            "Controller is off \u{00b7} Enter or Ctrl+T turns it on".to_string()
        } else {
            format!(
                "{} devices available \u{00b7} Ctrl+R rescans",
                self.devices.len()
            )
        };
        self.header = Header::new("LazyOS").subtitle("Bluetooth").status(status);
        self.keybar = vec![
            KeyHint::new("Enter", if self.powered { "connect" } else { "turn on" }),
            KeyHint::new("Tab", "actions"),
            KeyHint::new("Ctrl+R", "rescan"),
            KeyHint::new("Ctrl+T", "power"),
            KeyHint::new("Esc", "back"),
        ];
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
        } else if !self.powered {
            let x = draw_badge(buf, area.x, y, design, "RADIO OFF", ColorRole::Warn);
            let _ = put_trunc(
                buf,
                x,
                y,
                "  Bluetooth controller is disabled \u{00b7} press Enter or Ctrl+T to turn on",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        } else if let Some(active) = self.devices.iter().find(|d| d.connected) {
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
                &active.name,
                area.right().saturating_sub(x + 22),
                design.style(TextStyle::Title),
            );
            let meta = format!("  \u{00b7}  {}", active.mac);
            let _ = put_trunc(
                buf,
                x,
                y,
                &meta,
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        } else {
            let x = draw_badge(buf, area.x, y, design, "DISCONNECTED", ColorRole::Warn);
            let _ = put_trunc(
                buf,
                x,
                y,
                "  Select a device below to connect or pair",
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
            "scan" => {
                if self.powered {
                    self.start_scan(3);
                }
                Outcome::Consumed
            }
            "toggle" => {
                if self.powered {
                    self.start_power_off();
                } else {
                    self.start_power_on();
                }
                Outcome::Consumed
            }
            "ctl" => {
                crate::system::spawn_detached("exec foot --app-id=popup bluetoothctl");
                Outcome::Done(lazyos_ui::app::exit::OK)
            }
            _ => Outcome::Consumed,
        }
    }

    fn activate_device(&mut self, index: usize) -> Outcome {
        if self.runner.is_running() {
            return Outcome::Consumed;
        }
        let Some(d) = self.devices.get(index).cloned() else {
            return Outcome::Continue;
        };
        self.start_toggle_device(d);
        Outcome::Consumed
    }
}

impl Screen for BluetoothScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((860, 560))
    }

    fn needs_tick(&self) -> Option<Duration> {
        self.runner.tick_rate()
    }

    fn tick(&mut self) -> Outcome {
        if let Some(op) = self.runner.poll() {
            match op {
                BtOp::Scanned { powered, devices } => {
                    self.powered = powered;
                    self.devices = devices;
                    self.build_rows(true);
                    // Routine discovery clears the status banner immediately on completion.
                    self.runner.clear_status();
                }
                BtOp::PoweredOn { powered, devices } => {
                    self.powered = powered;
                    self.devices = devices;
                    self.build_rows(false);
                    self.runner.set_transient("Bluetooth turned on");
                }
                BtOp::PoweredOff => {
                    self.powered = false;
                    self.devices.clear();
                    self.build_rows(false);
                    self.runner.set_transient("Bluetooth turned off");
                }
                BtOp::DeviceResult {
                    message,
                    ok,
                    powered,
                    devices,
                } => {
                    self.powered = powered;
                    self.devices = devices;
                    self.build_rows(true);
                    if ok {
                        self.runner.set_transient(message);
                    } else {
                        // Connection/pairing failure overrides auto-hide so the user knows why.
                        self.runner.set_persistent_error(message);
                    }
                }
            }
            self.sync_chrome();
            return Outcome::Continue;
        }
        self.sync_chrome();
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Devices"), true);
        let (hero, strip, filter, rows) = Self::layout(ctx.area);
        self.draw_hero(ctx.buf, hero, ctx.design);
        self.strip.render(strip, ctx.buf, ctx.design);
        let hint = if !self.powered {
            "Bluetooth radio is off \u{00b7} press Enter or Ctrl+T to enable"
        } else {
            "type to filter devices \u{00b7} Enter connects \u{00b7} Tab focuses actions"
        };
        let _ = put;
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
        if !self.powered {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                "Bluetooth controller is off",
                "Press Enter or Ctrl+T to turn Bluetooth on and scan for devices",
            );
        } else if self.devices.is_empty() {
            draw_empty(
                ctx.buf,
                rows,
                ctx.design,
                if self.runner.is_running() {
                    "Scanning for nearby devices\u{2026}"
                } else {
                    "No Bluetooth devices found"
                },
                "Put your device in pairing mode and press Ctrl+R to rescan",
            );
        } else {
            self.list
                .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
        }
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
            return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('r')
                | KeyCode::Char('R')
                | KeyCode::Char('s')
                | KeyCode::Char('S') => {
                    return self.run_strip_action("scan");
                }
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    return self.run_strip_action("toggle");
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    return self.run_strip_action("ctl");
                }
                _ => {}
            }
        }
        if !self.powered && key.code == KeyCode::Enter {
            return self.run_strip_action("toggle");
        }
        let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL);
        if typed {
            self.list.type_filter(key);
            return Outcome::Consumed;
        }
        let rows_height = self.rows_height.get();
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate_device(index),
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
                return self.activate_device(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
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
}

/// The `--status` (`status`) JSON output for waybar (`custom/bt`).
pub fn status() -> String {
    let powered = BluetoothScreen::get_powered();
    if !powered {
        return format_status(false, &[]);
    }
    let connected = out(&["bluetoothctl", "devices", "Connected"]).unwrap_or_default();
    let names: Vec<String> = connected
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Device ")?;
            let (_, name) = rest.split_once(' ')?;
            Some(name.trim().to_string())
        })
        .collect();
    format_status(true, &names)
}

fn format_status(powered: bool, connected_names: &[String]) -> String {
    // The state is the picture, not a colour. Both glyphs are Material Design
    // Icons (`fonts-materialdesignicons-legacy`, already in
    // uis/sway/packages.list) so the radio and the slash are drawn by one
    // family: `f00af` is the rune, `f00b2` the same rune with a slash through
    // it - the pattern the network module uses for wifi (`f05a9`/`f05aa`) and
    // the volume module for mute (`f0581`).
    const ON: &str = "\u{f00af}";
    const OFF: &str = "\u{f00b2}";
    if !powered {
        return format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span>\",\"class\":\"off\",\"tooltip\":\"Bluetooth off (click to turn on)\"}}",
            OFF
        );
    }
    if connected_names.is_empty() {
        format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span>\",\"class\":\"on\",\"tooltip\":\"Bluetooth on · nothing connected\"}}",
            ON
        )
    } else {
        let tooltip = connected_names.join("\\n").replace('"', "'");
        // No count on the bar: the only number on the line is the battery's
        // charge. What is connected is in the tooltip.
        format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span>\",\"class\":\"on\",\"tooltip\":\"{}\"}}",
            ON, tooltip
        )
    }
}

/// `lazyos menu bluetooth` (`bar-bt`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Bluetooth",
            format!("{said} (bluez, foot)"),
        ));
    }
    let mut screen = BluetoothScreen::new();
    crate::run_screen(terminal, design, "Bluetooth", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_needed_tools() {
        assert!(NEEDED.contains(&"bluetoothctl"));
        assert!(NEEDED.contains(&"foot"));
    }

    #[test]
    fn raw_mac_detection() {
        assert!(is_raw_mac("44-F9-B4-7F-6C-CA"));
        assert!(is_raw_mac("44:F9:B4:7F:6C:CA"));
        assert!(!is_raw_mac("Sala de estar tv"));
        assert!(!is_raw_mac("AirPods"));
    }

    #[test]
    fn status_json_for_waybar() {
        let off = format_status(false, &[]);
        assert!(off.contains("\"class\":\"off\""));
        assert!(off.contains("\u{f00b2}"));
        let on = format_status(true, &["Buds".to_string()]);
        assert!(on.contains("\"class\":\"on\""));
        assert!(on.contains("\u{f00af}"));
        assert!(!on.contains("</span> 1\""));
        assert!(on.contains("\"tooltip\":\"Buds\""));
    }
}
