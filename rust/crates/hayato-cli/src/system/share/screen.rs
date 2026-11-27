//! Ratatui Share Sheet UI (`ShareScreen`) for **Hayato Share**.
//!
//! - Single phase enum (`SharePhase`) for deterministic rendering and snapshots.
//! - Master-detail Cyber-HUD deck on wide terminals (`>= 104` cols) and clean
//!   single-pane layout on compact terminals (`80x24`).
//! - Zero `Color::Rgb` (100% `hayato_ui::Design` tokens), zero `Frame`, zero
//!   steady-state allocations in `draw()`.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hayato_ui::widgets::{draw_gauge, put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, KeyHint, ListAction,
    ListState, Outcome, QrCode, Row, Screen, TextStyle, draw_badge, draw_card, draw_qr,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use tachyonfx::{Effect, EffectTimer, Interpolation, fx};

use super::localsend::{LocalSendPeer, PeerScanner, push_to_peer};
use super::server::{ShareServer, default_downloads_dir, detect_lan_ip, human_size};
use super::{SharePayload, SharePreset, directory_size_bytes};

const LARGE_ZIP_BYTES: u64 = 100 * 1024 * 1024;
const BRAILLE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

enum SharePhase {
    Sheet,
    ActiveLan,
    DirectQr {
        qr: QrCode,
        title: String,
        text: String,
    },
    Receive,
    ConfirmLargeZip(Confirm),
}

#[derive(Clone, Debug)]
enum TargetKey {
    LanQr,
    DirectQr,
    WifiQr,
    Receive,
    LocalSend(LocalSendPeer),
    CopyClipboard,
    SaveNote,
    OpenDefault,
}

pub struct ShareScreen {
    payload: SharePayload,
    phase: SharePhase,
    server: Option<ShareServer>,
    qr: Option<QrCode>,
    scanner: Option<PeerScanner>,
    peers: Vec<LocalSendPeer>,
    list: ListState,
    keys: Vec<Option<TargetKey>>,
    badge: &'static str,
    badge_role: ColorRole,
    summary_title: String,
    summary_meta: String,
    status_line: String,
    countdown_line: String,
    progress_ratio: Option<f64>,
    progress_label: String,
    last_seq: u64,
    spinner_idx: usize,
    flash: Option<Effect>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    list_area: Cell<Rect>,
    confirm_area: Cell<Rect>,
    direct_qr_status: bool,
    lan_ip: String,
}

impl ShareScreen {
    pub fn new(payload: SharePayload, preset: SharePreset) -> Result<Self, String> {
        let lan_ip = detect_lan_ip();
        let (badge, badge_role, summary_title, summary_meta) = describe_payload(&payload);
        let scanner = Some(PeerScanner::start());

        let mut screen = Self {
            payload,
            phase: SharePhase::Sheet,
            server: None,
            qr: None,
            scanner,
            peers: Vec::new(),
            list: ListState::new(),
            keys: Vec::new(),
            badge,
            badge_role,
            summary_title,
            summary_meta,
            status_line: "Select a target · Enter shares immediately".to_string(),
            countdown_line: String::new(),
            progress_ratio: None,
            progress_label: String::new(),
            last_seq: u64::MAX,
            spinner_idx: 0,
            flash: None,
            header: Header::new("Hayato")
                .subtitle("Share")
                .status(format!("{lan_ip} · Ready")),
            keybar: Vec::new(),
            rows_height: Cell::new(14),
            list_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
            direct_qr_status: false,
            lan_ip,
        };

        screen.rebuild_rows();
        match preset {
            SharePreset::LanQr => {
                screen.activate_lan(false)?;
            }
            SharePreset::DirectQr => {
                screen.activate_direct_qr();
            }
            SharePreset::Receive => {
                screen.activate_receive()?;
            }
            SharePreset::Sheet => {
                // Pre-start LAN QR on wide screens for instant scan unless large folder
                if !screen.is_large_bundle()
                    && !matches!(screen.payload, SharePayload::Receive { .. })
                {
                    let _ = screen.ensure_lan_server();
                }
            }
        }
        screen.sync_chrome();
        Ok(screen)
    }

