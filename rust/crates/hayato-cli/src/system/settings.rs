//! Quick settings: the switches and panels of the session in one list (`hayato-settings`).
//!
//! Every row states what is on and what pressing it does. It re-draws itself
//! after a switch, so the state in the list is never stale. It leaves for the
//! pickers and panels that already exist (bar-net, bar-bt, bar-audio, bar-power)
//! instead of duplicating them. The two radio switches (Wi-Fi, Bluetooth) live
//! here as well because that is the one thing the pickers do not answer at a
//! glance.
//!
//! Nothing here is a daemon: every draw reads the state from nmcli,
//! bluetoothctl, swaymsg, makoctl, the night-light helper and sysfs.

use std::cell::{Cell, RefCell};
use std::process::{Command, Stdio};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Screen,
    TextStyle, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::pick::{Picker, Question, row};

use super::{checked, out, sudo};
use crate::installer::config::validate_password;

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["nmcli", "bluetoothctl", "swaymsg", "iw"];

fn bin_script(name: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    format!("{home}/.config/bin/{name}")
}

/// The canonical rows of the Settings menu (`key`, `label`, `runs`), exposed
/// for `hayato menu settings --rows` so `check-system.sh` never greps an ELF.
pub fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("wifi-radio", "Wi-Fi radio", "nmcli radio wifi"),
        ("wifi", "Wi-Fi networks", "bar-net"),
        ("portal", "Network sign-in", "hayato menu net --portal"),
        ("bt-radio", "Bluetooth radio", "bluetoothctl power"),
        ("bt", "Bluetooth devices", "bar-bt"),
        ("dns", "DNS", "hayato-dns"),
        ("night", "Night light", "night-light apply"),
        ("notify", "Notifications", "notify.conf"),
        ("dnd", "Do not disturb", "makoctl mode -t do-not-disturb"),
        ("vnc", "Share this screen", "hayato-vnc"),
        ("ssh", "Remote login (SSH)", "hayato-ssh"),
        ("tz", "Date & time", "hayato-tz"),
        ("locale", "Language & region", "localectl"),
        ("hostname", "Hostname", "hostnamectl"),
        ("default-apps", "Default apps", "xdg-mime"),
        ("password", "Change password", "chpasswd"),
        ("clipboard", "Clipboard history", "clipboard.conf"),
        ("touchpad", "Touchpad", "touchpad.conf"),
        ("rotation", "Rotation", "swaymsg output transform"),
        ("display", "Display arrangement", "hayato-display"),
        ("workspaces", "Workspaces per monitor", "35-workspaces.conf"),
        ("theme", "Theme", "hayato-theme-picker"),
        ("wallpaper", "Wallpaper", "hayato menu files ~/Pictures"),
        (
            "wallpaper-next",
            "Next preset",
            "hayato theme wallpaper next",
        ),
        (
            "wallpaper-theme",
            "Use theme wallpaper",
            "36-custom-wallpaper.conf",
        ),
        (
            "bar-position",
            "Bar position",
            "~/.config/waybar/config.jsonc",
        ),
        (
            "bar-density",
            "Bar density",
            "~/.config/waybar/config.jsonc",
        ),
        (
            "bar-modules",
            "Bar modules",
            "modules-right in config.jsonc",
        ),
        ("corners", "Window corners", "swaymsg corner_radius"),
        ("shadows", "Window shadows", "swaymsg shadows"),
        ("blur", "Window blur", "swaymsg blur"),
        ("animations", "Animations", "swaymsg animation_duration_ms"),
        ("greeter", "Login screen (Greeter)", "hayato menu greeter"),
        ("automount", "Auto-mount USB drives", "automount.conf"),
        ("idle", "Idle & lock", "hayato-idle"),
        ("updates", "System updates", "hayato-pkg"),
        ("clean", "Free space", "hayato-clean"),
        ("tablet", "Tablet mode", "tablet-mode toggle"),
        (
            "tablet-calibrate",
            "Calibrate rotation",
            "tablet-mode calibrate",
        ),
        ("layout", "Keyboard layout", "hayato-layout"),
        ("reduced-motion", "Reduced motion", "reduced-motion.conf"),
        ("translate", "Translate", "translate config"),
        ("audio", "Audio output", "bar-audio"),
        ("mixer", "Volume per application", "pavucontrol"),
        ("media", "Media", "hayato-media"),
        ("power", "Battery", "bar-power"),
    ]
}

/// The top-level sections, for `hayato menu settings --sections` and for an
/// agent that needs to know where a setting lives.
pub fn sections() -> Vec<(&'static str, &'static str, &'static str)> {
    Section::ALL
        .iter()
        .map(|section| (section.key(), section.label(), section.detail()))
        .collect()
}

/// The `--status` output (not used as a separate subcommand, but the Settings
/// row in the menu shows the same probes).
pub fn status() -> String {
    let wifi = if wifi_on() { "on" } else { "off" };
    let bt = if bt_on() { "on" } else { "off" };
    let night = if night_on() { "on" } else { "off" };
    let dnd = if dnd_on() { "dnd" } else { "allowed" };
    let tp = if touchpad_on() { "enabled" } else { "disabled" };
    let am = if automount_on() { "on" } else { "off" };
    format!(
        "Wi-Fi: {} · BT: {} · Night: {} · DND: {} · TP: {} · AM: {} · Appearance: {}",
        wifi,
        bt,
        night,
        dnd,
        tp,
        am,
        super::appearance::status()
    )
}

fn wifi_on() -> bool {
    out(&["nmcli", "radio", "wifi"])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false)
}

fn wifi_if() -> Option<String> {
    out(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device", "status"]).and_then(|s| {
        s.lines().find_map(|l| {
            let parts: Vec<&str> = l.split(':').collect();
            if parts.len() == 2 && parts[1] == "wifi" {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
    })
}

fn ssid() -> Option<String> {
    wifi_if().and_then(|dev| {
        out(&["iw", "dev", &dev, "link"]).and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("SSID:").map(|s| s.trim().to_string()))
        })
    })
}

fn wifi_detail() -> String {
    if !wifi_on() {
        "off".to_string()
    } else if let Some(s) = ssid() {
        s
    } else {
        "not connected".to_string()
    }
}

fn bt_on() -> bool {
    // busctl is one property read; bluetoothctl connects a whole D-Bus client
    // and asks bluez for everything, which is the slow half of this panel.
    let powered = out(&[
        "busctl",
        "--system",
        "get-property",
        "org.bluez",
        "/org/bluez/hci0",
        "org.bluez.Adapter1",
        "Powered",
    ])
    .and_then(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix('b').map(|s| s.trim().to_string()))
    })
    .map(|s| s == "true");
    if let Some(p) = powered {
        return p;
    }
    out(&["bluetoothctl", "show"])
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Powered:").map(|s| s.trim().to_string()))
        })
        .map(|s| s == "yes")
        .unwrap_or(false)
}

fn bt_detail() -> String {
    if !bt_on() {
        "off".to_string()
    } else {
        let n = out(&["bluetoothctl", "devices", "Connected"])
            .map(|s| s.lines().count())
            .unwrap_or(0);
        match n {
            0 => "on, nothing connected".to_string(),
            1 => "1 device connected".to_string(),
            _ => format!("{} devices connected", n),
        }
    }
}

fn night_on() -> bool {
    out(&[&bin_script("night-light"), "status"])
        .map(|s| s.starts_with("on"))
        .unwrap_or(false)
}

fn dnd_path() -> std::path::PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(runtime)
        .join("hayato")
        .join("notify")
        .join("dnd")
}

fn dnd_on() -> bool {
    dnd_path().exists()
}

fn touchpad_on() -> bool {
    out(&["swaymsg", "-t", "get_inputs", "-r"])
        .and_then(|s| {
            let v: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&s);
            v.ok().and_then(|inputs| {
                inputs
                    .iter()
                    .filter(|i| i["type"].as_str() == Some("touchpad"))
                    .map(|i| i["libinput"]["send_events"].as_str().unwrap_or("enabled"))
                    .collect::<Vec<_>>()
                    .first()
                    .map(|s| *s != "disabled")
            })
        })
        .unwrap_or(true)
}

fn focused_output() -> String {
    out(&["swaymsg", "-t", "get_outputs", "-r"])
        .and_then(|s| {
            let v: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&s);
            v.ok().and_then(|outputs| {
                outputs
                    .iter()
                    .find(|o| o["focused"].as_bool().unwrap_or(false))
                    .and_then(|o| o["name"].as_str())
                    .map(|s| s.to_string())
            })
        })
        .unwrap_or_else(|| "eDP-1".to_string())
}

