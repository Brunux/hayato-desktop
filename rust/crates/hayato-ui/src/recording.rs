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

use serde::{Deserialize, Serialize};

/// The process name the compositor's recorder runs under.
pub const COMM: &str = "wf-recorder";

/// The directory under `$XDG_RUNTIME_DIR` that holds `recording.pid` and
/// `recording.file`. It dies with the session, which is what makes a stale
/// recorder impossible to remember after a reboot.
pub const DIR: &str = "hayato-capture";

/// The directory under `$XDG_RUNTIME_DIR` that holds Hayato Studio's live
/// recording session (`session.json`, PipeWire mixer module IDs, `mpv.sock`).
pub const STUDIO_DIR: &str = "hayato-studio";

/// `$XDG_RUNTIME_DIR/hayato-capture`, or `/tmp/hayato-capture` in a session
/// that has no runtime directory.
pub fn state_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(DIR),
        None => PathBuf::from("/tmp").join(DIR),
    }
}

/// `$XDG_RUNTIME_DIR/hayato-studio`, or `/tmp/hayato-studio` when unset.
pub fn studio_state_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(STUDIO_DIR),
        None => PathBuf::from("/tmp").join(STUDIO_DIR),
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

/// What to record beside the screen or webcam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioMode {
    Off,
    Mic,
    System,
    Both,
}

impl AudioMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mic => "mic",
            Self::System => "system",
            Self::Both => "both",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "off" => Some(Self::Off),
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "no audio",
            Self::Mic => "microphone",
            Self::System => "system audio (line out)",
            Self::Both => "mic + system (mixed)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "Muted",
            Self::Mic => "Mic",
            Self::System => "System",
            Self::Both => "Mic + System",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Mic,
            Self::Mic => Self::System,
            Self::System => Self::Both,
            Self::Both => Self::Off,
        }
    }
}

/// Simple quality presets tuned for MT8192 8-core ARM64 / 4 GB RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    Compact,
    Balanced,
    Crisp,
}

impl QualityPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Balanced => "balanced",
            Self::Crisp => "crisp",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "compact" => Some(Self::Compact),
            "balanced" => Some(Self::Balanced),
            "crisp" => Some(Self::Crisp),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact (24fps)",
            Self::Balanced => "Balanced (30fps)",
            Self::Crisp => "Crisp (30fps HQ)",
        }
    }

    pub fn fps(self) -> u32 {
        match self {
            Self::Compact => 24,
            Self::Balanced | Self::Crisp => 30,
        }
    }

    pub fn x264_preset(self) -> &'static str {
        match self {
            Self::Compact => "ultrafast",
            Self::Balanced => "veryfast",
            Self::Crisp => "faster",
        }
    }

    pub fn crf(self) -> u8 {
        match self {
            Self::Compact => 30,
            Self::Balanced => 26,
            Self::Crisp => 22,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Compact => Self::Balanced,
            Self::Balanced => Self::Crisp,
            Self::Crisp => Self::Compact,
        }
    }
}

/// Webcam Picture-in-Picture position presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl PipCorner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "tl",
            Self::TopRight => "tr",
            Self::BottomLeft => "bl",
            Self::BottomRight => "br",
            Self::Center => "center",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::TopLeft => "TL",
            Self::TopRight => "TR",
            Self::BottomLeft => "BL",
            Self::BottomRight => "BR",
            Self::Center => "CTR",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "tl" | "top-left" => Some(Self::TopLeft),
            "tr" | "top-right" => Some(Self::TopRight),
            "bl" | "bottom-left" => Some(Self::BottomLeft),
            "br" | "bottom-right" => Some(Self::BottomRight),
            "center" | "ctr" | "c" => Some(Self::Center),
            _ => None,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::TopLeft => Self::TopRight,
            Self::TopRight => Self::BottomRight,
            Self::BottomRight => Self::BottomLeft,
            Self::BottomLeft => Self::Center,
            Self::Center => Self::TopLeft,
        }
    }
}

/// Webcam Picture-in-Picture size presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipSize {
    Small,
    Medium,
    Large,
}

impl PipSize {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "s",
            Self::Medium => "m",
            Self::Large => "l",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "s" | "small" => Some(Self::Small),
            "m" | "medium" => Some(Self::Medium),
            "l" | "large" => Some(Self::Large),
            _ => None,
        }
    }

    pub fn dims(self) -> (i64, i64) {
        match self {
            Self::Small => (240, 180),
            Self::Medium => (320, 240),
            Self::Large => (480, 360),
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Large,
            Self::Large => Self::Small,
        }
    }
}

