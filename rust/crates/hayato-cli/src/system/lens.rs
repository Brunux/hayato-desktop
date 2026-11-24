//! Hayato Lens (`hayato-lens`, `hayato menu lens`): local "Circle to Search".
//!
//! Captures a screen region (`slurp` + `grim`), extracts text via `tesseract`
//! and QR/barcodes via `zbarimg`, detects entities (URLs, code/errors, plain
//! text), and opens an interactive action studio for one-key / touch actions:
//! copy, open URL, clean URL, translate (online or local offline AI), save to
//! `~/Documents/Notes/Lens.md`, or render as QR.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, QrCode,
    Row, Screen, TextStyle, draw_badge, draw_card, draw_qr,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::clip::clean_url;
use super::translate::{name_of, query_google, query_local_translate, target_code};
use super::{have, out, say};

/// What kind of content Lens detected in the captured region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensKind {
    Url,
    Code,
    Text,
}

impl LensKind {
    fn badge(self) -> (&'static str, ColorRole) {
        match self {
            Self::Url => ("LINK", ColorRole::Accent),
            Self::Code => ("CODE / LOG", ColorRole::Warn),
            Self::Text => ("TEXT", ColorRole::Ok),
        }
    }
}

/// Extract the first `http://` or `https://` URL from text, trimming trailing
/// punctuation that commonly wraps links in prose.
pub fn extract_first_url(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let stripped = token.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | ',' | ';' | '.'
            )
        });
        if (stripped.starts_with("https://") || stripped.starts_with("http://"))
            && stripped.len() > 10
        {
            return Some(stripped.to_string());
        }
    }
    None
}

