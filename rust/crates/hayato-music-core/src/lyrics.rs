//! Synchronized `.lrc` lyrics loader and timestamp lookup.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
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
        Self { lines }
    }

    /// Index of the active lyric line for `position_ms`.
    pub fn active_index(&self, position_ms: u64) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let pos = self.lines.partition_point(|l| l.time_ms <= position_ms);
        pos.checked_sub(1)
    }
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
