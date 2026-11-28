//! Idle and lock timeouts (`hayato-idle`, Settings → Power & Idle).
//!
//! The session's swayidle chain used to be three numbers in
//! `/etc/sway/config.d/23-hayato-screensaver.conf`. The user-level helper
//! `~/.config/bin/hayato-idle` reads `~/.config/hayato/idle.conf` and starts
//! the chain; this module owns that file. The system chain still starts at
//! login, and the helper's `start` replaces it, so a machine without the
//! setting keeps the old defaults.
//!
//! Seconds are the unit on disk; the menu shows minutes. `0` means never and
//! omits that timeout from the swayidle command.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Screen off, lock, panel off, in seconds. The old system chain's values.
pub const DEFAULTS: (u64, u64, u64) = (150, 300, 360);

pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/"))
        });
    base.join("hayato/idle.conf")
}

fn read_from(path: &Path) -> (u64, u64, u64) {
    let mut values = DEFAULTS;
    let Ok(text) = std::fs::read_to_string(path) else {
        return values;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "screen_off" => values.0 = value,
            "lock" => values.1 = value,
            "dpms" => values.2 = value,
            _ => {}
        }
    }
    values
}

fn write_to(path: &Path, screen_off: u64, lock: u64, dpms: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(
        &tmp,
        format!(
            "# Hayato idle timeouts, in seconds. 0 means never.\n\
             # Written by Settings -> Power & Idle; hayato-idle reads it.\n\
             screen_off={screen_off}\nlock={lock}\ndpms={dpms}\n"
        ),
    )?;
    std::fs::rename(&tmp, path)
}

pub fn read() -> (u64, u64, u64) {
    read_from(&config_path())
}

pub fn write(screen_off: u64, lock: u64, dpms: u64) -> io::Result<()> {
    write_to(&config_path(), screen_off, lock, dpms)
}

fn human(seconds: u64) -> String {
    if seconds == 0 {
        return "never".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if rest == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m{rest}s")
    }
}

pub fn format_summary(screen_off: u64, lock: u64, dpms: u64) -> String {
    if screen_off == 0 && lock == 0 && dpms == 0 {
        return "screen stays on · no idle lock".to_string();
    }
    format!(
        "screen {} · lock {} · panel {}",
        human(screen_off),
        human(lock),
        human(dpms)
    )
}

pub fn preset_name(screen_off: u64, lock: u64, dpms: u64) -> &'static str {
    presets()
        .into_iter()
        .find(|(s, l, d, _, _)| *s == screen_off && *l == lock && *d == dpms)
        .map(|(_, _, _, name, _)| name)
        .unwrap_or("Custom")
}

pub fn status() -> String {
    let (screen_off, lock, dpms) = read();
    format_summary(screen_off, lock, dpms)
}

pub fn presets() -> Vec<(u64, u64, u64, &'static str, &'static str)> {
    vec![
        (60, 120, 180, "Quick", "screen 1m · lock 2m · panel 3m"),
        (150, 300, 360, "Balanced", "screen 2m · lock 5m · panel 6m"),
        (300, 600, 900, "Relaxed", "screen 5m · lock 10m · panel 15m"),
        (600, 1200, 1800, "Long", "screen 10m · lock 20m · panel 30m"),
        (0, 0, 0, "Never", "screen stays on; no idle lock"),
    ]
}

pub fn restart() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let helper = PathBuf::from(home).join(".config/bin/hayato-idle");
    if !helper.is_file() {
        return Err("hayato-idle is not installed".to_string());
    }
    let status = Command::new(&helper)
        .arg("restart")
        .status()
        .map_err(|error| format!("cannot run hayato-idle: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("hayato-idle restart exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hayato-idle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("idle.conf")
    }

    #[test]
    fn missing_or_broken_config_uses_the_system_defaults() {
        let path = temp_path("defaults");
        assert_eq!(read_from(&path), DEFAULTS);
        std::fs::write(&path, "screen_off=nope\nlock=60\n").unwrap();
        assert_eq!(read_from(&path), (150, 60, 360));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_written_config_round_trips() {
        let path = temp_path("roundtrip");
        write_to(&path, 60, 120, 180).unwrap();
        assert_eq!(read_from(&path), (60, 120, 180));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("screen_off=60\nlock=120\ndpms=180\n"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn human_reads_minutes_and_never() {
        assert_eq!(human(0), "never");
        assert_eq!(human(45), "45s");
        assert_eq!(human(120), "2m");
        assert_eq!(human(150), "2m30s");
    }
}
