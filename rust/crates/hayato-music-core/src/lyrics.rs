//! Synchronized `.lrc` lyrics loader, LRCLIB online fetcher, disk cache, and timestamp lookup.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub synced: bool,
}

impl Lyrics {
    /// Load a sidecar `<track_stem>.lrc` next to a local audio file.
    pub fn load_sidecar(audio_path: &Path) -> Option<Self> {
        let lrc_path = audio_path.with_extension("lrc");
        let text = std::fs::read_to_string(lrc_path).ok()?;
        let parsed = Self::parse_lrc(&text);
        (!parsed.lines.is_empty()).then_some(parsed)
    }

    /// Parse standard LRC timestamps (`[mm:ss.xx]` or `[mm:ss.xxx]` or `[mm:ss]`).
    pub fn parse_lrc(content: &str) -> Self {
        let mut lines = Vec::new();
        for raw in content.lines() {
            let mut rest = raw.trim();
            let mut stamps = Vec::new();
            while let Some(after_open) = rest.strip_prefix('[') {
                let Some(close) = after_open.find(']') else {
                    break;
                };
                let tag = &after_open[..close];
                if let Some(ms) = parse_timestamp(tag) {
                    stamps.push(ms);
                    rest = &after_open[close + 1..];
                } else {
                    // Metadata tag like [ar:...] or [ti:...]
                    stamps.clear();
                    break;
                }
            }
            let text = rest.trim().to_string();
            for ms in stamps {
                lines.push(LyricLine {
                    time_ms: ms,
                    text: text.clone(),
                });
            }
        }
        lines.sort_by_key(|l| l.time_ms);
        Self {
            lines,
            synced: true,
        }
    }

    /// Build unsynced lyrics from plain multi-line text.
    pub fn from_plain(content: &str) -> Self {
        let lines = content
            .lines()
            .map(|l| LyricLine {
                time_ms: 0,
                text: l.trim().to_string(),
            })
            .collect();
        Self {
            lines,
            synced: false,
        }
    }

    /// Look up cached lyrics on disk, or fetch synced/plain lyrics from LRCLIB
    /// (free, zero-auth) and persist to `cache_dir/lyrics/`.
    pub fn fetch_or_cache(
        title: &str,
        artist: &str,
        album: &str,
        duration_secs: u64,
        cache_dir: &Path,
    ) -> Option<Self> {
        let title = title.trim();
        let artist = artist.trim();
        if title.is_empty() {
            return None;
        }
        let dir = cache_dir.join("lyrics");
        let key = sanitize_key(&format!("{artist}__{title}"));
        let lrc_cache = dir.join(format!("{key}.lrc"));
        let txt_cache = dir.join(format!("{key}.txt"));

        if let Ok(raw) = std::fs::read_to_string(&lrc_cache) {
            let parsed = Self::parse_lrc(&raw);
            if !parsed.lines.is_empty() {
                return Some(parsed);
            }
        }
        if let Ok(raw) = std::fs::read_to_string(&txt_cache) {
            let parsed = Self::from_plain(&raw);
            if !parsed.lines.is_empty() {
                return Some(parsed);
            }
        }

        // Primary LRCLIB exact match endpoint
        let mut url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            url_encode(title),
            url_encode(artist),
        );
        if !album.trim().is_empty() {
            url.push_str("&album_name=");
            url.push_str(&url_encode(album.trim()));
        }
        if duration_secs > 0 {
            url.push_str(&format!("&duration={duration_secs}"));
        }

        let found = curl_json(&url)
            .and_then(|v| parse_lrclib_entry(&v))
            .or_else(|| {
                // Fallback to LRCLIB fuzzy search when exact album/duration differs
                let search_url = format!(
                    "https://lrclib.net/api/search?track_name={}&artist_name={}",
                    url_encode(title),
                    url_encode(artist),
                );
                let arr = curl_json(&search_url)?;
                let items = arr.as_array()?;
                // Prefer synced results first, then plain lyrics
                items
                    .iter()
                    .find_map(|item| {
                        let synced = item["syncedLyrics"].as_str()?;
                        let parsed = Self::parse_lrc(synced);
                        (!parsed.lines.is_empty()).then_some((parsed, true, synced.to_string()))
                    })
                    .or_else(|| items.iter().find_map(parse_lrclib_entry))
            });

        if let Some((lyrics, is_synced, raw)) = found {
            let _ = std::fs::create_dir_all(&dir);
            let target: PathBuf = if is_synced { lrc_cache } else { txt_cache };
            let _ = std::fs::write(target, raw);
            return Some(lyrics);
        }
        None
    }

    /// Index of the active lyric line for `position_ms`.
    pub fn active_index(&self, position_ms: u64) -> Option<usize> {
        if self.lines.is_empty() || !self.synced {
            return None;
        }
        let pos = self.lines.partition_point(|l| l.time_ms <= position_ms);
        pos.checked_sub(1)
    }
}

fn parse_lrclib_entry(v: &serde_json::Value) -> Option<(Lyrics, bool, String)> {
    if let Some(synced) = v["syncedLyrics"].as_str()
        && !synced.trim().is_empty()
    {
        let parsed = Lyrics::parse_lrc(synced);
        if !parsed.lines.is_empty() {
            return Some((parsed, true, synced.to_string()));
        }
    }
    if let Some(plain) = v["plainLyrics"].as_str()
        && !plain.trim().is_empty()
    {
        let parsed = Lyrics::from_plain(plain);
        if !parsed.lines.is_empty() {
            return Some((parsed, false, plain.to_string()));
        }
    }
    None
}

fn curl_json(url: &str) -> Option<serde_json::Value> {
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "6",
            "-H",
            "User-Agent: HayatoCassette/1.0 (https://github.com/Brunux/Hayato)",
            url,
        ])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn sanitize_key(s: &str) -> String {
    let mut out: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.len() > 96 {
        out.truncate(96);
    }
    out
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn parse_timestamp(tag: &str) -> Option<u64> {
    let (min_s, sec_s) = tag.split_once(':')?;
    let mins: u64 = min_s.trim().parse().ok()?;
    let (sec_whole, frac_ms) = if let Some((s, f)) = sec_s.split_once('.') {
        let sec: u64 = s.parse().ok()?;
        let ms = match f.len() {
            0 => 0,
            1 => f.parse::<u64>().ok()? * 100,
            2 => f.parse::<u64>().ok()? * 10,
            _ => f[..3].parse::<u64>().ok()?,
        };
        (sec, ms)
    } else {
        (sec_s.parse().ok()?, 0)
    };
    Some((mins * 60 + sec_whole) * 1000 + frac_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_stamp_lrc_and_finds_active_line() {
        let src = "[ti:Cyberdeck]\n[00:05.50]First line\n[00:12.00][01:02.25]Chorus\n";
        let lyr = Lyrics::parse_lrc(src);
        assert_eq!(lyr.lines.len(), 3);
        assert_eq!(lyr.active_index(4000), None);
        assert_eq!(lyr.active_index(6000), Some(0));
        assert_eq!(lyr.active_index(12000), Some(1));
        assert_eq!(lyr.active_index(63000), Some(2));
        assert_eq!(lyr.lines[2].text, "Chorus");
    }
}
