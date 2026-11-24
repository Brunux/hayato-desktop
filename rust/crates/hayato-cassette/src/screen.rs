//! Main Cyberdeck TUI for **Hayato Cassette** (`CassetteScreen`).
//!
//! - Single `CassettePhase` enum for mutually exclusive overlays/modals.
//! - Zero `Color::Rgb` (uses `Design::color` and `Design::ramp` LUTs).
//! - Renders strictly into `hayato_ui::BodyCtx`.
//! - Pre-formatted status strings rebuilt only on state/second changes for
//!   zero steady-state frame allocations.

use std::cell::Cell;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use hayato_music_core::dsp::{BANDS, SpectrumAnalyzer, WAVE_POINTS};
use hayato_music_core::engine::AudioEngine;
use hayato_music_core::library::{
    load_cached_tracks, load_local_playlists, read_track_metadata, save_m3u8_playlist,
    scan_directories,
};
use hayato_music_core::lyrics::Lyrics;
use hayato_music_core::mpris::{MprisCommand, MprisHandle, notify_track_change};
#[cfg(feature = "spotify")]
use hayato_music_core::spotify::{self, SpotifyDevice, SpotifyToken};
use hayato_music_core::{
    CassetteConfig, NowPlayingFile, PlayState, Playlist, Queue, SourceKind, Track, VisualizerMode,
    cache_dir, format_time, load_config, save_config,
};
use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, Input, InputAction, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_badge, draw_card,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use tachyonfx::{Effect, EffectTimer, Interpolation, Motion, fx};
use tui_big_text::{BigText, PixelSize};

use crate::art::AlbumArt;
use crate::sway_ipc;

/// Global flag set by `SIGUSR1` when `hayato theme set` reloads palettes live.
pub static THEME_RELOAD_FLAG: AtomicBool = AtomicBool::new(false);

const BLOCKS: &[&str] = &[" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Deck,
    Library,
    Queue,
    Search,
    Playlists,
    Spotify,
    Settings,
}

impl Tab {
    const ALL: &[Tab] = &[
        Tab::Deck,
        Tab::Library,
        Tab::Queue,
        Tab::Search,
        Tab::Playlists,
        Tab::Spotify,
        Tab::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Deck => "1 Deck",
            Tab::Library => "2 Library",
            Tab::Queue => "3 Queue",
            Tab::Search => "4 Search",
            Tab::Playlists => "5 Playlists",
            Tab::Spotify => "6 Spotify",
            Tab::Settings => ", Settings",
        }
    }
}

enum CassettePhase {
    Normal,
    ConfirmClearQueue(Confirm),
    ConfirmLogoutSpotify(Confirm),
    Palette(ListState),
    EditClientId(Input),
}

pub struct CassetteScreen {
    config: CassetteConfig,
    tab: Tab,
    mini: bool,
    focused: bool,
    unfocused_since: Option<Instant>,
    phase: CassettePhase,

    // Core audio & library state
    engine: AudioEngine,
    analyzer: SpectrumAnalyzer,
    queue: Queue,
    library: Vec<Track>,
    playlists: Vec<Playlist>,
    lyrics: Option<Lyrics>,
    art: AlbumArt,
    mpris: MprisHandle,
    mpris_rx: Receiver<MprisCommand>,
    scan_rx: Option<Receiver<Vec<Track>>>,

    // Spotify state
    #[cfg(feature = "spotify")]
    spotify_token: Option<SpotifyToken>,
    #[cfg(feature = "spotify")]
    spotify_devices: Vec<SpotifyDevice>,
    #[cfg(feature = "spotify")]
    spotify_search: Vec<Track>,
    #[cfg(feature = "spotify")]
    spotify_remote_playing: bool,
    #[cfg(feature = "spotify")]
    spotify_phase: f32,

    // Cached lists & UI state
    lib_list: ListState,
    queue_list: ListState,
    search_input: Input,
    search_list: ListState,
    search_results: Vec<Track>,
    playlist_list: ListState,
    spotify_list: ListState,
    settings_list: ListState,
    rows_height: Cell<u16>,
    scrubber_rect: Cell<Rect>,

    // Telemetry & pre-formatted strings (zero per-frame format!)
    last_finished_gen: u64,
    last_pos_sec: u64,
    last_dur_sec: u64,
    time_buf: String,
    codec_buf: String,
    status_line: String,
    header: Header,
    keybar: Vec<KeyHint>,

    // Motion & precomputed colour LUTs
    design_override: Option<Design>,
    border_ramp: [Color; 8],
    spectrum_ramp: [Color; 6],
    fx: Option<Effect>,
    last_fx_tick: Instant,
    headless_test: bool,
}

