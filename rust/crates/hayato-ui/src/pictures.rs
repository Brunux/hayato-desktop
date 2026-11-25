//! Terminal image capability detection (Sixel vs half-blocks), shared by
//! `hayato-cli` (file previews) and `hayato-cassette` (album art).
//!
//! `ratatui_image`'s own query reads stdin in a background thread that can
//! outlive a timeout and swallow the next keypress. Here nothing outlives the
//! call: a terminal that stays silent costs `timeout` once before the first
//! frame and falls back cleanly to half-blocks. `foot` answers both queries in
//! a few milliseconds and gets Sixel.

use std::time::{Duration, Instant};

use ratatui_image::picker::{Picker, ProtocolType};

/// Default deadline for terminal capability queries.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(300);

/// How pictures are drawn, decided once before the first frame.
pub enum Pictures {
    Ready(Box<Picker>),
    /// No pictures (`HAYATO_PICTURES=off`).
    Off,
}

impl Pictures {
    /// `HAYATO_PICTURES=off|halfblocks` decides without asking; otherwise
    /// the terminal is queried synchronously with [`QUERY_TIMEOUT`].
    pub fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("off") => Pictures::Off,
            Some("halfblocks") => Pictures::Ready(Box::new(Picker::halfblocks())),
            _ => Pictures::Ready(Box::new(query_terminal(QUERY_TIMEOUT))),
        }
    }
}

/// What a terminal said about itself: the cell size in pixels (`CSI 16 t`)
/// and whether its device attributes (`CSI c`) list sixel (attribute 4).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub cell: Option<(u16, u16)>,
    pub sixel: bool,
    /// The device attributes arrived: the terminal has said all it will.
    pub complete: bool,
}

/// Parse `ESC [ 6 ; height ; width t` and `ESC [ ? 62 ; 4 ; ... c`.
pub fn parse_answer(bytes: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(bytes);
    let mut answer = Answer::default();
    for part in text.split('\x1b').filter_map(|p| p.strip_prefix('[')) {
        if let Some(body) = part.strip_prefix("6;")
            && let Some(end) = body.find('t')
        {
            let mut numbers = body[..end].split(';').filter_map(|n| n.parse::<u16>().ok());
            if let (Some(height), Some(width)) = (numbers.next(), numbers.next())
                && height > 0
                && width > 0
            {
                answer.cell = Some((width, height));
            }
        } else if let Some(body) = part.strip_prefix('?')
            && let Some(end) = body.find('c')
        {
            answer.complete = true;
            answer.sixel = body[..end].split(';').any(|attribute| attribute == "4");
        }
    }
    answer
}

/// Ask the terminal how to draw pictures on this thread, reading stdin with a
/// bounded deadline so no orphaned reader thread can steal keys.
pub fn query_terminal(timeout: Duration) -> Picker {
    let answer = parse_answer(&ask(b"\x1b[16t\x1b[c", timeout));
    let font = answer
        .cell
        .map(|(width, height)| ratatui_image::FontSize::new(width, height));
    #[allow(deprecated)]
    let mut picker = match font {
        Some(font) => Picker::from_fontsize(font),
        None => Picker::halfblocks(),
    };
    if answer.sixel && font.is_some() {
        picker.set_protocol_type(ProtocolType::Sixel);
    } else {
        picker.set_protocol_type(ProtocolType::Halfblocks);
    }
    picker
}

fn ask(question: &[u8], timeout: Duration) -> Vec<u8> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Vec::new();
    }
    let mut out = std::io::stdout();
    if out.write_all(question).and_then(|()| out.flush()).is_err() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    let mut answer = Vec::new();
    while !parse_answer(&answer).complete {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut poll = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd that lives across the call.
        let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().max(1) as i32) };
        if ready <= 0 {
            break;
        }
        let mut chunk = [0u8; 256];
        // SAFETY: reads into the stack buffer, at most its length.
        let read = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if read <= 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..read as usize]);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cell_and_sixel_attributes() {
        let ans = parse_answer(b"\x1b[6;18;9t\x1b[?62;4;22c");
        assert_eq!(ans.cell, Some((9, 18)));
        assert!(ans.sixel);
        assert!(ans.complete);
    }
}
