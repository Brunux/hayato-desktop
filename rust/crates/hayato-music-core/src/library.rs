//! Local music library scanner (`lofty` metadata + embedded/folder cover art
//! extraction), XDG disk cache, and `.m3u8` playlist reader/writer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};

use crate::{CodecInfo, Playlist, SourceKind, Track, TrackId, cache_dir, fnv1a_64};

const AUDIO_EXTS: &[&str] = &["mp3", "flac", "ogg", "opus", "wav", "m4a", "aac", "alac"];
const COVER_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "folder.jpg",
    "folder.png",
    "front.jpg",
    "front.png",
    "album.jpg",
    "album.png",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedEntry {
    mtime_secs: u64,
    size_bytes: u64,
    track: Track,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct LibraryCache {
    version: u32,
    entries: HashMap<PathBuf, CachedEntry>,
}

const CACHE_VERSION: u32 = 1;

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))
}

/// Load the cached library immediately (for instant startup before background rescan).
pub fn load_cached_tracks() -> Vec<Track> {
    let path = cache_dir().join("index.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<LibraryCache>(&text) else {
        return Vec::new();
    };
    if cache.version != CACHE_VERSION {
        return Vec::new();
    }
    let mut tracks: Vec<Track> = cache
        .entries
        .into_values()
        .filter(|e| match &e.track.source {
            SourceKind::Local { path, .. } => path.exists(),
            SourceKind::Spotify { .. } => true,
        })
        .map(|e| e.track)
        .collect();
    sort_tracks(&mut tracks);
    tracks
}

/// Incrementally scan `dirs`, reusing cached metadata for unchanged `(mtime, size)` files.
pub fn scan_directories(dirs: &[PathBuf]) -> Vec<Track> {
    let cache_path = cache_dir().join("index.json");
    let art_cache_dir = cache_dir().join("art");
    let _ = fs::create_dir_all(&art_cache_dir);

    let prev_cache: LibraryCache = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|c: &LibraryCache| c.version == CACHE_VERSION)
        .unwrap_or_else(|| LibraryCache {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        });

    let mut files = Vec::new();
    for dir in dirs {
        if dir.is_dir() {
            walk_audio_files(dir, &mut files, 0);
        }
    }

    let mut next_entries = HashMap::with_capacity(files.len());
    for path in files {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let size_bytes = meta.len();

        if let Some(prev) = prev_cache.entries.get(&path)
            && prev.mtime_secs == mtime_secs
            && prev.size_bytes == size_bytes
        {
            next_entries.insert(path, prev.clone());
            continue;
        }

        let track = read_track_metadata(&path, &art_cache_dir);
        next_entries.insert(
            path,
            CachedEntry {
                mtime_secs,
                size_bytes,
                track,
            },
        );
    }

    let mut tracks: Vec<Track> = next_entries.values().map(|e| e.track.clone()).collect();
    sort_tracks(&mut tracks);

    let new_cache = LibraryCache {
        version: CACHE_VERSION,
        entries: next_entries,
    };
    let _ = fs::create_dir_all(cache_dir());
    let tmp = cache_dir().join("index.json.tmp");
    if let Ok(json) = serde_json::to_string(&new_cache)
        && fs::write(&tmp, json).is_ok()
    {
        let _ = fs::rename(&tmp, &cache_path);
    }

    tracks
}

fn walk_audio_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if let Some(name) = p.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        if p.is_dir() {
            walk_audio_files(&p, out, depth + 1);
        } else if is_audio_file(&p) {
            out.push(p);
        }
    }
}

