//! The notification daemon's user settings.
//!
//! Read once at startup. Changing a setting writes the file and restarts the
//! daemon; the notification path never opens a config file.
//!
//! `default_timeout` only replaces the `-1` timeout an application sends;
//! explicit timeouts and critical cards are untouched.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub default_timeout: Duration,
    pub max_visible: usize,
    pub card_width: f32,
    pub history_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_millis(5000),
            max_visible: 4,
            card_width: 340.0,
            history_limit: 200,
        }
    }
}

pub fn path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        });
    base.join("lazyos/notify.conf")
}

pub fn load() -> Config {
    std::fs::read_to_string(path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

fn parse(text: &str) -> Config {
    let mut config = Config::default();
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
                    config.default_timeout = Duration::from_millis(ms.min(60_000));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_current_behaviour() {
        let config = parse("");
        assert_eq!(config.default_timeout, Duration::from_millis(5000));
        assert_eq!(config.max_visible, 4);
        assert_eq!(config.card_width, 340.0);
        assert_eq!(config.history_limit, 200);
    }

    #[test]
    fn values_are_clamped_and_unknown_keys_ignored() {
        let config = parse(
            "default_timeout=999999\nmax_visible=0\ncard_width=10\nhistory_limit=1\nnope=1\n",
        );
        assert_eq!(config.default_timeout, Duration::from_secs(60));
        assert_eq!(config.max_visible, 1);
        assert_eq!(config.card_width, 260.0);
        assert_eq!(config.history_limit, 20);
    }

    #[test]
    fn a_full_file_is_read() {
        let config =
            parse("default_timeout=8000\nmax_visible=3\ncard_width=380\nhistory_limit=500\n");
        assert_eq!(config.default_timeout, Duration::from_millis(8000));
        assert_eq!(config.max_visible, 3);
        assert_eq!(config.card_width, 380.0);
        assert_eq!(config.history_limit, 500);
    }
}
