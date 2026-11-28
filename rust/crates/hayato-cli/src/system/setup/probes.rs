//! Non-blocking OS & hardware probes for the 8-step Personal Setup Assistant.
//!
//! Collects live machine facts so the assistant discovers what is already
//! configured, what hardware is present, and what actually needs attention.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::system::setup::bundles::{self, BundleStatus};
use crate::system::setup::state::{SetupState, StepId, StepStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepHealth {
    /// Already configured or user-completed.
    Ready,
    /// Action recommended (e.g. default password, firewall off, no Wi-Fi, dummy audio).
    Attention,
    /// Optional / not yet reviewed.
    Pending,
}

#[derive(Clone, Debug, Default)]
pub struct SetupSnapshot {
    pub loaded: bool,
    // Step 1: Welcome
    pub hostname: String,
    pub board: String,
    pub boot_type: String,
    pub kernel: String,
    pub memory_gib: f64,
    pub provisioning: String,
    pub version: String,
    // Step 2: Security
    pub username: String,
    pub default_password_risk: bool,
    pub root_encrypted: bool,
    pub root_fstype: String,
    pub ufw_active: bool,
    pub ssh_enabled: bool,
    pub ssh_keys_count: usize,
    pub vnc_running: bool,
    // Step 3: Connectivity
    pub online: bool,
    pub connection_summary: String,
    pub bt_present: bool,
    pub bt_powered: bool,
    pub vpn_count: usize,
    // Step 4: Personalization
    pub locale: String,
    pub timezone: String,
    pub ntp_synced: bool,
    pub keymap: String,
    pub theme_name: String,
    pub touchpad_present: bool,
    pub installed_fresh: bool,
    // Step 5: Hardware
    pub audio_sink: String,
    pub audio_is_dummy: bool,
    pub mic_muted: bool,
    pub webcam_device: Option<String>,
    pub touch_present: bool,
    pub display_count: usize,
    pub display_summary: String,
    // Step 6: Software
    pub root_free_gb: f64,
    pub default_browser: String,
    pub bundles: Vec<BundleStatus>,
    // Step 7: Privacy
    pub local_llm_present: bool,
    pub whisper_present: bool,
    pub default_agent: String,
    pub clipboard_enabled: bool,
    pub automount_enabled: bool,
    // Step 8: Protection
    pub backup_drives: Vec<(String, PathBuf, f64)>,
    pub existing_backup_found: bool,
    pub aux_swap_configured: bool,
}

impl SetupSnapshot {
    /// Fast deterministic fixture used in headless snapshot tests.
    pub fn sample() -> Self {
        Self {
            loaded: true,
            hostname: "hayato".to_string(),
            board: "Lenovo IdeaPad Duet 3 (google-asurada)".to_string(),
            boot_type: "depthcharge".to_string(),
            kernel: "6.12.0-arm64".to_string(),
            memory_gib: 3.8,
            provisioning: "Pre-built Release Image".to_string(),
            version: crate::version::version().to_string(),
            username: "brunux".to_string(),
            default_password_risk: true,
            root_encrypted: false,
            root_fstype: "ext4".to_string(),
            ufw_active: false,
            ssh_enabled: false,
            ssh_keys_count: 0,
            vnc_running: false,
            online: true,
            connection_summary: "Wi-Fi · HayatoLab".to_string(),
            bt_present: true,
            bt_powered: true,
            vpn_count: 0,
            locale: "en_US.UTF-8".to_string(),
            timezone: "America/Mexico_City".to_string(),
            ntp_synced: true,
            keymap: "us".to_string(),
            theme_name: "Tokyo Night".to_string(),
            touchpad_present: true,
            installed_fresh: false,
            audio_sink: "Built-in Speaker".to_string(),
            audio_is_dummy: false,
            mic_muted: false,
            webcam_device: Some("/dev/video0".to_string()),
            touch_present: true,
            display_count: 1,
            display_summary: "eDP-1 (internal panel)".to_string(),
            root_free_gb: 24.5,
            default_browser: "chromium".to_string(),
            bundles: bundles::probe_all(&Default::default(), false, false),
            local_llm_present: false,
            whisper_present: false,
            default_agent: "Local AI".to_string(),
            clipboard_enabled: true,
            automount_enabled: true,
            backup_drives: Vec::new(),
            existing_backup_found: false,
            aux_swap_configured: false,
        }
    }

    pub fn collect(state: &SetupState) -> Self {
        let mut s = Self {
            loaded: true,
            version: crate::version::version().to_string(),
            ..Self::default()
        };

        // ── Step 1: Machine & Provenance ──────────────────────────────────
        s.hostname = fs::read_to_string("/etc/hostname")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "hayato".to_string());
        s.kernel = run_out("uname", &["-r"]).unwrap_or_else(|| "linux".to_string());
        s.memory_gib = read_mem_gib();
        let (board, boot_type) = read_device_info();
        s.board = board;
        s.boot_type = boot_type;
        let (prov, installed_fresh) = read_provisioning();
        s.provisioning = prov;
        s.installed_fresh = installed_fresh;

        // ── Step 2: Security ──────────────────────────────────────────────
        s.username = std::env::var("USER").unwrap_or_else(|_| "brunux".to_string());
        s.default_password_risk =
            !installed_fresh && !state.password_verified && s.username == "brunux";
        let (enc, fstype) = probe_root_fs();
        s.root_encrypted = enc;
        s.root_fstype = fstype;
        s.ufw_active = run_out("sudo", &["-n", "ufw", "status"])
            .is_some_and(|out| out.contains("Status: active"));
        s.ssh_enabled =
            run_out("systemctl", &["is-enabled", "ssh"]).is_some_and(|out| out.trim() == "enabled");
        let home = home_dir();
        s.ssh_keys_count = fs::read_to_string(home.join(".ssh/authorized_keys"))
            .map(|t| {
                t.lines()
                    .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
                    .count()
            })
            .unwrap_or(0);
        s.vnc_running = run_out("pgrep", &["-x", "wayvnc"]).is_some_and(|o| !o.trim().is_empty());

        // ── Step 3: Connectivity ──────────────────────────────────────────
        let (online, conn_summary) = probe_connectivity();
        s.online = online;
        s.connection_summary = conn_summary;
        s.bt_present = fs::read_dir("/sys/class/bluetooth")
            .map(|rd| rd.flatten().next().is_some())
            .unwrap_or(false);
        s.bt_powered = s.bt_present
            && run_out("bluetoothctl", &["show"]).is_some_and(|o| o.contains("Powered: yes"));
        s.vpn_count = run_out("nmcli", &["-t", "-f", "TYPE", "connection", "show"])
            .map(|o| {
                o.lines()
                    .filter(|l| matches!(l.trim(), "wireguard" | "vpn"))
                    .count()
            })
            .unwrap_or(0);

        // ── Step 4: Personalization ───────────────────────────────────────
        s.locale = std::env::var("LANG")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "en_US.UTF-8".to_string());
        s.timezone = fs::read_to_string("/etc/timezone")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "UTC".to_string());
        s.ntp_synced = run_out("timedatectl", &["show", "-p", "NTPSynchronized", "--value"])
            .is_some_and(|v| v.trim() == "yes");
        s.keymap = crate::system::layout::active_layout().unwrap_or_else(|| "us".to_string());
        s.theme_name = crate::system::appearance::theme_name();
        let (touchpad, touch) = probe_sway_inputs();
        s.touchpad_present = touchpad;
        s.touch_present = touch;

        // ── Step 5: Hardware ──────────────────────────────────────────────
        let (sink, dummy, mic_muted) = probe_audio();
        s.audio_sink = sink;
        s.audio_is_dummy = dummy;
        s.mic_muted = mic_muted;
        s.webcam_device = crate::system::studio::v4l2::find_webcam().map(|w| w.device);
        let (disp_count, disp_summary) = probe_displays();
        s.display_count = disp_count;
        s.display_summary = disp_summary;

        // ── Step 6 & 7: Software & Privacy ────────────────────────────────
        s.root_free_gb = free_gb(Path::new("/"));
        s.default_browser = run_out("xdg-settings", &["get", "default-web-browser"])
            .unwrap_or_else(|| "chromium.desktop".to_string())
            .trim_end_matches(".desktop")
            .to_string();
        s.local_llm_present = crate::ask::local_model_path().is_some_and(|p| p.is_file());
        s.whisper_present = home
            .join(".local/share/hayato/whisper/ggml-tiny.en.bin")
            .is_file()
            || home
                .join(".local/share/hayato/whisper/ggml-base.en.bin")
                .is_file();
        let installed_pkgs = bundles::installed_dpkg_set();
        s.bundles = bundles::probe_all(&installed_pkgs, s.local_llm_present, s.whisper_present);
        s.default_agent = fs::read_to_string(config_dir().join("hayato/agents/default"))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                if s.local_llm_present {
                    "local".to_string()
                } else {
                    "none".to_string()
                }
            });
        s.clipboard_enabled = fs::read_to_string(config_dir().join("hayato/clipboard.conf"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("enabled=").map(|v| v.trim() != "no"))
            })
            .unwrap_or(true);
        s.automount_enabled = fs::read_to_string(config_dir().join("hayato/automount.conf"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("enabled=").map(|v| v.trim() != "no"))
            })
            .unwrap_or(true);

        // ── Step 8: Protection ────────────────────────────────────────────
        s.backup_drives = probe_backup_drives();
        s.existing_backup_found = has_existing_backup(&home, &s.backup_drives);
        s.aux_swap_configured = fs::read_to_string("/etc/fstab")
            .map(|t| t.contains("hayato_swap") || t.lines().any(|l| l.contains(" swap ")))
            .unwrap_or(false);

        s
    }
}