fn rotation_name() -> String {
    out(&["swaymsg", "-t", "get_outputs", "-r"])
        .and_then(|s| {
            let v: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&s);
            v.ok().and_then(|outputs| {
                outputs
                    .iter()
                    .find(|o| o["focused"].as_bool().unwrap_or(false))
                    .and_then(|o| o["transform"].as_str())
                    .map(|s| s.to_string())
            })
        })
        .unwrap_or_else(|| "normal".to_string())
}

fn onscreen_keyboard_running() -> bool {
    out(&["pgrep", "-x", "wvkbd-mobintl"]).is_some_and(|out| !out.trim().is_empty())
}

fn tablet_mode_detail() -> String {
    out(&[&bin_script("tablet-mode"), "status"]).unwrap_or_else(|| "off".to_string())
}

fn automount_conf() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(".config")
        })
        .join("hayato/automount.conf")
}

fn automount_on() -> bool {
    std::fs::read_to_string(automount_conf())
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("enabled=").map(|s| s.to_string()))
        })
        .map(|v| v != "no")
        .unwrap_or(true)
}

fn clipboard_conf_path() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(".config")
        })
        .join("hayato/clipboard.conf")
}

fn clipboard_enabled() -> bool {
    std::fs::read_to_string(clipboard_conf_path())
        .map(|text| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix("enabled="))
                .map(|value| value != "no")
                .unwrap_or(true)
        })
        .unwrap_or(true)
}

fn set_clipboard_enabled(enabled: bool) -> std::io::Result<()> {
    let path = clipboard_conf_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(
        &tmp,
        format!("enabled={}\n", if enabled { "yes" } else { "no" }),
    )?;
    std::fs::rename(tmp, path)
}

fn clipboard_status() -> String {
    let status = out(&[&bin_script("hayato-clipboard"), "status"]).unwrap_or_default();
    let enabled = status.contains("enabled=yes");
    let running = status.contains("running=yes");
    match (enabled, running) {
        (true, true) => "on · running".to_string(),
        (true, false) => "on · not running".to_string(),
        _ => "off".to_string(),
    }
}

fn reduced_motion_path() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                .join(".config")
        })
        .join("hayato/reduced-motion")
}

fn reduced_motion_on() -> bool {
    std::fs::read_to_string(reduced_motion_path())
        .map(|text| matches!(text.trim(), "1" | "yes" | "true"))
        .unwrap_or(false)
}

fn set_reduced_motion(on: bool) -> std::io::Result<()> {
    let path = reduced_motion_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, if on { "1\n" } else { "0\n" })
}

fn audio_detail() -> String {
    out(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"])
        .map(|v| {
            if v.contains("MUTED") {
                "muted".to_string()
            } else {
                v.strip_prefix("Volume: ")
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<f32>().ok())
                    .map(|f| format!("{}%", (f * 100.0 + 0.5) as u32))
                    .unwrap_or("unknown".to_string())
            }
        })
        .unwrap_or("unknown".to_string())
}

fn media_detail() -> String {
    out(&[
        "playerctl",
        "metadata",
        "--format",
        "{{artist}} — {{title}}",
    ])
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "nothing playing".to_string())
}

fn battery_detail() -> String {
    let c = std::fs::read_to_string("/sys/class/power_supply/sbs-8-000b/capacity").ok();
    let s = std::fs::read_to_string("/sys/class/power_supply/sbs-8-000b/status").ok();
    match (c, s) {
        (Some(cap), Some(sta)) => format!("{}% · {}", cap.trim(), sta.trim().to_lowercase()),
        _ => String::new(),
    }
}

/// Build the rows for the settings menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Network,
    DisplaySound,
    Appearance,
    Input,
    PowerIdle,
    System,
    Accessibility,
}

impl Section {
    const ALL: [Self; 7] = [
        Self::Network,
        Self::DisplaySound,
        Self::Appearance,
        Self::Input,
        Self::PowerIdle,
        Self::System,
        Self::Accessibility,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::DisplaySound => "display-sound",
            Self::Appearance => "appearance",
            Self::Input => "input",
            Self::PowerIdle => "power-idle",
            Self::System => "system",
            Self::Accessibility => "accessibility",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.key() == key)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Network => "Network",
            Self::DisplaySound => "Display & Sound",
            Self::Appearance => "Appearance",
            Self::Input => "Input",
            Self::PowerIdle => "Power & Idle",
            Self::System => "System",
            Self::Accessibility => "Accessibility",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Network => "Wi-Fi, Bluetooth, DNS, SSH, screen sharing",
            Self::DisplaySound => "Night light, display, rotation, audio, media",
            Self::Appearance => "Theme, wallpaper, bar, windows, animations",
            Self::Input => "Touchpad, keyboard, tablet mode",
            Self::PowerIdle => "Battery, idle & lock, power menu",
            Self::System => "Date & time, updates, storage, notifications, login",
            Self::Accessibility => "Reduced motion, on-screen keyboard",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Network => "Net",
            Self::DisplaySound => "Disp",
            Self::Appearance => "Look",
            Self::Input => "Input",
            Self::PowerIdle => "Power",
            Self::System => "Sys",
            Self::Accessibility => "A11y",
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Self::Network => Glyph::Wifi,
            Self::DisplaySound => Glyph::Display,
            Self::Appearance => Glyph::Image,
            Self::Input => Glyph::Keyboard,
            Self::PowerIdle => Glyph::Battery,
            Self::System => Glyph::Gear,
            Self::Accessibility => Glyph::Eye,
        }
    }
}

fn section_keys(section: Section) -> &'static [&'static str] {
    match section {
        Section::Network => &[
            "wifi-radio",
            "wifi",
            "portal",
            "bt-radio",
            "bt",
            "dns",
            "vnc",
            "ssh",
        ],
        Section::DisplaySound => &[
            "night",
            "rotation",
            "display",
            "workspaces",
            "audio",
            "mixer",
            "media",
        ],
        Section::Appearance => &[
            "theme",
            "wallpaper",
            "wallpaper-next",
            "wallpaper-theme",
            "bar-position",
            "bar-density",
            "bar-modules",
            "corners",
            "shadows",
            "blur",
            "animations",
        ],
        Section::Input => &[
            "touchpad",
            "layout",
            "controller",
            "tablet",
            "tablet-calibrate",
        ],
        Section::PowerIdle => &["power", "idle"],
        Section::System => &[
            "tz",
            "locale",
            "hostname",
            "default-apps",
            "password",
            "clipboard",
            "notify",
            "dnd",
            "updates",
            "clean",
            "automount",
            "greeter",
            "translate",
        ],
        Section::Accessibility => &["reduced-motion", "onscreen-keyboard"],
    }
}

fn section_rows() -> (Vec<hayato_ui::Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    for section in Section::ALL {
        rows.push(row(section.label(), section.detail(), section.glyph()));
        keys.push(format!("section:{}", section.key()));
    }
    (rows, keys)
}

fn back_row(rows: &mut Vec<hayato_ui::Row>, keys: &mut Vec<String>) {
    rows.push(row("Back", "Settings sections", Glyph::ArrowLeft));
    keys.push("back".to_string());
}

