//! Native Hayato VPN Studio: WireGuard and OpenVPN over NetworkManager (`hayato menu vpn`).
//!
//! Master-detail Tunnel Studio (`width >= 100`, single-pane + context strip on 80x24)
//! paired with a single-screen multi-field Form Editor (`VpnPhase::EditForm`) using
//! `Input::render_field`:
//! - Inline protocol toggle (`[WireGuard | OpenVPN]`), live validation, and live config preview
//! - Instant WireGuard keypair generator (`Ctrl+G`) displaying the derived Public Key (`Ctrl+Y` copy)
//! - Smart clipboard `.conf` / `.ovpn` paste-parser (`v` in Browse, `Ctrl+V` in Form)
//! - Native `FilesScreen` browser for `.conf` / `.ovpn` import (`i`) and OpenVPN CA certs (`Ctrl+O`)
//! - In-place profile editing (`e`), autoconnect (`a`), Cancel-first delete (`d`), and `custom/vpn` Waybar indicator.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use hayato_ui::widgets::{Confirm, ConfirmAction, Input, InputAction, put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card_with,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

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

/// Detect whether a config file or text is WireGuard (`.conf`) or OpenVPN (`.ovpn`).
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

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn pango_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Signal Waybar (`RTMIN+8`) so `custom/vpn` updates immediately on toggle.
pub fn signal_bar() {
    let _ = out(&["pkill", "-RTMIN+8", "waybar"]);
}

/// Render the Waybar JSON payload for `custom/vpn` (`hayato menu vpn --bar`).
/// Collapses to zero width (`class: "empty"`) when no VPN tunnel is active.
pub fn bar_json_for(profiles: &[VpnProfile]) -> String {
    let active: Vec<&VpnProfile> = profiles.iter().filter(|p| p.active).collect();
    if active.is_empty() {
        return r#"{"text":"","class":"empty","tooltip":""}"#.to_string();
    }
    let text = if active.len() == 1 {
        "<span font_desc='JetBrains Mono 13'>\u{f132}</span>".to_string()
    } else {
        format!(
            "<span font_desc='JetBrains Mono 13'>\u{f132}</span> {}",
            active.len()
        )
    };
    let first = active[0];
    let proto = first.kind.label().to_uppercase();
    let mut lines = vec![format!(
        "<span alpha='50%'>╭─</span> <b>VPN // {}</b>",
        pango_escape(&proto)
    )];
    for p in &active {
        let dev_suffix = p
            .device
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        lines.push(format!(
            "<span alpha='45%'>│</span> <span alpha='60%'>TUN</span>  {}{}",
            pango_escape(&p.name),
            pango_escape(&dev_suffix)
        ));
        if let Some(ip) = &p.ipv4 {
            lines.push(format!(
                "<span alpha='45%'>│</span> <span alpha='60%'>IP </span>  {}",
                pango_escape(ip)
            ));
        }
        if p.rx_bytes > 0 || p.tx_bytes > 0 {
            lines.push(format!(
                "<span alpha='45%'>│</span> <span alpha='60%'>IO </span>  \u{2191}{} \u{2193}{}",
                format_bytes(p.tx_bytes),
                format_bytes(p.rx_bytes)
            ));
        }
    }
    lines.push(
        "<span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] VPN · [RMB] disconnect</span>"
            .to_string(),
    );
    let tooltip = lines.join("\n");
    format!(
        r#"{{"text":"{}","class":"connected","tooltip":"{}"}}"#,
        json_escape(&text),
        json_escape(&tooltip)
    )
}

/// Live Waybar JSON payload (`hayato menu vpn --bar`).
pub fn bar_json() -> String {
    bar_json_for(&list_profiles())
}

/// Headless disconnect for Waybar right-click (`hayato menu vpn --disconnect`).
pub fn disconnect_active() -> i32 {
    let active: Vec<VpnProfile> = list_profiles().into_iter().filter(|p| p.active).collect();
    if active.is_empty() {
        signal_bar();
        return hayato_ui::app::exit::OK;
    }
    let mut disconnected = Vec::new();
    for p in &active {
        if nmcli_run(&["connection", "down", "uuid", &p.uuid]).is_ok() {
            disconnected.push(p.name.clone());
        }
    }
    signal_bar();
    if !disconnected.is_empty() {
        super::say(
            "VPN",
            format!("Disconnected: {}", disconnected.join(", ")),
            hayato_ui::Urgency::Normal,
            Glyph::Shield,
        );
    }
    hayato_ui::app::exit::OK
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
        VpnKind::OpenVpn | VpnKind::Other(_) => "openvpn",
    };
    let path_str = path.to_string_lossy().into_owned();
    nmcli_run(&["connection", "import", "type", nm_type, "file", &path_str])?;
    signal_bar();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");
    Ok(format!("Imported {} profile: {stem}", kind.label()))
}

pub fn toggle_connection(p: &VpnProfile) -> Result<String, String> {
    if p.active {
        nmcli_run(&["connection", "down", "uuid", &p.uuid])?;
        signal_bar();
        Ok(format!("Disconnected: {}", p.name))
    } else {
        nmcli_run(&["connection", "up", "uuid", &p.uuid])?;
        signal_bar();
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

pub fn delete_profile(name: &str, uuid: &str) -> Result<String, String> {
    nmcli_run(&["connection", "delete", "uuid", uuid])?;
    signal_bar();
    Ok(format!("Deleted VPN profile: {name}"))
}

// ─── Config Specs, Validation, Parsing & Keygen ──────────────────────────────

/// Parameters for creating or editing a WireGuard tunnel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireGuardSpec {
    pub name: String,
    pub address: String,
    pub private_key: String,
    pub peer_public_key: String,
    pub endpoint: String,
    pub allowed_ips: String,
    pub dns: String,
}

/// Parameters for creating or editing an OpenVPN tunnel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenVpnSpec {
    pub name: String,
    pub remote: String,
    pub port: String,
    pub proto_tcp: bool,
    pub username: String,
    pub password: String,
    pub ca_path: Option<PathBuf>,
}

/// Validate a WireGuard base64 key (44 chars ending in `=`, valid base64 alphabet).
pub fn is_valid_wg_key(s: &str) -> bool {
    let t = s.trim();
    t.len() == 44
        && t.ends_with('=')
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// Return per-field validation checks for a WireGuard spec: `(ok, message)`.
pub fn validate_wireguard_spec(spec: &WireGuardSpec) -> Vec<(bool, String)> {
    let mut checks = Vec::new();
    let name = spec.name.trim();
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    checks.push((
        name_ok,
        if name_ok {
            "Connection name valid".to_string()
        } else {
            "Name required (letters, digits, '-' or '_')".to_string()
        },
    ));

    let addr = spec.address.trim();
    let addr_ok = !addr.is_empty() && addr.contains('/');
    checks.push((
        addr_ok,
        if addr_ok {
            "Local CIDR valid".to_string()
        } else {
            "Address must include CIDR (e.g. 10.66.0.2/32)".to_string()
        },
    ));

    let priv_k = spec.private_key.trim();
    let priv_ok = is_valid_wg_key(priv_k);
    checks.push((
        priv_ok,
        if priv_ok {
            "Private key valid (44-char base64)".to_string()
        } else if priv_k.is_empty() {
            "Private key required (or press Ctrl+G)".to_string()
        } else {
            "Private key must be 44-char base64 ending in '='".to_string()
        },
    ));

    let peer_k = spec.peer_public_key.trim();
    let swapped = !priv_k.is_empty() && priv_k == peer_k;
    let peer_ok = is_valid_wg_key(peer_k) && !swapped;
    checks.push((
        peer_ok,
        if swapped {
            "Peer key matches Private key (swapped?)".to_string()
        } else if peer_ok {
            "Peer public key valid".to_string()
        } else if peer_k.is_empty() {
            "Peer public key required (44-char base64)".to_string()
        } else {
            "Peer public key must be 44-char base64 ending in '='".to_string()
        },
    ));

    let ep = spec.endpoint.trim();
    let ep_ok = ep.rsplit_once(':').is_some_and(|(h, p)| {
        !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())
    });
    checks.push((
        ep_ok,
        if ep_ok {
            "Endpoint valid".to_string()
        } else {
            "Endpoint must be host:port (e.g. vpn.host:51820)".to_string()
        },
    ));

    checks
}

/// Return per-field validation checks for an OpenVPN spec: `(ok, message)`.
pub fn validate_openvpn_spec(spec: &OpenVpnSpec) -> Vec<(bool, String)> {
    let mut checks = Vec::new();
    let name_ok = !spec.name.trim().is_empty();
    checks.push((
        name_ok,
        if name_ok {
            "Connection name valid".to_string()
        } else {
            "Connection name is required".to_string()
        },
    ));

    let remote_ok = !spec.remote.trim().is_empty();
    checks.push((
        remote_ok,
        if remote_ok {
            "Server gateway valid".to_string()
        } else {
            "Server hostname or IP is required".to_string()
        },
    ));

    if let Some(ca) = &spec.ca_path {
        let ca_ok = ca.is_file();
        checks.push((
            ca_ok,
            if ca_ok {
                "CA certificate found".to_string()
            } else {
                format!("CA file not found: {}", ca.display())
            },
        ));
    }

    checks
}

