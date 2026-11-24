//! Hybrid **Spotify Web Player + Hayato Cyber-HUD** UI for **Hayato Cassette**.
//!
//! Layout grammar:
//! - **Top Chrome**: `[1 Home]` · always-visible `[/ Search Spotify & local…]` · `[● Device Pill]` · `[⚙]`
//! - **Left Sidebar ("Your Library")**: `[All | Spotify | Local]` · `♥ Liked Songs` · Playlists (real counts) · Local
//! - **Center Main Stage**: `Home` (Cyberdeck Hero + Quick Launch) · `Collection` (Playlist Drill-Down + 403 `context_uri` play) · `Search` (auto on type, debounced 250 ms) · `Visualizer` (`V`) · `Settings` (`4`/`,`)
//! - **Right Drawer** (wide): `NowPlaying` (`i`) · `Queue` (`q`/`3`) · `Lyrics` (`L`) · `Hidden`
//! - **Persistent Bottom Player Bar**: Always visible across every view (Track/Artist/Source · Transport + Live Scrubber · Device Pill · Volume · Mini `w`)
//!
//! Architectural invariants:
//! - Zero `Color::Rgb` (uses `Design::color` and `Design::ramp` LUTs).
//! - Renders strictly into `hayato_ui::BodyCtx`.
//! - Zero blocking network I/O on the UI thread (all provider I/O runs on `ProviderHub`).

use std::cell::Cell;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use hayato_music_core::dsp::{BANDS, SpectrumAnalyzer, WAVE_POINTS};
use hayato_music_core::engine::AudioEngine;
use hayato_music_core::library::{load_cached_tracks, read_track_metadata, save_m3u8_playlist};
use hayato_music_core::lyrics::Lyrics;
use hayato_music_core::mpris::{MprisCommand, MprisHandle};
use hayato_music_core::providers::{
    Collection, CollectionItems, CollectionKind, PlaybackDevice, ProviderCmd, ProviderEvent,
    ProviderHub, ProviderId, SearchResults,
};
use hayato_music_core::{
    CassetteConfig, NowPlayingFile, PlayState, Queue, RepeatMode, SourceKind, Track, TrackId,
    VisualizerMode, cache_dir, format_time, load_config, runtime_dir, save_config,
};
use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Confirm, ConfirmAction, Design, Glyph, Header, Input, InputAction, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_card,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
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

const HIT_FILTER_ALL: usize = 100;
const HIT_FILTER_SPOTIFY: usize = 101;
const HIT_FILTER_LOCAL: usize = 105;
const HIT_SEARCH_BOX: usize = 102;
const HIT_DEVICE_PILL: usize = 103;
const HIT_SETTINGS: usize = 104;
const HIT_BTN_PREV: usize = 110;
const HIT_BTN_PLAY: usize = 111;
const HIT_BTN_NEXT: usize = 112;
const HIT_BTN_VIZ: usize = 113;
const HIT_BTN_LYRICS: usize = 114;
const HIT_BTN_MINI: usize = 116;
const HIT_BTN_SHUFFLE: usize = 117;
const HIT_BTN_REPEAT: usize = 118;
const HIT_PLAY_ALL: usize = 120;
const HIT_BTN_VOL_DOWN: usize = 121;
const HIT_BTN_VOL_MUTE: usize = 122;
const HIT_BTN_VOL_UP: usize = 123;
const HIT_SCRUB_BASE: usize = 1000;
const HIT_SIDEBAR_BASE: usize = 2000;
const HIT_MAIN_BASE: usize = 4000;
const HIT_DRAWER_BASE: usize = 6000;
const HIT_MODAL_BASE: usize = 8000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Main,
    RightDrawer,
    SearchBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceFilter {
    All,
    Spotify,
    Local,
}

impl SourceFilter {
    fn cycle(self) -> Self {
        match self {
            Self::All => Self::Spotify,
            Self::Spotify => Self::Local,
            Self::Local => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Spotify => "Spotify",
            Self::Local => "Local",
        }
    }

