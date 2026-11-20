//! The Quake console (`hayato quake`, `Alt+\``).
//!
//! A terminal that drops from the top of the screen and goes away again on the
//! same key, holding the default coding agent (or a shell when no agent is
//! installed). Sway's scratchpad is the whole state: while hidden the window
//! lives there, so toggling costs no daemon and the terminal keeps its
//! scrollback and the agent's session. The geometry comes from
//! `config.d/34-agent-console.conf`, and this module repeats it on every show
//! because a scratchpad window can come back at the size it was last dragged
//! to.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{agents, have, out, spawn_detached_argv};

/// The app-id in `config.d/34-agent-console.conf` and in every `swaymsg`
/// criteria below. One string, so the rule and the toggle cannot disagree.
pub const APP_ID: &str = "hayato-quake";

/// How much of the screen the console takes: the top 60%, leaving the window
/// below visible and clickable while the agent runs.
pub const HEIGHT_PERCENT: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleState {
    /// No console window in the tree: the next toggle starts one.
    Absent,
    /// The window is in the scratchpad.
    Hidden,
    /// The window is on a workspace and can be typed into.
    Visible,
}

impl ConsoleState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Hidden => "hidden",
            Self::Visible => "visible",
        }
    }
}

fn tree() -> Option<String> {
    out(&["swaymsg", "-t", "get_tree", "-r"])
}

fn find_console(node: &Value) -> Option<bool> {
    if node.get("app_id").and_then(Value::as_str) == Some(APP_ID) {
        return Some(
            node.get("visible")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
    }
    for key in ["nodes", "floating_nodes"] {
        let Some(children) = node.get(key).and_then(Value::as_array) else {
            continue;
        };
        for child in children {
            if let Some(visible) = find_console(child) {
                return Some(visible);
            }
        }
    }
    None
}

/// Where the console is, read from a `swaymsg -t get_tree -r` document. Pure,
/// so the toggle's one interesting question is a test instead of a click.
pub fn state_from_tree(text: &str) -> ConsoleState {
    let Ok(root) = serde_json::from_str::<Value>(text) else {
        return ConsoleState::Absent;
    };
    match find_console(&root) {
        None => ConsoleState::Absent,
        Some(true) => ConsoleState::Visible,
        Some(false) => ConsoleState::Hidden,
    }
}

pub fn state() -> Option<ConsoleState> {
    tree().map(|text| state_from_tree(&text))
}

fn sway(command: &str) -> Result<(), String> {
    super::checked(&["swaymsg", command]).map(|_| ())
}

fn criteria() -> String {
    format!("[app_id=\"{APP_ID}\"]")
}

fn hide() -> Result<(), String> {
    sway(&format!("{} move scratchpad", criteria()))
}

/// Put the window on screen at the top, at the size the console owns.
///
/// `move scratchpad` runs first: a freshly mapped window may still be tiled
/// (the `for_window` rule can lose the race with foot's first commit), and
/// moving a window that is already in the scratchpad changes nothing.
fn show() -> Result<(), String> {
    sway(&format!("{} move scratchpad", criteria()))?;
    sway(&format!("{} scratchpad show", criteria()))?;
    sway(&format!(
        "{} resize set width 100 ppt height {HEIGHT_PERCENT} ppt, move absolute position 0 0",
        criteria()
    ))?;
    Ok(())
}

fn work_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let work = PathBuf::from(&home).join("Work");
    if work.is_dir() {
        work
    } else {
        PathBuf::from(home)
    }
}

fn shell_line_for(shell: &str) -> String {
    format!("exec '{}' -l", shell.replace('\'', "'\\''"))
}

/// The title and the `bash -lc` line the console runs: the default agent when
/// one is installed, a login shell otherwise. The agent gets a session of its
/// own, so the console never shows the conversation another window has.
fn console_session(dir: &Path) -> (String, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_session = || ("Shell".to_string(), shell_line_for(&shell));
    let Some(agent) = agents::get_default_agent() else {
        return shell_session();
    };
    let account = agents::default_account(agent);
    agents::console_session(agent, account.as_deref(), dir).unwrap_or_else(shell_session)
}

fn spawn_console() -> Result<(), String> {
    if !have("foot") {
        return Err("foot is missing - install foot".to_string());
    }
    let workdir = work_dir();
    let (title, line) = console_session(&workdir);
    let dir = workdir.display().to_string();
    let title = format!("--title={title}");
    let app_id = format!("--app-id={APP_ID}");
    spawn_detached_argv(
        "foot",
        &[
            &app_id,
            &title,
            "--working-directory",
            &dir,
            "-e",
            "bash",
            "-lc",
            &line,
        ],
    );
    Ok(())
}

