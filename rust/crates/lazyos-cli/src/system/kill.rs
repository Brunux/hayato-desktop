//! Process Manager & Killer (`rofi-kill`, `lazyos menu kill`).
//!
//! Lists current user processes with PID, command name, and memory usage.
//! Filter as you type; Enter sends SIGTERM, with follow-up SIGKILL option if
//! process refuses to terminate.

use std::process::Command;

use lazyos_ui::{Design, Glyph};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::pick::{Picker, Question, row};
use super::out;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub rss_kb: u64,
    pub comm: String,
}

/// Parse `ps -eo pid=,rss=,comm= --sort=-rss` output.
pub fn parse_ps_table(raw: &str, self_pid: u32) -> Vec<ProcessRecord> {
    let mut procs = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        if pid <= 1 || pid == self_pid {
            continue;
        }
        let rss_kb = parts[1].parse::<u64>().unwrap_or(0);
        let comm = parts[2..].join(" ");

        procs.push(ProcessRecord { pid, rss_kb, comm });
    }
    procs
}

/// Interactive `lazyos menu kill`.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let self_pid = std::process::id();
    let mut status_note: Option<String> = None;

    loop {
        let ps_out = out(&["ps", "-eo", "pid=,rss=,comm=", "--sort=-rss"]).unwrap_or_default();
        let procs = parse_ps_table(&ps_out, self_pid);

        let mut rows = Vec::new();
        let mut keys = Vec::new();

        rows.push(row(
            "Refresh process list",
            "rescan running user processes",
            Glyph::Sync,
        ));
        keys.push("refresh".to_string());

        for p in procs.iter().take(50) {
            let mb = p.rss_kb / 1024;
            rows.push(
                row(&p.comm, format!("pid {}", p.pid), Glyph::Bug).value(format!("{mb} MB")),
            );
            keys.push(format!("{}:{}", p.pid, p.comm));
        }

        let note = status_note
            .clone()
            .unwrap_or_else(|| "Enter sends SIGTERM · type to filter · Escape closes".to_string());

        let mut picker = Picker::new(
            "End a process",
            vec![Question::choose("End a process", note, rows, keys)],
        );

        let code = crate::run_screen(terminal, design, "End a process", &mut picker)?;
        if code != 0 {
            return Ok(code);
        }

        let answers = picker.answers();
        let Some(action) = answers.first() else {
            return Ok(1);
        };

        if action == "refresh" {
            status_note = Some("Refreshed process list".to_string());
            continue;
        }

        let Some((pid_str, comm)) = action.split_once(':') else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<i32>() else {
            continue;
        };

        // Send SIGTERM
        let term_res = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
        if term_res.map(|s| s.success()).unwrap_or(false) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Check if still running
            let alive = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if alive {
                // Prompt for SIGKILL
                let mut kill_picker = Picker::new(
                    &format!("Still running: {comm}"),
                    vec![Question::choose(
                        format!("Still running: {comm}"),
                        "SIGKILL cannot be ignored, unsaved data is lost",
                        vec![
                            row(
                                "Force it (SIGKILL)",
                                "terminate immediately",
                                Glyph::Warning,
                            ),
                            row("Leave it alone", "keep process running", Glyph::Bug),
                        ],
                        vec!["kill".to_string(), "leave".to_string()],
                    )],
                );
                let kc = crate::run_screen(
                    terminal,
                    design,
                    &format!("Still running: {comm}"),
                    &mut kill_picker,
                )?;
                if kc == 0 && kill_picker.answers().first().map(String::as_str) == Some("kill") {
                    let _ = Command::new("kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                    status_note = Some(format!("Force-killed {comm} (pid {pid})"));
                } else {
                    status_note = Some(format!("Left {comm} (pid {pid}) running"));
                }
            } else {
                status_note = Some(format!("Ended {comm} (pid {pid})"));
            }
        } else {
            status_note = Some(format!(
                "Cannot signal {comm} ({pid}): it is not yours or already exited"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ps_table_filters_pid_1_and_self() {
        let raw = "\
    1  1024 systemd
  100 20480 foot
  200 40960 rustc
  300  5120 lazyos
";
        let procs = parse_ps_table(raw, 300);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 100);
        assert_eq!(procs[0].comm, "foot");
        assert_eq!(procs[1].pid, 200);
        assert_eq!(procs[1].comm, "rustc");
    }
}
