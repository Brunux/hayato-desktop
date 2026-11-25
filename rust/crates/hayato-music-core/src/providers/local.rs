//! Local filesystem audio provider (`LocalProvider`), wrapping `library.rs`.

use std::path::PathBuf;

use crate::library::{load_local_playlists, scan_directories};
use crate::providers::{
    Collection, CollectionItems, CollectionKind, MusicProvider, ProviderCaps, ProviderId,
    SearchResults,
};
use crate::{Playlist, RepeatMode, Track};

pub struct LocalProvider {
    music_dirs: Vec<PathBuf>,
    tracks: Vec<Track>,
    playlists: Vec<Playlist>,
}

impl LocalProvider {
    pub fn new(music_dirs: Vec<PathBuf>) -> Self {
        Self {
            music_dirs,
            tracks: Vec::new(),
            playlists: Vec::new(),
        }
    }

    pub fn set_music_dirs(&mut self, dirs: Vec<PathBuf>) {
        self.music_dirs = dirs;
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }
}

impl MusicProvider for LocalProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Local
    }

    fn name(&self) -> &'static str {
        "Local Library"
    }

    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            remote_devices: false,
            remote_playback_sync: false,
            liked_tracks: false,
            playlist_context_play: false,
            seek: true,
            volume: true,
            live_pcm_fft: true,
        }
    }

    fn is_authenticated(&self) -> bool {
        true
    }

    fn list_collections(&mut self) -> Result<Vec<Collection>, String> {
        self.tracks = scan_directories(&self.music_dirs);
        self.playlists = load_local_playlists(&self.music_dirs, &self.tracks);

        let mut cols = vec![Collection {
            id: "local:all".to_string(),
            provider: ProviderId::Local,
            kind: CollectionKind::AllTracks,
            name: "All Local Tracks".to_string(),
            owner: Some("This Machine".to_string()),
            track_count: Some(self.tracks.len() as u32),
            context_uri: None,
            tracks_restricted: false,
        }];
        for pl in &self.playlists {
            cols.push(Collection {
                id: pl.id.clone(),
                provider: ProviderId::Local,
                kind: CollectionKind::Playlist,
                name: pl.name.clone(),
                owner: Some(".m3u8".to_string()),
                track_count: Some(pl.tracks.len() as u32),
                context_uri: None,
                tracks_restricted: false,
            });
        }
        Ok(cols)
    }

    fn collection_items(&mut self, collection: &Collection) -> Result<CollectionItems, String> {
        if collection.id == "local:all" {
            return Ok(CollectionItems::Tracks(self.tracks.clone()));
        }
        if let Some(pl) = self.playlists.iter().find(|p| p.id == collection.id) {
            return Ok(CollectionItems::Tracks(pl.tracks.clone()));
        }
        Ok(CollectionItems::Tracks(Vec::new()))
    }

    fn search(&mut self, query: &str, limit: usize) -> Result<SearchResults, String> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(SearchResults::default());
        }
        let tracks = self
            .tracks
            .iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&q)
                    || t.artist.to_lowercase().contains(&q)
                    || t.album.to_lowercase().contains(&q)
            })
            .take(limit)
            .cloned()
            .collect();
        Ok(SearchResults {
            tracks,
            collections: Vec::new(),
        })
    }

    fn play_track(&mut self, _track: &Track, _device_id: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    fn play_context(
        &mut self,
        _context_uri: &str,
        _offset_uri: Option<&str>,
        _device_id: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn next(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn prev(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn seek_ms(&mut self, _pos_ms: u64) -> Result<(), String> {
        Ok(())
    }

    fn set_volume(&mut self, _pct: u8) -> Result<(), String> {
        Ok(())
    }

    fn set_shuffle(&mut self, _on: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_repeat(&mut self, _mode: RepeatMode) -> Result<(), String> {
        Ok(())
    }
}
