//! Per-monitor workspaces (`Settings -> Workspaces per monitor`).
//!
//! Sway already supports `workspace N output NAME`; this menu is the missing
//! writer. It has two useful shapes: split the ten workspaces between the first
//! two monitors (1–5 / 6–10), or put all ten on one monitor. The generated
//! drop-in is `~/.config/sway/config.d/35-workspaces.conf`, read after the
//! theme's `35-theme.conf` and before the display arrangement. The same
//! commands are sent through `swaymsg` so the current session changes now.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use hayato_ui::{Design, Glyph};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::pick::{Picker, Question, row};

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        });
    base.join("sway/config.d/35-workspaces.conf")
}

fn write_atomic(path: &PathBuf, body: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension("new");
    std::fs::write(&temp, body).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Split,
    AllOn,
}

fn assignments(
    outputs: &[String],
    layout: Layout,
    output: Option<&str>,
) -> Result<Vec<(u32, String)>, String> {
    match layout {
        Layout::Split => {
            if outputs.len() < 2 {
                return Err("Connect a second monitor first".to_string());
            }
            let mut rows = Vec::with_capacity(10);
            for workspace in 1..=5 {
                rows.push((workspace, outputs[0].clone()));
            }
            for workspace in 6..=10 {
                rows.push((workspace, outputs[1].clone()));
            }
            Ok(rows)
        }
        Layout::AllOn => {
            let output = output
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "Choose a monitor".to_string())?;
            Ok((1..=10)
                .map(|workspace| (workspace, output.to_string()))
                .collect())
        }
    }
}

fn config_body(rows: &[(u32, String)]) -> String {
    let mut body = String::from(
        "# Written by Settings -> Workspaces per monitor.\n\
         # Delete this file to let Sway use its default output assignment.\n",
    );
    for (workspace, output) in rows {
        body.push_str(&format!("workspace {workspace} output {output}\n"));
    }
    body
}

fn apply(rows: &[(u32, String)]) -> Result<(), String> {
    write_atomic(&config_path(), &config_body(rows))?;
    for (workspace, output) in rows {
        let _ = Command::new("swaymsg")
            .arg(format!("workspace {workspace} output {output}"))
            .status();
    }
    Ok(())
}

fn clear() -> Result<String, String> {
    let _ = std::fs::remove_file(config_path());
    let Some(focused) = super::display::focused_output() else {
        return Ok("Workspaces: cleared (next login uses Sway defaults)".to_string());
    };
    for workspace in 1..=10 {
        let _ = Command::new("swaymsg")
            .arg(format!("workspace {workspace} output {focused}"))
            .status();
    }
    Ok(format!("Workspaces: all on {focused}"))
}

fn ranges(numbers: &[u32]) -> String {
    let mut numbers = numbers.to_vec();
    numbers.sort_unstable();
    numbers.dedup();
    let mut parts = Vec::new();
    let mut start = 0;
    while start < numbers.len() {
        let mut end = start;
        while end + 1 < numbers.len() && numbers[end + 1] == numbers[end] + 1 {
            end += 1;
        }
        if start == end {
            parts.push(numbers[start].to_string());
        } else {
            parts.push(format!("{}-{}", numbers[start], numbers[end]));
        }
        start = end + 1;
    }
    parts.join(",")
}

fn status_from_config(text: &str) -> String {
    let mut by_output: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        if words.next() != Some("workspace") {
            continue;
        }
        let Some(workspace) = words.next().and_then(|word| word.parse::<u32>().ok()) else {
            continue;
        };
        if words.next() != Some("output") {
            continue;
        }
        let Some(output) = words.next() else {
            continue;
        };
        by_output
            .entry(output.to_string())
            .or_default()
            .push(workspace);
    }
    if by_output.is_empty() {
        return "all workspaces follow the focused monitor".to_string();
    }
    by_output
        .into_iter()
        .map(|(output, numbers)| format!("{output} {}", ranges(&numbers)))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn status() -> String {
    std::fs::read_to_string(config_path())
        .map(|text| status_from_config(&text))
        .unwrap_or_else(|_| status_from_config(""))
}

pub fn is_split_configured() -> bool {
    std::fs::read_to_string(config_path())
        .map(|text| {
            text.lines()
                .any(|l| l.trim_start().starts_with("workspace 6 output"))
        })
        .unwrap_or(false)
}

