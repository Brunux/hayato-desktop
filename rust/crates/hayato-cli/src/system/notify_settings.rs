//! Notification daemon settings (`hayato-notify`), Settings → System.
//!
//! The daemon reads `~/.config/hayato/notify.conf` once at startup, so the
//! notification path never opens a file. Changing a value writes the config and
//! restarts the daemon; current cards are cleared by that restart, which the
//! Settings banner says.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Milliseconds; 0 means never.
    pub default_timeout: u64,
    pub max_visible: usize,
    pub card_width: f32,
    pub history_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_timeout: 5000,
            max_visible: 4,
            card_width: 340.0,
            history_limit: 200,
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        });
    base.join("hayato/notify.conf")
}

fn read_from(path: &Path) -> Config {
    let mut config = Config::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return config;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "default_timeout" => {
                if let Ok(ms) = value.parse::<u64>() {
                    config.default_timeout = ms.min(60_000);
                }
            }
            "max_visible" => {
                if let Ok(n) = value.parse::<usize>() {
                    config.max_visible = n.clamp(1, 10);
                }
            }
            "card_width" => {
                if let Ok(width) = value.parse::<f32>() {
                    config.card_width = width.clamp(260.0, 480.0);
                }
            }
            "history_limit" => {
                if let Ok(n) = value.parse::<usize>() {
                    config.history_limit = n.clamp(20, 2000);
                }
            }
            _ => {}
        }
    }
    config
}

fn write_to(path: &Path, config: Config) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(
        &tmp,
        format!(
            "# Hayato notifications. Read by hayato-notify at startup;\n\
             # Settings restarts the daemon after a change.\n\
             default_timeout={}\nmax_visible={}\ncard_width={:.0}\nhistory_limit={}\n",
            config.default_timeout, config.max_visible, config.card_width, config.history_limit
        ),
    )?;
    std::fs::rename(&tmp, path)
}

pub fn read() -> Config {
    read_from(&config_path())
}

pub fn write(config: Config) -> io::Result<()> {
    write_to(&config_path(), config)
}

pub fn status() -> String {
    let config = read();
    let timeout = if config.default_timeout == 0 {
        "never".to_string()
    } else {
        format!("{}s", config.default_timeout / 1000)
    };
    format!(
        "{timeout} · {} visible · {} px",
        config.max_visible, config.card_width as u32
    )
}

pub fn restart() -> Result<(), String> {
    let _ = Command::new("pkill")
        .args(["-TERM", "-x", "hayato-notify"])
        .status();
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let running = Command::new("pgrep")
            .args(["-x", "hayato-notify"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !running {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut command = Command::new("hayato-notify");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start hayato-notify: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hayato-notify-settings-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("notify.conf")
    }

    #[test]
    fn defaults_match_the_daemon() {
        let path = temp_path("defaults");
        assert_eq!(read_from(&path), Config::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_written_config_round_trips() {
        let path = temp_path("roundtrip");
        write_to(
            &path,
            Config {
                default_timeout: 8000,
                max_visible: 3,
                card_width: 380.0,
                history_limit: 500,
            },
        )
        .unwrap();
        assert_eq!(
            read_from(&path),
            Config {
                default_timeout: 8000,
                max_visible: 3,
                card_width: 380.0,
                history_limit: 500,
            }
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn zero_timeout_means_never() {
        let path = temp_path("never");
        std::fs::write(&path, "default_timeout=0\n").unwrap();
        assert_eq!(read_from(&path).default_timeout, 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