    #[cfg(test)]
    pub fn for_test(payload: SharePayload) -> Self {
        let (badge, badge_role, summary_title, summary_meta) = describe_payload(&payload);
        let mut screen = Self {
            payload,
            phase: SharePhase::Sheet,
            server: None,
            qr: QrCode::encode("http://192.168.1.42:49152/test/"),
            scanner: None,
            peers: Vec::new(),
            list: ListState::new(),
            keys: Vec::new(),
            badge,
            badge_role,
            summary_title,
            summary_meta,
            status_line: "Ready · Enter shares selected target".to_string(),
            countdown_line: "05:00 left · 0 done".to_string(),
            progress_ratio: None,
            progress_label: String::new(),
            last_seq: 0,
            spinner_idx: 0,
            flash: None,
            header: Header::new("Hayato")
                .subtitle("Share")
                .status("192.168.1.42 · Ready"),
            keybar: Vec::new(),
            rows_height: Cell::new(14),
            list_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
            direct_qr_status: false,
            lan_ip: "192.168.1.42".to_string(),
        };
        screen.rebuild_rows();
        screen.sync_chrome();
        screen
    }

    fn is_large_bundle(&self) -> bool {
        match &self.payload {
            SharePayload::File(p) if p.is_dir() => directory_size_bytes(p) > LARGE_ZIP_BYTES,
            SharePayload::Files(v) if v.len() > 1 => {
                v.iter().map(|p| directory_size_bytes(p)).sum::<u64>() > LARGE_ZIP_BYTES
            }
            _ => false,
        }
    }

    fn direct_qr_text(&self) -> Option<&str> {
        match &self.payload {
            SharePayload::Url { url, .. } if url.len() <= 1200 => Some(url.as_str()),
            SharePayload::Text { content, .. } if content.len() <= 1200 => Some(content.as_str()),
            _ => None,
        }
    }

    fn ensure_lan_server(&mut self) -> Result<(), String> {
        if self.server.is_some() {
            return Ok(());
        }
        let srv = ShareServer::start(&self.payload, default_downloads_dir())?;
        self.qr = QrCode::encode(&srv.url);
        self.server = Some(srv);
        self.poll_telemetry();
        Ok(())
    }

    fn activate_lan(&mut self, check_large: bool) -> Result<(), String> {
        if check_large && self.server.is_none() && self.is_large_bundle() {
            self.phase = SharePhase::ConfirmLargeZip(
                Confirm::new("Archive folder over 100 MiB?", "Zip & Share").with_detail(
                    "Bundling a large folder into an ephemeral ZIP may take a moment.",
                ),
            );
            self.sync_chrome();
            return Ok(());
        }
        self.ensure_lan_server()?;
        self.phase = SharePhase::ActiveLan;
        self.sync_chrome();
        Ok(())
    }

    fn activate_receive(&mut self) -> Result<(), String> {
        let dest = match &self.payload {
            SharePayload::Receive { dest_dir } => dest_dir.clone(),
            _ => default_downloads_dir(),
        };
        let recv_payload = SharePayload::Receive { dest_dir: dest };
        let srv = ShareServer::start(&recv_payload, default_downloads_dir())?;
        self.qr = QrCode::encode(&srv.url);
        self.server = Some(srv);
        self.phase = SharePhase::Receive;
        self.poll_telemetry();
        self.sync_chrome();
        Ok(())
    }

    fn activate_direct_qr(&mut self) {
        if let Some(text) = self.direct_qr_text().map(str::to_string)
            && let Some(qr) = QrCode::encode(&text)
        {
            self.phase = SharePhase::DirectQr {
                qr,
                title: "Instant Offline QR".to_string(),
                text,
            };
            self.status_line = "Offline QR ready · s saves PNG · c copies".to_string();
            self.sync_chrome();
        }
    }