pub fn apply_split(panel: &str, mon: &str) -> Result<(), String> {
    let outputs = vec![panel.to_string(), mon.to_string()];
    let rows = assignments(&outputs, Layout::Split, None)?;
    apply(&rows)
}

pub fn clear_assignments() -> Result<String, String> {
    clear()
}

/// The Settings submenu. Returns the banner line, or `None` on Escape.
pub fn choose(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<Option<String>, crate::Failure> {
    let outputs = super::display::active_outputs();
    let mut rows = Vec::new();
    let mut keys = Vec::new();
    if outputs.len() >= 2 {
        rows.push(row(
            "Split 1–5 / 6–10",
            format!("{} then {}", outputs[0], outputs[1]),
            Glyph::Display,
        ));
        keys.push("split".to_string());
    }
    for output in &outputs {
        rows.push(row(
            format!("All workspaces on {output}"),
            "1–10 on this monitor",
            Glyph::Display,
        ));
        keys.push(format!("all:{output}"));
    }
    rows.push(row(
        "Clear assignments",
        "follow the focused monitor",
        Glyph::Undo,
    ));
    keys.push("clear".to_string());
    rows.push(row("Back", "Settings sections", Glyph::ArrowLeft));
    keys.push("back".to_string());

    let mut picker = Picker::new(
        "Workspaces per monitor",
        vec![Question::choose(
            "Workspaces per monitor",
            "Enter assigns workspaces to the chosen monitor",
            rows,
            keys,
        )],
    );
    let code = crate::run_screen(terminal, design, "Workspaces per monitor", &mut picker)?;
    if code != 0 {
        return Ok(None);
    }
    let Some(answer) = picker.answers().first().cloned() else {
        return Ok(None);
    };
    if answer == "back" {
        return Ok(None);
    }
    if answer == "split" {
        let rows = assignments(&outputs, Layout::Split, None)
            .map_err(|reason| crate::Failure::new("Workspaces", reason))?;
        apply(&rows).map_err(|reason| crate::Failure::new("Workspaces", reason))?;
        return Ok(Some("Workspaces: 1–5 / 6–10 split".to_string()));
    }
    if answer == "clear" {
        return clear()
            .map(Some)
            .map_err(|reason| crate::Failure::new("Workspaces", reason));
    }
    if let Some(output) = answer.strip_prefix("all:") {
        let rows = assignments(&outputs, Layout::AllOn, Some(output))
            .map_err(|reason| crate::Failure::new("Workspaces", reason))?;
        apply(&rows).map_err(|reason| crate::Failure::new("Workspaces", reason))?;
        return Ok(Some(format!("Workspaces: all on {output}")));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_uses_the_first_two_monitors() {
        let outputs = vec!["eDP-1".to_string(), "DP-1".to_string()];
        let rows = assignments(&outputs, Layout::Split, None).unwrap();
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], (1, "eDP-1".to_string()));
        assert_eq!(rows[4], (5, "eDP-1".to_string()));
        assert_eq!(rows[5], (6, "DP-1".to_string()));
        assert_eq!(rows[9], (10, "DP-1".to_string()));
    }

    #[test]
    fn split_needs_a_second_monitor() {
        let outputs = vec!["eDP-1".to_string()];
        assert!(assignments(&outputs, Layout::Split, None).is_err());
    }

    #[test]
    fn all_on_one_output_covers_one_to_ten() {
        let outputs = vec!["eDP-1".to_string(), "DP-1".to_string()];
        let rows = assignments(&outputs, Layout::AllOn, Some("DP-1")).unwrap();
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|(_, output)| output == "DP-1"));
    }

    #[test]
    fn the_config_round_trips_into_a_readable_status() {
        let rows = vec![
            (1, "eDP-1".to_string()),
            (2, "eDP-1".to_string()),
            (3, "eDP-1".to_string()),
            (4, "eDP-1".to_string()),
            (5, "eDP-1".to_string()),
            (6, "DP-1".to_string()),
            (7, "DP-1".to_string()),
            (8, "DP-1".to_string()),
            (9, "DP-1".to_string()),
            (10, "DP-1".to_string()),
        ];
        let body = config_body(&rows);
        assert_eq!(status_from_config(&body), "DP-1 6-10 · eDP-1 1-5");
    }

    #[test]
    fn an_empty_config_means_follow_the_focused_monitor() {
        assert_eq!(
            status_from_config(""),
            "all workspaces follow the focused monitor"
        );
    }
}