/// Extract tags, codec telemetry, and embedded/folder cover art from one file.
pub fn read_track_metadata(path: &Path, art_cache_dir: &Path) -> Track {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let id = TrackId::Local(fnv1a_64(canonical.to_string_lossy().as_bytes()));
    let fallback_title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown Track")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("AUDIO")
        .to_ascii_uppercase();

    let mut title = fallback_title;
    let mut artist = "Unknown Artist".to_string();
    let mut album = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("Singles")
        .to_string();
    let mut track_number = None;
    let mut year = None;
    let mut duration_secs = 0u64;
    let mut codec = CodecInfo {
        format: ext,
        ..CodecInfo::default()
    };
    let mut art_path = None;

    if let Ok(probe) = Probe::open(path)
        && let Ok(tagged) = probe.read()
    {
        let props = tagged.properties();
        duration_secs = props.duration().as_secs();
        codec.sample_rate_hz = props.sample_rate().unwrap_or(0);
        codec.bit_depth = props.bit_depth().map(u16::from);
        codec.bitrate_kbps = props.audio_bitrate();

        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(t) = tag.title().filter(|s| !s.trim().is_empty()) {
                title = t.into_owned();
            }
            if let Some(a) = tag.artist().filter(|s| !s.trim().is_empty()) {
                artist = a.into_owned();
            }
            if let Some(al) = tag.album().filter(|s| !s.trim().is_empty()) {
                album = al.into_owned();
            }
            track_number = tag.track();
            year = tag.year();

            if let Some(pic) = tag.pictures().first() {
                let pic_hash = fnv1a_64(format!("{artist}\0{album}").as_bytes());
                let pic_ext = match pic.mime_type() {
                    Some(lofty::picture::MimeType::Png) => "png",
                    _ => "jpg",
                };
                let dst = art_cache_dir.join(format!("{pic_hash:016x}.{pic_ext}"));
                if !dst.exists() {
                    let _ = fs::write(&dst, pic.data());
                }
                if dst.exists() {
                    art_path = Some(dst);
                }
            }
        }
    }

    if art_path.is_none()
        && let Some(parent) = path.parent()
    {
        for name in COVER_NAMES {
            let cand = parent.join(name);
            if cand.is_file() {
                art_path = Some(cand);
                break;
            }
        }
    }

    Track {
        id,
        title,
        artist,
        album,
        track_number,
        year,
        duration_secs,
        source: SourceKind::Local {
            path: canonical,
            codec,
        },
        art_path,
    }
}

pub fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| {
        a.artist
            .to_lowercase()
            .cmp(&b.artist.to_lowercase())
            .then_with(|| a.album.to_lowercase().cmp(&b.album.to_lowercase()))
            .then_with(|| a.track_number.unwrap_or(9999).cmp(&b.track_number.unwrap_or(9999)))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Load local `.m3u8` / `.m3u` playlists from `<music_dir>/Playlists/`.
pub fn load_local_playlists(music_dirs: &[PathBuf], library: &[Track]) -> Vec<Playlist> {
    let by_path: HashMap<&Path, &Track> = library
        .iter()
        .filter_map(|t| match &t.source {
            SourceKind::Local { path, .. } => Some((path.as_path(), t)),
            SourceKind::Spotify { .. } => None,
        })
        .collect();

    let mut out = Vec::new();
    for dir in music_dirs {
        let pdir = dir.join("Playlists");
        let Ok(entries) = fs::read_dir(&pdir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let is_m3u = p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("m3u8") || e.eq_ignore_ascii_case("m3u"));
            if !is_m3u {
                continue;
            }
            let Ok(text) = fs::read_to_string(&p) else {
                continue;
            };
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Playlist")
                .to_string();
            let mut tracks = Vec::new();
            for line in text.lines() {
                let l = line.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let candidate = if Path::new(l).is_absolute() {
                    PathBuf::from(l)
                } else {
                    pdir.join(l)
                };
                let canon = fs::canonicalize(&candidate).unwrap_or(candidate);
                if let Some(t) = by_path.get(canon.as_path()) {
                    tracks.push((*t).clone());
                } else if canon.is_file() {
                    tracks.push(read_track_metadata(&canon, &cache_dir().join("art")));
                }
            }
            out.push(Playlist {
                id: format!("local:{name}"),
                name,
                is_spotify: false,
                path: Some(p),
                tracks,
            });
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Save a queue as a `.m3u8` playlist under `<first_music_dir>/Playlists/<name>.m3u8`.
pub fn save_m3u8_playlist(
    music_dirs: &[PathBuf],
    name: &str,
    tracks: &[Track],
) -> Result<PathBuf, String> {
    let base = music_dirs
        .first()
        .cloned()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Music"));
    let pdir = base.join("Playlists");
    fs::create_dir_all(&pdir).map_err(|e| format!("create Playlists dir: {e}"))?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dst = pdir.join(format!("{}.m3u8", safe.trim()));
    let mut body = String::from("#EXTM3U\n");
    for t in tracks {
        if let SourceKind::Local { path, .. } = &t.source {
            body.push_str(&format!("#EXTINF:{},{} - {}\n", t.duration_secs, t.artist, t.title));
            body.push_str(&format!("{}\n", path.display()));
        }
    }
    fs::write(&dst, body).map_err(|e| format!("write playlist: {e}"))?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reloads_m3u8_playlist() {
        let tmp = tempfile::tempdir().unwrap();
        let song = tmp.path().join("demo.mp3");
        fs::write(&song, b"").unwrap();
        let track = read_track_metadata(&song, tmp.path());
        let saved = save_m3u8_playlist(
            &[tmp.path().to_path_buf()],
            "Night Drive",
            std::slice::from_ref(&track),
        )
            .unwrap();
        assert!(saved.exists());
        let loaded = load_local_playlists(&[tmp.path().to_path_buf()], &[track]);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Night Drive");
        assert_eq!(loaded[0].tracks.len(), 1);
    }
}