pub fn step_health(step: StepId, snap: &SetupSnapshot, state: &SetupState) -> StepHealth {
    if state.status(step) == StepStatus::Done {
        // Still flag critical live regressions on Security!
        if step == StepId::Security && snap.default_password_risk {
            return StepHealth::Attention;
        }
        return StepHealth::Ready;
    }
    if state.status(step) == StepStatus::Skipped {
        return StepHealth::Pending;
    }
    if !snap.loaded {
        return StepHealth::Pending;
    }
    match step {
        StepId::Welcome => StepHealth::Pending,
        StepId::Security => {
            if snap.default_password_risk || !snap.ufw_active {
                StepHealth::Attention
            } else {
                StepHealth::Ready
            }
        }
        StepId::Connectivity => {
            if snap.online {
                StepHealth::Ready
            } else {
                StepHealth::Attention
            }
        }
        StepId::Personalization => {
            if snap.installed_fresh {
                StepHealth::Ready
            } else {
                StepHealth::Pending
            }
        }
        StepId::Hardware => {
            if snap.audio_is_dummy {
                StepHealth::Attention
            } else {
                StepHealth::Pending
            }
        }
        StepId::Software => StepHealth::Pending,
        StepId::Privacy => StepHealth::Pending,
        StepId::Protection => {
            if snap.existing_backup_found {
                StepHealth::Ready
            } else {
                StepHealth::Pending
            }
        }
    }
}