/// Parse a WireGuard `.conf` snippet (from clipboard or file) into [`WireGuardSpec`].
pub fn parse_wireguard_conf(text: &str) -> Option<WireGuardSpec> {
    if !text.contains("[Interface]") && !text.contains("[Peer]") {
        return None;
    }
    let mut section = "";
    let mut spec = WireGuardSpec {
        name: "wg-tunnel".to_string(),
        address: String::new(),
        private_key: String::new(),
        peer_public_key: String::new(),
        endpoint: String::new(),
        allowed_ips: "0.0.0.0/0, ::/0".to_string(),
        dns: String::new(),
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line;
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        match (section, key.as_str()) {
            ("[Interface]", "privatekey") => spec.private_key = val.to_string(),
            ("[Interface]", "address") => spec.address = val.to_string(),
            ("[Interface]", "dns") => spec.dns = val.to_string(),
            ("[Peer]", "publickey") => spec.peer_public_key = val.to_string(),
            ("[Peer]", "endpoint") => spec.endpoint = val.to_string(),
            ("[Peer]", "allowedips") => spec.allowed_ips = val.to_string(),
            _ => {}
        }
    }
    if spec.private_key.is_empty() && spec.peer_public_key.is_empty() && spec.endpoint.is_empty() {
        return None;
    }
    Some(spec)
}

/// Parse an OpenVPN `.ovpn` snippet (from clipboard or file) into [`OpenVpnSpec`].
pub fn parse_openvpn_conf(text: &str) -> Option<OpenVpnSpec> {
    let mut spec = OpenVpnSpec {
        name: "ovpn-tunnel".to_string(),
        remote: String::new(),
        port: String::new(),
        proto_tcp: false,
        username: String::new(),
        password: String::new(),
        ca_path: None,
    };
    let mut saw_marker = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        match cmd {
            "client" => saw_marker = true,
            "remote" => {
                saw_marker = true;
                if let Some(host) = parts.next() {
                    if let Some(port) = parts.next() {
                        spec.remote = format!("{host}:{port}");
                    } else {
                        spec.remote = host.to_string();
                    }
                }
            }
            "proto" => {
                if let Some(p) = parts.next() {
                    spec.proto_tcp = p.to_ascii_lowercase().starts_with("tcp");
                }
            }
            "ca" => {
                if let Some(ca) = parts.next() {
                    spec.ca_path = Some(PathBuf::from(ca));
                }
            }
            _ => {}
        }
    }
    if saw_marker && !spec.remote.is_empty() {
        Some(spec)
    } else {
        None
    }
}

/// Build a standard WireGuard `.conf` file from user inputs.
pub fn build_wireguard_config(spec: &WireGuardSpec) -> Result<String, String> {
    if let Some((_, err)) = validate_wireguard_spec(spec)
        .into_iter()
        .find(|(ok, _)| !ok)
    {
        return Err(err);
    }
    let allowed = if spec.allowed_ips.trim().is_empty() {
        "0.0.0.0/0, ::/0"
    } else {
        spec.allowed_ips.trim()
    };

    let mut conf = format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}\n",
        spec.private_key.trim(),
        spec.address.trim()
    );
    if !spec.dns.trim().is_empty() {
        conf.push_str(&format!("DNS = {}\n", spec.dns.trim()));
    }
    conf.push_str(&format!(
        "\n[Peer]\nPublicKey = {}\nEndpoint = {}\nAllowedIPs = {}\nPersistentKeepalive = 25\n",
        spec.peer_public_key.trim(),
        spec.endpoint.trim(),
        allowed
    ));
    Ok(conf)
}

/// Create (or replace) a WireGuard connection via a `0600` temp `.conf` imported into `nmcli`.
pub fn create_wireguard_profile(
    spec: &WireGuardSpec,
    replace_uuid: Option<&str>,
) -> Result<String, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let conf = build_wireguard_config(spec)?;
    let safe_name = spec.name.trim();
    let tmp_dir = std::env::temp_dir().join(format!("hayato-wg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let tmp_path = tmp_dir.join(format!("{safe_name}.conf"));
    let write_res = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(conf.as_bytes())
        });
    if let Err(e) = write_res {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!("Could not write temporary WireGuard config: {e}"));
    }
    if let Some(uuid) = replace_uuid {
        let _ = nmcli_run(&["connection", "delete", "uuid", uuid]);
    }
    let path_str = tmp_path.to_string_lossy().into_owned();
    let res = nmcli_run(&[
        "connection",
        "import",
        "type",
        "wireguard",
        "file",
        &path_str,
    ]);
    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir(&tmp_dir);
    res?;
    signal_bar();
    let verb = if replace_uuid.is_some() {
        "Updated"
    } else {
        "Created"
    };
    Ok(format!("{verb} WireGuard profile: {safe_name}"))
}

/// Build the `vpn.data` value string for `nmcli connection add type vpn vpn-type openvpn`.
pub fn build_openvpn_data(spec: &OpenVpnSpec) -> Result<String, String> {
    if let Some((_, err)) = validate_openvpn_spec(spec).into_iter().find(|(ok, _)| !ok) {
        return Err(err);
    }
    let remote = spec.remote.trim();
    let (host, parsed_port) = match remote.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.trim(), Some(p.trim()))
        }
        _ => (remote, None),
    };
    let port = if !spec.port.trim().is_empty() {
        spec.port.trim()
    } else {
        parsed_port.unwrap_or("1194")
    };
    let mut pairs = vec![
        format!("remote={host}"),
        format!("port={port}"),
        format!("proto-tcp={}", if spec.proto_tcp { "yes" } else { "no" }),
        "connection-type=password".to_string(),
        "dev-type=tun".to_string(),
    ];
    if !spec.username.trim().is_empty() {
        pairs.push(format!("username={}", spec.username.trim()));
    }
    if !spec.password.is_empty() {
        pairs.push("password-flags=0".to_string());
    }
    if let Some(ca) = &spec.ca_path {
        pairs.push(format!("ca={}", ca.display()));
    }
    Ok(pairs.join(", "))
}

/// Create (or replace) an OpenVPN profile directly via `nmcli connection add`.
pub fn create_openvpn_profile(
    spec: &OpenVpnSpec,
    replace_uuid: Option<&str>,
) -> Result<String, String> {
    if !openvpn_plugin_installed() {
        return Err("OpenVPN plugin missing — select 'Install OpenVPN support' first".to_string());
    }
    let vpn_data = build_openvpn_data(spec)?;
    let name = spec.name.trim();
    if let Some(uuid) = replace_uuid {
        let _ = nmcli_run(&["connection", "delete", "uuid", uuid]);
    }
    let mut args = vec![
        "connection",
        "add",
        "type",
        "vpn",
        "vpn-type",
        "openvpn",
        "con-name",
        name,
        "--",
        "vpn.data",
        &vpn_data,
    ];
    let secrets;
    if !spec.password.is_empty() {
        secrets = format!("password={}", spec.password);
        args.push("vpn.secrets");
        args.push(&secrets);
    }
    nmcli_run(&args)?;
    signal_bar();
    let verb = if replace_uuid.is_some() {
        "Updated"
    } else {
        "Created"
    };
    Ok(format!("{verb} OpenVPN profile: {name}"))
}

fn derive_wg_pubkey(private_key: &str) -> Option<String> {
    if !is_valid_wg_key(private_key) || !have("wg") {
        return None;
    }
    let mut child = Command::new("wg")
        .arg("pubkey")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = writeln!(stdin, "{}", private_key.trim());
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let pubkey = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!pubkey.is_empty()).then_some(pubkey)
}

fn generate_wg_keypair() -> Result<(String, String), String> {
    if !have("wg") {
        return Err(
            "wireguard-tools ('wg') not installed — paste a private key manually".to_string(),
        );
    }
    let priv_key = out(&["wg", "genkey"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "wg genkey failed".to_string())?;
    let pub_key = derive_wg_pubkey(&priv_key).unwrap_or_default();
    Ok((priv_key, pub_key))
}

fn clipboard_paste() -> Option<String> {
    out(&["wl-paste", "-n"]).filter(|s| !s.trim().is_empty())
}

fn clipboard_copy(text: &str) -> bool {
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }
        return child.wait().is_ok_and(|s| s.success());
    }
    false
}

// ─── Single-Screen Multi-Field Form Editor ───────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormProtocol {
    WireGuard,
    OpenVpn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FormFocus {
    Protocol,
    Name,
    // WireGuard
    Address,
    PrivateKey,
    PeerPublicKey,
    Endpoint,
    AllowedIps,
    Dns,
    // OpenVPN
    OvpnRemote,
    OvpnTransport,
    OvpnUsername,
    OvpnPassword,
    OvpnCaPath,
    // Actions
    Save,
    Cancel,
}

#[derive(Default)]
struct FormHits {
    wg_pill: Rect,
    ovpn_pill: Rect,
    fields: Vec<(FormFocus, Rect)>,
    genkey_btn: Rect,
    copypub_btn: Rect,
    browse_ca_btn: Rect,
    save_btn: Rect,
    cancel_btn: Rect,
}

