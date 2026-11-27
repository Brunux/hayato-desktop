//! QR codes: the Wi-Fi network this machine is on, clipboard text, or any text (`hayato-qr`).
//!
//! The payload is the Wi-Fi QR format phones understand:
//!
//! ```text
//! WIFI:T:WPA;S:<ssid>;P:<password>;;
//! ```
//!
//! with `\`, `;`, `,` and `:` escaped, which is what the ZXing reader expects.
//! The password is read with `nmcli -s` through sudo, encoded in memory with
//! `qrencode -t ASCII -m 2`, and rendered directly inside the Ratatui window
//! using Unicode half-blocks (`▀`, `▄`, `█`) with dynamic window sizing.
//! Generated QR codes can also be saved as high-resolution PNG files under
//! `~/Pictures/Screenshots/<YYYY-MM-DD>/qr-<timestamp>.png`, matching the
//! screenshot storage convention.

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use hayato_ui::widgets::put_trunc;
use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Design, Glyph, Header, Input,
    InputAction, KeyHint, ListAction, ListState, Outcome, QrCode, Row, Screen, TextStyle,
    draw_card, draw_qr,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

use crate::system::{missing, out, sudo};

const NEEDED: &[&str] = &["qrencode", "nmcli", "sudo"];

enum QrMode {
    Menu,
    Typing(Input),
    Viewing {
        qr: QrCode,
        title: String,
        subtitle: String,
        payload: String,
        direct: bool,
        saved_path: Option<PathBuf>,
    },
}

/// The QR screen.
pub struct QrScreen {
    wifi_detail: String,
    wifi_colour: ColorRole,
    list: ListState,
    view_strip: ActionStrip,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    mode: QrMode,
    status: String,
}

/// Resolve the daily screenshot directory (`~/Pictures/Screenshots/<YYYY-MM-DD>`),
/// matching `~/.config/bin/screenshot`.
pub fn qr_target_dir() -> PathBuf {
    let base = std::env::var("XDG_PICTURES_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::picture_dir)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join("Pictures")
        });
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    base.join("Screenshots").join(day)
}

