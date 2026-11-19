//! Share this screen with another machine: wayvnc on the loopback address
//! (`lazyos-vnc`), either read-only or with remote control.
//!
//! Four decisions hold this together:
//!
//! * **Read-only is the default.** `wayvnc -d/--disable-input` creates no
//!   virtual keyboard and no virtual pointer, so a viewer looks and cannot
//!   type. **Remote control** is the explicit second start: the same server
//!   without that flag, so the viewer can type and click.
//!
//! * **Loopback only, both modes.** wayvnc 0.10 authenticates only through
//!   ~/.config/wayvnc/config (`enable_auth` wants a certificate, a private key
//!   and a password; `enable_pam` would accept the login password). This image
//!   ships no such file, so a server on the network address would be an
//!   unauthenticated desktop on every network the machine joins. 127.0.0.1 is
//!   reached through the ssh login the image already answers, which makes the
//!   tunnel the authentication - including when the viewer may type.
//!
//! * **The mode is the running process.** `start()` chooses the argv, and
//!   every reader (`status`, the bar, the menu) asks `/proc/<pid>/cmdline`
//!   whether `--disable-input` is there. There is no second state file to go
//!   stale beside the pid.
//!
//! * **Nothing resident.** The server is started from here and stopped from
//!   here; its pid and its log live in $XDG_RUNTIME_DIR/lazyos-vnc, so a
//!   session that dies takes the state with it and no "on" survives a logout.
//!   The bar's icon is a state file being read (waybar polls `status --bar`
//!   every ten seconds), not a process.
//!
//! The pid is checked twice before it is trusted: `kill -0` says something is
//! there, and `/proc/PID/comm` says it is wayvnc - a pid can be reused after
//! a crash, and killing a stranger would be a worse bug than a stale file.

use std::time::Duration;

use lazyos_ui::{Card, Design, Glyph, Urgency};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::out;
use super::pick::{Picker, Question, row};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["wayvnc", "ss"];

const PORT: u16 = 5900;
const STATE_DIR: &str = "lazyos-vnc";

/// Whether a viewer may only look, or may also type and click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `--disable-input`: pixels out, nothing in.
    View,
    /// No `--disable-input`: the viewer's keyboard and pointer reach this
    /// session, still only through the loopback address and the ssh tunnel.
    Control,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::View => "read-only",
            Self::Control => "remote control",
        }
    }

    fn bar_class(self) -> &'static str {
        match self {
            Self::View => "on",
            Self::Control => "control",
        }
    }
}

/// Exact argv for each mode. `nohup` stays first so `$!` is wayvnc's own pid.
const VIEW_COMMAND_LINE: [&str; 5] = ["nohup", "wayvnc", "--disable-input", "127.0.0.1", "5900"];
const CONTROL_COMMAND_LINE: [&str; 4] = ["nohup", "wayvnc", "127.0.0.1", "5900"];

/// The command line `start(mode)` runs, exposed for `lazyos menu vnc --command`.
pub fn command_line(mode: Mode) -> Vec<String> {
    let argv = match mode {
        Mode::View => &VIEW_COMMAND_LINE[..],
        Mode::Control => &CONTROL_COMMAND_LINE[..],
    };
    argv.iter().map(|s| (*s).to_string()).collect()
}

/// The SSH tunnel command a viewer runs on the other machine.
pub fn tunnel_command() -> String {
    tunnel_cmd()
}

fn state_dir() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join(STATE_DIR)
}

fn pid_file() -> std::path::PathBuf {
    state_dir().join("pid")
}

fn log_file() -> std::path::PathBuf {
    state_dir().join("wayvnc.log")
}

/// Is the pid still running? `kill -0` also succeeds for a zombie, and a
/// child of the menu stays one until the menu exits, so ask /proc for the
/// state: a `Z` means the server is gone even though the pid exists.
fn alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    rest.trim_start()
        .chars()
        .next()
        .is_some_and(|state| state != 'Z')
}

/// running: the pid file names a live wayvnc.
fn running() -> Option<u32> {
    let pid_content = std::fs::read_to_string(pid_file()).ok()?;
    let pid: u32 = pid_content.trim().parse().ok()?;
    if alive(pid)
        && let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid))
        && comm.trim() == "wayvnc"
    {
        return Some(pid);
    }
    None
}

