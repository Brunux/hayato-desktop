//! QR codes: the Wi-Fi network this machine is on, or any text (`rofi-qr`).
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

use std::cell::Cell;
use std::io::Write;
use std::process::{Command, Stdio};

use lazyos_ui::widgets::put_trunc;
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, Input, InputAction, KeyHint, ListAction, ListState,
    Outcome, QrCode, Row, Screen, TextStyle, draw_card, draw_qr,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
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
    },
}

/// The QR screen.
pub struct QrScreen {
    wifi_detail: String,
    wifi_colour: ColorRole,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    mode: QrMode,
    status: String,
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
    Some((format_wifi_payload(&ssid, &psk), format!("Wi-Fi \u{00b7} {ssid}")))
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
        let mut screen = Self {
            wifi_detail: "not connected".to_string(),
            wifi_colour: ColorRole::Dim,
            list: ListState::new(),
            header: Header::new("LazyOS").subtitle("QR code"),
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
                self.header = Header::new("LazyOS")
                    .subtitle("QR code")
                    .status(self.status.clone());
                self.keybar = vec![
                    KeyHint::new("Enter", "show the code"),
                    KeyHint::new("Esc", "close"),
                ];
            }
            QrMode::Typing(_) => {
                self.header = Header::new("LazyOS")
                    .subtitle("QR code \u{00b7} Any text")
                    .status("Type text or URL \u{00b7} Enter encodes");
                self.keybar = vec![
                    KeyHint::new("Enter", "encode"),
                    KeyHint::new("Esc", "back"),
                ];
            }
            QrMode::Viewing { title, .. } => {
                self.header = Header::new("LazyOS")
                    .subtitle(title.clone())
                    .status(self.status.clone());
                self.keybar = vec![
                    KeyHint::new("c", "copy payload"),
                    KeyHint::new("Enter/Esc", "back"),
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
        ("not connected".to_string(), ColorRole::Dim)
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
            self.status = "Scan with a phone camera \u{00b7} c copies payload".to_string();
            self.mode = QrMode::Viewing {
                qr,
                title,
                subtitle: "Scan with camera to join Wi-Fi".to_string(),
                payload,
                direct,
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
            self.status = "Scan with a camera \u{00b7} c copies payload".to_string();
            self.mode = QrMode::Viewing {
                qr,
                title: "QR code".to_string(),
                subtitle: preview,
                payload,
                direct: false,
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
}

impl Screen for QrScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        match &self.mode {
            QrMode::Menu | QrMode::Typing(_) => Some((680, 340)),
            QrMode::Viewing { qr, .. } => Some(qr.recommended_window_size()),
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
            QrMode::Typing(input) => {
                draw_card(ctx.buf, ctx.area, ctx.design, Some("QR code"), true);
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
                draw_card(ctx.buf, ctx.area, ctx.design, Some("QR code"), true);
                let (filter, rows) = super::pick::list_layout(ctx.area);
                put_trunc(
                    ctx.buf,
                    filter.x,
                    filter.y,
                    &self.status,
                    filter.width,
                    ctx.design.style(TextStyle::Dim),
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
            QrMode::Viewing {
                payload, direct, ..
            } => match key.code {
                KeyCode::Char('c') => {
                    if copy_to_clipboard(payload) {
                        self.status = "Copied QR payload to clipboard".to_string();
                    } else {
                        self.status = "Could not copy payload (wl-copy unavailable)".to_string();
                    }
                    self.sync_chrome();
                    Outcome::Consumed
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    if *direct {
                        return Outcome::Done(lazyos_ui::app::exit::OK);
                    }
                    self.mode = QrMode::Menu;
                    self.status = "Enter shows the code \u{00b7} Escape closes".to_string();
                    self.sync_chrome();
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
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
                    KeyCode::Esc => return Outcome::Done(lazyos_ui::app::exit::ESCAPE),
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
                    self.status = "Clipboard is empty \u{00b7} copy text or a URL first".to_string();
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

/// `lazyos menu qr` (`rofi-qr`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    menu_with_args(&[], terminal, design)
}

/// `lazyos menu qr [--wifi]`: opens in menu mode or directly in Wi-Fi QR view.
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