pub fn run_out(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

fn read_mem_gib() -> f64 {
    let Ok(raw) = fs::read_to_string("/proc/meminfo") else {
        return 0.0;
    };
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<f64>().ok())
            {
                return kb / (1024.0 * 1024.0);
            }
        }
    }
    0.0
}

fn read_device_info() -> (String, String) {
    let mut name = String::new();
    let mut boot = String::new();
    if let Ok(raw) = fs::read_to_string("/etc/hayato/deviceinfo") {
        for line in raw.lines() {
            if let Some(v) = line.strip_prefix("deviceinfo_name=") {
                name = v.trim_matches('"').trim().to_string();
            } else if let Some(v) = line.strip_prefix("deviceinfo_boot_type=") {
                boot = v.trim_matches('"').trim().to_string();
            }
        }
    }
    if name.is_empty() {
        name = fs::read_to_string("/sys/firmware/devicetree/base/model")
            .ok()
            .map(|s| s.trim_matches('\0').trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    }
    if boot.is_empty() {
        boot = if Path::new("/sys/firmware/efi").is_dir() {
            "uefi".to_string()
        } else {
            "depthcharge".to_string()
        };
    }
    (name, boot)
}

fn read_provisioning() -> (String, bool) {
    let Ok(raw) = fs::read_to_string("/etc/hayato/installation") else {
        return ("Pre-built Release Image".to_string(), false);
    };
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
        let mode = val
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("fresh")
            .to_ascii_lowercase();
        if mode == "clone" {
            return ("Cloned Installation".to_string(), false);
        }
        return ("Fresh On-Device Install".to_string(), true);
    }
    ("Installed System".to_string(), true)
}

