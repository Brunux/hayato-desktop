//! Night-light mode and warmth (`night-light`, Settings → Display & Sound).
//!
//! The helper owns the wlsunset command line; this module owns the user's
//! choice in `~/.config/lazyos/night-light.conf`. `mode` is `auto` (sunset),
//! `warm` (always) or `off`; `warmth` is the Kelvin value passed to wlsunset.
//! `night-light apply` reads the same file, so the login line and the Settings
//! row cannot disagree.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Warm,
    Off,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Warm => "warm",
            Self::Off => "off",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "warm" => Some(Self::Warm),
            "off" => Some(Self::Off),
            _ => None,
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
    base.join("lazyos/night-light.conf")
}

fn read_from(path: &Path) -> (Mode, u16) {
    let mut mode = Mode::Auto;
    let mut warmth = 4000;
    let Ok(text) = std::fs::read_to_string(path) else {
        return (mode, warmth);
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "mode" => {
                if let Some(parsed) = Mode::parse(value.trim()) {
                    mode = parsed;
                }
            }
            "warmth" => {
                if let Ok(parsed) = value.trim().parse::<u16>()
                    && (2000..=6500).contains(&parsed)
                {
                    warmth = parsed;
                }
            }
            _ => {}
        }
    }
    (mode, warmth)
}

fn write_to(path: &Path, mode: Mode, warmth: u16) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(
        &tmp,
        format!(
            "# LazyOS night light. mode = auto (sunset), warm (always) or off.\n\
             # Written by Settings -> Display & Sound; night-light apply reads it.\n\
             mode={}\nwarmth={warmth}\n",
            mode.as_str()
        ),
    )?;
    std::fs::rename(&tmp, path)
}

pub fn read() -> (Mode, u16) {
    read_from(&config_path())
}

pub fn write(mode: Mode, warmth: u16) -> io::Result<()> {
    write_to(&config_path(), mode, warmth)
}

pub fn status() -> String {
    let (mode, warmth) = read();
    let mode = match mode {
        Mode::Auto => "sunset",
        Mode::Warm => "warm",
        Mode::Off => "off",
    };
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let helper = PathBuf::from(home).join(".config/bin/night-light");
    let actual = Command::new(helper)
        .arg("status")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| mode.to_string());
    format!("{actual} · {warmth} K")
}

pub fn apply() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let helper = PathBuf::from(home).join(".config/bin/night-light");
    if !helper.is_file() {
        return Err("night-light is not installed".to_string());
    }
    let status = Command::new(&helper)
        .arg("apply")
        .status()
        .map_err(|error| format!("cannot run night-light: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("night-light apply exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lazyos-night-light-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("night-light.conf")
    }

    #[test]
    fn missing_or_broken_config_uses_sunset_4000k() {
        let path = temp_path("defaults");
        assert_eq!(read_from(&path), (Mode::Auto, 4000));
        std::fs::write(&path, "mode=nope\nwarmth=99999\n").unwrap();
        assert_eq!(read_from(&path), (Mode::Auto, 4000));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_written_config_round_trips() {
        let path = temp_path("roundtrip");
        write_to(&path, Mode::Warm, 3500).unwrap();
        assert_eq!(read_from(&path), (Mode::Warm, 3500));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mode=warm\nwarmth=3500\n"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn status_names_the_mode_and_kelvin() {
        let path = temp_path("status");
        write_to(&path, Mode::Auto, 4000).unwrap();
        assert_eq!(read_from(&path), (Mode::Auto, 4000));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
