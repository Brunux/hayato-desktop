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
    ActionRunner, BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState,
    Outcome, Row, Screen, TextStyle, draw_badge, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["bluetoothctl", "foot"];

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

#[derive(Clone, Debug)]
struct Device {
    mac: String,
    name: String,
    connected: bool,
}

#[derive(Clone, Debug)]
enum BtAction {
    PowerOn,
    PowerOff,
    Scan,
    ToggleDevice(Device),
    OpenCtl,
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

/// The bluetooth screen.
pub struct BluetoothScreen {
    devices: Vec<Device>,
    powered: bool,
    actions: Vec<BtAction>,
    runner: ActionRunner<BtOp>,
    banner: Option<(String, ColorRole)>,
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
            actions: Vec::new(),
            runner: ActionRunner::new(),
            banner: None,
            list: ListState::new(),
            header: Header::new("LazyOS").subtitle("Bluetooth"),
            keybar: vec![
                KeyHint::new("Enter", "connect / toggle"),
                KeyHint::new("Ctrl+S", "scan"),
                KeyHint::new("Esc", "close"),
            ],
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
        self.runner.spawn("Scanning for nearby devices\u{2026}", move || {
            do_scan(secs);
            let powered = BluetoothScreen::get_powered();
            let devices = if powered {
                BluetoothScreen::get_devices()
            } else {
                Vec::new()
            };
            BtOp::Scanned { powered, devices }
        });
        self.build_rows(true);
        self.sync_chrome();
    }