/// listening: the kernel says something is bound to the loopback port.
fn listening() -> bool {
    let filter = format!("sport = :{}", PORT);
    let needle = format!("127.0.0.1:{}", PORT);
    out(&["ss", "-ltnH", &filter])
        .map(|s| s.contains(&needle))
        .unwrap_or(false)
}

/// The mode of the running server, read from its own argv: `--disable-input`
/// (or its short `-d`) means a viewer cannot type.
pub fn mode_from_cmdline(cmdline: &str) -> Option<Mode> {
    let mut is_wayvnc = false;
    let mut disabled = false;
    for arg in cmdline.split('\0').filter(|arg| !arg.is_empty()) {
        if arg == "wayvnc" || arg.ends_with("/wayvnc") {
            is_wayvnc = true;
        }
        if arg == "--disable-input" || arg == "-d" {
            disabled = true;
        }
    }
    if !is_wayvnc {
        return None;
    }
    Some(if disabled { Mode::View } else { Mode::Control })
}

/// The mode of the live server, or `None` when it is not running. Asking the
/// process keeps the pid file and the mode from disagreeing.
fn running_mode() -> Option<Mode> {
    let pid = running()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    mode_from_cmdline(&String::from_utf8_lossy(&cmdline))
}

fn tunnel_cmd() -> String {
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "lazyos".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "brunux".to_string());
    format!("ssh -L {}:127.0.0.1:{} {}@{}", PORT, PORT, user, host)
}

fn signal_bar() {
    let _ = out(&["pkill", "-RTMIN+8", "waybar"]);
}

/// The `--status` output.
pub fn status() -> String {
    status_for(running_mode(), listening())
}

fn status_for(mode: Option<Mode>, listening: bool) -> String {
    match mode {
        Some(mode) if listening => format!("localhost:{} · {}", PORT, mode.label()),
        _ => "off".to_string(),
    }
}

/// The `--status --bar` output for waybar.
pub fn status_bar() -> String {
    match running_mode() {
        Some(mode) if listening() => bar_json(mode),
        _ => empty_bar_json(),
    }
}

fn bar_json(mode: Mode) -> String {
    let (glyph, tooltip) = match mode {
        Mode::View => (
            "\u{f06e}", // Font Awesome eye
            format!(
                "sharing this screen read-only on localhost:{PORT}\\nclick for the menu, right click to stop"
            ),
        ),
        Mode::Control => (
            "\u{f11c}", // Font Awesome keyboard
            format!(
                "remote control is on: a viewer can type and click on localhost:{PORT}\\nclick for the menu, right click to stop"
            ),
        ),
    };
    format!(
        "{{\"text\":\"<span font_desc='JetBrains Mono 13'>{glyph}</span>\",\"class\":\"{}\",\"tooltip\":\"{tooltip}\"}}",
        mode.bar_class()
    )
}

fn empty_bar_json() -> String {
    "{\"text\":\"\",\"class\":\"empty\",\"tooltip\":\"share this screen\"}".to_string()
}