fn build_rows(section: Section) -> (Vec<hayato_ui::Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    match section {
        Section::Network => {
            // Wi-Fi
            rows.push(row(
                "Wi-Fi radio",
                if wifi_on() { "on" } else { "off" },
                Glyph::Wifi,
            ));
            rows.push(row("Wi-Fi networks", wifi_detail(), Glyph::Globe));
            keys.push("wifi-radio".to_string());
            keys.push("wifi".to_string());
            if super::portal::needs_signin() {
                rows.push(row(
                    "Network sign-in",
                    "the network asks for a page",
                    Glyph::Globe,
                ));
                keys.push("portal".to_string());
            }

            // Bluetooth
            rows.push(row(
                "Bluetooth radio",
                if bt_on() { "on" } else { "off" },
                Glyph::Bluetooth,
            ));
            rows.push(row("Bluetooth devices", bt_detail(), Glyph::Link));
            keys.push("bt-radio".to_string());
            keys.push("bt".to_string());

            rows.push(row(
                "DNS",
                "the resolver this connection uses",
                Glyph::Cloud,
            ));
            keys.push("dns".to_string());

            rows.push(row("Share this screen", super::vnc::status(), Glyph::Eye));
            keys.push("vnc".to_string());

            rows.push(row(
                "Remote login (SSH)",
                "remote login status",
                Glyph::Server,
            ));
            keys.push("ssh".to_string());
        }
        Section::DisplaySound => {
            rows.push(row(
                "Night light",
                super::night_light::status(),
                Glyph::Moon,
            ));
            keys.push("night".to_string());

            rows.push(row("Rotation", rotation_name(), Glyph::Rotate));
            keys.push("rotation".to_string());

            rows.push(row(
                "Display arrangement",
                super::display::status(),
                Glyph::Display,
            ));
            keys.push("display".to_string());

            rows.push(row(
                "Workspaces per monitor",
                super::workspaces::status(),
                Glyph::Window,
            ));
            keys.push("workspaces".to_string());

            rows.push(row("Audio output", audio_detail(), Glyph::Volume));
            keys.push("audio".to_string());

            rows.push(row("Volume per application", "pavucontrol", Glyph::Volume));
            keys.push("mixer".to_string());

            rows.push(row("Media", media_detail(), Glyph::Music));
            keys.push("media".to_string());
        }
        Section::Appearance => {
            rows.push(row("Theme", super::appearance::theme_name(), Glyph::Theme));
            keys.push("theme".to_string());

            rows.push(row(
                "Wallpaper",
                super::appearance::wallpaper_name(),
                Glyph::Image,
            ));
            keys.push("wallpaper".to_string());

            rows.push(row(
                "Next preset",
                "cycle the theme's wallpapers",
                Glyph::Next,
            ));
            keys.push("wallpaper-next".to_string());

            if super::appearance::has_custom_wallpaper() {
                rows.push(row(
                    "Use theme wallpaper",
                    "back to the theme preset",
                    Glyph::Undo,
                ));
                keys.push("wallpaper-theme".to_string());
            }

            rows.push(row(
                "Bar position",
                super::appearance::bar_position_label(),
                Glyph::Display,
            ));
            keys.push("bar-position".to_string());

            rows.push(row(
                "Bar density",
                super::appearance::bar_density_label(),
                Glyph::Display,
            ));
            keys.push("bar-density".to_string());

            rows.push(row(
                "Bar modules",
                super::appearance::bar_modules_label(),
                Glyph::Display,
            ));
            keys.push("bar-modules".to_string());

            rows.push(row(
                "Window corners",
                super::appearance::corners_label(),
                Glyph::Window,
            ));
            keys.push("corners".to_string());

            rows.push(row(
                "Window shadows",
                super::appearance::shadows_label(),
                Glyph::Window,
            ));
            keys.push("shadows".to_string());

            rows.push(row(
                "Window blur",
                super::appearance::blur_label(),
                Glyph::Window,
            ));
            keys.push("blur".to_string());

            rows.push(row(
                "Animations",
                super::appearance::animation_label(),
                Glyph::Sync,
            ));
            keys.push("animations".to_string());
        }
        Section::Input => {
            rows.push(row("Touchpad", super::touchpad::status(), Glyph::Mouse));
            keys.push("touchpad".to_string());

            let layout = out(&["swaymsg", "-t", "get_inputs", "-r"])
                .and_then(|s| {
                    let v: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&s);
                    v.ok().and_then(|inputs| {
                        inputs
                            .iter()
                            .find(|i| i["type"].as_str() == Some("keyboard"))
                            .and_then(|i| i["xkb_active_layout_name"].as_str())
                            .map(|s| s.to_string())
                    })
                })
                .unwrap_or_else(|| "unknown".to_string());
            rows.push(row("Keyboard layout", layout, Glyph::Keyboard));
            keys.push("layout".to_string());

            rows.push(row("Controller", crate::padmap::status(), Glyph::Sliders));
            keys.push("controller".to_string());

            let tm = tablet_mode_detail();
            rows.push(row("Tablet mode", tm.clone(), Glyph::Keyboard));
            keys.push("tablet".to_string());
            if tm != "off" {
                let calib = out(&[&bin_script("tablet-mode"), "calibration"])
                    .unwrap_or_else(|| "unknown".to_string());
                rows.push(row("Calibrate rotation", calib, Glyph::Rotate));
                keys.push("tablet-calibrate".to_string());
            }
        }
        Section::PowerIdle => {
            rows.push(row("Battery", battery_detail(), Glyph::Battery));
            keys.push("power".to_string());
            rows.push(row("Idle & lock", super::idle::status(), Glyph::Clock));
            keys.push("idle".to_string());
        }
        Section::System => {
            rows.push(row("Date & time", super::tz::status(), Glyph::Clock));
            keys.push("tz".to_string());

            rows.push(row(
                "Language & region",
                super::locale::current(),
                Glyph::Globe,
            ));
            keys.push("locale".to_string());

            rows.push(row("Hostname", super::hostname::current(), Glyph::Server));
            keys.push("hostname".to_string());

            rows.push(row(
                "Default apps",
                super::default_apps::summary(),
                Glyph::Globe,
            ));
            keys.push("default-apps".to_string());

            rows.push(row("Change password", "this account", Glyph::Key));
            keys.push("password".to_string());

            rows.push(row(
                "Clipboard history",
                clipboard_status(),
                Glyph::Clipboard,
            ));
            keys.push("clipboard".to_string());

            if dnd_on() {
                rows.push(row("Notifications", "do not disturb", Glyph::BellOff));
                rows.push(row("Do not disturb", "on", Glyph::BellOff));
            } else {
                rows.push(row(
                    "Notifications",
                    super::notify_settings::status(),
                    Glyph::Bell,
                ));
                rows.push(row("Do not disturb", "off", Glyph::Bell));
            }
            keys.push("notify".to_string());
            keys.push("dnd".to_string());

            rows.push(row(
                "System updates",
                "apt: search, install, upgrade",
                Glyph::Sync,
            ));
            keys.push("updates".to_string());

            rows.push(row("Free space", "caches, journal and trash", Glyph::Trash));
            keys.push("clean".to_string());

            if automount_on() {
                rows.push(row(
                    "Auto-mount USB drives",
                    "a plugged stick mounts itself",
                    Glyph::Disk,
                ));
            } else {
                rows.push(row(
                    "Auto-mount USB drives",
                    "off · Places mounts a stick when you pick it",
                    Glyph::Disk,
                ));
            }
            keys.push("automount".to_string());

            rows.push(row(
                "Login screen (Greeter)",
                crate::greeter::settings::status_summary(),
                Glyph::Gear,
            ));
            keys.push("greeter".to_string());

            let translate_target = out(&[&bin_script("translate"), "--target-name"])
                .unwrap_or_else(|| "English".to_string());
            rows.push(row(
                "Translate",
                format!("into {}", translate_target),
                Glyph::Globe,
            ));
            keys.push("translate".to_string());
        }
        Section::Accessibility => {
            rows.push(row(
                "Reduced motion",
                if reduced_motion_on() {
                    "on · new windows"
                } else {
                    "off"
                },
                Glyph::Eye,
            ));
            keys.push("reduced-motion".to_string());

            if super::have("wvkbd-mobintl") {
                rows.push(row(
                    "On-screen keyboard",
                    if onscreen_keyboard_running() {
                        "running \u{00b7} the bar at the bottom of the screen"
                    } else {
                        "show it for typing without the keyboard"
                    },
                    Glyph::Keyboard,
                ));
                keys.push("onscreen-keyboard".to_string());
            }
        }
    }

    (rows, keys)
}

const HIT_SECTION_BASE: usize = 20_000;

/// Master-Detail Settings Control Center.
///
/// Wide terminals (`>= 100` cols) render a persistent left section rail paired
/// with the active section's live settings list on the right. Compact terminals
/// (`< 100` cols) render a clickable horizontal section tab strip above the list.
pub struct SettingsScreen {
    active_section: usize,
    focus_rail: bool,
    cache: Vec<Option<(Vec<hayato_ui::Row>, Vec<String>)>>,
    keys: Vec<String>,
    list: ListState,
    banner: Option<String>,
    pub selected_key: Option<String>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    section_hits: RefCell<Vec<Rect>>,
    static_mode: bool,
}

impl Default for SettingsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsScreen {
    /// Open the Control Center on the first section (`Network`).
    pub fn new() -> Self {
        Self::with_state(0, None)
    }

