//! Master-detail Ratatui UI (`ConnectScreen`) for **Hayato Connect**.
//!
//! Single `ConnectPhase` state machine implementing `specs/hayato-connect.md`:
//! - `[1] Overview`, `[2] Transfer`, `[3] Remote`, `[4] Alerts`
//! - Live right deck on `width >= DUAL_PANE_MIN_WIDTH (84)`
//! - Full-body `AuthorizePeer` (6-digit SAS verification, Cancel-first)
//! - Full-body `PairQr`, inline `ComposePush`, and destructive `ConfirmAction`
//! - Zero resident daemons, zero `Color::Rgb`, zero `Alt+` TUI chords.

use std::cell::Cell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use hayato_ui::widgets::{ActionItem, ActionStrip, ActionStripEvent, draw_gauge, put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, Input, KeyHint, ListAction,
    ListState, Outcome, QrCode, Row, Screen, TextStyle, draw_badge, draw_card,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use super::server::{ConnectEvent, ConnectServer, ConnectTelemetry};
use super::trust::{TrustStore, TrustedDevice};
use crate::system::share::SharePayload;
use crate::system::share::localsend::{LocalSendPeer, PeerScanner, push_to_peer};
use crate::system::share::server::{default_downloads_dir, detect_lan_ip, human_size};

const HIT_BTN_QR: usize = 30_000;
const HIT_BTN_COPY: usize = 30_001;
const HIT_STRIP_BASE: usize = 31_000;
const MAX_ACTIVITY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectSection {
    Overview,
    Transfer,
    Remote,
    Alerts,
}

impl ConnectSection {
    fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Transfer => 1,
            Self::Remote => 2,
            Self::Alerts => 3,
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "overview" => Some(Self::Overview),
            "transfer" => Some(Self::Transfer),
            "remote" => Some(Self::Remote),
            "alerts" => Some(Self::Alerts),
            _ => None,
        }
    }

    fn card_title(self) -> &'static str {
        match self {
            Self::Overview => "Devices & Live Session",
            Self::Transfer => "Transfer & Clipboard Bridge",
            Self::Remote => "Remote Media & Phone Camera",
            Self::Alerts => "Notification Bridge (Scoped)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushKind {
    Link,
    ClipboardText,
}

#[derive(Clone, Debug)]
pub enum PendingConfirm {
    UnpairDevice { id: String, label: String },
    StopActiveAndClose,
}

pub enum ConnectRequest {
    PickFileToSend,
    PickSyncFolder,
}

#[derive(Clone)]
enum RowAction {
    SendFile,
    Compose(PushKind),
    PushClipboardNow,
    ToggleAutoClip,
    ShowPairQr,
    RescanPeers,
    PushClipToPeer(LocalSendPeer),
    UnpairDevice { id: String, label: String },
    OpenDownloads,
    CopyUrl,
    PickSyncFolder,
    ScanSyncFolder,
    MediaCmd(&'static str),
    ToggleMediaMirror,
    TogglePhoneCam,
    FlipCamLens,
    ToggleCamViewfinder,
    TogglePhoneMic,
    SnapCamPhoto,
    ToggleAlertsMirror,
    SendTestAlert,
    RingPhone,
}

enum ConnectPhase {
    Workspace {
        section: ConnectSection,
        strip: ActionStrip,
        strip_focused: bool,
    },
    PairQr {
        qr: QrCode,
        url: String,
    },
    AuthorizePeer {
        req_id: String,
        device_label: String,
        remote_ip: String,
        sas_code: String,
        confirm: Confirm,
    },
    ComposePush {
        kind: PushKind,
        input: Input,
    },
    ConfirmAction {
        action: PendingConfirm,
        confirm: Confirm,
    },
}

pub struct ConnectScreen {
    phase: ConnectPhase,
    server: Option<ConnectServer>,
    ev_rx: Option<Receiver<ConnectEvent>>,
    scanner: Option<PeerScanner>,
    qr: Option<QrCode>,
    pair_url: String,
    spki_fp: String,
    lan_ip: String,
    port: u16,
    trusted: Vec<TrustedDevice>,
    peers: Vec<LocalSendPeer>,
    telemetry: ConnectTelemetry,
    activity: VecDeque<String>,
    alerts_log: VecDeque<String>,
    list: ListState,
    actions: Vec<Option<RowAction>>,
    status_line: String,
    deck_title: String,
    cam_label_line: String,
    auto_clip: bool,
    media_mirror: bool,
    alerts_mirror: bool,
    cam_facing: String,
    sync_folder: PathBuf,
    poll_ticks: u8,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    list_area: Cell<Rect>,
    strip_area: Cell<Rect>,
    confirm_area: Cell<Rect>,
    deck_btns: Cell<[Rect; 2]>,
    pub pending_request: Option<ConnectRequest>,
}

fn make_section_strip(active: ConnectSection) -> ActionStrip {
    ActionStrip::new(vec![
        ActionItem::new("overview", "1", "Overview").glyph(Glyph::Home),
        ActionItem::new("transfer", "2", "Transfer").glyph(Glyph::Sync),
        ActionItem::new("remote", "3", "Remote").glyph(Glyph::Play),
        ActionItem::new("alerts", "4", "Alerts").glyph(Glyph::Bell),
        ActionItem::new("qr", "^Q", "Pair QR").glyph(Glyph::Qr),
    ])
    .with_active(Some(active.index()))
}

impl ConnectScreen {
    pub fn new() -> Result<Self, String> {
        let trust = TrustStore::load_default();
        let trusted = trust.devices().to_vec();
        let (server, ev_rx) = ConnectServer::start(trust, None, default_downloads_dir())?;
        let qr = QrCode::encode(&server.pair_url);
        let pair_url = server.pair_url.clone();
        let spki_fp = server.spki_fp.clone();
        let lan_ip = server.ip.clone();
        let port = server.port;
        let auto_clip = server.auto_clip();
        let media_mirror = server.media_mirror();
        let alerts_mirror = server.alerts_mirror();
        let sync_folder = server.sync_root();
        let telemetry = server.snapshot();
        let scanner = Some(PeerScanner::start());

        let mut screen = Self {
            phase: ConnectPhase::Workspace {
                section: ConnectSection::Overview,
                strip: make_section_strip(ConnectSection::Overview),
                strip_focused: false,
            },
            server: Some(server),
            ev_rx: Some(ev_rx),
            scanner,
            qr,
            pair_url,
            spki_fp,
            lan_ip: lan_ip.clone(),
            port,
            trusted,
            peers: Vec::new(),
            telemetry,
            activity: VecDeque::with_capacity(MAX_ACTIVITY),
            alerts_log: VecDeque::with_capacity(8),
            list: ListState::new(),
            actions: Vec::new(),
            status_line: "Ready · scan QR on phone or Ctrl+S to send".to_string(),
            deck_title: String::from("Pairing Deck"),
            cam_label_line: String::new(),
            auto_clip,
            media_mirror,
            alerts_mirror,
            cam_facing: String::from("environment"),
            sync_folder,
            poll_ticks: 0,
            header: Header::new("Hayato").subtitle("Connect"),
            keybar: Vec::new(),
            rows_height: Cell::new(14),
            list_area: Cell::new(Rect::default()),
            strip_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
            deck_btns: Cell::new([Rect::default(); 2]),
            pending_request: None,
        };
        screen.rebuild_rows();
        screen.sync_chrome();
        Ok(screen)
    }

