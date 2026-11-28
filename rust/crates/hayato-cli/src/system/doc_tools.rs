//! The two GUI document tools, started with the session's dark preference.
//!
//! `foliate` is GTK4/libadwaita and `xournalpp` is GTK3. Both read the colour
//! scheme from the settings portal (`org.freedesktop.appearance`), which this
//! image does not ship, or from their own defaults, so they opened light beside
//! a dark session. `GTK_THEME=Adwaita:dark` is GTK's own override: the chrome
//! goes dark and a document page stays white, which is what a reader wants.
//!
//! One helper so the Notes menu, Files' actions and `hayato open` cannot start
//! the same program with three different environments.

use std::path::Path;
use std::process::Command;

/// GTK's supported dark override, and the only variable both toolkits honour.
pub const DARK: (&str, &str) = ("GTK_THEME", "Adwaita:dark");

/// The environment a program needs to match the session, if any.
pub fn dark_env(program: &str) -> Option<(&'static str, &'static str)> {
    matches!(program, "foliate" | "xournalpp").then_some(DARK)
}

/// Start a GUI document tool detached, in its own session.
pub fn spawn(program: &str, args: &[&str]) {
    spawn_in(None, program, args);
}

/// The same, with `dir` as the working directory: xournalpp's save dialog then
/// starts in the notes folder.
pub fn spawn_in(dir: Option<&Path>, program: &str, args: &[&str]) {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    if let Some((key, value)) = dark_env(program) {
        command.env(key, value);
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let _ = command.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_document_tools_get_the_dark_theme() {
        assert_eq!(dark_env("foliate"), Some(DARK));
        assert_eq!(dark_env("xournalpp"), Some(DARK));
        assert_eq!(dark_env("papers"), None);
        assert_eq!(dark_env("chromium"), None);
    }
}