    /// Open on a specific section with an optional post-action status banner.
    pub fn with_state(section_idx: usize, banner: Option<String>) -> Self {
        let idx = section_idx.min(Section::ALL.len().saturating_sub(1));
        let mut screen = Self {
            active_section: idx,
            focus_rail: false,
            cache: vec![None; Section::ALL.len()],
            keys: Vec::new(),
            list: ListState::new().with_empty("no matching settings"),
            banner,
            selected_key: None,
            header: Header::new("Hayato").subtitle("Settings"),
            keybar: vec![
                KeyHint::new("Tab", "sections"),
                KeyHint::new("Enter", "open / toggle"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            section_hits: RefCell::new(Vec::new()),
            static_mode: false,
        };
        screen.load_section(idx);
        screen
    }

    /// Deterministic constructor for golden snapshot tests (never shells out).
    pub fn new_for_test() -> Self {
        let mut cache = vec![None; Section::ALL.len()];
        cache[0] = Some((
            vec![
                row("Wi-Fi radio", "on", Glyph::Wifi),
                row("Wi-Fi networks", "Hayato-5G", Glyph::Globe),
                row("Bluetooth radio", "on", Glyph::Bluetooth),
                row("Bluetooth devices", "1 device connected", Glyph::Link),
                row("DNS", "the resolver this connection uses", Glyph::Cloud),
                row("Share this screen", "off", Glyph::Eye),
                row("Remote login (SSH)", "remote login status", Glyph::Server),
            ],
            vec![
                "wifi-radio".into(),
                "wifi".into(),
                "bt-radio".into(),
                "bt".into(),
                "dns".into(),
                "vnc".into(),
                "ssh".into(),
            ],
        ));
        let mut screen = Self {
            active_section: 0,
            focus_rail: false,
            cache,
            keys: Vec::new(),
            list: ListState::new().with_empty("no matching settings"),
            banner: None,
            selected_key: None,
            header: Header::new("Hayato").subtitle("Settings"),
            keybar: vec![
                KeyHint::new("Tab", "sections"),
                KeyHint::new("Enter", "open / toggle"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            section_hits: RefCell::new(Vec::new()),
            static_mode: true,
        };
        screen.load_section(0);
        screen
    }

    pub fn active_section_index(&self) -> usize {
        self.active_section
    }

    fn load_section(&mut self, idx: usize) {
        let idx = idx.min(Section::ALL.len().saturating_sub(1));
        self.active_section = idx;
        if self.cache[idx].is_none() {
            let section = Section::ALL[idx];
            if self.static_mode {
                let mut rows = Vec::new();
                let mut keys = Vec::new();
                for &k in section_keys(section) {
                    rows.push(row(k, "", section.glyph()));
                    keys.push(k.to_string());
                }
                self.cache[idx] = Some((rows, keys));
            } else {
                self.cache[idx] = Some(build_rows(section));
            }
        }
        if let Some((rows, keys)) = self.cache[idx].clone() {
            self.keys = keys;
            self.list.set_filter("");
            self.list.set_rows(rows);
        }
        let sec = Section::ALL[idx];
        self.header = Header::new("Hayato").subtitle("Settings").status(format!(
            "{} · {}",
            sec.label(),
            sec.detail()
        ));
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 100 {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 28) / 100).clamp(28, 36) as u16;
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

    fn right_layout(area: Rect, compact: bool) -> (Option<Rect>, Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if compact {
            let tabs = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
            let filter_y = inner.y.saturating_add(2).min(inner.bottom());
            let filter = Rect::new(
                inner.x,
                filter_y,
                inner.width,
                1.min(inner.bottom().saturating_sub(filter_y)),
            );
            let rows_y = inner.y.saturating_add(4).min(inner.bottom());
            let rows = Rect::new(
                inner.x,
                rows_y,
                inner.width,
                inner.bottom().saturating_sub(rows_y),
            );
            (Some(tabs), filter, rows)
        } else {
            let filter = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
            let rows_y = inner.y.saturating_add(2).min(inner.bottom());
            let rows = Rect::new(
                inner.x,
                rows_y,
                inner.width,
                inner.bottom().saturating_sub(rows_y),
            );
            (None, filter, rows)
        }
    }

    fn activate_selected(&mut self) -> Outcome {
        if let Some(index) = self.list.selected_index()
            && let Some(key) = self.keys.get(index)
        {
            self.selected_key = Some(key.clone());
            return Outcome::Done(hayato_ui::app::exit::OK);
        }
        Outcome::Continue
    }
}

impl Screen for SettingsScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        let mut hits = self.section_hits.borrow_mut();
        hits.clear();

        if let Some(right_area) = right {
            // Wide Master-Detail: persistent left section rail + right live settings pane.
            draw_card(ctx.buf, left, ctx.design, Some("Sections"), self.focus_rail);
            let inner = Rect::new(
                left.x + 2,
                left.y + 1,
                left.width.saturating_sub(4),
                left.height.saturating_sub(2),
            );
            let mut y = inner.y;
            for (idx, sec) in Section::ALL.iter().enumerate() {
                if y >= inner.bottom() {
                    break;
                }
                let active = idx == self.active_section;
                let marker = if active { "▍" } else { " " };
                let marker_style = if active {
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Accent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    ctx.design.style(TextStyle::Dim)
                };
                put(ctx.buf, inner.x, y, marker, 1, marker_style);
                let label_style = if active {
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Accent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    ctx.design.style(TextStyle::Body)
                };
                let label = format!("{}  {}", sec.glyph().cell(), sec.label());
                put_trunc(
                    ctx.buf,
                    inner.x + 2,
                    y,
                    &label,
                    inner.width.saturating_sub(2),
                    label_style,
                );
                hits.push(Rect::new(inner.x, y, inner.width, 1));
                y += 1;
                if y < inner.bottom() && inner.height >= 16 {
                    put_trunc(
                        ctx.buf,
                        inner.x + 5,
                        y,
                        sec.detail(),
                        inner.width.saturating_sub(5),
                        ctx.design.style(TextStyle::Dim),
                    );
                    y += 2;
                } else {
                    y += 1;
                }
            }

            let sec = Section::ALL[self.active_section];
            draw_card(
                ctx.buf,
                right_area,
                ctx.design,
                Some(sec.label()),
                !self.focus_rail,
            );
            let (_, filter, rows) = Self::right_layout(right_area, false);
            let msg = self
                .banner
                .as_deref()
                .unwrap_or("Type to filter · Enter opens or toggles · Tab switches pane");
            draw_filter_bar(
                ctx.buf,
                filter,
                ctx.design,
                self.list.filter(),
                msg,
                self.list.visible_len(),
                self.keys.len(),
            );
            self.rows_height.set(rows.height);
            self.list
                .render(rows, ctx.buf, ctx.design, !self.focus_rail);
        } else {
            // Compact (< 100 cols): horizontal section tab bar + live settings list.
            let sec = Section::ALL[self.active_section];
            draw_card(ctx.buf, left, ctx.design, Some(sec.label()), true);
            let (tabs, filter, rows) = Self::right_layout(left, true);
            if let Some(tab_area) = tabs {
                let mut x = tab_area.x;
                for (idx, s) in Section::ALL.iter().enumerate() {
                    let active = idx == self.active_section;
                    let text = if active {
                        format!("[{}]", s.short_label())
                    } else {
                        format!(" {} ", s.short_label())
                    };
                    let w = text.chars().count() as u16;
                    if x + w > tab_area.right() {
                        break;
                    }
                    let st = if active {
                        Style::default()
                            .fg(ctx.design.color(ColorRole::Accent))
                            .add_modifier(Modifier::BOLD)
                    } else {
                        ctx.design.style(TextStyle::Dim)
                    };
                    put_trunc(ctx.buf, x, tab_area.y, &text, w, st);
                    hits.push(Rect::new(x, tab_area.y, w, 1));
                    x += w + 1;
                }
            }
            let msg = self
                .banner
                .as_deref()
                .unwrap_or("Tab/←→ switches section · type to filter · Enter acts");
            draw_filter_bar(
                ctx.buf,
                filter,
                ctx.design,
                self.list.filter(),
                msg,
                self.list.visible_len(),
                self.keys.len(),
            );
            self.rows_height.set(rows.height);
            self.list.render(rows, ctx.buf, ctx.design, true);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Tab => {
                self.focus_rail = !self.focus_rail;
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                let n = Section::ALL.len();
                let prev = (self.active_section + n - 1) % n;
                self.load_section(prev);
                return Outcome::Consumed;
            }
            KeyCode::Left if !self.focus_rail && self.list.filter().is_empty() => {
                let n = Section::ALL.len();
                let prev = (self.active_section + n - 1) % n;
                self.load_section(prev);
                return Outcome::Consumed;
            }
            KeyCode::Right if self.focus_rail => {
                self.focus_rail = false;
                return Outcome::Consumed;
            }
            KeyCode::Right if !self.focus_rail && self.list.filter().is_empty() => {
                let next = (self.active_section + 1) % Section::ALL.len();
                self.load_section(next);
                return Outcome::Consumed;
            }
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            _ => {}
        }

        if self.focus_rail {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    let prev = self.active_section.saturating_sub(1);
                    self.load_section(prev);
                    return Outcome::Consumed;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let next = (self.active_section + 1).min(Section::ALL.len() - 1);
                    self.load_section(next);
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    self.focus_rail = false;
                    return Outcome::Consumed;
                }
                _ => return Outcome::Continue,
            }
        }

        if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && self.list.type_filter(key)
        {
            self.banner = None;
            return Outcome::Consumed;
        }

        if key.code == KeyCode::Enter {
            return self.activate_selected();
        }

        let rows_height = self.rows_height.get();
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(_) => self.activate_selected(),
            ListAction::Moved | ListAction::Ignored => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let hits = self.section_hits.borrow();
        for (idx, r) in hits.iter().enumerate() {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(HIT_SECTION_BASE + idx);
            }
        }
        let (left, right) = Self::split_panes(area);
        let (_, _, rows) = match right {
            Some(r) => Self::right_layout(r, false),
            None => Self::right_layout(left, true),
        };
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(r) = row else {
            return Outcome::Continue;
        };
        if r >= HIT_SECTION_BASE {
            let idx = r - HIT_SECTION_BASE;
            if idx < Section::ALL.len() {
                self.load_section(idx);
                self.focus_rail = false;
            }
            return Outcome::Consumed;
        }
        self.focus_rail = false;
        self.list.set_cursor(r);
        self.activate_selected()
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(Section::ALL[self.active_section].key())
    }
}