    #[allow(dead_code)]
    pub fn for_test() -> Self {
        let trusted = vec![TrustedDevice {
            id: "dev-pixel8".into(),
            name: "Pixel 8".into(),
            user_agent: "Android 15 · Chrome".into(),
            secret_hash: "0".repeat(64),
            protocol: "webapp".into(),
            cert_fp: String::new(),
            paired_at: 100,
            last_seen: 200,
            capabilities: Default::default(),
        }];
        let pair_url = "https://192.168.1.42:51420/?pair=demo123#fp=demo_fp".to_string();
        let qr = QrCode::encode(&pair_url).or_else(|| Some(crate::system::fallback_qr()));
        let telemetry = ConnectTelemetry {
            last_clip_preview: "cargo test -p hayato-cli".to_string(),
            ..ConnectTelemetry::default()
        };
        let mut screen = Self {
            phase: ConnectPhase::Workspace {
                section: ConnectSection::Overview,
                strip: make_section_strip(ConnectSection::Overview),
                strip_focused: false,
            },
            server: None,
            ev_rx: None,
            scanner: None,
            qr,
            pair_url,
            spki_fp: "demo_fp_abc123".into(),
            lan_ip: "192.168.1.42".into(),
            port: 51420,
            trusted,
            peers: vec![LocalSendPeer {
                alias: "Bruno iPad".into(),
                fingerprint: "ipad-1".into(),
                ip: "192.168.1.58".into(),
                port: 53317,
                protocol: "https".into(),
                device_model: "iPad".into(),
            }],
            telemetry,
            activity: VecDeque::from([
                "Received IMG_4021.jpg -> ~/Downloads".to_string(),
                "Synced clipboard (24 B)".to_string(),
            ]),
            alerts_log: VecDeque::from(["[Signal] Alice: Hey — check the PDF".to_string()]),
            list: ListState::new(),
            actions: Vec::new(),
            status_line: "Ready · 1 trusted device".to_string(),
            deck_title: String::from("Pairing Deck"),
            cam_label_line: String::new(),
            auto_clip: true,
            media_mirror: true,
            alerts_mirror: true,
            cam_facing: String::from("environment"),
            sync_folder: PathBuf::from("/home/user/Documents/Notes"),
            poll_ticks: 0,
            header: Header::new("Hayato").subtitle("Connect"),
            keybar: Vec::new(),
            rows_height: Cell::new(14),
            list_area: Cell::new(Rect::default()),
            strip_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
            deck_btns: Cell::new([Rect::default(); 2]),
            pending_request: None,
        };
        screen.rebuild_rows();
        screen.sync_chrome();
        screen
    }

    pub fn send_path(&mut self, path: PathBuf) {
        let payload = SharePayload::from_path(path.clone());
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("selection")
            .to_string();
        if let Some(srv) = &self.server {
            match srv.offer_payload(&payload) {
                Ok(item) => {
                    let msg = format!(
                        "Ready to download: {} ({})",
                        item.title,
                        human_size(item.size)
                    );
                    self.push_activity(msg.clone());
                    self.status_line = msg;
                }
                Err(e) => {
                    self.status_line = e;
                }
            }
        } else {
            self.push_activity(format!("Selected {name}"));
        }
        self.rebuild_rows();
        self.sync_chrome();
    }

    fn push_activity(&mut self, line: String) {
        if self.activity.len() >= MAX_ACTIVITY {
            self.activity.pop_back();
        }
        self.activity.push_front(line);
    }

    fn active_section(&self) -> ConnectSection {
        match &self.phase {
            ConnectPhase::Workspace { section, .. } => *section,
            _ => ConnectSection::Overview,
        }
    }

    fn set_section(&mut self, target: ConnectSection) {
        if let ConnectPhase::Workspace {
            section,
            strip,
            strip_focused,
        } = &mut self.phase
        {
            *section = target;
            *strip_focused = false;
            strip.set_focused(false);
            strip.set_active(Some(target.index()));
        } else {
            self.phase = ConnectPhase::Workspace {
                section: target,
                strip: make_section_strip(target),
                strip_focused: false,
            };
        }
        self.list.set_cursor(0);
        self.rebuild_rows();
        self.sync_chrome();
    }