    fn activate_wifi_qr(&mut self) {
        let Some((payload, title)) = crate::system::qr::payload_wifi() else {
            self.status_line = "Not connected to a Wi-Fi network".to_string();
            return;
        };
        if let Some(qr) = QrCode::encode(&payload) {
            self.phase = SharePhase::DirectQr {
                qr,
                title,
                text: payload,
            };
            self.status_line = "Scan with phone camera to join Wi-Fi · s saves PNG".to_string();
            self.sync_chrome();
        } else {
            self.status_line = "Could not encode Wi-Fi QR".to_string();
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys: Vec<Option<TargetKey>> = Vec::new();

        rows.push(Row::heading("NEARBY & MOBILE"));
        keys.push(None);

        if self.direct_qr_text().is_some() {
            rows.push(
                Row::new("Instant QR Code (Offline)")
                    .glyph(Glyph::Qr)
                    .detail("no Wi-Fi needed · scan directly"),
            );
            keys.push(Some(TargetKey::DirectQr));
        }

        if !matches!(self.payload, SharePayload::Receive { .. }) {
            rows.push(
                Row::new("Send via Phone QR (Wi-Fi)")
                    .glyph(Glyph::Wifi)
                    .detail("zero-app · themed mobile web card"),
            );
            keys.push(Some(TargetKey::LanQr));
        }

        rows.push(
            Row::new("Receive from Phone")
                .glyph(Glyph::Download)
                .detail("upload files \u{2192} ~/Downloads or clipboard"),
        );
        keys.push(Some(TargetKey::Receive));

        rows.push(
            Row::new("Share Current Wi-Fi (QR)")
                .glyph(Glyph::Qr)
                .detail("scan with phone camera to join Wi-Fi"),
        );
        keys.push(Some(TargetKey::WifiQr));

        for peer in &self.peers {
            rows.push(
                Row::new(peer.alias.clone())
                    .glyph(Glyph::Globe)
                    .detail(format!("LocalSend · {} · {}", peer.device_model, peer.ip)),
            );
            keys.push(Some(TargetKey::LocalSend(peer.clone())));
        }

        if !matches!(self.payload, SharePayload::Receive { .. }) {
            rows.push(Row::heading("QUICK & LOCAL"));
            keys.push(None);

            rows.push(
                Row::new("Copy to Clipboard")
                    .glyph(Glyph::Clipboard)
                    .detail("wl-copy"),
            );
            keys.push(Some(TargetKey::CopyClipboard));

            if matches!(
                self.payload,
                SharePayload::Text { .. } | SharePayload::Url { .. }
            ) {
                rows.push(
                    Row::new("Save to Notes Inbox")
                        .glyph(Glyph::File)
                        .detail("~/Documents/Notes/Inbox.md"),
                );
                keys.push(Some(TargetKey::SaveNote));
            }

            if matches!(
                self.payload,
                SharePayload::Url { .. } | SharePayload::File(_) | SharePayload::Image { .. }
            ) {
                rows.push(
                    Row::new("Open in Default App")
                        .glyph(Glyph::Link)
                        .detail("hayato open"),
                );
                keys.push(Some(TargetKey::OpenDefault));
            }
        }

        self.keys = keys;
        self.list.update_rows(rows);
    }

    fn sync_chrome(&mut self) {
        let status = match &self.phase {
            SharePhase::Sheet => format!("{} · Ready", self.lan_ip),
            SharePhase::ActiveLan => format!("{} · {}", self.lan_ip, self.countdown_line),
            SharePhase::DirectQr { .. } => "Offline QR".to_string(),
            SharePhase::Receive => format!("Receive · {}", self.countdown_line),
            SharePhase::ConfirmLargeZip(_) => "Confirm large archive".to_string(),
        };
        self.header = Header::new("Hayato").subtitle("Share").status(status);
        self.keybar = match &self.phase {
            SharePhase::Sheet => vec![
                KeyHint::new("Enter", "share"),
                KeyHint::new("v", "receive"),
                KeyHint::new("c", "copy URL"),
                KeyHint::new("r", "+5m"),
                KeyHint::new("Esc", "close"),
            ],
            SharePhase::ActiveLan | SharePhase::Receive => vec![
                KeyHint::new("c", "copy URL"),
                KeyHint::new("r", "+5m"),
                KeyHint::new("Esc", "back"),
            ],
            SharePhase::DirectQr { .. } => vec![
                KeyHint::new("s", "save PNG"),
                KeyHint::new("c", "copy"),
                KeyHint::new("Esc", "back"),
            ],
            SharePhase::ConfirmLargeZip(_) => vec![
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
        };
    }

    fn poll_telemetry(&mut self) {
        if let Some(scanner) = &self.scanner {
            let discovered = scanner.snapshot();
            if discovered.len() != self.peers.len() {
                self.peers = discovered;
                self.rebuild_rows();
            }
        }

        if let Some(srv) = &self.server {
            let snap = srv.snapshot();
            let left = snap
                .expires_at
                .saturating_duration_since(Instant::now())
                .as_secs();
            let mins = left / 60;
            let secs = left % 60;
            self.countdown_line = if snap.expired {
                format!("EXPIRED · {} done", snap.transfers)
            } else {
                format!("{mins:02}:{secs:02} left · {} done", snap.transfers)
            };
            if snap.bytes_total > 0 && !snap.active_label.is_empty() {
                let ratio = (snap.bytes_done as f64 / snap.bytes_total as f64).clamp(0.0, 1.0);
                self.progress_ratio = Some(ratio);
                self.progress_label = format!(
                    "{} · {} / {}",
                    snap.active_label,
                    human_size(snap.bytes_done),
                    human_size(snap.bytes_total)
                );
            } else {
                self.progress_ratio = None;
                self.progress_label.clear();
            }
            if snap.seq != self.last_seq {
                if self.last_seq != u64::MAX && snap.transfers > 0 {
                    self.flash = Some(fx::fade_from_fg(
                        ratatui::style::Color::Reset,
                        EffectTimer::from_ms(220, Interpolation::QuadOut),
                    ));
                }
                self.last_seq = snap.seq;
                self.status_line = snap.last_event;
                self.sync_chrome();
            }
        }
    }

    fn copy_text(&mut self, text: &str, label: &str) {
        let ok = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                child.wait()
            })
            .map(|s| s.success())
            .unwrap_or(false);
        self.status_line = if ok {
            format!("Copied {label} to clipboard")
        } else {
            format!("Could not copy {label} (wl-copy failed)")
        };
    }