/// `hayato menu settings` (`hayato-settings`): Master-Detail Settings Control Center.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Settings",
            format!("{said} (network, bluetooth, sway, iw)"),
        ));
    }

    let mut banner: Option<String> = None;
    let mut active_section: usize = 0;

    loop {
        let mut screen = SettingsScreen::with_state(active_section, banner.take());
        let code = crate::run_screen(terminal, design, "Settings", &mut screen)?;
        active_section = screen.active_section_index();
        if code != 0 {
            return Ok(code);
        }

        let Some(key) = screen.selected_key.take() else {
            return Ok(1);
        };

        match key.as_str() {
            "wifi-radio" => {
                let on = wifi_on();
                let action = if on { "off" } else { "on" };
                if checked(&["nmcli", "radio", "wifi", action]).is_err()
                    && sudo(&["nmcli", "radio", "wifi", action]).is_err()
                {
                    banner = Some(format!("Could not turn Wi-Fi radio {action}"));
                } else {
                    banner = Some(format!("Wi-Fi radio turned {action}"));
                }
            }
            "bt-radio" => {
                let on = bt_on();
                let action = if on { "off" } else { "on" };
                if checked(&["bluetoothctl", "power", action]).is_err()
                    && sudo(&["bluetoothctl", "power", action]).is_err()
                {
                    banner = Some(format!("Could not power Bluetooth controller {action}"));
                } else {
                    super::bluetooth::signal_bar();
                    banner = Some(format!("Bluetooth controller powered {action}"));
                }
            }
            "wifi" => {
                let _ = super::net::menu(terminal, design)?;
            }
            "portal" => match super::portal::open() {
                Ok(url) => banner = Some(format!("Opened the sign-in page: {url}")),
                Err(reason) => banner = Some(reason),
            },
            "bt" => {
                let _ = super::bluetooth::menu(terminal, design)?;
            }
            "dns" => {
                let _ = super::dns::menu(terminal, design)?;
            }
            "night" => {
                use super::night_light::Mode;
                let current = super::night_light::read();
                let choices = [
                    (Mode::Off, 4000, "Off", "no warm tint"),
                    (Mode::Auto, 3500, "Sunset · 3500 K", "warmer after dark"),
                    (Mode::Auto, 4000, "Sunset · 4000 K", "default warmth"),
                    (Mode::Auto, 4500, "Sunset · 4500 K", "milder after dark"),
                    (Mode::Warm, 3500, "Warm now · 3500 K", "always warm"),
                    (Mode::Warm, 4000, "Warm now · 4000 K", "always warm"),
                    (Mode::Warm, 4500, "Warm now · 4500 K", "always mild"),
                ];
                let rows: Vec<hayato_ui::Row> = choices
                    .iter()
                    .map(|(mode, warmth, label, detail)| {
                        let state = if current == (*mode, *warmth) {
                            "current"
                        } else {
                            detail
                        };
                        row((*label).to_string(), state.to_string(), Glyph::Moon)
                    })
                    .collect();
                let keys: Vec<String> = choices
                    .iter()
                    .map(|(mode, warmth, _, _)| format!("{}:{warmth}", mode.as_str()))
                    .collect();
                let mut night_picker = Picker::new(
                    "Night light",
                    vec![Question::choose(
                        "Night light",
                        "Enter applies · Escape returns to Settings",
                        rows,
                        keys,
                    )],
                );
                let code = crate::run_screen(terminal, design, "Night light", &mut night_picker)?;
                if code == 0
                    && let Some(answer) = night_picker.answers().first()
                    && let Some((mode, warmth)) = answer.split_once(':')
                    && let Some(mode) = Mode::parse(mode)
                    && let Ok(warmth) = warmth.parse::<u16>()
                {
                    if let Err(error) = super::night_light::write(mode, warmth) {
                        banner = Some(format!("Could not write night-light.conf: {error}"));
                    } else if let Err(error) = super::night_light::apply() {
                        banner = Some(format!("Saved, but {error}"));
                    } else {
                        banner = Some(format!("Night light: {}", super::night_light::status()));
                    }
                }
            }
            "dnd" => {
                let path = dnd_path();
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                    banner = Some("Do not disturb: off (notifications allowed)".to_string());
                } else {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&path, "1\n");
                    banner = Some("Do not disturb: on".to_string());
                }
            }
            "notify" => {
                let mut note = "Enter opens · Escape returns to Settings".to_string();
                loop {
                    let config = super::notify_settings::read();
                    let timeout = if config.default_timeout == 0 {
                        "never".to_string()
                    } else {
                        format!("{}s", config.default_timeout / 1000)
                    };
                    let rows = vec![
                        row(
                            "Notification history",
                            "copy or clear past cards",
                            Glyph::Bell,
                        ),
                        row(
                            "Do not disturb",
                            if dnd_on() { "on" } else { "off" },
                            Glyph::BellOff,
                        ),
                        row("Default timeout", timeout, Glyph::Clock),
                        row(
                            "Max visible",
                            format!("{} cards", config.max_visible),
                            Glyph::Bell,
                        ),
                        row(
                            "Card width",
                            format!("{} px", config.card_width as u32),
                            Glyph::Display,
                        ),
                        row(
                            "History limit",
                            format!("{} entries", config.history_limit),
                            Glyph::Trash,
                        ),
                        row("Clear notification history", "asks first", Glyph::Trash),
                        row("Back", "Settings sections", Glyph::ArrowLeft),
                    ];
                    let keys = [
                        "history",
                        "dnd",
                        "timeout",
                        "visible",
                        "width",
                        "history-limit",
                        "clear",
                        "back",
                    ];
                    let mut picker = Picker::new(
                        "Notifications",
                        vec![Question::choose(
                            "Notifications",
                            note.clone(),
                            rows,
                            keys.iter().map(|key| (*key).to_string()).collect(),
                        )],
                    );
                    let code = crate::run_screen(terminal, design, "Notifications", &mut picker)?;
                    if code != 0 {
                        break;
                    }
                    let Some(key) = picker.answers().first().cloned() else {
                        break;
                    };
                    if key == "back" {
                        break;
                    }
                    match key.as_str() {
                        "history" => {
                            let _ = super::notify::menu(terminal, design)?;
                        }
                        "dnd" => {
                            let path = dnd_path();
                            if path.exists() {
                                let _ = std::fs::remove_file(&path);
                                note = "Do not disturb: off".to_string();
                            } else {
                                if let Some(parent) = path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::write(&path, "1\n");
                                note = "Do not disturb: on".to_string();
                            }
                        }
                        "timeout" => {
                            let choices = [
                                (3000u64, "3 seconds"),
                                (5000, "5 seconds"),
                                (10000, "10 seconds"),
                                (0, "Never"),
                            ];
                            let rows: Vec<hayato_ui::Row> = choices
                                .iter()
                                .map(|(value, label)| {
                                    row(
                                        (*label).to_string(),
                                        if config.default_timeout == *value {
                                            "current"
                                        } else {
                                            ""
                                        },
                                        Glyph::Clock,
                                    )
                                })
                                .collect();
                            let keys: Vec<String> =
                                choices.iter().map(|(value, _)| value.to_string()).collect();
                            let mut picker = Picker::new(
                                "Default timeout",
                                vec![Question::choose(
                                    "Default timeout",
                                    "Enter applies · Escape returns",
                                    rows,
                                    keys,
                                )],
                            );
                            let code = crate::run_screen(
                                terminal,
                                design,
                                "Default timeout",
                                &mut picker,
                            )?;
                            if code == 0
                                && let Some(value) = picker.answers().first()
                                && let Ok(value) = value.parse::<u64>()
                            {
                                let mut next = config;
                                next.default_timeout = value;
                                match save_notify(next) {
                                    Ok(message) => note = message,
                                    Err(error) => note = error,
                                }
                            }
                        }
                        "visible" => {
                            let choices = [1usize, 3, 5];
                            let rows: Vec<hayato_ui::Row> = choices
                                .iter()
                                .map(|value| {
                                    row(
                                        format!(
                                            "{value} card{}",
                                            if *value == 1 { "" } else { "s" }
                                        ),
                                        if config.max_visible == *value {
                                            "current"
                                        } else {
                                            ""
                                        },
                                        Glyph::Bell,
                                    )
                                })
                                .collect();
                            let keys: Vec<String> =
                                choices.iter().map(|value| value.to_string()).collect();
                            let mut picker = Picker::new(
                                "Max visible",
                                vec![Question::choose(
                                    "Max visible",
                                    "Enter applies · Escape returns",
                                    rows,
                                    keys,
                                )],
                            );
                            let code =
                                crate::run_screen(terminal, design, "Max visible", &mut picker)?;
                            if code == 0
                                && let Some(value) = picker.answers().first()
                                && let Ok(value) = value.parse::<usize>()
                            {
                                let mut next = config;
                                next.max_visible = value;
                                match save_notify(next) {
                                    Ok(message) => note = message,
                                    Err(error) => note = error,
                                }
                            }
                        }
                        "width" => {
                            let choices = [300u32, 340, 400];
                            let rows: Vec<hayato_ui::Row> = choices
                                .iter()
                                .map(|value| {
                                    row(
                                        format!("{value} px"),
                                        if config.card_width as u32 == *value {
                                            "current"
                                        } else {
                                            ""
                                        },
                                        Glyph::Display,
                                    )
                                })
                                .collect();
                            let keys: Vec<String> =
                                choices.iter().map(|value| value.to_string()).collect();
                            let mut picker = Picker::new(
                                "Card width",
                                vec![Question::choose(
                                    "Card width",
                                    "Enter applies · Escape returns",
                                    rows,
                                    keys,
                                )],
                            );
                            let code =
                                crate::run_screen(terminal, design, "Card width", &mut picker)?;
                            if code == 0
                                && let Some(value) = picker.answers().first()
                                && let Ok(value) = value.parse::<u32>()
                            {
                                let mut next = config;
                                next.card_width = value as f32;
                                match save_notify(next) {
                                    Ok(message) => note = message,
                                    Err(error) => note = error,
                                }
                            }
                        }
                        "history-limit" => {
                            let choices = [100usize, 200, 500];
                            let rows: Vec<hayato_ui::Row> = choices
                                .iter()
                                .map(|value| {
                                    row(
                                        format!("{value} entries"),
                                        if config.history_limit == *value {
                                            "current"
                                        } else {
                                            ""
                                        },
                                        Glyph::Trash,
                                    )
                                })
                                .collect();
                            let keys: Vec<String> =
                                choices.iter().map(|value| value.to_string()).collect();
                            let mut picker = Picker::new(
                                "History limit",
                                vec![Question::choose(
                                    "History limit",
                                    "Enter applies · Escape returns",
                                    rows,
                                    keys,
                                )],
                            );
                            let code =
                                crate::run_screen(terminal, design, "History limit", &mut picker)?;
                            if code == 0
                                && let Some(value) = picker.answers().first()
                                && let Ok(value) = value.parse::<usize>()
                            {
                                let mut next = config;
                                next.history_limit = value;
                                match save_notify(next) {
                                    Ok(message) => note = message,
                                    Err(error) => note = error,
                                }
                            }
                        }
                        "clear" => {
                            let mut picker = Picker::new(
                                "Clear notification history",
                                vec![Question::ask(
                                    "Clear notification history?",
                                    "This removes the stored cards; current cards stay on screen",
                                    "clear the history",
                                    "Clear",
                                    "yes",
                                )],
                            );
                            let code = crate::run_screen(
                                terminal,
                                design,
                                "Clear notification history",
                                &mut picker,
                            )?;
                            if code == 0
                                && picker.answers().first().map(String::as_str) == Some("yes")
                            {
                                match super::notify::clear_history() {
                                    Ok(()) => note = "Notification history cleared".to_string(),
                                    Err(error) => {
                                        note = format!("Could not clear history: {error}")
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                banner = Some(note);
            }
            "vnc" => {
                let _ = super::vnc::menu(terminal, design)?;
            }
            "ssh" => {
                let _ = super::ssh::menu(terminal, design)?;
            }
            "tz" => {
                let _ = super::tz::menu(terminal, design)?;
            }
            "locale" => {
                let current = super::locale::current();
                let locales = super::locale::list();
                if locales.is_empty() {
                    banner =
                        Some("No locales are available (localectl list-locales is empty)".into());
                } else {
                    let rows: Vec<hayato_ui::Row> = locales
                        .iter()
                        .map(|locale| {
                            row(
                                locale.clone(),
                                if *locale == current { "current" } else { "" },
                                Glyph::Globe,
                            )
                        })
                        .collect();
                    let mut picker = Picker::new(
                        "Language & region",
                        vec![Question::choose(
                            "Language & region",
                            format!("Current: {current} · Enter sets · Escape returns"),
                            rows,
                            locales.clone(),
                        )],
                    );
                    let code =
                        crate::run_screen(terminal, design, "Language & region", &mut picker)?;
                    if code == 0
                        && let Some(locale) = picker.answers().first()
                    {
                        match super::locale::set(locale) {
                            Ok(()) => {
                                banner = Some(format!(
                                    "Language & region: {locale} (log out and back in)"
                                ));
                            }
                            Err(error) => banner = Some(format!("Could not set locale: {error}")),
                        }
                    }
                }
            }
            "hostname" => {
                let current = super::hostname::current();
                let mut picker = Picker::new(
                    "Hostname",
                    vec![Question::input(
                        "Hostname",
                        format!("Current: {current} · letters, numbers and hyphens"),
                        "my-chromebook",
                        false,
                    )],
                );
                let code = crate::run_screen(terminal, design, "Hostname", &mut picker)?;
                if code == 0
                    && let Some(name) = picker.answers().first()
                {
                    match super::hostname::set(name) {
                        Ok(()) => banner = Some(format!("Hostname: {name}")),
                        Err(error) => banner = Some(format!("Could not set hostname: {error}")),
                    }
                }
            }
            "default-apps" => {
                let rows: Vec<hayato_ui::Row> = super::default_apps::CATEGORIES
                    .iter()
                    .map(|category| {
                        let current = super::default_apps::current(category)
                            .map(|id| super::default_apps::desktop_name(&id))
                            .unwrap_or_else(|| "not set".to_string());
                        row(category.label, current, Glyph::Desktop)
                    })
                    .collect();
                let keys: Vec<String> = super::default_apps::CATEGORIES
                    .iter()
                    .map(|category| category.key.to_string())
                    .collect();
                let mut picker = Picker::new(
                    "Default apps",
                    vec![Question::choose(
                        "Default apps",
                        "Enter opens a category · Escape returns to Settings",
                        rows,
                        keys,
                    )],
                );
                let code = crate::run_screen(terminal, design, "Default apps", &mut picker)?;
                if code == 0
                    && let Some(key) = picker.answers().first()
                    && let Some(category) = super::default_apps::category(key)
                {
                    let ids = super::default_apps::candidates(category);
                    if ids.is_empty() {
                        banner = Some(format!(
                            "No applications are registered for {}",
                            category.label
                        ));
                    } else {
                        let current = super::default_apps::current(category);
                        let rows: Vec<hayato_ui::Row> = ids
                            .iter()
                            .map(|id| {
                                row(
                                    super::default_apps::desktop_name(id),
                                    if current.as_deref() == Some(id.as_str()) {
                                        "current"
                                    } else {
                                        ""
                                    },
                                    Glyph::Desktop,
                                )
                            })
                            .collect();
                        let mut picker = Picker::new(
                            category.label,
                            vec![Question::choose(
                                category.label,
                                "Enter sets the default · Escape returns",
                                rows,
                                ids.clone(),
                            )],
                        );
                        let code =
                            crate::run_screen(terminal, design, category.label, &mut picker)?;
                        if code == 0
                            && let Some(id) = picker.answers().first()
                        {
                            match super::default_apps::set(category, id) {
                                Ok(()) => {
                                    banner = Some(format!(
                                        "{}: {}",
                                        category.label,
                                        super::default_apps::desktop_name(id)
                                    ));
                                }
                                Err(error) => {
                                    banner = Some(format!("Could not set default: {error}"));
                                }
                            }
                        }
                    }
                }
            }
            "password" => {
                let mut picker = Picker::new(
                    "Change password",
                    vec![
                        Question::input(
                            "New password",
                            "at least one character · Escape returns",
                            "new password",
                            true,
                        ),
                        Question::input(
                            "Confirm password",
                            "type it again",
                            "confirm password",
                            true,
                        ),
                    ],
                );
                let code = crate::run_screen(terminal, design, "Change password", &mut picker)?;
                if code == 0
                    && let [password, confirm] = picker.answers().as_slice()
                {
                    match validate_password(password, confirm) {
                        Err(error) => banner = Some(error.to_string()),
                        Ok(()) => match change_password(password) {
                            Ok(()) => banner = Some("Password changed".to_string()),
                            Err(error) => {
                                banner = Some(format!("Could not change password: {error}"))
                            }
                        },
                    }
                }
            }
            "clipboard" => {
                let mut note = "Enter acts · Escape returns to Settings".to_string();
                loop {
                    let enabled = clipboard_enabled();
                    let rows = vec![
                        row(
                            "Keep clipboard history",
                            if enabled { "on" } else { "off" },
                            Glyph::Clipboard,
                        ),
                        row(
                            "Open clipboard history",
                            clipboard_status(),
                            Glyph::Clipboard,
                        ),
                        row("Clear clipboard history", "asks first", Glyph::Trash),
                        row("Back", "Settings sections", Glyph::ArrowLeft),
                    ];
                    let keys = ["toggle", "history", "clear", "back"];
                    let mut picker = Picker::new(
                        "Clipboard history",
                        vec![Question::choose(
                            "Clipboard history",
                            note.clone(),
                            rows,
                            keys.iter().map(|key| (*key).to_string()).collect(),
                        )],
                    );
                    let code =
                        crate::run_screen(terminal, design, "Clipboard history", &mut picker)?;
                    if code != 0 {
                        break;
                    }
                    let Some(key) = picker.answers().first().cloned() else {
                        break;
                    };
                    match key.as_str() {
                        "back" => break,
                        "toggle" => {
                            let next = !enabled;
                            if let Err(error) = set_clipboard_enabled(next) {
                                note = format!("Could not save clipboard.conf: {error}");
                            } else {
                                let _ = out(&[&bin_script("hayato-clipboard"), "restart"]);
                                note = format!(
                                    "Clipboard history: {}",
                                    if next { "on" } else { "off" }
                                );
                            }
                        }
                        "history" => {
                            let _ = super::clip::menu(terminal, design)?;
                        }
                        "clear" => {
                            let mut picker = Picker::new(
                                "Clear clipboard history",
                                vec![Question::ask(
                                    "Clear clipboard history?",
                                    "This removes the stored entries; the current selection stays",
                                    "wipe the history",
                                    "Clear",
                                    "yes",
                                )],
                            );
                            let code = crate::run_screen(
                                terminal,
                                design,
                                "Clear clipboard history",
                                &mut picker,
                            )?;
                            if code == 0
                                && picker.answers().first().map(String::as_str) == Some("yes")
                            {
                                match checked(&["cliphist", "wipe"]) {
                                    Ok(_) => note = "Clipboard history cleared".to_string(),
                                    Err(error) => {
                                        note =
                                            format!("Could not clear clipboard history: {error}");
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                banner = Some(note);
            }
            "touchpad" => {
                let mut note =
                    "Enter toggles or opens speed · Escape returns to Settings".to_string();
                loop {
                    let settings = super::touchpad::read();
                    let rows = vec![
                        row(
                            "Tap to click",
                            if settings.tap { "on" } else { "off" },
                            Glyph::Mouse,
                        ),
                        row(
                            "Natural scrolling",
                            if settings.natural_scroll { "on" } else { "off" },
                            Glyph::Mouse,
                        ),
                        row(
                            "Disable while typing",
                            if settings.dwt { "on" } else { "off" },
                            Glyph::Mouse,
                        ),
                        row("Pointer speed", settings.speed_label(), Glyph::Mouse),
                        row("Back", "Settings sections", Glyph::ArrowLeft),
                    ];
                    let keys = ["tap", "natural", "dwt", "speed", "back"];
                    let mut pad_picker = Picker::new(
                        "Touchpad",
                        vec![Question::choose(
                            "Touchpad",
                            note.clone(),
                            rows,
                            keys.iter().map(|key| (*key).to_string()).collect(),
                        )],
                    );
                    let code = crate::run_screen(terminal, design, "Touchpad", &mut pad_picker)?;
                    if code != 0 {
                        break;
                    }
                    let Some(key) = pad_picker.answers().first().cloned() else {
                        break;
                    };
                    if key == "back" {
                        break;
                    }
                    let mut next = settings;
                    match key.as_str() {
                        "tap" => next.tap = !next.tap,
                        "natural" => next.natural_scroll = !next.natural_scroll,
                        "dwt" => next.dwt = !next.dwt,
                        "speed" => {
                            let speeds = [
                                (-0.3f32, "Slow", "less pointer movement"),
                                (0.0, "Normal", "libinput default"),
                                (0.3, "Fast", "more pointer movement"),
                            ];
                            let s_rows: Vec<hayato_ui::Row> = speeds
                                .iter()
                                .map(|(_, label, detail)| {
                                    row((*label).to_string(), (*detail).to_string(), Glyph::Mouse)
                                })
                                .collect();
                            let s_keys: Vec<String> = speeds
                                .iter()
                                .map(|(value, _, _)| format!("{value}"))
                                .collect();
                            let mut speed_picker = Picker::new(
                                "Pointer speed",
                                vec![Question::choose(
                                    "Pointer speed",
                                    "Enter applies · Escape returns to Touchpad",
                                    s_rows,
                                    s_keys,
                                )],
                            );
                            let s_code = crate::run_screen(
                                terminal,
                                design,
                                "Pointer speed",
                                &mut speed_picker,
                            )?;
                            if s_code == 0
                                && let Some(value) = speed_picker.answers().first()
                                && let Ok(value) = value.parse::<f32>()
                            {
                                next.accel = value;
                            } else {
                                continue;
                            }
                        }
                        _ => continue,
                    }
                    if let Err(error) = super::touchpad::write(next) {
                        note = format!("Could not write touchpad.conf: {error}");
                    } else if let Err(error) = super::touchpad::apply(next) {
                        note = format!("Saved, but {error}");
                    } else {
                        note = format!("Touchpad: {}", super::touchpad::status());
                    }
                }
                banner = Some(note);
            }
            "rotation" => {
                let cur = rotation_name();
                let choices = [
                    ("normal", "Normal", "landscape (default)"),
                    ("90", "90° clockwise", "portrait"),
                    ("180", "180°", "inverted landscape"),
                    ("270", "270° clockwise", "portrait"),
                ];
                let r_rows: Vec<hayato_ui::Row> = choices
                    .iter()
                    .map(|(k, l, d)| {
                        let detail = if *k == cur { "current" } else { *d };
                        row((*l).to_string(), detail.to_string(), Glyph::Rotate)
                    })
                    .collect();
                let r_keys: Vec<String> =
                    choices.iter().map(|(k, _, _)| (*k).to_string()).collect();
                let mut r_picker = Picker::new(
                    "Rotation",
                    vec![Question::choose(
                        "Rotation",
                        "Enter applies \u{00b7} Escape returns to Settings",
                        r_rows,
                        r_keys,
                    )],
                );
                let r_code = crate::run_screen(terminal, design, "Rotation", &mut r_picker)?;
                if r_code == 0
                    && let Some(rot) = r_picker.answers().first()
                {
                    let out_name = focused_output();
                    let _ = out(&["swaymsg", "output", &out_name, "transform", rot]);
                    banner = Some(format!("Rotation: {out_name} \u{2192} {rot}"));
                }
            }
            "display" => {
                let _ = super::display::menu(terminal, design)?;
            }
            "workspaces" => {
                if let Some(message) = super::workspaces::choose(terminal, design)? {
                    banner = Some(message);
                }
            }
            "theme" => {
                let _ = super::theme_picker::menu(terminal, design)?;
            }
            "wallpaper" => {
                if let Some(message) = super::appearance::pick_wallpaper(terminal, design)? {
                    banner = Some(message);
                }
            }
            "wallpaper-next" => match super::appearance::cycle_wallpaper() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "wallpaper-theme" => match super::appearance::clear_custom_wallpaper() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "bar-position" => match super::appearance::toggle_bar_position() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "bar-density" => match super::appearance::toggle_bar_density() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "bar-modules" => match super::appearance::toggle_bar_modules() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "corners" => match super::appearance::cycle_corners() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "shadows" => match super::appearance::toggle_shadows() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "blur" => match super::appearance::cycle_blur() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "animations" => match super::appearance::cycle_animations() {
                Ok(message) => banner = Some(message),
                Err(reason) => banner = Some(reason),
            },
            "greeter" => {
                let _ = crate::greeter::settings::menu(terminal, design)?;
            }
            "automount" => {
                let conf = automount_conf();
                let (value, word) = if automount_on() {
                    ("no", "off \u{00b7} Places mounts a stick when you pick it")
                } else {
                    ("yes", "on \u{00b7} a plugged USB drive mounts itself")
                };
                let part = conf.with_extension("part");
                let conf_display = conf.display().to_string();
                if std::fs::create_dir_all(conf.parent().unwrap()).is_ok()
                    && std::fs::write(&part, format!("enabled={value}\n")).is_ok()
                    && std::fs::rename(&part, &conf).is_ok()
                {
                    banner = Some(format!("Auto-mount USB drives: {word}"));
                } else {
                    let _ = std::fs::remove_file(&part);
                    banner = Some(format!("Could not write {conf_display}"));
                }
            }
            "updates" => {
                let _ = super::pkg::menu(terminal, design)?;
            }
            "clean" => {
                let _ = super::clean::menu(terminal, design)?;
            }
            "tablet" => {
                let _ = out(&[&bin_script("tablet-mode"), "toggle"]);
                banner = Some(format!("Tablet mode: {}", tablet_mode_detail()));
            }
            "onscreen-keyboard" => {
                if onscreen_keyboard_running() {
                    banner = Some("The on-screen keyboard is already up".to_string());
                } else {
                    let _ = Command::new("wvkbd-mobintl")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn();
                    banner = Some("On-screen keyboard: shown at the bottom".to_string());
                }
            }
            "tablet-calibrate" => {
                let _ = out(&[&bin_script("tablet-mode"), "calibrate"]);
                banner = Some("Calibrated rotation sensor".to_string());
            }
            "layout" => {
                let _ = super::layout::menu(terminal, design)?;
            }
            "controller" => {
                let _ = super::pad::menu(terminal, design)?;
            }
            "translate" => {
                let _ = super::translate::menu(&["config".to_string()], terminal, design)?;
            }
            "reduced-motion" => {
                let on = !reduced_motion_on();
                if let Err(error) = set_reduced_motion(on) {
                    banner = Some(format!("Could not save reduced motion: {error}"));
                } else {
                    banner = Some(format!(
                        "Reduced motion: {} (new windows)",
                        if on { "on" } else { "off" }
                    ));
                }
            }
            "audio" => {
                let _ = super::audio::menu(terminal, design)?;
            }
            "mixer" => {
                spawn_subcommand("pavucontrol", &[]);
                return Ok(0);
            }
            "media" => {
                let _ = super::media::menu(terminal, design)?;
            }
            "idle" => {
                let current = super::idle::read();
                let presets = super::idle::presets();
                let rows: Vec<hayato_ui::Row> = presets
                    .iter()
                    .map(|(screen_off, lock, dpms, label, detail)| {
                        let state = if current == (*screen_off, *lock, *dpms) {
                            "current"
                        } else {
                            detail
                        };
                        row((*label).to_string(), state.to_string(), Glyph::Clock)
                    })
                    .collect();
                let keys: Vec<String> = presets
                    .iter()
                    .map(|(screen_off, lock, dpms, _, _)| format!("{screen_off}:{lock}:{dpms}"))
                    .collect();
                let mut idle_picker = Picker::new(
                    "Idle & lock",
                    vec![Question::choose(
                        "Idle & lock",
                        "Enter applies \u{00b7} Escape returns to Settings",
                        rows,
                        keys,
                    )],
                );
                let code = crate::run_screen(terminal, design, "Idle & lock", &mut idle_picker)?;
                if code == 0
                    && let Some(answer) = idle_picker.answers().first()
                {
                    let parts: Vec<u64> = answer
                        .split(':')
                        .filter_map(|part| part.parse::<u64>().ok())
                        .collect();
                    if let [screen_off, lock, dpms] = parts.as_slice() {
                        if let Err(error) = super::idle::write(*screen_off, *lock, *dpms) {
                            banner = Some(format!("Could not write idle.conf: {error}"));
                        } else if let Err(error) = super::idle::restart() {
                            banner = Some(format!("Saved, but {error}"));
                        } else {
                            banner = Some(format!("Idle & lock: {}", super::idle::status()));
                        }
                    }
                }
            }
            "power" => {
                spawn_subcommand("hayato", &["menu", "power"]);
                return Ok(0);
            }
            _ => {}
        }
    }
}

fn save_notify(config: super::notify_settings::Config) -> Result<String, String> {
    super::notify_settings::write(config)
        .map_err(|error| format!("could not write notify.conf: {error}"))?;
    super::notify_settings::restart()?;
    Ok(format!(
        "Notifications restarted (cards cleared): {}",
        super::notify_settings::status()
    ))
}

fn change_password(password: &str) -> Result<(), String> {
    use std::io::Write as _;
    if password.is_empty() || password.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err("password cannot be empty or contain newline characters".to_string());
    }
    let user = out(&["id", "-un"]).unwrap_or_else(|| "hayato".to_string());
    let mut child = std::process::Command::new("sudo")
        .args(["-n", "/usr/sbin/chpasswd"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run chpasswd: {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err("chpasswd has no stdin".to_string());
    };
    if let Err(error) = writeln!(stdin, "{user}:{password}") {
        return Err(format!("cannot send the password: {error}"));
    }
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("chpasswd did not finish: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if reason.is_empty() {
            "chpasswd refused the change".to_string()
        } else {
            reason
        })
    }
}

fn spawn_subcommand(prog: &str, args: &[&str]) {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(prog);
    command
        .args(args)
        .env_remove("HAYATO_UI")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let _ = command.spawn();
}

fn missing(names: &[&str]) -> Option<String> {
    let gone: Vec<&str> = names.iter().copied().filter(|n| !have(n)).collect();
    if gone.is_empty() {
        None
    } else {
        Some(format!("Missing: {}", gone.join(" ")))
    }
}

fn have(program: &str) -> bool {
    crate::files::open::on_path(&std::env::var("PATH").unwrap_or_default(), program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_row_belongs_to_one_section() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for section in Section::ALL {
            for key in section_keys(section) {
                assert!(seen.insert(*key), "{key} is in two sections");
            }
        }
        for (key, _, _) in rows() {
            assert!(seen.contains(key), "{key} is in no section");
        }
    }

    #[test]
    fn rows_table_wires_real_session_commands() {
        let table = rows();
        assert!(
            table
                .iter()
                .any(|(k, _, r)| *k == "display" && *r == "hayato-display")
        );
        assert!(
            table
                .iter()
                .any(|(k, _, r)| *k == "vnc" && *r == "hayato-vnc")
        );
        assert!(
            table
                .iter()
                .any(|(k, _, r)| *k == "night" && *r == "night-light apply")
        );
        assert!(
            table
                .iter()
                .any(|(k, _, r)| *k == "translate" && *r == "translate config")
        );
        assert!(
            table
                .iter()
                .any(|(k, l, _)| *k == "automount" && *l == "Auto-mount USB drives")
        );
        for (key, runs) in [
            ("dns", "hayato-dns"),
            ("ssh", "hayato-ssh"),
            ("tz", "hayato-tz"),
            ("updates", "hayato-pkg"),
            ("clean", "hayato-clean"),
            ("idle", "hayato-idle"),
            ("touchpad", "touchpad.conf"),
            ("reduced-motion", "reduced-motion.conf"),
            ("night", "night-light apply"),
        ] {
            assert!(
                table.iter().any(|(k, _, r)| *k == key && *r == runs),
                "Settings lost the {key} row"
            );
        }
    }

    #[test]
    fn settings_control_center_switches_sections_and_activates() {
        let design = Design::solarized();
        let mut screen = SettingsScreen::new_for_test();
        assert_eq!(screen.phase(), Some("network"));
        assert_eq!(screen.active_section_index(), 0);

        // Right arrow cycles to Display & Sound.
        let _ = screen.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &design);
        assert_eq!(screen.active_section_index(), 1);
        assert_eq!(screen.phase(), Some("display-sound"));

        // Left arrow cycles back to Network.
        let _ = screen.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &design);
        assert_eq!(screen.active_section_index(), 0);

        // Enter activates the first row ("wifi-radio").
        let outcome = screen.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &design);
        assert!(matches!(outcome, Outcome::Done(0)));
        assert_eq!(screen.selected_key.as_deref(), Some("wifi-radio"));
    }
}
