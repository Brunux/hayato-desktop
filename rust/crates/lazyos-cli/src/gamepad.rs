//! The session's controller mapper: a gamepad as arrow keys for the framework's
//! own windows.
//!
//! The framework windows (the root menu, Settings, Files, every `lazyos` TUI)
//! are `foot` surfaces with an app id starting `lazyos-`. They already read
//! arrows, Enter and Escape; this process reads `/dev/input` through evdev and
//! injects exactly those keys with `wtype` when one of those windows has focus.
//!
//! Three buttons carry the Games library's own actions — X opens the options,
//! Y the manual, L1 the game's folder in Files — as **Alt+letter**. A bare
//! letter would type into another screen's filter (the root menu types to
//! filter, the files list too), while Alt+O, Alt+M and Alt+R are unbound in
//! every other framework window, so a press still cannot move two things.
//!
//! The button table is not in this file: it is [`crate::padmap`], the same one
//! the Controller screen (`lazyos menu controller`) edits and saves to
//! `~/.config/lazyos/pad.json`. This loop reloads that file when its mtime
//! changes, so a remap takes effect within two seconds and never needs a
//! restart.
//!
//! The Games library reads the pad itself (`lazyos-games`), RetroArch and the
//! HTML games keep their own input, and a plain `foot` shell or any other app is
//! left alone, so the same D-pad cannot move two things at once.
//!
//! It is the session's one resident input helper. The lock file keeps a second
//! login (or a restarted sway) from starting a second mapper.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::padmap::{Action, Bindings};

struct Pad {
    device: evdev::Device,
    x: (i32, i32),
    y: (i32, i32),
}