    fn save_to_notes_inbox(&mut self) {
        let body = match &self.payload {
            SharePayload::Text { content, .. } => content.clone(),
            SharePayload::Url { url, .. } => url.clone(),
            _ => return,
        };
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let dir = PathBuf::from(home).join("Documents").join("Notes");
        let _ = std::fs::create_dir_all(&dir);
        let inbox = dir.join("Inbox.md");
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let entry = format!("\n- [{stamp}] {body}\n");
        let ok = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inbox)
            .and_then(|mut f| f.write_all(entry.as_bytes()))
            .is_ok();
        self.status_line = if ok {
            "Saved to ~/Documents/Notes/Inbox.md".to_string()
        } else {
            "Could not write ~/Documents/Notes/Inbox.md".to_string()
        };
    }

    fn open_in_default(&mut self) {
        match &self.payload {
            SharePayload::Url { url, .. } => {
                crate::system::spawn_detached_argv("hayato-browser", &[url]);
                self.status_line = "Opened link in browser".to_string();
            }
            SharePayload::File(p) | SharePayload::Image { path: p, .. } => {
                let s = p.to_string_lossy().into_owned();
                crate::system::spawn_detached_argv("hayato", &["open", &s]);
                self.status_line = format!("Opened {}", p.display());
            }
            _ => {}
        }
    }

    fn activate_selected(&mut self) {
        let Some(idx) = self.list.selected_index() else {
            return;
        };
        let Some(Some(target)) = self.keys.get(idx).cloned() else {
            return;
        };
        match target {
            TargetKey::LanQr => {
                if let Err(e) = self.activate_lan(true) {
                    self.status_line = e;
                }
            }
            TargetKey::DirectQr => self.activate_direct_qr(),
            TargetKey::WifiQr => self.activate_wifi_qr(),
            TargetKey::Receive => {
                if let Err(e) = self.activate_receive() {
                    self.status_line = e;
                }
            }
            TargetKey::LocalSend(peer) => {
                self.status_line = format!("Pushing to {}…", peer.alias);
                match push_to_peer(&peer, &self.payload) {
                    Ok(msg) => self.status_line = msg,
                    Err(err) => self.status_line = err,
                }
            }
            TargetKey::CopyClipboard => match &self.payload {
                SharePayload::Text { content, .. } => {
                    let c = content.clone();
                    self.copy_text(&c, "text");
                }
                SharePayload::Url { url, .. } => {
                    let u = url.clone();
                    self.copy_text(&u, "link");
                }
                SharePayload::File(p) | SharePayload::Image { path: p, .. } => {
                    let s = p.display().to_string();
                    self.copy_text(&s, "file path");
                }
                SharePayload::Files(v) => {
                    let joined = v
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.copy_text(&joined, "file paths");
                }
                SharePayload::Receive { .. } => {}
            },
            TargetKey::SaveNote => self.save_to_notes_inbox(),
            TargetKey::OpenDefault => self.open_in_default(),
        }
    }

    fn draw_qr_panel(&self, ctx: &mut BodyCtx, area: Rect, title: &str) {
        let inner = draw_card(ctx.buf, area, ctx.design, Some(title), false);
        if inner.width < 16 || inner.height < 6 {
            return;
        }
        let footer_y = inner.bottom().saturating_sub(1);
        let url_y = footer_y.saturating_sub(2);
        let bar_y = footer_y.saturating_sub(1);

        if let Some(qr) = &self.qr {
            let qr_h = url_y.saturating_sub(inner.y);
            let qr_area = Rect::new(inner.x, inner.y, inner.width, qr_h);
            if qr_h >= 2 {
                crate::system::draw_qr_fit(
                    ctx.buf,
                    qr_area,
                    ctx.design,
                    qr,
                    "QR too large for this pane · press Enter for full view",
                );
            }
        }
        if let Some(srv) = &self.server {
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                url_y,
                &srv.url,
                inner.width,
                ctx.design.style(TextStyle::Code),
            );
        }
        if let Some(ratio) = self.progress_ratio {
            draw_gauge(
                ctx.buf,
                Rect::new(inner.x, bar_y, inner.width, 1),
                ctx.design,
                ratio,
                &self.progress_label,
            );
        } else {
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                bar_y,
                &self.countdown_line,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );
        }
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            footer_y,
            &self.status_line,
            inner.width,
            ctx.design.style(TextStyle::Body),
        );
    }
}