fn wait_for_window(timeout: Duration) -> Option<ConsoleState> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(state) = state() {
            if state != ConsoleState::Absent {
                return Some(state);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Show, hide or start the console. Returns the line the caller prints, so a
/// keybinding and `hayato quake` say the same thing.
pub fn toggle() -> Result<String, String> {
    let state = state().ok_or("swaymsg cannot read the window tree - is this a Sway session?")?;
    match state {
        ConsoleState::Visible => {
            hide()?;
            Ok("Agent console hidden".to_string())
        }
        ConsoleState::Hidden => {
            show()?;
            Ok("Agent console shown".to_string())
        }
        ConsoleState::Absent => {
            spawn_console()?;
            if wait_for_window(Duration::from_secs(3)).is_none() {
                return Err("the console window did not appear".to_string());
            }
            show()?;
            Ok("Agent console opened".to_string())
        }
    }
}

const USAGE: &str = "\
usage: hayato quake [toggle|show|hide|status]

  toggle  show the console, hide it, or start it (the default)
  show    show or start the console
  hide    send the console to the scratchpad
  status  print absent, hidden or visible
";

pub fn run(args: &[String]) -> i32 {
    let action = args.first().map(String::as_str).unwrap_or("toggle");
    let result = match action {
        "toggle" => toggle(),
        "show" => match state() {
            Some(ConsoleState::Visible) => Ok("Agent console shown".to_string()),
            Some(ConsoleState::Hidden) => show().map(|()| "Agent console shown".to_string()),
            Some(ConsoleState::Absent) => spawn_console()
                .and_then(|()| {
                    wait_for_window(Duration::from_secs(3))
                        .ok_or_else(|| "the console window did not appear".to_string())
                })
                .and_then(|_| show())
                .map(|()| "Agent console opened".to_string()),
            None => Err("swaymsg cannot read the window tree".to_string()),
        },
        "hide" => match state() {
            Some(ConsoleState::Absent) => Ok("Agent console hidden".to_string()),
            Some(_) => hide().map(|()| "Agent console hidden".to_string()),
            None => Err("swaymsg cannot read the window tree".to_string()),
        },
        "status" | "--status" => match state() {
            Some(state) => Ok(state.label().to_string()),
            None => Err("swaymsg cannot read the window tree".to_string()),
        },
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return crate::EXIT_OK;
        }
        other => {
            eprintln!("hayato quake: unknown action `{other}`");
            eprint!("{USAGE}");
            return crate::EXIT_USAGE;
        }
    };
    match result {
        Ok(line) => {
            println!("{line}");
            crate::EXIT_OK
        }
        Err(reason) => {
            eprintln!("hayato quake: {reason}");
            crate::EXIT_FAILED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_with(app_id: &str, visible: bool) -> String {
        format!(
            r#"{{"name":"root","visible":true,"nodes":[
                {{"name":"eDP-1","visible":true,"nodes":[
                    {{"name":"1","visible":true,"nodes":[],"floating_nodes":[
                        {{"app_id":"{app_id}","visible":{visible},"nodes":[]}}
                    ]}}
                ]}}
            ]}}"#
        )
    }

    #[test]
    fn a_missing_console_is_absent() {
        assert_eq!(
            state_from_tree(&tree_with("foot", true)),
            ConsoleState::Absent
        );
        assert_eq!(state_from_tree("not json"), ConsoleState::Absent);
    }

    #[test]
    fn a_visible_console_is_on_a_workspace() {
        assert_eq!(
            state_from_tree(&tree_with(APP_ID, true)),
            ConsoleState::Visible
        );
    }

    #[test]
    fn a_scratchpad_console_is_hidden() {
        assert_eq!(
            state_from_tree(&tree_with(APP_ID, false)),
            ConsoleState::Hidden
        );
    }

    #[test]
    fn the_console_is_found_inside_floating_nodes() {
        let text = format!(
            r#"{{"nodes":[],"floating_nodes":[
                {{"app_id":"hayato-pick","visible":true}},
                {{"app_id":"{APP_ID}","visible":true}}
            ]}}"#
        );
        assert_eq!(state_from_tree(&text), ConsoleState::Visible);
    }

    #[test]
    fn the_fallback_shell_is_a_login_shell() {
        assert_eq!(shell_line_for("/bin/bash"), "exec '/bin/bash' -l");
        assert_eq!(shell_line_for("/opt/my shell"), "exec '/opt/my shell' -l");
    }

    #[test]
    fn the_console_takes_the_top_sixty_percent() {
        assert_eq!(HEIGHT_PERCENT, 60);
        assert_eq!(APP_ID, "hayato-quake");
    }
}
