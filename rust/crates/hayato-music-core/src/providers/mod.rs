//! Modular music provider abstraction (`MusicProvider`) and non-blocking
//! background worker (`ProviderHub`).
//!
//! Adding a new streaming or local backend (Navidrome/Subsonic, YouTube Music,
//! Tidal, etc.) requires only implementing [`MusicProvider`] and registering it
//! in [`ProviderHub::spawn`] — zero UI changes needed.

pub mod local;
#[cfg(feature = "spotify")]
pub mod spotify;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dsp::PcmTapWriter;
use crate::{PlayState, RepeatMode, Track};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    Local,
    Spotify,
}

impl ProviderId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Spotify => "SPOTIFY",
        }
    }
}

/// Capabilities advertised by a provider so the UI adapts controls dynamically.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderCaps {
    pub remote_devices: bool,
    pub remote_playback_sync: bool,
    pub liked_tracks: bool,
    pub playlist_context_play: bool,
    pub seek: bool,
    pub volume: bool,
    pub live_pcm_fft: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionKind {
    LikedSongs,
    Playlist,
    AllTracks,
}

/// A browsable collection (Liked Songs, Playlist, or Local All Tracks).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub provider: ProviderId,
    pub kind: CollectionKind,
    pub name: String,
    pub owner: Option<String>,
    /// Real track count from API (`items.total` / `tracks.total`) or local scan.
    pub track_count: Option<u32>,
    /// Playback context URI (e.g. `spotify:playlist:<id>`) for direct playback.
    pub context_uri: Option<String>,
    pub tracks_restricted: bool,
}

/// Result of drilling into a [`Collection`].
#[derive(Clone, Debug)]
pub enum CollectionItems {
    Tracks(Vec<Track>),
    /// Spotify 2026: editorial/followed playlists return HTTP 403 on `/items`,
    /// but can still be played directly via `context_uri`.
    Restricted {
        context_uri: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackDevice {
    pub id: String,
    pub provider: ProviderId,
    pub name: String,
    pub kind: String,
    pub is_active: bool,
    pub volume_pct: u8,
}

#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
    pub provider: ProviderId,
    pub state: PlayState,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub device: Option<PlaybackDevice>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume_pct: u8,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub collections: Vec<Collection>,
}

/// Single trait implemented by every local or streaming music backend.
/// All methods are synchronous and called strictly on the background
/// [`ProviderHub`] worker thread — never on the Ratatui UI thread.
pub trait MusicProvider: Send {
    fn id(&self) -> ProviderId;
    fn name(&self) -> &'static str;
    fn caps(&self) -> ProviderCaps;
    fn is_authenticated(&self) -> bool;

    fn list_collections(&mut self) -> Result<Vec<Collection>, String>;
    fn collection_items(&mut self, collection: &Collection) -> Result<CollectionItems, String>;
    fn search(&mut self, query: &str, limit: usize) -> Result<SearchResults, String>;

    fn play_track(&mut self, track: &Track, device_id: Option<&str>) -> Result<(), String>;
    fn play_context(
        &mut self,
        context_uri: &str,
        offset_uri: Option<&str>,
        device_id: Option<&str>,
    ) -> Result<(), String>;
    fn add_to_queue(&mut self, _track: &Track, _device_id: Option<&str>) -> Result<(), String> {
        Ok(())
    }
    fn pause(&mut self) -> Result<(), String>;
    fn resume(&mut self) -> Result<(), String>;
    fn next(&mut self) -> Result<(), String>;
    fn prev(&mut self) -> Result<(), String>;
    fn seek_ms(&mut self, pos_ms: u64) -> Result<(), String>;
    fn set_volume(&mut self, pct: u8) -> Result<(), String>;
    fn set_shuffle(&mut self, on: bool) -> Result<(), String>;
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<(), String>;

