//! Session-scoped MPRIS2 + PipeWire volume controller for Hayato Connect.
//!
//! Uses existing system `playerctl` and `wpctl` CLI binaries without spawning
//! any background daemon: telemetry is polled on demand while `ConnectScreen`
//! is open.

use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSnapshot {
    pub player: String,
    pub title: String,
    pub artist: String,
    pub status: String,
    pub position_s: u64,
    pub length_s: u64,
    pub volume_pct: u8,
    pub muted: bool,
}

impl MediaSnapshot {
    pub fn has_track(&self) -> bool {
        !self.player.is_empty() && (!self.title.is_empty() || !self.status.is_empty())
    }

    pub fn summary_line(&self) -> String {
        if !self.has_track() {
            return "No active media player".to_string();
        }
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.artist, self.title)
        }
    }
}

/// Poll active MPRIS player state + default audio sink volume/mute.
pub fn poll_media() -> MediaSnapshot {
    let (volume_pct, muted) = read_sink_volume();
    let Some(raw) = run_cmd(
        "playerctl",
        &[
            "metadata",
            "--format",
            "{{playerName}}\t{{status}}\t{{title}}\t{{artist}}\t{{position}}\t{{mpris:length}}",
        ],
    ) else {
        return MediaSnapshot {
            volume_pct,
            muted,
            ..MediaSnapshot::default()
        };
    };
    parse_playerctl_line(&raw, volume_pct, muted)
}

pub fn parse_playerctl_line(line: &str, volume_pct: u8, muted: bool) -> MediaSnapshot {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return MediaSnapshot {
            volume_pct,
            muted,
            ..MediaSnapshot::default()
        };
    }
    let mut parts = trimmed.splitn(6, '\t');
    let player = parts.next().unwrap_or("").trim().to_string();
    let status = parts.next().unwrap_or("").trim().to_string();
    let title = parts.next().unwrap_or("").trim().to_string();
    let artist = parts.next().unwrap_or("").trim().to_string();
    let pos_us: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let len_us: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    MediaSnapshot {
        player,
        title,
        artist,
        status,
        position_s: pos_us / 1_000_000,
        length_s: len_us / 1_000_000,
        volume_pct,
        muted,
    }
}

/// Execute an authenticated remote transport or volume command on desktop.
pub fn exec_media_cmd(cmd: &str) -> Result<String, String> {
    match cmd {
        "play_pause" => {
            spawn_ok("playerctl", &["play-pause"])?;
            Ok("Toggled play/pause".to_string())
        }
        "next" => {
            spawn_ok("playerctl", &["next"])?;
            Ok("Skipped to next track".to_string())
        }
        "prev" => {
            spawn_ok("playerctl", &["previous"])?;
            Ok("Returned to previous track".to_string())
        }
        "seek_fwd" => {
            spawn_ok("playerctl", &["position", "10+"])?;
            Ok("Seeked +10s".to_string())
        }
        "seek_back" => {
            spawn_ok("playerctl", &["position", "10-"])?;
            Ok("Seeked -10s".to_string())
        }
        "vol_up" => {
            spawn_ok(
                "wpctl",
                &["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", "5%+"],
            )?;
            Ok("Volume +5%".to_string())
        }
        "vol_down" => {
            spawn_ok("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"])?;
            Ok("Volume -5%".to_string())
        }
        "mute" => {
            spawn_ok("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])?;
            Ok("Toggled sink mute".to_string())
        }
        other => Err(format!("Unsupported media command: {other}")),
    }
}

fn read_sink_volume() -> (u8, bool) {
    let Some(raw) = run_cmd("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) else {
        return (0, false);
    };
    parse_wpctl_volume(&raw)
}

pub fn parse_wpctl_volume(raw: &str) -> (u8, bool) {
    // Expected: "Volume: 0.72" or "Volume: 0.72 [MUTED]"
    let muted = raw.contains("[MUTED]");
    let pct = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| (v * 100.0).round().clamp(0.0, 150.0) as u8)
        .unwrap_or(0);
    (pct, muted)
}

fn run_cmd(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn spawn_ok(bin: &str, args: &[&str]) -> Result<(), String> {
    let st = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("{bin} failed: {e}"))?;
    if !st.success() {
        return Err(format!("{bin} exited with {st}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playerctl_and_wpctl_outputs() {
        let (vol, muted) = parse_wpctl_volume("Volume: 0.65 [MUTED]");
        assert_eq!(vol, 65);
        assert!(muted);

        let snap = parse_playerctl_line(
            "hayato-cassette\tPlaying\tFeather\tNujabes\t42000000\t175000000",
            72,
            false,
        );
        assert_eq!(snap.player, "hayato-cassette");
        assert_eq!(snap.title, "Feather");
        assert_eq!(snap.artist, "Nujabes");
        assert_eq!(snap.status, "Playing");
        assert_eq!(snap.position_s, 42);
        assert_eq!(snap.length_s, 175);
        assert_eq!(snap.volume_pct, 72);
        assert!(!snap.muted);
    }
}