impl CassetteScreen {
    pub fn new(initial_play: Option<PathBuf>, start_mini: bool, design: &Design) -> Self {
        let config = load_config();
        let (engine, analyzer) = AudioEngine::spawn(config.volume_pct);
        let (mpris_tx, mpris_rx) = mpsc::channel();
        let mpris = MprisHandle::spawn(mpris_tx);

        let library = load_cached_tracks();
        let playlists = load_local_playlists(&config.music_dirs, &library);

        // Kick off incremental background rescan
        let dirs_clone = config.music_dirs.clone();
        let (scan_tx, scan_rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("cassette-scan".into())
            .spawn(move || {
                let tracks = scan_directories(&dirs_clone);
                let _ = scan_tx.send(tracks);
            });

        #[cfg(feature = "spotify")]
        let spotify_token = spotify::load_token();

        let mut screen = Self {
            config,
            tab: Tab::Deck,
            mini: start_mini,
            focused: true,
            unfocused_since: None,
            phase: CassettePhase::Normal,
            engine,
            analyzer,
            queue: Queue::new(),
            library,
            playlists,
            lyrics: None,
            art: AlbumArt::from_env(),
            mpris,
            mpris_rx,
            scan_rx: Some(scan_rx),
            #[cfg(feature = "spotify")]
            spotify_token,
            #[cfg(feature = "spotify")]
            spotify_devices: Vec::new(),
            #[cfg(feature = "spotify")]
            spotify_search: Vec::new(),
            #[cfg(feature = "spotify")]
            spotify_remote_playing: false,
            #[cfg(feature = "spotify")]
            spotify_phase: 0.0,
            lib_list: ListState::new().with_empty("No audio files found — press R to scan ~/Music"),
            queue_list: ListState::new().with_empty("Queue is empty — press 2 to browse Library"),
            search_input: Input::new(),
            search_list: ListState::new().with_empty("Type to search local library & Spotify"),
            search_results: Vec::new(),
            playlist_list: ListState::new().with_empty("No playlists yet — press s in Queue to save"),
            spotify_list: ListState::new().with_empty("Not connected to Spotify"),
            settings_list: ListState::new(),
            rows_height: Cell::new(14),
            scrubber_rect: Cell::new(Rect::default()),
            last_finished_gen: 0,
            last_pos_sec: 0,
            last_dur_sec: 0,
            time_buf: "00:00 / 00:00".to_string(),
            codec_buf: "IDLE".to_string(),
            status_line: "Ready · Space play · / search · : palette · Alt+M hide".to_string(),
            header: Header::new("Hayato").subtitle("Cassette"),
            keybar: default_keybar(),
            design_override: None,
            border_ramp: design.ramp(ColorRole::Border, ColorRole::Accent),
            spectrum_ramp: design.ramp(ColorRole::AccentAlt, ColorRole::Accent),
            fx: None,
            last_fx_tick: Instant::now(),
            headless_test: false,
        };

        screen.rebuild_library_rows();
        screen.rebuild_playlist_rows();
        screen.rebuild_spotify_rows();
        screen.rebuild_settings_rows();
        screen.sync_chrome();

        if let Some(path) = initial_play
            && path.is_file()
        {
            let t = read_track_metadata(&path, &cache_dir().join("art"));
            screen.play_tracks(vec![t], 0);
        }

        screen
    }

    /// Deterministic constructor for snapshot & `no_alloc` tests (no background threads/DBus).
    #[allow(dead_code)]
    pub fn for_test(tracks: Vec<Track>, mini: bool, design: &Design) -> Self {
        let config = CassetteConfig::default();
        let (engine, analyzer) = AudioEngine::spawn(80);
        let (_tx, mpris_rx) = mpsc::channel();
        let mpris = MprisHandle::spawn(mpsc::channel().0);
        let mut queue = Queue::new();
        if !tracks.is_empty() {
            queue.set_and_play(tracks.clone(), 0);
        }
        let mut screen = Self {
            config,
            tab: Tab::Deck,
            mini,
            focused: true,
            unfocused_since: None,
            phase: CassettePhase::Normal,
            engine,
            analyzer,
            queue,
            library: tracks,
            playlists: Vec::new(),
            lyrics: None,
            art: AlbumArt::off(),
            mpris,
            mpris_rx,
            scan_rx: None,
            #[cfg(feature = "spotify")]
            spotify_token: None,
            #[cfg(feature = "spotify")]
            spotify_devices: Vec::new(),
            #[cfg(feature = "spotify")]
            spotify_search: Vec::new(),
            #[cfg(feature = "spotify")]
            spotify_remote_playing: false,
            #[cfg(feature = "spotify")]
            spotify_phase: 0.0,
            lib_list: ListState::new(),
            queue_list: ListState::new(),
            search_input: Input::new(),
            search_list: ListState::new(),
            search_results: Vec::new(),
            playlist_list: ListState::new(),
            spotify_list: ListState::new(),
            settings_list: ListState::new(),
            rows_height: Cell::new(14),
            scrubber_rect: Cell::new(Rect::default()),
            last_finished_gen: 0,
            last_pos_sec: 134,
            last_dur_sec: 245,
            time_buf: "02:14 / 04:05".to_string(),
            codec_buf: "FLAC · 48kHz · 24-bit".to_string(),
            status_line: "Ready".to_string(),
            header: Header::new("Hayato").subtitle("Cassette"),
            keybar: default_keybar(),
            design_override: None,
            border_ramp: design.ramp(ColorRole::Border, ColorRole::Accent),
            spectrum_ramp: design.ramp(ColorRole::AccentAlt, ColorRole::Accent),
            fx: None,
            last_fx_tick: Instant::now(),
            headless_test: true,
        };
        screen.rebuild_library_rows();
        screen.rebuild_queue_rows();
        screen.rebuild_settings_rows();
        screen.sync_chrome();
        screen
    }

    fn current_play_state(&self) -> PlayState {
        #[cfg(feature = "spotify")]
        if self.queue.current_track().is_some_and(Track::is_spotify) {
            return if self.spotify_remote_playing {
                PlayState::Playing
            } else {
                PlayState::Paused
            };
        }
        self.engine.telemetry.play_state()
    }

    fn play_tracks(&mut self, tracks: Vec<Track>, start_idx: usize) {
        if let Some(track) = self.queue.set_and_play(tracks, start_idx).cloned() {
            self.start_track(&track);
            self.rebuild_queue_rows();
            self.tab = Tab::Deck;
            self.sync_chrome();
        }
    }

    fn start_track(&mut self, track: &Track) {
        self.art.set_path(track.art_path.as_deref());
        self.lyrics = match &track.source {
            SourceKind::Local { path, .. } => Lyrics::load_sidecar(path),
            SourceKind::Spotify { .. } => None,
        };
        self.last_pos_sec = 0;
        self.last_dur_sec = track.duration_secs;
        self.time_buf = format!("00:00 / {}", format_time(track.duration_secs));

        match &track.source {
            SourceKind::Local { path, codec } => {
                self.codec_buf = codec.short_label();
                #[cfg(feature = "spotify")]
                {
                    self.spotify_remote_playing = false;
                }
                self.engine.play_file(path.clone(), track.duration_secs);
            }
            SourceKind::Spotify { uri, url } => {
                self.engine.stop();
                self.codec_buf = "SPOTIFY · CONNECT".to_string();
                #[cfg(feature = "spotify")]
                {
                    if let Some(tok) = self.spotify_token.as_mut() {
                        if tok.is_premium {
                            let active_dev = self
                                .spotify_devices
                                .iter()
                                .find(|d| d.is_active)
                                .or_else(|| self.spotify_devices.first())
                                .map(|d| d.id.clone());
                            match spotify::play_uri(tok, uri, active_dev.as_deref()) {
                                Ok(()) => {
                                    self.spotify_remote_playing = true;
                                    self.status_line =
                                        format!("Playing on Spotify Connect: {}", track.title);
                                }
                                Err(e) => {
                                    self.spotify_remote_playing = false;
                                    self.status_line = format!("Spotify Connect error: {e}");
                                }
                            }
                        } else {
                            self.status_line =
                                "Spotify Free account: opening track in Firefox".to_string();
                            let _ = Command::new("xdg-open")
                                .arg(if url.is_empty() { uri } else { url })
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                        }
                    } else {
                        self.status_line = "Sign in to Spotify (tab 6) to play".to_string();
                    }
                }
                #[cfg(not(feature = "spotify"))]
                {
                    let _ = (uri, url);
                }
            }
        }

        // Trigger bounded track-change dissolve effect
        self.fx = Some(fx::coalesce(EffectTimer::from_ms(
            240,
            Interpolation::CubicOut,
        )));
        self.last_fx_tick = Instant::now();

        if !self.focused && self.config.notify_when_unfocused && !self.headless_test {
            notify_track_change(track);
        }
        self.publish_external_state();
    }

    fn toggle_play_pause(&mut self) {
        if self.queue.current_track().is_none() {
            if !self.library.is_empty() {
                let all = self.library.clone();
                self.play_tracks(all, 0);
            }
            return;
        }
        #[cfg(feature = "spotify")]
        if self.queue.current_track().is_some_and(Track::is_spotify) {
            if let Some(tok) = self.spotify_token.as_mut() {
                let res = if self.spotify_remote_playing {
                    spotify::pause_remote(tok)
                } else {
                    spotify::resume_remote(tok)
                };
                if res.is_ok() {
                    self.spotify_remote_playing = !self.spotify_remote_playing;
                }
            }
            self.publish_external_state();
            return;
        }
        match self.engine.telemetry.play_state() {
            PlayState::Playing => self.engine.pause(),
            PlayState::Paused => self.engine.resume(),
            PlayState::Stopped => {
                if let Some(t) = self.queue.current_track().cloned() {
                    self.start_track(&t);
                }
            }
        }
        self.publish_external_state();
    }

    fn next_track(&mut self, auto_end: bool) {
        if let Some(t) = self.queue.advance_next(auto_end).cloned() {
            self.start_track(&t);
            self.rebuild_queue_rows();
        } else {
            self.engine.stop();
            self.publish_external_state();
        }
        self.sync_chrome();
    }

    fn prev_track(&mut self) {
        if self.last_pos_sec > 3 {
            self.engine.seek_secs(0);
            self.last_pos_sec = 0;
            self.time_buf = format!("00:00 / {}", format_time(self.last_dur_sec));
            return;
        }
        if let Some(t) = self.queue.advance_prev().cloned() {
            self.start_track(&t);
            self.rebuild_queue_rows();
        }
        self.sync_chrome();
    }

    fn seek_delta(&mut self, delta: i64) {
        let target = (self.last_pos_sec as i64 + delta).clamp(0, self.last_dur_sec as i64) as u64;
        self.engine.seek_secs(target);
        self.last_pos_sec = target;
        self.time_buf = format!(
            "{} / {}",
            format_time(self.last_pos_sec),
            format_time(self.last_dur_sec)
        );
    }

    fn adjust_volume(&mut self, delta: i16) {
        let next = (i16::from(self.config.volume_pct) + delta).clamp(0, 100) as u8;
        self.config.volume_pct = next;
        self.engine.set_volume(next);
        let _ = save_config(&self.config);
        self.status_line = format!("Volume {next}%");
        self.publish_external_state();
    }

    fn share_current(&mut self) {
        let Some(track) = self.queue.current_track() else {
            self.status_line = "Nothing playing to share".to_string();
            return;
        };
        let (is_file, target) = track.share_target();
        let mut cmd = Command::new("hayato");
        cmd.arg("share");
        if is_file {
            cmd.arg(&target);
        } else {
            cmd.args(["--text", &target]);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = cmd.spawn();
        self.status_line = format!("Opened Hayato Share for {}", track.title);
    }

    fn publish_external_state(&self) {
        if self.headless_test {
            return;
        }
        let state = self.current_play_state();
        let track = self.queue.current_track();
        self.mpris.update(state, track, self.last_pos_sec);

        let now_file = NowPlayingFile {
            pid: std::process::id(),
            state,
            title: track.map(|t| t.title.clone()).unwrap_or_default(),
            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
            album: track.map(|t| t.album.clone()).unwrap_or_default(),
            source_label: track
                .map(|t| t.source_badge().to_string())
                .unwrap_or_default(),
            codec_label: self.codec_buf.clone(),
            position_secs: self.last_pos_sec,
            duration_secs: self.last_dur_sec,
            volume_pct: self.config.volume_pct,
        };
        now_file.write_atomic();
        let _ = Command::new("pkill")
            .args(["-RTMIN+8", "-x", "waybar"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    fn rebuild_library_rows(&mut self) {
        let rows: Vec<Row> = self
            .library
            .iter()
            .map(|t| {
                Row::new(&t.title)
                    .detail(format!("{} · {}", t.artist, t.album))
                    .value(format_time(t.duration_secs))
                    .glyph(Glyph::Music)
            })
            .collect();
        self.lib_list.update_rows(rows);
    }

    fn rebuild_queue_rows(&mut self) {
        let cur = self.queue.current_index();
        let rows: Vec<Row> = self
            .queue
            .items()
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let prefix = if cur == Some(i) { "▶ " } else { "  " };
                Row::new(format!("{prefix}{}. {}", i + 1, t.title))
                    .detail(format!("{} · {}", t.artist, t.source_badge()))
                    .value(format_time(t.duration_secs))
                    .glyph(Glyph::Music)
            })
            .collect();
        self.queue_list.update_rows(rows);
    }

    fn rebuild_playlist_rows(&mut self) {
        let rows: Vec<Row> = self
            .playlists
            .iter()
            .map(|p| {
                let kind = if p.is_spotify { "Spotify" } else { "Local .m3u8" };
                Row::new(&p.name)
                    .detail(kind)
                    .value(format!("{} tracks", p.tracks.len()))
                    .glyph(Glyph::Folder)
            })
            .collect();
        self.playlist_list.update_rows(rows);
    }

    fn rebuild_spotify_rows(&mut self) {
        #[cfg(feature = "spotify")]
        {
            let mut rows = Vec::new();
            if let Some(tok) = &self.spotify_token {
                let tier = if tok.is_premium { "PREMIUM" } else { "FREE" };
                rows.push(
                    Row::new(format!("Account: {} [{tier}]", tok.display_name))
                        .detail("Press r to refresh devices · x to sign out")
                        .glyph(Glyph::Wifi),
                );
                if self.spotify_devices.is_empty() {
                    rows.push(
                        Row::new("No active Spotify Connect devices found")
                            .detail("Open Spotify on your phone or start librespot, then press r")
                            .glyph(Glyph::Volume),
                    );
                } else {
                    for d in &self.spotify_devices {
                        let dot = if d.is_active { "● " } else { "○ " };
                        rows.push(
                            Row::new(format!("{dot}{}", d.name))
                                .detail(format!("{} · Vol {}%", d.device_type, d.volume_percent))
                                .value(if d.is_active { "active" } else { "transfer" })
                                .glyph(Glyph::Volume),
                        );
                    }
                }
            } else {
                rows.push(
                    Row::new("Sign in with Spotify (Browser OAuth PKCE)")
                        .detail(format!("Redirect URI: {}", spotify::REDIRECT_URI))
                        .value("Enter")
                        .glyph(Glyph::Key),
                );
                rows.push(
                    Row::new("Set Spotify Developer Client ID")
                        .detail(if self.config.spotify_client_id.is_empty() {
                            "Required for Spotify 2026 Web API (press c to paste)"
                        } else {
                            "Client ID configured (press c to edit)"
                        })
                        .value("c")
                        .glyph(Glyph::Gear),
                );
            }
            self.spotify_list.update_rows(rows);
        }
    }

    fn rebuild_settings_rows(&mut self) {
        let dirs_str: String = self
            .config
            .music_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let rows = vec![
            Row::new("Visualizer Mode")
                .detail("Cycle Spectrum, Oscilloscope, or Off")
                .value(self.config.visualizer.label())
                .glyph(Glyph::Music),
            Row::new("Synced Lyrics Drawer")
                .detail("Show sidecar .lrc lyrics on Now Playing deck")
                .value(if self.config.show_lyrics { "On" } else { "Off" })
                .glyph(Glyph::File),
            Row::new("Unfocused Track Notifications")
                .detail("Send hayato-notify card on track change while hidden")
                .value(if self.config.notify_when_unfocused {
                    "On"
                } else {
                    "Off"
                })
                .glyph(Glyph::Bell),
            Row::new("Rescan Music Folders")
                .detail(dirs_str)
                .value("Enter")
                .glyph(Glyph::Folder),
            Row::new("Spotify Developer Client ID")
                .detail("For PKCE OAuth & Web API search/playlists")
                .value(if self.config.spotify_client_id.is_empty() {
                    "unset"
                } else {
                    "configured"
                })
                .glyph(Glyph::Key),
        ];
        self.settings_list.update_rows(rows);
    }

    fn run_unified_search(&mut self) {
        let q = self.search_input.text().trim().to_lowercase();
        self.search_results.clear();
        if q.is_empty() {
            self.search_list.update_rows(Vec::new());
            return;
        }
        let mut rows = Vec::new();
        let local_matches: Vec<Track> = self
            .library
            .iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&q)
                    || t.artist.to_lowercase().contains(&q)
                    || t.album.to_lowercase().contains(&q)
            })
            .take(40)
            .cloned()
            .collect();
        for t in local_matches {
            rows.push(
                Row::new(&t.title)
                    .detail(format!("{} · {} [LOCAL]", t.artist, t.album))
                    .value(format_time(t.duration_secs))
                    .glyph(Glyph::Music),
            );
            self.search_results.push(t);
        }

        #[cfg(feature = "spotify")]
        if let Some(tok) = self.spotify_token.as_mut()
            && q.len() >= 2
            && let Ok(sp_tracks) = spotify::search_tracks(tok, &q, 12)
        {
            self.spotify_search = sp_tracks.clone();
            for t in sp_tracks {
                rows.push(
                    Row::new(&t.title)
                        .detail(format!("{} · {} [SPOTIFY]", t.artist, t.album))
                        .value(format_time(t.duration_secs))
                        .glyph(Glyph::Wifi),
                );
                self.search_results.push(t);
            }
        }
        self.search_list.update_rows(rows);
    }