    fn list_devices(&mut self) -> Result<Vec<PlaybackDevice>, String> {
        Ok(Vec::new())
    }
    fn transfer_device(&mut self, _device_id: &str) -> Result<(), String> {
        Ok(())
    }
    fn poll_playback(&mut self) -> Result<Option<PlaybackSnapshot>, String> {
        Ok(None)
    }
}

/// Commands sent from the UI thread to the background [`ProviderHub`] worker.
#[derive(Clone, Debug)]
pub enum ProviderCmd {
    RefreshAll {
        music_dirs: Vec<PathBuf>,
    },
    LoadCollection(Collection),
    Search {
        seq: u64,
        query: String,
    },
    PlayTrack {
        provider: ProviderId,
        track: Track,
        device_id: Option<String>,
    },
    PlayContext {
        provider: ProviderId,
        context_uri: String,
        offset_uri: Option<String>,
        device_id: Option<String>,
    },
    AddToQueue {
        provider: ProviderId,
        track: Track,
        device_id: Option<String>,
    },
    FetchLyrics {
        track_id: crate::TrackId,
        title: String,
        artist: String,
        album: String,
        duration_secs: u64,
    },
    RemotePause(ProviderId),
    RemoteResume(ProviderId),
    RemoteNext(ProviderId),
    RemotePrev(ProviderId),
    RemoteSeekMs(ProviderId, u64),
    RemoteVolume(ProviderId, u8),
    RemoteShuffle(ProviderId, bool),
    RemoteRepeat(ProviderId, RepeatMode),
    TransferDevice {
        provider: ProviderId,
        device_id: String,
    },
    EnableLocalConnect,
    RefreshDevices,
    SpotifyLogin {
        client_id: String,
    },
    SpotifyLogout,
    SetFocused(bool),
    Shutdown,
}

/// Asynchronous events returned by the [`ProviderHub`] worker to the UI thread.
#[derive(Clone, Debug)]
pub enum ProviderEvent {
    CollectionsUpdated {
        collections: Vec<Collection>,
        local_tracks: Vec<Track>,
    },
    CollectionLoaded {
        collection_id: String,
        items: CollectionItems,
    },
    SearchCompleted {
        seq: u64,
        query: String,
        spotify: SearchResults,
    },
    DevicesUpdated(Vec<PlaybackDevice>),
    LocalConnectChanged {
        ready: bool,
        device_id: Option<String>,
    },
    RemotePlayback(Box<PlaybackSnapshot>),
    SpotifyAuthChanged {
        authenticated: bool,
        display_name: String,
        is_premium: bool,
    },
    LyricsLoaded {
        track_id: crate::TrackId,
        lyrics: Option<crate::lyrics::Lyrics>,
    },
    Status(String),
    Error(String),
}

/// Non-blocking handle to the background provider worker thread.
pub struct ProviderHub {
    tx: Sender<ProviderCmd>,
    rx: Receiver<ProviderEvent>,
}

impl ProviderHub {
    pub fn spawn(
        music_dirs: Vec<PathBuf>,
        tap: Arc<Mutex<PcmTapWriter>>,
        local_spotify_pcm_active: Arc<AtomicBool>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();

        let _ = thread::Builder::new()
            .name("cassette-providers".into())
            .spawn(move || worker_loop(music_dirs, tap, local_spotify_pcm_active, cmd_rx, ev_tx));

        Self {
            tx: cmd_tx,
            rx: ev_rx,
        }
    }

    /// Inert hub for deterministic snapshot tests (no network/disk threads).
    pub fn inert() -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        let (_ev_tx, ev_rx) = mpsc::channel();
        Self {
            tx: cmd_tx,
            rx: ev_rx,
        }
    }

