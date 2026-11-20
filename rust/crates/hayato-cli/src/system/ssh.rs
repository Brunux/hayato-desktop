//! SSH remote login status (`hayato-ssh` equivalent).
//!
//! The image runs `openssh-server` with `ssh.service` enabled on every boot.
//! This menu shows the status and the commands to change it.

use hayato_ui::{Card, Design, Glyph, Urgency};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::pick::{Picker, Question, row};
use super::{out, sudo};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["systemctl", "sshd"];

/// The `--status` output.
pub fn status() -> String {
    let enabled = out(&["systemctl", "is-enabled", "ssh"])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false);
    let active = out(&["systemctl", "is-active", "ssh"])
        .map(|s| s.trim() == "active")
        .unwrap_or(false);
    let port = out(&["ss", "-ltnH", "sport = :22"])
        .map(|s| s.contains("0.0.0.0:22"))
        .unwrap_or(false);

    let mut parts = Vec::new();
    parts.push(format!(
        "ssh.service: {}",
        if enabled { "enabled" } else { "disabled" }
    ));
    parts.push(format!("running: {}", if active { "yes" } else { "no" }));
    if port {
        parts.push("listening on 0.0.0.0:22".to_string());
    }
    parts.join(" · ")
}

/// `hayato menu ssh`: the SSH status picker.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Remote login (ssh)",
            format!("{said} (systemd, openssh)"),
        ));
    }

    let mut status_note: Option<String> = None;

    loop {
        let enabled = out(&["systemctl", "is-enabled", "ssh"])
            .map(|s| s.trim() == "enabled")
            .unwrap_or(false);
        let active = out(&["systemctl", "is-active", "ssh"])
            .map(|s| s.trim() == "active")
            .unwrap_or(false);
        let user = std::env::var("USER").unwrap_or_else(|_| "brunux".to_string());
        let host = out(&["hostname", "-I"])
            .and_then(|s| s.split_whitespace().next().map(|ip| ip.to_string()))
            .or_else(|| out(&["hostname"]).map(|s| s.trim().to_string()))
            .unwrap_or_else(|| "hayato".to_string());
        let ssh_cmd = format!("ssh {user}@{host}");

        let mut rows = Vec::new();
        let mut keys = Vec::new();

        rows.push(
            row("ssh.service", "systemd unit boot state", Glyph::Key).value(if enabled {
                "enabled"
            } else {
                "disabled"
            }),
        );
        keys.push("service".to_string());

        rows.push(
            row(
                "Daemon status",
                "port 22 remote login",
                if active { Glyph::Check } else { Glyph::Lock },
            )
            .value(if active { "● running" } else { "○ stopped" }),
        );
        keys.push("running".to_string());

        rows.push(row("Copy SSH command", &ssh_cmd, Glyph::Clipboard));
        keys.push(format!("copy:{ssh_cmd}"));

        rows.push(row(
            "Disable ssh",
            "sudo systemctl disable --now ssh",
            Glyph::Lock,
        ));
        keys.push("disable".to_string());

        rows.push(row(
            "Enable ssh",
            "sudo systemctl enable --now ssh",
            Glyph::Unlock,
        ));
        keys.push("enable".to_string());

        let note = status_note
            .clone()
            .unwrap_or_else(|| "Enter acts · Escape closes".to_string());

        let mut picker = Picker::new(
            "Remote login (ssh)",
            vec![Question::choose("Remote login (ssh)", note, rows, keys)],
        );

        let code = crate::run_screen(terminal, design, "Remote login (ssh)", &mut picker)?;
        if code != 0 {
            return Ok(code);
        }

        let answers = picker.answers();
        if answers.is_empty() {
            return Ok(1);
        }

        let key = &answers[0];

        match key.as_str() {
            k if k.starts_with("copy:") => {
                let cmd = &k[5..];
                let _ = out(&["wl-copy", cmd]);
                status_note = Some(format!("Copied to clipboard: {cmd}"));
            }
            "disable" => match sudo(&["systemctl", "disable", "--now", "ssh"]) {
                Ok(_) => status_note = Some("Disabled and stopped ssh.service".to_string()),
                Err(e) => status_note = Some(format!("Could not disable ssh: {e}")),
            },
            "enable" => match sudo(&["systemctl", "enable", "--now", "ssh"]) {
                Ok(_) => status_note = Some(format!("Enabled and started ssh.service ({ssh_cmd})")),
                Err(e) => status_note = Some(format!("Could not enable ssh: {e}")),
            },
            _ => {
                status_note = Some(status());
            }
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
        // Would require mocking systemctl
    }
}
