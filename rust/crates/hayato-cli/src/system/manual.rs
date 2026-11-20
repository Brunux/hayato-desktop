//! The manual renderer shared by Learn and the Games library.
//!
//! The pages are markdown, and **glow** is what turns them into a document —
//! headings, real tables, inline code, lists — instead of source. It is the
//! same binary and the same `dark` style the Learn pages use. glow emits ANSI,
//! which [`parse_ansi`] maps back onto the design tokens, so the page follows
//! the session palette rather than glow's own colours.
//!
//! glow is a shipped dependency (`packages/tools.list`); [`plain`] is the
//! fallback for a machine where it is missing or fails, and it keeps the
//! headings, the lists and the wrapping so the page is still readable.

use std::io::Write;
use std::process::{Command, Stdio};

use hayato_ui::TextStyle;

/// One run of text with one design style: what a rendered markdown line is
/// made of.
#[derive(Clone, Debug)]
pub struct DocSpan {
    pub text: String,
    pub style: TextStyle,
}

/// Render markdown at a width. glow when it is there, the plain renderer
/// otherwise. The result is one `Vec<DocSpan>` per line.
pub fn render(markdown: &str, width: u16) -> Vec<Vec<DocSpan>> {
    glow(markdown, width).unwrap_or_else(|| plain(markdown, usize::from(width)))
}

fn glow(markdown: &str, width: u16) -> Option<Vec<Vec<DocSpan>>> {
    let width = width.max(20).to_string();
    let mut child = Command::new("glow")
        .args(["-s", "dark", "-w", &width, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(markdown.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let lines = parse_ansi(&text);
    (!lines.is_empty()).then_some(lines)
}

/// glow's ANSI back to spans: a bold run is a heading, an inline-code run is
/// `Code`, everything else is body text.
pub fn parse_ansi(ansi: &str) -> Vec<Vec<DocSpan>> {
    let mut lines = Vec::new();
    for raw_line in ansi.lines() {
        // A table row keeps glow's code padding: the padding is part of the
        // width glamour aligned the columns with, so trimming it would pull
        // the separators left by two cells per code run.
        let in_table = raw_line.contains('│');
        let mut spans: Vec<DocSpan> = Vec::new();
        let mut current_style = TextStyle::Body;
        let mut current_text = String::new();
        let mut chars = raw_line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' && chars.peek() == Some(&'[') {
                let _ = chars.next();
                let mut seq = String::new();
                while let Some(&c) = chars.peek() {
                    let _ = chars.next();
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                    seq.push(c);
                }
                if !current_text.is_empty() {
                    let text = std::mem::take(&mut current_text);
                    push_span(&mut spans, text, current_style, in_table);
                }
                if seq == "0" || seq.is_empty() {
                    current_style = TextStyle::Body;
                } else if seq.split(';').any(|part| part == "1") {
                    current_style = TextStyle::Title;
                } else if seq == ";" {
                    current_style = TextStyle::Code;
                }
            } else {
                current_text.push(ch);
            }
        }
        if !current_text.is_empty() {
            push_span(&mut spans, current_text, current_style, in_table);
        }
        // Trim the trailing spaces glow pads the line with.
        while let Some(last) = spans.last_mut() {
            let trimmed = last.text.trim_end_matches(' ');
            if trimmed.is_empty() {
                spans.pop();
            } else {
                last.text.truncate(trimmed.len());
                break;
            }
        }
        lines.push(spans);
    }
    lines
}

/// glow pads an inline-code run with one space on each side for its background
/// box. The box is dropped (the palette has no per-span background), so the
/// padding goes too, or every `path` would read with gaps around it.
fn push_span(spans: &mut Vec<DocSpan>, text: String, style: TextStyle, in_table: bool) {
    let text = if style == TextStyle::Code && !in_table {
        text.trim_matches(' ').to_string()
    } else {
        text
    };
    if !text.is_empty() {
        spans.push(DocSpan { text, style });
    }
}

/// The fallback: strip the markers, keep the structure. A table becomes its
/// cells separated by a rule, a fence becomes a box, a bullet stays a bullet.
fn plain(markdown: &str, width: usize) -> Vec<Vec<DocSpan>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            push_line(&mut lines, format!("  {line}"), TextStyle::Code, width);
            continue;
        }
        if trimmed.is_empty() {
            lines.push(Vec::new());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            push_line(&mut lines, rest.to_string(), TextStyle::Title, width);
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            push_line(&mut lines, rest.to_string(), TextStyle::Title, width);
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            push_line(&mut lines, rest.to_string(), TextStyle::Title, width);
        } else if trimmed.starts_with('|') {
            // A table row: drop the outer pipes and keep the cells apart.
            let cells: Vec<&str> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.chars().all(|c| c == '-' || c == ':'))
                .collect();
            if !cells.is_empty() {
                push_line(&mut lines, cells.join(" · "), TextStyle::Body, width);
            }
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            push_line(&mut lines, format!("• {rest}"), TextStyle::Body, width);
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            push_line(&mut lines, format!("• {rest}"), TextStyle::Body, width);
        } else {
            push_line(&mut lines, strip_markers(trimmed), TextStyle::Body, width);
        }
    }
    lines
}

fn push_line(lines: &mut Vec<Vec<DocSpan>>, text: String, style: TextStyle, width: usize) {
    for line in wrap(&text, width) {
        lines.push(vec![DocSpan { text: line, style }]);
    }
}

fn strip_markers(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

/// Word-wrap `text` to `width` display columns, keeping explicit newlines. A
/// word wider than the line is hard-broken. Shared with the Games hero, which
/// wraps a description the same way.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    let width = width.max(8);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut used = 0usize;
        for word in paragraph.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);
            if used > 0 && used + 1 + word_width <= width {
                line.push(' ');
                line.push_str(word);
                used += 1 + word_width;
                continue;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
            }
            if word_width <= width {
                line.push_str(word);
                used = word_width;
                continue;
            }
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for character in word.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(1);
                if chunk_width + character_width > width {
                    out.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(character);
                chunk_width += character_width;
            }
            line = chunk;
            used = chunk_width;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_is_split_into_styled_spans() {
        let lines = parse_ansi("plain \x1b[1;mbold\x1b[0m and \x1b[;mcode\x1b[0m\n");
        assert_eq!(lines.len(), 1);
        let styles: Vec<TextStyle> = lines[0].iter().map(|span| span.style).collect();
        assert!(styles.contains(&TextStyle::Title));
        assert!(styles.contains(&TextStyle::Code));
        let text: String = lines[0].iter().map(|span| span.text.as_str()).collect();
        assert_eq!(text, "plain bold and code");
    }

    #[test]
    fn inline_code_loses_glows_padding() {
        let lines = parse_ansi("put it in \x1b[;m bios/ \x1b[0m now\n");
        let text: String = lines[0].iter().map(|span| span.text.as_str()).collect();
        assert_eq!(text, "put it in bios/ now");
    }

    #[test]
    fn the_fallback_keeps_the_structure() {
        let lines = plain(
            "# Title\n\nA paragraph that is long enough to wrap at a narrow width.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            24,
        );
        let text: Vec<String> = lines
            .iter()
            .map(|spans| spans.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert!(text.iter().any(|line| line == "Title"));
        assert!(text.iter().any(|line| line == "• one"));
        assert!(text.iter().any(|line| line == "a · b"));
        assert!(text.iter().any(|line| line == "1 · 2"));
        for line in &text {
            assert!(
                unicode_width::UnicodeWidthStr::width(line.as_str()) <= 24,
                "{line:?} is wider than 24"
            );
        }
    }
}