    fn matches(self, provider: ProviderId) -> bool {
        match self {
            Self::All => true,
            Self::Spotify => provider == ProviderId::Spotify,
            Self::Local => provider == ProviderId::Local,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MainStage {
    Collection,
    Search,
    Visualizer,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RightDrawer {
    Queue,
    Lyrics,
    Hidden,
}

enum CassettePhase {
    Normal,
    DevicePicker(ListState),
    Help,
    Palette(ListState),
    ConfirmClearQueue(Confirm),
    ConfirmLogoutSpotify(Confirm),
    EditClientId(Input),
}

#[derive(Clone)]
enum SearchRowItem {
    Track(Track),
    Collection(Collection),
}

#[derive(Clone, Copy, Default)]
struct HitRects {
    filter_all: Rect,
    filter_spotify: Rect,
    filter_local: Rect,
    search: Rect,
    device: Rect,
    settings: Rect,
    sidebar_list: Rect,
    play_all: Rect,
    main_list: Rect,
    drawer_list: Rect,
    modal_list: Rect,
    deck_area: Rect,
    scrubber: Rect,
    btn_shuffle: Rect,
    btn_prev: Rect,
    btn_play: Rect,
    btn_next: Rect,
    btn_repeat: Rect,
    btn_viz: Rect,
    btn_lyrics: Rect,
    btn_mini: Rect,
    btn_vol_down: Rect,
    btn_vol_mute: Rect,
    btn_vol_up: Rect,
}

pub struct CassetteScreen {
    config: CassetteConfig,
    focus: Focus,
    stage: MainStage,
    prev_stage: MainStage,
    drawer: RightDrawer,
    sidebar_open: bool,
    source_filter: SourceFilter,
    mini: bool,
    focused: bool,
    unfocused_since: Option<Instant>,
    phase: CassettePhase,

    // Core audio & modular provider hub
    engine: AudioEngine,
    analyzer: SpectrumAnalyzer,
    hub: ProviderHub,
    queue: Queue,
    local_tracks: Vec<Track>,
    collections: Vec<Collection>,
    filtered_collections: Vec<Collection>,
    active_collection: Option<Collection>,
    playing_collection_id: Option<String>,
    active_collection_tracks: Vec<Track>,
    active_collection_restricted: Option<(String, String)>,
    loading_collection: bool,
    lyrics: Option<Lyrics>,
    lyrics_track_id: Option<TrackId>,
    lyrics_loading: bool,
    art: AlbumArt,
    mpris: MprisHandle,
    mpris_rx: Receiver<MprisCommand>,

    // Spotify / Remote Connect live state
    spotify_authed: bool,
    spotify_user: String,
    spotify_premium: bool,
    local_connect_ready: bool,
    local_device_id: Option<String>,
    preferred_device_id: Option<String>,
    local_spotify_pcm_active: Arc<AtomicBool>,
    devices: Vec<PlaybackDevice>,
    active_provider: ProviderId,
    remote_state: PlayState,
    remote_track: Option<Track>,
    remote_pos_ms: u64,
    remote_dur_ms: u64,
    remote_synced_at: Instant,
    optimistic_until: Option<Instant>,
    last_fx_track_id: Option<TrackId>,
    remote_phase: f32,

    // Search state (instant local + 250ms debounced Spotify)
    search_input: Input,
    search_list: ListState,
    search_items: Vec<SearchRowItem>,
    spotify_search_cache: SearchResults,
    search_gen: u64,
    pending_search_at: Option<Instant>,

    // Lists
    sidebar_list: ListState,
    collection_list: ListState,
    queue_list: ListState,
    settings_list: ListState,
    rows_height: Cell<u16>,
    hits: Cell<HitRects>,

    // Telemetry & pre-formatted strings (zero per-frame format!)
    last_finished_gen: u64,
    last_pos_sec: u64,
    last_dur_sec: u64,
    time_buf: String,
    codec_buf: String,
    device_pill_buf: String,
    vol_buf: String,
    queue_title_buf: String,
    collection_title_buf: String,
    search_title_buf: String,
    status_line: String,
    header: Header,
    keybar: Vec<KeyHint>,

    // Theme & motion
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
        let local_spotify_pcm_active = Arc::new(AtomicBool::new(false));
        let hub = ProviderHub::spawn(
            config.music_dirs.clone(),
            engine.shared_tap(),
            Arc::clone(&local_spotify_pcm_active),
        );
        let local_tracks = load_cached_tracks();

        let effective_mini = start_mini || config.mini_mode;
        let _ = std::fs::create_dir_all(runtime_dir());
        let _ = std::fs::write(
            runtime_dir().join("mode"),
            if effective_mini { "mini" } else { "full" },
        );

        let mut screen = Self {
            config,
            focus: Focus::Main,
            stage: MainStage::Collection,
            prev_stage: MainStage::Collection,
            drawer: RightDrawer::Hidden,
            sidebar_open: true,
            source_filter: SourceFilter::All,
            mini: effective_mini,
            focused: true,
            unfocused_since: None,
            phase: CassettePhase::Normal,
            engine,
            analyzer,
            hub,
            queue: Queue::new(),
            local_tracks,
            collections: Vec::new(),
            filtered_collections: Vec::new(),
            active_collection: None,
            playing_collection_id: None,
            active_collection_tracks: Vec::new(),
            active_collection_restricted: None,
            loading_collection: false,
            lyrics: None,
            lyrics_track_id: None,
            lyrics_loading: false,
            art: AlbumArt::from_env(),
            mpris,
            mpris_rx,
            spotify_authed: false,
            spotify_user: String::new(),
            spotify_premium: false,
            local_connect_ready: false,
            local_device_id: None,
            preferred_device_id: None,
            local_spotify_pcm_active,
            devices: Vec::new(),
            active_provider: ProviderId::Local,
            remote_state: PlayState::Stopped,
            remote_track: None,
            remote_pos_ms: 0,
            remote_dur_ms: 0,
            remote_synced_at: Instant::now(),
            optimistic_until: None,
            last_fx_track_id: None,
            remote_phase: 0.0,
            search_input: Input::new(),
            search_list: ListState::new().with_empty("Type to search tracks & playlists"),
            search_items: Vec::new(),
            spotify_search_cache: SearchResults::default(),
            search_gen: 0,
            pending_search_at: None,
            sidebar_list: ListState::new().with_empty("No playlists yet — press R to refresh"),
            collection_list: ListState::new()
                .with_empty("Select a playlist on the left, or press / to search"),
            queue_list: ListState::new().with_empty("Queue is empty"),
            settings_list: ListState::new(),
            rows_height: Cell::new(14),
            hits: Cell::new(HitRects::default()),
            last_finished_gen: 0,
            last_pos_sec: 0,
            last_dur_sec: 0,
            time_buf: "00:00 / 00:00".to_string(),
            codec_buf: "IDLE".to_string(),
            device_pill_buf: "○ Sign in".to_string(),
            vol_buf: "VOL 80%".to_string(),
            queue_title_buf: "UP NEXT // 0 TRACKS".to_string(),
            collection_title_buf: "LIBRARY".to_string(),
            search_title_buf: "SEARCH".to_string(),
            status_line: "Ready · / search · D device · v viz · ? help".to_string(),
            header: Header::new("Hayato").subtitle("Cassette"),
            keybar: Vec::new(),
            design_override: None,
            border_ramp: design.ramp(ColorRole::Border, ColorRole::Accent),
            spectrum_ramp: design.ramp(ColorRole::AccentAlt, ColorRole::Accent),
            fx: None,
            last_fx_tick: Instant::now(),
            headless_test: false,
        };

        screen.vol_buf = format!("VOL {}%", screen.config.volume_pct);
        screen.rebuild_sidebar_and_home();
        screen.rebuild_settings_rows();
        screen.sync_chrome();

        if let Some(path) = initial_play
            && path.is_file()
        {
            let t = read_track_metadata(&path, &cache_dir().join("art"));
            screen.play_local_tracks(vec![t], 0);
        }

        screen
    }

    /// Deterministic constructor for snapshot tests (no background threads/DBus).
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
        let sample_cols = vec![
            Collection {
                id: "spotify:liked".into(),
                provider: ProviderId::Spotify,
                kind: CollectionKind::LikedSongs,
                name: "Liked Songs".into(),
                owner: Some("Bruno".into()),
                track_count: Some(842),
                context_uri: None,
                tracks_restricted: false,
            },
            Collection {
                id: "spotify:playlist:demo".into(),
                provider: ProviderId::Spotify,
                kind: CollectionKind::Playlist,
                name: "Night Drive Synth".into(),
                owner: Some("Bruno".into()),
                track_count: Some(47),
                context_uri: Some("spotify:playlist:demo".into()),
                tracks_restricted: false,
            },
            Collection {
                id: "local:all".into(),
                provider: ProviderId::Local,
                kind: CollectionKind::AllTracks,
                name: "All Local Tracks".into(),
                owner: Some("This Machine".into()),
                track_count: Some(tracks.len() as u32),
                context_uri: None,
                tracks_restricted: false,
            },
        ];

        let mut screen = Self {
            config,
            focus: Focus::Main,
            stage: MainStage::Collection,
            prev_stage: MainStage::Collection,
            drawer: RightDrawer::Hidden,
            sidebar_open: true,
            source_filter: SourceFilter::All,
            mini,
            focused: true,
            unfocused_since: None,
            phase: CassettePhase::Normal,
            engine,
            analyzer,
            hub: ProviderHub::inert(),
            queue,
            local_tracks: tracks.clone(),
            collections: sample_cols.clone(),
            filtered_collections: Vec::new(),
            active_collection: sample_cols.first().cloned(),
            playing_collection_id: sample_cols.first().map(|c| c.id.clone()),
            active_collection_tracks: tracks,
            active_collection_restricted: None,
            loading_collection: false,
            lyrics: None,
            lyrics_track_id: None,
            lyrics_loading: false,
            art: AlbumArt::off(),
            mpris,
            mpris_rx,
            spotify_authed: true,
            spotify_user: "Bruno".into(),
            spotify_premium: true,
            local_connect_ready: false,
            local_device_id: None,
            preferred_device_id: None,
            local_spotify_pcm_active: Arc::new(AtomicBool::new(false)),
            devices: vec![PlaybackDevice {
                id: "dev1".into(),
                provider: ProviderId::Spotify,
                name: "Living Room Speaker".into(),
                kind: "Speaker".into(),
                is_active: true,
                volume_pct: 80,
            }],
            active_provider: ProviderId::Local,
            remote_state: PlayState::Stopped,
            remote_track: None,
            remote_pos_ms: 0,
            remote_dur_ms: 0,
            remote_synced_at: Instant::now(),
            optimistic_until: None,
            last_fx_track_id: None,
            remote_phase: 0.0,
            search_input: Input::new(),
            search_list: ListState::new(),
            search_items: Vec::new(),
            spotify_search_cache: SearchResults::default(),
            search_gen: 0,
            pending_search_at: None,
            sidebar_list: ListState::new(),
            collection_list: ListState::new(),
            queue_list: ListState::new(),
            settings_list: ListState::new(),
            rows_height: Cell::new(14),
            hits: Cell::new(HitRects::default()),
            last_finished_gen: 0,
            last_pos_sec: 134,
            last_dur_sec: 245,
            time_buf: "02:14 / 04:05".to_string(),
            codec_buf: "FLAC · 48kHz · 24-bit".to_string(),
            device_pill_buf: "● This Device ▾".to_string(),
            vol_buf: "VOL 80%".to_string(),
            queue_title_buf: "UP NEXT // 2 TRACKS (07:47)".to_string(),
            collection_title_buf: "LIKED SONGS · 842 TRACKS".to_string(),
            search_title_buf: "SEARCH".to_string(),
            status_line: "Ready".to_string(),
            header: Header::new("Hayato").subtitle("Cassette"),
            keybar: Vec::new(),
            design_override: None,
            border_ramp: design.ramp(ColorRole::Border, ColorRole::Accent),
            spectrum_ramp: design.ramp(ColorRole::AccentAlt, ColorRole::Accent),
            fx: None,
            last_fx_tick: Instant::now(),
            headless_test: true,
        };
        screen.rebuild_sidebar_and_home();
        screen.rebuild_collection_rows();
        screen.rebuild_queue_rows();
        screen.rebuild_settings_rows();
        screen.sync_chrome();
        screen
    }

    fn set_mini_mode(&mut self, mini: bool) {
        self.mini = mini;
        self.config.mini_mode = mini;
        let _ = save_config(&self.config);
        let _ = std::fs::create_dir_all(runtime_dir());
        let _ = std::fs::write(
            runtime_dir().join("mode"),
            if mini { "mini" } else { "full" },
        );
        let _ = sway_ipc::resize_window(mini);
    }

    fn current_track(&self) -> Option<&Track> {
        if self.active_provider == ProviderId::Spotify {
            self.remote_track
                .as_ref()
                .or_else(|| self.queue.current_track())
        } else {
            self.queue.current_track().or(self.remote_track.as_ref())
        }
    }

    fn current_play_state(&self) -> PlayState {
        if self.active_provider == ProviderId::Spotify {
            self.remote_state
        } else {
            self.engine.telemetry.play_state()
        }
    }

    fn is_local_device(&self, d: &PlaybackDevice) -> bool {
        if let Some(local_id) = &self.local_device_id
            && d.id == *local_id
        {
            return true;
        }
        d.name.eq_ignore_ascii_case("Hayato Cassette")
    }

    fn target_device(&self) -> Option<&PlaybackDevice> {
        // 1. Explicit user pick from the D Device Picker takes priority.
        if let Some(pref) = &self.preferred_device_id
            && let Some(d) = self.devices.iter().find(|d| &d.id == pref)
        {
            return Some(d);
        }
        // 2. Default to THIS Chromebook ("Hayato Cassette") even if an external
        //    speaker (e.g. Echo) was active from an earlier session.
        if let Some(local) = self.devices.iter().find(|d| self.is_local_device(d)) {
            return Some(local);
        }
        // 3. Fallback to active or first available Spotify Connect device.
        self.devices
            .iter()
            .find(|d| d.is_active)
            .or_else(|| self.devices.first())
    }

    fn active_device_id(&self) -> Option<String> {
        if let Some(dev) = self.target_device() {
            return Some(dev.id.clone());
        }
        // If our embedded receiver is ready before `GET /me/player/devices` refreshes,
        // still target this Chromebook directly.
        if self.local_connect_ready && self.preferred_device_id.is_none() {
            return self.local_device_id.clone();
        }
        None
    }

    fn describe_target_device(&self) -> Option<(bool, String)> {
        if let Some(dev) = self.target_device() {
            return Some((self.is_local_device(dev), dev.name.clone()));
        }
        if self.local_connect_ready && self.preferred_device_id.is_none() {
            return Some((true, "Hayato Cassette".to_string()));
        }
        None
    }

    fn play_local_tracks(&mut self, tracks: Vec<Track>, start_idx: usize) {
        if let Some(track) = self.queue.set_and_play(tracks, start_idx).cloned() {
            self.active_provider = ProviderId::Local;
            self.start_local_track(&track);
            self.rebuild_queue_rows();
            self.sync_chrome();
        }
    }

    fn request_lyrics_for(&mut self, track: &Track) {
        if self.lyrics_track_id.as_ref() == Some(&track.id)
            && (self.lyrics.is_some() || self.lyrics_loading)
        {
            return;
        }
        self.lyrics_track_id = Some(track.id.clone());
        if let SourceKind::Local { path, .. } = &track.source
            && let Some(sidecar) = Lyrics::load_sidecar(path)
        {
            self.lyrics = Some(sidecar);
            self.lyrics_loading = false;
            return;
        }
        self.lyrics = None;
        if self.headless_test {
            self.lyrics_loading = false;
            return;
        }
        self.lyrics_loading = true;
        self.hub.send(ProviderCmd::FetchLyrics {
            track_id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_secs: track.duration_secs,
        });
    }

    fn enqueue_track(&mut self, track: Track, play_next: bool) {
        let title = track.title.clone();
        if play_next {
            self.queue.push_next(track.clone());
        } else {
            self.queue.push_back(track.clone());
        }
        self.rebuild_queue_rows();
        if track.is_spotify() {
            self.hub.send(ProviderCmd::AddToQueue {
                provider: ProviderId::Spotify,
                track,
                device_id: self.active_device_id(),
            });
            self.status_line = format!("Queueing: {title}…");
        } else {
            self.status_line = if play_next {
                format!("Play next: {title}")
            } else {
                format!("Queued: {title}")
            };
        }
    }

    fn start_local_track(&mut self, track: &Track) {
        self.art.set_path(track.art_path.as_deref());
        self.request_lyrics_for(track);
        self.last_pos_sec = 0;
        self.last_dur_sec = track.duration_secs;
        self.time_buf = format!("00:00 / {}", format_time(track.duration_secs));
        if let SourceKind::Local { path, codec } = &track.source {
            self.codec_buf = codec.short_label();
            self.engine.play_file(path.clone(), track.duration_secs);
        }
        self.trigger_track_fx_for(&track.id);
        self.rebuild_sidebar_and_home();
        self.rebuild_collection_rows();
        self.publish_external_state();
    }

    fn play_spotify_track(&mut self, track: &Track, context_uri: Option<&str>) {
        self.engine.stop();
        self.active_provider = ProviderId::Spotify;
        self.remote_track = Some(track.clone());
        self.remote_state = PlayState::Playing;
        self.remote_pos_ms = 0;
        self.remote_dur_ms = track.duration_secs.saturating_mul(1000);
        self.remote_synced_at = Instant::now();
        self.optimistic_until = Some(Instant::now() + Duration::from_secs(3));
        self.last_pos_sec = 0;
        self.last_dur_sec = track.duration_secs;
        self.time_buf = format!("00:00 / {}", format_time(track.duration_secs));
        self.codec_buf = "CONNECT · 320k".to_string();
        self.art.set_path(track.art_path.as_deref());
        self.request_lyrics_for(track);
        self.trigger_track_fx_for(&track.id);

        let dev_id = self.active_device_id();
        if let Some((is_local, dev_name)) = self.describe_target_device() {
            self.status_line = if is_local {
                format!("Playing {} on This Device", track.title)
            } else if !self.local_connect_ready {
                format!("Playing on {dev_name} · press D then E to enable This Device audio")
            } else {
                format!("Playing {} on {dev_name}", track.title)
            };
        }
        if let (Some(ctx), SourceKind::Spotify { uri, .. }) = (context_uri, &track.source) {
            self.hub.send(ProviderCmd::PlayContext {
                provider: ProviderId::Spotify,
                context_uri: ctx.to_string(),
                offset_uri: Some(uri.clone()),
                device_id: dev_id,
            });
        } else {
            self.hub.send(ProviderCmd::PlayTrack {
                provider: ProviderId::Spotify,
                track: track.clone(),
                device_id: dev_id,
            });
        }
        self.rebuild_sidebar_and_home();
        self.rebuild_collection_rows();
        self.publish_external_state();
    }

    fn play_collection_now(&mut self, col: &Collection) {
        self.playing_collection_id = Some(col.id.clone());
        match col.provider {
            ProviderId::Spotify => {
                if let Some(ctx) = &col.context_uri {
                    self.engine.stop();
                    self.active_provider = ProviderId::Spotify;
                    self.remote_state = PlayState::Playing;
                    self.codec_buf = "CONNECT · 320k".to_string();
                    self.hub.send(ProviderCmd::PlayContext {
                        provider: ProviderId::Spotify,
                        context_uri: ctx.clone(),
                        offset_uri: None,
                        device_id: self.active_device_id(),
                    });
                    self.status_line = match self.describe_target_device() {
                        Some((true, _)) => format!("Playing {} on This Device", col.name),
                        Some((false, dev_name)) if !self.local_connect_ready => format!(
                            "Playing {} on {dev_name} · press D then E for This Device audio",
                            col.name
                        ),
                        Some((false, dev_name)) => format!("Playing {} on {dev_name}", col.name),
                        None => format!("Playing {} on Spotify Connect", col.name),
                    };
                } else if self
                    .active_collection
                    .as_ref()
                    .is_some_and(|c| c.id == col.id)
                    && let Some(first) = self.active_collection_tracks.first().cloned()
                {
                    self.play_spotify_track(&first, None);
                } else {
                    self.open_collection(col.clone());
                }
            }
            ProviderId::Local => {
                if self
                    .active_collection
                    .as_ref()
                    .is_some_and(|c| c.id == col.id)
                    && !self.active_collection_tracks.is_empty()
                {
                    let list = self.active_collection_tracks.clone();
                    self.play_local_tracks(list, 0);
                } else if col.id == "local:all" && !self.local_tracks.is_empty() {
                    let list = self.local_tracks.clone();
                    self.play_local_tracks(list, 0);
                } else {
                    self.open_collection(col.clone());
                }
            }
        }
    }

    fn open_collection(&mut self, col: Collection) {
        self.prev_stage = self.stage.clone();
        self.stage = MainStage::Collection;
        self.focus = Focus::Main;
        self.active_collection = Some(col.clone());
        self.active_collection_tracks.clear();
        self.active_collection_restricted = None;
        self.loading_collection = true;
        let count_txt = col
            .track_count
            .map(|n| format!("{n} TRACKS"))
            .unwrap_or_else(|| "LOADING…".into());
        self.collection_title_buf = format!("{} · {count_txt}", col.name.to_uppercase());
        self.collection_list
            .set_rows(vec![Row::new("Loading tracks…").glyph(Glyph::Music)]);
        self.hub.send(ProviderCmd::LoadCollection(col));
        self.sync_chrome();
    }

    fn trigger_track_fx_for(&mut self, id: &TrackId) {
        if self.last_fx_track_id.as_ref() == Some(id) {
            return;
        }
        self.last_fx_track_id = Some(id.clone());
        self.fx = Some(fx::coalesce(EffectTimer::from_ms(
            220,
            Interpolation::CubicOut,
        )));
        self.last_fx_tick = Instant::now();
    }

    fn toggle_play_pause(&mut self) {
        if self.active_provider == ProviderId::Spotify {
            self.optimistic_until = Some(Instant::now() + Duration::from_secs(3));
            if self.remote_state == PlayState::Playing {
                self.remote_state = PlayState::Paused;
                self.hub.send(ProviderCmd::RemotePause(ProviderId::Spotify));
            } else {
                self.remote_state = PlayState::Playing;
                self.remote_synced_at = Instant::now();
                self.hub
                    .send(ProviderCmd::RemoteResume(ProviderId::Spotify));
            }
            self.sync_chrome();
            self.publish_external_state();
            return;
        }
        if self.queue.current_track().is_none() {
            if !self.local_tracks.is_empty() {
                let all = self.local_tracks.clone();
                self.play_local_tracks(all, 0);
            }
            return;
        }
        match self.engine.telemetry.play_state() {
            PlayState::Playing => self.engine.pause(),
            PlayState::Paused => self.engine.resume(),
            PlayState::Stopped => {
                if let Some(t) = self.queue.current_track().cloned() {
                    self.start_local_track(&t);
                }
            }
        }
        self.sync_chrome();
        self.publish_external_state();
    }

    fn next_track(&mut self, auto_end: bool) {
        if self.active_provider == ProviderId::Spotify {
            self.hub.send(ProviderCmd::RemoteNext(ProviderId::Spotify));
            return;
        }
        if let Some(t) = self.queue.advance_next(auto_end).cloned() {
            self.start_local_track(&t);
            self.rebuild_queue_rows();
        } else {
            self.engine.stop();
            self.publish_external_state();
        }
        self.sync_chrome();
    }

    fn prev_track(&mut self) {
        if self.active_provider == ProviderId::Spotify {
            self.hub.send(ProviderCmd::RemotePrev(ProviderId::Spotify));
            return;
        }
        if self.last_pos_sec > 3 {
            self.seek_to_secs(0);
            return;
        }
        if let Some(t) = self.queue.advance_prev().cloned() {
            self.start_local_track(&t);
            self.rebuild_queue_rows();
        }
        self.sync_chrome();
    }

    fn seek_delta(&mut self, delta: i64) {
        let target = (self.last_pos_sec as i64 + delta).clamp(0, self.last_dur_sec as i64) as u64;
        self.seek_to_secs(target);
    }

    fn seek_to_secs(&mut self, secs: u64) {
        self.last_pos_sec = secs;
        self.time_buf = format!(
            "{} / {}",
            format_time(self.last_pos_sec),
            format_time(self.last_dur_sec)
        );
        if self.active_provider == ProviderId::Spotify {
            self.remote_pos_ms = secs.saturating_mul(1000);
            self.remote_synced_at = Instant::now();
            self.hub.send(ProviderCmd::RemoteSeekMs(
                ProviderId::Spotify,
                self.remote_pos_ms,
            ));
        } else {
            self.engine.seek_secs(secs);
        }
    }

    fn adjust_volume(&mut self, delta: i16) {
        let next = (i16::from(self.config.volume_pct) + delta).clamp(0, 100) as u8;
        self.config.volume_pct = next;
        self.vol_buf = format!("VOL {next}%");
        let _ = save_config(&self.config);
        if self.active_provider == ProviderId::Spotify {
            self.hub
                .send(ProviderCmd::RemoteVolume(ProviderId::Spotify, next));
        } else {
            self.engine.set_volume(next);
        }
        self.publish_external_state();
    }

    fn toggle_mute(&mut self) {
        let muted = !self.engine.telemetry.muted();
        self.engine.set_muted(muted);
        if self.active_provider == ProviderId::Spotify {
            self.hub.send(ProviderCmd::RemoteVolume(
                ProviderId::Spotify,
                if muted { 0 } else { self.config.volume_pct },
            ));
        }
        self.vol_buf = if muted {
            "VOL MUTE".to_string()
        } else {
            format!("VOL {}%", self.config.volume_pct)
        };
    }

    fn toggle_shuffle(&mut self) {
        self.queue.shuffle = !self.queue.shuffle;
        if self.active_provider == ProviderId::Spotify {
            self.hub.send(ProviderCmd::RemoteShuffle(
                ProviderId::Spotify,
                self.queue.shuffle,
            ));
        }
    }

    fn cycle_repeat(&mut self) {
        self.queue.repeat = self.queue.repeat.cycle();
        if self.active_provider == ProviderId::Spotify {
            self.hub.send(ProviderCmd::RemoteRepeat(
                ProviderId::Spotify,
                self.queue.repeat,
            ));
        }
    }

    fn share_current(&mut self) {
        let Some(track) = self.current_track().cloned() else {
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
        let track = self.current_track();
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

    fn rebuild_sidebar_and_home(&mut self) {
        self.filtered_collections = self
            .collections
            .iter()
            .filter(|c| self.source_filter.matches(c.provider))
            .cloned()
            .collect();

        let is_playing = self.current_play_state() != PlayState::Stopped;
        let playing_col = if is_playing {
            self.playing_collection_id.as_deref()
        } else {
            None
        };

        let mut s_rows = Vec::new();
        for c in &self.filtered_collections {
            let count = c
                .track_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "…".into());
            let glyph = match c.kind {
                CollectionKind::LikedSongs => Glyph::Star,
                CollectionKind::Playlist => Glyph::Music,
                CollectionKind::AllTracks => Glyph::Folder,
            };
            let is_active_col = playing_col == Some(c.id.as_str());
            let prefix = if is_active_col {
                "♫ "
            } else if c.kind == CollectionKind::LikedSongs {
                "♥ "
            } else {
                ""
            };
            let mut row = Row::new(format!("{prefix}{}", c.name))
                .value(count)
                .glyph(glyph);
            if is_active_col {
                row = row.tone(ColorRole::Accent);
            }
            s_rows.push(row);
        }
        self.sidebar_list.update_rows(s_rows);
    }

    fn rebuild_collection_rows(&mut self) {
        let playing_id = (self.current_play_state() != PlayState::Stopped)
            .then(|| self.current_track().map(|t| t.id.clone()))
            .flatten();
        let rows: Vec<Row> = self
            .active_collection_tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let is_playing = playing_id.as_ref() == Some(&t.id);
                let label = if is_playing {
                    format!("▶   {}", t.title)
                } else {
                    format!("{:02}  {}", i + 1, t.title)
                };
                let mut row = Row::new(label)
                    .detail(format!("{} · {}", t.artist, t.album))
                    .value(format_time(t.duration_secs))
                    .glyph(if t.is_spotify() {
                        Glyph::Wifi
                    } else {
                        Glyph::Music
                    });
                if is_playing {
                    row = row.tone(ColorRole::Accent);
                }
                row
            })
            .collect();
        self.collection_list.update_rows(rows);
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
        self.queue_title_buf = format!(
            "UP NEXT // {} TRACKS ({})",
            self.queue.len(),
            format_time(self.queue.remaining_duration_secs())
        );
    }

    fn rebuild_search_rows(&mut self) {
        let q = self.search_input.text().trim().to_lowercase();
        self.search_items.clear();
        let mut rows = Vec::new();

        if q.is_empty() {
            self.search_title_buf = "SEARCH".to_string();
            self.search_list.update_rows(rows);
            return;
        }

        // 1. Streaming Tracks (from debounced background worker)
        if self.source_filter.matches(ProviderId::Spotify)
            && !self.spotify_search_cache.tracks.is_empty()
        {
            rows.push(Row::heading("Tracks"));
            for t in &self.spotify_search_cache.tracks {
                rows.push(
                    Row::new(&t.title)
                        .detail(format!("{} · {}", t.artist, t.album))
                        .value(format_time(t.duration_secs))
                        .glyph(Glyph::Wifi),
                );
                self.search_items.push(SearchRowItem::Track(t.clone()));
            }
        }

        // 2. Streaming Playlists
        if self.source_filter.matches(ProviderId::Spotify)
            && !self.spotify_search_cache.collections.is_empty()
        {
            rows.push(Row::heading("Playlists"));
            for c in &self.spotify_search_cache.collections {
                let cnt = c
                    .track_count
                    .map(|n| format!("{n} tracks"))
                    .unwrap_or_else(|| "open".into());
                rows.push(
                    Row::new(&c.name)
                        .detail("Playlist")
                        .value(cnt)
                        .glyph(Glyph::Folder),
                );
                self.search_items.push(SearchRowItem::Collection(c.clone()));
            }
        }

        // 3. Instant Local Matches (0 ms)
        if self.source_filter.matches(ProviderId::Local) {
            let local_hits: Vec<Track> = self
                .local_tracks
                .iter()
                .filter(|t| {
                    t.title.to_lowercase().contains(&q)
                        || t.artist.to_lowercase().contains(&q)
                        || t.album.to_lowercase().contains(&q)
                })
                .take(25)
                .cloned()
                .collect();
            if !local_hits.is_empty() {
                rows.push(Row::heading("Local"));
                for t in local_hits {
                    rows.push(
                        Row::new(&t.title)
                            .detail(format!("{} · {}", t.artist, t.album))
                            .value(format_time(t.duration_secs))
                            .glyph(Glyph::Music),
                    );
                    self.search_items.push(SearchRowItem::Track(t));
                }
            }
        }

        self.search_title_buf = format!(
            "SEARCH // \"{}\" ({}) [{}]",
            self.search_input.text().trim(),
            self.search_items.len(),
            self.source_filter.label()
        );
        self.search_list.update_rows(rows);
    }

    fn rebuild_settings_rows(&mut self) {
        let sp_status = if self.spotify_authed {
            let tier = if self.spotify_premium {
                "PREMIUM"
            } else {
                "FREE"
            };
            format!("{} [{tier}]", self.spotify_user)
        } else {
            "Not signed in".to_string()
        };
        let dirs_str: String = self
            .config
            .music_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let local_audio_detail = if self.local_connect_ready {
            "Active — defaulting Spotify playback to this Chromebook (Hayato Cassette)"
        } else {
            "Play Spotify directly on this Chromebook (1-click browser pairing or LAN Connect)"
        };
        let rows = vec![
            Row::new(if self.spotify_authed {
                "Spotify Account (Enter to Sign Out)"
            } else {
                "Sign in with Spotify (Browser PKCE)"
            })
            .detail(sp_status)
            .value(if self.spotify_authed {
                "linked"
            } else {
                "login"
            })
            .glyph(Glyph::Wifi),
            Row::new("This Device Audio (Spotify Connect)")
                .detail(local_audio_detail)
                .value(if self.local_connect_ready {
                    "ready"
                } else {
                    "enable"
                })
                .glyph(Glyph::Volume),
            Row::new("Spotify Developer Client ID")
                .detail("Redirect URI: http://127.0.0.1:8898/callback")
                .value(if self.config.spotify_client_id.is_empty() {
                    "unset"
                } else {
                    "configured"
                })
                .glyph(Glyph::Key),
            Row::new("Visualizer Mode")
                .detail("Cycle Spectrum, Mirror EQ, Oscilloscope, VU Deck, Cyber Plasma, or Off")
                .value(self.config.visualizer.label())
                .glyph(Glyph::Music),
            Row::new("Refresh Spotify & Rescan Local Music")
                .detail(dirs_str)
                .value("R")
                .glyph(Glyph::Folder),
        ];
        self.settings_list.update_rows(rows);
    }

    fn rebuild_device_pill(&mut self) {
        if !self.spotify_authed {
            self.device_pill_buf = "○ Sign in".to_string();
        } else if let Some((is_local, name)) = self.describe_target_device() {
            if is_local {
                self.device_pill_buf = "● This Device ▾".to_string();
            } else {
                let short: String = name.chars().take(16).collect();
                self.device_pill_buf = format!("◉ {short} ▾");
            }
        } else {
            self.device_pill_buf = "○ Enable Audio ▾".to_string();
        }
    }

    fn open_device_picker(&mut self) {
        if !self.spotify_authed {
            self.stage = MainStage::Settings;
            self.focus = Focus::Main;
            self.sync_chrome();
            return;
        }
        let empty_msg = if self.local_connect_ready {
            "Connecting This Device (Hayato Cassette)… press R to refresh"
        } else {
            "No local audio yet — press E to enable This Device audio, or R to refresh"
        };
        let mut list = ListState::new().with_empty(empty_msg);
        let target_id = self.active_device_id();
        let mut rows: Vec<Row> = Vec::new();
        if !self.local_connect_ready {
            rows.push(
                Row::new("★ Enable This Device Audio (Play on this Chromebook)")
                    .detail("1-click Spotify pairing · defaults playback to this Chromebook")
                    .value("enable")
                    .glyph(Glyph::Volume),
            );
        }
        rows.extend(self.devices.iter().map(|d| {
            let is_local = self.is_local_device(d);
            let is_target = target_id.as_deref() == Some(d.id.as_str());
            let dot = if is_target { "● " } else { "○ " };
            let label = if is_local {
                format!("{dot}{} (This Device)", d.name)
            } else {
                format!("{dot}{}", d.name)
            };
            let badge = if is_target {
                "default"
            } else if d.is_active {
                "active"
            } else {
                "transfer"
            };
            Row::new(label)
                .detail(format!("{} · Vol {}%", d.kind, d.volume_pct))
                .value(badge)
                .glyph(Glyph::Volume)
        }));
        list.set_rows(rows);
        self.phase = CassettePhase::DevicePicker(list);
        self.sync_chrome();
    }

    fn open_palette(&mut self) {
        let mut list = ListState::new().ranked();
        list.set_rows(vec![
            Row::new("Play / Pause").value("Space").glyph(Glyph::Play),
            Row::new("Spotify Connect Devices")
                .value("D")
                .glyph(Glyph::Volume),
            Row::new("Search Spotify & Local")
                .value("/")
                .glyph(Glyph::Search),
            Row::new("Full Cyberdeck Visualizer")
                .value("V")
                .glyph(Glyph::Music),
            Row::new("Toggle Mini HUD").value("w").glyph(Glyph::Window),
            Row::new("Toggle Left Library Sidebar")
                .value("b")
                .glyph(Glyph::Folder),
            Row::new("Toggle Right Queue Drawer")
                .value("q")
                .glyph(Glyph::Music),
            Row::new("Toggle Synced Lyrics")
                .value("L")
                .glyph(Glyph::File),
            Row::new("Share Current Track (Hayato Share)")
                .value("Ctrl+S")
                .glyph(Glyph::Qr),
            Row::new("Refresh Spotify & Local Library")
                .value("R")
                .glyph(Glyph::Folder),
            Row::new("Save Queue as .m3u8 Playlist")
                .value("S")
                .glyph(Glyph::Save),
            Row::new("Keymap Help").value("?").glyph(Glyph::Info),
            Row::new("Hide to Scratchpad")
                .value("Esc")
                .glyph(Glyph::Window),
            Row::new("Quit Hayato Cassette")
                .value("Q")
                .glyph(Glyph::Power),
        ]);
        self.phase = CassettePhase::Palette(list);
        self.sync_chrome();
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
        self.header = Header::new("Hayato").subtitle(sub).status(state_dot);

        self.rebuild_device_pill();

        // Dynamic per-focus / per-phase keybar keeping Esc on the right
        self.keybar = match &self.phase {
            CassettePhase::DevicePicker(_) => {
                let mut hints = vec![KeyHint::new("Enter", "use device")];
                if !self.local_connect_ready {
                    hints.push(KeyHint::new("E", "enable this device"));
                }
                hints.push(KeyHint::new("R", "refresh"));
                hints.push(KeyHint::new("Esc", "back"));
                hints
            }
            CassettePhase::Help => vec![KeyHint::new("Esc", "close")],
            CassettePhase::Palette(_) => {
                vec![KeyHint::new("Enter", "run"), KeyHint::new("Esc", "back")]
            }
            CassettePhase::EditClientId(_) => {
                vec![KeyHint::new("Enter", "save"), KeyHint::new("Esc", "cancel")]
            }
            CassettePhase::ConfirmClearQueue(_) | CassettePhase::ConfirmLogoutSpotify(_) => vec![
                KeyHint::new("←/→", "choose"),
                KeyHint::new("Enter", "confirm"),
                KeyHint::new("Esc", "cancel"),
            ],
            CassettePhase::Normal => match self.focus {
                Focus::SearchBar => vec![
                    KeyHint::new("Enter", "play/open"),
                    KeyHint::new("Ctrl+A", "enqueue"),
                    KeyHint::new("Tab", "scope"),
                    KeyHint::new("Esc", "clear/back"),
                ],
                Focus::Sidebar => vec![
                    KeyHint::new("Enter", "open"),
                    KeyHint::new("P", "play list"),
                    KeyHint::new("f", "filter"),
                    KeyHint::new("Tab", "pane"),
                    KeyHint::new("Esc", "hide"),
                ],
                Focus::RightDrawer => vec![
                    KeyHint::new("Enter", "jump"),
                    KeyHint::new("J/K", "reorder"),
                    KeyHint::new("Ctrl+X", "clear"),
                    KeyHint::new("S", "save"),
                    KeyHint::new("Esc", "back"),
                ],
                Focus::Main => match self.stage {
                    MainStage::Collection => vec![
                        KeyHint::new("Enter", "play"),
                        KeyHint::new("P", "play all"),
                        KeyHint::new("a/A", "queue"),
                        KeyHint::new("/", "search"),
                        KeyHint::new("D", "device"),
                        KeyHint::new("Esc", "back"),
                    ],
                    _ => vec![
                        KeyHint::new("Space", "play/pause"),
                        KeyHint::new("/", "search"),
                        KeyHint::new("D", "device"),
                        KeyHint::new("V", "viz"),
                        KeyHint::new("Tab", "pane"),
                        KeyHint::new("?", "help"),
                        KeyHint::new("Esc", "hide"),
                    ],
                },
            },
        };
    }

    fn drain_provider_events(&mut self) {
        while let Some(ev) = self.hub.try_recv() {
            match ev {
                ProviderEvent::CollectionsUpdated {
                    collections,
                    local_tracks,
                } => {
                    self.collections = collections;
                    self.local_tracks = local_tracks;
                    self.rebuild_sidebar_and_home();
                    if self.active_collection.is_none()
                        && let Some(first) = self.filtered_collections.first().cloned()
                    {
                        self.open_collection(first);
                    }
                    if self.stage == MainStage::Search {
                        self.rebuild_search_rows();
                    }
                    self.sync_chrome();
                }
                ProviderEvent::CollectionLoaded {
                    collection_id,
                    items,
                } => {
                    if self
                        .active_collection
                        .as_ref()
                        .is_some_and(|c| c.id == collection_id)
                    {
                        self.loading_collection = false;
                        match items {
                            CollectionItems::Tracks(tracks) => {
                                self.active_collection_restricted = None;
                                let cnt = tracks.len();
                                self.active_collection_tracks = tracks;
                                if let Some(c) = &self.active_collection {
                                    self.collection_title_buf = format!(
                                        "{} // {} · {cnt} TRACKS",
                                        c.name.to_uppercase(),
                                        c.provider.label()
                                    );
                                }
                                self.rebuild_collection_rows();
                            }
                            CollectionItems::Restricted {
                                context_uri,
                                reason,
                            } => {
                                self.active_collection_tracks.clear();
                                self.active_collection_restricted = Some((context_uri, reason));
                                self.collection_list.update_rows(Vec::new());
                            }
                        }
                    }
                }
                ProviderEvent::SearchCompleted { seq, spotify, .. } => {
                    if seq == self.search_gen {
                        self.spotify_search_cache = spotify;
                        self.rebuild_search_rows();
                    }
                }
                ProviderEvent::DevicesUpdated(mut devs) => {
                    // Always sort This Device ("Hayato Cassette") to the top of the list.
                    let local_id = self.local_device_id.clone();
                    devs.sort_by_key(|d| {
                        let is_loc = local_id.as_deref() == Some(d.id.as_str())
                            || d.name.eq_ignore_ascii_case("Hayato Cassette");
                        if is_loc {
                            0
                        } else if d.is_active {
                            1
                        } else {
                            2
                        }
                    });
                    self.devices = devs;
                    self.rebuild_device_pill();
                    if matches!(self.phase, CassettePhase::DevicePicker(_)) {
                        self.open_device_picker();
                    }
                }
                ProviderEvent::LocalConnectChanged { ready, device_id } => {
                    self.local_connect_ready = ready;
                    self.local_device_id = device_id;
                    self.rebuild_device_pill();
                    self.rebuild_settings_rows();
                    if matches!(self.phase, CassettePhase::DevicePicker(_)) {
                        self.open_device_picker();
                    }
                    self.sync_chrome();
                }
                ProviderEvent::RemotePlayback(snap) => {
                    let snap = *snap;
                    let local_playing = self.engine.telemetry.play_state() == PlayState::Playing;
                    if !local_playing || self.active_provider == ProviderId::Spotify {
                        // Ignore stale cloud snapshots during our 3s optimistic window after
                        // a user-initiated play/pause/track switch so the UI and FX never bounce.
                        if let Some(until) = self.optimistic_until {
                            let now = Instant::now();
                            if now < until {
                                let same_track = snap.track.as_ref().map(|t| &t.id)
                                    == self.remote_track.as_ref().map(|t| &t.id);
                                if !same_track || snap.state != self.remote_state {
                                    continue;
                                }
                            }
                            self.optimistic_until = None;
                        }
                        if let Some(dev) = snap.device {
                            if let Some(slot) = self.devices.iter_mut().find(|d| d.id == dev.id) {
                                *slot = dev;
                            } else {
                                self.devices.push(dev);
                            }
                        }
                        self.active_provider = ProviderId::Spotify;
                        self.remote_state = snap.state;
                        self.remote_pos_ms = snap.position_ms;
                        self.remote_dur_ms = snap.duration_ms;
                        self.remote_synced_at = Instant::now();
                        self.queue.shuffle = snap.shuffle;
                        self.queue.repeat = snap.repeat;
                        self.config.volume_pct = snap.volume_pct;
                        self.vol_buf = format!("VOL {}%", snap.volume_pct);
                        self.codec_buf = "CONNECT · 320k".to_string();

                        if let Some(t) = snap.track {
                            let changed = self.remote_track.as_ref().map(|r| &r.id) != Some(&t.id);
                            self.art.set_path(t.art_path.as_deref());
                            self.request_lyrics_for(&t);
                            self.trigger_track_fx_for(&t.id);
                            self.remote_track = Some(t);
                            if changed {
                                self.rebuild_sidebar_and_home();
                                self.rebuild_collection_rows();
                            }
                        }
                        self.last_pos_sec = self.remote_pos_ms / 1000;
                        self.last_dur_sec = self.remote_dur_ms / 1000;
                        self.time_buf = format!(
                            "{} / {}",
                            format_time(self.last_pos_sec),
                            format_time(self.last_dur_sec)
                        );
                        self.sync_chrome();
                        self.publish_external_state();
                    }
                }
                ProviderEvent::SpotifyAuthChanged {
                    authenticated,
                    display_name,
                    is_premium,
                } => {
                    self.spotify_authed = authenticated;
                    self.spotify_user = display_name;
                    self.spotify_premium = is_premium;
                    self.rebuild_sidebar_and_home();
                    self.rebuild_settings_rows();
                    self.sync_chrome();
                }
                ProviderEvent::LyricsLoaded { track_id, lyrics } => {
                    if self.lyrics_track_id.as_ref() == Some(&track_id) {
                        self.lyrics = lyrics;
                        self.lyrics_loading = false;
                    }
                }
                ProviderEvent::Status(msg) | ProviderEvent::Error(msg) => {
                    self.status_line = msg;
                }
            }
        }
    }

    fn activate_collection_row(&mut self, i: usize) {
        if let Some(t) = self.active_collection_tracks.get(i).cloned() {
            self.playing_collection_id = self.active_collection.as_ref().map(|c| c.id.clone());
            if t.is_spotify() {
                let ctx = self
                    .active_collection
                    .as_ref()
                    .and_then(|c| c.context_uri.clone());
                let all = self.active_collection_tracks.clone();
                self.queue.set_and_play(all, i);
                self.rebuild_queue_rows();
                self.play_spotify_track(&t, ctx.as_deref());
            } else {
                let all = self.active_collection_tracks.clone();
                self.play_local_tracks(all, i);
            }
        }
    }

    fn activate_search_selection(&mut self) {
        if let Some(item) = self.selected_search_item() {
            match item {
                SearchRowItem::Track(t) => {
                    if t.is_spotify() {
                        self.play_spotify_track(&t, None);
                    } else {
                        self.play_local_tracks(vec![t], 0);
                    }
                }
                SearchRowItem::Collection(c) => {
                    self.open_collection(c);
                }
            }
        }
    }

    fn activate_queue_row(&mut self, i: usize) {
        if let Some(t) = self.queue.jump(i).cloned() {
            if t.is_spotify() {
                let ctx = self
                    .active_collection
                    .as_ref()
                    .and_then(|c| c.context_uri.clone());
                self.play_spotify_track(&t, ctx.as_deref());
            } else {
                self.active_provider = ProviderId::Local;
                self.start_local_track(&t);
            }
            self.rebuild_queue_rows();
        }
    }

    fn activate_settings_row(&mut self, idx: usize) {
        match idx {
            0 => {
                if self.spotify_authed {
                    let confirm = Confirm::new(
                        "Sign Out of Spotify",
                        "Remove saved Spotify OAuth token from this machine?",
                    )
                    .with_options(&["Cancel", "Sign out"])
                    .destructive(true);
                    self.phase = CassettePhase::ConfirmLogoutSpotify(confirm);
                } else {
                    self.hub.send(ProviderCmd::SpotifyLogin {
                        client_id: self.config.spotify_client_id.clone(),
                    });
                    self.status_line = "Opening Firefox for Spotify PKCE login…".into();
                }
            }
            1 => {
                if self.local_connect_ready {
                    self.preferred_device_id = None;
                    if let Some(id) = self.active_device_id() {
                        self.hub.send(ProviderCmd::TransferDevice {
                            provider: ProviderId::Spotify,
                            device_id: id,
                        });
                    }
                    self.status_line = "This Device audio is active (Hayato Cassette)".into();
                } else {
                    self.hub.send(ProviderCmd::EnableLocalConnect);
                    self.status_line = "Opening Firefox to enable This Device audio…".into();
                }
            }
            2 => {
                let mut inp = Input::new();
                inp.set(&self.config.spotify_client_id);
                self.phase = CassettePhase::EditClientId(inp);
            }
            3 => {
                self.config.visualizer = self.config.visualizer.cycle();
                let _ = save_config(&self.config);
                self.status_line = format!("Visualizer: {}", self.config.visualizer.label());
            }
            4 => {
                self.hub.send(ProviderCmd::RefreshAll {
                    music_dirs: self.config.music_dirs.clone(),
                });
                self.status_line = "Refreshing Spotify & local library…".into();
            }
            _ => {}
        }
        self.rebuild_settings_rows();
        self.sync_chrome();
    }

    fn activate_device_picker_row(&mut self, idx: usize) {
        if !self.local_connect_ready && idx == 0 {
            self.hub.send(ProviderCmd::EnableLocalConnect);
            self.status_line = "Opening Firefox to enable This Device audio…".into();
            self.phase = CassettePhase::Normal;
            self.sync_chrome();
            return;
        }
        let dev_idx = if self.local_connect_ready {
            idx
        } else {
            idx.saturating_sub(1)
        };
        if let Some(dev) = self.devices.get(dev_idx).cloned() {
            let is_local = self.is_local_device(&dev);
            self.preferred_device_id = if is_local { None } else { Some(dev.id.clone()) };
            self.hub.send(ProviderCmd::TransferDevice {
                provider: dev.provider,
                device_id: dev.id,
            });
            self.status_line = if is_local {
                "Switched Spotify playback to This Device".into()
            } else {
                format!("Switched Spotify playback to {}", dev.name)
            };
            self.phase = CassettePhase::Normal;
            self.sync_chrome();
        }
    }

    fn activate_palette_row(&mut self, idx: usize) {
        self.phase = CassettePhase::Normal;
        match idx {
            0 => self.toggle_play_pause(),
            1 => self.open_device_picker(),
            2 => {
                self.prev_stage = self.stage.clone();
                self.stage = MainStage::Search;
                self.focus = Focus::SearchBar;
            }
            3 => {
                self.stage = MainStage::Visualizer;
                self.focus = Focus::Main;
            }
            4 => self.set_mini_mode(!self.mini),
            5 => self.sidebar_open = !self.sidebar_open,
            6 => {
                self.drawer = if self.drawer == RightDrawer::Queue {
                    RightDrawer::Hidden
                } else {
                    RightDrawer::Queue
                };
            }
            7 => {
                self.drawer = if self.drawer == RightDrawer::Lyrics {
                    RightDrawer::Hidden
                } else {
                    RightDrawer::Lyrics
                };
            }
            8 => self.share_current(),
            9 => {
                self.hub.send(ProviderCmd::RefreshAll {
                    music_dirs: self.config.music_dirs.clone(),
                });
                self.status_line = "Refreshing Spotify & local library…".into();
            }
            10 => {
                if !self.queue.is_empty() {
                    match save_m3u8_playlist(
                        &self.config.music_dirs,
                        "Cassette Mix",
                        self.queue.items(),
                    ) {
                        Ok(p) => self.status_line = format!("Saved {}", p.display()),
                        Err(e) => self.status_line = e,
                    }
                }
            }
            11 => self.phase = CassettePhase::Help,
            12 => {
                let _ = sway_ipc::hide_to_scratchpad();
            }
            _ => {}
        }
        self.sync_chrome();
    }

    fn handle_mpris(&mut self) -> Outcome {
        while let Ok(cmd) = self.mpris_rx.try_recv() {
            match cmd {
                MprisCommand::Play => {
                    if self.current_play_state() != PlayState::Playing {
                        self.toggle_play_pause();
                    }
                }
                MprisCommand::Pause => {
                    if self.current_play_state() == PlayState::Playing {
                        self.toggle_play_pause();
                    }
                }
                MprisCommand::PlayPause => self.toggle_play_pause(),
                MprisCommand::Stop => {
                    self.engine.stop();
                    if self.active_provider == ProviderId::Spotify {
                        self.remote_state = PlayState::Paused;
                        self.hub.send(ProviderCmd::RemotePause(ProviderId::Spotify));
                    }
                }
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

    fn draw_top_chrome(&self, buf: &mut Buffer, area: Rect, design: &Design, hits: &mut HitRects) {
        let mut x = area.x;
        // Source Filter Tabs: [ All ] [ Spotify ] [ Local ]
        let tabs: [(SourceFilter, &str); 3] = [
            (SourceFilter::All, " All "),
            (SourceFilter::Spotify, " Spotify "),
            (SourceFilter::Local, " Local "),
        ];
        for (filter, lbl) in tabs {
            let w = lbl.chars().count() as u16;
            let st = if self.source_filter == filter {
                design.style(TextStyle::Key)
            } else {
                design.style(TextStyle::Dim)
            };
            let _ = put(buf, x, area.y, lbl, w, st);
            let r = Rect::new(x, area.y, w, 1);
            match filter {
                SourceFilter::All => hits.filter_all = r,
                SourceFilter::Spotify => hits.filter_spotify = r,
                SourceFilter::Local => hits.filter_local = r,
            }
            x += w + 1;
        }

        // Right-aligned [Device Pill] + [⚙]
        let gear_w = 5u16;
        let gear_x = area.right().saturating_sub(gear_w);
        let gear_st = if self.stage == MainStage::Settings {
            design.style(TextStyle::Key)
        } else {
            design.style(TextStyle::Dim)
        };
        let _ = put(buf, gear_x, area.y, " ⚙ 4 ", gear_w, gear_st);
        hits.settings = Rect::new(gear_x, area.y, gear_w, 1);

        let dev_w = (self.device_pill_buf.chars().count() as u16 + 2).min(24);
        let dev_x = gear_x.saturating_sub(dev_w + 1);
        if dev_x > x + 12 {
            let dev_st = Style::default().fg(design.color(if self.spotify_authed {
                ColorRole::Ok
            } else {
                ColorRole::Warn
            }));
            let _ = put_trunc(buf, dev_x, area.y, &self.device_pill_buf, dev_w, dev_st);
            hits.device = Rect::new(dev_x, area.y, dev_w, 1);
        }

        // Search Box: aligned cleanly over the Main Stage (just past sidebar when open),
        // rendered as a tactile Surface pill with leading ⌕ icon and right [/] hint.
        let search_left = if self.sidebar_open && area.width >= 96 {
            (area.x + 28).max(x + 1)
        } else {
            x + 1
        };
        let search_right = if hits.device.width > 0 {
            dev_x.saturating_sub(2)
        } else {
            gear_x.saturating_sub(2)
        };
        let search_w = search_right.saturating_sub(search_left).min(52);
        if search_w >= 16 {
            let srect = Rect::new(search_left, area.y, search_w, 1);
            hits.search = srect;
            let focused = self.focus == Focus::SearchBar;
            let bg = design.color(ColorRole::Surface);
            let fill_st = Style::default().bg(bg);
            for col in srect.x..srect.right() {
                if let Some(cell) = buf.cell_mut((col, srect.y)) {
                    cell.set_symbol(" ").set_style(fill_st);
                }
            }
            let icon_st = Style::default()
                .fg(design.color(if focused {
                    ColorRole::Accent
                } else {
                    ColorRole::AccentAlt
                }))
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let _ = put(buf, srect.x, srect.y, " ⌕ ", 3, icon_st);

            let badge = if focused { " Esc " } else { " / " };
            let badge_w = badge.len() as u16;
            let input_x = srect.x + 3;
            let input_w = srect.width.saturating_sub(3 + badge_w);
            if input_w >= 6 {
                self.search_input.render_field(
                    Rect::new(input_x, srect.y, input_w, 1),
                    buf,
                    design,
                    "Search tracks & playlists…",
                    focused,
                    true,
                    ColorRole::Surface,
                    TextStyle::Body,
                );
            }
            let badge_st = Style::default()
                .fg(design.color(if focused {
                    ColorRole::Accent
                } else {
                    ColorRole::Muted
                }))
                .bg(bg);
            let _ = put(
                buf,
                srect.right().saturating_sub(badge_w),
                srect.y,
                badge,
                badge_w,
                badge_st,
            );
        }
    }

    fn draw_cassette_deck(
        &mut self,
        buf: &mut Buffer,
        hero_rect: Rect,
        design: &Design,
        hits: &mut HitRects,
    ) {
        hits.deck_area = hero_rect;
        let beat_idx = ((self.analyzer.beat_intensity * 7.0).round() as usize).min(7);
        let border = if design.reduced_motion() {
            design.color(ColorRole::Border)
        } else {
            self.border_ramp[beat_idx]
        };
        let deck_title = format!(
            "HAYATO // CASSETTE DECK · {}",
            self.config.visualizer.label().to_uppercase()
        );
        let inner = draw_beat_card(buf, hero_rect, design, &deck_title, border);
        let (title, artist_album) = match self.current_track() {
            Some(t) if !t.album.is_empty() => {
                (t.title.clone(), format!("{} · {}", t.artist, t.album))
            }
            Some(t) => (t.title.clone(), t.artist.clone()),
            None => (
                "CASSETTE".to_string(),
                "Pick a playlist on the left or press / to search".to_string(),
            ),
        };
        if inner.height < 5 || inner.width < 26 {
            return;
        }

        let play_icon = if self.current_play_state() == PlayState::Playing {
            "[ ⏸ ]"
        } else {
            "[ ▶ ]"
        };
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let dim = design.style(TextStyle::Dim);
        let shuf_st = if self.queue.shuffle {
            Style::default().fg(design.color(ColorRole::Accent))
        } else {
            dim
        };
        let rep_st = if self.queue.repeat != RepeatMode::Off {
            Style::default().fg(design.color(ColorRole::Accent))
        } else {
            dim
        };

        // Left column (24 cols): Album Art + Transport Controls directly underneath the song image
        let art_w = if inner.width >= 60 { 24 } else { 0 };
        let ctrl_y = inner.bottom().saturating_sub(1);
        if art_w > 0 {
            let art_h = inner.height.saturating_sub(1);
            if art_h >= 4 {
                let art_rect = Rect::new(inner.x, inner.y, art_w, art_h);
                if self.art.has_image() {
                    self.art.render(art_rect, buf);
                } else {
                    draw_cassette_ascii(buf, art_rect, design);
                }
            }
            let cx = inner.x;
            let _ = put(buf, cx, ctrl_y, "⇄", 1, shuf_st);
            hits.btn_shuffle = Rect::new(cx, ctrl_y, 2, 1);
            let _ = put(buf, cx + 2, ctrl_y, "[⏮]", 3, dim);
            hits.btn_prev = Rect::new(cx + 2, ctrl_y, 3, 1);
            let _ = put(buf, cx + 6, ctrl_y, play_icon, 5, accent);
            hits.btn_play = Rect::new(cx + 6, ctrl_y, 5, 1);
            let _ = put(buf, cx + 12, ctrl_y, "[⏭]", 3, dim);
            hits.btn_next = Rect::new(cx + 12, ctrl_y, 3, 1);
            let _ = put(
                buf,
                cx + 16,
                ctrl_y,
                self.queue.repeat.short_label(),
                5,
                rep_st,
            );
            hits.btn_repeat = Rect::new(cx + 16, ctrl_y, 5, 1);
        }

        // Right column: Title + Artist/Album + Live Visualizer + Scrubber + Volume & View Toggles
        let rx = inner.x + if art_w > 0 { art_w + 2 } else { 0 };
        let rw = inner.right().saturating_sub(rx);
        if rw < 14 {
            return;
        }

        let scrub_y = ctrl_y.saturating_sub(1);
        let avail_top = scrub_y.saturating_sub(inner.y);
        let bt_h = if avail_top >= 8 && rw >= 44 { 4 } else { 1 };
        if bt_h == 4 {
            let bt_rect = Rect::new(rx, inner.y, rw, 4);
            let big = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(
                    Style::default()
                        .fg(design.color(ColorRole::Text))
                        .add_modifier(Modifier::BOLD),
                )
                .lines(vec![title.into()])
                .build();
            big.render(bt_rect, buf);
        } else {
            let _ = put_trunc(
                buf,
                rx,
                inner.y,
                &title,
                rw,
                Style::default()
                    .fg(design.color(ColorRole::Text))
                    .add_modifier(Modifier::BOLD),
            );
        }
        if inner.y + bt_h < scrub_y {
            let _ = put_trunc(
                buf,
                rx,
                inner.y + bt_h,
                &artist_album,
                rw,
                Style::default().fg(design.color(ColorRole::Text)),
            );
        }
        let viz_y = inner.y + bt_h + 1;
        let viz_h = scrub_y.saturating_sub(viz_y);
        if viz_h >= 1 {
            self.draw_active_visualizer(buf, Rect::new(rx, viz_y, rw, viz_h), design, false);
        }

        if rw >= 14 {
            let srect = Rect::new(rx, scrub_y, rw, 1);
            let label_w = (self.time_buf.len() as u16 + 2).min(srect.width / 2);
            let bar_w = srect.width.saturating_sub(label_w).max(1);
            hits.scrubber = Rect::new(srect.x, srect.y, bar_w, 1);
            draw_scrubber(
                buf,
                srect,
                design,
                self.last_pos_sec,
                self.last_dur_sec,
                &self.time_buf,
            );
        }

        let mut ux = rx;
        if art_w == 0 && rw >= 30 {
            let _ = put(buf, ux, ctrl_y, "[⏮]", 3, dim);
            hits.btn_prev = Rect::new(ux, ctrl_y, 3, 1);
            let _ = put(buf, ux + 4, ctrl_y, play_icon, 5, accent);
            hits.btn_play = Rect::new(ux + 4, ctrl_y, 5, 1);
            let _ = put(buf, ux + 10, ctrl_y, "[⏭]", 3, dim);
            hits.btn_next = Rect::new(ux + 10, ctrl_y, 3, 1);
            ux += 15;
        }
        // Clickable Volume Controls: [-] VOL 80% [+]
        let vol_st = Style::default().fg(design.color(ColorRole::AccentAlt));
        let vol_w = self.vol_buf.len() as u16;
        if ux + 8 + vol_w <= inner.right() {
            let _ = put(buf, ux, ctrl_y, "[-]", 3, dim);
            hits.btn_vol_down = Rect::new(ux, ctrl_y, 3, 1);
            ux += 4;
            let _ = put_trunc(buf, ux, ctrl_y, &self.vol_buf, vol_w, vol_st);
            hits.btn_vol_mute = Rect::new(ux, ctrl_y, vol_w, 1);
            ux += vol_w + 1;
            let _ = put(buf, ux, ctrl_y, "[+]", 3, dim);
            hits.btn_vol_up = Rect::new(ux, ctrl_y, 3, 1);
            ux += 5;
        } else {
            let w = vol_w.min(inner.right().saturating_sub(ux));
            let _ = put_trunc(buf, ux, ctrl_y, &self.vol_buf, w, vol_st);
            hits.btn_vol_mute = Rect::new(ux, ctrl_y, w, 1);
            ux += w + 2;
        }

        if ux + 11 <= inner.right() {
            let _ = put(
                buf,
                ux,
                ctrl_y,
                "[V]",
                3,
                if self.stage == MainStage::Visualizer {
                    accent
                } else {
                    dim
                },
            );
            hits.btn_viz = Rect::new(ux, ctrl_y, 3, 1);
            let _ = put(
                buf,
                ux + 4,
                ctrl_y,
                "[L]",
                3,
                if self.drawer == RightDrawer::Lyrics {
                    accent
                } else {
                    dim
                },
            );
            hits.btn_lyrics = Rect::new(ux + 4, ctrl_y, 3, 1);
            let _ = put(buf, ux + 8, ctrl_y, "[w]", 3, dim);
            hits.btn_mini = Rect::new(ux + 8, ctrl_y, 3, 1);
            ux += 13;
        }
        if ux + 8 < inner.right() {
            let _ = put_trunc(
                buf,
                ux,
                ctrl_y,
                &self.status_line,
                inner.right().saturating_sub(ux),
                dim,
            );
        }
    }

    fn draw_collection_stage(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        design: &Design,
        hits: &mut HitRects,
    ) {
        let inner = draw_card(
            buf,
            area,
            design,
            Some(&self.collection_title_buf),
            self.focus == Focus::Main,
        );
        if inner.height < 3 {
            return;
        }
        let _ = put_trunc(
            buf,
            inner.x,
            inner.y,
            "[▶ Play all (P)]   [Click/Enter] Play track   [a] Enqueue   [Esc] Hide",
            inner.width,
            Style::default().fg(design.color(ColorRole::Accent)),
        );
        hits.play_all = Rect::new(inner.x, inner.y, 16.min(inner.width), 1);

        if let Some((_, reason)) = &self.active_collection_restricted {
            let _ = put_trunc(
                buf,
                inner.x,
                inner.y + 2,
                reason,
                inner.width,
                Style::default().fg(design.color(ColorRole::Warn)),
            );
            let _ = put_trunc(
                buf,
                inner.x,
                inner.y + 4,
                "Press [P] or [Enter] to start this playlist directly on Spotify Connect.",
                inner.width,
                design.style(TextStyle::Dim),
            );
            return;
        }

        let list_rect = Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        );
        self.rows_height.set(list_rect.height);
        hits.main_list = list_rect;
        self.collection_list
            .render(list_rect, buf, design, self.focus == Focus::Main);
    }

    fn draw_visualizer_stage(&mut self, buf: &mut Buffer, area: Rect, design: &Design) {
        let beat_idx = ((self.analyzer.beat_intensity * 7.0).round() as usize).min(7);
        let border = if design.reduced_motion() {
            design.color(ColorRole::Border)
        } else {
            self.border_ramp[beat_idx]
        };
        let title_str = format!(
            "CYBERDECK // {}  [v: next mode]",
            self.config.visualizer.label().to_uppercase()
        );
        let inner = draw_beat_card(buf, area, design, &title_str, border);
        if inner.height < 8 || inner.width < 24 {
            return;
        }
        let art_w = if inner.width >= 60 { 24 } else { 0 };
        let header_h = if inner.height >= 16 {
            8
        } else if inner.height >= 12 {
            6
        } else {
            2
        };
        if art_w > 0 && header_h >= 6 {
            let art_rect = Rect::new(inner.x, inner.y, art_w, header_h);
            if self.art.has_image() {
                self.art.render(art_rect, buf);
            } else {
                draw_cassette_ascii(buf, art_rect, design);
            }
        }
        let rx = inner.x
            + if art_w > 0 && header_h >= 6 {
                art_w + 2
            } else {
                0
            };
        let rw = inner.right().saturating_sub(rx);
        let (title, artist) = match self.current_track() {
            Some(t) => (t.title.clone(), format!("{} · {}", t.artist, t.album)),
            None => ("HAYATO CASSETTE".to_string(), "Idle".to_string()),
        };
        if header_h >= 6 && rw >= 40 {
            let big = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(
                    Style::default()
                        .fg(design.color(ColorRole::Text))
                        .add_modifier(Modifier::BOLD),
                )
                .lines(vec![title.into()])
                .build();
            big.render(Rect::new(rx, inner.y, rw, 4), buf);
            let _ = put_trunc(
                buf,
                rx,
                inner.y + 4,
                &artist,
                rw,
                design.style(TextStyle::Dim),
            );
        } else {
            let _ = put_trunc(
                buf,
                rx,
                inner.y,
                &title,
                rw,
                Style::default()
                    .fg(design.color(ColorRole::Text))
                    .add_modifier(Modifier::BOLD),
            );
            let _ = put_trunc(
                buf,
                rx,
                inner.y + 1,
                &artist,
                rw,
                design.style(TextStyle::Dim),
            );
        }
        let viz_y = inner.y + header_h + 1;
        let viz_h = inner.bottom().saturating_sub(viz_y);
        if viz_h >= 2 {
            let vrect = Rect::new(inner.x, viz_y, inner.width, viz_h);
            self.draw_active_visualizer(buf, vrect, design, true);
        }
    }

    fn draw_right_drawer(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        design: &Design,
        hits: &mut HitRects,
    ) {
        let focused = self.focus == Focus::RightDrawer;
        match self.drawer {
            RightDrawer::Hidden => {}
            RightDrawer::Queue => {
                let inner = draw_card(buf, area, design, Some(&self.queue_title_buf), focused);
                hits.drawer_list = inner;
                if focused {
                    self.rows_height.set(inner.height);
                }
                self.queue_list.render(inner, buf, design, focused);
            }
            RightDrawer::Lyrics => {
                let title = if self.lyrics.as_ref().is_some_and(|l| l.synced) {
                    "LYRICS // SYNCED"
                } else {
                    "LYRICS"
                };
                let inner = draw_card(buf, area, design, Some(title), focused);
                let Some(lyr) = &self.lyrics else {
                    let msg = if self.lyrics_loading {
                        "Fetching lyrics…"
                    } else if self.current_track().is_none() {
                        "Play a track to load lyrics"
                    } else {
                        "No lyrics found for this track"
                    };
                    let _ = put_trunc(
                        buf,
                        inner.x,
                        inner.y + inner.height / 2,
                        msg,
                        inner.width,
                        design.style(TextStyle::Dim),
                    );
                    return;
                };
                if lyr.synced {
                    let active = lyr
                        .active_index(self.last_pos_sec.saturating_mul(1000))
                        .unwrap_or(0);
                    let start = active.saturating_sub((inner.height / 2) as usize);
                    for (row, line) in lyr
                        .lines
                        .iter()
                        .skip(start)
                        .take(inner.height as usize)
                        .enumerate()
                    {
                        let st = if start + row == active {
                            Style::default()
                                .fg(design.color(ColorRole::Accent))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            design.style(TextStyle::Dim)
                        };
                        let _ = put_trunc(
                            buf,
                            inner.x,
                            inner.y + row as u16,
                            &line.text,
                            inner.width,
                            st,
                        );
                    }
                } else {
                    for (row, line) in lyr.lines.iter().take(inner.height as usize).enumerate() {
                        let _ = put_trunc(
                            buf,
                            inner.x,
                            inner.y + row as u16,
                            &line.text,
                            inner.width,
                            design.style(TextStyle::Body),
                        );
                    }
                }
            }
        }
    }

    fn draw_active_visualizer(
        &self,
        buf: &mut Buffer,
        area: Rect,
        design: &Design,
        show_off_hint: bool,
    ) {
        match self.config.visualizer {
            VisualizerMode::Spectrum => self.draw_spectrum_bars(buf, area, design),
            VisualizerMode::Mirror => self.draw_mirror_eq(buf, area, design),
            VisualizerMode::Oscilloscope => self.draw_oscilloscope(buf, area, design),
            VisualizerMode::VuDeck => self.draw_vu_deck(buf, area, design),
            VisualizerMode::Plasma => self.draw_cyber_plasma(buf, area, design),
            VisualizerMode::Off => {
                if show_off_hint && area.height > 0 {
                    let _ = put_trunc(
                        buf,
                        area.x,
                        area.y + area.height / 2,
                        "Visualizer off — press v to cycle visualizers",
                        area.width,
                        design.style(TextStyle::Dim),
                    );
                }
            }
        }
    }

    fn draw_spectrum_bars(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        let h = area.height as usize;
        let step = if area.width as usize >= BANDS * 2 {
            2
        } else {
            1
        };
        let count = ((area.width as usize) / step).min(BANDS);
        for b in 0..count {
            let x = area.x + (b * step) as u16;
            let val = self.analyzer.bands[b];
            let peak = self.analyzer.peaks[b];
            let total_eighths = (val * (h * 8) as f32).round() as usize;
            let peak_row = ((1.0 - peak) * h.saturating_sub(1) as f32).round() as usize;
            for row in 0..h {
                let y = area.y + row as u16;
                let from_bottom = h - 1 - row;
                let cell_eighths = total_eighths.saturating_sub(from_bottom * 8).min(8);
                if cell_eighths > 0 {
                    let ramp_idx = (from_bottom * 5 / h.max(1)).min(5);
                    let _ = put(
                        buf,
                        x,
                        y,
                        BLOCKS[cell_eighths],
                        1,
                        Style::default().fg(self.spectrum_ramp[ramp_idx]),
                    );
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

    fn draw_mirror_eq(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let mid = area.y + area.height / 2;
        let half_h = (area.height / 2).max(1) as usize;
        let step = if area.width as usize >= BANDS * 2 {
            2
        } else {
            1
        };
        let count = ((area.width as usize) / step).min(BANDS);
        let axis_st = design.style(TextStyle::Dim);
        for col in 0..area.width {
            let _ = put(buf, area.x + col, mid, "─", 1, axis_st);
        }
        for b in 0..count {
            let x = area.x + (b * step) as u16;
            let val = self.analyzer.bands[b];
            let rows = ((val * half_h as f32).round() as usize).min(half_h);
            for r in 0..rows {
                let ramp_idx = (r * 5 / half_h.max(1)).min(5);
                let st = Style::default().fg(self.spectrum_ramp[ramp_idx]);
                let glyph = if r + 1 == rows { "▀" } else { "█" };
                let up_y = mid.saturating_sub(r as u16);
                if up_y >= area.y {
                    let _ = put(buf, x, up_y, glyph, 1, st);
                }
                let dn_y = mid + r as u16;
                if dn_y < area.bottom() {
                    let dn_glyph = if r + 1 == rows { "▄" } else { "█" };
                    let _ = put(buf, x, dn_y, dn_glyph, 1, st);
                }
            }
        }
    }

    fn draw_oscilloscope(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let mid = area.y + area.height / 2;
        let half_h = (area.height / 2).max(1) as f32;
        let dim = design.style(TextStyle::Dim);
        for col in 0..area.width {
            let _ = put(buf, area.x + col, mid, "·", 1, dim);
        }
        let mut prev_y: Option<u16> = None;
        for col in 0..area.width {
            let idx = (col as usize * WAVE_POINTS) / (area.width as usize).max(1);
            let sample = self.analyzer.wave[idx.min(WAVE_POINTS - 1)];
            let dy = (sample * half_h).round() as i32;
            let y = (mid as i32 - dy).clamp(area.y as i32, area.bottom().saturating_sub(1) as i32)
                as u16;
            let dist =
                ((mid as i32 - y as i32).unsigned_abs() as usize * 5) / (half_h as usize).max(1);
            let st = Style::default().fg(self.spectrum_ramp[dist.min(5)]);
            if let Some(py) = prev_y {
                let (lo, hi) = if py < y { (py, y) } else { (y, py) };
                for yy in lo..=hi {
                    let ch = if yy == y { "●" } else { "│" };
                    let _ = put(buf, area.x + col, yy, ch, 1, st);
                }
            } else {
                let _ = put(buf, area.x + col, y, "●", 1, st);
            }
            prev_y = Some(y);
        }
    }

    fn draw_vu_deck(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.width < 12 || area.height < 2 {
            return;
        }
        let avg = |range: std::ops::Range<usize>| -> f32 {
            let len = (range.end - range.start).max(1) as f32;
            self.analyzer.bands[range].iter().sum::<f32>() / len
        };
        let bass = avg(0..6).clamp(0.0, 1.0);
        let low_mid = avg(6..14).clamp(0.0, 1.0);
        let hi_mid = avg(14..24).clamp(0.0, 1.0);
        let treble = avg(24..BANDS).clamp(0.0, 1.0);
        let pulse = self.analyzer.beat_intensity.clamp(0.0, 1.0);

        let meters: [(&str, f32); 5] = [
            ("SUB ", bass),
            ("LOW ", low_mid),
            ("MID ", hi_mid),
            ("AIR ", treble),
            ("BEAT", pulse),
        ];
        let bar_w = area.width.saturating_sub(10) as usize;
        let label_st = Style::default()
            .fg(design.color(ColorRole::Text))
            .add_modifier(Modifier::BOLD);
        let dim = design.style(TextStyle::Dim);
        let rows_to_draw = meters.len().min(area.height as usize);
        let start_y = area.y + area.height.saturating_sub(rows_to_draw as u16) / 2;

        for (i, (name, level)) in meters.iter().take(rows_to_draw).enumerate() {
            let y = start_y + i as u16;
            let _ = put(buf, area.x, y, name, 4, label_st);
            let filled = ((*level * bar_w as f32).round() as usize).min(bar_w);
            for c in 0..bar_w {
                let x = area.x + 5 + c as u16;
                if c < filled {
                    let ramp_idx = (c * 5 / bar_w.max(1)).min(5);
                    let ch = if c % 2 == 0 { "█" } else { "▓" };
                    let _ = put(
                        buf,
                        x,
                        y,
                        ch,
                        1,
                        Style::default().fg(self.spectrum_ramp[ramp_idx]),
                    );
                } else {
                    let _ = put(buf, x, y, "░", 1, dim);
                }
            }
            let pct = format!("{:>3}%", (*level * 100.0).round() as u8);
            let _ = put(buf, area.x + 6 + bar_w as u16, y, &pct, 4, dim);
        }
    }

    fn draw_cyber_plasma(&self, buf: &mut Buffer, area: Rect, _design: &Design) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        const SHADES: &[&str] = &[" ", "·", "░", "▒", "▓", "█"];
        let phase = self.remote_phase;
        let beat = self.analyzer.beat_intensity;
        let w = area.width as usize;
        let h = area.height as usize;

        for row in 0..h {
            let ry = row as f32 / h.max(1) as f32;
            for col in 0..w {
                let band_idx = (col * BANDS) / w.max(1);
                let energy = self.analyzer.bands[band_idx.min(BANDS - 1)];
                let rx = col as f32 / w.max(1) as f32;
                let wave = ((rx * 7.0 + phase).sin() * 0.5 + 0.5)
                    * ((ry * 5.0 - phase * 0.7).cos() * 0.5 + 0.5);
                let intensity =
                    (energy * 0.75 + wave * energy * 0.45 + beat * 0.18).clamp(0.0, 1.0);
                let idx = (intensity * 5.0).round() as usize;
                if idx > 0 {
                    let _ = put(
                        buf,
                        area.x + col as u16,
                        area.y + row as u16,
                        SHADES[idx.min(5)],
                        1,
                        Style::default().fg(self.spectrum_ramp[idx.min(5)]),
                    );
                }
            }
        }
    }

    fn draw_mini(&mut self, ctx: &mut BodyCtx, hits: &mut HitRects) {
        let inner = draw_card(
            ctx.buf,
            ctx.area,
            ctx.design,
            Some("CASSETTE // MINI"),
            true,
        );
        if inner.height < 4 || inner.width < 20 {
            return;
        }
        let art_w = if self.art.has_image() && inner.width >= 36 {
            10
        } else {
            0
        };
        if art_w > 0 {
            self.art.render(
                Rect::new(inner.x, inner.y, art_w, inner.height.min(5)),
                ctx.buf,
            );
        }
        let track = self.current_track();
        let title = track.map(|t| t.title.as_str()).unwrap_or("Hayato Cassette");
        let sub = track
            .map(|t| format!("{} · {}", t.artist, t.album))
            .unwrap_or_else(|| "Idle — press w for Full Deck".to_string());
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
        let _ = put_trunc(
            ctx.buf,
            tx,
            inner.y + 1,
            &sub,
            tw,
            ctx.design.style(TextStyle::Dim),
        );
        let _ = put_trunc(
            ctx.buf,
            tx,
            inner.y + 2,
            &self.device_pill_buf,
            tw,
            Style::default().fg(ctx.design.color(ColorRole::Ok)),
        );
        if inner.height >= 6 {
            self.draw_spectrum_bars(
                ctx.buf,
                Rect::new(tx, inner.y + 3, tw.min(BANDS as u16), 1),
                ctx.design,
            );
        }

        let scrub_y = inner.bottom().saturating_sub(2);
        let srect = Rect::new(inner.x, scrub_y, inner.width, 1);
        hits.scrubber = srect;
        draw_scrubber(
            ctx.buf,
            srect,
            ctx.design,
            self.last_pos_sec,
            self.last_dur_sec,
            &self.time_buf,
        );
        let _ = put_trunc(
            ctx.buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "[p] Prev  [Space] Play  [n] Next  [D] Device  [w] Full  [Esc] Hide",
            inner.width,
            ctx.design.style(TextStyle::Dim),
        );
    }

    fn cycle_pane(&mut self, reverse: bool) {
        let mut panes = Vec::new();
        if self.sidebar_open {
            panes.push(Focus::Sidebar);
        }
        panes.push(Focus::Main);
        if self.drawer == RightDrawer::Queue {
            panes.push(Focus::RightDrawer);
        }
        let idx = panes.iter().position(|&f| f == self.focus).unwrap_or(0);
        let next = if reverse {
            if idx == 0 { panes.len() - 1 } else { idx - 1 }
        } else {
            (idx + 1) % panes.len()
        };
        self.focus = panes[next];
        self.sync_chrome();
    }
}

impl Screen for CassetteScreen {
    fn phase(&self) -> Option<&'static str> {
        Some(match &self.phase {
            CassettePhase::Normal => {
                if self.mini {
                    "cassette.mini"
                } else if self.focus == Focus::SearchBar {
                    "cassette.search"
                } else {
                    match self.stage {
                        MainStage::Collection => "cassette.collection",
                        MainStage::Search => "cassette.search",
                        MainStage::Visualizer => "cassette.visualizer",
                        MainStage::Settings => "cassette.settings",
                    }
                }
            }
            CassettePhase::DevicePicker(_) => "cassette.devices",
            CassettePhase::Help => "cassette.help",
            CassettePhase::Palette(_) => "cassette.palette",
            CassettePhase::ConfirmClearQueue(_) => "cassette.confirm_clear",
            CassettePhase::ConfirmLogoutSpotify(_) => "cassette.confirm_logout",
            CassettePhase::EditClientId(_) => "cassette.edit_client_id",
        })
    }

    fn design(&self) -> Option<Design> {
        self.design_override.clone()
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.fx.is_some() || self.pending_search_at.is_some() {
            return Some(Duration::from_millis(33));
        }
        if self.current_play_state() == PlayState::Playing {
            let sleep = !self.focused
                && self
                    .unfocused_since
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(10));
            Some(Duration::from_millis(if sleep { 250 } else { 33 }))
        } else {
            Some(Duration::from_millis(250))
        }
    }

    fn tick(&mut self) -> Outcome {
        if THEME_RELOAD_FLAG.swap(false, Ordering::Relaxed) {
            let d = Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
            self.border_ramp = d.ramp(ColorRole::Border, ColorRole::Accent);
            self.spectrum_ramp = d.ramp(ColorRole::AccentAlt, ColorRole::Accent);
            self.design_override = Some(d);
        }

        self.drain_provider_events();

        // Fire debounced Spotify search (250ms) on background worker
        if let Some(due) = self.pending_search_at
            && Instant::now() >= due
        {
            self.pending_search_at = None;
            let q = self.search_input.text().trim().to_string();
            if q.len() >= 2 {
                self.search_gen += 1;
                self.hub.send(ProviderCmd::Search {
                    seq: self.search_gen,
                    query: q,
                });
            }
        }

        let mpris_out = self.handle_mpris();
        if mpris_out != Outcome::Continue {
            return mpris_out;
        }

        if let Some(err) = self.engine.telemetry.take_error() {
            self.status_line = err;
        }
        let fin = self.engine.telemetry.finished_gen();
        if fin != self.last_finished_gen {
            self.last_finished_gen = fin;
            self.next_track(true);
        }

        // Update position timer (interpolates Spotify Connect smoothly between 2.5s polls)
        let (pos_s, dur_s) = if self.active_provider == ProviderId::Spotify {
            let extra = if self.remote_state == PlayState::Playing {
                self.remote_synced_at.elapsed().as_millis() as u64
            } else {
                0
            };
            let p = self
                .remote_pos_ms
                .saturating_add(extra)
                .min(self.remote_dur_ms);
            (p / 1000, self.remote_dur_ms / 1000)
        } else {
            (
                self.engine.telemetry.position_ms() / 1000,
                self.engine.telemetry.duration_ms() / 1000,
            )
        };
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

        // Focus-gated DSP update
        let sleep_viz = !self.focused
            && self
                .unfocused_since
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(10));
        if !sleep_viz && self.config.visualizer != VisualizerMode::Off {
            let playing = self.current_play_state() == PlayState::Playing;
            let live_spotify_pcm = self.local_spotify_pcm_active.load(Ordering::Relaxed);
            if self.active_provider == ProviderId::Spotify && !live_spotify_pcm {
                self.remote_phase += 0.14;
                self.analyzer.update_synthetic(self.remote_phase, playing);
            } else {
                self.remote_phase += 0.08;
                self.analyzer.update(!playing);
            }
        }

        Outcome::Continue
    }

    fn event(&mut self, event: &Event, design: &Design) -> Outcome {
        match event {
            Event::FocusGained => {
                self.focused = true;
                self.unfocused_since = None;
                self.hub.send(ProviderCmd::SetFocused(true));
                Outcome::Consumed
            }
            Event::FocusLost => {
                self.focused = false;
                self.unfocused_since = Some(Instant::now());
                self.hub.send(ProviderCmd::SetFocused(false));
                Outcome::Consumed
            }
            Event::Mouse(m) => {
                let hits = self.hits.get();
                let in_rect = |r: Rect| {
                    r.width > 0
                        && r.height > 0
                        && m.column >= r.x
                        && m.column < r.right()
                        && m.row >= r.y
                        && m.row < r.bottom()
                };
                match m.kind {
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        let code = if m.kind == MouseEventKind::ScrollUp {
                            KeyCode::Up
                        } else {
                            KeyCode::Down
                        };
                        let synth = KeyEvent::new(code, KeyModifiers::NONE);
                        if !matches!(self.phase, CassettePhase::Normal) {
                            match &mut self.phase {
                                CassettePhase::DevicePicker(list)
                                | CassettePhase::Palette(list) => {
                                    let _ = list.handle(synth, hits.modal_list.height, design);
                                }
                                _ => {}
                            }
                            return Outcome::Consumed;
                        }
                        if in_rect(hits.sidebar_list) {
                            let _ =
                                self.sidebar_list
                                    .handle(synth, hits.sidebar_list.height, design);
                        } else if in_rect(hits.main_list) {
                            match self.stage {
                                MainStage::Collection => {
                                    let _ = self.collection_list.handle(
                                        synth,
                                        hits.main_list.height,
                                        design,
                                    );
                                }
                                MainStage::Search => {
                                    let _ = self.search_list.handle(
                                        synth,
                                        hits.main_list.height,
                                        design,
                                    );
                                }
                                MainStage::Settings => {
                                    let _ = self.settings_list.handle(
                                        synth,
                                        hits.main_list.height,
                                        design,
                                    );
                                }
                                MainStage::Visualizer => {}
                            }
                        } else if in_rect(hits.drawer_list) {
                            let _ = self
                                .queue_list
                                .handle(synth, hits.drawer_list.height, design);
                        } else if in_rect(hits.deck_area) {
                            self.adjust_volume(if m.kind == MouseEventKind::ScrollUp {
                                5
                            } else {
                                -5
                            });
                        }
                        Outcome::Consumed
                    }
                    _ => Outcome::Continue,
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let h = self.hits.get();
        let inside = |r: Rect| {
            r.width > 0 && r.height > 0 && x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
        };

        // Modal phases own the entire screen: never leak hits to background controls.
        match &self.phase {
            CassettePhase::Normal => {}
            CassettePhase::DevicePicker(list) | CassettePhase::Palette(list) => {
                if inside(h.modal_list) {
                    return list
                        .hit_row(h.modal_list, x, y, design)
                        .map(|r| HIT_MODAL_BASE + r);
                }
                return None;
            }
            _ => return None,
        }

        if inside(h.filter_all) {
            return Some(HIT_FILTER_ALL);
        }
        if inside(h.filter_spotify) {
            return Some(HIT_FILTER_SPOTIFY);
        }
        if inside(h.filter_local) {
            return Some(HIT_FILTER_LOCAL);
        }
        if inside(h.search) {
            return Some(HIT_SEARCH_BOX);
        }
        if inside(h.device) {
            return Some(HIT_DEVICE_PILL);
        }
        if inside(h.settings) {
            return Some(HIT_SETTINGS);
        }
        if inside(h.play_all) {
            return Some(HIT_PLAY_ALL);
        }
        if inside(h.btn_shuffle) {
            return Some(HIT_BTN_SHUFFLE);
        }
        if inside(h.btn_prev) {
            return Some(HIT_BTN_PREV);
        }
        if inside(h.btn_play) {
            return Some(HIT_BTN_PLAY);
        }
        if inside(h.btn_next) {
            return Some(HIT_BTN_NEXT);
        }
        if inside(h.btn_repeat) {
            return Some(HIT_BTN_REPEAT);
        }
        if inside(h.btn_vol_down) {
            return Some(HIT_BTN_VOL_DOWN);
        }
        if inside(h.btn_vol_mute) {
            return Some(HIT_BTN_VOL_MUTE);
        }
        if inside(h.btn_vol_up) {
            return Some(HIT_BTN_VOL_UP);
        }
        if inside(h.btn_viz) {
            return Some(HIT_BTN_VIZ);
        }
        if inside(h.btn_lyrics) {
            return Some(HIT_BTN_LYRICS);
        }
        if inside(h.btn_mini) {
            return Some(HIT_BTN_MINI);
        }
        if inside(h.scrubber) {
            let pct = ((x - h.scrubber.x) as usize * 100) / h.scrubber.width.max(1) as usize;
            return Some(HIT_SCRUB_BASE + pct.min(100));
        }
        if inside(h.sidebar_list) {
            return self
                .sidebar_list
                .hit_row(h.sidebar_list, x, y, design)
                .map(|r| HIT_SIDEBAR_BASE + r);
        }
        if inside(h.main_list) {
            let hit_idx = match self.stage {
                MainStage::Collection => self.collection_list.hit_row(h.main_list, x, y, design),
                MainStage::Search => self.search_list.hit_row(h.main_list, x, y, design),
                MainStage::Settings => self.settings_list.hit_row(h.main_list, x, y, design),
                MainStage::Visualizer => None,
            };
            return hit_idx.map(|r| HIT_MAIN_BASE + r);
        }
        if inside(h.drawer_list) {
            return self
                .queue_list
                .hit_row(h.drawer_list, x, y, design)
                .map(|r| HIT_DRAWER_BASE + r);
        }
        None
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(code) = row else {
            return Outcome::Continue;
        };
        if code >= HIT_MODAL_BASE {
            let idx = code - HIT_MODAL_BASE;
            match &self.phase {
                CassettePhase::DevicePicker(_) => self.activate_device_picker_row(idx),
                CassettePhase::Palette(_) => self.activate_palette_row(idx),
                _ => {}
            }
            return Outcome::Consumed;
        }
        if !matches!(self.phase, CassettePhase::Normal) {
            return Outcome::Consumed;
        }
        match code {
            HIT_FILTER_ALL | HIT_FILTER_SPOTIFY | HIT_FILTER_LOCAL => {
                self.source_filter = match code {
                    HIT_FILTER_ALL => SourceFilter::All,
                    HIT_FILTER_SPOTIFY => SourceFilter::Spotify,
                    _ => SourceFilter::Local,
                };
                self.rebuild_sidebar_and_home();
                if self.stage == MainStage::Search {
                    self.rebuild_search_rows();
                } else {
                    self.stage = MainStage::Collection;
                    if let Some(first) = self.filtered_collections.first().cloned() {
                        self.open_collection(first);
                    }
                }
                self.status_line = format!("Filter: {}", self.source_filter.label());
                self.sync_chrome();
            }
            HIT_SEARCH_BOX => {
                if self.stage != MainStage::Search {
                    self.prev_stage = self.stage.clone();
                }
                self.stage = MainStage::Search;
                self.focus = Focus::SearchBar;
                self.sync_chrome();
            }
            HIT_DEVICE_PILL => self.open_device_picker(),
            HIT_SETTINGS => {
                self.stage = MainStage::Settings;
                self.focus = Focus::Main;
                self.sync_chrome();
            }
            HIT_PLAY_ALL => {
                if let Some(c) = self.active_collection.clone() {
                    self.play_collection_now(&c);
                }
            }
            HIT_BTN_SHUFFLE => {
                self.toggle_shuffle();
                self.status_line =
                    format!("Shuffle {}", if self.queue.shuffle { "ON" } else { "OFF" });
            }
            HIT_BTN_PREV => self.prev_track(),
            HIT_BTN_PLAY => self.toggle_play_pause(),
            HIT_BTN_NEXT => self.next_track(false),
            HIT_BTN_REPEAT => {
                self.cycle_repeat();
                self.status_line = format!("Repeat: {}", self.queue.repeat.short_label());
            }
            HIT_BTN_VOL_DOWN => self.adjust_volume(-5),
            HIT_BTN_VOL_MUTE => self.toggle_mute(),
            HIT_BTN_VOL_UP => self.adjust_volume(5),
            HIT_BTN_VIZ => {
                self.stage = if self.stage == MainStage::Visualizer {
                    MainStage::Collection
                } else {
                    MainStage::Visualizer
                };
                self.focus = Focus::Main;
                self.sync_chrome();
            }
            HIT_BTN_LYRICS => {
                self.drawer = if self.drawer == RightDrawer::Lyrics {
                    RightDrawer::Hidden
                } else {
                    if let Some(t) = self.current_track().cloned() {
                        self.request_lyrics_for(&t);
                    }
                    RightDrawer::Lyrics
                };
                self.sync_chrome();
            }
            HIT_BTN_MINI => {
                self.set_mini_mode(!self.mini);
                self.sync_chrome();
            }
            c if (HIT_SCRUB_BASE..HIT_SIDEBAR_BASE).contains(&c) => {
                let pct = (c - HIT_SCRUB_BASE).min(100) as u64;
                self.seek_to_secs(self.last_dur_sec * pct / 100);
            }
            c if (HIT_SIDEBAR_BASE..HIT_MAIN_BASE).contains(&c) => {
                self.focus = Focus::Sidebar;
                let idx = c - HIT_SIDEBAR_BASE;
                self.sidebar_list.set_cursor(idx);
                if let Some(col) = self.filtered_collections.get(idx).cloned() {
                    self.open_collection(col);
                }
            }
            c if (HIT_MAIN_BASE..HIT_DRAWER_BASE).contains(&c) => {
                let idx = c - HIT_MAIN_BASE;
                match self.stage {
                    MainStage::Collection => {
                        self.focus = Focus::Main;
                        self.collection_list.set_cursor(idx);
                        self.activate_collection_row(idx);
                    }
                    MainStage::Search => {
                        self.focus = Focus::SearchBar;
                        self.search_list.set_cursor(idx);
                        self.activate_search_selection();
                    }
                    MainStage::Settings => {
                        self.focus = Focus::Main;
                        self.settings_list.set_cursor(idx);
                        self.activate_settings_row(idx);
                    }
                    MainStage::Visualizer => {}
                }
                self.sync_chrome();
            }
            c if (HIT_DRAWER_BASE..HIT_MODAL_BASE).contains(&c) => {
                self.focus = Focus::RightDrawer;
                let idx = c - HIT_DRAWER_BASE;
                self.queue_list.set_cursor(idx);
                self.activate_queue_row(idx);
                self.sync_chrome();
            }
            _ => return Outcome::Continue,
        }
        Outcome::Consumed
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let mut hits = HitRects::default();

        // Body-replacing modals (AGENTS.md Rule 11) — always update `self.hits` so
        // stale background hit rects never leak through modals.
        if let CassettePhase::ConfirmClearQueue(c) = &mut self.phase {
            c.render(ctx.area, ctx.buf, ctx.design);
            self.hits.set(hits);
            return;
        }
        if let CassettePhase::ConfirmLogoutSpotify(c) = &mut self.phase {
            c.render(ctx.area, ctx.buf, ctx.design);
            self.hits.set(hits);
            return;
        }
        if let CassettePhase::DevicePicker(list) = &mut self.phase {
            let inner = draw_card(
                ctx.buf,
                ctx.area,
                ctx.design,
                Some("SPOTIFY CONNECT // DEVICES"),
                true,
            );
            hits.modal_list = inner;
            self.rows_height.set(inner.height);
            list.render(inner, ctx.buf, ctx.design, true);
            self.hits.set(hits);
            return;
        }
        if let CassettePhase::Help = &self.phase {
            let inner = draw_card(
                ctx.buf,
                ctx.area,
                ctx.design,
                Some("CASSETTE // KEYMAP"),
                true,
            );
            const LINES: &[&str] = &[
                "GLOBAL:  Space play/pause · n/p next/prev · [/] seek ±5s · +/- vol · m mute",
                "         z shuffle · r repeat · R refresh · v viz · V full deck · w mini",
                "         D devices · / search · b sidebar · q queue · L lyrics · 4 settings",
                "         Tab / h / l switch panes · Ctrl+S share · : palette · Esc back/hide · Q quit",
                "",
                "LIBRARY: Click/Enter open playlist · P play whole playlist · f filter (All/Spotify/Local)",
                "TRACKS:  Click/Enter play from track · P play all · a enqueue · A play next · Bksp back",
                "QUEUE:   Click/Enter jump · J/K reorder · d remove · Ctrl+X clear · S save .m3u8",
            ];
            for (i, line) in LINES.iter().enumerate() {
                if inner.y + i as u16 >= inner.bottom() {
                    break;
                }
                let _ = put_trunc(
                    ctx.buf,
                    inner.x,
                    inner.y + i as u16,
                    line,
                    inner.width,
                    ctx.design.style(TextStyle::Body),
                );
            }
            self.hits.set(hits);
            return;
        }
        if let CassettePhase::Palette(list) = &mut self.phase {
            let inner = draw_card(ctx.buf, ctx.area, ctx.design, Some("COMMAND PALETTE"), true);
            hits.modal_list = inner;
            self.rows_height.set(inner.height);
            list.render(inner, ctx.buf, ctx.design, true);
            self.hits.set(hits);
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
                input.render_field(
                    Rect::new(inner.x, inner.y + 2, inner.width, 1),
                    ctx.buf,
                    ctx.design,
                    "e.g. 4f8b9c...",
                    true,
                    true,
                    ColorRole::Bg,
                    TextStyle::Body,
                );
            }
            self.hits.set(hits);
            return;
        }

        if self.mini || ctx.area.width < 70 || ctx.area.height < 14 {
            self.draw_mini(ctx, &mut hits);
            self.hits.set(hits);
            return;
        }

        // 1. Top Chrome Bar (1 row): [All] [Spotify] [Local] · [/ Search] · [Device] · [⚙]
        let top_rect = Rect::new(ctx.area.x, ctx.area.y, ctx.area.width, 1);
        self.draw_top_chrome(ctx.buf, top_rect, ctx.design, &mut hits);

        // 2. Full Workspace below Top Chrome (no duplicate Bottom Player Bar; framework keybar only)
        let mid_y = ctx.area.y + 1;
        let mid_h = ctx.area.bottom().saturating_sub(mid_y);
        let mid_rect = Rect::new(ctx.area.x, mid_y, ctx.area.width, mid_h);

        let show_sidebar = self.sidebar_open && mid_rect.width >= 96;
        let show_drawer = self.drawer != RightDrawer::Hidden
            && mid_rect.width >= 96
            && self.stage != MainStage::Visualizer;

        let sb_w = if show_sidebar { 27u16 } else { 0 };
        let dr_w = if show_drawer { 30u16 } else { 0 };
        let main_x = mid_rect.x + if sb_w > 0 { sb_w + 1 } else { 0 };
        let main_w = mid_rect
            .width
            .saturating_sub(sb_w + dr_w + u16::from(sb_w > 0) + u16::from(dr_w > 0));

        // Compact single-column fallback when RightDrawer (Queue/Lyrics) is open on <96 cols
        if !show_drawer
            && self.drawer != RightDrawer::Hidden
            && (self.focus == Focus::RightDrawer || self.drawer == RightDrawer::Lyrics)
        {
            self.draw_right_drawer(ctx.buf, mid_rect, ctx.design, &mut hits);
            self.hits.set(hits);
            return;
        }

        // Compact single-column fallback when sidebar is focused on <96 cols
        if !show_sidebar && self.focus == Focus::Sidebar {
            let inner = draw_card(
                ctx.buf,
                mid_rect,
                ctx.design,
                Some("YOUR LIBRARY // [f] FILTER"),
                true,
            );
            hits.sidebar_list = inner;
            self.rows_height.set(inner.height);
            self.sidebar_list.render(inner, ctx.buf, ctx.design, true);
            self.hits.set(hits);
            return;
        }

        if show_sidebar {
            let sb_rect = Rect::new(mid_rect.x, mid_rect.y, sb_w, mid_rect.height);
            let sb_inner = draw_card(
                ctx.buf,
                sb_rect,
                ctx.design,
                Some("YOUR LIBRARY"),
                self.focus == Focus::Sidebar,
            );
            if sb_inner.height >= 1 {
                hits.sidebar_list = sb_inner;
                if self.focus == Focus::Sidebar {
                    self.rows_height.set(sb_inner.height);
                }
                self.sidebar_list.render(
                    sb_inner,
                    ctx.buf,
                    ctx.design,
                    self.focus == Focus::Sidebar,
                );
            }
        }

        let main_rect = Rect::new(main_x, mid_rect.y, main_w, mid_rect.height);
        // Keep HAYATO // CASSETTE DECK pinned across Library/Collection and Search
        let show_deck = matches!(self.stage, MainStage::Collection | MainStage::Search);
        let deck_h = if !show_deck {
            0
        } else if main_rect.height >= 22 {
            12
        } else if main_rect.height >= 16 {
            10
        } else {
            0
        };
        if deck_h > 0 {
            let deck_rect = Rect::new(main_rect.x, main_rect.y, main_rect.width, deck_h);
            self.draw_cassette_deck(ctx.buf, deck_rect, ctx.design, &mut hits);
        }
        let stage_rect = Rect::new(
            main_rect.x,
            main_rect.y + deck_h,
            main_rect.width,
            main_rect.height.saturating_sub(deck_h),
        );

        match self.stage {
            MainStage::Collection => {
                self.draw_collection_stage(ctx.buf, stage_rect, ctx.design, &mut hits)
            }
            MainStage::Search => {
                let inner = draw_card(
                    ctx.buf,
                    stage_rect,
                    ctx.design,
                    Some(&self.search_title_buf),
                    self.focus == Focus::SearchBar || self.focus == Focus::Main,
                );
                if inner.height >= 3 {
                    let _ = put_trunc(
                        ctx.buf,
                        inner.x,
                        inner.y,
                        "[Enter] Play   [Ctrl+A] Queue   [Tab] Filter   [Esc] Library",
                        inner.width,
                        ctx.design.style(TextStyle::Dim),
                    );
                    let list_rect = Rect::new(
                        inner.x,
                        inner.y + 2,
                        inner.width,
                        inner.height.saturating_sub(2),
                    );
                    hits.main_list = list_rect;
                    self.rows_height.set(list_rect.height);
                    self.search_list.render(
                        list_rect,
                        ctx.buf,
                        ctx.design,
                        self.focus == Focus::SearchBar || self.focus == Focus::Main,
                    );
                } else {
                    hits.main_list = inner;
                    self.rows_height.set(inner.height);
                    self.search_list.render(
                        inner,
                        ctx.buf,
                        ctx.design,
                        self.focus == Focus::SearchBar || self.focus == Focus::Main,
                    );
                }
            }
            MainStage::Visualizer => self.draw_visualizer_stage(ctx.buf, main_rect, ctx.design),
            MainStage::Settings => {
                let inner = draw_card(
                    ctx.buf,
                    main_rect,
                    ctx.design,
                    Some("SETTINGS // PROVIDERS & AUDIO"),
                    self.focus == Focus::Main,
                );
                hits.main_list = inner;
                self.rows_height.set(inner.height);
                self.settings_list
                    .render(inner, ctx.buf, ctx.design, self.focus == Focus::Main);
            }
        }

        if show_drawer {
            let dr_rect = Rect::new(
                mid_rect.right().saturating_sub(dr_w),
                mid_rect.y,
                dr_w,
                mid_rect.height,
            );
            self.draw_right_drawer(ctx.buf, dr_rect, ctx.design, &mut hits);
        }

        // Bounded tachyonfx on Main Stage only (never over Sixel drawer!)
        if let Some(effect) = self.fx.as_mut() {
            if ctx.design.reduced_motion() {
                self.fx = None;
            } else {
                let dt = self.last_fx_tick.elapsed();
                self.last_fx_tick = Instant::now();
                effect.process(dt.into(), ctx.buf, main_rect);
                if effect.done() {
                    self.fx = None;
                }
            }
        }

        self.hits.set(hits);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        // 1. Active modal phases
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
                    self.sync_chrome();
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::ConfirmLogoutSpotify(c) = &mut self.phase {
            match c.handle(key) {
                ConfirmAction::Chosen(1) => {
                    self.hub.send(ProviderCmd::SpotifyLogout);
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                ConfirmAction::Chosen(_) | ConfirmAction::Cancelled => {
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                ConfirmAction::None => {}
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::Help = &self.phase {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter) {
                self.phase = CassettePhase::Normal;
                self.sync_chrome();
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::EditClientId(input) = &mut self.phase {
            match key.code {
                KeyCode::Esc => {
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                KeyCode::Enter => {
                    self.config.spotify_client_id = input.text().trim().to_string();
                    let _ = save_config(&self.config);
                    self.phase = CassettePhase::Normal;
                    self.rebuild_sidebar_and_home();
                    self.rebuild_settings_rows();
                    self.status_line = "Saved Spotify Client ID".into();
                    self.sync_chrome();
                }
                _ => {
                    let _ = input.handle(key);
                }
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::DevicePicker(list) = &mut self.phase {
            match key.code {
                KeyCode::Esc => {
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                KeyCode::Char('e' | 'E') => {
                    self.hub.send(ProviderCmd::EnableLocalConnect);
                    self.status_line = "Opening browser to enable This Device audio…".into();
                    self.phase = CassettePhase::Normal;
                    self.sync_chrome();
                }
                KeyCode::Char('R') => {
                    self.hub.send(ProviderCmd::RefreshDevices);
                    self.status_line = "Refreshing Spotify Connect devices…".into();
                }
                _ => {
                    if let ListAction::Activated(idx) =
                        list.handle(key, self.rows_height.get(), design)
                    {
                        self.activate_device_picker_row(idx);
                    }
                }
            }
            return Outcome::Consumed;
        }
        if let CassettePhase::Palette(list) = &mut self.phase {
            if key.code == KeyCode::Esc {
                self.phase = CassettePhase::Normal;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            if let ListAction::Activated(idx) = list.handle(key, self.rows_height.get(), design) {
                if idx == 13 {
                    self.engine.stop();
                    NowPlayingFile::clear();
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                self.activate_palette_row(idx);
            }
            return Outcome::Consumed;
        }

        // 2. Top Search Box focused
        if self.focus == Focus::SearchBar {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Esc => {
                    if !self.search_input.text().is_empty() {
                        self.search_input.set("");
                        self.spotify_search_cache = SearchResults::default();
                        self.rebuild_search_rows();
                    } else {
                        self.focus = Focus::Main;
                        self.stage = self.prev_stage.clone();
                        self.sync_chrome();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Tab => {
                    self.source_filter = self.source_filter.cycle();
                    self.rebuild_search_rows();
                    return Outcome::Consumed;
                }
                KeyCode::Up | KeyCode::Down => {
                    let _ = self.search_list.handle(key, self.rows_height.get(), design);
                    return Outcome::Consumed;
                }
                KeyCode::Char('a') if ctrl => {
                    if let Some(item) = self.selected_search_item()
                        && let SearchRowItem::Track(t) = item
                    {
                        self.enqueue_track(t, false);
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Enter => {
                    self.activate_search_selection();
                    return Outcome::Consumed;
                }
                _ => {
                    if matches!(self.search_input.handle(key), InputAction::Changed) {
                        self.stage = MainStage::Search;
                        if self.search_input.text().trim().len() < 2 {
                            self.spotify_search_cache = SearchResults::default();
                            self.pending_search_at = None;
                        } else {
                            self.pending_search_at =
                                Some(Instant::now() + Duration::from_millis(250));
                        }
                        self.rebuild_search_rows();
                        return Outcome::Consumed;
                    }
                    return Outcome::Consumed;
                }
            }
        }

        // 3. Global Ctrl shortcuts
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            match key.code {
                KeyCode::Char('s') => {
                    self.share_current();
                    return Outcome::Consumed;
                }
                KeyCode::Char('k') => {
                    if self.stage != MainStage::Search {
                        self.prev_stage = self.stage.clone();
                    }
                    self.stage = MainStage::Search;
                    self.focus = Focus::SearchBar;
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Char('p') => {
                    self.open_palette();
                    return Outcome::Consumed;
                }
                KeyCode::Char('x')
                    if self.focus == Focus::RightDrawer && !self.queue.is_empty() =>
                {
                    let confirm = Confirm::new(
                        "Clear Queue",
                        format!("Clear all {} queued tracks?", self.queue.len()),
                    )
                    .with_options(&["Cancel", "Clear"])
                    .destructive(true);
                    self.phase = CassettePhase::ConfirmClearQueue(confirm);
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        // 4. Global single-key zero-conflict bindings
        match key.code {
            KeyCode::Char('Q') => {
                self.engine.stop();
                NowPlayingFile::clear();
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
            KeyCode::Esc => {
                if self.focus == Focus::RightDrawer {
                    self.drawer = RightDrawer::Hidden;
                    self.focus = Focus::Main;
                    self.sync_chrome();
                } else if self.focus == Focus::Sidebar {
                    self.focus = Focus::Main;
                    self.sync_chrome();
                } else if self.stage != MainStage::Collection {
                    self.stage = MainStage::Collection;
                    self.focus = Focus::Main;
                    self.sync_chrome();
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
            KeyCode::Char('[') => {
                self.seek_delta(-5);
                return Outcome::Consumed;
            }
            KeyCode::Char(']') => {
                self.seek_delta(5);
                return Outcome::Consumed;
            }
            KeyCode::Char('{') => {
                self.seek_delta(-30);
                return Outcome::Consumed;
            }
            KeyCode::Char('}') => {
                self.seek_delta(30);
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
            KeyCode::Char('m') => {
                self.toggle_mute();
                return Outcome::Consumed;
            }
            KeyCode::Char('z') => {
                self.toggle_shuffle();
                return Outcome::Consumed;
            }
            KeyCode::Char('r') => {
                self.cycle_repeat();
                return Outcome::Consumed;
            }
            KeyCode::Char('R') | KeyCode::F(5) => {
                self.hub.send(ProviderCmd::RefreshAll {
                    music_dirs: self.config.music_dirs.clone(),
                });
                self.status_line = "Refreshing library…".into();
                return Outcome::Consumed;
            }
            KeyCode::Char('v') => {
                self.config.visualizer = self.config.visualizer.cycle();
                let _ = save_config(&self.config);
                self.rebuild_settings_rows();
                return Outcome::Consumed;
            }
            KeyCode::Char('V') => {
                self.stage = if self.stage == MainStage::Visualizer {
                    MainStage::Collection
                } else {
                    MainStage::Visualizer
                };
                self.focus = Focus::Main;
                self.fx = Some(fx::sweep_in(
                    Motion::LeftToRight,
                    6,
                    0,
                    design.color(ColorRole::Bg),
                    EffectTimer::from_ms(140, Interpolation::QuadOut),
                ));
                self.last_fx_tick = Instant::now();
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('w') => {
                self.set_mini_mode(!self.mini);
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('D') => {
                self.open_device_picker();
                return Outcome::Consumed;
            }
            KeyCode::Char('/') => {
                if self.stage != MainStage::Search {
                    self.prev_stage = self.stage.clone();
                }
                self.stage = MainStage::Search;
                self.focus = Focus::SearchBar;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('1') => {
                self.stage = MainStage::Collection;
                self.focus = Focus::Main;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('2') => {
                self.sidebar_open = true;
                self.focus = Focus::Sidebar;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('3') | KeyCode::Char('q') => {
                if self.drawer == RightDrawer::Queue {
                    self.drawer = RightDrawer::Hidden;
                    if self.focus == Focus::RightDrawer {
                        self.focus = Focus::Main;
                    }
                } else {
                    self.drawer = RightDrawer::Queue;
                    self.focus = Focus::RightDrawer;
                }
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('4') | KeyCode::Char(',') => {
                self.stage = MainStage::Settings;
                self.focus = Focus::Main;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('b') => {
                self.sidebar_open = !self.sidebar_open;
                if !self.sidebar_open && self.focus == Focus::Sidebar {
                    self.focus = Focus::Main;
                }
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('i') => {
                self.stage = MainStage::Collection;
                self.focus = Focus::Main;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Char('L') => {
                if self.drawer == RightDrawer::Lyrics {
                    self.drawer = RightDrawer::Hidden;
                    if self.focus == Focus::RightDrawer {
                        self.focus = Focus::Main;
                    }
                } else {
                    if let Some(t) = self.current_track().cloned() {
                        self.request_lyrics_for(&t);
                    }
                    self.drawer = RightDrawer::Lyrics;
                }
                self.sync_chrome();
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                self.cycle_pane(false);
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.cycle_pane(true);
                return Outcome::Consumed;
            }
            KeyCode::Char(':') => {
                self.open_palette();
                return Outcome::Consumed;
            }
            KeyCode::Char('?') => {
                self.phase = CassettePhase::Help;
                self.sync_chrome();
                return Outcome::Consumed;
            }
            _ => {}
        }

        // 5. Focused pane context keys
        match self.focus {
            Focus::Sidebar => match key.code {
                KeyCode::Char('l') | KeyCode::Right => {
                    self.focus = Focus::Main;
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
                KeyCode::Char('f') => {
                    self.source_filter = self.source_filter.cycle();
                    self.rebuild_sidebar_and_home();
                    return Outcome::Consumed;
                }
                KeyCode::Char('P') => {
                    if let Some(i) = self.sidebar_list.selected_index()
                        && let Some(c) = self.filtered_collections.get(i).cloned()
                    {
                        self.play_collection_now(&c);
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    if let ListAction::Activated(i) =
                        self.sidebar_list
                            .handle(key, self.rows_height.get(), design)
                        && let Some(c) = self.filtered_collections.get(i).cloned()
                    {
                        self.open_collection(c);
                    }
                    return Outcome::Consumed;
                }
            },
            Focus::RightDrawer => match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    self.focus = Focus::Main;
                    self.sync_chrome();
                    return Outcome::Consumed;
                }
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
                KeyCode::Char('S') if !self.queue.is_empty() => {
                    match save_m3u8_playlist(
                        &self.config.music_dirs,
                        "Cassette Mix",
                        self.queue.items(),
                    ) {
                        Ok(p) => {
                            self.status_line = format!("Saved {}", p.display());
                            self.hub.send(ProviderCmd::RefreshAll {
                                music_dirs: self.config.music_dirs.clone(),
                            });
                        }
                        Err(e) => self.status_line = e,
                    }
                    return Outcome::Consumed;
                }
                _ => {
                    if let ListAction::Activated(i) =
                        self.queue_list.handle(key, self.rows_height.get(), design)
                    {
                        self.activate_queue_row(i);
                    }
                    return Outcome::Consumed;
                }
            },
            Focus::Main => match self.stage {
                MainStage::Collection => match key.code {
                    KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => {
                        self.sidebar_open = true;
                        self.focus = Focus::Sidebar;
                        self.sync_chrome();
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        if self.drawer == RightDrawer::Queue {
                            self.focus = Focus::RightDrawer;
                            self.sync_chrome();
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('P') => {
                        if let Some(c) = self.active_collection.clone() {
                            self.play_collection_now(&c);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Enter if self.active_collection_restricted.is_some() => {
                        if let Some(c) = self.active_collection.clone() {
                            self.play_collection_now(&c);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('a') => {
                        if let Some(i) = self.collection_list.selected_index()
                            && let Some(t) = self.active_collection_tracks.get(i).cloned()
                        {
                            self.enqueue_track(t, false);
                        }
                        return Outcome::Consumed;
                    }
                    KeyCode::Char('A') => {
                        if let Some(i) = self.collection_list.selected_index()
                            && let Some(t) = self.active_collection_tracks.get(i).cloned()
                        {
                            self.enqueue_track(t, true);
                        }
                        return Outcome::Consumed;
                    }
                    _ => {
                        if let ListAction::Activated(i) =
                            self.collection_list
                                .handle(key, self.rows_height.get(), design)
                        {
                            self.activate_collection_row(i);
                        }
                        return Outcome::Consumed;
                    }
                },
                MainStage::Visualizer => match key.code {
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
                    _ => {}
                },
                MainStage::Settings => {
                    if let ListAction::Activated(idx) =
                        self.settings_list
                            .handle(key, self.rows_height.get(), design)
                    {
                        self.activate_settings_row(idx);
                    }
                    return Outcome::Consumed;
                }
                MainStage::Search => {
                    self.focus = Focus::SearchBar;
                    if key.code == KeyCode::Enter {
                        self.activate_search_selection();
                    }
                    return Outcome::Consumed;
                }
            },
            Focus::SearchBar => {}
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

impl CassetteScreen {
    /// Map the selected row in `search_list` (which contains section headings) back to `search_items`.
    fn selected_search_item(&self) -> Option<SearchRowItem> {
        let row_idx = self.search_list.selected_index()?;
        let mut item_idx = 0usize;
        for (i, row) in self.search_list.rows().iter().enumerate() {
            if row.heading {
                continue;
            }
            if i == row_idx {
                return self.search_items.get(item_idx).cloned();
            }
            item_idx += 1;
        }
        None
    }
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
        .style(Style::default().bg(design.color(ColorRole::Surface)));
    let inner = block.inner(area);
    block.render(area, buf);
    let _ = put_trunc(
        buf,
        area.x + 2,
        area.y,
        title,
        area.width.saturating_sub(4),
        Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_this_device_over_active_external_speaker() {
        let design = Design::solarized();
        let mut screen = CassetteScreen::for_test(Vec::new(), false, &design);
        screen.devices = vec![
            PlaybackDevice {
                id: "echo-1".into(),
                provider: ProviderId::Spotify,
                name: "Living Room Echo".into(),
                kind: "Speaker".into(),
                is_active: true,
                volume_pct: 60,
            },
            PlaybackDevice {
                id: "local-1".into(),
                provider: ProviderId::Spotify,
                name: "Hayato Cassette".into(),
                kind: "Chromebook".into(),
                is_active: false,
                volume_pct: 80,
            },
        ];
        // Defaults to This Device even when Echo is active
        assert_eq!(screen.active_device_id().as_deref(), Some("local-1"));
        screen.rebuild_device_pill();
        assert_eq!(screen.device_pill_buf, "● This Device ▾");

        // Explicit user pick overrides default
        screen.preferred_device_id = Some("echo-1".into());
        assert_eq!(screen.active_device_id().as_deref(), Some("echo-1"));
        screen.rebuild_device_pill();
        assert!(screen.device_pill_buf.contains("Living Room Echo"));
    }
}
