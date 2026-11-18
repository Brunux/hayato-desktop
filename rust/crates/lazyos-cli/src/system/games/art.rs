//! The shelf's own artwork: a 5×5 block alphabet for the hero's monogram, the
//! cartridge in front of each card, and the text helpers a card needs.
//!
//! Nothing here is an asset. The art is derived from the game's own title, so a
//! library with no cover files still looks like a shelf, and a theme switch
//! repaints it like everything else.

use lazyos_ui::{ColorRole, Design};
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The letter a game's cartridge and monogram show: the first letter of its
/// title, upper-cased, or `?` when the title has none.
pub fn initial(title: &str) -> char {
    title
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_ascii_uppercase())
        .unwrap_or('?')
}

/// A 5×5 bitmap font, `#` for a lit cell. Only the characters a title can
/// start with are here; anything else draws as a question mark.
pub fn monogram(character: char) -> [&'static str; 5] {
    match character.to_ascii_uppercase() {
        'A' => [" ### ", "#   #", "#####", "#   #", "#   #"],
        'B' => ["#### ", "#   #", "#### ", "#   #", "#### "],
        'C' => [" ####", "#    ", "#    ", "#    ", " ####"],
        'D' => ["#### ", "#   #", "#   #", "#   #", "#### "],
        'E' => ["#####", "#    ", "#### ", "#    ", "#####"],
        'F' => ["#####", "#    ", "#### ", "#    ", "#    "],
        'G' => [" ####", "#    ", "#  ##", "#   #", " ####"],
        'H' => ["#   #", "#   #", "#####", "#   #", "#   #"],
        'I' => ["#####", "  #  ", "  #  ", "  #  ", "#####"],
        'J' => ["  ###", "   # ", "   # ", "#  # ", " ##  "],
        'K' => ["#   #", "#  # ", "###  ", "#  # ", "#   #"],
        'L' => ["#    ", "#    ", "#    ", "#    ", "#####"],
        'M' => ["#   #", "## ##", "# # #", "#   #", "#   #"],
        'N' => ["#   #", "##  #", "# # #", "#  ##", "#   #"],
        'O' => [" ### ", "#   #", "#   #", "#   #", " ### "],
        'P' => ["#### ", "#   #", "#### ", "#    ", "#    "],
        'Q' => [" ### ", "#   #", "# # #", "#  # ", " ## #"],
        'R' => ["#### ", "#   #", "#### ", "#  # ", "#   #"],
        'S' => [" ####", "#    ", " ### ", "    #", "#### "],
        'T' => ["#####", "  #  ", "  #  ", "  #  ", "  #  "],
        'U' => ["#   #", "#   #", "#   #", "#   #", " ### "],
        'V' => ["#   #", "#   #", "#   #", " # # ", "  #  "],
        'W' => ["#   #", "#   #", "# # #", "## ##", "#   #"],
        'X' => ["#   #", " # # ", "  #  ", " # # ", "#   #"],
        'Y' => ["#   #", " # # ", "  #  ", "  #  ", "  #  "],
        'Z' => ["#####", "   # ", "  #  ", " #   ", "#####"],
        '0' => [" ### ", "#  ##", "# # #", "##  #", " ### "],
        '1' => ["  #  ", " ##  ", "  #  ", "  #  ", "#####"],
        '2' => ["#### ", "    #", " ### ", "#    ", "#####"],
        '3' => ["#### ", "    #", " ### ", "    #", "#### "],
        '4' => ["#   #", "#   #", "#####", "    #", "    #"],
        '5' => ["#####", "#    ", "#### ", "    #", "#### "],
        '6' => [" ### ", "#    ", "#### ", "#   #", " ### "],
        '7' => ["#####", "    #", "   # ", "  #  ", "  #  "],
        '8' => [" ### ", "#   #", " ### ", "#   #", " ### "],
        '9' => [" ### ", "#   #", " ####", "    #", " ### "],
        _ => ["     ", " ### ", " # # ", " ### ", "     "],
    }
}