    fn open_palette(&mut self) {
        let mut list = ListState::new().ranked();
        list.set_rows(vec![
            Row::new("Play / Pause").value("Space").glyph(Glyph::Play),
            Row::new("Next Track").value("n").glyph(Glyph::Music),
            Row::new("Previous Track").value("p").glyph(Glyph::Music),
            Row::new("Toggle Mini HUD").value("m").glyph(Glyph::Window),
            Row::new("Cycle Visualizer").value("v").glyph(Glyph::Music),
            Row::new("Toggle Lyrics").value("L").glyph(Glyph::File),
            Row::new("Share Current Track (Hayato Share)")
                .value("Ctrl+S")
                .glyph(Glyph::Qr),
            Row::new("Rescan Local Music Library")
                .value("R")
                .glyph(Glyph::Folder),
            Row::new("Save Queue as .m3u8 Playlist")
                .value("s")
                .glyph(Glyph::Save),
            Row::new("Hide to Scratchpad").value("Esc").glyph(Glyph::Window),
            Row::new("Quit Hayato Cassette").value("Q").glyph(Glyph::Power),
        ]);
        self.phase = CassettePhase::Palette(list);
    }

    fn sync_chrome(&mut self) {
        let sub = if self.mini {
            "Cassette [MINI]"
        } else {
            "Cassette"
        };
        let state_dot = match self.current_play_state() {
            PlayState::Playing => "● PLAYING",
            PlayState::Paused => "⏸ PAUSED",
            PlayState::Stopped => "○ IDLE",
        };
        self.header = Header::new("Hayato")
            .subtitle(sub)
            .status(format!("{state_dot} · {} tracks", self.library.len()));
    }