/// Classify captured text into URL, Code/Log, or plain Text.
pub fn classify_text(text: &str) -> LensKind {
    if extract_first_url(text).is_some() {
        return LensKind::Url;
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("error[")
        || lower.contains("traceback")
        || lower.contains("exception")
        || lower.contains("panic")
        || text.contains("::")
        || text.contains("sudo ")
        || text.contains("cargo ")
    {
        return LensKind::Code;
    }
    LensKind::Text
}

/// Capture a screen region using `slurp` + `grim`, then run `zbarimg` and
/// `tesseract` on the PNG. Returns `Ok(None)` if the user cancelled `slurp`.
fn capture_region() -> Result<Option<(String, &'static str)>, String> {
    if !have("slurp") || !have("grim") {
        return Err("slurp and grim are required for screen capture".to_string());
    }
    let geom = Command::new("slurp")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Could not run slurp: {e}"))?;
    if !geom.status.success() {
        return Ok(None);
    }
    let region = String::from_utf8_lossy(&geom.stdout).trim().to_string();
    if region.is_empty() {
        return Ok(None);
    }
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let png = PathBuf::from(&runtime_dir).join(format!("hayato-lens-{}.png", std::process::id()));
    let grim_ok = Command::new("grim")
        .args(["-g", &region])
        .arg(&png)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !grim_ok {
        let _ = std::fs::remove_file(&png);
        return Err("grim failed to capture the selected region".to_string());
    }

    // Try QR / barcode scan first (instant and exact).
    if have("zbarimg")
        && let Ok(out_qr) = Command::new("zbarimg")
            .args(["--quiet", "--raw"])
            .arg(&png)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        && out_qr.status.success()
    {
        let qr_text = String::from_utf8_lossy(&out_qr.stdout).trim().to_string();
        if !qr_text.is_empty() {
            let _ = std::fs::remove_file(&png);
            return Ok(Some((qr_text, "QR / Barcode")));
        }
    }

    // Fall back to Tesseract OCR.
    if !have("tesseract") {
        let _ = std::fs::remove_file(&png);
        return Err("tesseract is not installed for OCR".to_string());
    }
    let ocr = Command::new("tesseract")
        .arg(&png)
        .arg("stdout")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Could not run tesseract: {e}"))?;
    let _ = std::fs::remove_file(&png);
    let text = String::from_utf8_lossy(&ocr.stdout).trim().to_string();
    Ok(Some((text, "OCR")))
}

fn copy_text(text: &str) -> bool {
    Command::new("wl-copy")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub struct LensScreen {
    text: String,
    source: &'static str,
    kind: LensKind,
    url: Option<String>,
    translated: Option<String>,
    show_qr: Option<QrCode>,
    status: String,
    list: ListState,
    keys: Vec<&'static str>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
}

impl LensScreen {
    pub fn new(text: String, source: &'static str) -> Self {
        let kind = classify_text(&text);
        let url = extract_first_url(&text);
        let mut screen = Self {
            text,
            source,
            kind,
            url,
            translated: None,
            show_qr: None,
            status: format!("Captured via {source} · Enter runs action"),
            list: ListState::new(),
            keys: Vec::new(),
            header: Header::new("Hayato").subtitle("Lens"),
            keybar: vec![
                KeyHint::new("Enter", "run action"),
                KeyHint::new("c", "copy"),
                KeyHint::new("t", "translate"),
                KeyHint::new("n", "note"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(14),
        };
        screen.rebuild_rows();
        screen
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        rows.push(
            Row::new("Copy captured text")
                .glyph(Glyph::Clipboard)
                .detail("copy to clipboard (wl-copy)")
                .value("c"),
        );
        keys.push("copy");

        if let Some(url) = &self.url {
            rows.push(
                Row::new("Open detected URL")
                    .glyph(Glyph::Globe)
                    .detail(url.clone())
                    .value("o"),
            );
            keys.push("open-url");
            let cleaned = clean_url(url);
            if cleaned != *url {
                rows.push(
                    Row::new("Copy clean URL")
                        .glyph(Glyph::Link)
                        .detail("strip tracking parameters")
                        .value("u"),
                );
                keys.push("clean-url");
            }
        }

        let target = target_code();
        rows.push(
            Row::new(format!("Translate → {}", name_of(&target)))
                .glyph(Glyph::Globe)
                .detail("online or local offline AI fallback")
                .value("t"),
        );
        keys.push("translate");

        rows.push(
            Row::new("Save to Notes")
                .glyph(Glyph::Book)
                .detail("append to ~/Documents/Notes/Lens.md")
                .value("n"),
        );
        keys.push("note");

        rows.push(
            Row::new("Show as QR code")
                .glyph(Glyph::Qr)
                .detail("render scannable QR in preview pane")
                .value("q"),
        );
        keys.push("qr");

        self.keys = keys;
        self.list.set_rows(rows);
        self.header = Header::new("Hayato")
            .subtitle("Lens")
            .status(self.status.clone());
    }

    fn run_action(&mut self, action: &str) -> Outcome {
        match action {
            "copy" => {
                if copy_text(&self.text) {
                    self.status = "Copied captured text to clipboard".to_string();
                } else {
                    self.status = "Could not copy (wl-copy failed)".to_string();
                }
            }
            "open-url" => {
                if let Some(url) = &self.url {
                    crate::system::spawn_detached_argv("xdg-open", &[url]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
            "clean-url" => {
                if let Some(url) = &self.url {
                    let cleaned = clean_url(url);
                    let _ = copy_text(&cleaned);
                    self.status = format!("Copied clean URL: {cleaned}");
                }
            }
            "translate" => {
                let code = target_code();
                let tname = name_of(&code);
                let sample: String = self.text.chars().take(1500).collect();
                let res =
                    query_google(&sample, &code).or_else(|| query_local_translate(&sample, tname));
                match res {
                    Some((translated, src)) => {
                        let _ = copy_text(&translated);
                        self.translated = Some(translated);
                        self.show_qr = None;
                        self.status = format!("Translated ({src} → {tname}) and copied");
                    }
                    None => {
                        self.status = "Translation unavailable (offline & no local model)".into();
                    }
                }
            }
            "note" => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let dir = PathBuf::from(home).join("Documents/Notes");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join("Lens.md");
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M");
                let block = format!("\n## {ts} ({})\n\n{}\n", self.source, self.text);
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(block.as_bytes()))
                {
                    Ok(()) => self.status = "Saved to ~/Documents/Notes/Lens.md".into(),
                    Err(e) => self.status = format!("Could not save note: {e}"),
                }
            }
            "qr" => {
                if self.show_qr.is_some() {
                    self.show_qr = None;
                    self.status = "Showing captured text".into();
                } else {
                    let payload = self.url.as_deref().unwrap_or(&self.text);
                    if let Some(qr) = QrCode::encode(payload) {
                        self.show_qr = Some(qr);
                        self.status = "Scan QR with your phone camera".into();
                    } else {
                        self.status = "Captured text is too large for a QR code".into();
                    }
                }
            }
            _ => {}
        }
        self.header = Header::new("Hayato")
            .subtitle("Lens")
            .status(self.status.clone());
        Outcome::Consumed
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= 90 {
            let left_w = (u32::from(area.width) * 44 / 100).max(36) as u16;
            let right_w = area.width.saturating_sub(left_w + 1);
            (
                Rect::new(area.x, area.y, left_w, area.height),
                Some(Rect::new(area.x + left_w + 1, area.y, right_w, area.height)),
            )
        } else {
            (area, None)
        }
    }

    fn draw_preview(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let title = if self.show_qr.is_some() {
            "QR Preview"
        } else if self.translated.is_some() {
            "Translation"
        } else {
            "Captured Content"
        };
        let inner = draw_card(buf, area, design, Some(title), false);
        if inner.width < 8 || inner.height < 3 {
            return;
        }
        if let Some(qr) = &self.show_qr {
            draw_qr(buf, inner, design, qr);
            return;
        }
        let (badge, role) = self.kind.badge();
        let x = draw_badge(buf, inner.x, inner.y, design, badge, role);
        let _ = put(
            buf,
            x + 1,
            inner.y,
            self.source,
            inner.right().saturating_sub(x + 1),
            design.style(TextStyle::Dim),
        );
        let body = self.translated.as_deref().unwrap_or(&self.text);
        for (y, line) in (inner.y + 2..inner.bottom()).zip(body.lines()) {
            put_trunc(
                buf,
                inner.x,
                y,
                line,
                inner.width,
                design.style(TextStyle::Body),
            );
        }
    }
}

impl Screen for LensScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((920, 520))
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        let inner = draw_card(ctx.buf, left, ctx.design, Some("Lens Actions"), true);
        self.rows_height.set(inner.height);
        self.list.render(inner, ctx.buf, ctx.design, true);
        if let Some(right_area) = right {
            self.draw_preview(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc => return Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Char('c') => return self.run_action("copy"),
            KeyCode::Char('o') if self.url.is_some() => return self.run_action("open-url"),
            KeyCode::Char('u') if self.url.is_some() => return self.run_action("clean-url"),
            KeyCode::Char('t') => return self.run_action("translate"),
            KeyCode::Char('n') => return self.run_action("note"),
            KeyCode::Char('q') => return self.run_action("qr"),
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(idx) => {
                if let Some(&act) = self.keys.get(idx) {
                    self.run_action(act)
                } else {
                    Outcome::Continue
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (left, _) = Self::split_panes(area);
        let inner = Rect::new(
            left.x + 2,
            left.y + 1,
            left.width.saturating_sub(4),
            left.height.saturating_sub(2),
        );
        self.list.hit_row(inner, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            self.list.set_cursor(r);
            if let Some(idx) = self.list.selected_index()
                && let Some(&act) = self.keys.get(idx)
            {
                return self.run_action(act);
            }
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

/// Headless pre-capture entry point used when launched from a Sway keybinding:
/// runs `slurp` + `grim` + `zbarimg`/`tesseract` *before* any floating `foot`
/// window opens so the screen stays 100% unobstructed during region selection.
pub fn capture_and_launch() -> i32 {
    match capture_region() {
        Ok(None) => hayato_ui::app::exit::ESCAPE,
        Ok(Some((text, source))) => {
            if text.is_empty() {
                say(
                    "Hayato Lens",
                    "No text or QR code found in the selected region",
                    hayato_ui::Urgency::Normal,
                    Glyph::Eye,
                );
                return hayato_ui::app::exit::FAILED;
            }
            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "hayato".to_string());
            let _ = Command::new(&exe)
                .args(["menu", "lens", "--source", source, "--text", &text])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            hayato_ui::app::exit::OK
        }
        Err(err) => {
            say("Hayato Lens", err, hayato_ui::Urgency::Critical, Glyph::Eye);
            hayato_ui::app::exit::FAILED
        }
    }
}

/// `hayato menu lens` (`hayato-lens`): capture region or inspect clipboard text.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let explicit_source: Option<&'static str> = args
        .iter()
        .position(|a| a == "--source")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| match s.as_str() {
            "QR / Barcode" => "QR / Barcode",
            "OCR" => "OCR",
            "Clipboard" => "Clipboard",
            _ => "Text",
        });
    let (text, source) = if let Some(pos) = args.iter().position(|a| a == "--text")
        && let Some(val) = args.get(pos + 1)
    {
        (val.clone(), explicit_source.unwrap_or("Text"))
    } else if args.iter().any(|a| a == "--clipboard") {
        let clip = out(&["wl-paste", "-n"]).unwrap_or_default();
        (clip.trim().to_string(), "Clipboard")
    } else {
        match capture_region() {
            Ok(Some((t, src))) => (t, src),
            Ok(None) => return Ok(hayato_ui::app::exit::ESCAPE),
            Err(e) => return Err(crate::Failure::new("Hayato Lens", e)),
        }
    };
    if text.is_empty() {
        return Err(crate::Failure::new(
            "Hayato Lens",
            "No text or QR code was recognized in the selected region.",
        ));
    }
    let mut screen = LensScreen::new(text, source);
    crate::run_screen(terminal, design, "Hayato Lens", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_classifies_urls_and_code() {
        assert_eq!(
            extract_first_url("Visit (https://hayato.dev/docs?utm_source=x)."),
            Some("https://hayato.dev/docs?utm_source=x".to_string())
        );
        assert_eq!(
            classify_text("See https://example.com/a for details"),
            LensKind::Url
        );
        assert_eq!(
            classify_text("error[E0308]: mismatched types"),
            LensKind::Code
        );
        assert_eq!(classify_text("Hello world from Hayato"), LensKind::Text);
    }
}