/// Persisted Studio / Capture options (`~/.config/hayato/record.conf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub audio: AudioMode,
    pub webcam: bool,
    pub quality: QualityPreset,
    pub corner: PipCorner,
    pub pip_size: PipSize,
    pub mirror: bool,
    pub sticky: bool,
    pub timer_secs: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            audio: AudioMode::Off,
            webcam: false,
            quality: QualityPreset::Balanced,
            corner: PipCorner::BottomRight,
            pip_size: PipSize::Medium,
            mirror: true,
            sticky: true,
            timer_secs: 0,
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

/// Active Hayato Studio live recording session (`$XDG_RUNTIME_DIR/hayato-studio/session.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudioSession {
    pub kind: String,
    pub pid: u32,
    pub comm: String,
    pub file: PathBuf,
    pub started_epoch: u64,
    pub quality: QualityPreset,
    pub audio: AudioMode,
    pub geometry: Option<String>,
    pub cam_pid: Option<u32>,
    pub cam_visible: bool,
    pub cam_sticky: bool,
    pub cam_corner: PipCorner,
    pub cam_size: PipSize,
    pub cam_mirror: bool,
}

pub fn read_studio_session_in(dir: &Path) -> Option<StudioSession> {
    let raw = std::fs::read_to_string(dir.join("session.json")).ok()?;
    let session: StudioSession = serde_json::from_str(&raw).ok()?;
    let live_comm = read_proc_comm(session.pid)?;
    if (live_comm == session.comm || live_comm == "wf-recorder" || live_comm == "ffmpeg")
        && !is_zombie(session.pid)
    {
        Some(session)
    } else {
        None
    }
}

pub fn read_studio_session() -> Option<StudioSession> {
    read_studio_session_in(&studio_state_dir())
}

pub fn write_studio_session_in(dir: &Path, session: &StudioSession) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("session.json.part");
    let json = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, dir.join("session.json"))
}

pub fn write_studio_session(session: &StudioSession) -> std::io::Result<()> {
    write_studio_session_in(&studio_state_dir(), session)
}

pub fn clear_studio_session_in(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("session.json"));
}

pub fn clear_studio_session() {
    clear_studio_session_in(&studio_state_dir());
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
            "quality" => {
                if let Some(q) = QualityPreset::parse(value) {
                    options.quality = q;
                }
            }
            "corner" => {
                if let Some(c) = PipCorner::parse(value) {
                    options.corner = c;
                }
            }
            "pip_size" => {
                if let Some(s) = PipSize::parse(value) {
                    options.pip_size = s;
                }
            }
            "mirror" => options.mirror = value.trim() != "off",
            "sticky" => options.sticky = value.trim() != "off",
            "timer" => {
                if let Ok(t) = value.trim().parse::<u8>() {
                    options.timer_secs = match t {
                        3 => 3,
                        5 => 5,
                        _ => 0,
                    };
                }
            }
            _ => {}
        }
    }
    options
}

fn options_body(options: Options) -> String {
    format!(
        "# Hayato Studio & capture options.\n\
         audio={}\nwebcam={}\nquality={}\ncorner={}\npip_size={}\nmirror={}\nsticky={}\ntimer={}\n",
        options.audio.as_str(),
        if options.webcam { "on" } else { "off" },
        options.quality.as_str(),
        options.corner.as_str(),
        options.pip_size.as_str(),
        if options.mirror { "on" } else { "off" },
        if options.sticky { "on" } else { "off" },
        options.timer_secs,
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

/// Is a capture running in this session (either Hayato Studio or legacy capture)?
pub fn is_recording() -> bool {
    read_studio_session().is_some() || is_recording_in(&state_dir())
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
            ..Options::default()
        };
        let text = options_body(options);
        assert_eq!(parse_options(&text), options);
        assert_eq!(AudioMode::Off.cycle(), AudioMode::Mic);
        assert_eq!(AudioMode::Mic.cycle(), AudioMode::System);
        assert_eq!(AudioMode::System.cycle(), AudioMode::Both);
        assert_eq!(AudioMode::Both.cycle(), AudioMode::Off);
        assert_eq!(AudioMode::System.label(), "system audio (line out)");
    }

    #[test]
    fn critical_cards_always_draw() {
        assert!(would_draw(true));
    }
}