#[derive(Default)]
struct InspectorHits {
    toggle_btn: Rect,
    edit_btn: Rect,
    auto_btn: Rect,
    delete_btn: Rect,
}

pub struct TunnelForm {
    editing_uuid: Option<String>,
    protocol: FormProtocol,
    focus: FormFocus,
    // Shared
    name: Input,
    // WireGuard
    address: Input,
    private_key: Input,
    derived_pubkey: String,
    peer_public_key: Input,
    endpoint: Input,
    allowed_ips: Input,
    dns: Input,
    // OpenVPN
    ovpn_remote: Input,
    ovpn_tcp: bool,
    ovpn_username: Input,
    ovpn_password: Input,
    ovpn_ca_path: Input,
    // Cached state (recomputed on edit, never per frame)
    validation: Vec<(bool, String)>,
    first_error: Option<String>,
    preview_lines: Vec<String>,
    form_notice: Option<String>,
}

impl TunnelForm {
    fn new_blank(protocol: FormProtocol) -> Self {
        let mut f = Self {
            editing_uuid: None,
            protocol,
            focus: FormFocus::Name,
            name: Input::new(),
            address: Input::new(),
            private_key: Input::new().with_masked(true),
            derived_pubkey: String::new(),
            peer_public_key: Input::new(),
            endpoint: Input::new(),
            allowed_ips: Input::with_text("0.0.0.0/0, ::/0"),
            dns: Input::new(),
            ovpn_remote: Input::new(),
            ovpn_tcp: false,
            ovpn_username: Input::new(),
            ovpn_password: Input::new().with_masked(true),
            ovpn_ca_path: Input::new(),
            validation: Vec::new(),
            first_error: None,
            preview_lines: Vec::new(),
            form_notice: None,
        };
        f.recompute();
        f
    }

    fn from_wireguard_spec(spec: WireGuardSpec, editing_uuid: Option<String>) -> Self {
        let mut f = Self::new_blank(FormProtocol::WireGuard);
        f.editing_uuid = editing_uuid;
        f.name.set(&spec.name);
        f.address.set(&spec.address);
        f.private_key.set(&spec.private_key);
        f.peer_public_key.set(&spec.peer_public_key);
        f.endpoint.set(&spec.endpoint);
        if !spec.allowed_ips.is_empty() {
            f.allowed_ips.set(&spec.allowed_ips);
        }
        f.dns.set(&spec.dns);
        f.derived_pubkey = derive_wg_pubkey(&spec.private_key).unwrap_or_default();
        f.recompute();
        f
    }

    fn from_openvpn_spec(spec: OpenVpnSpec, editing_uuid: Option<String>) -> Self {
        let mut f = Self::new_blank(FormProtocol::OpenVpn);
        f.editing_uuid = editing_uuid;
        f.name.set(&spec.name);
        f.ovpn_remote.set(&spec.remote);
        f.ovpn_tcp = spec.proto_tcp;
        f.ovpn_username.set(&spec.username);
        f.ovpn_password.set(&spec.password);
        if let Some(ca) = spec.ca_path {
            f.ovpn_ca_path.set(ca.to_string_lossy());
        }
        f.recompute();
        f
    }

    fn from_existing_profile(p: &VpnProfile, focus_auth: bool) -> Self {
        let raw = out(&["nmcli", "-s", "-t", "connection", "show", "uuid", &p.uuid])
            .or_else(|| sudo(&["nmcli", "-s", "-t", "connection", "show", "uuid", &p.uuid]).ok())
            .unwrap_or_default();

        match p.kind {
            VpnKind::WireGuard => {
                let mut spec = WireGuardSpec {
                    name: p.name.clone(),
                    address: p.ipv4.clone().unwrap_or_default(),
                    private_key: String::new(),
                    peer_public_key: String::new(),
                    endpoint: p.endpoint.clone().unwrap_or_default(),
                    allowed_ips: "0.0.0.0/0, ::/0".to_string(),
                    dns: String::new(),
                };
                for line in raw.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        let v = v.trim();
                        if v.is_empty() || v == "--" {
                            continue;
                        }
                        match k.trim() {
                            "ipv4.addresses" if spec.address.is_empty() => {
                                spec.address = v.to_string();
                            }
                            "ipv4.dns" => spec.dns = v.to_string(),
                            "wireguard.private-key" => spec.private_key = v.to_string(),
                            _ => {}
                        }
                    }
                }
                Self::from_wireguard_spec(spec, Some(p.uuid.clone()))
            }
            VpnKind::OpenVpn | VpnKind::Other(_) => {
                let mut spec = OpenVpnSpec {
                    name: p.name.clone(),
                    remote: String::new(),
                    port: String::new(),
                    proto_tcp: false,
                    username: String::new(),
                    password: String::new(),
                    ca_path: None,
                };
                let mut host = String::new();
                let mut port = String::new();
                for line in raw.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        let v = v.trim();
                        if k.trim() == "vpn.data" {
                            for pair in v.split(',') {
                                if let Some((dk, dv)) = pair.trim().split_once('=') {
                                    match dk.trim() {
                                        "remote" => host = dv.trim().to_string(),
                                        "port" => port = dv.trim().to_string(),
                                        "proto-tcp" => spec.proto_tcp = dv.trim() == "yes",
                                        "username" => spec.username = dv.trim().to_string(),
                                        "ca" => spec.ca_path = Some(PathBuf::from(dv.trim())),
                                        _ => {}
                                    }
                                }
                            }
                        } else if k.trim() == "vpn.secrets" {
                            for pair in v.split(',') {
                                if let Some((dk, dv)) = pair.trim().split_once('=')
                                    && dk.trim() == "password"
                                {
                                    spec.password = dv.trim().to_string();
                                }
                            }
                        }
                    }
                }
                spec.remote = if !host.is_empty() && !port.is_empty() {
                    format!("{host}:{port}")
                } else {
                    host
                };
                let mut form = Self::from_openvpn_spec(spec, Some(p.uuid.clone()));
                if focus_auth {
                    form.focus = FormFocus::OvpnUsername;
                }
                form
            }
        }
    }

    fn focus_order(&self) -> &'static [FormFocus] {
        match self.protocol {
            FormProtocol::WireGuard => &[
                FormFocus::Protocol,
                FormFocus::Name,
                FormFocus::Address,
                FormFocus::PrivateKey,
                FormFocus::PeerPublicKey,
                FormFocus::Endpoint,
                FormFocus::AllowedIps,
                FormFocus::Dns,
                FormFocus::Save,
                FormFocus::Cancel,
            ],
            FormProtocol::OpenVpn => &[
                FormFocus::Protocol,
                FormFocus::Name,
                FormFocus::OvpnRemote,
                FormFocus::OvpnTransport,
                FormFocus::OvpnUsername,
                FormFocus::OvpnPassword,
                FormFocus::OvpnCaPath,
                FormFocus::Save,
                FormFocus::Cancel,
            ],
        }
    }

    fn step_focus(&mut self, forward: bool) {
        let order = self.focus_order();
        let idx = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward {
            (idx + 1) % order.len()
        } else {
            (idx + order.len() - 1) % order.len()
        };
        self.focus = order[next];
    }

    fn active_input_mut(&mut self) -> Option<&mut Input> {
        match self.focus {
            FormFocus::Name => Some(&mut self.name),
            FormFocus::Address => Some(&mut self.address),
            FormFocus::PrivateKey => Some(&mut self.private_key),
            FormFocus::PeerPublicKey => Some(&mut self.peer_public_key),
            FormFocus::Endpoint => Some(&mut self.endpoint),
            FormFocus::AllowedIps => Some(&mut self.allowed_ips),
            FormFocus::Dns => Some(&mut self.dns),
            FormFocus::OvpnRemote => Some(&mut self.ovpn_remote),
            FormFocus::OvpnUsername => Some(&mut self.ovpn_username),
            FormFocus::OvpnPassword => Some(&mut self.ovpn_password),
            FormFocus::OvpnCaPath => Some(&mut self.ovpn_ca_path),
            FormFocus::Protocol
            | FormFocus::OvpnTransport
            | FormFocus::Save
            | FormFocus::Cancel => None,
        }
    }

    fn wg_spec(&self) -> WireGuardSpec {
        WireGuardSpec {
            name: self.name.text().to_string(),
            address: self.address.text().to_string(),
            private_key: self.private_key.text().to_string(),
            peer_public_key: self.peer_public_key.text().to_string(),
            endpoint: self.endpoint.text().to_string(),
            allowed_ips: self.allowed_ips.text().to_string(),
            dns: self.dns.text().to_string(),
        }
    }

    fn ovpn_spec(&self) -> OpenVpnSpec {
        let ca_raw = self.ovpn_ca_path.text().trim();
        let ca_path = (!ca_raw.is_empty()).then(|| PathBuf::from(ca_raw));
        OpenVpnSpec {
            name: self.name.text().to_string(),
            remote: self.ovpn_remote.text().to_string(),
            port: String::new(),
            proto_tcp: self.ovpn_tcp,
            username: self.ovpn_username.text().to_string(),
            password: self.ovpn_password.text().to_string(),
            ca_path,
        }
    }

    fn recompute(&mut self) {
        match self.protocol {
            FormProtocol::WireGuard => {
                let spec = self.wg_spec();
                self.validation = validate_wireguard_spec(&spec);
                self.first_error = self
                    .validation
                    .iter()
                    .find(|(ok, _)| !ok)
                    .map(|(_, msg)| msg.clone());
                let priv_summary = if spec.private_key.trim().is_empty() {
                    "<missing>".to_string()
                } else {
                    format!("<hidden · {} chars>", spec.private_key.trim().len())
                };
                let peer_summary = if spec.peer_public_key.trim().is_empty() {
                    "<missing>".to_string()
                } else {
                    spec.peer_public_key.trim().to_string()
                };
                let allowed = if spec.allowed_ips.trim().is_empty() {
                    "0.0.0.0/0, ::/0"
                } else {
                    spec.allowed_ips.trim()
                };
                let mut lines = vec![
                    "[Interface]".to_string(),
                    format!("PrivateKey = {priv_summary}"),
                    format!("Address = {}", spec.address.trim()),
                ];
                if !spec.dns.trim().is_empty() {
                    lines.push(format!("DNS = {}", spec.dns.trim()));
                }
                lines.push(String::new());
                lines.push("[Peer]".to_string());
                lines.push(format!("PublicKey = {peer_summary}"));
                lines.push(format!("Endpoint = {}", spec.endpoint.trim()));
                lines.push(format!("AllowedIPs = {allowed}"));
                lines.push("PersistentKeepalive = 25".to_string());
                self.preview_lines = lines;
            }
            FormProtocol::OpenVpn => {
                let spec = self.ovpn_spec();
                self.validation = validate_openvpn_spec(&spec);
                self.first_error = self
                    .validation
                    .iter()
                    .find(|(ok, _)| !ok)
                    .map(|(_, msg)| msg.clone());
                let proto = if spec.proto_tcp { "tcp" } else { "udp" };
                let mut lines = vec![
                    "client".to_string(),
                    "dev tun".to_string(),
                    format!("proto {proto}"),
                    format!(
                        "remote {}",
                        if spec.remote.trim().is_empty() {
                            "<server>"
                        } else {
                            spec.remote.trim()
                        }
                    ),
                ];
                if !spec.username.trim().is_empty() {
                    lines.push(format!("auth-user-pass ({})", spec.username.trim()));
                }
                if let Some(ca) = &spec.ca_path {
                    lines.push(format!("ca {}", ca.display()));
                }
                self.preview_lines = lines;
            }
        }
    }

    fn apply_clipboard_text(&mut self, text: &str) -> bool {
        if let Some(wg) = parse_wireguard_conf(text) {
            self.protocol = FormProtocol::WireGuard;
            if self.name.text().trim().is_empty() {
                self.name.set(&wg.name);
            }
            self.address.set(&wg.address);
            self.private_key.set(&wg.private_key);
            self.derived_pubkey = derive_wg_pubkey(&wg.private_key).unwrap_or_default();
            self.peer_public_key.set(&wg.peer_public_key);
            self.endpoint.set(&wg.endpoint);
            self.allowed_ips.set(&wg.allowed_ips);
            self.dns.set(&wg.dns);
            self.form_notice = Some("Filled WireGuard fields from clipboard".to_string());
            self.recompute();
            return true;
        }
        if let Some(ovpn) = parse_openvpn_conf(text) {
            self.protocol = FormProtocol::OpenVpn;
            if self.name.text().trim().is_empty() {
                self.name.set(&ovpn.name);
            }
            self.ovpn_remote.set(&ovpn.remote);
            self.ovpn_tcp = ovpn.proto_tcp;
            if let Some(ca) = ovpn.ca_path {
                self.ovpn_ca_path.set(ca.to_string_lossy());
            }
            self.form_notice = Some("Filled OpenVPN fields from clipboard".to_string());
            self.recompute();
            return true;
        }
        false
    }

    fn save(&self) -> Result<String, String> {
        match self.protocol {
            FormProtocol::WireGuard => {
                create_wireguard_profile(&self.wg_spec(), self.editing_uuid.as_deref())
            }
            FormProtocol::OpenVpn => {
                create_openvpn_profile(&self.ovpn_spec(), self.editing_uuid.as_deref())
            }
        }
    }
}