    fn poll_events(&mut self) {
        if let Some(scanner) = &self.scanner {
            let found = scanner.snapshot();
            if found != self.peers {
                self.peers = found;
                self.rebuild_rows();
            }
        }
        if let Some(srv) = &self.server {
            self.poll_ticks = self.poll_ticks.wrapping_add(1);
            if self.poll_ticks.is_multiple_of(6) {
                let _ = srv.poll_and_broadcast_media();
            }
            let tel = srv.snapshot();
            if tel.seq != self.telemetry.seq {
                self.telemetry = tel;
                self.trusted = srv.trust_devices();
                self.rebuild_rows();
                self.sync_chrome();
            }
        }
        let mut collected = Vec::new();
        if let Some(rx) = &self.ev_rx {
            while let Ok(ev) = rx.try_recv() {
                collected.push(ev);
            }
        }
        for ev in collected {
            match ev {
                ConnectEvent::AuthRequested {
                    req_id,
                    device_label,
                    remote_ip,
                    sas_code,
                } => {
                    let confirm = Confirm::new("Authorize New Device?", format!("SAS: {sas_code}"))
                        .with_detail(format!(
                            "{device_label} ({remote_ip})\nGrants LAN access while Connect is open."
                        ))
                        .with_options(&["Deny", "Trust & Pair"]);
                    self.phase = ConnectPhase::AuthorizePeer {
                        req_id,
                        device_label,
                        remote_ip,
                        sas_code,
                        confirm,
                    };
                    self.sync_chrome();
                }
                ConnectEvent::PeerConnected(p) => {
                    let msg = format!("Connected: {} ({})", p.name, p.remote_ip);
                    self.push_activity(msg.clone());
                    self.status_line = msg;
                    if let Some(srv) = &self.server {
                        self.trusted = srv.trust_devices();
                    }
                    self.rebuild_rows();
                    self.sync_chrome();
                }
                ConnectEvent::PeerDisconnected { name, .. } => {
                    self.push_activity(format!("Disconnected: {name}"));
                    self.rebuild_rows();
                    self.sync_chrome();
                }
                ConnectEvent::ClipboardReceived { preview, bytes } => {
                    let msg = format!("Clipboard from phone ({bytes} B): {preview}");
                    self.push_activity(msg.clone());
                    self.status_line = msg;
                    self.rebuild_rows();
                }
                ConnectEvent::LinkReceived { url, from } => {
                    let msg = format!("Opened link from {from}: {url}");
                    self.push_activity(msg.clone());
                    self.status_line = msg;
                    self.rebuild_rows();
                }
                ConnectEvent::NoteReceived { from, .. } => {
                    let msg = format!("Saved note from {from} -> Notes Inbox");
                    self.push_activity(msg.clone());
                    self.status_line = msg;
                    self.rebuild_rows();
                }
                ConnectEvent::TransferFinished { summary } => {
                    self.push_activity(summary.clone());
                    self.status_line = summary;
                    self.rebuild_rows();
                }
                ConnectEvent::AlertReceived { title, body, from } => {
                    let line = format!("[{from}] {title}: {body}");
                    if self.alerts_log.len() >= 8 {
                        self.alerts_log.pop_back();
                    }
                    self.alerts_log.push_front(line.clone());
                    self.push_activity(line);
                    self.rebuild_rows();
                }
                ConnectEvent::MediaCmdExecuted { cmd, from } => {
                    let msg = format!("{from} -> {cmd}");
                    self.status_line = msg.clone();
                    self.push_activity(msg);
                    self.rebuild_rows();
                }
                ConnectEvent::SyncUpdated { summary } => {
                    self.status_line = format!("Folder diff: {summary}");
                    self.push_activity(self.status_line.clone());
                    self.rebuild_rows();
                }
                ConnectEvent::PairUrlRotated(next_url) => {
                    self.qr = QrCode::encode(&next_url);
                    self.pair_url = next_url.clone();
                    if let ConnectPhase::PairQr { qr, url } = &mut self.phase {
                        if let Some(new_qr) = &self.qr {
                            *qr = new_qr.clone();
                        }
                        *url = next_url;
                    }
                }
                ConnectEvent::Error(e) => {
                    self.status_line = e;
                }
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut actions: Vec<Option<RowAction>> = Vec::new();
        let section = self.active_section();

        match section {
            ConnectSection::Overview => {
                rows.push(Row::heading("Quick Actions"));
                actions.push(None);
                rows.push(
                    Row::new("Send File or Folder\u{2026}")
                        .glyph(Glyph::ArrowUp)
                        .detail("pick in Files -> encrypted download link")
                        .value("Ctrl+S"),
                );
                actions.push(Some(RowAction::SendFile));
                rows.push(
                    Row::new("Push Clipboard Now")
                        .glyph(Glyph::Clipboard)
                        .detail("send Wayland clipboard to paired devices")
                        .value("Ctrl+V"),
                );
                actions.push(Some(RowAction::PushClipboardNow));
                rows.push(
                    Row::new("Send Link or Snippet\u{2026}")
                        .glyph(Glyph::Link)
                        .detail("type or paste a URL or text for your phone")
                        .value("Ctrl+L"),
                );
                actions.push(Some(RowAction::Compose(PushKind::Link)));
                rows.push(
                    Row::new("Show Pairing QR (HTTPS)")
                        .glyph(Glyph::Qr)
                        .detail("connect a phone browser on this Wi-Fi")
                        .value("Ctrl+Q"),
                );
                actions.push(Some(RowAction::ShowPairQr));

                rows.push(
                    Row::new("Ring Paired Phone (KDE Connect)")
                        .glyph(Glyph::Bell)
                        .detail("chime a misplaced phone via KDE Connect")
                        .value("Ring"),
                );
                actions.push(Some(RowAction::RingPhone));

                rows.push(Row::heading("Paired & Nearby Devices"));
                actions.push(None);
                if self.trusted.is_empty()
                    && self.peers.is_empty()
                    && self.telemetry.kde_peers.is_empty()
                {
                    rows.push(
                        Row::new("No devices paired yet")
                            .glyph(Glyph::Info)
                            .detail("Pair via KDE Connect app or scan the QR for Web App"),
                    );
                    actions.push(None);
                } else {
                    for dev in &self.trusted {
                        let ws_online = self
                            .telemetry
                            .connected_peers
                            .iter()
                            .any(|p| p.device_id == dev.id);
                        let kde_live = self
                            .telemetry
                            .kde_peers
                            .iter()
                            .find(|p| p.device_id == dev.id);
                        let state = if let Some(kp) = kde_live {
                            match kp.battery_pct {
                                Some(pct) if kp.charging => format!("Live · {pct}% \u{26a1}"),
                                Some(pct) => format!("Live · {pct}%"),
                                None => "Live (KDE)".to_string(),
                            }
                        } else if ws_online {
                            "Live (Web)".to_string()
                        } else {
                            "Saved".to_string()
                        };
                        rows.push(
                            Row::new(&dev.name)
                                .glyph(Glyph::Display)
                                .detail(&dev.user_agent)
                                .value(state),
                        );
                        actions.push(Some(RowAction::UnpairDevice {
                            id: dev.id.clone(),
                            label: dev.name.clone(),
                        }));
                    }
                    for peer in &self.peers {
                        rows.push(
                            Row::new(format!("{} (LocalSend)", peer.alias))
                                .glyph(Glyph::Wifi)
                                .detail(format!("{} \u{00b7} {}", peer.ip, peer.device_model))
                                .value("Enter push clip"),
                        );
                        actions.push(Some(RowAction::PushClipToPeer(peer.clone())));
                    }
                }
            }
            ConnectSection::Transfer => {
                rows.push(Row::heading("Send & Receive"));
                actions.push(None);
                rows.push(
                    Row::new("Send File / Folder Bundle\u{2026}")
                        .glyph(Glyph::ArrowUp)
                        .detail("auto-ZIP folders & push link over TLS")
                        .value("Ctrl+S"),
                );
                actions.push(Some(RowAction::SendFile));
                rows.push(
                    Row::new("Send Link or Note\u{2026}")
                        .glyph(Glyph::Link)
                        .detail("open URL on phone or share text snippet")
                        .value("Ctrl+L"),
                );
                actions.push(Some(RowAction::Compose(PushKind::Link)));
                rows.push(
                    Row::new("Receive Inbox")
                        .glyph(Glyph::Download)
                        .detail(default_downloads_dir().display().to_string())
                        .value(format!("{} done", self.telemetry.transfers)),
                );
                actions.push(Some(RowAction::OpenDownloads));

                rows.push(Row::heading("Clipboard Bridge"));
                actions.push(None);
                let clip_state = if self.auto_clip { "ON" } else { "OFF" };
                let clip_det = if self.telemetry.last_clip_preview.is_empty() {
                    "two-way sync while this window is open".to_string()
                } else {
                    format!("Last: {}", self.telemetry.last_clip_preview)
                };
                rows.push(
                    Row::new("Auto-sync Desktop <-> Phone")
                        .glyph(Glyph::Clipboard)
                        .detail(clip_det)
                        .value(clip_state),
                );
                actions.push(Some(RowAction::ToggleAutoClip));

                rows.push(Row::heading("On-Demand Folder Sync"));
                actions.push(None);
                let sync_stat = if self.telemetry.sync_summary.is_empty() {
                    "Idle · 1-shot manifest diff (no daemon)"
                } else {
                    self.telemetry.sync_summary.as_str()
                };
                rows.push(
                    Row::new(format!("Sync Root: {}", self.sync_folder.display()))
                        .glyph(Glyph::Folder)
                        .detail(sync_stat)
                        .value("Change\u{2026}"),
                );
                actions.push(Some(RowAction::PickSyncFolder));
                rows.push(
                    Row::new("Scan Sync Folder Manifest")
                        .glyph(Glyph::Sync)
                        .detail("index local files for phone two-way diff")
                        .value("Run"),
                );
                actions.push(Some(RowAction::ScanSyncFolder));
            }
            ConnectSection::Remote => {
                rows.push(Row::heading("Now Playing (MPRIS2 <-> Phone Web Remote)"));
                actions.push(None);
                let md = &self.telemetry.media;
                let vol_str = if md.muted {
                    format!("{}% [MUTE]", md.volume_pct)
                } else {
                    format!("{}%", md.volume_pct)
                };
                let state_badge = if md.status.is_empty() {
                    "Idle".to_string()
                } else {
                    format!("{} · {vol_str}", md.status)
                };
                rows.push(
                    Row::new(md.summary_line())
                        .glyph(Glyph::Music)
                        .detail(if md.player.is_empty() {
                            "Start Cassette, Spotify, or browser video"
                        } else {
                            md.player.as_str()
                        })
                        .value(state_badge),
                );
                actions.push(None);
                rows.push(
                    Row::new("Play / Pause")
                        .glyph(Glyph::Play)
                        .detail("toggle active player playback")
                        .value("p / Space"),
                );
                actions.push(Some(RowAction::MediaCmd("play_pause")));
                rows.push(
                    Row::new("Next Track")
                        .glyph(Glyph::Next)
                        .detail("skip forward")
                        .value("n"),
                );
                actions.push(Some(RowAction::MediaCmd("next")));
                rows.push(
                    Row::new("Previous Track")
                        .glyph(Glyph::Prev)
                        .detail("skip backward")
                        .value("b"),
                );
                actions.push(Some(RowAction::MediaCmd("prev")));
                rows.push(
                    Row::new("Volume Up / Down / Mute")
                        .glyph(Glyph::Volume)
                        .detail("adjust PipeWire default sink (+ / - / m)")
                        .value(vol_str),
                );
                actions.push(Some(RowAction::MediaCmd("mute")));
                rows.push(
                    Row::new("Mirror Controls to Phone")
                        .glyph(Glyph::Sync)
                        .detail("push track & volume updates over WSS")
                        .value(if self.media_mirror { "ON" } else { "OFF" }),
                );
                actions.push(Some(RowAction::ToggleMediaMirror));

                rows.push(Row::heading("Phone Camera -> Desktop Webcam"));
                actions.push(None);
                let cam = &self.telemetry.cam;
                let cam_val = if cam.active {
                    format!("LIVE · {} fps · {} kbps", cam.fps, cam.kbps)
                } else if let Some(dev) = &cam.v4l2_device {
                    format!("Ready ({dev})")
                } else {
                    "Off".to_string()
                };
                rows.push(
                    Row::new("Request Phone Camera Stream")
                        .glyph(Glyph::Camera)
                        .detail("stream getUserMedia frames over TLS WSS")
                        .value(cam_val),
                );
                actions.push(Some(RowAction::TogglePhoneCam));
                rows.push(
                    Row::new("Switch Camera Lens")
                        .glyph(Glyph::Rotate)
                        .detail("toggle front selfie vs back camera")
                        .value(if self.cam_facing == "user" {
                            "Front"
                        } else {
                            "Back"
                        }),
                );
                actions.push(Some(RowAction::FlipCamLens));
                rows.push(
                    Row::new("Open Viewfinder / Virtual Cam Sink")
                        .glyph(Glyph::Video)
                        .detail(if cam.v4l2_device.is_some() {
                            "pipes frames to v4l2loopback node"
                        } else {
                            "opens low-latency mpv Phone Cam window (v)"
                        })
                        .value(if cam.viewfinder_running {
                            "Running"
                        } else {
                            "Closed"
                        }),
                );
                actions.push(Some(RowAction::ToggleCamViewfinder));
                rows.push(
                    Row::new("Include Phone Mic -> PipeWire")
                        .glyph(Glyph::Mic)
                        .detail("streams 24 kHz phone mic to Hayato_Phone_Mic sink")
                        .value(if cam.mic_active {
                            "LIVE"
                        } else if cam.mic_enabled {
                            "ON"
                        } else {
                            "OFF"
                        }),
                );
                actions.push(Some(RowAction::TogglePhoneMic));
                rows.push(
                    Row::new("Save Phone Camera Snapshot")
                        .glyph(Glyph::Image)
                        .detail("~/Pictures/Camera/hayato-phone-*.jpg")
                        .value("Snap"),
                );
                actions.push(Some(RowAction::SnapCamPhoto));

                rows.push(Row::heading("KDE Connect iOS / Android Input & Commands"));
                actions.push(None);
                rows.push(
                    Row::new("Virtual Touchpad, Keyboard & Presenter")
                        .glyph(Glyph::Mouse)
                        .detail("Sway cursor/clicks + wtype keys + gyro presenter active")
                        .value(if self.telemetry.kde_peers.is_empty() {
                            "Standby"
                        } else {
                            "Ready"
                        }),
                );
                actions.push(None);
                rows.push(
                    Row::new("Curated Run Commands (Lock, Media, Shot, Mute)")
                        .glyph(Glyph::Terminal)
                        .detail("advertised over TLS to paired KDE Connect iPhone/Android apps")
                        .value("6 cmds"),
                );
                actions.push(None);
            }
            ConnectSection::Alerts => {
                rows.push(Row::heading("Scoped Notification Bridge"));
                actions.push(None);
                rows.push(
                    Row::new("Desktop -> Phone Alerts")
                        .glyph(Glyph::Bell)
                        .detail("forwards session notifications (pauses while recording)")
                        .value(if self.alerts_mirror { "ON" } else { "OFF" }),
                );
                actions.push(Some(RowAction::ToggleAlertsMirror));
                rows.push(
                    Row::new("Send Test Alert to Phone")
                        .glyph(Glyph::Sparkle)
                        .detail("verify foreground browser notification on phone")
                        .value("Test"),
                );
                actions.push(Some(RowAction::SendTestAlert));
                rows.push(
                    Row::new("Phone -> Desktop Endpoint")
                        .glyph(Glyph::Info)
                        .detail("POST /api/notify or Phone Web App 'Send Alert' button")
                        .value("Active"),
                );
                actions.push(None);

                rows.push(Row::heading("Recent Alerts"));
                actions.push(None);
                if self.alerts_log.is_empty() {
                    rows.push(
                        Row::new("No alerts relayed this session")
                            .glyph(Glyph::Info)
                            .detail("Notifications stay strictly in session memory"),
                    );
                    actions.push(None);
                } else {
                    for item in self.alerts_log.iter().take(6) {
                        rows.push(Row::new(item).glyph(Glyph::Bell));
                        actions.push(None);
                    }
                }
            }
        }

        if !self.activity.is_empty() && matches!(section, ConnectSection::Overview) {
            rows.push(Row::heading("Recent Activity"));
            actions.push(None);
            for entry in self.activity.iter().take(4) {
                rows.push(Row::new(entry).glyph(Glyph::Check));
                actions.push(None);
            }
        }

        self.list.update_rows(rows);
        self.actions = actions;
    }

    fn sync_chrome(&mut self) {
        self.deck_title = if let Some(p) = self.telemetry.connected_peers.first() {
            format!("{} · Live Deck", p.name)
        } else if let Some(k) = self.telemetry.kde_peers.first() {
            format!("{} · KDE Live", k.name)
        } else if let Some(d) = self.trusted.first() {
            format!("{} · Saved Deck", d.name)
        } else {
            "Pairing Deck".to_string()
        };
        let cam = &self.telemetry.cam;
        self.cam_label_line = if cam.active {
            format!(
                "Cam LIVE {} @ {} fps · {} kbps",
                cam.resolution, cam.fps, cam.kbps
            )
        } else if self.telemetry.media.has_track() {
            format!(
                "{}: {}",
                self.telemetry.media.status,
                self.telemetry.media.summary_line()
            )
        } else {
            String::new()
        };
        let connected = self.telemetry.connected_peers.len() + self.telemetry.kde_peers.len();
        let peer_note = if connected > 0 {
            format!("{connected} live")
        } else {
            format!("{} paired", self.trusted.len())
        };
        let status = match &self.phase {
            ConnectPhase::AuthorizePeer { .. } => "Authorization Required".to_string(),
            ConnectPhase::PairQr { .. } => format!("https://{}:{}", self.lan_ip, self.port),
            ConnectPhase::ComposePush { .. } => "Send Link or Snippet".to_string(),
            ConnectPhase::ConfirmAction { .. } => "Confirm".to_string(),
            ConnectPhase::Workspace { .. } => {
                format!("{}:{} · TLS · {peer_note}", self.lan_ip, self.port)
            }
        };
        self.header = Header::new("Hayato").subtitle("Connect").status(status);
        self.keybar = match &self.phase {
            ConnectPhase::Workspace {
                strip_focused: true,
                ..
            } => vec![
                KeyHint::new("\u{2190}\u{2192}", "section"),
                KeyHint::new("Enter", "select"),
                KeyHint::new("Tab", "list"),
                KeyHint::new("Esc", "back"),
            ],
            ConnectPhase::Workspace { section, .. } => {
                let mut hints = vec![
                    KeyHint::new("1-4", "section"),
                    KeyHint::new("Tab", "strip"),
                    KeyHint::new("Ctrl+Q", "QR"),
                    KeyHint::new("Ctrl+S", "send"),
                    KeyHint::new("Ctrl+V", "clip"),
                    KeyHint::new("Ctrl+L", "link"),
                ];
                if *section == ConnectSection::Overview {
                    hints.push(KeyHint::new("d", "unpair"));
                } else if *section == ConnectSection::Remote {
                    hints.push(KeyHint::new("p/n/b", "media"));
                    hints.push(KeyHint::new("v", "cam"));
                }
                hints.push(KeyHint::new("Esc", "close"));
                hints
            }
            ConnectPhase::PairQr { .. } => {
                vec![KeyHint::new("c", "copy URL"), KeyHint::new("Esc", "back")]
            }
            ConnectPhase::ComposePush { .. } => {
                vec![KeyHint::new("Enter", "send"), KeyHint::new("Esc", "cancel")]
            }
            ConnectPhase::AuthorizePeer { .. } | ConnectPhase::ConfirmAction { .. } => Vec::new(),
        };
    }

    fn activate_row_action(&mut self, action: RowAction) -> Outcome {
        match action {
            RowAction::SendFile => {
                self.pending_request = Some(ConnectRequest::PickFileToSend);
                return Outcome::Done(0);
            }
            RowAction::Compose(kind) => {
                self.phase = ConnectPhase::ComposePush {
                    kind,
                    input: Input::new(),
                };
                self.sync_chrome();
            }
            RowAction::PushClipboardNow => self.push_clipboard_now(),
            RowAction::ToggleAutoClip => {
                self.auto_clip = !self.auto_clip;
                if let Some(srv) = &self.server {
                    srv.set_auto_clip(self.auto_clip);
                }
                self.status_line = format!(
                    "Two-way clipboard sync {}",
                    if self.auto_clip { "ON" } else { "OFF" }
                );
                self.rebuild_rows();
            }
            RowAction::ShowPairQr => self.open_pair_qr(),
            RowAction::RescanPeers => {
                self.scanner = Some(PeerScanner::start());
                if let Some(srv) = &self.server {
                    srv.announce_kde();
                }
                self.status_line =
                    "Rescanning LocalSend & KDE Connect LAN peers\u{2026}".to_string();
            }
            RowAction::PushClipToPeer(peer) => {
                if let Some(text) = read_clip_text() {
                    let payload = SharePayload::from_text_or_url(
                        text,
                        Some("Clipboard".into()),
                        crate::system::share::ShareSource::Clipboard,
                    );
                    match push_to_peer(&peer, &payload) {
                        Ok(msg) => {
                            self.push_activity(msg.clone());
                            self.status_line = msg;
                            self.rebuild_rows();
                        }
                        Err(e) => self.status_line = e,
                    }
                } else {
                    self.status_line = "Clipboard empty — copy text first".to_string();
                }
            }
            RowAction::UnpairDevice { id, label } => {
                let confirm = Confirm::new(
                    format!("Unpair \"{label}\"?"),
                    "Removes saved device token.",
                )
                .with_detail("Device will need a fresh QR scan.")
                .with_options(&["Cancel", "Unpair"])
                .destructive(true);
                self.phase = ConnectPhase::ConfirmAction {
                    action: PendingConfirm::UnpairDevice { id, label },
                    confirm,
                };
                self.sync_chrome();
            }
            RowAction::OpenDownloads => {
                let dir = default_downloads_dir().to_string_lossy().into_owned();
                crate::system::spawn_detached_argv("hayato-files", &["--", &dir]);
                self.status_line = format!("Opened {dir}");
            }
            RowAction::CopyUrl => self.copy_pair_url(),
            RowAction::PickSyncFolder => {
                self.pending_request = Some(ConnectRequest::PickSyncFolder);
                return Outcome::Done(0);
            }
            RowAction::ScanSyncFolder => {
                match super::sync::build_manifest(&self.sync_folder) {
                    Ok(entries) => {
                        self.status_line = format!(
                            "Indexed {} file(s) in {}",
                            entries.len(),
                            self.sync_folder.display()
                        );
                        self.push_activity(self.status_line.clone());
                    }
                    Err(e) => self.status_line = e,
                }
                self.rebuild_rows();
            }
            RowAction::MediaCmd(cmd) => self.run_media_cmd(cmd),
            RowAction::ToggleMediaMirror => {
                self.media_mirror = !self.media_mirror;
                if let Some(srv) = &self.server {
                    srv.set_media_mirror(self.media_mirror);
                }
                self.status_line = format!(
                    "Media remote mirror {}",
                    if self.media_mirror { "ON" } else { "OFF" }
                );
                self.rebuild_rows();
            }
            RowAction::TogglePhoneCam => {
                let next = !self.telemetry.cam.active;
                if let Some(srv) = &self.server {
                    let count = srv.request_phone_cam(
                        next,
                        &self.cam_facing,
                        self.telemetry.cam.mic_enabled,
                    );
                    self.status_line = if count > 0 {
                        format!(
                            "Requested phone camera {} ({})",
                            if next { "START" } else { "STOP" },
                            self.cam_facing
                        )
                    } else {
                        "Connect phone browser first to stream camera".to_string()
                    };
                }
            }
            RowAction::FlipCamLens => {
                self.cam_facing = if self.cam_facing == "environment" {
                    "user".to_string()
                } else {
                    "environment".to_string()
                };
                if let Some(srv) = &self.server
                    && self.telemetry.cam.active
                {
                    srv.request_phone_cam(true, &self.cam_facing, self.telemetry.cam.mic_enabled);
                }
                self.status_line = format!(
                    "Camera lens preference: {}",
                    if self.cam_facing == "user" {
                        "Front"
                    } else {
                        "Back"
                    }
                );
                self.rebuild_rows();
            }
            RowAction::ToggleCamViewfinder => {
                if let Some(srv) = &self.server {
                    match srv.toggle_cam_viewfinder() {
                        Ok(msg) => {
                            self.telemetry = srv.snapshot();
                            self.status_line = msg;
                            self.rebuild_rows();
                        }
                        Err(e) => self.status_line = e,
                    }
                }
            }
            RowAction::TogglePhoneMic => {
                let next = !self.telemetry.cam.mic_enabled;
                if let Some(srv) = &self.server {
                    srv.set_phone_mic(next);
                    if self.telemetry.cam.active {
                        srv.request_phone_cam(true, &self.cam_facing, next);
                    }
                    self.telemetry = srv.snapshot();
                } else {
                    self.telemetry.cam.mic_enabled = next;
                }
                self.status_line =
                    format!("Phone Mic -> PipeWire {}", if next { "ON" } else { "OFF" });
                self.rebuild_rows();
            }
            RowAction::SnapCamPhoto => {
                if let Some(srv) = &self.server {
                    match srv.save_cam_snapshot() {
                        Ok(p) => {
                            self.status_line = format!("Saved {}", p.display());
                            self.push_activity(self.status_line.clone());
                            self.rebuild_rows();
                        }
                        Err(e) => self.status_line = e,
                    }
                }
            }
            RowAction::ToggleAlertsMirror => {
                self.alerts_mirror = !self.alerts_mirror;
                if let Some(srv) = &self.server {
                    srv.set_alerts_mirror(self.alerts_mirror);
                }
                self.status_line = format!(
                    "Desktop -> Phone alerts {}",
                    if self.alerts_mirror { "ON" } else { "OFF" }
                );
                self.rebuild_rows();
            }
            RowAction::SendTestAlert => {
                if let Some(srv) = &self.server {
                    let count = srv.broadcast_alert(
                        "Hayato Connect",
                        "Test Alert",
                        "Notification forwarding is active",
                    );
                    self.status_line = if count > 0 {
                        format!("Sent test alert to {count} phone(s)")
                    } else {
                        "No phone connected to receive test alert".to_string()
                    };
                }
            }
            RowAction::RingPhone => {
                if let Some(srv) = &self.server {
                    let count = srv.ring_phone();
                    self.status_line = if count > 0 {
                        format!("Ringing {count} KDE Connect phone(s)")
                    } else {
                        "No paired KDE Connect phone connected right now".to_string()
                    };
                }
            }
        }
        Outcome::Consumed
    }

    pub fn set_sync_folder(&mut self, folder: PathBuf) {
        let _ = super::sync::save_sync_folder(&super::sync::default_sync_config_path(), &folder);
        if let Some(srv) = &self.server {
            srv.set_sync_root(folder.clone());
        }
        self.status_line = format!("Sync root: {}", folder.display());
        self.push_activity(self.status_line.clone());
        self.sync_folder = folder;
        self.rebuild_rows();
    }

    fn run_media_cmd(&mut self, cmd: &str) {
        let res = if let Some(srv) = &self.server {
            let r = srv.exec_local_media(cmd);
            self.telemetry = srv.snapshot();
            r
        } else {
            super::media::exec_media_cmd(cmd)
        };
        match res {
            Ok(msg) => {
                self.status_line = msg;
                self.rebuild_rows();
                self.sync_chrome();
            }
            Err(e) => self.status_line = e,
        }
    }

    fn open_pair_qr(&mut self) {
        if let Some(qr) = self.qr.clone() {
            self.phase = ConnectPhase::PairQr {
                qr,
                url: self.pair_url.clone(),
            };
            self.sync_chrome();
        }
    }

    fn copy_pair_url(&mut self) {
        if copy_clip_text(&self.pair_url) {
            self.status_line = "Copied HTTPS pairing URL to clipboard".to_string();
        } else {
            self.status_line = "Could not copy pairing URL (wl-copy missing)".to_string();
        }
    }

    fn push_clipboard_now(&mut self) {
        let Some(text) = read_clip_text() else {
            self.status_line = "Clipboard is empty — copy text first".to_string();
            return;
        };
        if let Some(srv) = &self.server {
            let count = srv.broadcast_clipboard(&text);
            let msg = if count > 0 {
                format!(
                    "Pushed clipboard ({}) to {count} device(s)",
                    human_size(text.len() as u64)
                )
            } else {
                "No paired browser connected — open URL from QR on phone first".to_string()
            };
            self.push_activity(msg.clone());
            self.status_line = msg;
            self.rebuild_rows();
        }
    }

    fn submit_compose(&mut self, kind: PushKind, raw: String) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            self.set_section(ConnectSection::Overview);
            return;
        }
        if let Some(srv) = &self.server {
            let count = match kind {
                PushKind::Link
                    if trimmed.starts_with("http://") || trimmed.starts_with("https://") =>
                {
                    srv.broadcast_link(trimmed, None)
                }
                _ => {
                    let payload = SharePayload::Text {
                        content: trimmed.to_string(),
                        label: Some("Snippet".into()),
                        source: crate::system::share::ShareSource::Cli,
                    };
                    let _ = srv.offer_payload(&payload);
                    srv.broadcast_clipboard(trimmed)
                }
            };
            let msg = if count > 0 {
                format!("Sent to {count} connected device(s)")
            } else {
                "Queued locally (pair a phone via QR to deliver)".to_string()
            };
            self.push_activity(msg.clone());
            self.status_line = msg;
        }
        self.set_section(ConnectSection::Overview);
    }