/// Save a QR payload as a PNG image in `~/Pictures/Screenshots/<YYYY-MM-DD>/<prefix>-<timestamp>.png`.
/// Pipes the payload via stdin so sensitive values (such as Wi-Fi PSKs) never appear in `argv`.
pub fn save_qr_png(payload: &str, prefix: &str) -> Result<PathBuf, String> {
    let dir = qr_target_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let file = dir.join(format!("{prefix}-{stamp}.png"));

    let mut child = Command::new("qrencode")
        .args([
            "-t",
            "PNG",
            "-s",
            "8",
            "-m",
            "2",
            "-o",
            file.to_str().unwrap_or(""),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not run qrencode: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    let status = child.wait().map_err(|e| format!("qrencode failed: {e}"))?;
    if status.success() && file.exists() {
        Ok(file)
    } else {
        Err("qrencode did not write the PNG file".to_string())
    }
}

/// Format a Wi-Fi QR payload (`WIFI:T:WPA;S:...;P:...;;` or `nopass`).
pub fn format_wifi_payload(ssid: &str, psk: &str) -> String {
    if psk.is_empty() {
        format!("WIFI:T:nopass;S:{};;", QrScreen::qr_escape(ssid))
    } else {
        format!(
            "WIFI:T:WPA;S:{};P:{};;",
            QrScreen::qr_escape(ssid),
            QrScreen::qr_escape(psk)
        )
    }
}

/// Read the connected Wi-Fi network's QR payload and window title.
pub fn payload_wifi() -> Option<(String, String)> {
    let dev = out(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device", "status"])?;
    let dev = dev
        .lines()
        .find(|l| l.split(':').nth(1) == Some("wifi"))
        .and_then(|l| l.split(':').next())?;
    let con = out(&["nmcli", "-g", "GENERAL.CONNECTION", "device", "show", dev])?;
    let con = con.lines().next().unwrap_or("").trim();
    if con.is_empty() || con == "--" {
        return None;
    }
    let ssid = out(&[
        "nmcli",
        "-g",
        "802-11-wireless.ssid",
        "connection",
        "show",
        con,
    ])
    .unwrap_or_default()
    .trim()
    .to_string();
    if ssid.is_empty() {
        return None;
    }
    let psk = sudo(&[
        "nmcli",
        "-s",
        "-g",
        "802-11-wireless-security.psk",
        "connection",
        "show",
        con,
    ])
    .unwrap_or_default()
    .trim()
    .to_string();
    Some((
        format_wifi_payload(&ssid, &psk),
        format!("Wi-Fi \u{00b7} {ssid}"),
    ))
}

fn copy_to_clipboard(text: &str) -> bool {
    let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

impl Default for QrScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl QrScreen {
    pub fn new() -> Self {
        let mut view_strip = ActionStrip::default();
        view_strip.set_items(vec![
            ActionItem::new("save", "^S", "Save PNG").glyph(Glyph::Save),
            ActionItem::new("copy", "^C", "Copy payload").glyph(Glyph::Clipboard),
            ActionItem::new("open", "^O", "Open folder").glyph(Glyph::Folder),
        ]);
        let mut screen = Self {
            wifi_detail: "not connected".to_string(),
            wifi_colour: ColorRole::Muted,
            list: ListState::new(),
            view_strip,
            header: Header::new("Hayato").subtitle("QR code"),
            keybar: Vec::new(),
            rows_height: Cell::new(20),
            mode: QrMode::Menu,
            status: "Enter shows the code \u{00b7} Escape closes".to_string(),
        };
        screen.reload();
        screen.sync_chrome();
        screen
    }

    pub fn new_wifi_direct() -> Self {
        let mut screen = Self::new();
        screen.open_wifi_qr(true);
        screen
    }

    fn sync_chrome(&mut self) {
        match &self.mode {
            QrMode::Menu => {
                self.header = Header::new("Hayato")
                    .subtitle("QR code")
                    .status(self.status.clone());
                self.keybar = vec![
                    KeyHint::new("Enter", "show the code"),
                    KeyHint::new("Esc", "close"),
                ];
            }
            QrMode::Typing(_) => {
                self.header = Header::new("Hayato")
                    .subtitle("QR code \u{00b7} Any text")
                    .status("Type text or URL \u{00b7} Enter encodes");
                self.keybar = vec![KeyHint::new("Enter", "encode"), KeyHint::new("Esc", "back")];
            }
            QrMode::Viewing { title, .. } => {
                self.header = Header::new("Hayato")
                    .subtitle(title.clone())
                    .status(self.status.clone());
                self.keybar = vec![
                    KeyHint::new("s/^S", "save PNG"),
                    KeyHint::new("c/^C", "copy payload"),
                    KeyHint::new("o/^O", "open"),
                    KeyHint::new("Esc", "back"),
                ];
            }
        }
    }

    fn reload(&mut self) {
        let (detail, colour) = Self::get_wifi_status();
        self.wifi_detail = detail;
        self.wifi_colour = colour;
        self.build_rows();
    }

    fn get_wifi_status() -> (String, ColorRole) {
        let dev =
            out(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device", "status"]).unwrap_or_default();
        let dev = dev
            .lines()
            .find(|l| l.split(':').nth(1) == Some("wifi"))
            .and_then(|l| l.split(':').next())
            .map(|s| s.to_string());
        if let Some(dev) = dev {
            let con = out(&["nmcli", "-g", "GENERAL.CONNECTION", "device", "show", &dev])
                .unwrap_or_default();
            let con = con.lines().next().unwrap_or("").trim();
            if !con.is_empty() && con != "--" {
                return (con.to_string(), ColorRole::Ok);
            }
        }
        ("not connected".to_string(), ColorRole::Muted)
    }

    fn build_rows(&mut self) {
        let clip_detail = out(&["wl-paste", "--no-newline"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let first = s.lines().next().unwrap_or("");
                let short: String = first.chars().take(44).collect();
                if first.chars().count() > 44 {
                    format!("{short}\u{2026}")
                } else {
                    short
                }
            })
            .unwrap_or_else(|| "encode current clipboard text".to_string());

        let rows = vec![
            Row::new("Wi-Fi network")
                .glyph(Glyph::Wifi)
                .detail(self.wifi_detail.clone())
                .value("wifi"),
            Row::new("From the clipboard")
                .glyph(Glyph::Clipboard)
                .detail(clip_detail)
                .value("clipboard"),
            Row::new("Any text")
                .glyph(Glyph::Qr)
                .detail("type it, and it becomes a code")
                .value("text"),
        ];
        self.list.set_rows(rows);
    }

    pub fn qr_escape(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace(';', "\\;")
            .replace(',', "\\,")
            .replace(':', "\\:")
    }

    fn open_wifi_qr(&mut self, direct: bool) {
        let Some((payload, title)) = payload_wifi() else {
            self.status = "Not connected to a Wi-Fi network".to_string();
            self.sync_chrome();
            return;
        };
        if let Some(qr) = QrCode::encode(&payload) {
            self.view_strip.set_focused(false);
            self.status =
                "s saves PNG \u{00b7} c copies payload \u{00b7} Tab focuses actions".to_string();
            self.mode = QrMode::Viewing {
                qr,
                title,
                subtitle: "Scan with camera to join Wi-Fi".to_string(),
                payload,
                direct,
                saved_path: None,
            };
            self.sync_chrome();
        } else {
            self.status = "Could not encode QR: payload too large".to_string();
            self.sync_chrome();
        }
    }

    fn open_text_qr(&mut self, payload: String) {
        if let Some(qr) = QrCode::encode(&payload) {
            let preview: String = payload.chars().take(42).collect();
            self.view_strip.set_focused(false);
            self.status =
                "s saves PNG \u{00b7} c copies payload \u{00b7} Tab focuses actions".to_string();
            self.mode = QrMode::Viewing {
                qr,
                title: "QR code".to_string(),
                subtitle: preview,
                payload,
                direct: false,
                saved_path: None,
            };
            self.sync_chrome();
        } else {
            self.status = "Could not encode QR: payload too large".to_string();
            self.mode = QrMode::Menu;
            self.sync_chrome();
        }
    }

    fn text_qr(&mut self) {
        self.mode = QrMode::Typing(Input::new());
        self.sync_chrome();
    }

    fn run_view_action(&mut self, id: &str) -> Outcome {
        let QrMode::Viewing {
            payload,
            title,
            saved_path,
            ..
        } = &mut self.mode
        else {
            return Outcome::Continue;
        };
        match id {
            "save" => {
                let prefix = if title.starts_with("Wi-Fi") {
                    "wifi-qr"
                } else {
                    "qr"
                };
                match save_qr_png(payload, prefix) {
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
                        *saved_path = Some(path);
                        self.status = format!("Saved Screenshots/{day}/{name}");
                    }
                    Err(err) => {
                        self.status = err;
                    }
                }
                self.sync_chrome();
                Outcome::Consumed
            }
            "copy" => {
                if copy_to_clipboard(payload) {
                    self.status = "Copied QR payload to clipboard".to_string();
                } else {
                    self.status = "Could not copy payload (wl-copy unavailable)".to_string();
                }
                self.sync_chrome();
                Outcome::Consumed
            }
            "open" => {
                let target = if let Some(p) = saved_path.clone() {
                    p
                } else {
                    let prefix = if title.starts_with("Wi-Fi") {
                        "wifi-qr"
                    } else {
                        "qr"
                    };
                    match save_qr_png(payload, prefix) {
                        Ok(p) => {
                            *saved_path = Some(p.clone());
                            p
                        }
                        Err(_) => qr_target_dir(),
                    }
                };
                let _ = fs::create_dir_all(qr_target_dir());
                let target_str = target.to_string_lossy().into_owned();
                crate::system::spawn_detached_argv("xdg-open", &[&target_str]);
                self.status = format!("Opened {}", target.display());
                self.sync_chrome();
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

impl Screen for QrScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        match &self.mode {
            QrMode::Menu | QrMode::Typing(_) => Some((680, 340)),
            QrMode::Viewing { qr, .. } => {
                let (w, h) = qr.recommended_window_size();
                Some((w.max(560), h.saturating_add(28)))
            }
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        match &self.mode {
            QrMode::Viewing {
                qr,
                title,
                subtitle,
                ..
            } => {
                draw_card(ctx.buf, ctx.area, ctx.design, Some(title), true);
                let strip_area = Rect {
                    x: ctx.area.x + 2,
                    y: ctx.area.y + 1,
                    width: ctx.area.width.saturating_sub(4),
                    height: 1.min(ctx.area.height.saturating_sub(2)),
                };
                self.view_strip.render(strip_area, ctx.buf, ctx.design);
                if ctx.area.height >= 5 && !subtitle.is_empty() {
                    put_trunc(
                        ctx.buf,
                        ctx.area.x + 2,
                        ctx.area.y + 2,
                        subtitle,
                        ctx.area.width.saturating_sub(4),
                        ctx.design.style(TextStyle::Dim),
                    );
                }
                let inner = Rect {
                    x: ctx.area.x + 2,
                    y: ctx.area.y + 3,
                    width: ctx.area.width.saturating_sub(4),
                    height: ctx.area.height.saturating_sub(4),
                };
                draw_qr(ctx.buf, inner, ctx.design, qr);
            }
            QrMode::Typing(input) => {
                draw_card(ctx.buf, ctx.area, ctx.design, Some("The code"), true);
                let (filter, rows) = super::pick::list_layout(ctx.area);
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    "Type text or URL \u{00b7} Enter shows the code \u{00b7} Escape returns",
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
                );
                let field = Rect {
                    height: 1.min(rows.height),
                    ..rows
                };
                input.render(field, ctx.buf, ctx.design, "text or URL\u{2026}");
            }
            QrMode::Menu => {
                let (left, right) = super::split_panes(ctx.area, 50, 46);
                draw_card(ctx.buf, left, ctx.design, Some("The code"), true);
                let (filter, rows) = super::pick::list_layout(left);
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    &self.status,
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
                );
                self.rows_height.set(rows.height);
                if let Some(right_area) = right {
                    draw_card(
                        ctx.buf,
                        right_area,
                        ctx.design,
                        Some("QR Studio Deck"),
                        false,
                    );
                    let r_in = Rect::new(
                        right_area.x + 2,
                        right_area.y + 1,
                        right_area.width.saturating_sub(4),
                        right_area.height.saturating_sub(2),
                    );
                    if r_in.width >= 16 && r_in.height >= 8 {
                        let dim = ctx.design.style(TextStyle::Dim);
                        let body = ctx.design.style(TextStyle::Body);
                        let accent = ratatui::style::Style::default()
                            .fg(ctx.design.color(ColorRole::Accent))
                            .add_modifier(ratatui::style::Modifier::BOLD);
                        let mut ry = r_in.y;
                        put_trunc(ctx.buf, r_in.x, ry, "QR GENERATOR", r_in.width, accent);
                        ry += 2;
                        put_trunc(
                            ctx.buf,
                            r_in.x,
                            ry,
                            &format!("Wi-Fi     {}", self.wifi_detail),
                            r_in.width,
                            body,
                        );
                        ry += 2;
                        for line in [
                            "Renders crisp Unicode half-block QR codes",
                            "directly in the terminal with zero temp files.",
                            "",
                            "While viewing a QR code:",
                            "  s / Ctrl+S   Save PNG to ~/Pictures/Screenshots",
                            "  c / Ctrl+C   Copy raw payload to clipboard",
                            "  o / Ctrl+O   Open saved PNG",
                        ] {
                            if ry >= r_in.bottom().saturating_sub(1) {
                                break;
                            }
                            let st = if line.starts_with("While") {
                                accent
                            } else if line.starts_with("  ") {
                                body
                            } else {
                                dim
                            };
                            put_trunc(ctx.buf, r_in.x, ry, line, r_in.width, st);
                            ry += 1;
                        }
                    }
                }
                self.list.render(rows, ctx.buf, ctx.design, true);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if matches!(self.mode, QrMode::Viewing { .. }) {
            match self.view_strip.handle(key) {
                ActionStripEvent::Activated(id) => return self.run_view_action(id),
                ActionStripEvent::Consumed => return Outcome::Consumed,
                ActionStripEvent::Ignored => {}
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    return self.run_view_action("save");
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    return self.run_view_action("copy");
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    return self.run_view_action("open");
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') if !ctrl => {
                    let is_direct = matches!(self.mode, QrMode::Viewing { direct: true, .. });
                    if is_direct {
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    self.mode = QrMode::Menu;
                    self.status = "Enter shows the code \u{00b7} Escape closes".to_string();
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                _ => return Outcome::Consumed,
            }
        }

        match &mut self.mode {
            QrMode::Viewing { .. } => Outcome::Consumed,
            QrMode::Typing(input) => match input.handle(key) {
                InputAction::Submitted => {
                    let trimmed = input.text().trim().to_string();
                    if trimmed.is_empty() {
                        self.mode = QrMode::Menu;
                        self.sync_chrome();
                        return Outcome::Consumed;
                    }
                    self.open_text_qr(trimmed);
                    Outcome::Consumed
                }
                InputAction::Cancelled => {
                    self.mode = QrMode::Menu;
                    self.status = "Enter shows the code \u{00b7} Escape closes".to_string();
                    self.sync_chrome();
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
            QrMode::Menu => {
                let rows_height = self.rows_height.get();
                match key.code {
                    KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
                    KeyCode::Enter => {
                        if let Some(index) = self.list.selected_index() {
                            return self.activate(index);
                        }
                    }
                    _ => {}
                }
                match self.list.handle(key, rows_height, design) {
                    ListAction::Activated(index) => self.activate(index),
                    _ => Outcome::Continue,
                }
            }
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if matches!(self.mode, QrMode::Viewing { .. }) {
            if let Some(r) = row
                && r >= 10_000
                && let Some(item) = self.view_strip.items().get(r - 10_000)
            {
                return self.run_view_action(item.id);
            }
            return Outcome::Continue;
        }
        if !matches!(self.mode, QrMode::Menu) {
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
        if matches!(self.mode, QrMode::Viewing { .. }) {
            let strip_area = Rect {
                x: area.x + 2,
                y: area.y + 1,
                width: area.width.saturating_sub(4),
                height: 1,
            };
            if let Some(idx) = self.view_strip.hit_index(strip_area, x, y) {
                return Some(10_000 + idx);
            }
            return None;
        }
        if !matches!(self.mode, QrMode::Menu) {
            return None;
        }
        let (_, rows) = super::pick::list_layout(area);
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
            QrMode::Menu => "menu",
            QrMode::Typing(_) => "typing",
            QrMode::Viewing { .. } => "viewing",
        })
    }
}

impl QrScreen {
    fn activate(&mut self, index: usize) -> Outcome {
        let rows = self.list.rows();
        let Some(row) = rows.get(index) else {
            return Outcome::Continue;
        };
        let Some(key) = row.value.as_deref() else {
            return Outcome::Continue;
        };
        if key.is_empty() || key == "-" {
            return Outcome::Continue;
        }
        match key {
            "wifi" => {
                self.open_wifi_qr(false);
                Outcome::Consumed
            }
            "clipboard" => {
                let clip = out(&["wl-paste", "--no-newline"])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if clip.is_empty() {
                    self.status =
                        "Clipboard is empty \u{00b7} copy text or a URL first".to_string();
                    self.sync_chrome();
                } else {
                    self.open_text_qr(clip);
                }
                Outcome::Consumed
            }
            "text" => {
                self.text_qr();
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }
}

/// `hayato menu qr` (`hayato-qr`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    menu_with_args(&[], terminal, design)
}

/// `hayato menu qr [--wifi]`: opens in menu mode or directly in Wi-Fi QR view.
pub fn menu_with_args(
    args: &[String],
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "QR code",
            format!("{said} (qrencode, nmcli, sudo)"),
        ));
    }
    let wifi_direct = args.iter().any(|a| a == "--wifi");
    let mut screen = if wifi_direct {
        QrScreen::new_wifi_direct()
    } else {
        QrScreen::new()
    };
    crate::run_screen(terminal, design, "QR code", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_escape_chars() {
        assert_eq!(QrScreen::qr_escape("a\\b;c,d:e"), "a\\\\b\\;c\\,d\\:e");
        assert_eq!(QrScreen::qr_escape("plain"), "plain");
    }

    #[test]
    fn wifi_payload_formats_wpa_and_nopass() {
        assert_eq!(
            format_wifi_payload("My:Net;1", "pa,ss\\2"),
            "WIFI:T:WPA;S:My\\:Net\\;1;P:pa\\,ss\\\\2;;"
        );
        assert_eq!(
            format_wifi_payload("OpenNet", ""),
            "WIFI:T:nopass;S:OpenNet;;"
        );
    }

    #[test]
    fn qr_target_dir_uses_screenshots_date_subfolder() {
        let dir = qr_target_dir();
        let path_str = dir.to_string_lossy();
        assert!(path_str.contains("Screenshots/"));
    }

    #[test]
    fn text_row_opens_input_and_esc_returns_to_list() {
        let mut screen = QrScreen::new();
        screen.build_rows();
        assert_eq!(screen.list.rows().len(), 3);
        assert_eq!(screen.list.rows()[0].value.as_deref(), Some("wifi"));
        assert_eq!(screen.list.rows()[1].value.as_deref(), Some("clipboard"));
        assert_eq!(screen.list.rows()[2].value.as_deref(), Some("text"));
        assert_eq!(screen.activate(2), Outcome::Consumed);
        assert!(matches!(screen.mode, QrMode::Typing(_)));
        let design = Design::solarized();
        let esc = KeyEvent::from(KeyCode::Esc);
        assert_eq!(screen.key(esc, &design), Outcome::Consumed);
        assert!(matches!(screen.mode, QrMode::Menu));
    }
}