pub fn run() -> i32 {
    if let Some(lock) = lock_path() {
        if already_running(&lock) {
            return 0;
        }
        let _ = std::fs::write(&lock, std::process::id().to_string());
    }

    let mut bindings = Bindings::load();
    let mut config_mtime = config_stamp();
    let mut pads: Vec<Pad> = Vec::new();
    let mut known: BTreeSet<PathBuf> = BTreeSet::new();
    let mut last_scan = Instant::now() - Duration::from_secs(5);
    let mut focused = String::new();
    let mut last_focus = Instant::now() - Duration::from_secs(5);
    let mut stick = (0.0_f32, 0.0_f32);
    let mut direction = "";
    let mut next = Instant::now();
    let mut combo = (false, false);

    loop {
        if last_focus.elapsed() >= Duration::from_secs(1) {
            last_focus = Instant::now();
            focused = focused_app().unwrap_or_default();
        }

        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let mtime = config_stamp();
            if mtime != config_mtime {
                config_mtime = mtime;
                bindings = Bindings::load();
            }
            for (path, device) in evdev::enumerate() {
                if !known.insert(path) {
                    continue;
                }
                let gamepad = device
                    .supported_keys()
                    .map(|keys| keys.contains(evdev::KeyCode::BTN_SOUTH))
                    .unwrap_or(false);
                if !gamepad {
                    continue;
                }
                let mut x = (-32768, 32767);
                let mut y = (-32768, 32767);
                if let Ok(info) = device.get_absinfo() {
                    for (axis, abs) in info {
                        match axis {
                            evdev::AbsoluteAxisCode::ABS_X => x = (abs.minimum(), abs.maximum()),
                            evdev::AbsoluteAxisCode::ABS_Y => y = (abs.minimum(), abs.maximum()),
                            _ => {}
                        }
                    }
                }
                pads.push(Pad { device, x, y });
            }
        }

        let active = focused.starts_with("lazyos-");
        for pad in &mut pads {
            let Ok(events) = pad.device.fetch_events() else {
                continue;
            };
            for event in events {
                match event.destructure() {
                    evdev::EventSummary::Key(_, key, 1) => {
                        if active {
                            if let Some(action) = bindings.action_for(key) {
                                send_action(action);
                            }
                        }
                        match key {
                            evdev::KeyCode::BTN_SELECT => combo.0 = true,
                            evdev::KeyCode::BTN_START => combo.1 = true,
                            _ => {}
                        }
                        if combo.0 && combo.1 {
                            if active {
                                send_key("Escape");
                            }
                            combo = (false, false);
                        }
                    }
                    evdev::EventSummary::Key(_, key, 0) => match key {
                        evdev::KeyCode::BTN_SELECT => combo.0 = false,
                        evdev::KeyCode::BTN_START => combo.1 = false,
                        _ => {}
                    },
                    evdev::EventSummary::AbsoluteAxis(_, axis, value) => match axis {
                        evdev::AbsoluteAxisCode::ABS_X => stick.0 = normalise(value, pad.x),
                        evdev::AbsoluteAxisCode::ABS_Y => stick.1 = normalise(value, pad.y),
                        evdev::AbsoluteAxisCode::ABS_HAT0X if active && value != 0 => {
                            send_key(if value < 0 { "Left" } else { "Right" });
                        }
                        evdev::AbsoluteAxisCode::ABS_HAT0Y if active && value != 0 => {
                            send_key(if value < 0 { "Up" } else { "Down" });
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        if active {
            let now = Instant::now();
            let dir = if stick.0 < -0.55 {
                "Left"
            } else if stick.0 > 0.55 {
                "Right"
            } else if stick.1 < -0.55 {
                "Up"
            } else if stick.1 > 0.55 {
                "Down"
            } else {
                ""
            };
            if dir != direction {
                direction = dir;
                next = now + Duration::from_millis(260);
                if !dir.is_empty() {
                    send_key(dir);
                }
            } else if !dir.is_empty() && now >= next {
                next = now + Duration::from_millis(170);
                send_key(dir);
            }
        } else {
            direction = "";
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

/// What a table action becomes on the keyboard: a key every framework window
/// reads, or an Alt+letter for the three actions that are the library's own.
fn send_action(action: Action) {
    if let Some(letter) = action.alt() {
        send_alt(letter);
        return;
    }
    let key = match action {
        Action::Nothing => return,
        Action::Up => "Up",
        Action::Down => "Down",
        Action::Left => "Left",
        Action::Right => "Right",
        Action::Enter => "Return",
        Action::Escape => "Escape",
        // `alt()` handled these three above.
        Action::Options | Action::Manual | Action::Folder => return,
    };
    send_key(key);
}

/// The pad file's mtime, or `None` when there is no file yet. The mapper reads
/// the table again when this changes, which is how a remap applies without a
/// restart.
fn config_stamp() -> Option<SystemTime> {
    let path = Bindings::path()?;
    std::fs::metadata(path).ok()?.modified().ok()
}

fn send_alt(letter: &str) {
    let _ = Command::new("wtype")
        .args(["-M", "alt", "-k", letter, "-m", "alt"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn send_key(name: &str) {
    let _ = Command::new("wtype")
        .arg("-k")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn normalise(value: i32, (min, max): (i32, i32)) -> f32 {
    if max <= min {
        return 0.0;
    }
    (2.0 * (value - min) as f32 / (max - min) as f32) - 1.0
}

/// The app id of the focused sway node, if it is a Wayland window.
fn focused_app() -> Option<String> {
    let output = Command::new("swaymsg")
        .args(["-t", "get_tree", "-r"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tree: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    find_focused(&tree)
}

fn find_focused(node: &serde_json::Value) -> Option<String> {
    if node.get("focused").and_then(|value| value.as_bool()) == Some(true) {
        if let Some(app) = node.get("app_id").and_then(|value| value.as_str()) {
            return Some(app.to_string());
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(|value| value.as_array()) {
            for child in children {
                if let Some(app) = find_focused(child) {
                    return Some(app);
                }
            }
        }
    }
    None
}

fn lock_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(dir).join("lazyos-gamepad.pid"))
}

fn already_running(lock: &PathBuf) -> bool {
    let Ok(text) = std::fs::read_to_string(lock) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    cmdline
        .split(|byte| *byte == 0)
        .any(|part| part == b"gamepad")
}
