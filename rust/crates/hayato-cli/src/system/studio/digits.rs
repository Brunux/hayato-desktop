//! Zero-allocation 3-row pixel-hacker half-block (`▀▄█ `) digit renderer
//! for Studio photo countdowns (`3` → `2` → `1`) and live REC timers (`00:04:12`).

use hayato_ui::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::style::Style;

const GLYPH_0: [&str; 3] = ["▄▀▄", "█ █", "▀▄▀"];
const GLYPH_1: [&str; 3] = ["▄█ ", " █ ", "▄█▄"];
const GLYPH_2: [&str; 3] = ["▀▀▄", "▄▀ ", "▀▀▀"];
const GLYPH_3: [&str; 3] = ["▀▀▄", " ▀▄", "▀▀ "];
const GLYPH_4: [&str; 3] = ["█ █", "▀▀█", "  ▀"];
const GLYPH_5: [&str; 3] = ["█▀▀", "▀▀▄", "▀▀ "];
const GLYPH_6: [&str; 3] = ["▄▀▀", "█▀▄", "▀▄▀"];
const GLYPH_7: [&str; 3] = ["▀▀█", " ▄▀", " █ "];
const GLYPH_8: [&str; 3] = ["▄▀▄", "▄▀▄", "▀▄▀"];
const GLYPH_9: [&str; 3] = ["▄▀▄", "▀▀█", "▄▄▀"];
const GLYPH_COLON: [&str; 3] = [" ", "▄", "▀"];
const GLYPH_SPACE: [&str; 3] = [" ", " ", " "];

fn glyph_rows(ch: char) -> [&'static str; 3] {
    match ch {
        '0' => GLYPH_0,
        '1' => GLYPH_1,
        '2' => GLYPH_2,
        '3' => GLYPH_3,
        '4' => GLYPH_4,
        '5' => GLYPH_5,
        '6' => GLYPH_6,
        '7' => GLYPH_7,
        '8' => GLYPH_8,
        '9' => GLYPH_9,
        ':' => GLYPH_COLON,
        _ => GLYPH_SPACE,
    }
}

pub fn pixel_width(text: &str) -> u16 {
    let mut w: u16 = 0;
    for (idx, ch) in text.chars().enumerate() {
        if idx > 0 {
            w = w.saturating_add(1);
        }
        w = w.saturating_add(if ch == ':' || ch == ' ' { 1 } else { 3 });
    }
    w
}

/// Draw `text` (digits `0-9` and `:`) as 3-row half-block pixel numerals at `(x, y)`.
pub fn draw_pixel_digits(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) {
    let mut cx = x;
    let right = x.saturating_add(max_w);
    for ch in text.chars() {
        let rows = glyph_rows(ch);
        let cw: u16 = if ch == ':' || ch == ' ' { 1 } else { 3 };
        if cx.saturating_add(cw) > right {
            break;
        }
        for (dy, row) in rows.iter().enumerate() {
            let _ = put(buf, cx, y + dy as u16, row, cw, style);
        }
        cx = cx.saturating_add(cw + 1);
    }
}
