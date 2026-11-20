//! The one recorder a session can have, and where its state lives.
//!
//! The capture menu starts `wf-recorder` and writes its pid and output file
//! under `$XDG_RUNTIME_DIR/hayato-capture` only once the process is really the
//! recorder. Both ends of the session read the same files:
//!
//! - the menu offers **Stop recording** and signals that exact pid;
//! - `hayato-notify` keeps non-critical cards off the screen while it runs,
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
pub const DIR: &str = "hayato-capture";

/// `$XDG_RUNTIME_DIR/hayato-capture`, or `/tmp/hayato-capture` in a session
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

/// What the state directory says about the session's one recorder.
///
/// `Idle` and `Recording` are the only two answers, and `Recording` carries
/// the pid and file together: a caller cannot hold a pid from one read and a
/// file from another, or offer Stop after the recorder is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recorder {
    Idle,
    Recording {
        pid: u32,
        /// The file the recorder is writing. `None` only when a recorder is
        /// alive but its file was not saved, which the menu shows as an empty
        /// detail rather than as a different state.
        file: Option<PathBuf>,
    },
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        matches!(self, Self::Recording { .. })
    }

    pub fn pid(&self) -> Option<u32> {
        match self {
            Self::Recording { pid, .. } => Some(*pid),
            Self::Idle => None,
        }
    }

    pub fn file(&self) -> Option<&Path> {
        match self {
            Self::Recording { file, .. } => file.as_deref(),
            Self::Idle => None,
        }
    }
}

/// Read the recorder state from `state`, without clearing a stale pid: the
/// caller decides whether to forget it. A pid that is not the recorder is
/// `Idle`, because a reused number is not a capture.
pub fn read_in(state: &Path) -> Recorder {
    let Some(pid) = pid_in(state) else {
        return Recorder::Idle;
    };
    if !is_recorder(pid) {
        return Recorder::Idle;
    }
    Recorder::Recording {
        pid,
        file: file_in(state),
    }
}

/// Read this session's recorder state.
pub fn read() -> Recorder {
    read_in(&state_dir())
}

/// What to record beside the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMode {
    Off,
    Mic,
    System,
}

impl AudioMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mic => "mic",
            Self::System => "system",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "off" => Some(Self::Off),
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "no audio",
            Self::Mic => "microphone",
            Self::System => "system audio (line out)",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Mic,
            Self::Mic => Self::System,
            Self::System => Self::Off,
        }
    }
}

/// The webinar options saved between recordings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub audio: AudioMode,
    pub webcam: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            audio: AudioMode::Off,
            webcam: false,
        }
    }
}

impl Options {
    pub fn summary(self) -> String {
        format!(
            "{} · camera {}",
            self.audio.label(),
            if self.webcam { "on" } else { "off" }
        )
    }
}

/// `~/.config/hayato/record.conf`: what the next recording includes.
pub fn options_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        });
    base.join("hayato/record.conf")
}

fn parse_options(text: &str) -> Options {
    let mut options = Options::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "audio" => {
                if let Some(mode) = AudioMode::parse(value) {
                    options.audio = mode;
                }
            }
            "webcam" => options.webcam = value.trim() == "on",
            _ => {}
        }
    }
    options
}

fn options_body(options: Options) -> String {
    format!(
        "# Hayato recording options. Read by `hayato menu capture --start`.\n\
         audio={}\nwebcam={}\n",
        options.audio.as_str(),
        if options.webcam { "on" } else { "off" }
    )
}

pub fn load_options() -> Options {
    std::fs::read_to_string(options_path())
        .map(|text| parse_options(&text))
        .unwrap_or_default()
}

pub fn save_options(options: Options) -> std::io::Result<()> {
    let path = options_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, options_body(options))?;
    std::fs::rename(tmp, path)
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
    let _ = std::fs::remove_file(state.join("camera.pid"));
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
            std::env::temp_dir().join(format!("hayato-recording-{name}-{}", std::process::id()));
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
        assert_eq!(read_in(&state), Recorder::Idle);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_recorder_carries_its_pid_and_file_together() {
        let recorder = Recorder::Recording {
            pid: 7,
            file: Some(PathBuf::from("/tmp/take.mp4")),
        };
        assert!(recorder.is_recording());
        assert_eq!(recorder.pid(), Some(7));
        assert_eq!(recorder.file(), Some(Path::new("/tmp/take.mp4")));
        assert_eq!(Recorder::Idle.pid(), None);
        assert_eq!(Recorder::Idle.file(), None);
    }

    #[test]
    fn a_live_pid_that_is_not_the_recorder_is_stale() {
        let state = temp_dir("stale");
        std::fs::write(
            state.join("recording.pid"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        std::fs::write(state.join("recording.file"), "/tmp/hayato-test.mp4\n").unwrap();
        // The test binary's comm is not wf-recorder, so this is a reused or
        // abandoned pid, never a capture.
        assert!(!is_recording_in(&state));
        assert_eq!(pid_in(&state), Some(std::process::id()));
        assert_eq!(file_in(&state), Some(PathBuf::from("/tmp/hayato-test.mp4")));
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
    fn record_options_round_trip_and_cycle() {
        let options = Options {
            audio: AudioMode::Mic,
            webcam: true,
        };
        let text = options_body(options);
        assert_eq!(parse_options(&text), options);
        assert_eq!(AudioMode::Off.cycle(), AudioMode::Mic);
        assert_eq!(AudioMode::Mic.cycle(), AudioMode::System);
        assert_eq!(AudioMode::System.cycle(), AudioMode::Off);
        assert_eq!(AudioMode::System.label(), "system audio (line out)");
    }

    #[test]
    fn critical_cards_always_draw() {
        assert!(would_draw(true));
    }
}