    fn start_power_on(&mut self) {
        self.runner
            .spawn("Powering on Bluetooth & scanning\u{2026}", move || {
                let _ = Command::new("bluetoothctl")
                    .args(["power", "on"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
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
        self.runner.spawn("Powering off Bluetooth\u{2026}", move || {
            let _ = Command::new("bluetoothctl")
                .args(["power", "off"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            BtOp::PoweredOff
        });
        self.sync_chrome();
    }

    fn start_toggle_device(&mut self, d: Device) {
        let label = if d.connected {
            format!("Disconnecting {}\u{2026}", d.name)
        } else {
            format!("Connecting to {}\u{2026}", d.name)
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
                let out = Command::new("bluetoothctl")
                    .args(["connect", &d.mac])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output();
                let text = out
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(&o.stdout))
                    .unwrap_or_default();
                if text.contains("Connection successful") {
                    (true, format!("Connected to {}", d.name))
                } else {
                    let pair_out = Command::new("bluetoothctl")
                        .args(["pair", &d.mac])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .output();
                    let pair_text = pair_out
                        .as_ref()
                        .map(|o| String::from_utf8_lossy(&o.stdout))
                        .unwrap_or_default();
                    if pair_text.contains("Pairing successful") {
                        let _ = Command::new("bluetoothctl")
                            .args(["trust", &d.mac])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                        let _ = Command::new("bluetoothctl")
                            .args(["connect", &d.mac])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                        (true, format!("Paired and connected to {}", d.name))
                    } else {
                        (false, format!("Could not connect to {}", d.name))
                    }
                }
            };
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
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        if !self.powered {
            rows.push(
                Row::new("Turn Bluetooth on")
                    .glyph(Glyph::Bluetooth)
                    .detail("power on controller and scan for devices"),
            );
            actions.push(BtAction::PowerOn);
            rows.push(
                Row::new("Open bluetoothctl")
                    .glyph(Glyph::Terminal)
                    .detail("pairing keys, trust, remove, agent"),
            );
            actions.push(BtAction::OpenCtl);
        } else {
            for d in &self.devices {
                let glyph = if d.connected {
                    Glyph::Check
                } else {
                    Glyph::Bluetooth
                };
                let state = if d.connected {
                    "\u{25cf} connected"
                } else {
                    "available"
                };
                rows.push(
                    Row::new(d.name.clone())
                        .glyph(glyph)
                        .detail(d.mac.clone())
                        .value(state),
                );
                actions.push(BtAction::ToggleDevice(d.clone()));
            }

            let (scan_label, scan_detail) = if self.runner.is_running() {
                (
                    "Scanning for devices\u{2026}",
                    "discovery in progress \u{00b7} list updates automatically",
                )
            } else {
                (
                    "Scan for devices",
                    "discover nearby Bluetooth devices (Ctrl+S)",
                )
            };
            rows.push(
                Row::new(scan_label)
                    .glyph(Glyph::Sync)
                    .detail(scan_detail),
            );
            actions.push(BtAction::Scan);

            rows.push(
                Row::new("Turn Bluetooth off")
                    .glyph(Glyph::Power)
                    .detail("power off the controller"),
            );
            actions.push(BtAction::PowerOff);

            rows.push(
                Row::new("Open bluetoothctl")
                    .glyph(Glyph::Terminal)
                    .detail("pairing keys, trust, remove, agent"),
            );
            actions.push(BtAction::OpenCtl);
        }

        self.actions = actions;
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
        } else if let Some((msg, _)) = &self.banner {
            msg.clone()
        } else if !self.powered {
            "Controller is off \u{00b7} Enter turns it on".to_string()
        } else {
            format!("{} devices known \u{00b7} Ctrl+S scans", self.devices.len())
        };
        self.header = Header::new("LazyOS")
            .subtitle("Bluetooth")
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
        if area.height == 0 || area.width < 16 {
            return;
        }
        let y = area.y;
        if !self.powered {
            let x = draw_badge(buf, area.x, y, design, "POWERED OFF", ColorRole::Warn);
            let _ = put_trunc(
                buf,
                x,
                y,
                "  Press Enter on 'Turn Bluetooth on' to enable & scan",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        } else {
            let mut x = draw_badge(buf, area.x, y, design, "POWERED ON", ColorRole::Ok);
            let connected: Vec<&str> = self
                .devices
                .iter()
                .filter(|d| d.connected)
                .map(|d| d.name.as_str())
                .collect();
            if connected.is_empty() {
                x = put_trunc(
                    buf,
                    x,
                    y,
                    "  No active audio/input device connected",
                    area.right().saturating_sub(x),
                    design.style(TextStyle::Dim),
                );
            } else {
                let summary = format!("  \u{25cf} {}", connected.join(", "));
                x = put_trunc(
                    buf,
                    x,
                    y,
                    &summary,
                    area.right().saturating_sub(x),
                    design.style(TextStyle::Title),
                );
            }
            let _ = x;
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
        let Some(action) = self.actions.get(index).cloned() else {
            return Outcome::Continue;
        };
        match action {
            BtAction::Scan => {
                self.banner = None;
                self.start_scan(3);
            }
            BtAction::PowerOn => {
                self.banner = None;
                self.start_power_on();
            }
            BtAction::PowerOff => {
                self.banner = None;
                self.start_power_off();
            }
            BtAction::OpenCtl => {
                crate::system::spawn_detached("exec foot --app-id=popup bluetoothctl");
                return Outcome::Done(lazyos_ui::app::exit::OK);
            }
            BtAction::ToggleDevice(d) => {
                self.banner = None;
                self.start_toggle_device(d);
            }
        }
        Outcome::Consumed
    }
}

impl Screen for BluetoothScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((840, 520))
    }

    fn needs_tick(&self) -> Option<Duration> {
        self.runner.tick_rate()
    }

    fn tick(&mut self) -> Outcome {
        self.runner.tick();
        if let Some(op) = self.runner.poll() {
            match op {
                BtOp::Scanned { powered, devices } => {
                    self.powered = powered;
                    self.devices = devices;
                    self.build_rows(true);
                    self.banner = Some((
                        format!("Discovery finished \u{00b7} {} devices", self.devices.len()),
                        ColorRole::Ok,
                    ));
                }
                BtOp::PoweredOn { powered, devices } => {
                    self.powered = powered;
                    self.devices = devices;
                    self.build_rows(false);
                    self.banner = Some(("Bluetooth powered on".to_string(), ColorRole::Ok));
                }
                BtOp::PoweredOff => {
                    self.powered = false;
                    self.devices.clear();
                    self.build_rows(false);
                    self.banner = Some(("Bluetooth powered off".to_string(), ColorRole::Warn));
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
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Bluetooth"), true);
        let (hero, filter, rows) = Self::layout(ctx.area);
        self.draw_hero(ctx.buf, hero, ctx.design);
        let hint = if self.runner.is_running() {
            self.runner.label().unwrap_or("Scanning\u{2026}")
        } else if let Some((msg, _)) = &self.banner {
            msg.as_str()
        } else if !self.powered {
            "Bluetooth is off \u{00b7} Enter turns it on"
        } else {
            "type to filter devices \u{00b7} Enter connects \u{00b7} Ctrl+S scans"
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
        if (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')))
            || (self.list.filter().is_empty() && key.code == KeyCode::Char('s'))
        {
            if self.powered && !self.runner.is_running() {
                self.banner = None;
                self.start_scan(3);
            }
            return Outcome::Consumed;
        }
        let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL);
        if typed {
            self.list.type_filter(key);
            return Outcome::Consumed;
        }
        let rows_height = self.rows_height.get();
        match self.list.handle(key, rows_height, design) {
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
    const ICON: &str = "\u{f294}";
    if !powered {
        return format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span>\",\"class\":\"off\",\"tooltip\":\"Bluetooth off (click to turn on)\"}}",
            ICON
        );
    }
    if connected_names.is_empty() {
        format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span>\",\"class\":\"on\",\"tooltip\":\"Bluetooth on · nothing connected\"}}",
            ICON
        )
    } else {
        let tooltip = connected_names.join("\\n").replace('"', "'");
        format!(
            "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{}</span> {}\",\"class\":\"on\",\"tooltip\":\"{}\"}}",
            ICON,
            connected_names.len(),
            tooltip
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
        assert!(off.contains("\u{f294}"));
        let on = format_status(true, &["Buds".to_string()]);
        assert!(on.contains("\"class\":\"on\""));
        assert!(on.contains("</span> 1\""));
        assert!(on.contains("\"tooltip\":\"Buds\""));
    }
}