    pub fn send(&self, cmd: ProviderCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv(&self) -> Option<ProviderEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ProviderHub {
    fn drop(&mut self) {
        self.send(ProviderCmd::Shutdown);
    }
}

fn worker_loop(
    initial_dirs: Vec<PathBuf>,
    #[allow(unused_variables)] tap: Arc<Mutex<PcmTapWriter>>,
    #[allow(unused_variables)] local_spotify_pcm_active: Arc<AtomicBool>,
    cmd_rx: Receiver<ProviderCmd>,
    ev_tx: Sender<ProviderEvent>,
) {
    let mut local_prov = local::LocalProvider::new(initial_dirs);
    #[cfg(feature = "spotify")]
    let mut spotify_prov = spotify::SpotifyProvider::from_saved();
    #[cfg(feature = "spotify")]
    let (connect_tx, connect_rx) = mpsc::channel::<crate::connect::ConnectEvent>();
    #[cfg(feature = "spotify")]
    let local_connect =
        crate::connect::LocalConnect::spawn(80, tap, local_spotify_pcm_active, connect_tx);
    #[cfg(feature = "spotify")]
    let mut pending_devices_refresh: Option<Instant> = None;
    #[cfg(feature = "spotify")]
    let mut local_device_id: Option<String> = None;
    #[cfg(feature = "spotify")]
    let mut using_local_device = true;

    let mut focused = true;
    let mut last_poll = Instant::now() - Duration::from_secs(10);

    // Emit initial Spotify auth state & initial collections immediately
    #[cfg(feature = "spotify")]
    if let Some((name, prem)) = spotify_prov.account_summary() {
        let _ = ev_tx.send(ProviderEvent::SpotifyAuthChanged {
            authenticated: true,
            display_name: name,
            is_premium: prem,
        });
    }

    refresh_everything(
        &mut local_prov,
        #[cfg(feature = "spotify")]
        &mut spotify_prov,
        &ev_tx,
    );

    loop {
        let poll_interval = if focused {
            Duration::from_millis(2500)
        } else {
            Duration::from_secs(10)
        };

        match cmd_rx.recv_timeout(Duration::from_millis(120)) {
            Ok(ProviderCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(ProviderCmd::SetFocused(f)) => focused = f,
            Ok(ProviderCmd::RefreshAll { music_dirs }) => {
                local_prov.set_music_dirs(music_dirs);
                refresh_everything(
                    &mut local_prov,
                    #[cfg(feature = "spotify")]
                    &mut spotify_prov,
                    &ev_tx,
                );
                last_poll = Instant::now();
            }
            Ok(ProviderCmd::LoadCollection(col)) => {
                let res = match col.provider {
                    ProviderId::Local => local_prov.collection_items(&col),
                    ProviderId::Spotify => {
                        #[cfg(feature = "spotify")]
                        {
                            spotify_prov.collection_items(&col)
                        }
                        #[cfg(not(feature = "spotify"))]
                        {
                            Err("Spotify feature disabled".into())
                        }
                    }
                };
                match res {
                    Ok(items) => {
                        let _ = ev_tx.send(ProviderEvent::CollectionLoaded {
                            collection_id: col.id,
                            items,
                        });
                    }
                    Err(e) => {
                        let _ = ev_tx.send(ProviderEvent::Error(e));
                    }
                }
            }
            Ok(ProviderCmd::Search { seq, query }) => {
                #[cfg(feature = "spotify")]
                let sp = if spotify_prov.is_authenticated() && query.trim().len() >= 2 {
                    match spotify_prov.search(&query, 10) {
                        Ok(res) => res,
                        Err(e) => {
                            let _ = ev_tx.send(ProviderEvent::Error(e));
                            SearchResults::default()
                        }
                    }
                } else {
                    SearchResults::default()
                };
                #[cfg(not(feature = "spotify"))]
                let sp = SearchResults::default();

                let _ = ev_tx.send(ProviderEvent::SearchCompleted {
                    seq,
                    query,
                    spotify: sp,
                });
            }
            Ok(ProviderCmd::PlayTrack {
                provider,
                track,
                device_id,
            }) => {
                if provider == ProviderId::Spotify {
                    #[cfg(feature = "spotify")]
                    match spotify_prov.play_track(&track, device_id.as_deref()) {
                        Ok(()) => {
                            let _ = ev_tx
                                .send(ProviderEvent::Status(format!("Playing {}", track.title)));
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        Err(e) => {
                            let _ = ev_tx.send(ProviderEvent::Error(e));
                        }
                    }
                }
            }
            Ok(ProviderCmd::PlayContext {
                provider,
                context_uri,
                offset_uri,
                device_id,
            }) => {
                if provider == ProviderId::Spotify {
                    #[cfg(feature = "spotify")]
                    match spotify_prov.play_context(
                        &context_uri,
                        offset_uri.as_deref(),
                        device_id.as_deref(),
                    ) {
                        Ok(()) => {
                            let _ = ev_tx.send(ProviderEvent::Status(
                                "Started playback on Spotify Connect".into(),
                            ));
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        Err(e) => {
                            let _ = ev_tx.send(ProviderEvent::Error(e));
                        }
                    }
                }
            }
            Ok(ProviderCmd::AddToQueue {
                provider,
                track,
                device_id,
            }) => {
                if provider == ProviderId::Spotify {
                    #[cfg(feature = "spotify")]
                    match spotify_prov.add_to_queue(&track, device_id.as_deref()) {
                        Ok(()) => {
                            let _ = ev_tx
                                .send(ProviderEvent::Status(format!("Queued: {}", track.title)));
                        }
                        Err(e) => {
                            let _ = ev_tx.send(ProviderEvent::Error(format!("Queue failed: {e}")));
                        }
                    }
                }
            }
            Ok(ProviderCmd::FetchLyrics {
                track_id,
                title,
                artist,
                album,
                duration_secs,
            }) => {
                let tx = ev_tx.clone();
                let _ = thread::Builder::new()
                    .name("hayato-lyrics".into())
                    .spawn(move || {
                        let lyrics = crate::lyrics::Lyrics::fetch_or_cache(
                            &title,
                            &artist,
                            &album,
                            duration_secs,
                            &crate::cache_dir(),
                        );
                        let _ = tx.send(ProviderEvent::LyricsLoaded { track_id, lyrics });
                    });
            }
            Ok(ProviderCmd::RemotePause(ProviderId::Spotify)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.pause();
                } else if let Err(e) = spotify_prov.pause() {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::RemoteResume(ProviderId::Spotify)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.play();
                } else if let Err(e) = spotify_prov.resume() {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::RemoteNext(ProviderId::Spotify)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.next();
                    last_poll = Instant::now() - Duration::from_secs(10);
                } else if let Err(e) = spotify_prov.next() {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                } else {
                    last_poll = Instant::now() - Duration::from_secs(10);
                }
            }
            Ok(ProviderCmd::RemotePrev(ProviderId::Spotify)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.prev();
                    last_poll = Instant::now() - Duration::from_secs(10);
                } else if let Err(e) = spotify_prov.prev() {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                } else {
                    last_poll = Instant::now() - Duration::from_secs(10);
                }
            }
            Ok(ProviderCmd::RemoteSeekMs(ProviderId::Spotify, ms)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.seek_ms(ms.min(u64::from(u32::MAX)) as u32);
                } else if let Err(e) = spotify_prov.seek_ms(ms) {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::RemoteVolume(ProviderId::Spotify, pct)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.set_volume_pct(pct);
                } else if let Err(e) = spotify_prov.set_volume(pct) {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::RemoteShuffle(ProviderId::Spotify, on)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.set_shuffle(on);
                } else if let Err(e) = spotify_prov.set_shuffle(on) {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::RemoteRepeat(ProviderId::Spotify, mode)) => {
                #[cfg(feature = "spotify")]
                if local_device_id.is_some() && using_local_device {
                    local_connect.set_repeat(mode);
                } else if let Err(e) = spotify_prov.set_repeat(mode) {
                    let _ = ev_tx.send(ProviderEvent::Error(e));
                }
            }
            Ok(ProviderCmd::TransferDevice {
                provider: ProviderId::Spotify,
                device_id,
            }) => {
                #[cfg(feature = "spotify")]
                {
                    using_local_device = local_device_id.as_deref() == Some(device_id.as_str());
                    match spotify_prov.transfer_device(&device_id) {
                        Ok(()) => {
                            let _ = ev_tx.send(ProviderEvent::Status(
                                "Transferred Spotify Connect playback".into(),
                            ));
                            if let Ok(devs) = spotify_prov.list_devices() {
                                let _ = ev_tx.send(ProviderEvent::DevicesUpdated(devs));
                            }
                            last_poll = Instant::now() - Duration::from_secs(10);
                        }
                        Err(e) => {
                            let _ = ev_tx.send(ProviderEvent::Error(e));
                        }
                    }
                }
            }
            Ok(ProviderCmd::EnableLocalConnect) => {
                #[cfg(feature = "spotify")]
                {
                    local_connect.pair_oauth();
                }
            }
            Ok(ProviderCmd::RefreshDevices) => {
                #[cfg(feature = "spotify")]
                if spotify_prov.is_authenticated()
                    && let Ok(devs) = spotify_prov.list_devices()
                {
                    let _ = ev_tx.send(ProviderEvent::DevicesUpdated(devs));
                }
            }
            Ok(ProviderCmd::SpotifyLogin { client_id }) => {
                #[cfg(feature = "spotify")]
                match spotify_prov.login_pkce(&client_id) {
                    Ok((name, prem)) => {
                        let _ = ev_tx.send(ProviderEvent::SpotifyAuthChanged {
                            authenticated: true,
                            display_name: name,
                            is_premium: prem,
                        });
                        let _ = ev_tx.send(ProviderEvent::Status("Connected to Spotify!".into()));
                        refresh_everything(&mut local_prov, &mut spotify_prov, &ev_tx);
                        last_poll = Instant::now() - Duration::from_secs(10);
                    }
                    Err(e) => {
                        let _ = ev_tx.send(ProviderEvent::Error(e));
                    }
                }
            }
            Ok(ProviderCmd::SpotifyLogout) => {
                #[cfg(feature = "spotify")]
                {
                    spotify_prov.logout();
                    local_connect.clear_and_disconnect();
                    let _ = ev_tx.send(ProviderEvent::SpotifyAuthChanged {
                        authenticated: false,
                        display_name: String::new(),
                        is_premium: false,
                    });
                    let _ = ev_tx.send(ProviderEvent::LocalConnectChanged {
                        ready: false,
                        device_id: None,
                    });
                    let _ = ev_tx.send(ProviderEvent::DevicesUpdated(Vec::new()));
                    refresh_everything(&mut local_prov, &mut spotify_prov, &ev_tx);
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        // Drain embedded local Spotify Connect receiver events
        #[cfg(feature = "spotify")]
        while let Ok(cev) = connect_rx.try_recv() {
            match cev {
                crate::connect::ConnectEvent::Ready { device_id } => {
                    local_device_id = Some(device_id.clone());
                    using_local_device = true;
                    let _ = ev_tx.send(ProviderEvent::LocalConnectChanged {
                        ready: true,
                        device_id: Some(device_id),
                    });
                    pending_devices_refresh = Some(Instant::now() + Duration::from_millis(1200));
                }
                crate::connect::ConnectEvent::Unpaired => {
                    local_device_id = None;
                    using_local_device = false;
                    let _ = ev_tx.send(ProviderEvent::LocalConnectChanged {
                        ready: false,
                        device_id: None,
                    });
                }
                crate::connect::ConnectEvent::Status(msg) => {
                    let _ = ev_tx.send(ProviderEvent::Status(msg));
                }
                crate::connect::ConnectEvent::Error(msg) => {
                    let _ = ev_tx.send(ProviderEvent::Error(msg));
                }
            }
        }

        #[cfg(feature = "spotify")]
        if let Some(at) = pending_devices_refresh
            && Instant::now() >= at
        {
            pending_devices_refresh = None;
            if spotify_prov.is_authenticated()
                && let Ok(devs) = spotify_prov.list_devices()
            {
                let _ = ev_tx.send(ProviderEvent::DevicesUpdated(devs));
            }
        }

        // Periodic live Spotify Connect state sync (non-blocking)
        #[cfg(feature = "spotify")]
        if spotify_prov.is_authenticated() && last_poll.elapsed() >= poll_interval {
            last_poll = Instant::now();
            if let Ok(Some(snap)) = spotify_prov.poll_playback() {
                let _ = ev_tx.send(ProviderEvent::RemotePlayback(Box::new(snap)));
            }
        }
    }
}

fn refresh_everything(
    local_prov: &mut local::LocalProvider,
    #[cfg(feature = "spotify")] spotify_prov: &mut spotify::SpotifyProvider,
    ev_tx: &Sender<ProviderEvent>,
) {
    let mut collections = Vec::new();

    #[cfg(feature = "spotify")]
    if spotify_prov.is_authenticated() {
        if let Ok(mut sp_cols) = spotify_prov.list_collections() {
            collections.append(&mut sp_cols);
        }
        if let Ok(devs) = spotify_prov.list_devices() {
            let _ = ev_tx.send(ProviderEvent::DevicesUpdated(devs));
        }
    }

    if let Ok(mut loc_cols) = local_prov.list_collections() {
        collections.append(&mut loc_cols);
    }

    let _ = ev_tx.send(ProviderEvent::CollectionsUpdated {
        collections,
        local_tracks: local_prov.tracks().to_vec(),
    });
}