    fn draw_deck_full(&mut self, ctx: &mut BodyCtx) {
        let area = ctx.area;
        let left_w = (area.width / 3).clamp(28, 38);
        let right_w = area.width.saturating_sub(left_w + 1);
        let left_rect = Rect::new(area.x, area.y, left_w, area.height);
        let right_rect = Rect::new(area.x + left_w + 1, area.y, right_w, area.height);

        // Left pane: Album Art + Up Next
        let left_inner = draw_card(ctx.buf, left_rect, ctx.design, Some("ALBUM ART"), false);
        if left_inner.height >= 8 && left_inner.width >= 12 {
            let art_h = (left_inner.height / 2).clamp(6, 14);
            let art_rect = Rect::new(left_inner.x + 1, left_inner.y, left_inner.width.saturating_sub(2), art_h);
            if self.art.has_image() {
                self.art.render(art_rect, ctx.buf);
            } else {
                draw_cassette_ascii(ctx.buf, art_rect, ctx.design);
            }

            let info_y = art_rect.bottom() + 1;
            if info_y + 2 < left_inner.bottom() {
                let state_str = match self.current_play_state() {
                    PlayState::Playing => "● PLAYING",
                    PlayState::Paused => "⏸ PAUSED",
                    PlayState::Stopped => "○ STOPPED",
                };
                let role = match self.current_play_state() {
                    PlayState::Playing => ColorRole::Ok,
                    PlayState::Paused => ColorRole::Warn,
                    PlayState::Stopped => ColorRole::Muted,
                };
                let bx = draw_badge(ctx.buf, left_inner.x, info_y, ctx.design, state_str, role);
                let _ = put_trunc(
                    ctx.buf,
                    bx + 1,
                    info_y,
                    &self.time_buf,
                    left_inner.right().saturating_sub(bx + 1),
                    ctx.design.style(TextStyle::Dim),
                );
                let _ = put_trunc(
                    ctx.buf,
                    left_inner.x,
                    info_y + 1,
                    &self.codec_buf,
                    left_inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
            }

            let up_y = info_y + 3;
            if up_y + 2 < left_inner.bottom() {
                let _ = put_trunc(
                    ctx.buf,
                    left_inner.x,
                    up_y,
                    "UP NEXT",
                    left_inner.width,
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Accent))
                        .add_modifier(Modifier::BOLD),
                );
                let cur = self.queue.current_index().unwrap_or(0);
                for (offset, t) in self.queue.items().iter().skip(cur + 1).take(4).enumerate() {
                    let y = up_y + 1 + offset as u16;
                    if y >= left_inner.bottom() {
                        break;
                    }
                    let line = format!("{}. {}", offset + 1, t.title);
                    let _ = put_trunc(
                        ctx.buf,
                        left_inner.x,
                        y,
                        &line,
                        left_inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                }
            }
        }

        // Right pane: Beat-reactive border + BigText title + Spectrum/Lyrics + Scrubber
        let beat_idx = ((self.analyzer.beat_intensity * 7.0).round() as usize).min(7);
        let beat_color = if ctx.design.reduced_motion() {
            ctx.design.color(ColorRole::Border)
        } else {
            self.border_ramp[beat_idx]
        };
        let right_inner = draw_beat_card(
            ctx.buf,
            right_rect,
            ctx.design,
            "NOW PLAYING // CASSETTE DECK",
            beat_color,
        );
        if right_inner.height < 8 || right_inner.width < 20 {
            return;
        }

        let track = self.queue.current_track();
        let title_str = track.map(|t| t.title.as_str()).unwrap_or("HAYATO CASSETTE");
        let subtitle_str = track
            .map(|t| format!("{}  ·  {}", t.artist, t.album))
            .unwrap_or_else(|| "Press 2 for Library, / to Search, or 6 for Spotify".to_string());

        // BigText title (4 rows) when tall enough, else bold single-line
        let meta_y = if right_inner.height >= 16 {
            let bt_rect = Rect::new(right_inner.x, right_inner.y, right_inner.width, 4);
            let big = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(
                    Style::default()
                        .fg(ctx.design.color(ColorRole::Text))
                        .add_modifier(Modifier::BOLD),
                )
                .lines(vec![title_str.into()])
                .build();
            big.render(bt_rect, ctx.buf);
            right_inner.y + 4
        } else {
            let _ = put_trunc(
                ctx.buf,
                right_inner.x,
                right_inner.y,
                title_str,
                right_inner.width,
                Style::default()
                    .fg(ctx.design.color(ColorRole::Text))
                    .add_modifier(Modifier::BOLD),
            );
            right_inner.y + 1
        };

        let _ = put_trunc(
            ctx.buf,
            right_inner.x,
            meta_y,
            &subtitle_str,
            right_inner.width,
            ctx.design.style(TextStyle::Dim),
        );

        // Badges row: source + volume + shuffle + repeat
        let badge_y = meta_y + 1;
        let src_badge = track.map(Track::source_badge).unwrap_or("LOCAL");
        let src_role = if track.is_some_and(Track::is_spotify) {
            ColorRole::Accent
        } else {
            ColorRole::Ok
        };
        let mut bx = draw_badge(ctx.buf, right_inner.x, badge_y, ctx.design, src_badge, src_role);
        let vol_lbl = if self.engine.telemetry.muted() {
            "MUTED".to_string()
        } else {
            format!("VOL {}%", self.config.volume_pct)
        };
        bx = draw_badge(ctx.buf, bx + 1, badge_y, ctx.design, &vol_lbl, ColorRole::AccentAlt);
        if self.queue.shuffle {
            bx = draw_badge(ctx.buf, bx + 1, badge_y, ctx.design, "SHUF", ColorRole::Accent);
        }
        let _ = draw_badge(
            ctx.buf,
            bx + 1,
            badge_y,
            ctx.design,
            self.queue.repeat.short_label(),
            ColorRole::Muted,
        );

        // Scrubber + transport at bottom of right_inner
        let transport_y = right_inner.bottom().saturating_sub(1);
        let scrub_y = transport_y.saturating_sub(2);
        let viz_y = badge_y + 2;
        let viz_h = scrub_y.saturating_sub(viz_y + 1);

        if viz_h >= 3 {
            let viz_rect = Rect::new(right_inner.x, viz_y, right_inner.width, viz_h);
            if self.config.show_lyrics {
                self.draw_lyrics(ctx.buf, viz_rect, ctx.design);
            } else {
                match self.config.visualizer {
                    VisualizerMode::Spectrum => self.draw_spectrum(ctx.buf, viz_rect, ctx.design),
                    VisualizerMode::Oscilloscope => {
                        self.draw_oscilloscope(ctx.buf, viz_rect, ctx.design)
                    }
                    VisualizerMode::Off => {
                        let _ = put_trunc(
                            ctx.buf,
                            viz_rect.x,
                            viz_rect.y + viz_rect.height / 2,
                            "Visualizer off · press v to enable",
                            viz_rect.width,
                            ctx.design.style(TextStyle::Dim),
                        );
                    }
                }
            }
        }

        // Scrubber bar
        let scrub_rect = Rect::new(right_inner.x, scrub_y, right_inner.width, 1);
        self.scrubber_rect.set(scrub_rect);
        draw_scrubber(
            ctx.buf,
            scrub_rect,
            ctx.design,
            self.last_pos_sec,
            self.last_dur_sec,
            &self.time_buf,
        );

        // Transport hints
        let _ = put_trunc(
            ctx.buf,
            right_inner.x,
            transport_y,
            "[p] Prev   [Space] Play/Pause   [n] Next   [v] Viz   [L] Lyrics   [^S] Share   [m] Mini",
            right_inner.width,
            ctx.design.style(TextStyle::Dim),
        );

        // Apply bounded tachyonfx strictly to right_inner (never touching Sixel art rect!)
        if let Some(effect) = self.fx.as_mut() {
            if ctx.design.reduced_motion() {
                self.fx = None;
            } else {
                let dt = self.last_fx_tick.elapsed();
                self.last_fx_tick = Instant::now();
                effect.process(dt.into(), ctx.buf, right_inner);
                if effect.done() {
                    self.fx = None;
                }
            }
        }
    }

    fn draw_deck_mini(&mut self, ctx: &mut BodyCtx) {
        let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("CASSETTE // MINI"), true);
        if inner.height < 4 || inner.width < 20 {
            return;
        }
        let track = self.queue.current_track();
        let title = track.map(|t| t.title.as_str()).unwrap_or("Hayato Cassette");
        let sub = track
            .map(|t| format!("{} · {}", t.artist, t.album))
            .unwrap_or_else(|| "Idle — press m for Full Deck".to_string());

        let art_w = if self.art.has_image() && inner.width >= 36 {
            10
        } else {
            0
        };
        if art_w > 0 {
            let art_rect = Rect::new(inner.x, inner.y, art_w, inner.height.min(5));
            self.art.render(art_rect, ctx.buf);
        }
        let tx = inner.x + if art_w > 0 { art_w + 1 } else { 0 };
        let tw = inner.right().saturating_sub(tx);