fn probe_root_fs() -> (bool, String) {
    let fstype = run_out("findmnt", &["-no", "FSTYPE", "/"]).unwrap_or_else(|| "ext4".to_string());
    let src = run_out("findmnt", &["-no", "SOURCE", "/"]).unwrap_or_default();
    let encrypted = src.contains("/mapper/")
        || fs::read_to_string("/etc/crypttab")
            .map(|t| {
                t.lines()
                    .any(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
            })
            .unwrap_or(false);
    (encrypted, fstype)
}

fn probe_connectivity() -> (bool, String) {
    let Some(out) = run_out("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]) else {
        return (false, "Offline".to_string());
    };
    for line in out.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[1] == "connected" && parts[0] != "loopback" {
            let kind = match parts[0] {
                "wifi" => "Wi-Fi",
                "ethernet" => "Ethernet",
                other => other,
            };
            return (true, format!("{kind} · {}", parts[2]));
        }
    }
    (false, "Not connected".to_string())
}

fn probe_sway_inputs() -> (bool, bool) {
    let Some(raw) = run_out("swaymsg", &["-t", "get_inputs", "-r"]) else {
        return (true, false);
    };
    let Ok(val) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return (true, false);
    };
    let touchpad = val
        .iter()
        .any(|i| i.get("type").and_then(|v| v.as_str()) == Some("touchpad"));
    let touch = val
        .iter()
        .any(|i| i.get("type").and_then(|v| v.as_str()) == Some("touch"));
    (touchpad, touch)
}

fn probe_audio() -> (String, bool, bool) {
    let sink = run_out("pactl", &["get-default-sink"]).unwrap_or_else(|| "unknown".to_string());
    let dummy = sink.contains("auto_null") || sink == "unknown";
    let label = if dummy {
        "Dummy Output (no audio hardware)".to_string()
    } else {
        sink
    };
    let mic_muted = run_out("pactl", &["get-source-mute", "@DEFAULT_SOURCE@"])
        .is_some_and(|o| o.contains("yes"));
    (label, dummy, mic_muted)
}

fn probe_displays() -> (usize, String) {
    let Some(raw) = run_out("swaymsg", &["-t", "get_outputs", "-r"]) else {
        return (1, "Internal panel".to_string());
    };
    let Ok(val) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return (1, "Internal panel".to_string());
    };
    let active: Vec<String> = val
        .iter()
        .filter(|o| o.get("active").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|o| o.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let count = active.len().max(1);
    let summary = if active.is_empty() {
        "Internal panel".to_string()
    } else {
        active.join(" + ")
    };
    (count, summary)
}

pub fn free_gb(path: &Path) -> f64 {
    let Some(out) = run_out("df", &["-B1", "--output=avail", &path.to_string_lossy()]) else {
        return 0.0;
    };
    out.lines()
        .nth(1)
        .and_then(|l| l.trim().parse::<f64>().ok())
        .map(|b| b / (1024.0 * 1024.0 * 1024.0))
        .unwrap_or(0.0)
}

pub fn probe_backup_drives() -> Vec<(String, PathBuf, f64)> {
    let Some(raw) = run_out(
        "lsblk",
        &["-J", "-o", "NAME,RM,HOTPLUG,TRAN,MOUNTPOINT,LABEL"],
    ) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let mut drives = Vec::new();
    let Some(devs) = val.get("blockdevices").and_then(|v| v.as_array()) else {
        return drives;
    };
    for dev in devs {
        let removable = dev.get("rm").and_then(|v| v.as_bool()).unwrap_or(false)
            || dev
                .get("hotplug")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            || matches!(
                dev.get("tran").and_then(|v| v.as_str()),
                Some("usb" | "mmc" | "sd")
            );
        if !removable {
            continue;
        }
        let mut nodes = vec![dev];
        if let Some(children) = dev.get("children").and_then(|v| v.as_array()) {
            nodes.extend(children.iter());
        }
        for n in nodes {
            let Some(mp) = n
                .get("mountpoint")
                .and_then(|v| v.as_str())
                .filter(|m| !m.is_empty() && *m != "/" && !m.starts_with("/boot"))
            else {
                continue;
            };
            let label = n
                .get("label")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| n.get("name").and_then(|v| v.as_str()))
                .unwrap_or("Drive")
                .to_string();
            let path = PathBuf::from(mp);
            let avail = free_gb(&path);
            drives.push((label, path, avail));
        }
    }
    drives
}

fn has_existing_backup(home: &Path, drives: &[(String, PathBuf, f64)]) -> bool {
    let mut dirs = vec![home.join("Backups"), home.join("Downloads")];
    for (_, mp, _) in drives {
        dirs.push(mp.clone());
    }
    for dir in dirs {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("hayato-profile-") || name.starts_with("hayato-backup-") {
                    return true;
                }
            }
        }
    }
    false
}