    fn try_close(&mut self) -> Outcome {
        if !self.telemetry.active_label.is_empty() {
            let confirm = Confirm::new(
                "Stop active transfer & close?",
                format!("Active: {}", self.telemetry.active_label),
            )
            .with_options(&["Cancel", "Stop & Close"])
            .destructive(true);
            self.phase = ConnectPhase::ConfirmAction {
                action: PendingConfirm::StopActiveAndClose,
                confirm,
            };
            self.sync_chrome();
            return Outcome::Consumed;
        }
        Outcome::Done(hayato_ui::app::exit::ESCAPE)
    }

    fn draw_live_deck(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        self.deck_btns.set([Rect::default(); 2]);
        draw_card(buf, area, design, Some(&self.deck_title), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 6 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        let (badge_txt, badge_role, desc) = if !self.telemetry.kde_peers.is_empty() {
            ("● KDE LIVE", ColorRole::Ok, "KDE Connect TLS + Web App")
        } else if !self.telemetry.connected_peers.is_empty() {
            ("● PAIRED", ColorRole::Ok, "TLS 1.3 + WSS live")
        } else if !self.trusted.is_empty() {
            (
                "○ STANDBY",
                ColorRole::AccentAlt,
                "KDE Connect or QR reconnect",
            )
        } else {
            ("○ UNPAIRED", ColorRole::Warn, "KDE Connect app or scan QR")
        };
        let bx = draw_badge(buf, inner.x, y, design, badge_txt, badge_role);
        put_trunc(
            buf,
            bx + 1,
            y,
            desc,
            inner.right().saturating_sub(bx + 1),
            body,
        );
        y += 1;

        if y < inner.bottom() {
            put_trunc(
                buf,
                inner.x,
                y,
                &self.pair_url,
                inner.width,
                design.style(TextStyle::Code),
            );
            y += 1;
        }

        let footer_y = inner.bottom().saturating_sub(1);
        let btns_y = footer_y.saturating_sub(1);
        let progress_reserve = if self.telemetry.bytes_total > 0 { 4 } else { 0 };
        let padded_top = (y + 1).min(btns_y);
        let padded_bottom = btns_y.saturating_sub(1 + progress_reserve);
        let (qr_top, qr_bottom) = match &self.qr {
            Some(qr) => {
                let (_, need_h) = qr.cell_size();
                if padded_bottom.saturating_sub(padded_top) >= need_h {
                    (padded_top, padded_bottom)
                } else if btns_y.saturating_sub(progress_reserve + y) >= need_h {
                    (y, btns_y.saturating_sub(progress_reserve))
                } else {
                    (padded_top, padded_bottom)
                }
            }
            None => (padded_top, padded_bottom),
        };
        y = qr_top;
        if let Some(qr) = &self.qr
            && qr_bottom > y + 2
        {
            let qr_area = Rect::new(inner.x, y, inner.width, qr_bottom - y);
            crate::system::draw_qr_fit(
                buf,
                qr_area,
                design,
                qr,
                "Press Ctrl+Q for full pairing QR",
            );
            y = qr_bottom + 1;
        }

        if self.telemetry.bytes_total > 0 && y + 2 <= btns_y {
            let label = if self.telemetry.active_label.is_empty() {
                "Transferring\u{2026}"
            } else {
                self.telemetry.active_label.as_str()
            };
            put_trunc(buf, inner.x, y, label, inner.width, accent);
            let ratio = (self.telemetry.bytes_done as f64 / self.telemetry.bytes_total as f64)
                .clamp(0.0, 1.0);
            draw_gauge(
                buf,
                Rect::new(inner.x, y + 1, inner.width, 1),
                design,
                ratio,
                "",
            );
        } else if !self.cam_label_line.is_empty() && y < btns_y {
            put_trunc(buf, inner.x, y, &self.cam_label_line, inner.width, accent);
        }

        // Bottom interactive deck pills
        if btns_y >= inner.y {
            let pill_style = Style::default()
                .fg(design.color(ColorRole::KeyText))
                .bg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD);
            let ghost_style = Style::default()
                .fg(design.color(ColorRole::Text))
                .bg(design.color(ColorRole::SurfaceAlt));
            let b1 = " ^Q Full QR ";
            let b2 = " Copy URL ";
            let mut cx = inner.x;
            let mut rects = [Rect::default(); 2];
            if cx + b1.len() as u16 <= inner.right() {
                put(buf, cx, btns_y, b1, b1.len() as u16, pill_style);
                rects[0] = Rect::new(cx, btns_y, b1.len() as u16, 1);
                cx += b1.len() as u16 + 1;
            }
            if cx + b2.len() as u16 <= inner.right() {
                put(buf, cx, btns_y, b2, b2.len() as u16, ghost_style);
                rects[1] = Rect::new(cx, btns_y, b2.len() as u16, 1);
            }
            self.deck_btns.set(rects);
        }

        put_trunc(
            buf,
            inner.x,
            footer_y,
            "Zero-daemon · closing stops all ports",
            inner.width,
            dim,
        );
    }
}

impl Screen for ConnectScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((1220, 800))
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        self.confirm_area.set(Rect::default());
        self.strip_area.set(Rect::default());
        self.list_area.set(Rect::default());
        self.deck_btns.set([Rect::default(); 2]);