        let _ = put_trunc(
            ctx.buf,
            tx,
            inner.y,
            title,
            tw,
            Style::default()
                .fg(ctx.design.color(ColorRole::Text))
                .add_modifier(Modifier::BOLD),
        );
        let _ = put_trunc(ctx.buf, tx, inner.y + 1, &sub, tw, ctx.design.style(TextStyle::Dim));

        // 1-row mini spectrum
        if inner.height >= 6 {
            let mini_viz = Rect::new(tx, inner.y + 2, tw.min(BANDS as u16), 1);
            for i in 0..mini_viz.width as usize {
                let b = (self.analyzer.bands[i % BANDS] * 8.0).round() as usize;
                let ch = BLOCKS[b.min(8)];
                let col = self.spectrum_ramp[(b * 5 / 8).min(5)];
                let _ = put(
                    ctx.buf,
                    mini_viz.x + i as u16,
                    mini_viz.y,
                    ch,
                    1,
                    Style::default().fg(col),
                );
            }
        }

        let scrub_y = inner.bottom().saturating_sub(2);
        let scrub_rect = Rect::new(inner.x, scrub_y, inner.width, 1);
        self.scrubber_rect.set(scrub_rect);
        draw_scrubber(
            ctx.buf,
            scrub_rect,
            ctx.design,
            self.last_pos_sec,
            self.last_dur_sec,
            &self.time_buf,
        );
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "[p] Prev  [Space] Play  [n] Next  [m] Full  [^S] Share  [Esc] Hide",
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
    }

    fn draw_spectrum(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let inner = draw_card(buf, area, design, Some("SPECTRUM // 32-BAND FFT"), false);
        if inner.width < 8 || inner.height < 2 {
            return;
        }
        let h = inner.height as usize;
        let cols = (inner.width as usize).min(BANDS * 2);
        let step = if cols >= BANDS * 2 { 2 } else { 1 };
        let count = (cols / step).min(BANDS);

        for b in 0..count {
            let x = inner.x + (b * step) as u16;
            let val = self.analyzer.bands[b];
            let peak = self.analyzer.peaks[b];
            let total_eighths = (val * (h * 8) as f32).round() as usize;
            let peak_row = ((1.0 - peak) * h.saturating_sub(1) as f32).round() as usize;

            for row in 0..h {
                let y = inner.y + row as u16;
                let from_bottom = h - 1 - row;
                let cell_eighths = total_eighths.saturating_sub(from_bottom * 8).min(8);
                if cell_eighths > 0 {
                    let ramp_idx = (from_bottom * 5 / h.max(1)).min(5);
                    let st = Style::default().fg(self.spectrum_ramp[ramp_idx]);
                    let _ = put(buf, x, y, BLOCKS[cell_eighths], 1, st);
                } else if row == peak_row && peak > 0.04 {
                    let _ = put(
                        buf,
                        x,
                        y,
                        "▀",
                        1,
                        Style::default().fg(design.color(ColorRole::Warn)),
                    );
                }
            }
        }
    }

    fn draw_oscilloscope(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let inner = draw_card(buf, area, design, Some("OSCILLOSCOPE // LIVE PCM"), false);
        if inner.width < 8 || inner.height < 2 {
            return;
        }
        let mid = inner.y + inner.height / 2;
        let half_h = (inner.height / 2).max(1) as f32;
        let st = Style::default().fg(design.color(ColorRole::Accent));
        for col in 0..inner.width {
            let idx = (col as usize * WAVE_POINTS) / (inner.width as usize).max(1);
            let sample = self.analyzer.wave[idx.min(WAVE_POINTS - 1)];
            let dy = (sample * half_h).round() as i32;
            let y = (mid as i32 - dy).clamp(inner.y as i32, inner.bottom().saturating_sub(1) as i32)
                as u16;
            let _ = put(buf, inner.x + col, y, "•", 1, st);
        }
    }

    fn draw_lyrics(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let inner = draw_card(buf, area, design, Some("LYRICS // .LRC"), false);
        let Some(lyr) = &self.lyrics else {
            let _ = put_trunc(
                buf,
                inner.x,
                inner.y + inner.height / 2,
                "No sidecar .lrc lyrics file found for this track",
                inner.width,
                design.style(TextStyle::Dim),
            );
            return;
        };
        let pos_ms = self.engine.telemetry.position_ms();
        let active = lyr.active_index(pos_ms).unwrap_or(0);
        let half = (inner.height / 2) as usize;
        let start = active.saturating_sub(half);
        for (row, line) in lyr.lines.iter().skip(start).take(inner.height as usize).enumerate() {
            let idx = start + row;
            let st = if idx == active {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Dim)
            };
            let _ = put_trunc(buf, inner.x, inner.y + row as u16, &line.text, inner.width, st);
        }
    }

    fn draw_tab_bar(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let mut x = area.x;
        for &t in Tab::ALL {
            let lbl = t.label();
            let active = self.tab == t;
            let st = if active {
                design.style(TextStyle::Key)
            } else {
                design.style(TextStyle::Dim)
            };
            let pill = format!(" {lbl} ");
            let w = pill.len() as u16;
            if x + w > area.right() {
                break;
            }
            let _ = put(buf, x, area.y, &pill, w, st);
            x += w + 1;
        }
    }

    fn handle_mpris(&mut self) -> Outcome {
        while let Ok(cmd) = self.mpris_rx.try_recv() {
            match cmd {
                MprisCommand::Play => self.engine.resume(),
                MprisCommand::Pause => self.engine.pause(),
                MprisCommand::PlayPause => self.toggle_play_pause(),
                MprisCommand::Stop => self.engine.stop(),
                MprisCommand::Next => self.next_track(false),
                MprisCommand::Previous => self.prev_track(),
                MprisCommand::Raise => {
                    let _ = sway_ipc::show_from_scratchpad(self.mini);
                }
                MprisCommand::Quit => {
                    self.engine.stop();
                    NowPlayingFile::clear();
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
        }
        Outcome::Continue
    }
}

impl Screen for CassetteScreen {
    fn phase(&self) -> Option<&'static str> {
        Some(match self.phase {
            CassettePhase::Normal => match self.tab {
                Tab::Deck if self.mini => "cassette.mini",
                Tab::Deck => "cassette.deck",
                Tab::Library => "cassette.library",
                Tab::Queue => "cassette.queue",
                Tab::Search => "cassette.search",
                Tab::Playlists => "cassette.playlists",
                Tab::Spotify => "cassette.spotify",
                Tab::Settings => "cassette.settings",
            },
            CassettePhase::ConfirmClearQueue(_) => "cassette.confirm_clear",
            CassettePhase::ConfirmLogoutSpotify(_) => "cassette.confirm_logout",
            CassettePhase::Palette(_) => "cassette.palette",
            CassettePhase::EditClientId(_) => "cassette.edit_client_id",
        })
    }

    fn design(&self) -> Option<Design> {
        self.design_override.clone()
    }

    fn needs_tick(&self) -> Option<Duration> {
        let playing = self.current_play_state() == PlayState::Playing;
        if self.fx.is_some() {
            return Some(Duration::from_millis(33));
        }
        if playing {
            let idle_unfocused = !self.focused
                && self
                    .unfocused_since
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(10));
            if idle_unfocused {
                Some(Duration::from_millis(250))
            } else {
                Some(Duration::from_millis(33))
            }
        } else {
            Some(Duration::from_millis(300))
        }
    }

    fn tick(&mut self) -> Outcome {
        // 1. Live theme reload via SIGUSR1
        if THEME_RELOAD_FLAG.swap(false, Ordering::Relaxed) {
            let d = Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
            self.border_ramp = d.ramp(ColorRole::Border, ColorRole::Accent);
            self.spectrum_ramp = d.ramp(ColorRole::AccentAlt, ColorRole::Accent);
            self.design_override = Some(d);
        }

        // 2. Background library scan completion
        if let Some(rx) = &self.scan_rx
            && let Ok(tracks) = rx.try_recv()
        {
            self.library = tracks;
            self.playlists = load_local_playlists(&self.config.music_dirs, &self.library);
            self.rebuild_library_rows();
            self.rebuild_playlist_rows();
            self.sync_chrome();
            self.scan_rx = None;
        }

        // 3. MPRIS commands
        let mpris_out = self.handle_mpris();
        if mpris_out != Outcome::Continue {
            return mpris_out;
        }

        // 4. Audio engine errors & track completion
        if let Some(err) = self.engine.telemetry.take_error() {
            self.status_line = err;
        }
        let fin = self.engine.telemetry.finished_gen();
        if fin != self.last_finished_gen {
            self.last_finished_gen = fin;
            self.next_track(true);
        }

        // 5. Update pre-formatted time buffer only when the second changes
        let pos_s = self.engine.telemetry.position_ms() / 1000;
        let dur_s = self.engine.telemetry.duration_ms() / 1000;
        if pos_s != self.last_pos_sec || (dur_s > 0 && dur_s != self.last_dur_sec) {
            self.last_pos_sec = pos_s;
            if dur_s > 0 {
                self.last_dur_sec = dur_s;
            }
            self.time_buf = format!(
                "{} / {}",
                format_time(self.last_pos_sec),
                format_time(self.last_dur_sec)
            );
            self.publish_external_state();
        }

        // 6. DSP spectrum/beat update (focus-gated to save CPU when hidden!)
        let sleep_viz = !self.focused
            && self
                .unfocused_since
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(10));
        if !sleep_viz && self.config.visualizer != VisualizerMode::Off {
            let playing = self.current_play_state() == PlayState::Playing;
            #[cfg(feature = "spotify")]
            if self.queue.current_track().is_some_and(Track::is_spotify) {
                self.spotify_phase += 0.14;
                self.analyzer.update_synthetic(self.spotify_phase, playing);
                return Outcome::Continue;
            }
            self.analyzer.update(!playing);
        }

        Outcome::Continue
    }

    fn event(&mut self, event: &Event, _design: &Design) -> Outcome {
        match event {
            Event::FocusGained => {
                self.focused = true;
                self.unfocused_since = None;
                Outcome::Consumed
            }
            Event::FocusLost => {
                self.focused = false;
                self.unfocused_since = Some(Instant::now());
                Outcome::Consumed
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => {
                    self.adjust_volume(5);
                    Outcome::Consumed
                }
                MouseEventKind::ScrollDown => {
                    self.adjust_volume(-5);
                    Outcome::Consumed
                }
                _ => Outcome::Continue,
            },
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, _design: &Design) -> Option<usize> {
        let scrub = self.scrubber_rect.get();
        if y == scrub.y && x >= scrub.x && x < scrub.right() && scrub.width > 0 {
            let pct = ((x - scrub.x) as usize * 100) / scrub.width as usize;
            return Some(1000 + pct);
        }
        let _ = area;
        None
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(code) = row
            && code >= 1000
        {
            let pct = (code - 1000).min(100) as u64;
            let target = self.last_dur_sec * pct / 100;
            self.engine.seek_secs(target);
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let CassettePhase::ConfirmClearQueue(c) = &mut self.phase {
            c.render(ctx.area, ctx.buf, ctx.design);
            return;
        }
        if let CassettePhase::ConfirmLogoutSpotify(c) = &mut self.phase {
            c.render(ctx.area, ctx.buf, ctx.design);
            return;
        }
        if let CassettePhase::Palette(list) = &mut self.phase {
            let inner = draw_card(
                ctx.buf,
                ctx.area,
                ctx.design,
                Some("COMMAND PALETTE"),
                true,
            );
            self.rows_height.set(inner.height);
            list.render(inner, ctx.buf, ctx.design, true);
            return;
        }
        if let CassettePhase::EditClientId(input) = &mut self.phase {
            let inner = draw_card(
                ctx.buf,
                ctx.area,
                ctx.design,
                Some("SPOTIFY DEVELOPER CLIENT ID"),
                true,
            );
            if inner.height >= 4 {
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    inner.y,
                    "Paste your Spotify Developer Dashboard Client ID (Enter saves, Esc cancels):",
                    inner.width,
                    ctx.design.style(TextStyle::Dim),
                );
                let field = Rect::new(inner.x, inner.y + 2, inner.width, 1);
                input.render_field(
                    field,
                    ctx.buf,
                    ctx.design,
                    "e.g. 4f8b9c...",
                    true,
                    true,
                    ColorRole::Bg,
                    TextStyle::Body,
                );
            }
            return;
        }

        if self.mini || ctx.area.width < 74 || ctx.area.height < 14 {
            self.draw_deck_mini(ctx);
            return;
        }

        // Top tab strip (1 row) + main body
        let tab_rect = Rect::new(ctx.area.x, ctx.area.y, ctx.area.width, 1);
        self.draw_tab_bar(ctx.buf, tab_rect, ctx.design);
        let body_rect = Rect::new(
            ctx.area.x,
            ctx.area.y + 1,
            ctx.area.width,
            ctx.area.height.saturating_sub(1),
        );
        let mut sub_ctx = BodyCtx {
            area: body_rect,
            buf: ctx.buf,
            design: ctx.design,
        };

        match self.tab {
            Tab::Deck => self.draw_deck_full(&mut sub_ctx),
            Tab::Library => {
                let inner = draw_card(
                    sub_ctx.buf,
                    sub_ctx.area,
                    sub_ctx.design,
                    Some("LIBRARY // LOCAL AUDIO"),
                    true,
                );
                let list_h = inner.height.saturating_sub(1);
                self.rows_height.set(list_h);
                self.lib_list.render(
                    Rect::new(inner.x, inner.y, inner.width, list_h),
                    sub_ctx.buf,
                    sub_ctx.design,
                    true,
                );
                let _ = put_trunc(
                    sub_ctx.buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    "[Enter] play   [e] enqueue   [E] play next   [R] rescan ~/Music",
                    inner.width,
                    sub_ctx.design.style(TextStyle::Dim),
                );
            }
            Tab::Queue => {
                let title = format!(
                    "UP NEXT // {} TRACKS ({})",
                    self.queue.len(),
                    format_time(self.queue.remaining_duration_secs())
                );
                let inner = draw_card(sub_ctx.buf, sub_ctx.area, sub_ctx.design, Some(&title), true);
                let list_h = inner.height.saturating_sub(1);
                self.rows_height.set(list_h);
                self.queue_list.render(
                    Rect::new(inner.x, inner.y, inner.width, list_h),
                    sub_ctx.buf,
                    sub_ctx.design,
                    true,
                );
                let _ = put_trunc(
                    sub_ctx.buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    "[Enter] jump   [J/K] reorder   [d] remove   [c] clear   [s] save .m3u8",
                    inner.width,
                    sub_ctx.design.style(TextStyle::Dim),
                );
            }
            Tab::Search => {
                let inner = draw_card(
                    sub_ctx.buf,
                    sub_ctx.area,
                    sub_ctx.design,
                    Some("SEARCH // LOCAL + SPOTIFY"),
                    true,
                );
                if inner.height >= 3 {
                    let field = Rect::new(inner.x, inner.y, inner.width, 1);
                    self.search_input.render_field(
                        field,
                        sub_ctx.buf,
                        sub_ctx.design,
                        "Type artist, track, or album and press Enter…",
                        true,
                        true,
                        ColorRole::Bg,
                        TextStyle::Body,
                    );
                    let list_rect = Rect::new(
                        inner.x,
                        inner.y + 2,
                        inner.width,
                        inner.height.saturating_sub(2),
                    );
                    self.rows_height.set(list_rect.height);
                    self.search_list
                        .render(list_rect, sub_ctx.buf, sub_ctx.design, true);
                }
            }
            Tab::Playlists => {
                let inner = draw_card(
                    sub_ctx.buf,
                    sub_ctx.area,
                    sub_ctx.design,
                    Some("PLAYLISTS // .M3U8 & SPOTIFY"),
                    true,
                );
                self.rows_height.set(inner.height);
                self.playlist_list
                    .render(inner, sub_ctx.buf, sub_ctx.design, true);
            }
            Tab::Spotify => {
                let inner = draw_card(
                    sub_ctx.buf,
                    sub_ctx.area,
                    sub_ctx.design,
                    Some("SPOTIFY // CONNECT & ACCOUNT"),
                    true,
                );
                let list_h = inner.height.saturating_sub(1);
                self.rows_height.set(list_h);
                self.spotify_list.render(
                    Rect::new(inner.x, inner.y, inner.width, list_h),
                    sub_ctx.buf,
                    sub_ctx.design,
                    true,
                );
                let _ = put_trunc(
                    sub_ctx.buf,
                    inner.x,
                    inner.bottom().saturating_sub(1),
                    &self.status_line,
                    inner.width,
                    sub_ctx.design.style(TextStyle::Dim),
                );
            }
            Tab::Settings => {
                let inner = draw_card(
                    sub_ctx.buf,
                    sub_ctx.area,
                    sub_ctx.design,
                    Some("SETTINGS // CASSETTE"),
                    true,
                );
                self.rows_height.set(inner.height);
                self.settings_list
                    .render(inner, sub_ctx.buf, sub_ctx.design, true);
            }
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        // Handle active modal phases first
        if let CassettePhase::ConfirmClearQueue(c) = &mut self.phase {
            match c.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.queue.clear();
                    self.engine.stop();
                    self.rebuild_queue_rows();
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = CassettePhase::Normal;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::ConfirmLogoutSpotify(c) = &mut self.phase {
            match c.handle(key) {
                ConfirmAction::Chosen(1) => {
                    #[cfg(feature = "spotify")]
                    {
                        spotify::clear_token();
                        self.spotify_token = None;
                        self.spotify_devices.clear();
                        self.rebuild_spotify_rows();
                    }
                    self.phase = CassettePhase::Normal;
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = CassettePhase::Normal;
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::EditClientId(input) = &mut self.phase {
            match key.code {
                KeyCode::Esc => {
                    self.phase = CassettePhase::Normal;
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    self.config.spotify_client_id = input.text().trim().to_string();
                    let _ = save_config(&self.config);
                    self.phase = CassettePhase::Normal;
                    self.rebuild_spotify_rows();
                    self.rebuild_settings_rows();
                    self.status_line = "Saved Spotify Client ID".to_string();
                    return Outcome::Consumed;
                }
                _ => {
                    let _ = input.handle(key);
                    return Outcome::Consumed;
                }
            }
        }
        if let CassettePhase::Palette(list) = &mut self.phase {
            if key.code == KeyCode::Esc {
                self.phase = CassettePhase::Normal;
                return Outcome::Consumed;
            }
            if let ListAction::Activated(idx) = list.handle(key, self.rows_height.get(), design) {
                self.phase = CassettePhase::Normal;
                match idx {
                    0 => self.toggle_play_pause(),
                    1 => self.next_track(false),
                    2 => self.prev_track(),
                    3 => {
                        self.mini = !self.mini;
                        let _ = sway_ipc::resize_window(self.mini);
                        self.sync_chrome();
                    }
                    4 => {
                        self.config.visualizer = self.config.visualizer.cycle();
                        let _ = save_config(&self.config);
                    }
                    5 => {
                        self.config.show_lyrics = !self.config.show_lyrics;
                        let _ = save_config(&self.config);
                    }
                    6 => self.share_current(),
                    7 => {
                        self.library = scan_directories(&self.config.music_dirs);
                        self.rebuild_library_rows();
                        self.sync_chrome();
                    }
                    8 => {
                        let _ = save_m3u8_playlist(
                            &self.config.music_dirs,
                            "Cassette Mix",
                            self.queue.items(),
                        );
                        self.playlists = load_local_playlists(&self.config.music_dirs, &self.library);
                        self.rebuild_playlist_rows();
                    }
                    9 => {
                        let _ = sway_ipc::hide_to_scratchpad();
                    }
                    10 => {
                        self.engine.stop();
                        NowPlayingFile::clear();
                        return Outcome::Done(hayato_ui::app::exit::OK);
                    }
                    _ => {}
                }
            }
            return Outcome::Consumed;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('s') => {
                    self.share_current();
                    return Outcome::Consumed;
                }
                KeyCode::Char('p') => {
                    self.open_palette();
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        // Search tab text input handling
        if self.tab == Tab::Search {
            match key.code {
                KeyCode::Esc => {
                    self.tab = Tab::Deck;
                    return Outcome::Consumed;
                }
                KeyCode::Up | KeyCode::Down => {
                    let _ = self.search_list.handle(key, self.rows_height.get(), design);
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    if let Some(idx) = self.search_list.selected_index()
                        && let Some(t) = self.search_results.get(idx).cloned()
                    {
                        self.play_tracks(vec![t], 0);
                    } else {
                        self.run_unified_search();
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    if matches!(self.search_input.handle(key), InputAction::Changed) {
                        self.run_unified_search();
                        return Outcome::Consumed;
                    }
                }
            }
        }

        // Global shortcuts
        match key.code {
            KeyCode::Char('Q') => {
                self.engine.stop();
                NowPlayingFile::clear();
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
            KeyCode::Esc => {
                if self.tab != Tab::Deck {
                    self.tab = Tab::Deck;
                } else if !sway_ipc::hide_to_scratchpad() {
                    self.engine.stop();
                    NowPlayingFile::clear();
                    return Outcome::Done(hayato_ui::app::exit::ESCAPE);
                }
                return Outcome::Consumed;
            }
            KeyCode::Char(' ') => {
                self.toggle_play_pause();
                return Outcome::Consumed;
            }
            KeyCode::Char('n') | KeyCode::Char('>') => {
                self.next_track(false);
                return Outcome::Consumed;
            }
            KeyCode::Char('p') | KeyCode::Char('<') => {
                self.prev_track();
                return Outcome::Consumed;
            }
            KeyCode::Left => {
                self.seek_delta(if key.modifiers.contains(KeyModifiers::SHIFT) {
                    -30
                } else {
                    -5
                });
                return Outcome::Consumed;
            }
            KeyCode::Right => {
                self.seek_delta(if key.modifiers.contains(KeyModifiers::SHIFT) {
                    30
                } else {
                    5
                });
                return Outcome::Consumed;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_volume(5);
                return Outcome::Consumed;
            }
            KeyCode::Char('-') => {
                self.adjust_volume(-5);
                return Outcome::Consumed;
            }
            KeyCode::Char('M') => {
                let m = !self.engine.telemetry.muted();
                self.engine.set_muted(m);
                return Outcome::Consumed;
            }
            KeyCode::Char('z') => {
                self.queue.shuffle = !self.queue.shuffle;
                return Outcome::Consumed;
            }
            KeyCode::Char('r') if self.tab != Tab::Spotify => {
                self.queue.repeat = self.queue.repeat.cycle();
                return Outcome::Consumed;
            }
            KeyCode::Char('v') => {
                self.config.visualizer = self.config.visualizer.cycle();
                let _ = save_config(&self.config);
                return Outcome::Consumed;
            }
            KeyCode::Char('L') => {
                self.config.show_lyrics = !self.config.show_lyrics;
                let _ = save_config(&self.config);
                return Outcome::Consumed;
            }
            KeyCode::Char('m') => {
                self.mini = !self.mini;
                let _ = sway_ipc::resize_window(self.mini);
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char(':') => {
                self.open_palette();
                return Outcome::Consumed;
            }
            KeyCode::Char('/') => {
                self.tab = Tab::Search;
                return Outcome::Consumed;
            }
            KeyCode::Char('1') => {
                self.tab = Tab::Deck;
                self.fx = Some(fx::sweep_in(
                    Motion::LeftToRight,
                    6,
                    0,
                    design.color(ColorRole::Bg),
                    EffectTimer::from_ms(140, Interpolation::QuadOut),
                ));
                self.last_fx_tick = Instant::now();
                return Outcome::Consumed;
            }
            KeyCode::Char('2') => {
                self.tab = Tab::Library;
                return Outcome::Consumed;
            }
            KeyCode::Char('3') => {
                self.tab = Tab::Queue;
                return Outcome::Consumed;
            }
            KeyCode::Char('4') => {
                self.tab = Tab::Search;
                return Outcome::Consumed;
            }
            KeyCode::Char('5') => {
                self.tab = Tab::Playlists;
                return Outcome::Consumed;
            }
            KeyCode::Char('6') | KeyCode::Char('S') => {
                self.tab = Tab::Spotify;
                return Outcome::Consumed;
            }
            KeyCode::Char(',') => {
                self.tab = Tab::Settings;
                return Outcome::Consumed;
            }
            _ => {}
        }

        // Per-tab list interactions
        match self.tab {
            Tab::Library => match key.code {
                KeyCode::Char('R') => {
                    self.library = scan_directories(&self.config.music_dirs);
                    self.rebuild_library_rows();
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Char('e') => {
                    if let Some(i) = self.lib_list.selected_index()
                        && let Some(t) = self.library.get(i).cloned()
                    {
                        self.queue.push_back(t);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('E') => {
                    if let Some(i) = self.lib_list.selected_index()
                        && let Some(t) = self.library.get(i).cloned()
                    {
                        self.queue.push_next(t);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    if let ListAction::Activated(i) =
                        self.lib_list.handle(key, self.rows_height.get(), design)
                    {
                        let all = self.library.clone();
                        self.play_tracks(all, i);
                    }
                    return Outcome::Consumed;
                }
            },
            Tab::Queue => match key.code {
                KeyCode::Char('J') => {
                    if let Some(i) = self.queue_list.selected_index() {
                        let _ = self.queue.move_down(i);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('K') => {
                    if let Some(i) = self.queue_list.selected_index() {
                        let _ = self.queue.move_up(i);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(i) = self.queue_list.selected_index() {
                        self.queue.remove(i);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('c') if !self.queue.is_empty() => {
                    let confirm = Confirm::new(
                        "Clear Queue",
                        format!("Clear all {} queued tracks?", self.queue.len()),
                    )
                    .with_options(&["Cancel", "Clear"])
                    .destructive(true);
                    self.phase = CassettePhase::ConfirmClearQueue(confirm);
                    return Outcome::Consumed;
                }
                KeyCode::Char('s') if !self.queue.is_empty() => {
                    match save_m3u8_playlist(
                        &self.config.music_dirs,
                        "Cassette Mix",
                        self.queue.items(),
                    ) {
                        Ok(p) => {
                            self.status_line = format!("Saved playlist {}", p.display());
                            self.playlists =
                                load_local_playlists(&self.config.music_dirs, &self.library);
                            self.rebuild_playlist_rows();
                        }
                        Err(e) => self.status_line = e,
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    if let ListAction::Activated(i) =
                        self.queue_list.handle(key, self.rows_height.get(), design)
                        && let Some(t) = self.queue.jump(i).cloned()
                    {
                        self.start_track(&t);
                        self.rebuild_queue_rows();
                    }
                    return Outcome::Consumed;
                }
            },
            Tab::Playlists => {
                if let ListAction::Activated(i) =
                    self.playlist_list.handle(key, self.rows_height.get(), design)
                    && let Some(pl) = self.playlists.get(i).cloned()
                {
                    let tracks = if pl.is_spotify && pl.tracks.is_empty() {
                        #[cfg(feature = "spotify")]
                        {
                            self.spotify_token
                                .as_mut()
                                .and_then(|tok| spotify::playlist_tracks(tok, &pl.id).ok())
                                .unwrap_or_default()
                        }
                        #[cfg(not(feature = "spotify"))]
                        Vec::new()
                    } else {
                        pl.tracks
                    };
                    if !tracks.is_empty() {
                        self.play_tracks(tracks, 0);
                    }
                }
                return Outcome::Consumed;
            }
            Tab::Spotify => {
                #[cfg(feature = "spotify")]
                match key.code {
                    KeyCode::Char('c') => {
                        let mut inp = Input::new();
                        inp.set(&self.config.spotify_client_id);
                        self.phase = CassettePhase::EditClientId(inp);
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('r') => {
                        if let Some(tok) = self.spotify_token.as_mut() {
                            match spotify::list_devices(tok) {
                                Ok(devs) => {
                                    self.spotify_devices = devs;
                                    self.status_line = "Refreshed Spotify Connect devices".into();
                                }
                                Err(e) => self.status_line = e,
                            }
                            if let Ok(mut sp_pl) = spotify::list_playlists(tok) {
                                let mut local =
                                    load_local_playlists(&self.config.music_dirs, &self.library);
                                local.append(&mut sp_pl);
                                self.playlists = local;
                                self.rebuild_playlist_rows();
                            }
                            self.rebuild_spotify_rows();
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('x') if self.spotify_token.is_some() => {
                        let confirm = Confirm::new(
                            "Sign Out of Spotify",
                            "Remove saved Spotify OAuth token from this machine?",
                        )
                        .with_options(&["Cancel", "Sign out"])
                        .destructive(true);
                        self.phase = CassettePhase::ConfirmLogoutSpotify(confirm);
                        return Outcome::Consumed;
                    }
                    _ => {
                        if let ListAction::Activated(idx) =
                            self.spotify_list.handle(key, self.rows_height.get(), design)
                        {
                            if self.spotify_token.is_none() {
                                if idx == 0 {
                                    match spotify::login_pkce(&self.config.spotify_client_id) {
                                        Ok(tok) => {
                                            self.spotify_token = Some(tok);
                                            self.status_line = "Connected to Spotify!".into();
                                            self.rebuild_spotify_rows();
                                        }
                                        Err(e) => self.status_line = e,
                                    }
                                } else {
                                    let mut inp = Input::new();
                                    inp.set(&self.config.spotify_client_id);
                                    self.phase = CassettePhase::EditClientId(inp);
                                }
                            } else if idx > 0
                                && let Some(dev) = self.spotify_devices.get(idx - 1).cloned()
                                && let Some(tok) = self.spotify_token.as_mut()
                            {
                                match spotify::transfer_device(tok, &dev.id) {
                                    Ok(()) => {
                                        self.status_line =
                                            format!("Transferred playback to {}", dev.name);
                                    }
                                    Err(e) => self.status_line = e,
                                }
                            }
                        }
                        return Outcome::Consumed;
                    }
                }
                #[cfg(not(feature = "spotify"))]
                return Outcome::Consumed;
            }
            Tab::Settings => {
                if let ListAction::Activated(idx) =
                    self.settings_list.handle(key, self.rows_height.get(), design)
                {
                    match idx {
                        0 => self.config.visualizer = self.config.visualizer.cycle(),
                        1 => self.config.show_lyrics = !self.config.show_lyrics,
                        2 => self.config.notify_when_unfocused = !self.config.notify_when_unfocused,
                        3 => {
                            self.library = scan_directories(&self.config.music_dirs);
                            self.rebuild_library_rows();
                            self.sync_chrome();
                        }
                        4 => {
                            let mut inp = Input::new();
                            inp.set(&self.config.spotify_client_id);
                            self.phase = CassettePhase::EditClientId(inp);
                            return Outcome::Consumed;
                        }
                        _ => {}
                    }
                    let _ = save_config(&self.config);
                    self.rebuild_settings_rows();
                }
                return Outcome::Consumed;
            }
            Tab::Deck | Tab::Search => {}
        }

        Outcome::Continue
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }
}

fn default_keybar() -> Vec<KeyHint> {
    vec![
        KeyHint::new("Space", "play/pause"),
        KeyHint::new("n/p", "next/prev"),
        KeyHint::new("/", "search"),
        KeyHint::new(":", "palette"),
        KeyHint::new("m", "mini"),
        KeyHint::new("Esc", "hide"),
        KeyHint::new("Q", "quit"),
    ]
}

fn draw_beat_card(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    title: &str,
    border_color: Color,
) -> Rect {
    if area.width < 2 || area.height < 2 {
        return area;
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(format!(" {title} "))
        .title_style(
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(design.color(ColorRole::Surface)));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

fn draw_scrubber(
    buf: &mut Buffer,
    area: Rect,
    design: &Design,
    pos_secs: u64,
    dur_secs: u64,
    time_buf: &str,
) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    let label_w = (time_buf.len() as u16 + 2).min(area.width / 2);
    let bar_w = area.width.saturating_sub(label_w);
    let ratio = if dur_secs > 0 {
        (pos_secs as f32 / dur_secs as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (ratio * bar_w.saturating_sub(1) as f32).round() as u16;
    let accent = Style::default().fg(design.color(ColorRole::Accent));
    let dim = design.style(TextStyle::Dim);

    for i in 0..bar_w {
        let (ch, st) = if i < filled {
            ("━", accent)
        } else if i == filled {
            ("●", accent)
        } else {
            ("─", dim)
        };
        let _ = put(buf, area.x + i, area.y, ch, 1, st);
    }
    let _ = put_trunc(
        buf,
        area.x + bar_w + 1,
        area.y,
        time_buf,
        label_w,
        design.style(TextStyle::Dim),
    );
}

fn draw_cassette_ascii(buf: &mut Buffer, area: Rect, design: &Design) {
    const ART: &[&str] = &[
        "╭──────────────────────╮",
        "│  HAYATO // CASSETTE  │",
        "│   ╭───╮      ╭───╮   │",
        "│   │ ◉ │──────│ ◉ │   │",
        "│   ╰───╯      ╰───╯   │",
        "│  [A] HIGH BIAS 90m   │",
        "╰──────────────────────╯",
    ];
    let start_y = area.y + area.height.saturating_sub(ART.len() as u16) / 2;
    let st = Style::default().fg(design.color(ColorRole::AccentAlt));
    for (i, line) in ART.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.bottom() {
            break;
        }
        let _ = put_trunc(buf, area.x, y, line, area.width, st);
    }
}