// ─── TUI Screen ──────────────────────────────────────────────────────────────

enum VpnPhase {
    Browse,
    EditForm(Box<TunnelForm>),
    ConfirmDelete {
        name: String,
        uuid: String,
        confirm: Confirm,
    },
}

enum VpnRowKind {
    Profile(usize),
    Heading,
    CreateConnection,
    ImportFile,
    PasteConfig,
    InstallOpenVpn,
}

pub enum VpnRequest {
    ImportFile,
    BrowseForForm { form: Box<TunnelForm>, for_ca: bool },
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
    form_hits: RefCell<FormHits>,
    inspector_hits: RefCell<InspectorHits>,
    cursor_blink_on: bool,
    last_blink: Instant,
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
            form_hits: RefCell::new(FormHits::default()),
            inspector_hits: RefCell::new(InspectorHits::default()),
            cursor_blink_on: true,
            last_blink: Instant::now(),
        };
        s.rebuild_rows();
        s
    }

    fn with_form(form: Box<TunnelForm>, banner: Option<String>) -> Self {
        let mut s = Self::new(banner);
        s.phase = VpnPhase::EditForm(form);
        s.update_chrome();
        s
    }

    /// Deterministic snapshot fixture for `tests/screens.rs` (Browse Studio).
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
            form_hits: RefCell::new(FormHits::default()),
            inspector_hits: RefCell::new(InspectorHits::default()),
            cursor_blink_on: true,
            last_blink: Instant::now(),
        };
        s.rebuild_rows();
        s
    }

    /// Deterministic snapshot fixture for `tests/screens.rs` (EditForm).
    pub fn for_form_snapshot() -> Self {
        let mut s = Self::for_snapshot();
        let mut form = TunnelForm::new_blank(FormProtocol::WireGuard);
        form.name.set("work-wg");
        form.address.set("10.66.0.2/32");
        form.private_key
            .set("gI7gS3V1mQ6K9h2x4p8n1r5t7v9w2y4z6a8c0e2g4i8=");
        form.derived_pubkey = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=".to_string();
        form.endpoint.set("vpn.example.com:51820");
        form.dns.set("1.1.1.1");
        form.recompute();
        s.phase = VpnPhase::EditForm(Box::new(form));
        s.update_chrome();
        s
    }

    pub fn refresh(&mut self) {
        self.profiles = list_profiles();
        self.openvpn_ready = openvpn_plugin_installed();
        self.rebuild_rows();
    }

    fn update_chrome(&mut self) {
        match &self.phase {
            VpnPhase::Browse => {
                let active = self.profiles.iter().filter(|p| p.active).count();
                let status = if active > 0 {
                    format!("{active} active \u{00b7} WireGuard & OpenVPN")
                } else if self.profiles.is_empty() {
                    "WireGuard & OpenVPN".to_string()
                } else {
                    format!("{} saved \u{00b7} WireGuard & OpenVPN", self.profiles.len())
                };
                self.header = Header::new("Hayato").subtitle("VPN").status(status);
                self.keybar = vec![
                    KeyHint::new("Enter", "toggle"),
                    KeyHint::new("n", "new"),
                    KeyHint::new("e", "edit"),
                    KeyHint::new("i", "import"),
                    KeyHint::new("v", "paste"),
                    KeyHint::new("a", "auto"),
                    KeyHint::new("d", "delete"),
                    KeyHint::new("Esc", "close"),
                ];
            }
            VpnPhase::EditForm(form) => {
                let verb = if form.editing_uuid.is_some() {
                    "Edit"
                } else {
                    "New"
                };
                let proto = match form.protocol {
                    FormProtocol::WireGuard => "WireGuard",
                    FormProtocol::OpenVpn => "OpenVPN",
                };
                self.header = Header::new("Hayato")
                    .subtitle("VPN")
                    .status(format!("{verb} {proto} Tunnel"));
                self.keybar = match form.protocol {
                    FormProtocol::WireGuard => vec![
                        KeyHint::new("Tab/\u{2191}\u{2193}", "field"),
                        KeyHint::new("Ctrl+S", "save"),
                        KeyHint::new("Ctrl+G", "genkey"),
                        KeyHint::new("Ctrl+Y", "copy pub"),
                        KeyHint::new("Ctrl+V", "paste"),
                        KeyHint::new("Esc", "cancel"),
                    ],
                    FormProtocol::OpenVpn => vec![
                        KeyHint::new("Tab/\u{2191}\u{2193}", "field"),
                        KeyHint::new("Ctrl+S", "save"),
                        KeyHint::new("Ctrl+O", "browse"),
                        KeyHint::new("Ctrl+V", "paste"),
                        KeyHint::new("Ctrl+R", "reveal"),
                        KeyHint::new("Esc", "cancel"),
                    ],
                };
            }
            VpnPhase::ConfirmDelete { .. } => {
                self.header = Header::new("Hayato").subtitle("VPN").status("Delete");
                self.keybar = vec![
                    KeyHint::new("Enter", "confirm"),
                    KeyHint::new("Esc", "cancel"),
                ];
            }
        }
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

        rows.push(Row::heading("Actions"));
        kinds.push(VpnRowKind::Heading);

        rows.push(
            Row::new("New tunnel")
                .detail("WireGuard or OpenVPN form (or Ctrl+V paste)")
                .glyph(Glyph::Network),
        );
        kinds.push(VpnRowKind::CreateConnection);

        rows.push(
            Row::new("Import config")
                .detail("browse .conf / .ovpn with live preview")
                .glyph(Glyph::Folder),
        );
        kinds.push(VpnRowKind::ImportFile);

        rows.push(
            Row::new("Paste config")
                .detail("parse .conf / .ovpn from clipboard")
                .glyph(Glyph::Clipboard),
        );
        kinds.push(VpnRowKind::PasteConfig);

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

    fn open_paste_form(&mut self) {
        let Some(clip) = clipboard_paste() else {
            self.banner = Some("Clipboard is empty (copy a .conf or .ovpn first)".to_string());
            return;
        };
        let mut form = TunnelForm::new_blank(FormProtocol::WireGuard);
        if form.apply_clipboard_text(&clip) {
            self.phase = VpnPhase::EditForm(Box::new(form));
            self.update_chrome();
        } else {
            self.banner =
                Some("Clipboard text is not a WireGuard .conf or OpenVPN .ovpn".to_string());
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
            VpnRowKind::Heading => Outcome::Consumed,
            VpnRowKind::CreateConnection => {
                self.phase =
                    VpnPhase::EditForm(Box::new(TunnelForm::new_blank(FormProtocol::WireGuard)));
                self.update_chrome();
                Outcome::Consumed
            }
            VpnRowKind::ImportFile => {
                self.pending_request = Some(VpnRequest::ImportFile);
                Outcome::Done(0)
            }
            VpnRowKind::PasteConfig => {
                self.open_paste_form();
                Outcome::Consumed
            }
            VpnRowKind::InstallOpenVpn => {
                self.pending_request = Some(VpnRequest::InstallOpenVpn);
                Outcome::Done(0)
            }
        }
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 52) / 100).max(48) as u16;
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

    fn draw_inspector(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let inner = draw_card_with(
            buf,
            area,
            design,
            Some("Tunnel Inspector"),
            ColorRole::Surface,
            design.style(TextStyle::Title),
        );
        let mut hits = InspectorHits::default();
        if inner.width < 20 || inner.height < 8 {
            *self.inspector_hits.borrow_mut() = hits;
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let ok_style = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        let x = inner.x + 1;
        let w = inner.width.saturating_sub(2);

        if let Some(p) = self.selected_profile() {
            put_trunc(
                buf,
                x,
                y,
                if p.active {
                    "ACTIVE TUNNEL"
                } else {
                    "SAVED TUNNEL"
                },
                w,
                accent,
            );
            y += 1;
            put_trunc(
                buf,
                x,
                y,
                &p.name,
                w,
                design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
            );
            y += 2;

            let status_line = if p.active {
                match &p.device {
                    Some(d) => format!("\u{25cf} Connected ({d})"),
                    None => "\u{25cf} Connected".to_string(),
                }
            } else {
                "\u{25cb} Disconnected".to_string()
            };
            put_trunc(buf, x, y, "Status", 13, dim);
            put_trunc(
                buf,
                x + 13,
                y,
                &status_line,
                w.saturating_sub(13),
                if p.active { ok_style } else { body },
            );
            y += 1;

            put_trunc(buf, x, y, "Protocol", 13, dim);
            let proto_desc = match p.kind {
                VpnKind::WireGuard => "WireGuard (kernel / NetworkManager)",
                VpnKind::OpenVpn => "OpenVPN (NetworkManager plugin)",
                VpnKind::Other(ref s) => s.as_str(),
            };
            put_trunc(buf, x + 13, y, proto_desc, w.saturating_sub(13), body);
            y += 1;

            if let Some(ip) = &p.ipv4 {
                put_trunc(buf, x, y, "Address", 13, dim);
                put_trunc(buf, x + 13, y, ip, w.saturating_sub(13), body);
                y += 1;
            }
            if let Some(ep) = &p.endpoint {
                put_trunc(buf, x, y, "Endpoint", 13, dim);
                put_trunc(buf, x + 13, y, ep, w.saturating_sub(13), body);
                y += 1;
            }
            if p.active {
                put_trunc(buf, x, y, "Traffic", 13, dim);
                let io_str = format!(
                    "\u{2191} {} sent \u{00b7} \u{2193} {} received",
                    format_bytes(p.tx_bytes),
                    format_bytes(p.rx_bytes)
                );
                put_trunc(buf, x + 13, y, &io_str, w.saturating_sub(13), body);
                y += 1;
            }
            put_trunc(buf, x, y, "Autoconnect", 13, dim);
            put_trunc(
                buf,
                x + 13,
                y,
                if p.autoconnect {
                    "Enabled (starts on boot)"
                } else {
                    "Manual"
                },
                w.saturating_sub(13),
                body,
            );
            y += 2;

            if y + 2 < inner.bottom() {
                put_trunc(buf, x, y, "QUICK ACTIONS", w, accent);
                y += 1;
                let pill_style = Style::default()
                    .fg(design.color(ColorRole::Text))
                    .bg(design.color(ColorRole::SurfaceAlt));
                let mut cx = x;
                let toggle_lbl = if p.active {
                    " Disconnect "
                } else {
                    " Connect "
                };
                let tw = toggle_lbl.len() as u16;
                put(buf, cx, y, toggle_lbl, tw, pill_style);
                hits.toggle_btn = Rect::new(cx, y, tw, 1);
                cx += tw + 2;

                let edit_lbl = " Edit ";
                let ew = edit_lbl.len() as u16;
                if cx + ew <= inner.right() {
                    put(buf, cx, y, edit_lbl, ew, pill_style);
                    hits.edit_btn = Rect::new(cx, y, ew, 1);
                    cx += ew + 2;
                }

                let auto_lbl = if p.autoconnect {
                    " Auto: On "
                } else {
                    " Auto: Off "
                };
                let aw = auto_lbl.len() as u16;
                if cx + aw <= inner.right() {
                    put(buf, cx, y, auto_lbl, aw, pill_style);
                    hits.auto_btn = Rect::new(cx, y, aw, 1);
                    cx += aw + 2;
                }

                let del_lbl = " Delete ";
                let dw = del_lbl.len() as u16;
                if cx + dw <= inner.right() {
                    let del_style = Style::default()
                        .fg(design.color(ColorRole::Danger))
                        .bg(design.color(ColorRole::SurfaceAlt));
                    put(buf, cx, y, del_lbl, dw, del_style);
                    hits.delete_btn = Rect::new(cx, y, dw, 1);
                }
                y += 2;
            }
        } else {
            put_trunc(buf, x, y, "HAYATO VPN STUDIO", w, accent);
            y += 2;
            put_trunc(buf, x, y, "Manage WireGuard and OpenVPN tunnels", w, body);
            y += 1;
            put_trunc(buf, x, y, "natively through NetworkManager.", w, dim);
            y += 2;
        }

        if y + 2 < inner.bottom() {
            put_trunc(buf, x, y, "SHORTCUTS", w, accent);
            y += 1;
            put_trunc(
                buf,
                x,
                y,
                "n  New tunnel form   e  Edit focused tunnel",
                w,
                dim,
            );
            y += 1;
            put_trunc(
                buf,
                x,
                y,
                "v  Paste .conf/.ovpn i  Import via Files",
                w,
                dim,
            );
        }

        *self.inspector_hits.borrow_mut() = hits;
    }

    fn draw_edit_form(
        form: &TunnelForm,
        blink_on: bool,
        area: Rect,
        buf: &mut Buffer,
        design: &Design,
    ) -> FormHits {
        let (left, right) = Self::split_panes(area);
        let inner = draw_card_with(
            buf,
            left,
            design,
            Some("Tunnel Configuration"),
            ColorRole::Surface,
            design.style(TextStyle::Title),
        );
        let mut hits = FormHits::default();
        if inner.width < 28 || inner.height < 10 {
            return hits;
        }

        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let ok_style = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);
        let err_style = Style::default()
            .fg(design.color(ColorRole::Danger))
            .add_modifier(Modifier::BOLD);

        let x = inner.x + 1;
        let label_w: u16 = 12;
        let field_x = x + label_w;
        let field_w = inner.width.saturating_sub(label_w + 2);
        let mut y = inner.y;

        // ── Protocol Toggle Row ────────────────────────────────────────────
        let proto_focused = form.focus == FormFocus::Protocol;
        put_trunc(
            buf,
            x,
            y,
            "Protocol",
            label_w,
            if proto_focused { accent } else { dim },
        );
        let active_pill = Style::default()
            .fg(design.color(ColorRole::SelectionText))
            .bg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let idle_pill = Style::default()
            .fg(design.color(ColorRole::Text))
            .bg(design.color(ColorRole::Bg));

        let wg_lbl = if form.protocol == FormProtocol::WireGuard {
            " \u{25cf} WireGuard "
        } else {
            "   WireGuard "
        };
        let ovpn_lbl = if form.protocol == FormProtocol::OpenVpn {
            " \u{25cf} OpenVPN "
        } else {
            "   OpenVPN "
        };
        let wg_w = 13u16;
        let ovpn_w = 11u16;
        put(
            buf,
            field_x,
            y,
            wg_lbl,
            wg_w,
            if form.protocol == FormProtocol::WireGuard {
                active_pill
            } else {
                idle_pill
            },
        );
        hits.wg_pill = Rect::new(field_x, y, wg_w, 1);

        put(
            buf,
            field_x + wg_w + 1,
            y,
            ovpn_lbl,
            ovpn_w,
            if form.protocol == FormProtocol::OpenVpn {
                active_pill
            } else {
                idle_pill
            },
        );
        hits.ovpn_pill = Rect::new(field_x + wg_w + 1, y, ovpn_w, 1);
        y += 2;

        let draw_input_row = |buf: &mut Buffer,
                              fields: &mut Vec<(FormFocus, Rect)>,
                              y: u16,
                              label: &str,
                              focus_id: FormFocus,
                              input: &Input,
                              placeholder: &str,
                              right_btn: Option<(&str, &mut Rect)>| {
            let focused = form.focus == focus_id;
            put_trunc(
                buf,
                x,
                y,
                label,
                label_w,
                if focused { accent } else { dim },
            );
            let btn_w = right_btn
                .as_ref()
                .map(|(l, _)| l.len() as u16 + 1)
                .unwrap_or(0);
            let iw = field_w.saturating_sub(btn_w);
            let rect = Rect::new(field_x, y, iw, 1);
            input.render_field(
                rect,
                buf,
                design,
                placeholder,
                focused,
                blink_on,
                ColorRole::Bg,
                TextStyle::Body,
            );
            fields.push((focus_id, rect));
            if let Some((btn_label, out_rect)) = right_btn {
                let bx = field_x + iw + 1;
                let bw = btn_label.len() as u16;
                let btn_style = Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(design.color(ColorRole::SurfaceAlt));
                put(buf, bx, y, btn_label, bw, btn_style);
                *out_rect = Rect::new(bx, y, bw, 1);
            }
        };

        let gap: u16 = if inner.height >= 24 { 2 } else { 1 };

        match form.protocol {
            FormProtocol::WireGuard => {
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Name",
                    FormFocus::Name,
                    &form.name,
                    "work-wg",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Address",
                    FormFocus::Address,
                    &form.address,
                    "10.66.0.2/32",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Priv Key",
                    FormFocus::PrivateKey,
                    &form.private_key,
                    "44-char base64 or Ctrl+G",
                    Some((" GenKey ", &mut hits.genkey_btn)),
                );
                y += 1;
                // Derived Public Key row (always visible right under Private Key!)
                put_trunc(buf, x, y, "Pub Key", label_w, dim);
                let copy_lbl = " Copy ";
                let cw = copy_lbl.len() as u16;
                let pw = field_w.saturating_sub(cw + 1);
                let pub_display = if form.derived_pubkey.is_empty() {
                    "(generated from Private Key via wg)"
                } else {
                    form.derived_pubkey.as_str()
                };
                put_trunc(
                    buf,
                    field_x,
                    y,
                    pub_display,
                    pw,
                    if form.derived_pubkey.is_empty() {
                        dim
                    } else {
                        ok_style
                    },
                );
                if !form.derived_pubkey.is_empty() {
                    let bx = field_x + pw + 1;
                    put(
                        buf,
                        bx,
                        y,
                        copy_lbl,
                        cw,
                        Style::default()
                            .fg(design.color(ColorRole::Accent))
                            .bg(design.color(ColorRole::SurfaceAlt)),
                    );
                    hits.copypub_btn = Rect::new(bx, y, cw, 1);
                }
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Peer Key",
                    FormFocus::PeerPublicKey,
                    &form.peer_public_key,
                    "server WireGuard public key",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Endpoint",
                    FormFocus::Endpoint,
                    &form.endpoint,
                    "vpn.example.com:51820",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "AllowedIPs",
                    FormFocus::AllowedIps,
                    &form.allowed_ips,
                    "0.0.0.0/0, ::/0",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "DNS",
                    FormFocus::Dns,
                    &form.dns,
                    "1.1.1.1 (optional)",
                    None,
                );
                y += gap;
            }
            FormProtocol::OpenVpn => {
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Name",
                    FormFocus::Name,
                    &form.name,
                    "office-ovpn",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Server",
                    FormFocus::OvpnRemote,
                    &form.ovpn_remote,
                    "vpn.example.com:1194",
                    None,
                );
                y += gap;
                let tr_focused = form.focus == FormFocus::OvpnTransport;
                put_trunc(
                    buf,
                    x,
                    y,
                    "Transport",
                    label_w,
                    if tr_focused { accent } else { dim },
                );
                let udp_lbl = if !form.ovpn_tcp {
                    " \u{25cf} UDP "
                } else {
                    "   UDP "
                };
                let tcp_lbl = if form.ovpn_tcp {
                    " \u{25cf} TCP "
                } else {
                    "   TCP "
                };
                put(
                    buf,
                    field_x,
                    y,
                    udp_lbl,
                    7,
                    if !form.ovpn_tcp {
                        active_pill
                    } else {
                        idle_pill
                    },
                );
                put(
                    buf,
                    field_x + 8,
                    y,
                    tcp_lbl,
                    7,
                    if form.ovpn_tcp {
                        active_pill
                    } else {
                        idle_pill
                    },
                );
                hits.fields
                    .push((FormFocus::OvpnTransport, Rect::new(field_x, y, 16, 1)));
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Username",
                    FormFocus::OvpnUsername,
                    &form.ovpn_username,
                    "optional",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "Password",
                    FormFocus::OvpnPassword,
                    &form.ovpn_password,
                    "optional (Ctrl+R reveal)",
                    None,
                );
                y += gap;
                draw_input_row(
                    buf,
                    &mut hits.fields,
                    y,
                    "CA Cert",
                    FormFocus::OvpnCaPath,
                    &form.ovpn_ca_path,
                    "optional .crt/.pem",
                    Some((" Browse ", &mut hits.browse_ca_btn)),
                );
                y += gap;
            }
        }

        // ── Inline Validation / Notice Line ────────────────────────────────
        if y < inner.bottom().saturating_sub(2) {
            if let Some(notice) = &form.form_notice {
                put_trunc(buf, x, y, notice, inner.width.saturating_sub(2), accent);
            } else if let Some(err) = &form.first_error {
                put_trunc(
                    buf,
                    x,
                    y,
                    &format!("\u{2717} {err}"),
                    inner.width.saturating_sub(2),
                    err_style,
                );
            } else {
                put_trunc(
                    buf,
                    x,
                    y,
                    "\u{2713} Configuration valid \u{00b7} ready to save",
                    inner.width.saturating_sub(2),
                    ok_style,
                );
            }
            y += 1;
        }

        // ── Save / Cancel Buttons ──────────────────────────────────────────
        let btn_y = y.min(inner.bottom().saturating_sub(1));
        let save_lbl = " Save Tunnel (Ctrl+S) ";
        let cancel_lbl = " Cancel (Esc) ";
        let sw = save_lbl.len() as u16;
        let cw = cancel_lbl.len() as u16;
        let save_style = if form.focus == FormFocus::Save {
            active_pill
        } else {
            Style::default()
                .fg(design.color(ColorRole::Ok))
                .bg(design.color(ColorRole::SurfaceAlt))
                .add_modifier(Modifier::BOLD)
        };
        let cancel_style = if form.focus == FormFocus::Cancel {
            active_pill
        } else {
            Style::default()
                .fg(design.color(ColorRole::Text))
                .bg(design.color(ColorRole::SurfaceAlt))
        };
        put(buf, x, btn_y, save_lbl, sw, save_style);
        hits.save_btn = Rect::new(x, btn_y, sw, 1);
        put(buf, x + sw + 2, btn_y, cancel_lbl, cw, cancel_style);
        hits.cancel_btn = Rect::new(x + sw + 2, btn_y, cw, 1);

        // ── Right Pane: Live Config Preview & Validation Checklist ─────────
        if let Some(right_area) = right {
            let r_in = draw_card_with(
                buf,
                right_area,
                design,
                Some("Live Config Preview"),
                ColorRole::Surface,
                design.style(TextStyle::Title),
            );
            let rx = r_in.x + 1;
            let rw = r_in.width.saturating_sub(2);
            let mut ry = r_in.y;
            for line in &form.preview_lines {
                if ry >= r_in.bottom().saturating_sub(8) {
                    break;
                }
                let st = if line.starts_with('[') {
                    accent
                } else if line.contains("<missing>") {
                    err_style
                } else {
                    body
                };
                put_trunc(buf, rx, ry, line, rw, st);
                ry += 1;
            }
            ry += 1;
            if ry < r_in.bottom().saturating_sub(2) {
                put_trunc(buf, rx, ry, "VALIDATION", rw, accent);
                ry += 1;
                for (ok, msg) in &form.validation {
                    if ry >= r_in.bottom().saturating_sub(1) {
                        break;
                    }
                    let (mark, st) = if *ok {
                        ("\u{2713}", ok_style)
                    } else {
                        ("\u{2717}", err_style)
                    };
                    put_trunc(buf, rx, ry, &format!("{mark} {msg}"), rw, st);
                    ry += 1;
                }
            }
        }

        hits
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
            VpnPhase::EditForm(_) => "edit-form",
            VpnPhase::ConfirmDelete { .. } => "confirm-delete",
        })
    }

    fn tick(&mut self) -> Outcome {
        if matches!(self.phase, VpnPhase::EditForm(_))
            && self.last_blink.elapsed().as_millis() >= 500
        {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let area = ctx.area;
        if area.width < 30 || area.height < 8 {
            return;
        }

        match &mut self.phase {
            VpnPhase::ConfirmDelete { confirm, .. } => {
                self.confirm_area.set(area);
                confirm.render(area, ctx.buf, ctx.design);
                return;
            }
            VpnPhase::EditForm(form) => {
                let blink = self.cursor_blink_on || ctx.design.reduced_motion();
                let hits = Self::draw_edit_form(form, blink, area, ctx.buf, ctx.design);
                *self.form_hits.borrow_mut() = hits;
                return;
            }
            VpnPhase::Browse => {}
        }

        let (left, right) = Self::split_panes(area);
        let card = draw_card_with(
            ctx.buf,
            left,
            ctx.design,
            Some("Tunnels"),
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

        y += 1;

        // Reserve a 2-line context strip on compact (single-pane) grids.
        let compact_footer = if right.is_none() && card.bottom().saturating_sub(y) >= 6 {
            2
        } else {
            0
        };
        let list_h = card.bottom().saturating_sub(y + compact_footer);
        let list_rect = Rect::new(card.x, y, card.width, list_h);
        self.list_area.set(list_rect);
        self.list.render(list_rect, ctx.buf, ctx.design, true);

        if compact_footer == 2 {
            let fy = card.bottom().saturating_sub(2);
            let rule_style = Style::default().fg(ctx.design.color(ColorRole::Border));
            for col in card.x..card.right() {
                if let Some(cell) = ctx.buf.cell_mut((col, fy)) {
                    cell.set_symbol("\u{2500}").set_style(rule_style);
                }
            }
            let summary = if let Some(p) = self.selected_profile() {
                let mut parts = vec![p.name.clone()];
                if let Some(d) = &p.device {
                    parts.push(d.clone());
                }
                if let Some(ep) = &p.endpoint {
                    parts.push(ep.clone());
                }
                parts.push(if p.autoconnect {
                    "autoconnect on".to_string()
                } else {
                    "manual".to_string()
                });
                parts.join(" \u{00b7} ")
            } else {
                "Press [n] for new tunnel form, [v] to paste config, or [i] to browse files"
                    .to_string()
            };
            put_trunc(
                ctx.buf,
                card.x + 1,
                fy + 1,
                &summary,
                card.width.saturating_sub(2),
                ctx.design.style(TextStyle::Dim),
            );
        }

        if let Some(right_area) = right {
            self.draw_inspector(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();

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

        if let VpnPhase::EditForm(form) = &mut self.phase {
            form.form_notice = None;
            // Control chords inside EditForm
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        match form.save() {
                            Ok(msg) => {
                                self.phase = VpnPhase::Browse;
                                self.banner = Some(msg);
                                self.refresh();
                            }
                            Err(err) => {
                                form.form_notice = Some(err);
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('g') | KeyCode::Char('G') => {
                        if form.protocol == FormProtocol::WireGuard {
                            match generate_wg_keypair() {
                                Ok((priv_k, pub_k)) => {
                                    form.private_key.set(&priv_k);
                                    form.derived_pubkey = pub_k;
                                    form.form_notice = Some(
                                        "Generated keypair · press Ctrl+Y to copy Public Key"
                                            .to_string(),
                                    );
                                    form.recompute();
                                }
                                Err(err) => form.form_notice = Some(err),
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if !form.derived_pubkey.is_empty() {
                            if clipboard_copy(&form.derived_pubkey) {
                                form.form_notice =
                                    Some("Copied WireGuard Public Key to clipboard".to_string());
                            } else {
                                form.form_notice = Some("wl-copy not available".to_string());
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        match form.protocol {
                            FormProtocol::WireGuard => {
                                let m = !form.private_key.is_masked();
                                form.private_key.set_masked(m);
                            }
                            FormProtocol::OpenVpn => {
                                let m = !form.ovpn_password.is_masked();
                                form.ovpn_password.set_masked(m);
                            }
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('v') | KeyCode::Char('V') => {
                        if let Some(clip) = clipboard_paste() {
                            if !form.apply_clipboard_text(&clip)
                                && let Some(input) = form.active_input_mut()
                            {
                                let next = format!("{}{}", input.text(), clip.trim());
                                input.set(next);
                                if form.focus == FormFocus::PrivateKey {
                                    form.derived_pubkey = derive_wg_pubkey(form.private_key.text())
                                        .unwrap_or_default();
                                }
                                form.recompute();
                            }
                        }
                        self.update_chrome();
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('o') | KeyCode::Char('O') => {
                        let for_ca = form.protocol == FormProtocol::OpenVpn
                            && form.focus == FormFocus::OvpnCaPath;
                        let VpnPhase::EditForm(boxed) =
                            std::mem::replace(&mut self.phase, VpnPhase::Browse)
                        else {
                            unreachable!();
                        };
                        self.pending_request = Some(VpnRequest::BrowseForForm {
                            form: boxed,
                            for_ca,
                        });
                        return Outcome::Done(0);
                    }
                    _ => {}
                }
            }

            match key.code {
                KeyCode::Esc => {
                    self.phase = VpnPhase::Browse;
                    self.update_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Tab | KeyCode::Down => {
                    form.step_focus(true);
                    return Outcome::Consumed;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.step_focus(false);
                    return Outcome::Consumed;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                    if form.focus == FormFocus::Protocol =>
                {
                    form.protocol = match form.protocol {
                        FormProtocol::WireGuard => FormProtocol::OpenVpn,
                        FormProtocol::OpenVpn => FormProtocol::WireGuard,
                    };
                    form.recompute();
                    self.update_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                    if form.focus == FormFocus::OvpnTransport =>
                {
                    form.ovpn_tcp = !form.ovpn_tcp;
                    form.recompute();
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    match form.focus {
                        FormFocus::Save => match form.save() {
                            Ok(msg) => {
                                self.phase = VpnPhase::Browse;
                                self.banner = Some(msg);
                                self.refresh();
                            }
                            Err(err) => form.form_notice = Some(err),
                        },
                        FormFocus::Cancel => {
                            self.phase = VpnPhase::Browse;
                            self.update_chrome();
                        }
                        _ => form.step_focus(true),
                    }
                    return Outcome::Consumed;
                }
                _ => {}
            }

            if let Some(input) = form.active_input_mut() {
                match input.handle(key) {
                    InputAction::Changed => {
                        if form.focus == FormFocus::PrivateKey {
                            form.derived_pubkey =
                                derive_wg_pubkey(form.private_key.text()).unwrap_or_default();
                        }
                        form.recompute();
                        return Outcome::Consumed;
                    }
                    InputAction::Submitted => {
                        form.step_focus(true);
                        return Outcome::Consumed;
                    }
                    InputAction::Cancelled => {
                        self.phase = VpnPhase::Browse;
                        self.update_chrome();
                        return Outcome::Consumed;
                    }
                    InputAction::Ignored => {}
                }
            }
            return Outcome::Consumed;
        }

        // ── Browse Phase Keys ──────────────────────────────────────────────
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
        {
            self.open_paste_form();
            return Outcome::Consumed;
        }

        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
                KeyCode::Char('n') => {
                    self.phase = VpnPhase::EditForm(Box::new(TunnelForm::new_blank(
                        FormProtocol::WireGuard,
                    )));
                    self.update_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Char('e') => {
                    if let Some(p) = self.selected_profile().cloned() {
                        let form = TunnelForm::from_existing_profile(&p, false);
                        self.phase = VpnPhase::EditForm(Box::new(form));
                        self.update_chrome();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('u') => {
                    if let Some(p) = self.selected_profile().cloned() {
                        let form = TunnelForm::from_existing_profile(&p, true);
                        self.phase = VpnPhase::EditForm(Box::new(form));
                        self.update_chrome();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('v') => {
                    self.open_paste_form();
                    return Outcome::Consumed;
                }
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
                KeyCode::Char('c') | KeyCode::Char(' ') => return self.activate_selected(),
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
        let in_rect = |r: Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.right()
                && row >= r.y
                && row < r.bottom()
        };
        match &self.phase {
            VpnPhase::ConfirmDelete { confirm, .. } => {
                confirm.hit(self.confirm_area.get(), design, col, row)
            }
            VpnPhase::EditForm(_) => {
                let h = self.form_hits.borrow();
                if in_rect(h.wg_pill) {
                    return Some(100);
                }
                if in_rect(h.ovpn_pill) {
                    return Some(101);
                }
                if in_rect(h.genkey_btn) {
                    return Some(102);
                }
                if in_rect(h.copypub_btn) {
                    return Some(103);
                }
                if in_rect(h.browse_ca_btn) {
                    return Some(104);
                }
                if in_rect(h.save_btn) {
                    return Some(105);
                }
                if in_rect(h.cancel_btn) {
                    return Some(106);
                }
                for (i, (_, r)) in h.fields.iter().enumerate() {
                    if in_rect(*r) {
                        return Some(200 + i);
                    }
                }
                None
            }
            VpnPhase::Browse => {
                let ih = self.inspector_hits.borrow();
                if in_rect(ih.toggle_btn) {
                    return Some(500);
                }
                if in_rect(ih.edit_btn) {
                    return Some(501);
                }
                if in_rect(ih.auto_btn) {
                    return Some(502);
                }
                if in_rect(ih.delete_btn) {
                    return Some(503);
                }
                self.list.hit_row(self.list_area.get(), col, row, design)
            }
        }
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

        if let VpnPhase::EditForm(form) = &mut self.phase {
            match index {
                100 => {
                    form.protocol = FormProtocol::WireGuard;
                    form.recompute();
                    self.update_chrome();
                }
                101 => {
                    form.protocol = FormProtocol::OpenVpn;
                    form.recompute();
                    self.update_chrome();
                }
                102 => match generate_wg_keypair() {
                    Ok((priv_k, pub_k)) => {
                        form.private_key.set(&priv_k);
                        form.derived_pubkey = pub_k;
                        form.form_notice = Some("Generated WireGuard keypair".to_string());
                        form.recompute();
                    }
                    Err(err) => form.form_notice = Some(err),
                },
                103 => {
                    if !form.derived_pubkey.is_empty() && clipboard_copy(&form.derived_pubkey) {
                        form.form_notice = Some("Copied Public Key to clipboard".to_string());
                    }
                }
                104 => {
                    let VpnPhase::EditForm(boxed) =
                        std::mem::replace(&mut self.phase, VpnPhase::Browse)
                    else {
                        unreachable!();
                    };
                    self.pending_request = Some(VpnRequest::BrowseForForm {
                        form: boxed,
                        for_ca: true,
                    });
                    return Outcome::Done(0);
                }
                105 => match form.save() {
                    Ok(msg) => {
                        self.phase = VpnPhase::Browse;
                        self.banner = Some(msg);
                        self.refresh();
                    }
                    Err(err) => form.form_notice = Some(err),
                },
                106 => {
                    self.phase = VpnPhase::Browse;
                    self.update_chrome();
                }
                idx if idx >= 200 => {
                    let field_i = idx - 200;
                    if let Some((focus_id, _)) = self.form_hits.borrow().fields.get(field_i) {
                        form.focus = *focus_id;
                        if *focus_id == FormFocus::OvpnTransport {
                            form.ovpn_tcp = !form.ovpn_tcp;
                            form.recompute();
                        }
                    }
                }
                _ => {}
            }
            return Outcome::Consumed;
        }

        // Inspector quick actions
        match index {
            500 => return self.activate_selected(),
            501 => {
                if let Some(p) = self.selected_profile().cloned() {
                    self.phase =
                        VpnPhase::EditForm(Box::new(TunnelForm::from_existing_profile(&p, false)));
                    self.update_chrome();
                }
                return Outcome::Consumed;
            }
            502 => {
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
            503 => {
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
            _ => {}
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
    let mut resume_form: Option<Box<TunnelForm>> = None;
    loop {
        let mut screen = match resume_form.take() {
            Some(form) => VpnScreen::with_form(form, banner.take()),
            None => VpnScreen::new(banner.take()),
        };
        let code = crate::run_screen(terminal, design, "VPN", &mut screen)?;
        let Some(req) = screen.pending_request.take() else {
            return Ok(code);
        };

        match req {
            VpnRequest::ImportFile => {
                if let Some(chosen) =
                    browse_for_file(terminal, design, "Import VPN (.conf / .ovpn)")?
                {
                    banner = Some(match import_profile(&chosen) {
                        Ok(msg) => msg,
                        Err(err) => err,
                    });
                }
            }
            VpnRequest::BrowseForForm { mut form, for_ca } => {
                let title = if for_ca {
                    "Choose OpenVPN CA (.crt / .pem)"
                } else {
                    "Load VPN config (.conf / .ovpn)"
                };
                if let Some(chosen) = browse_for_file(terminal, design, title)? {
                    if for_ca {
                        form.ovpn_ca_path.set(chosen.to_string_lossy());
                        form.recompute();
                    } else if let Ok(content) = std::fs::read_to_string(&chosen) {
                        if !form.apply_clipboard_text(&content) {
                            form.form_notice = Some(
                                "File is not a recognized WireGuard .conf or OpenVPN .ovpn"
                                    .to_string(),
                            );
                        }
                    }
                }
                resume_form = Some(form);
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

/// Browse for a file using Hayato's native `FilesScreen` (with live preview).
fn browse_for_file(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
    subtitle: &str,
) -> Result<Option<PathBuf>, Failure> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let downloads = home.join("Downloads");
    let start_dir = if downloads.is_dir() {
        downloads
    } else {
        home.clone()
    };
    let state_dir = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/state"))
        .join("hayato");
    let setup = crate::files::browse::Setup {
        dir: start_dir,
        mode: crate::files::browse::Mode::Browse,
        hidden: false,
        home: home.clone(),
        memory: state_dir.join("vpn-browse-last"),
        path_var: std::env::var("PATH").unwrap_or_default(),
    };
    let previewer = crate::files::preview::Previewer::for_session(&home);
    let mut screen = crate::files::browse::FilesScreen::new(setup, Some(previewer));
    let code = crate::run_screen(terminal, design, subtitle, &mut screen)?;
    if code != 0 {
        return Ok(None);
    }
    match screen.chosen {
        Some(crate::files::Chosen::Open(path)) | Some(crate::files::Chosen::Actions(path)) => {
            Ok(Some(path))
        }
        _ => Ok(None),
    }
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

    #[test]
    fn bar_json_collapses_when_idle_and_shows_shield_when_connected() {
        assert_eq!(
            bar_json_for(&[]),
            r#"{"text":"","class":"empty","tooltip":""}"#
        );
        let active = vec![VpnProfile {
            name: "work-wg".to_string(),
            uuid: "1111".to_string(),
            kind: VpnKind::WireGuard,
            active: true,
            device: Some("wg0".to_string()),
            autoconnect: true,
            ipv4: Some("10.66.0.2/32".to_string()),
            rx_bytes: 39_950_745,
            tx_bytes: 4_404_019,
            endpoint: None,
        }];
        let json = bar_json_for(&active);
        assert!(json.contains(r#""class":"connected""#), "{json}");
        assert!(json.contains("VPN // WIREGUARD"), "{json}");
        assert!(json.contains("work-wg (wg0)"), "{json}");
        assert!(json.contains("10.66.0.2/32"), "{json}");
        assert!(json.contains("[LMB] VPN · [RMB] disconnect"), "{json}");
    }

    #[test]
    fn validates_and_builds_wireguard_and_openvpn_configs() {
        let priv_k = "gI7gS3V1mQ6K9h2x4p8n1r5t7v9w2y4z6a8c0e2g4i8=";
        let peer_k = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
        let wg = WireGuardSpec {
            name: "work-wg".to_string(),
            address: "10.66.0.2/32".to_string(),
            private_key: priv_k.to_string(),
            peer_public_key: peer_k.to_string(),
            endpoint: "vpn.example.com:51820".to_string(),
            allowed_ips: "".to_string(),
            dns: "1.1.1.1".to_string(),
        };
        let conf = build_wireguard_config(&wg).unwrap();
        assert!(conf.contains("[Interface]"));
        assert!(conf.contains("DNS = 1.1.1.1"));
        assert!(conf.contains("AllowedIPs = 0.0.0.0/0, ::/0"));

        // Swapped/duplicate key is rejected.
        let mut bad = wg.clone();
        bad.peer_public_key = priv_k.to_string();
        assert!(build_wireguard_config(&bad).is_err());

        // Clipboard round-trip parse.
        let parsed = parse_wireguard_conf(&conf).unwrap();
        assert_eq!(parsed.private_key, priv_k);
        assert_eq!(parsed.peer_public_key, peer_k);
        assert_eq!(parsed.endpoint, "vpn.example.com:51820");

        let ovpn = OpenVpnSpec {
            name: "office".to_string(),
            remote: "vpn.office.org:443".to_string(),
            port: String::new(),
            proto_tcp: true,
            username: "alice".to_string(),
            password: "secret".to_string(),
            ca_path: None,
        };
        let data = build_openvpn_data(&ovpn).unwrap();
        assert!(data.contains("remote=vpn.office.org"));
        assert!(data.contains("port=443"));
        assert!(data.contains("proto-tcp=yes"));
        assert!(data.contains("username=alice"));

        let ovpn_txt = "client\ndev tun\nproto tcp\nremote vpn.office.org 443\n";
        let parsed_ovpn = parse_openvpn_conf(ovpn_txt).unwrap();
        assert_eq!(parsed_ovpn.remote, "vpn.office.org:443");
        assert!(parsed_ovpn.proto_tcp);
    }
}
