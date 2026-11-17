//! The one recorder a session can have, and where its state lives.
//!
//! The capture menu starts `wf-recorder` and writes its pid and output file
//! under `$XDG_RUNTIME_DIR/lazyos-capture` only once the process is really the
//! recorder. Both ends of the session read the same files:
//!
//! - the menu offers **Stop recording** and signals that exact pid;
//! - `lazyos-notify` keeps non-critical cards off the screen while it runs,
//!   because a card drawn now is a card in the file (the "Recording" toast sat
//!   in the top-right corner of every capture and looked like a broken region).
//!
//! A pid is not proof by itself: a session that died can leave its number
//! behind, and the number can be reused. Like [`crate::task`], a pid counts as
//! the recorder only while `/proc/<pid>/comm` says `wf-recorder` and the process
//! is not a zombie. Everything takes the state directory as an argument as well
//! as from the environment, so a check can point it at a fixture.

use std::path::{Path, PathBuf};

/// The process name the compositor's recorder runs under.
pub const COMM: &str = "wf-recorder";

/// The directory under `$XDG_RUNTIME_DIR` that holds `recording.pid` and
/// `recording.file`. It dies with the session, which is what makes a stale
/// recorder impossible to remember after a reboot.
pub const DIR: &str = "lazyos-capture";

/// `$XDG_RUNTIME_DIR/lazyos-capture`, or `/tmp/lazyos-capture` in a session
/// that has no runtime directory.
pub fn state_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(DIR),
        None => PathBuf::from("/tmp").join(DIR),
    }
}

/// The pid the capture menu saved, if the file is readable and a number.
pub fn pid_in(state: &Path) -> Option<u32> {
    std::fs::read_to_string(state.join("recording.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The file the recorder is writing, if the capture menu saved one.
pub fn file_in(state: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(state.join("recording.file")).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// Is this pid the recorder itself? `/proc/<pid>/comm` is the same test
/// [`crate::task`] makes of a task it did not start, so a reused number is not
/// mistaken for a live capture.
pub fn is_recorder(pid: u32) -> bool {
    matches_comm(read_proc_comm(pid).as_deref()) && !is_zombie(pid)
}

/// Is a capture running in this state directory?
pub fn is_recording_in(state: &Path) -> bool {
    pid_in(state).is_some_and(is_recorder)
}

/// Is a capture running in this session?
pub fn is_recording() -> bool {
    is_recording_in(&state_dir())
}

/// Forget the recorder. Safe when there was none.
pub fn clear_in(state: &Path) {
    let _ = std::fs::remove_file(state.join("recording.pid"));
    let _ = std::fs::remove_file(state.join("recording.file"));
}

/// Forget this session's recorder. Safe when there was none.
pub fn clear() {
    clear_in(&state_dir());
}

/// Would a card of this urgency be drawn right now?
///
/// Critical cards always draw: a failing disk or a dying battery matters more
/// than a clean recording. Everything else waits, because the recorder is
/// writing the screen to a file.
pub fn would_draw(critical: bool) -> bool {
    critical || !is_recording()
}

fn matches_comm(comm: Option<&str>) -> bool {
    comm == Some(COMM)
}

fn read_proc_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .ok()
}

/// `/proc/<pid>/stat` is `pid (comm) state ...`; a `Z` state has exited and is
/// only waiting to be reaped, so it is not a recorder even though its name and
/// pid still answer `kill -0`.
fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let (_, rest) = stat.rsplit_once(')')?;
            rest.split_whitespace().next().map(|state| state == "Z")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lazyos-recording-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_the_recorder_name_matches() {
        assert!(matches_comm(Some(COMM)));
        assert!(!matches_comm(Some("sh")));
        assert!(!matches_comm(Some("wf-record")));
        assert!(!matches_comm(None));
    }

    #[test]
    fn no_state_is_not_a_recording() {
        let state = temp_dir("empty");
        assert!(!is_recording_in(&state));
        assert!(pid_in(&state).is_none());
        assert!(file_in(&state).is_none());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_live_pid_that_is_not_the_recorder_is_stale() {
        let state = temp_dir("stale");
        std::fs::write(
            state.join("recording.pid"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        std::fs::write(state.join("recording.file"), "/tmp/lazyos-test.mp4\n").unwrap();
        // The test binary's comm is not wf-recorder, so this is a reused or
        // abandoned pid, never a capture.
        assert!(!is_recording_in(&state));
        assert_eq!(pid_in(&state), Some(std::process::id()));
        assert_eq!(file_in(&state), Some(PathBuf::from("/tmp/lazyos-test.mp4")));
        clear_in(&state);
        assert!(pid_in(&state).is_none());
        assert!(file_in(&state).is_none());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_nonsense_pid_is_not_a_recording() {
        let state = temp_dir("nonsense");
        std::fs::write(state.join("recording.pid"), "not a pid\n").unwrap();
        assert!(!is_recording_in(&state));
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn critical_cards_always_draw() {
        assert!(would_draw(true));
    }
}