impl Screen for ShareScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((1060, 620))
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.phase {
            SharePhase::Sheet => "share.sheet",
            SharePhase::ActiveLan => "share.lan",
            SharePhase::DirectQr { .. } => "share.qr",
            SharePhase::Receive => "share.receive",
            SharePhase::ConfirmLargeZip(_) => "share.confirm_zip",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        let scanning = self.scanner.as_ref().is_some_and(PeerScanner::is_scanning);
        if self.server.is_some() || scanning || self.flash.is_some() {
            Some(Duration::from_millis(120))
        } else {
            None
        }
    }

    fn tick(&mut self) -> Outcome {
        self.spinner_idx = (self.spinner_idx + 1) % BRAILLE.len();
        self.poll_telemetry();
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let SharePhase::ConfirmLargeZip(confirm) = &mut self.phase {
            self.confirm_area.set(ctx.area);
            confirm.render(ctx.area, ctx.buf, ctx.design);
            return;
        }

        match &self.phase {
            SharePhase::DirectQr { qr, title, text } => {
                let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some(title.as_str()), true);
                if inner.height >= 6 {
                    let footer_y = inner.bottom().saturating_sub(1);
                    let qr_area = Rect::new(
                        inner.x,
                        inner.y,
                        inner.width,
                        footer_y.saturating_sub(inner.y + 1),
                    );
                    crate::system::draw_qr_fit(
                        ctx.buf,
                        qr_area,
                        ctx.design,
                        qr,
                        "QR too large for window · widen window or press Esc",
                    );
                    let display_footer = if self.direct_qr_status || title.starts_with("Wi-Fi") {
                        self.status_line.as_str()
                    } else {
                        text.as_str()
                    };
                    let _ = put_trunc(
                        ctx.buf,
                        inner.x,
                        footer_y,
                        display_footer,
                        inner.width,
                        ctx.design.style(TextStyle::Code),
                    );
                }
                return;
            }
            SharePhase::ActiveLan | SharePhase::Receive => {
                let title = if matches!(self.phase, SharePhase::Receive) {
                    "Receive from Phone (Wi-Fi)"
                } else {
                    "Send via Phone QR (Wi-Fi)"
                };
                self.draw_qr_panel(ctx, ctx.area, title);
                return;
            }
            SharePhase::Sheet | SharePhase::ConfirmLargeZip(_) => {}
        }

        let wide = ctx.area.width >= crate::system::DUAL_PANE_MIN_WIDTH;
        let (left_area, right_area) = if wide {
            let left_w = (ctx.area.width * 54) / 100;
            let gap = 1;
            let right_w = ctx.area.width.saturating_sub(left_w + gap);
            (
                Rect::new(ctx.area.x, ctx.area.y, left_w, ctx.area.height),
                Some(Rect::new(
                    ctx.area.x + left_w + gap,
                    ctx.area.y,
                    right_w,
                    ctx.area.height,
                )),
            )
        } else {
            (ctx.area, None)
        };

        let inner = draw_card(ctx.buf, left_area, ctx.design, Some("Share Targets"), true);
        if inner.width >= 16 && inner.height >= 6 {
            let x = draw_badge(
                ctx.buf,
                inner.x,
                inner.y,
                ctx.design,
                self.badge,
                self.badge_role,
            );
            let _ = put_trunc(
                ctx.buf,
                x + 2,
                inner.y,
                &self.summary_title,
                inner.right().saturating_sub(x + 2),
                ctx.design.style(TextStyle::Title),
            );
            let _ = put_trunc(
                ctx.buf,
                inner.x,
                inner.y + 1,
                &self.summary_meta,
                inner.width,
                ctx.design.style(TextStyle::Dim),
            );

            let footer_y = inner.bottom().saturating_sub(1);
            let list_y = inner.y + 3;
            let list_h = footer_y.saturating_sub(list_y);
            self.rows_height.set(list_h);
            let list_rect = Rect::new(inner.x, list_y, inner.width, list_h);
            self.list_area.set(list_rect);
            if list_h > 0 {
                self.list.render(list_rect, ctx.buf, ctx.design, true);
            }

            let scanning = self.scanner.as_ref().is_some_and(PeerScanner::is_scanning);
            if scanning {
                let spin = BRAILLE[self.spinner_idx];
                let _ = put(
                    ctx.buf,
                    inner.x,
                    footer_y,
                    spin,
                    2,
                    ratatui::style::Style::default().fg(ctx.design.color(ColorRole::Accent)),
                );
                let _ = put_trunc(
                    ctx.buf,
                    inner.x + 2,
                    footer_y,
                    "Scanning LAN for LocalSend peers\u{2026}",
                    inner.width.saturating_sub(2),
                    ctx.design.style(TextStyle::Dim),
                );
            } else {
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    footer_y,
                    &self.status_line,
                    inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
            }
        }

        if let Some(right) = right_area {
            self.draw_qr_panel(ctx, right, "Phone QR · Live Deck");
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if let SharePhase::ConfirmLargeZip(confirm) = &mut self.phase {
            match confirm.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.phase = SharePhase::Sheet;
                    if let Err(e) = self.activate_lan(false) {
                        self.status_line = e;
                    }
                    return Outcome::Consumed;
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = SharePhase::Sheet;
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                ConfirmAction::None => return Outcome::Consumed,
            }
        }

        match &self.phase {
            SharePhase::ActiveLan | SharePhase::Receive | SharePhase::DirectQr { .. } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.direct_qr_status = false;
                        self.phase = SharePhase::Sheet;
                        self.sync_chrome();
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('s') => {
                        if let SharePhase::DirectQr { title, text, .. } = &self.phase {
                            let prefix = if title.starts_with("Wi-Fi") {
                                "wifi-qr"
                            } else {
                                "qr"
                            };
                            match crate::system::qr::save_qr_png(text, prefix) {
                                Ok(p) => {
                                    self.direct_qr_status = true;
                                    self.status_line = format!(
                                        "Saved {}",
                                        p.file_name().unwrap_or_default().to_string_lossy()
                                    );
                                }
                                Err(e) => {
                                    self.direct_qr_status = true;
                                    self.status_line = e;
                                }
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('c') => {
                        if let SharePhase::DirectQr { text, .. } = &self.phase {
                            let t = text.clone();
                            self.direct_qr_status = true;
                            self.copy_text(&t, "payload");
                        } else if let Some(srv) = &self.server {
                            let u = srv.url.clone();
                            self.copy_text(&u, "Share URL");
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('r') => {
                        if let Some(srv) = &self.server {
                            srv.extend_ttl(Duration::from_secs(300));
                            self.poll_telemetry();
                        }
                        return Outcome::Consumed;
                    }
                    _ => return Outcome::Continue,
                }
            }
            SharePhase::Sheet | SharePhase::ConfirmLargeZip(_) => {}
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Char('v') => {
                if let Err(e) = self.activate_receive() {
                    self.status_line = e;
                }
                return Outcome::Consumed;
            }
            KeyCode::Char('c') => {
                if let Some(srv) = &self.server {
                    let u = srv.url.clone();
                    self.copy_text(&u, "Share URL");
                }
                return Outcome::Consumed;
            }
            KeyCode::Char('r') => {
                if let Some(srv) = &self.server {
                    srv.extend_ttl(Duration::from_secs(300));
                    self.poll_telemetry();
                }
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => {
                self.activate_selected();
                Outcome::Consumed
            }
            ListAction::Moved => Outcome::Consumed,
            ListAction::Ignored => Outcome::Continue,
        }
    }

    fn hit(&self, _area: Rect, col: u16, row: u16, design: &Design) -> Option<usize> {
        if let SharePhase::ConfirmLargeZip(confirm) = &self.phase {
            return confirm.hit(self.confirm_area.get(), design, col, row);
        }
        if matches!(self.phase, SharePhase::Sheet) {
            return self.list.hit_row(self.list_area.get(), col, row, design);
        }
        None
    }

    fn click(&mut self, index: Option<usize>) -> Outcome {
        let Some(row) = index else {
            return Outcome::Continue;
        };
        if let SharePhase::ConfirmLargeZip(confirm) = &mut self.phase {
            match confirm.click(row) {
                ConfirmAction::Chosen(1) => {
                    self.phase = SharePhase::Sheet;
                    if let Err(e) = self.activate_lan(false) {
                        self.status_line = e;
                    }
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = SharePhase::Sheet;
                    self.sync_chrome();
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if matches!(self.phase, SharePhase::Sheet) {
            self.list.set_cursor(row);
            self.activate_selected();
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        match &self.phase {
            SharePhase::ConfirmLargeZip(_) => None,
            _ => Some(&self.keybar),
        }
    }
}

fn describe_payload(payload: &SharePayload) -> (&'static str, ColorRole, String, String) {
    match payload {
        SharePayload::File(p) => {
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            let is_dir = p.is_dir();
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            let badge = if is_dir { "FOLDER" } else { "FILE" };
            let meta = if is_dir {
                format!("Will zip on demand · {}", p.display())
            } else {
                format!("{} · {}", human_size(size), p.display())
            };
            (badge, ColorRole::Accent, name, meta)
        }
        SharePayload::Files(v) => (
            "BUNDLE",
            ColorRole::Accent,
            format!("{} items", v.len()),
            "Auto-zipped bundle for mobile transfer".to_string(),
        ),
        SharePayload::Image { path, source, .. } => {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("image.png")
                .to_string();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            (
                "IMAGE",
                ColorRole::AccentAlt,
                name,
                format!("{} · from {}", human_size(size), source.label()),
            )
        }
        SharePayload::Text {
            content,
            label,
            source,
        } => {
            let preview: String = content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(48)
                .collect();
            let title = label.clone().filter(|s| !s.is_empty()).unwrap_or(preview);
            (
                "TEXT",
                ColorRole::Accent,
                title,
                format!(
                    "{} · from {}",
                    human_size(content.len() as u64),
                    source.label()
                ),
            )
        }
        SharePayload::Url { url, title, source } => (
            "LINK",
            ColorRole::AccentAlt,
            title.clone().unwrap_or_else(|| url.clone()),
            format!("{url} · from {}", source.label()),
        ),
        SharePayload::Receive { dest_dir } => (
            "RECV",
            ColorRole::Ok,
            "Receive from Phone".to_string(),
            format!("Saves uploads \u{2192} {}", dest_dir.display()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::share::ShareSource;
    use hayato_ui::{App, Header};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn share_screen_renders_at_compact_and_wide_sizes() {
        for (w, h) in [(80, 24), (152, 45)] {
            let mut screen = ShareScreen::for_test(SharePayload::Url {
                url: "https://hayato.dev/docs".into(),
                title: Some("Hayato Docs".into()),
                source: ShareSource::Lens,
            });
            let mut app = App::new(Header::new("Hayato"))
                .design(Design::solarized().with_reduced_motion(true))
                .screen(&mut screen);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| app.render(f)).unwrap();
            let rendered = hayato_ui::screen(term.backend().buffer());
            assert!(rendered.contains("Share Targets"), "{w}x{h}");
            assert!(rendered.contains("Hayato Docs"), "{w}x{h}");
        }
    }
}