/// The hero's monogram: the 5×5 bitmap, every lit cell doubled, and doubled
/// again in both directions when the hero is tall enough for a big letter.
/// The blank cells are written too, so the letter keeps its shape over a card.
pub fn draw_monogram(buf: &mut Buffer, x: u16, y: u16, design: &Design, letter: char, scale: u16) {
    let scale = scale.clamp(1, 2);
    let background = design.color(ColorRole::Bg);
    let lit = Style::default()
        .fg(design.color(ColorRole::Accent))
        .bg(background)
        .add_modifier(Modifier::BOLD);
    let blank = Style::default().bg(background);
    for (row, pattern) in monogram(letter).iter().enumerate() {
        for (column, cell) in pattern.chars().enumerate() {
            let style = if cell == '#' { lit } else { blank };
            let mut text = String::with_capacity(4);
            for _ in 0..scale {
                text.push_str(if cell == '#' { "██" } else { "  " });
            }
            for line in 0..scale {
                buf.set_string(
                    x + column as u16 * 2 * scale,
                    y + row as u16 * scale + line,
                    &text,
                    style,
                );
            }
        }
    }
}

/// The cartridge in front of a shelf card: five columns, three rows, with the
/// game's initial on the label.
pub fn draw_cartridge(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    design: &Design,
    letter: char,
    selected: bool,
) {
    let background = if selected {
        design.color(ColorRole::Selection)
    } else {
        design.color(ColorRole::Bg)
    };
    let border = Style::default()
        .fg(design.color(if selected {
            ColorRole::Accent
        } else {
            ColorRole::SurfaceAlt
        }))
        .bg(background);
    let label = Style::default()
        .fg(design.color(if selected {
            ColorRole::Accent
        } else {
            ColorRole::Text
        }))
        .bg(background)
        .add_modifier(Modifier::BOLD);
    buf.set_string(x, y, "╭───╮", border);
    buf.set_string(x, y + 1, "│", border);
    buf.set_string(x + 1, y + 1, " ", border);
    buf.set_string(x + 2, y + 1, &letter.to_string(), label);
    buf.set_string(x + 3, y + 1, " │", border);
    buf.set_string(x, y + 2, "╰───╯", border);
}

/// `text`, cut to `width` cells with an ellipsis when it does not fit.
pub fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let cell = UnicodeWidthChar::width(character).unwrap_or(1);
        if used + cell > width.saturating_sub(1) {
            break;
        }
        out.push(character);
        used += cell;
    }
    out.push('…');
    out
}

/// Bytes as a person reads them: `512 KB`, `24.1 MB`, `1.2 GB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

/// When a game was last played, in words. `now` is passed in so the tests do
/// not depend on the clock.
pub fn ago(last: i64, now: i64) -> String {
    if last <= 0 {
        return "never played".to_string();
    }
    let seconds = now.saturating_sub(last).max(0);
    match seconds {
        0..=90 => "just now".to_string(),
        _ if seconds < 3600 => format!("{} min ago", seconds / 60),
        _ if seconds < 86_400 => format!("{} h ago", seconds / 3600),
        86_400..=172_800 => "yesterday".to_string(),
        _ if seconds < 30 * 86_400 => format!("{} days ago", seconds / 86_400),
        _ if seconds < 365 * 86_400 => format!("{} months ago", seconds / (30 * 86_400)),
        _ => format!("{} years ago", seconds / (365 * 86_400)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_letter_and_digit_has_five_rows_of_five_cells() {
        for character in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            for row in monogram(character) {
                assert_eq!(
                    row.chars().count(),
                    5,
                    "{character} has a row that is not five cells: {row:?}"
                );
            }
        }
        assert_eq!(monogram('?').len(), 5);
    }

    #[test]
    fn the_initial_is_the_first_letter_of_the_title() {
        assert_eq!(initial("Snake"), 'S');
        assert_eq!(initial("  metal slug"), 'M');
        assert_eq!(initial("1943"), '1');
        assert_eq!(initial("!!!"), '?');
    }

    #[test]
    fn fit_cuts_with_an_ellipsis_and_never_overflows() {
        assert_eq!(fit("Snake", 10), "Snake");
        assert_eq!(fit("Metal Slug", 6), "Metal…");
        assert_eq!(fit("Metal Slug", 0), "");
        for width in 1..12 {
            assert!(UnicodeWidthStr::width(fit("Metal Slug", width).as_str()) <= width);
        }
    }

    #[test]
    fn sizes_and_times_read_like_a_person_wrote_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1536), "2 KB");
        assert_eq!(human_size(24 * 1024 * 1024), "24.0 MB");
        assert_eq!(ago(0, 100), "never played");
        assert_eq!(ago(100, 110), "just now");
        assert_eq!(ago(100, 100 + 600), "10 min ago");
        assert_eq!(ago(100, 100 + 7200), "2 h ago");
        assert_eq!(ago(100, 100 + 86_400), "yesterday");
        assert_eq!(ago(100, 100 + 3 * 86_400), "3 days ago");
    }
}
