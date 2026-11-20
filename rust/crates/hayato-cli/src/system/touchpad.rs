//! Touchpad behaviour (`Settings → Input → Touchpad`).
//!
//! The system config only enables tap-to-click and sets natural scrolling for
//! the Elan touchpad. This module owns a user drop-in,
//! `~/.config/sway/config.d/36-touchpad.conf`, read after `/etc/sway/config`,
//! with the four settings a laptop user expects: tap, natural scroll,
//! disable-while-typing and pointer speed. Writing it is followed by
//! `swaymsg reload`, which applies the input block to the running session.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub tap: bool,
    pub natural_scroll: bool,
    pub dwt: bool,
    /// `pointer_accel` in sway's -1..1 range; 0 is the libinput default.
    pub accel: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tap: true,
            natural_scroll: true,
            dwt: true,
            accel: 0.0,
        }
    }
}

impl Settings {
    pub fn speed_label(self) -> &'static str {
        if self.accel < -0.15 {
            "slow"
        } else if self.accel > 0.15 {
            "fast"
        } else {
            "normal"
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/"))
        });
    base.join("sway/config.d/36-touchpad.conf")
}

fn enabled(value: &serde_json::Value, key: &str) -> Option<bool> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|v| v == "enabled")
}

/// Read the current touchpad from `swaymsg -t get_inputs -r`. A missing
/// session or a device that does not report a field falls back to the
/// system defaults rather than failing the menu.
pub fn read() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = Command::new("swaymsg")
        .args(["-t", "get_inputs", "-r"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    else {
        return settings;
    };
    let Ok(inputs) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return settings;
    };
    let Some(pad) = inputs
        .iter()
        .find(|input| input.get("type").and_then(|v| v.as_str()) == Some("touchpad"))
    else {
        return settings;
    };
    let Some(libinput) = pad.get("libinput") else {
        return settings;
    };
    if let Some(value) = enabled(libinput, "tap") {
        settings.tap = value;
    }
    if let Some(value) = enabled(libinput, "natural_scroll") {
        settings.natural_scroll = value;
    }
    if let Some(value) = enabled(libinput, "dwt") {
        settings.dwt = value;
    }
    if let Some(value) = libinput
        .get("accel_speed")
        .or_else(|| libinput.get("pointer_accel"))
        .and_then(|v| v.as_f64())
    {
        settings.accel = value as f32;
    }
    settings
}

fn on_off(value: bool) -> &'static str {
    if value { "enabled" } else { "disabled" }
}

pub fn config_body(settings: Settings) -> String {
    format!(
        "# Hayato touchpad, written by Settings -> Input. Do not edit by hand;\n\
         # the next Settings change replaces this file. Read after /etc/sway/config,\n\
         # so it wins over the system tap/natural-scroll defaults.\n\
         input type:touchpad {{\n\
         \ttap {}\n\
         \tnatural_scroll {}\n\
         \tdwt {}\n\
         \tpointer_accel {:.2}\n\
         }}\n",
        on_off(settings.tap),
        on_off(settings.natural_scroll),
        on_off(settings.dwt),
        settings.accel
    )
}

fn write_to(path: &Path, settings: Settings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, config_body(settings))?;
    std::fs::rename(&tmp, path)
}

pub fn write(settings: Settings) -> io::Result<()> {
    write_to(&config_path(), settings)
}

pub fn apply(settings: Settings) -> Result<(), String> {
    for (key, value) in [
        ("tap", on_off(settings.tap)),
        ("natural_scroll", on_off(settings.natural_scroll)),
        ("dwt", on_off(settings.dwt)),
    ] {
        let status = Command::new("swaymsg")
            .args(["input", "type:touchpad", key, value])
            .status()
            .map_err(|error| format!("cannot run swaymsg input {key}: {error}"))?;
        if !status.success() {
            return Err(format!("swaymsg input {key} exited with {status}"));
        }
    }
    let accel = format!("{:.2}", settings.accel);
    let status = Command::new("swaymsg")
        .args(["input", "type:touchpad", "pointer_accel", &accel])
        .status()
        .map_err(|error| format!("cannot run swaymsg pointer_accel: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("swaymsg pointer_accel exited with {status}"))
    }
}

pub fn status() -> String {
    let settings = read();
    format!(
        "tap {} · natural {} · typing {} · {}",
        if settings.tap { "on" } else { "off" },
        if settings.natural_scroll { "on" } else { "off" },
        if settings.dwt { "on" } else { "off" },
        settings.speed_label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_has_every_input_line() {
        let body = config_body(Settings {
            tap: true,
            natural_scroll: false,
            dwt: true,
            accel: -0.3,
        });
        assert!(body.contains("input type:touchpad {"));
        assert!(body.contains("\ttap enabled"));
        assert!(body.contains("\tnatural_scroll disabled"));
        assert!(body.contains("\tdwt enabled"));
        assert!(body.contains("\tpointer_accel -0.30"));
    }

    #[test]
    fn speed_names_cover_the_three_presets() {
        let mut settings = Settings::default();
        assert_eq!(settings.speed_label(), "normal");
        settings.accel = -0.3;
        assert_eq!(settings.speed_label(), "slow");
        settings.accel = 0.3;
        assert_eq!(settings.speed_label(), "fast");
    }
}
