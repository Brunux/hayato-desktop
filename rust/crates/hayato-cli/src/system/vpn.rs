//! Native Hayato VPN client: WireGuard and OpenVPN over NetworkManager (`hayato menu vpn`).
//!
//! Replaces `nmtui` for VPN management: imports `.conf` (WireGuard) and `.ovpn`
//! (OpenVPN) profiles via `nmcli`, toggles tunnels on/off, manages autoconnect
//! and OpenVPN credentials, and reads live IPv4 + RX/TX byte counters directly
//! from NetworkManager and `/sys/class/net/<dev>/statistics/` with zero daemons.

use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hayato_ui::widgets::{Confirm, ConfirmAction, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card_with,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::pick::{Picker, Question, row as pick_row};
use super::{checked, have, out, sudo};
use crate::Failure;

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["nmcli"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VpnKind {
    WireGuard,
    OpenVpn,
    Other(String),
}

impl VpnKind {
    pub fn badge(&self) -> &str {
        match self {
            Self::WireGuard => "WG",
            Self::OpenVpn => "OVPN",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::WireGuard => "WireGuard",
            Self::OpenVpn => "OpenVPN",
            Self::Other(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpnProfile {
    pub name: String,
    pub uuid: String,
    pub kind: VpnKind,
    pub active: bool,
    pub device: Option<String>,
    pub autoconnect: bool,
    pub ipv4: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub endpoint: Option<String>,
}

/// Parse `nmcli -t -f NAME,UUID,TYPE,DEVICE,AUTOCONNECT connection show`.
/// Handles escaped colons (`\:`) inside connection names.
pub fn parse_nmcli_connections(text: &str) -> Vec<VpnProfile> {
    let mut out_list = Vec::new();
    for line in text.lines() {
        let fields = split_nmcli_terse(line);
        if fields.len() < 5 {
            continue;
        }
        let name = fields[0].clone();
        let uuid = fields[1].clone();
        let kind_str = fields[2].as_str();
        let dev_str = fields[3].as_str();
        let auto_str = fields[4].as_str();

        let kind = match kind_str {
            "wireguard" => VpnKind::WireGuard,
            "vpn" => VpnKind::OpenVpn,
            _ => continue,
        };

        let device = if dev_str.is_empty() || dev_str == "--" {
            None
        } else {
            Some(dev_str.to_string())
        };
        let active = device.is_some();
        let autoconnect = auto_str == "yes";

        out_list.push(VpnProfile {
            name,
            uuid,
            kind,
            active,
            device,
            autoconnect,
            ipv4: None,
            rx_bytes: 0,
            tx_bytes: 0,
            endpoint: None,
        });
    }
    // Active tunnels first, then alphabetical.
    out_list.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out_list
}

/// Split an `nmcli -t` line on unescaped `:` separators.
pub fn split_nmcli_terse(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
            }
        } else if c == ':' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    parts.push(current);
    parts
}

/// Detect whether a config file is WireGuard (`.conf`) or OpenVPN (`.ovpn`).
pub fn detect_config_kind(path: &Path, content: &str) -> Option<VpnKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let has_wg_markers = content.contains("[Interface]") && content.contains("[Peer]");
    let has_ovpn_markers = content.lines().any(|l| {
        let t = l.trim();
        t.starts_with("remote ") || t == "client" || t.starts_with("dev tun")
    });

    if has_wg_markers {
        return Some(VpnKind::WireGuard);
    }
    if has_ovpn_markers {
        return Some(VpnKind::OpenVpn);
    }
    match ext.as_str() {
        "ovpn" => Some(VpnKind::OpenVpn),
        "conf" => Some(VpnKind::WireGuard),
        _ => None,
    }
}

/// Format byte counters compactly (`420 B`, `14.2 KB`, `38.1 MB`, `1.4 GB`).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Check whether NetworkManager's OpenVPN plugin is installed.
pub fn openvpn_plugin_installed() -> bool {
    let candidates = [
        "/usr/lib/NetworkManager/VPN/nm-openvpn-service.name",
        "/usr/lib/aarch64-linux-gnu/NetworkManager/VPN/nm-openvpn-service.name",
        "/usr/lib/x86_64-linux-gnu/NetworkManager/VPN/nm-openvpn-service.name",
        "/etc/NetworkManager/VPN/nm-openvpn-service.name",
    ];
    if candidates.iter().any(|p| Path::new(p).is_file()) {
        return true;
    }
    out(&[
        "dpkg-query",
        "-W",
        "-f=${Status}",
        "network-manager-openvpn",
    ])
    .is_some_and(|s| s.contains("install ok installed"))
}

fn read_sysfs_u64(path: &Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn enrich_profile(p: &mut VpnProfile) {
    // Refine OpenVPN vs other NM VPN plugin if possible.
    if p.kind == VpnKind::OpenVpn
        && let Some(svc) = out(&[
            "nmcli",
            "-g",
            "vpn.service-type",
            "connection",
            "show",
            "uuid",
            &p.uuid,
        ])
    {
        let svc = svc.trim();
        if !svc.is_empty() && !svc.contains("openvpn") {
            let short = svc.rsplit('.').next().unwrap_or(svc).to_string();
            p.kind = VpnKind::Other(short);
        }
    }

    if let Some(dev) = p.device.as_deref() {
        p.ipv4 = out(&["nmcli", "-g", "IP4.ADDRESS", "device", "show", dev])
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
            .filter(|s| !s.is_empty());

        let stats = PathBuf::from("/sys/class/net").join(dev).join("statistics");
        p.rx_bytes = read_sysfs_u64(&stats.join("rx_bytes"));
        p.tx_bytes = read_sysfs_u64(&stats.join("tx_bytes"));

        if p.kind == VpnKind::WireGuard && have("wg") {
            p.endpoint = out(&["wg", "show", dev, "endpoints"])
                .or_else(|| sudo(&["wg", "show", dev, "endpoints"]).ok())
                .and_then(|s| {
                    s.lines().find_map(|l| {
                        let ep = l.split_whitespace().nth(1)?;
                        (ep != "(none)").then(|| ep.to_string())
                    })
                });
        }
    }
}

/// Probe all NetworkManager WireGuard & OpenVPN profiles on this machine.
pub fn list_profiles() -> Vec<VpnProfile> {
    let raw = out(&[
        "nmcli",
        "-t",
        "-f",
        "NAME,UUID,TYPE,DEVICE,AUTOCONNECT",
        "connection",
        "show",
    ])
    .unwrap_or_default();
    let mut profiles = parse_nmcli_connections(&raw);
    for p in &mut profiles {
        enrich_profile(p);
    }
    profiles
}

/// Headless one-line status for `hayato menu vpn --status` and `Settings`.
pub fn status() -> String {
    let profiles = list_profiles();
    if profiles.is_empty() {
        return "none configured".to_string();
    }
    let active: Vec<&VpnProfile> = profiles.iter().filter(|p| p.active).collect();
    if let Some(first) = active.first() {
        if active.len() == 1 {
            format!("connected: {} ({})", first.name, first.kind.label())
        } else {
            format!("{} connected ({})", active.len(), first.name)
        }
    } else if profiles.len() == 1 {
        format!("1 saved ({}) · off", profiles[0].name)
    } else {
        format!("{} saved · off", profiles.len())
    }
}

/// Scan common user directories for `.conf` (WireGuard) and `.ovpn` (OpenVPN) files.
pub fn scan_config_files() -> Vec<(PathBuf, VpnKind)> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let dirs = [
        home.join("Downloads"),
        home.join("Documents"),
        home.join(".config/vpn"),
        home,
    ];
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if ext != "conf" && ext != "ovpn" {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Some(kind) = detect_config_kind(&path, &content)
            {
                found.push((path, kind));
            }
            if found.len() >= 24 {
                return found;
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

fn nmcli_run(args: &[&str]) -> Result<String, String> {
    let mut argv = vec!["nmcli"];
    argv.extend_from_slice(args);
    match checked(&argv) {
        Ok(out) => Ok(out),
        Err(first_err) => sudo(&argv).map_err(|_| first_err),
    }
}

pub fn import_profile(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("File not found: {}", path.display()));
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Could not read config: {e}"))?;
    let kind = detect_config_kind(path, &content).ok_or_else(|| {
        "Unrecognized VPN config (expected WireGuard .conf or OpenVPN .ovpn)".to_string()
    })?;
    if kind == VpnKind::OpenVpn && !openvpn_plugin_installed() {
        return Err("OpenVPN plugin missing — select 'Install OpenVPN support' first".to_string());
    }
    let nm_type = match kind {
        VpnKind::WireGuard => "wireguard",
        VpnKind::OpenVpn => "openvpn",
        VpnKind::Other(_) => "openvpn",
    };
    let path_str = path.to_string_lossy().into_owned();
    nmcli_run(&["connection", "import", "type", nm_type, "file", &path_str])?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");
    Ok(format!("Imported {} profile: {stem}", kind.label()))
}

pub fn toggle_connection(p: &VpnProfile) -> Result<String, String> {
    if p.active {
        nmcli_run(&["connection", "down", "uuid", &p.uuid])?;
        Ok(format!("Disconnected: {}", p.name))
    } else {
        nmcli_run(&["connection", "up", "uuid", &p.uuid])?;
        Ok(format!("Connected: {}", p.name))
    }
}

pub fn toggle_autoconnect(p: &VpnProfile) -> Result<String, String> {
    let next = if p.autoconnect { "no" } else { "yes" };
    nmcli_run(&[
        "connection",
        "modify",
        "uuid",
        &p.uuid,
        "connection.autoconnect",
        next,
    ])?;
    Ok(format!(
        "Autoconnect {}: {}",
        if p.autoconnect { "off" } else { "on" },
        p.name
    ))
}

pub fn set_openvpn_credentials(
    uuid: &str,
    username: &str,
    password: &str,
) -> Result<String, String> {
    if username.trim().is_empty() {
        return Err("Username cannot be empty".to_string());
    }
    let user_kv = format!("username={}", username.trim());
    let pass_kv = format!("password={password}");
    nmcli_run(&[
        "connection",
        "modify",
        "uuid",
        uuid,
        "+vpn.data",
        &user_kv,
        "+vpn.data",
        "password-flags=0",
        "vpn.secrets",
        &pass_kv,
    ])?;
    Ok("OpenVPN credentials saved".to_string())
}

pub fn delete_profile(name: &str, uuid: &str) -> Result<String, String> {
    nmcli_run(&["connection", "delete", "uuid", uuid])?;
    Ok(format!("Deleted VPN profile: {name}"))
}

// ─── TUI Screen ──────────────────────────────────────────────────────────────

enum VpnPhase {
    Browse,
    ConfirmDelete {
        name: String,
        uuid: String,
        confirm: Confirm,
    },
}

enum VpnRowKind {
    Profile(usize),
    ImportFile,
    InstallOpenVpn,
}

pub enum VpnRequest {
    ImportFile,
    OpenVpnAuth { name: String, uuid: String },
    InstallOpenVpn,
}

pub struct VpnScreen {
    header: Header,
    keybar: Vec<KeyHint>,
    profiles: Vec<VpnProfile>,
    openvpn_ready: bool,
    list: ListState,
    row_kinds: Vec<VpnRowKind>,
    phase: VpnPhase,
    banner: Option<String>,
    pending_request: Option<VpnRequest>,
    list_area: Cell<Rect>,
    confirm_area: Cell<Rect>,
}

impl VpnScreen {
    pub fn new(banner: Option<String>) -> Self {
        let mut s = Self {
            header: Header::new("Hayato").subtitle("VPN"),
            keybar: Vec::new(),
            profiles: list_profiles(),
            openvpn_ready: openvpn_plugin_installed(),
            list: ListState::new(),
            row_kinds: Vec::new(),
            phase: VpnPhase::Browse,
            banner,
            pending_request: None,
            list_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
        };
        s.rebuild_rows();
        s
    }

    /// Deterministic snapshot fixture for `tests/screens.rs`.
    pub fn for_snapshot() -> Self {
        let profiles = vec![
            VpnProfile {
                name: "work-wg".to_string(),
                uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                kind: VpnKind::WireGuard,
                active: true,
                device: Some("wg0".to_string()),
                autoconnect: true,
                ipv4: Some("10.66.0.2/32".to_string()),
                rx_bytes: 39_950_745,
                tx_bytes: 4_404_019,
                endpoint: Some("203.0.113.10:51820".to_string()),
            },
            VpnProfile {
                name: "home-ovpn".to_string(),
                uuid: "66666666-7777-8888-9999-aaaaaaaaaaaa".to_string(),
                kind: VpnKind::OpenVpn,
                active: false,
                device: None,
                autoconnect: false,
                ipv4: None,
                rx_bytes: 0,
                tx_bytes: 0,
                endpoint: None,
            },
        ];
        let mut s = Self {
            header: Header::new("Hayato").subtitle("VPN"),
            keybar: Vec::new(),
            profiles,
            openvpn_ready: true,
            list: ListState::new(),
            row_kinds: Vec::new(),
            phase: VpnPhase::Browse,
            banner: None,
            pending_request: None,
            list_area: Cell::new(Rect::default()),
            confirm_area: Cell::new(Rect::default()),
        };
        s.rebuild_rows();
        s
    }

    pub fn refresh(&mut self) {
        self.profiles = list_profiles();
        self.openvpn_ready = openvpn_plugin_installed();
        self.rebuild_rows();
    }

    fn update_chrome(&mut self) {
        let active = self.profiles.iter().filter(|p| p.active).count();
        let status = if active > 0 {
            format!("{active} active \u{00b7} WireGuard & OpenVPN")
        } else if self.profiles.is_empty() {
            "WireGuard & OpenVPN".to_string()
        } else {
            format!("{} saved \u{00b7} WireGuard & OpenVPN", self.profiles.len())
        };
        self.header = Header::new("Hayato").subtitle("VPN").status(status);
        self.keybar = if matches!(self.phase, VpnPhase::ConfirmDelete { .. }) {
            vec![
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ]
        } else {
            vec![
                KeyHint::new("Enter", "toggle"),
                KeyHint::new("i", "import"),
                KeyHint::new("a", "auto"),
                KeyHint::new("u", "auth"),
                KeyHint::new("d", "delete"),
                KeyHint::new("Esc", "close"),
            ]
        };
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut kinds = Vec::new();

        for (i, p) in self.profiles.iter().enumerate() {
            let state_dot = if p.active { "\u{25cf}" } else { "\u{25cb}" };
            let label = format!("{state_dot} {} [{}]", p.name, p.kind.badge());
            let detail = if p.active {
                let mut parts = Vec::new();
                if let Some(ip) = &p.ipv4 {
                    parts.push(ip.clone());
                } else {
                    parts.push("connected".to_string());
                }
                if p.rx_bytes > 0 || p.tx_bytes > 0 {
                    parts.push(format!(
                        "\u{2191}{} \u{2193}{}",
                        format_bytes(p.tx_bytes),
                        format_bytes(p.rx_bytes)
                    ));
                }
                if p.autoconnect {
                    parts.push("auto".to_string());
                }
                parts.join(" \u{00b7} ")
            } else if p.autoconnect {
                "disconnected \u{00b7} autoconnect".to_string()
            } else {
                "disconnected".to_string()
            };
            let glyph = if p.active { Glyph::Lock } else { Glyph::Shield };
            rows.push(Row::new(label).detail(detail).glyph(glyph));
            kinds.push(VpnRowKind::Profile(i));
        }

        rows.push(
            Row::new("Import VPN config (.conf / .ovpn)")
                .detail("WireGuard or OpenVPN profile file")
                .glyph(Glyph::Download),
        );
        kinds.push(VpnRowKind::ImportFile);

        if !self.openvpn_ready {
            rows.push(
                Row::new("Install OpenVPN support")
                    .detail("apt install network-manager-openvpn")
                    .glyph(Glyph::Box),
            );
            kinds.push(VpnRowKind::InstallOpenVpn);
        }

        self.row_kinds = kinds;
        self.list.set_rows(rows);
        self.update_chrome();
    }

    fn selected_kind(&self) -> Option<&VpnRowKind> {
        self.list
            .selected_index()
            .and_then(|i| self.row_kinds.get(i))
    }

    fn selected_profile(&self) -> Option<&VpnProfile> {
        match self.selected_kind()? {
            VpnRowKind::Profile(i) => self.profiles.get(*i),
            _ => None,
        }
    }

    fn activate_selected(&mut self) -> Outcome {
        let Some(kind) = self.selected_kind() else {
            return Outcome::Consumed;
        };
        match kind {
            VpnRowKind::Profile(i) => {
                if let Some(p) = self.profiles.get(*i).cloned() {
                    match toggle_connection(&p) {
                        Ok(msg) => {
                            self.banner = Some(msg);
                            self.refresh();
                        }
                        Err(err) => self.banner = Some(err),
                    }
                }
                Outcome::Consumed
            }
            VpnRowKind::ImportFile => {
                self.pending_request = Some(VpnRequest::ImportFile);
                Outcome::Done(0)
            }
            VpnRowKind::InstallOpenVpn => {
                self.pending_request = Some(VpnRequest::InstallOpenVpn);
                Outcome::Done(0)
            }
        }
    }
}

impl Screen for VpnScreen {
    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.phase {
            VpnPhase::Browse => "browse",
            VpnPhase::ConfirmDelete { .. } => "confirm-delete",
        })
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let area = ctx.area;
        if area.width < 30 || area.height < 8 {
            return;
        }

        if let VpnPhase::ConfirmDelete { confirm, .. } = &mut self.phase {
            self.confirm_area.set(area);
            confirm.render(area, ctx.buf, ctx.design);
            return;
        }

        let card = draw_card_with(
            ctx.buf,
            area,
            ctx.design,
            Some("VPN Tunnels"),
            ColorRole::Surface,
            ctx.design.style(TextStyle::Title),
        );

        let mut y = card.y;
        let wg_note = if self.openvpn_ready {
            "WireGuard: native (NM) \u{00b7} OpenVPN: ready"
        } else {
            "WireGuard: native (NM) \u{00b7} OpenVPN: plugin not installed"
        };
        put_trunc(
            ctx.buf,
            card.x + 1,
            y,
            wg_note,
            card.width.saturating_sub(2),
            ctx.design.style(TextStyle::Dim),
        );
        y += 1;

        if let Some(msg) = &self.banner {
            put_trunc(
                ctx.buf,
                card.x + 1,
                y,
                msg,
                card.width.saturating_sub(2),
                Style::default()
                    .fg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD),
            );
            y += 1;
        }

        // Blank separator line.
        y += 1;

        let list_rect = Rect::new(card.x, y, card.width, card.bottom().saturating_sub(y));
        self.list_area.set(list_rect);
        self.list.render(list_rect, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        if let VpnPhase::ConfirmDelete {
            name,
            uuid,
            confirm,
        } = &mut self.phase
        {
            match confirm.handle(key) {
                ConfirmAction::Cancelled | ConfirmAction::Chosen(0) => {
                    self.phase = VpnPhase::Browse;
                    self.update_chrome();
                    return Outcome::Consumed;
                }
                ConfirmAction::Chosen(_) => {
                    let n = name.clone();
                    let u = uuid.clone();
                    self.phase = VpnPhase::Browse;
                    match delete_profile(&n, &u) {
                        Ok(msg) => {
                            self.banner = Some(msg);
                            self.refresh();
                        }
                        Err(err) => {
                            self.banner = Some(err);
                            self.update_chrome();
                        }
                    }
                    return Outcome::Consumed;
                }
                ConfirmAction::None => return Outcome::Consumed,
            }
        }

        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
                KeyCode::Char('i') => {
                    self.pending_request = Some(VpnRequest::ImportFile);
                    return Outcome::Done(0);
                }
                KeyCode::Char('r') => {
                    self.refresh();
                    self.banner = Some("Refreshed VPN status".to_string());
                    return Outcome::Consumed;
                }
                KeyCode::Char('a') => {
                    if let Some(p) = self.selected_profile().cloned() {
                        match toggle_autoconnect(&p) {
                            Ok(msg) => {
                                self.banner = Some(msg);
                                self.refresh();
                            }
                            Err(err) => self.banner = Some(err),
                        }
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('u') => {
                    if let Some(p) = self.selected_profile().cloned() {
                        if p.kind == VpnKind::OpenVpn {
                            self.pending_request = Some(VpnRequest::OpenVpnAuth {
                                name: p.name,
                                uuid: p.uuid,
                            });
                            return Outcome::Done(0);
                        } else {
                            self.banner =
                                Some("Credentials apply to OpenVPN profiles only".to_string());
                        }
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(p) = self.selected_profile().cloned() {
                        let confirm =
                            Confirm::new(format!("Delete VPN profile '{}'?", p.name), "Delete")
                                .with_detail(format!(
                                    "Removes {} configuration from NetworkManager",
                                    p.kind.label()
                                ));
                        self.phase = VpnPhase::ConfirmDelete {
                            name: p.name,
                            uuid: p.uuid,
                            confirm,
                        };
                        self.update_chrome();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('c') => return self.activate_selected(),
                _ => {}
            }
        }

        if key.modifiers == KeyModifiers::NONE || key.code == KeyCode::Enter {
            match self.list.handle(key, self.list_area.get().height, design) {
                ListAction::Activated(_) => return self.activate_selected(),
                ListAction::Moved => return Outcome::Consumed,
                ListAction::Ignored => {}
            }
        }

        Outcome::Continue
    }

    fn hit(&self, _area: Rect, col: u16, row: u16, design: &Design) -> Option<usize> {
        if let VpnPhase::ConfirmDelete { confirm, .. } = &self.phase {
            return confirm.hit(self.confirm_area.get(), design, col, row);
        }
        self.list.hit_row(self.list_area.get(), col, row, design)
    }

    fn click(&mut self, index: Option<usize>) -> Outcome {
        let Some(index) = index else {
            return Outcome::Continue;
        };
        if let VpnPhase::ConfirmDelete {
            name,
            uuid,
            confirm,
        } = &mut self.phase
        {
            match confirm.click(index) {
                ConfirmAction::Chosen(1) => {
                    let n = name.clone();
                    let u = uuid.clone();
                    self.phase = VpnPhase::Browse;
                    match delete_profile(&n, &u) {
                        Ok(msg) => {
                            self.banner = Some(msg);
                            self.refresh();
                        }
                        Err(err) => {
                            self.banner = Some(err);
                            self.update_chrome();
                        }
                    }
                }
                ConfirmAction::Cancelled | ConfirmAction::Chosen(0) => {
                    self.phase = VpnPhase::Browse;
                    self.update_chrome();
                }
                _ => {}
            }
            return Outcome::Consumed;
        }
        self.list.set_cursor(index);
        self.activate_selected()
    }
}

/// Interactive entry point for `hayato menu vpn`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    if let Some(msg) = super::missing(NEEDED) {
        return Err(Failure::new(msg, "Install network-manager to manage VPNs"));
    }

    let mut banner: Option<String> = None;
    loop {
        let mut screen = VpnScreen::new(banner.take());
        let code = crate::run_screen(terminal, design, "VPN", &mut screen)?;
        let Some(req) = screen.pending_request.take() else {
            return Ok(code);
        };

        match req {
            VpnRequest::ImportFile => {
                banner = run_import_picker(terminal, design)?;
            }
            VpnRequest::OpenVpnAuth { name, uuid } => {
                banner = run_openvpn_auth_picker(&name, &uuid, terminal, design)?;
            }
            VpnRequest::InstallOpenVpn => {
                run_interactive_apt_install(terminal);
                banner = Some(if openvpn_plugin_installed() {
                    "OpenVPN support installed".to_string()
                } else {
                    "OpenVPN plugin was not installed".to_string()
                });
            }
        }
    }
}

fn run_import_picker(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
) -> Result<Option<String>, Failure> {
    let discovered = scan_config_files();
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    for (path, kind) in &discovered {
        let label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config")
            .to_string();
        let detail = format!("{} \u{00b7} {}", kind.label(), path.display());
        rows.push(pick_row(label, detail, Glyph::File));
        keys.push(path.to_string_lossy().into_owned());
    }
    rows.push(pick_row(
        "Enter config file path...",
        "type or paste path to .conf or .ovpn",
        Glyph::Terminal,
    ));
    keys.push("__custom__".to_string());

    let note = if discovered.is_empty() {
        "No .conf/.ovpn files found in ~/Downloads or ~/Documents".to_string()
    } else {
        format!(
            "Found {} config file(s) \u{00b7} Enter imports \u{00b7} Esc cancels",
            discovered.len()
        )
    };
    let mut picker = Picker::new(
        "Import VPN profile",
        vec![Question::choose("Import VPN profile", note, rows, keys)],
    );
    let code = crate::run_screen(terminal, design, "Import VPN profile", &mut picker)?;
    if code != 0 {
        return Ok(None);
    }
    let Some(choice) = picker.answers().first().cloned() else {
        return Ok(None);
    };

    let chosen_path = if choice == "__custom__" {
        let mut input = Picker::new(
            "VPN config path",
            vec![Question::input(
                "Path to .conf (WireGuard) or .ovpn (OpenVPN)",
                "Supports ~/ prefix \u{00b7} Esc cancels",
                "~/Downloads/wg0.conf",
                false,
            )],
        );
        let c = crate::run_screen(terminal, design, "VPN config path", &mut input)?;
        if c != 0 {
            return Ok(None);
        }
        let Some(raw) = input.answers().first().cloned() else {
            return Ok(None);
        };
        expand_tilde(raw.trim())
    } else {
        PathBuf::from(choice)
    };

    match import_profile(&chosen_path) {
        Ok(msg) => Ok(Some(msg)),
        Err(err) => Ok(Some(err)),
    }
}

fn run_openvpn_auth_picker(
    name: &str,
    uuid: &str,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
) -> Result<Option<String>, Failure> {
    let mut picker = Picker::new(
        "OpenVPN credentials",
        vec![
            Question::input(
                format!("Username ({name})"),
                "OpenVPN account username",
                "user",
                false,
            ),
            Question::input(
                "Password",
                "stored in NetworkManager secrets",
                "password",
                true,
            ),
        ],
    );
    let code = crate::run_screen(terminal, design, "OpenVPN credentials", &mut picker)?;
    if code != 0 {
        return Ok(None);
    }
    if let [user, pass] = picker.answers().as_slice() {
        match set_openvpn_credentials(uuid, user, pass) {
            Ok(msg) => Ok(Some(msg)),
            Err(err) => Ok(Some(err)),
        }
    } else {
        Ok(None)
    }
}

fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
}

fn run_interactive_apt_install(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
    let _ = ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::LeaveAlternateScreen
    );
    let _ = Command::new("sh")
        .args([
            "-c",
            "sudo -n apt-get update && sudo -n apt-get install -y network-manager-openvpn; echo; read -r -p 'Press Enter to return...' _",
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let _ = ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::EnterAlternateScreen
    );
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
    let _ = terminal.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wireguard_and_openvpn_from_nmcli_terse() {
        let sample = "\
Home\\:WiFi:aaaa:802-11-wireless:wlan0:yes
work-wg:1111:wireguard:wg0:yes
home-ovpn:2222:vpn:--:no
lo:3333:loopback:lo:no
";
        let list = parse_nmcli_connections(sample);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "work-wg");
        assert_eq!(list[0].kind, VpnKind::WireGuard);
        assert!(list[0].active);
        assert_eq!(list[0].device.as_deref(), Some("wg0"));
        assert!(list[0].autoconnect);

        assert_eq!(list[1].name, "home-ovpn");
        assert_eq!(list[1].kind, VpnKind::OpenVpn);
        assert!(!list[1].active);
        assert!(list[1].device.is_none());
        assert!(!list[1].autoconnect);
    }

    #[test]
    fn detects_wireguard_and_openvpn_configs() {
        let wg =
            "[Interface]\nPrivateKey = abc\nAddress = 10.0.0.2/32\n\n[Peer]\nPublicKey = def\n";
        assert_eq!(
            detect_config_kind(Path::new("/tmp/mullvad.conf"), wg),
            Some(VpnKind::WireGuard)
        );
        let ovpn = "client\ndev tun\nproto udp\nremote vpn.example.com 1194\n";
        assert_eq!(
            detect_config_kind(Path::new("/tmp/office.ovpn"), ovpn),
            Some(VpnKind::OpenVpn)
        );
        assert_eq!(
            detect_config_kind(Path::new("/tmp/notes.txt"), "hello"),
            None
        );
    }

    #[test]
    fn formats_byte_counters_cleanly() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(39_950_745), "38.1 MB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
    }
}
