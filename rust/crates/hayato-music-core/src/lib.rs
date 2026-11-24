//! Core domain model, queue, audio engine, DSP spectrum/beat analyzer, library
//! indexer, MPRIS2 server, and Spotify integration for **Hayato Cassette**.
//!
//! Zero UI toolkit dependencies so every subsystem is pure, fast to compile,
//! and independently testable.

#[cfg(feature = "spotify")]
pub mod connect;
pub mod dsp;
pub mod engine;
pub mod library;
pub mod lyrics;
pub mod mpris;
pub mod providers;
#[cfg(feature = "spotify")]
pub mod spotify;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Stable unique track identifier across local files and Spotify URIs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackId {
    Local(u64),
    Spotify(String),
}

/// Codec & stream telemetry shown in the Cyber-HUD deck and Waybar tooltip.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecInfo {
    pub format: String,
    pub sample_rate_hz: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
}

impl CodecInfo {
    pub fn short_label(&self) -> String {
        let mut out = if self.format.is_empty() {
            "AUDIO".to_string()
        } else {
            self.format.to_uppercase()
        };
        if self.sample_rate_hz > 0 {
            let khz = self.sample_rate_hz as f32 / 1000.0;
            if (khz - khz.round()).abs() < 0.05 {
                out.push_str(&format!(" · {}kHz", khz.round() as u32));
            } else {
                out.push_str(&format!(" · {khz:.1}kHz"));
            }
        }
        if let Some(bits) = self.bit_depth {
            out.push_str(&format!(" · {bits}-bit"));
        } else if let Some(kbps) = self.bitrate_kbps {
            out.push_str(&format!(" · {kbps}k"));
        }
        out
    }
}

/// Where a track's audio bytes come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Local { path: PathBuf, codec: CodecInfo },
    Spotify { uri: String, url: String },
}

/// Unified metadata for one playable track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: u64,
    pub source: SourceKind,
    /// Optional cached album art path on disk (PNG/JPEG).
    pub art_path: Option<PathBuf>,
}

impl Track {
    pub fn is_spotify(&self) -> bool {
        matches!(self.source, SourceKind::Spotify { .. })
    }