        match &mut self.phase {
            ConnectPhase::AuthorizePeer { confirm, .. } => {
                self.confirm_area.set(ctx.area);
                confirm.render(ctx.area, ctx.buf, ctx.design);
            }
            ConnectPhase::ConfirmAction { confirm, .. } => {
                self.confirm_area.set(ctx.area);
                confirm.render(ctx.area, ctx.buf, ctx.design);
            }
            ConnectPhase::PairQr { qr, url } => {
                let inner = draw_card(
                    ctx.buf,
                    ctx.area,
                    ctx.design,
                    Some("Hayato Connect · Pairing QR (TLS)"),
                    true,
                );
                if inner.height >= 6 {
                    let status_y = inner.bottom().saturating_sub(1);
                    let cert_hint_y = status_y.saturating_sub(1);
                    let footer_y = cert_hint_y.saturating_sub(1);
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
                        "Window too narrow for QR · press c to copy URL",
                    );
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        footer_y,
                        url,
                        inner.width,
                        ctx.design.style(TextStyle::Code),
                    );
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        cert_hint_y,
                        "Safari/Chrome first tap: Show Details \u{2192} Visit Website (verified via #fp)",
                        inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        status_y,
                        &self.status_line,
                        inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                }
            }
            ConnectPhase::ComposePush { kind, input } => {
                let title = match kind {
                    PushKind::Link => "Send Link or Text to Phone",
                    PushKind::ClipboardText => "Push Text to Phone Clipboard",
                };
                let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some(title), true);
                if inner.height >= 4 {
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        inner.y,
                        "URLs open on phone; text copies to phone clipboard · Enter sends",
                        inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                    input.render_field(
                        Rect::new(inner.x, inner.y + 2, inner.width, 1),
                        ctx.buf,
                        ctx.design,
                        "https://... or snippet",
                        true,
                        true,
                        ColorRole::Bg,
                        TextStyle::Body,
                    );
                }
            }
            ConnectPhase::Workspace {
                section,
                strip,
                strip_focused,
            } => {
                let strip_h = 1.min(ctx.area.height);
                let strip_rect = Rect::new(ctx.area.x, ctx.area.y, ctx.area.width, strip_h);
                self.strip_area.set(strip_rect);
                strip.render_on(strip_rect, ctx.buf, ctx.design, ColorRole::Bg);

                let workspace = Rect::new(
                    ctx.area.x,
                    ctx.area.y + strip_h,
                    ctx.area.width,
                    ctx.area.height.saturating_sub(strip_h),
                );
                let (left, right) = crate::system::split_panes(workspace, 54, 44);
                let list_focused = !*strip_focused;
                draw_card(
                    ctx.buf,
                    left,
                    ctx.design,
                    Some(section.card_title()),
                    list_focused,
                );
                let inner = Rect::new(
                    left.x + 2,
                    left.y + 1,
                    left.width.saturating_sub(4),
                    left.height.saturating_sub(2),
                );
                let footer_h = if inner.height > 4 { 1 } else { 0 };
                let list_rect = Rect::new(
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height.saturating_sub(footer_h),
                );
                self.list_area.set(list_rect);
                self.rows_height.set(list_rect.height);
                self.list
                    .render(list_rect, ctx.buf, ctx.design, list_focused);

                if footer_h > 0 {
                    put_trunc(
                        ctx.buf,
                        inner.x,
                        inner.bottom().saturating_sub(1),
                        &self.status_line,
                        inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                }
                if let Some(right_rect) = right {
                    self.draw_live_deck(ctx.buf, right_rect, ctx.design);
                }
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if alt {
            return Outcome::Continue;
        }

        // Modal phases first
        if matches!(self.phase, ConnectPhase::AuthorizePeer { .. }) {
            let quick = if !ctrl {
                match key.code {
                    KeyCode::Char('t' | 'y') => Some(ConfirmAction::Chosen(1)),
                    KeyCode::Char('d' | 'n') => Some(ConfirmAction::Cancelled),
                    _ => None,
                }
            } else {
                None
            };
            let act = if let Some(q) = quick {
                q
            } else if let ConnectPhase::AuthorizePeer { confirm, .. } = &mut self.phase {
                confirm.handle(key)
            } else {
                ConfirmAction::None
            };
            match act {
                ConfirmAction::Chosen(1) => {
                    if let ConnectPhase::AuthorizePeer { req_id, .. } = &self.phase
                        && let Some(srv) = &self.server
                    {
                        srv.respond_auth(req_id, true);
                    }
                    self.status_line = "Authorized pairing request".to_string();
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    if let ConnectPhase::AuthorizePeer { req_id, .. } = &self.phase
                        && let Some(srv) = &self.server
                    {
                        srv.respond_auth(req_id, false);
                    }
                    self.status_line = "Denied pairing request".to_string();
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }

        if matches!(self.phase, ConnectPhase::ConfirmAction { .. }) {
            let act = if let ConnectPhase::ConfirmAction { confirm, .. } = &mut self.phase {
                confirm.handle(key)
            } else {
                ConfirmAction::None
            };
            match act {
                ConfirmAction::Chosen(1) => {
                    let pending = match &self.phase {
                        ConnectPhase::ConfirmAction { action, .. } => action.clone(),
                        _ => unreachable!(),
                    };
                    match pending {
                        PendingConfirm::StopActiveAndClose => {
                            return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                        }
                        PendingConfirm::UnpairDevice { id, label } => {
                            if let Some(srv) = &self.server {
                                let _ = srv.unpair_device(&id);
                                self.trusted = srv.trust_devices();
                            } else {
                                self.trusted.retain(|d| d.id != id);
                            }
                            self.push_activity(format!("Unpaired {label}"));
                            self.status_line = format!("Unpaired {label}");
                            self.set_section(ConnectSection::Overview);
                        }
                    }
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }

        if let ConnectPhase::PairQr { .. } = &self.phase {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.set_section(ConnectSection::Overview);
                }
                KeyCode::Char('c') if !ctrl => {
                    self.copy_pair_url();
                }
                _ => {}
            }
            return Outcome::Consumed;
        }

        if let ConnectPhase::ComposePush { kind, input } = &mut self.phase {
            let k = *kind;
            match input.handle(key) {
                hayato_ui::InputAction::Submitted => {
                    let text = input.text().to_string();
                    self.submit_compose(k, text);
                    return Outcome::Consumed;
                }
                hayato_ui::InputAction::Cancelled => {
                    self.set_section(ConnectSection::Overview);
                    return Outcome::Consumed;
                }
                hayato_ui::InputAction::Changed => return Outcome::Consumed,
                hayato_ui::InputAction::Ignored => return Outcome::Continue,
            }
        }

        // Workspace global Ctrl shortcuts
        if ctrl {
            match key.code {
                KeyCode::Char('q' | 'Q') => {
                    self.open_pair_qr();
                    return Outcome::Consumed;
                }
                KeyCode::Char('s' | 'S') => {
                    return self.activate_row_action(RowAction::SendFile);
                }
                KeyCode::Char('v' | 'V') => {
                    self.push_clipboard_now();
                    return Outcome::Consumed;
                }
                KeyCode::Char('l' | 'L') => {
                    return self.activate_row_action(RowAction::Compose(PushKind::Link));
                }
                KeyCode::Char('r' | 'R') => {
                    return self.activate_row_action(RowAction::RescanPeers);
                }
                _ => return Outcome::Continue,
            }
        }

        // Workspace strip navigation (`Tab` / `1-4`)
        if let ConnectPhase::Workspace {
            strip,
            strip_focused,
            ..
        } = &mut self.phase
        {
            match strip.handle(key) {
                ActionStripEvent::Activated("qr") => {
                    *strip_focused = false;
                    strip.set_focused(false);
                    self.open_pair_qr();
                    return Outcome::Consumed;
                }
                ActionStripEvent::Activated(id) => {
                    *strip_focused = false;
                    strip.set_focused(false);
                    if let Some(sec) = ConnectSection::from_id(id) {
                        self.set_section(sec);
                    }
                    return Outcome::Consumed;
                }
                ActionStripEvent::Consumed => {
                    *strip_focused = strip.is_focused();
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                ActionStripEvent::Ignored => {}
            }
        }

        match key.code {
            KeyCode::Char('1') => {
                self.set_section(ConnectSection::Overview);
                return Outcome::Consumed;
            }
            KeyCode::Char('2') => {
                self.set_section(ConnectSection::Transfer);
                return Outcome::Consumed;
            }
            KeyCode::Char('3') => {
                self.set_section(ConnectSection::Remote);
                return Outcome::Consumed;
            }
            KeyCode::Char('4') => {
                self.set_section(ConnectSection::Alerts);
                return Outcome::Consumed;
            }
            KeyCode::Char('[' | ']') => {
                let all = [
                    ConnectSection::Overview,
                    ConnectSection::Transfer,
                    ConnectSection::Remote,
                    ConnectSection::Alerts,
                ];
                let cur = match &self.phase {
                    ConnectPhase::Workspace { section, .. } => section.index(),
                    _ => 0,
                };
                let next = if key.code == KeyCode::Char(']') {
                    (cur + 1) % all.len()
                } else {
                    (cur + all.len() - 1) % all.len()
                };
                self.set_section(all[next]);
                return Outcome::Consumed;
            }
            KeyCode::Char('c') => {
                self.copy_pair_url();
                return Outcome::Consumed;
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.list.selected_index()
                    && let Some(Some(RowAction::UnpairDevice { id, label })) =
                        self.actions.get(idx).cloned()
                {
                    return self.activate_row_action(RowAction::UnpairDevice { id, label });
                }
            }
            KeyCode::Char('p') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("play_pause");
                return Outcome::Consumed;
            }
            KeyCode::Char('n') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("next");
                return Outcome::Consumed;
            }
            KeyCode::Char('b') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("prev");
                return Outcome::Consumed;
            }
            KeyCode::Char('+' | '=') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("vol_up");
                return Outcome::Consumed;
            }
            KeyCode::Char('-') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("vol_down");
                return Outcome::Consumed;
            }
            KeyCode::Char('m') if self.active_section() == ConnectSection::Remote => {
                self.run_media_cmd("mute");
                return Outcome::Consumed;
            }
            KeyCode::Char('v') if self.active_section() == ConnectSection::Remote => {
                return self.activate_row_action(RowAction::ToggleCamViewfinder);
            }
            KeyCode::Esc => return self.try_close(),
            _ => {}
        }

        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(idx) => {
                if let Some(Some(act)) = self.actions.get(idx).cloned() {
                    return self.activate_row_action(act);
                }
                Outcome::Consumed
            }
            ListAction::Moved => Outcome::Consumed,
            ListAction::Ignored => {
                if matches!(key.code, KeyCode::Char(' '))
                    && let Some(idx) = self.list.selected_index()
                    && let Some(Some(act)) = self.actions.get(idx).cloned()
                {
                    return self.activate_row_action(act);
                }
                Outcome::Continue
            }
        }
    }

    fn hit(&self, _area: Rect, col: u16, row: u16, design: &Design) -> Option<usize> {
        match &self.phase {
            ConnectPhase::AuthorizePeer { confirm, .. }
            | ConnectPhase::ConfirmAction { confirm, .. } => {
                confirm.hit(self.confirm_area.get(), design, col, row)
            }
            ConnectPhase::PairQr { .. } | ConnectPhase::ComposePush { .. } => None,
            ConnectPhase::Workspace { strip, .. } => {
                let btns = self.deck_btns.get();
                if btns[0].contains(Position { x: col, y: row }) {
                    return Some(HIT_BTN_QR);
                }
                if btns[1].contains(Position { x: col, y: row }) {
                    return Some(HIT_BTN_COPY);
                }
                if let Some(id) = strip.hit(self.strip_area.get(), col, row) {
                    let idx = match id {
                        "overview" => 0,
                        "transfer" => 1,
                        "remote" => 2,
                        "alerts" => 3,
                        "qr" => 4,
                        _ => return None,
                    };
                    return Some(HIT_STRIP_BASE + idx);
                }
                self.list.hit_row(self.list_area.get(), col, row, design)
            }
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(r) = row else {
            return Outcome::Continue;
        };
        if matches!(self.phase, ConnectPhase::AuthorizePeer { .. }) {
            let act = if let ConnectPhase::AuthorizePeer { confirm, .. } = &mut self.phase {
                confirm.click(r)
            } else {
                ConfirmAction::None
            };
            match act {
                ConfirmAction::Chosen(1) => {
                    if let ConnectPhase::AuthorizePeer { req_id, .. } = &self.phase
                        && let Some(srv) = &self.server
                    {
                        srv.respond_auth(req_id, true);
                    }
                    self.status_line = "Authorized pairing request".to_string();
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    if let ConnectPhase::AuthorizePeer { req_id, .. } = &self.phase
                        && let Some(srv) = &self.server
                    {
                        srv.respond_auth(req_id, false);
                    }
                    self.status_line = "Denied pairing request".to_string();
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if matches!(self.phase, ConnectPhase::ConfirmAction { .. }) {
            let act = if let ConnectPhase::ConfirmAction { confirm, .. } = &mut self.phase {
                confirm.click(r)
            } else {
                ConfirmAction::None
            };
            match act {
                ConfirmAction::Chosen(1) => {
                    let pending = match &self.phase {
                        ConnectPhase::ConfirmAction { action, .. } => action.clone(),
                        _ => unreachable!(),
                    };
                    match pending {
                        PendingConfirm::StopActiveAndClose => {
                            return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                        }
                        PendingConfirm::UnpairDevice { id, label } => {
                            if let Some(srv) = &self.server {
                                let _ = srv.unpair_device(&id);
                                self.trusted = srv.trust_devices();
                            } else {
                                self.trusted.retain(|d| d.id != id);
                            }
                            self.push_activity(format!("Unpaired {label}"));
                            self.status_line = format!("Unpaired {label}");
                            self.set_section(ConnectSection::Overview);
                        }
                    }
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.set_section(ConnectSection::Overview);
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if r == HIT_BTN_QR {
            self.open_pair_qr();
            return Outcome::Consumed;
        }
        if r == HIT_BTN_COPY {
            self.copy_pair_url();
            return Outcome::Consumed;
        }
        if r >= HIT_STRIP_BASE {
            let idx = r - HIT_STRIP_BASE;
            match idx {
                0 => self.set_section(ConnectSection::Overview),
                1 => self.set_section(ConnectSection::Transfer),
                2 => self.set_section(ConnectSection::Remote),
                3 => self.set_section(ConnectSection::Alerts),
                4 => self.open_pair_qr(),
                _ => {}
            }
            return Outcome::Consumed;
        }
        self.list.set_cursor(r);
        if let Some(Some(act)) = self.actions.get(r).cloned() {
            return self.activate_row_action(act);
        }
        Outcome::Consumed
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        if self.keybar.is_empty() {
            None
        } else {
            Some(&self.keybar)
        }
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match &self.phase {
            ConnectPhase::Workspace { section, .. } => match section {
                ConnectSection::Overview => "connect.overview",
                ConnectSection::Transfer => "connect.transfer",
                ConnectSection::Remote => "connect.remote",
                ConnectSection::Alerts => "connect.alerts",
            },
            ConnectPhase::PairQr { .. } => "connect.pair_qr",
            ConnectPhase::AuthorizePeer { .. } => "connect.authorize",
            ConnectPhase::ComposePush { .. } => "connect.compose",
            ConnectPhase::ConfirmAction { .. } => "connect.confirm",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        Some(Duration::from_millis(250))
    }

    fn event(&mut self, event: &ratatui::crossterm::event::Event, _design: &Design) -> Outcome {
        if let ratatui::crossterm::event::Event::Paste(text) = event
            && let ConnectPhase::ComposePush { input, .. } = &mut self.phase
        {
            for ch in text.chars().filter(|c| !c.is_control()) {
                input.handle(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            }
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn tick(&mut self) -> Outcome {
        self.poll_events();
        Outcome::Continue
    }
}

fn read_clip_text() -> Option<String> {
    super::server::read_wayland_clipboard()
}

fn copy_clip_text(text: &str) -> bool {
    super::server::write_wayland_clipboard(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(screen: &mut ConnectScreen, w: u16, h: u16) -> String {
        let design = Design::solarized().with_reduced_motion(true);
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = hayato_ui::App::new(Header::new("Hayato").subtitle("Connect"))
            .design(design)
            .screen(screen);
        term.draw(|f| app.render(f)).unwrap();
        hayato_ui::screen(term.backend().buffer())
    }

    #[test]
    fn renders_overview_transfer_remote_alerts_and_authorize_gate() {
        let mut screen = ConnectScreen::for_test();
        let narrow = render(&mut screen, 80, 24);
        assert!(narrow.contains("Pixel 8"));
        assert!(!narrow.contains("Pairing Deck"));

        let float_view = render(&mut screen, 94, 30);
        assert!(float_view.contains("Saved Deck"));
        assert!(float_view.contains("Pixel 8"));

        let wide = render(&mut screen, 152, 45);
        assert!(wide.contains("Bruno iPad"));

        screen.key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.transfer"));
        let transfer = render(&mut screen, 94, 30);
        assert!(transfer.contains("Auto-sync Desktop <-> Phone"));
        assert!(transfer.contains("Sync Root:"));

        screen.key(
            KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.remote"));
        let remote = render(&mut screen, 152, 45);
        assert!(remote.contains("Play / Pause"));
        assert!(remote.contains("Request Phone Camera Stream"));

        screen.key(
            KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.alerts"));
        let alerts = render(&mut screen, 94, 30);
        assert!(alerts.contains("Desktop -> Phone Alerts"));
        assert!(alerts.contains("Signal"));

        // PairQr modal + copy feedback
        screen.key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.pair_qr"));
        screen.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &Design::solarized(),
        );
        let qr_view = render(&mut screen, 94, 30);
        assert!(qr_view.contains("Pairing QR"));
        screen.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.overview"));

        // ComposePush modal + paste handling
        screen.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &Design::solarized(),
        );
        assert_eq!(screen.phase(), Some("connect.compose"));
        let compose_view = render(&mut screen, 80, 24);
        assert!(compose_view.contains("Send Link or Text"));
        screen.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Design::solarized(),
        );

        // ConfirmAction (UnpairDevice) defaults to Cancel, option 1 = Unpair
        let _ = screen.activate_row_action(RowAction::UnpairDevice {
            id: "dev-pixel8".into(),
            label: "Pixel 8".into(),
        });
        assert_eq!(screen.phase(), Some("connect.confirm"));
        let confirm_view = render(&mut screen, 80, 24);
        assert!(confirm_view.contains("Unpair \"Pixel 8\"?"));
        assert!(confirm_view.contains("Cancel"));
        assert!(confirm_view.contains("Unpair"));
        screen.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &Design::solarized(),
        );

        // AuthorizePeer modal via simulated AuthRequested
        screen.phase = ConnectPhase::AuthorizePeer {
            req_id: "req-1".into(),
            device_label: "Pixel 8".into(),
            remote_ip: "192.168.1.99".into(),
            sas_code: "482-910".into(),
            confirm: Confirm::new("Authorize New Device?", "SAS: 482-910")
                .with_detail("Pixel 8 (192.168.1.99)\nGrants LAN access while Connect is open.")
                .with_options(&["Deny", "Trust & Pair"]),
        };
        assert_eq!(screen.phase(), Some("connect.authorize"));
        let auth_view = render(&mut screen, 94, 30);
        assert!(auth_view.contains("482-910"));
        assert!(auth_view.contains("Trust & Pair"));
    }

    #[test]
    fn wide_window_renders_qr_deck_on_start() {
        let mut screen = ConnectScreen::for_test();
        assert_eq!(screen.desired_size(), Some((1220, 800)));
        // Representative 41x41 QR matrix (v5 + 2-module quiet zone): needs 41x21 cells.
        screen.qr = Some(QrCode {
            width: 41,
            height: 41,
            modules: (0..41 * 41).map(|i| i % 2 == 0).collect(),
        });
        // Body size inside a 1220x800 (`hayato-wide`) foot window (~134x39 minus 4 chrome rows).
        let view = render(&mut screen, 134, 35);
        assert!(
            !view.contains("Press Ctrl+Q for full pairing QR"),
            "QR should render inline on opening at Shape::Wide size:\n{view}"
        );
        assert!(
            view.contains('\u{2580}') || view.contains('\u{2588}'),
            "Expected QR half/full blocks in right deck:\n{view}"
        );
    }
}
