//! MPRIS2 D-Bus server (`org.mpris.MediaPlayer2.hayato_cassette`) over `zbus` 5
//! plus unfocused track-change notifications via `notify-send` (`hayato-notify`).

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use zbus::blocking::connection::Builder;
use zbus::interface;
use zbus::zvariant::Value;

use crate::{PlayState, Track};

/// Commands sent from external MPRIS clients (`playerctl`, media keys, Waybar) to Cassette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MprisCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Raise,
    Quit,
}

#[derive(Clone, Debug, Default)]
pub struct MprisState {
    pub play_state: PlayState,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_us: i64,
    pub position_us: i64,
    pub art_url: String,
}

struct RootIface {
    tx: Sender<MprisCommand>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl RootIface {
    fn raise(&self) {
        let _ = self.tx.send(MprisCommand::Raise);
    }

    fn quit(&self) {
        let _ = self.tx.send(MprisCommand::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Hayato Cassette".to_string()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "org.hayato.cassette".to_string()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".to_string(), "spotify".to_string()]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![
            "audio/mpeg".to_string(),
            "audio/flac".to_string(),
            "audio/ogg".to_string(),
            "audio/opus".to_string(),
            "audio/wav".to_string(),
            "audio/mp4".to_string(),
        ]
    }
}

struct PlayerIface {
    tx: Sender<MprisCommand>,
    state: Arc<Mutex<MprisState>>,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) {
        let _ = self.tx.send(MprisCommand::Next);
    }

    fn previous(&self) {
        let _ = self.tx.send(MprisCommand::Previous);
    }

    fn pause(&self) {
        let _ = self.tx.send(MprisCommand::Pause);
    }

    fn play_pause(&self) {
        let _ = self.tx.send(MprisCommand::PlayPause);
    }

    fn stop(&self) {
        let _ = self.tx.send(MprisCommand::Stop);
    }

    fn play(&self) {
        let _ = self.tx.send(MprisCommand::Play);
    }

    #[zbus(property)]
    fn playback_status(&self) -> String {
        let s = self
            .state
            .lock()
            .map(|g| g.play_state)
            .unwrap_or(PlayState::Stopped);
        match s {
            PlayState::Playing => "Playing".to_string(),
            PlayState::Paused => "Paused".to_string(),
            PlayState::Stopped => "Stopped".to_string(),
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let mut map = HashMap::new();
        if let Ok(g) = self.state.lock() {
            map.insert(
                "mpris:trackid".to_string(),
                Value::from("/org/hayato/cassette/track/current"),
            );
            if !g.title.is_empty() {
                map.insert("xesam:title".to_string(), Value::from(g.title.clone()));
            }
            if !g.artist.is_empty() {
                map.insert(
                    "xesam:artist".to_string(),
                    Value::from(vec![g.artist.clone()]),
                );
            }
            if !g.album.is_empty() {
                map.insert("xesam:album".to_string(), Value::from(g.album.clone()));
            }
            if g.duration_us > 0 {
                map.insert("mpris:length".to_string(), Value::from(g.duration_us));
            }
            if !g.art_url.is_empty() {
                map.insert("mpris:artUrl".to_string(), Value::from(g.art_url.clone()));
            }
        }
        map
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state.lock().map(|g| g.position_us).unwrap_or(0)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Handle to the running MPRIS2 background service.
pub struct MprisHandle {
    state: Arc<Mutex<MprisState>>,
}

impl MprisHandle {
    /// Start the MPRIS2 D-Bus server on the user session bus.
    /// Fails silently (returning a dummy handle) when no D-Bus session exists (e.g. unit tests).
    pub fn spawn(tx: Sender<MprisCommand>) -> Self {
        let state = Arc::new(Mutex::new(MprisState::default()));
        let state_bg = Arc::clone(&state);

        let _ = thread::Builder::new()
            .name("cassette-mpris".into())
            .spawn(move || {
                let conn = Builder::session()
                    .and_then(|b| b.name("org.mpris.MediaPlayer2.hayato_cassette"))
                    .and_then(|b| {
                        b.serve_at("/org/mpris/MediaPlayer2", RootIface { tx: tx.clone() })
                    })
                    .and_then(|b| {
                        b.serve_at(
                            "/org/mpris/MediaPlayer2",
                            PlayerIface {
                                tx,
                                state: state_bg,
                            },
                        )
                    })
                    .and_then(Builder::build);

                if let Ok(_conn) = conn {
                    loop {
                        thread::park();
                    }
                }
            });

        Self { state }
    }

    pub fn update(&self, play_state: PlayState, track: Option<&Track>, position_secs: u64) {
        if let Ok(mut g) = self.state.lock() {
            g.play_state = play_state;
            g.position_us = (position_secs as i64).saturating_mul(1_000_000);
            if let Some(t) = track {
                g.title = t.title.clone();
                g.artist = t.artist.clone();
                g.album = t.album.clone();
                g.duration_us = (t.duration_secs as i64).saturating_mul(1_000_000);
                g.art_url = t
                    .art_path
                    .as_ref()
                    .map(|p| format!("file://{}", p.display()))
                    .unwrap_or_default();
            } else {
                g.title.clear();
                g.artist.clear();
                g.album.clear();
                g.duration_us = 0;
                g.art_url.clear();
            }
        }
    }
}

/// Send a synchronous track-change notification card when Cassette is unfocused/hidden.
pub fn notify_track_change(track: &Track) {
    if std::env::var_os("HAYATO_NO_NOTIFY").is_some() {
        return;
    }
    let mut cmd = Command::new("notify-send");
    cmd.args([
        "-a",
        "Hayato",
        "-h",
        "string:x-canonical-private-synchronous:hayato-cassette",
        "-t",
        "3500",
    ]);
    if let Some(art) = &track.art_path
        && art.is_file()
    {
        cmd.arg("-i").arg(art);
    } else {
        cmd.args(["-i", "audio-x-generic"]);
    }
    let body = format!("{} · {}", track.artist, track.album);
    cmd.arg("--").arg(&track.title).arg(body);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.spawn();
}