/// Start the wayvnc server in `mode`.
pub fn start(mode: Mode) -> Result<(), String> {
    if running().is_some() {
        let current = running_mode().map(Mode::label).unwrap_or("sharing");
        return Err(format!("Already sharing this screen ({current})"));
    }
    if !have("wayvnc") {
        return Err("wayvnc is not installed (package wayvnc)".to_string());
    }
    std::fs::create_dir_all(state_dir()).map_err(|e| e.to_string())?;

    // nohup, not setsid: `$!` has to be wayvnc's own pid, and `setsid` forks
    // when it is already a process-group leader (an interactive shell with job
    // control), which leaves the pid file naming a process that has exited
    // while the server runs on unowned. nohup execs in place and ignores
    // SIGHUP, so the pid is right in both cases and a closed terminal cannot
    // take the server with it. At logout the session's cgroup takes it.
    let argv = command_line(mode);
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let log = std::fs::File::create(log_file()).map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(std::process::Stdio::from(log));
    // wayvnc's warnings go to the log, not to the terminal the menu is drawing
    // in: a line on stderr would paint over the frame.
    cmd.stderr(std::process::Stdio::from(log_err));
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    std::fs::write(pid_file(), pid.to_string()).map_err(|e| e.to_string())?;
    // Reap the server when it exits. The menu can start, switch and stop it
    // several times, and an unreaped child would stay a zombie that `kill -0`
    // calls alive.
    let mut child = child;
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    // wayvnc binds before it draws anything, so a second is generous
    std::thread::sleep(Duration::from_secs(1));
    if running_mode() != Some(mode) || !listening() {
        let reason = std::fs::read_to_string(log_file()).unwrap_or_default();
        let reason = reason.lines().rev().take(2).collect::<Vec<_>>().join(" ");
        let _ = std::process::Command::new("kill")
            .args([&pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_file(pid_file());
        return Err(format!(
            "It did not start: {}",
            if reason.is_empty() {
                "wayvnc exited without a message"
            } else {
                &reason
            }
        ));
    }
    signal_bar();
    Ok(())
}

/// Stop the server and start it again in the other mode.
pub fn restart(mode: Mode) -> Result<(), String> {
    stop()?;
    start(mode)
}

/// Stop the wayvnc server.
pub fn stop() -> Result<(), String> {
    let pid = match running() {
        Some(p) => p,
        None => {
            let _ = std::fs::remove_file(pid_file());
            signal_bar();
            return Ok(());
        }
    };
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status();
    for _ in 0..30 {
        if !alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if alive(pid) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = std::fs::remove_file(pid_file());
    signal_bar();
    Ok(())
}

/// `lazyos menu vnc` (`lazyos-vnc`): the VNC picker.
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Share this screen",
            format!("{said} (wayvnc, iproute2)"),
        ));
    }

    let mut status_note: Option<String> = None;

    loop {
        let mode = running_mode().filter(|_| listening());
        let (rows, keys, default_note) = match mode {
            Some(mode) => {
                let (title, glyph) = match mode {
                    Mode::View => ("Sharing this screen", Glyph::Eye),
                    Mode::Control => ("Remote control is on", Glyph::Keyboard),
                };
                let switch = match mode {
                    Mode::View => row(
                        "Switch to remote control",
                        "the viewer can type and click · stops and restarts",
                        Glyph::Keyboard,
                    ),
                    Mode::Control => row(
                        "Switch to view only",
                        "the viewer can look but not type",
                        Glyph::Eye,
                    ),
                };
                (
                    vec![
                        row(title, format!("localhost:{PORT} · {}", mode.label()), glyph)
                            .value("● active"),
                        row("Stop sharing", "the viewer loses the picture", Glyph::Stop),
                        switch,
                        row("Copy the tunnel command", tunnel_cmd(), Glyph::Link),
                        row(
                            "What this does",
                            "view looks only · control types here, over ssh",
                            Glyph::Info,
                        ),
                    ],
                    vec![
                        "none".into(),
                        "stop".into(),
                        "switch".into(),
                        "tunnel".into(),
                        "info".into(),
                    ],
                    format!("{} on localhost:{PORT} · Enter acts", mode.label()),
                )
            }
            None => (
                vec![
                    row(
                        "Share this screen",
                        format!("view only · localhost:{PORT} · watched over ssh"),
                        Glyph::Eye,
                    )
                    .value("○ off"),
                    row(
                        "Allow remote control",
                        format!("viewer can type and click · localhost:{PORT}, over ssh"),
                        Glyph::Keyboard,
                    ),
                    row("Copy the tunnel command", tunnel_cmd(), Glyph::Link),
                    row(
                        "What this does",
                        "view looks only · control types here, over ssh",
                        Glyph::Info,
                    ),
                ],
                vec![
                    "start".into(),
                    "control".into(),
                    "tunnel".into(),
                    "info".into(),
                ],
                "Read-only by default · localhost:5900 · watched from another machine".to_string(),
            ),
        };

        let note = status_note.clone().unwrap_or(default_note);
        let mut picker = Picker::new(
            "Share this screen",
            vec![Question::choose("Share this screen", note, rows, keys)],
        );
        let code = crate::run_screen(terminal, design, "Share this screen", &mut picker)?;
        if code != 0 {
            return Ok(code);
        }
        let answers = picker.answers();
        if answers.is_empty() {
            return Ok(1);
        }
        match answers[0].as_str() {
            "start" => match start(Mode::View) {
                Ok(()) => {
                    status_note = Some(format!(
                        "Sharing read-only on localhost:{PORT}. Tunnel: {}",
                        tunnel_cmd()
                    ));
                }
                Err(e) => {
                    status_note = Some(format!("Could not start wayvnc: {e}"));
                }
            },
            "control" => match start(Mode::Control) {
                Ok(()) => {
                    status_note = Some(format!(
                        "Remote control is on: the viewer can type and click on localhost:{PORT}. Tunnel: {}",
                        tunnel_cmd()
                    ));
                }
                Err(e) => {
                    status_note = Some(format!("Could not start wayvnc: {e}"));
                }
            },
            "stop" => match stop() {
                Ok(()) => {
                    status_note = Some("Stopped sharing. The viewer lost the picture.".to_string());
                }
                Err(e) => {
                    status_note = Some(format!("Could not stop wayvnc: {e}"));
                }
            },
            "switch" => {
                let target = match mode {
                    Some(Mode::View) => Mode::Control,
                    _ => Mode::View,
                };
                match restart(target) {
                    Ok(()) => {
                        status_note = Some(format!(
                            "Now {} on localhost:{PORT}. Tunnel: {}",
                            target.label(),
                            tunnel_cmd()
                        ));
                    }
                    Err(e) => {
                        status_note = Some(format!("Could not switch mode: {e}"));
                    }
                }
            }
            "tunnel" => {
                let cmd = tunnel_cmd();
                let _ = out(&["wl-copy", &cmd]);
                status_note = Some(format!(
                    "Copied tunnel command ({cmd}) · connect VNC viewer to localhost:{PORT}"
                ));
            }
            "info" => {
                status_note = Some(match mode {
                    Some(Mode::Control) => format!(
                        "wayvnc serves 127.0.0.1:{PORT} with input: a viewer types into this session, over ssh"
                    ),
                    Some(Mode::View) => format!(
                        "wayvnc serves 127.0.0.1:{PORT} read-only; authenticate via SSH tunnel"
                    ),
                    None => format!(
                        "wayvnc serves 127.0.0.1:{PORT}; read-only unless remote control is on, and always over ssh"
                    ),
                });
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
    use super::*;

    #[test]
    fn tunnel_cmd_contains_port() {
        let cmd = tunnel_cmd();
        assert!(cmd.contains(&PORT.to_string()));
        assert!(cmd.contains("ssh -L"));
    }

    #[test]
    fn the_view_command_is_read_only_loopback() {
        let cmd = command_line(Mode::View).join(" ");
        assert!(cmd.contains("--disable-input"));
        assert!(cmd.contains("127.0.0.1"));
        assert!(!cmd.contains("0.0.0.0"));
    }

    #[test]
    fn the_control_command_keeps_loopback_and_drops_the_guard() {
        let cmd = command_line(Mode::Control).join(" ");
        assert!(!cmd.contains("--disable-input"));
        assert!(cmd.contains("127.0.0.1"));
        assert!(!cmd.contains("0.0.0.0"));
    }

    #[test]
    fn the_mode_comes_from_the_running_argv() {
        assert_eq!(
            mode_from_cmdline("nohup\x00wayvnc\x00--disable-input\x00127.0.0.1\x005900\x00"),
            Some(Mode::View)
        );
        assert_eq!(
            mode_from_cmdline("nohup\x00wayvnc\x00-d\x00127.0.0.1\x005900\x00"),
            Some(Mode::View)
        );
        assert_eq!(
            mode_from_cmdline("nohup\x00wayvnc\x00127.0.0.1\x005900\x00"),
            Some(Mode::Control)
        );
        assert_eq!(mode_from_cmdline("nohup\x00something-else\x00"), None);
    }

    #[test]
    fn a_missing_pid_is_not_alive() {
        assert!(alive(std::process::id()));
        assert!(!alive(u32::MAX));
    }

    #[test]
    fn status_names_the_mode() {
        assert_eq!(
            status_for(Some(Mode::View), true),
            "localhost:5900 · read-only"
        );
        assert_eq!(
            status_for(Some(Mode::Control), true),
            "localhost:5900 · remote control"
        );
        assert_eq!(status_for(Some(Mode::Control), false), "off");
        assert_eq!(status_for(None, true), "off");
    }

    #[test]
    fn the_bar_icon_changes_with_the_mode() {
        let view = bar_json(Mode::View);
        assert!(view.contains("\"class\":\"on\""));
        assert!(view.contains("read-only"));
        let control = bar_json(Mode::Control);
        assert!(control.contains("\"class\":\"control\""));
        assert!(control.contains("remote control"));
        assert!(control.contains("\u{f11c}"));
        assert!(empty_bar_json().contains("\"class\":\"empty\""));
    }
}
