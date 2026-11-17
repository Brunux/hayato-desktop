//! Time zone: every zone tzdata knows, with the current one marked (`lazyos-tz`).
//!
//! The list is `timedatectl list-timezones` — 400-odd rows that the picker filters
//! as fast as anything else — and the network-time switch is the row at the
//! end, because a clock that is wrong and a clock that does not sync are the
//! same problem (systemd-timesyncd is in packages/base.list for that).
//!
//! sudo is passwordless on this image, hence -n: if it ever is not, the failure
//! is a notification instead of a password prompt with nowhere to appear.

use lazyos_ui::{Card, Design, Glyph, Urgency};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::pick::{Picker, Question, row};
use super::{out, sudo};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["timedatectl"];

/// The `--status` output.
pub fn status() -> String {
    let current = out(&["timedatectl", "show", "-p", "Timezone", "--value"]).unwrap_or_default();
    let ntp = out(&["timedatectl", "show", "-p", "NTP", "--value"]).unwrap_or_default();
    let ntp_str = if ntp == "yes" { "on" } else { "off" };
    format!("Current: {} · Network time: {}", current, ntp_str)
}

/// `lazyos menu tz` (`lazyos-tz`): the time zone picker.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Time zone",
            format!("{said} (systemd)"),
        ));
    }

    let zones = out(&["timedatectl", "list-timezones"]).unwrap_or_default();
    let mut status_note: Option<String> = None;

    loop {
        let current =
            out(&["timedatectl", "show", "-p", "Timezone", "--value"]).unwrap_or_default();
        let ntp = out(&["timedatectl", "show", "-p", "NTP", "--value"]).unwrap_or_default();

        let mut rows = Vec::new();
        let mut keys = Vec::new();

        if ntp.trim() == "yes" {
            rows.push(
                row(
                    "Set the clock from the network (NTP)",
                    "systemd-timesyncd automatic sync",
                    Glyph::Clock,
                )
                .value("● on"),
            );
            keys.push("ntp:off".into());
        } else {
            rows.push(
                row(
                    "Set the clock from the network (NTP)",
                    "systemd-timesyncd automatic sync",
                    Glyph::Clock,
                )
                .value("○ off"),
            );
            keys.push("ntp:on".into());
        }

        for zone in zones.lines() {
            let zone = zone.trim();
            if zone.is_empty() {
                continue;
            }
            if zone == current.trim() {
                rows.push(row(zone, "active system time zone", Glyph::Clock).value("● current"));
            } else {
                rows.push(row(zone, "", Glyph::Clock));
            }
            keys.push(format!("set:{zone}"));
        }

        let note = status_note.clone().unwrap_or_else(|| {
            format!(
                "Current: {} · type to filter · Enter sets · Escape closes",
                current.trim()
            )
        });

        let mut picker = Picker::new(
            "Time zone",
            vec![Question::choose("Time zone", note, rows, keys)],
        );

        let code = crate::run_screen(terminal, design, "Time zone", &mut picker)?;
        if code != 0 {
            return Ok(code);
        }

        let answers = picker.answers();
        if answers.is_empty() {
            return Ok(1);
        }
        let key = &answers[0];

        match key.as_str() {
            k if k.starts_with("set:") => {
                let zone = &k[4..];
                if let Err(e) = sudo(&["timedatectl", "set-timezone", zone]) {
                    status_note = Some(format!("Could not set {zone}: {e}"));
                    continue;
                }
                if out(&["pgrep", "-x", "waybar"]).is_some() {
                    let _ = out(&["pkill", "-x", "waybar"]);
                    let _ = std::process::Command::new("setsid").arg("waybar").spawn();
                }
                let now =
                    out(&["sh", "-c", &format!("TZ={zone} date '+%H:%M'")]).unwrap_or_default();
                status_note = Some(format!("Now {zone} · {}", now.trim()));
            }
            "ntp:on" | "ntp:off" => {
                let want = if key == "ntp:on" { "true" } else { "false" };
                let label = if key == "ntp:on" { "on" } else { "off" };
                match sudo(&["timedatectl", "set-ntp", want]) {
                    Ok(_) => status_note = Some(format!("Network time (NTP) is now {label}")),
                    Err(e) => status_note = Some(format!("Could not change network time: {e}")),
                }
            }
            _ => {}
        }
    }
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
    #[test]
    fn status_formats_correctly() {
        // Would require mocking timedatectl
    }
}