    pub fn source_badge(&self) -> &'static str {
        if self.is_spotify() {
            "SPOTIFY"
        } else {
            "LOCAL"
        }
    }

    pub fn share_target(&self) -> (bool, String) {
        match &self.source {
            SourceKind::Local { path, .. } => (true, path.to_string_lossy().into_owned()),
            SourceKind::Spotify { url, uri } => {
                let target = if url.is_empty() { uri.clone() } else { url.clone() };
                (false, target)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub is_spotify: bool,
    pub path: Option<PathBuf>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "REP:-",
            Self::All => "REP:A",
            Self::One => "REP:1",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Persistent user preferences for Hayato Cassette (`~/.config/hayato/cassette/config.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CassetteConfig {
    pub music_dirs: Vec<PathBuf>,
    pub visualizer: VisualizerMode,
    pub volume_pct: u8,
    pub show_lyrics: bool,
    #[serde(default)]
    pub notify_when_unfocused: bool,
    #[serde(default)]
    pub mini_mode: bool,
    pub spotify_client_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisualizerMode {
    #[default]
    Spectrum,
    Mirror,
    Oscilloscope,
    VuDeck,
    Plasma,
    Off,
}

impl VisualizerMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Spectrum => Self::Mirror,
            Self::Mirror => Self::Oscilloscope,
            Self::Oscilloscope => Self::VuDeck,
            Self::VuDeck => Self::Plasma,
            Self::Plasma => Self::Off,
            Self::Off => Self::Spectrum,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "Spectrum",
            Self::Mirror => "Mirror EQ",
            Self::Oscilloscope => "Oscilloscope",
            Self::VuDeck => "VU Deck",
            Self::Plasma => "Cyber Plasma",
            Self::Off => "Off",
        }
    }
}

impl Default for CassetteConfig {
    fn default() -> Self {
        let default_dir = dirs::audio_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Music"));
        Self {
            music_dirs: vec![default_dir],
            visualizer: VisualizerMode::Spectrum,
            volume_pct: 80,
            show_lyrics: false,
            notify_when_unfocused: false,
            mini_mode: false,
            spotify_client_id: String::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("hayato")
        .join("cassette")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
        .join("hayato")
        .join("cassette")
}

pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("hayato-cassette")
}

pub fn load_config() -> CassetteConfig {
    let path = config_dir().join("config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &CassetteConfig) -> Result<(), String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {e}"))?;
    let path = dir.join("config.json");
    let tmp = dir.join("config.json.tmp");
    let json = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&tmp, json).map_err(|e| format!("write config: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("commit config: {e}"))?;
    Ok(())
}

/// Up-Next playback queue with shuffle, repeat, and reordering.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    items: Vec<Track>,
    current: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    shuffle_seed: u64,
}

impl Queue {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            shuffle_seed: seed | 1,
            ..Self::default()
        }
    }

    pub fn items(&self) -> &[Track] {
        &self.items
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.items.get(i))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn set_and_play(&mut self, tracks: Vec<Track>, start_idx: usize) -> Option<&Track> {
        self.items = tracks;
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        self.current = Some(start_idx.min(self.items.len() - 1));
        self.current_track()
    }

    pub fn push_back(&mut self, track: Track) {
        self.items.push(track);
    }

    pub fn push_next(&mut self, track: Track) {
        match self.current {
            Some(i) if i < self.items.len() => self.items.insert(i + 1, track),
            _ => self.items.push(track),
        }
    }

    pub fn remove(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        self.items.remove(idx);
        if self.items.is_empty() {
            self.current = None;
        } else if let Some(cur) = self.current {
            if idx < cur {
                self.current = Some(cur - 1);
            } else if idx == cur && cur >= self.items.len() {
                self.current = Some(self.items.len() - 1);
            }
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
    }

    pub fn move_up(&mut self, idx: usize) -> Option<usize> {
        if idx == 0 || idx >= self.items.len() {
            return None;
        }
        self.items.swap(idx, idx - 1);
        if let Some(cur) = self.current {
            if cur == idx {
                self.current = Some(idx - 1);
            } else if cur == idx - 1 {
                self.current = Some(idx);
            }
        }
        Some(idx - 1)
    }

    pub fn move_down(&mut self, idx: usize) -> Option<usize> {
        if idx + 1 >= self.items.len() {
            return None;
        }
        self.items.swap(idx, idx + 1);
        if let Some(cur) = self.current {
            if cur == idx {
                self.current = Some(idx + 1);
            } else if cur == idx + 1 {
                self.current = Some(idx);
            }
        }
        Some(idx + 1)
    }

    pub fn jump(&mut self, idx: usize) -> Option<&Track> {
        if idx < self.items.len() {
            self.current = Some(idx);
            self.current_track()
        } else {
            None
        }
    }

    pub fn advance_next(&mut self, auto_end: bool) -> Option<&Track> {
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        let cur = self.current.unwrap_or(0);
        if auto_end && self.repeat == RepeatMode::One {
            self.current = Some(cur);
            return self.current_track();
        }
        if self.shuffle && self.items.len() > 1 {
            // xorshift64*
            let mut x = self.shuffle_seed;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.shuffle_seed = x;
            let mut next = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) as usize) % self.items.len();
            if next == cur {
                next = (next + 1) % self.items.len();
            }
            self.current = Some(next);
            return self.current_track();
        }
        if cur + 1 < self.items.len() {
            self.current = Some(cur + 1);
            self.current_track()
        } else if self.repeat == RepeatMode::All {
            self.current = Some(0);
            self.current_track()
        } else {
            None
        }
    }

    pub fn advance_prev(&mut self) -> Option<&Track> {
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        let cur = self.current.unwrap_or(0);
        if cur > 0 {
            self.current = Some(cur - 1);
        } else if self.repeat == RepeatMode::All {
            self.current = Some(self.items.len() - 1);
        } else {
            self.current = Some(0);
        }
        self.current_track()
    }

    pub fn remaining_duration_secs(&self) -> u64 {
        match self.current {
            Some(i) => self.items[i..].iter().map(|t| t.duration_secs).sum(),
            None => self.items.iter().map(|t| t.duration_secs).sum(),
        }
    }
}

/// Lightweight atomic state written to `$XDG_RUNTIME_DIR/hayato-cassette/now.json`
/// so `hayato-cassette --bar` responds in `<1 ms` with zero D-Bus overhead.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NowPlayingFile {
    pub pid: u32,
    pub state: PlayState,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source_label: String,
    pub codec_label: String,
    pub position_secs: u64,
    pub duration_secs: u64,
    pub volume_pct: u8,
}

impl NowPlayingFile {
    pub fn path() -> PathBuf {
        runtime_dir().join("now.json")
    }

    pub fn write_atomic(&self) {
        let dir = runtime_dir();
        let _ = std::fs::create_dir_all(&dir);
        let tmp = dir.join("now.json.tmp");
        let dst = dir.join("now.json");
        if let Ok(json) = serde_json::to_string(self)
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(&tmp, &dst);
        }
    }

    pub fn clear() {
        let _ = std::fs::remove_file(Self::path());
    }

    pub fn read_live() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        let data: Self = serde_json::from_str(&text).ok()?;
        if data.pid == 0 || !Path::new(&format!("/proc/{}", data.pid)).exists() {
            return None;
        }
        Some(data)
    }
}

/// Format seconds into `MM:SS` (or `H:MM:SS` if >= 1 hour).
pub fn format_time(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Stable 64-bit FNV-1a hash for local file paths.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_track(n: u64) -> Track {
        Track {
            id: TrackId::Local(n),
            title: format!("Track {n}"),
            artist: "Artist".into(),
            album: "Album".into(),
            track_number: Some(n as u32),
            year: Some(2025),
            duration_secs: 180,
            source: SourceKind::Local {
                path: PathBuf::from(format!("/music/t{n}.flac")),
                codec: CodecInfo {
                    format: "FLAC".into(),
                    sample_rate_hz: 48000,
                    bit_depth: Some(24),
                    bitrate_kbps: None,
                },
            },
            art_path: None,
        }
    }

    #[test]
    fn queue_reorder_repeat_and_advance() {
        let mut q = Queue::new();
        q.set_and_play(vec![dummy_track(1), dummy_track(2), dummy_track(3)], 0);
        assert_eq!(q.current_track().unwrap().title, "Track 1");
        assert_eq!(q.move_down(0), Some(1));
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.advance_next(false).unwrap().title, "Track 3");
        assert!(q.advance_next(true).is_none());
        q.repeat = RepeatMode::All;
        assert_eq!(q.advance_next(true).unwrap().title, "Track 2");
    }

    #[test]
    fn codec_and_time_formatting() {
        let c = CodecInfo {
            format: "flac".into(),
            sample_rate_hz: 48000,
            bit_depth: Some(24),
            bitrate_kbps: None,
        };
        assert_eq!(c.short_label(), "FLAC · 48kHz · 24-bit");
        assert_eq!(format_time(134), "02:14");
        assert_eq!(format_time(3665), "1:01:05");
    }
}
