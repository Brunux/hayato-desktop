//! Translation surface (`translate`, `hayato menu translate`).
//!
//! Translates selected text or clipboard via Google / MyMemory fallback.
//! Supports `--languages`, `--target-name`, and `config` language selector.

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_card, draw_filter_bar,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use super::pick::{Question, row};
use super::{out, say};

const HIT_TRANSLATE: usize = usize::MAX;

pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("pt", "Portuguese"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("ar", "Arabic"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-CN", "Chinese (simplified)"),
    ("zh-TW", "Chinese (traditional)"),
    ("vi", "Vietnamese"),
    ("th", "Thai"),
    ("id", "Indonesian"),
    ("sv", "Swedish"),
    ("cs", "Czech"),
    ("el", "Greek"),
    ("ro", "Romanian"),
    ("fa", "Persian"),
];

pub fn name_of(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
        .unwrap_or("English")
}

pub fn config_file() -> PathBuf {
    let base_config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".config")
        });
    base_config.join("hayato/translate.conf")
}

pub fn target_code() -> String {
    if let Ok(content) = fs::read_to_string(config_file()) {
        for line in content.lines() {
            if let Some(val) = line.trim().strip_prefix("target=") {
                let code = val.trim();
                if LANGUAGES.iter().any(|(c, _)| *c == code) {
                    return code.to_string();
                }
            }
        }
    }
    "en".to_string()
}

pub fn set_target_code(code: &str) -> std::io::Result<()> {
    let p = config_file();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(p, format!("target={code}\n"))
}

/// Headless query: print language table.
pub fn print_languages() {
    for (code, name) in LANGUAGES {
        println!("{code}\t{name}");
    }
}

/// Headless query: print target language name.
pub fn print_target_name() {
    println!("{}", name_of(&target_code()));
}

/// Offline translation fallback via Hayato's local GGUF model (`llama-cli`).
pub fn query_local_translate(text: &str, target_name: &str) -> Option<(String, String)> {
    let prompt = format!(
        "Translate the following text into {target_name}. Output ONLY the translation, with no explanation:\n\n{text}\n\nTranslation:"
    );
    let translated = crate::ask::query_local(&prompt)?;
    Some((translated, "Local AI".to_string()))
}

/// Query Google translation API.
pub fn query_google(text: &str, target: &str) -> Option<(String, String)> {
    let output = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "8",
            "-G",
            "https://clients5.google.com/translate_a/t",
            "--data-urlencode",
            "client=dict-chrome-ex",
            "--data-urlencode",
            "sl=auto",
            "--data-urlencode",
            &format!("tl={target}"),
            "--data-urlencode",
            &format!("q={text}"),
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout);
    // Parse JSON array `[[translated_text, detected_code]]`
    let val: serde_json::Value = serde_json::from_str(&body).ok()?;
    let arr = val.as_array()?;
    let mut translated = String::new();
    let mut detected = String::new();
    for item in arr {
        if let Some(pair) = item.as_array() {
            if let Some(t) = pair.first().and_then(|v| v.as_str()) {
                translated.push_str(t);
            }
            if detected.is_empty() {
                if let Some(d) = pair.get(1).and_then(|v| v.as_str()) {
                    detected = d.to_string();
                }
            }
        }
    }
    if translated.trim().is_empty() {
        None
    } else {
        Some((translated, detected))
    }
}

/// Perform translation and notification.
pub fn run_translate() -> i32 {
    let target = target_code();
    let target_name = name_of(&target);

    // Primary selection, then clipboard
    let mut text = out(&["wl-paste", "-p", "-n"]).unwrap_or_default();
    if text.trim().is_empty() {
        text = out(&["wl-paste", "-n"]).unwrap_or_default();
    }
    let text = text.trim();
    if text.is_empty() {
        say(
            "Translate",
            format!("No text selected to translate into {target_name}"),
            Urgency::Normal,
            Glyph::Globe,
        );
        return 0;
    }

    let capped_text: String = text.chars().take(3000).collect();

    say("Translate", "Translating…", Urgency::Normal, Glyph::Globe);

    let result = query_google(&capped_text, &target)
        .or_else(|| query_local_translate(&capped_text, target_name));
    if let Some((translated, detected)) = result {
        let _ = Command::new("wl-copy").arg(&translated).status();
        let src_name = if detected == "Local AI" {
            "Local AI"
        } else {
            name_of(&detected)
        };
        let title = format!("{src_name} → {target_name}");
        say(&title, &translated, Urgency::Normal, Glyph::Globe);

        if translated.len() > 400 || translated.lines().count() > 3 {
            // Open floated viewer
            let runtime_dir =
                std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
            let doc = std::path::Path::new(&runtime_dir)
                .join(format!("hayato-translate.{}.txt", std::process::id()));
            let content = format!("{title}\n\n{translated}\n\n── what was selected ──\n{text}\n");
            if std::fs::write(&doc, content).is_ok() {
                let escaped_title = title.replace('\'', "'\\''");
                let escaped_doc = doc.display().to_string().replace('\'', "'\\''");
                let script = format!(
                    "exec foot --app-id=popup --title='{escaped_title}' -e sh -c 'env BAT_THEME=ansi bat --paging=always --style=plain --file-name=\"$1\" -- \"$2\"; rm -f \"$2\"' _ '{escaped_title}' '{escaped_doc}'"
                );
                crate::system::spawn_detached(&script);
            }
        }
        0
    } else {
        say(
            "Translate",
            "No answer from translator (check connection)",
            Urgency::Critical,
            Glyph::Warning,
        );
        1
    }
}

/// Interactive language configuration picker: `hayato menu translate config`.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if args.first().map(String::as_str) == Some("config") {
        return menu_config(terminal, design);
    }
    Ok(run_translate())
}

/// The one question of `hayato menu translate config`, built from data so a
/// snapshot can draw it without the saved target language.
pub fn config_questions(current: &str, note: &str) -> Vec<Question> {
    let mut rows = Vec::new();
    let mut keys = Vec::new();

    for (code, name) in LANGUAGES {
        let is_cur = *code == current;
        let mut r = row(*name, *code, Glyph::Globe);
        if is_cur {
            r = r.value("● current");
        }
        rows.push(r);
        keys.push(code.to_string());
    }

    vec![Question::choose(
        "Translate Target Language",
        note,
        rows,
        keys,
    )]
}

/// Interactive Translation Studio screen.
pub struct TranslateScreen {
    current: String,
    list: ListState,
    clipboard_sample: String,
    translated: Option<(String, String)>,
    banner: Option<(String, bool)>,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    translate_btn: Cell<Rect>,
}

impl Default for TranslateScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslateScreen {
    pub fn new() -> Self {
        let current = target_code();
        let clip = out(&["wl-paste", "-n"])
            .unwrap_or_default()
            .trim()
            .chars()
            .take(500)
            .collect::<String>();
        let mut screen = Self {
            current,
            list: ListState::new().with_empty("no matching languages"),
            clipboard_sample: clip,
            translated: None,
            banner: None,
            header: Header::new("Hayato").subtitle("Translate Studio"),
            keybar: vec![
                KeyHint::new("Enter", "set target"),
                KeyHint::new("Ctrl+T", "translate clipboard"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(18),
            translate_btn: Cell::new(Rect::default()),
        };
        screen.rebuild_rows();
        screen
    }

    fn rebuild_rows(&mut self) {
        let rows: Vec<Row> = LANGUAGES
            .iter()
            .map(|(code, name)| {
                let mut r = Row::new(*name).glyph(Glyph::Globe).detail(*code);
                if *code == self.current {
                    r = r.value("● current");
                }
                r
            })
            .collect();
        self.list.update_rows(rows);
        self.header = Header::new("Hayato")
            .subtitle("Translate Studio")
            .status(format!(
                "Auto-detect → {} ({})",
                name_of(&self.current),
                self.current
            ));
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        super::split_panes(area, 44, 40)
    }

    fn left_layout(area: Rect) -> (Rect, Rect) {
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        let filter = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(2).min(inner.bottom()),
            inner.width,
            inner.height.saturating_sub(2),
        );
        (filter, rows)
    }

    fn select_current(&mut self) {
        if let Some(idx) = self.list.selected_index()
            && let Some((code, name)) = LANGUAGES.get(idx)
        {
            if set_target_code(code).is_ok() {
                self.current = (*code).to_string();
                self.banner = Some((format!("Target language set to {name} ({code})"), false));
                self.rebuild_rows();
            }
        }
    }

    fn translate_clipboard_now(&mut self) {
        let text = self.clipboard_sample.trim();
        if text.is_empty() {
            self.banner = Some(("Clipboard is empty — copy text first".into(), true));
            return;
        }
        let target_name = name_of(&self.current);
        let result =
            query_google(text, &self.current).or_else(|| query_local_translate(text, target_name));
        match result {
            Some((out_text, detected)) => {
                let _ = Command::new("wl-copy").arg(&out_text).status();
                let src = if detected == "Local AI" {
                    "Local AI".to_string()
                } else {
                    name_of(&detected).to_string()
                };
                self.translated = Some((src.clone(), out_text));
                self.banner = Some((
                    format!("Translated {src} → {target_name} & copied to clipboard"),
                    false,
                ));
            }
            None => {
                self.banner = Some(("Could not reach translator (check network)".into(), true));
            }
        }
    }

    fn draw_translate_pill(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        x: u16,
        y: u16,
        max_w: u16,
        design: &Design,
    ) {
        let label = " ^T Translate clipboard ";
        let pill_w = (label.len() as u16).min(max_w);
        if pill_w < 8 {
            return;
        }
        let pill_style = Style::default()
            .fg(design.color(ColorRole::KeyText))
            .bg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        put(buf, x, y, label, pill_w, pill_style);
        self.translate_btn.set(Rect::new(x, y, pill_w, 1));
    }

    fn draw_workbench(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Live Translator"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 16 || inner.height < 8 {
            return;
        }
        let dim = design.style(TextStyle::Dim);
        let body = design.style(TextStyle::Body);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let ok = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);

        let mut y = inner.y;
        put_trunc(
            buf,
            inner.x,
            y,
            &format!(
                "Auto-detect  →  {} ({})",
                name_of(&self.current),
                self.current
            ),
            inner.width,
            accent,
        );
        y += 2;

        put_trunc(buf, inner.x, y, "Clipboard Source", inner.width, dim);
        y += 1;
        let w = usize::from(inner.width.max(1));
        if self.clipboard_sample.is_empty() {
            put_trunc(buf, inner.x, y, "(clipboard is empty)", inner.width, dim);
            y += 2;
        } else {
            for line in super::manual::wrap(&self.clipboard_sample, w)
                .into_iter()
                .take(4)
            {
                if y >= inner.bottom().saturating_sub(6) {
                    break;
                }
                put_trunc(buf, inner.x, y, &line, inner.width, body);
                y += 1;
            }
            y += 1;
        }

        if let Some((src, out_text)) = &self.translated {
            put_trunc(
                buf,
                inner.x,
                y,
                &format!("Translation ({src} → {})", name_of(&self.current)),
                inner.width,
                ok,
            );
            y += 1;
            for line in super::manual::wrap(out_text, w) {
                if y >= inner.bottom().saturating_sub(3) {
                    break;
                }
                put_trunc(buf, inner.x, y, &line, inner.width, body);
                y += 1;
            }
        } else if y < inner.bottom().saturating_sub(3) {
            put_trunc(
                buf,
                inner.x,
                y,
                "Press Ctrl+T to translate clipboard text live here.",
                inner.width,
                dim,
            );
        }

        let footer_y = inner.bottom().saturating_sub(1);
        if inner.height >= 10 {
            self.draw_translate_pill(
                buf,
                inner.x,
                footer_y.saturating_sub(1),
                inner.width,
                design,
            );
        }
        put_trunc(
            buf,
            inner.x,
            footer_y,
            "Enter saves · Ctrl+T translates & copies",
            inner.width,
            dim,
        );
    }
}

impl Screen for TranslateScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        self.translate_btn.set(Rect::default());
        let (left, right) = Self::split_panes(ctx.area);
        draw_card(ctx.buf, left, ctx.design, Some("Target Language"), true);
        let (filter, mut rows) = Self::left_layout(left);
        let default_msg = format!(
            "Everything goes into {} · Enter sets · Ctrl+T translates",
            name_of(&self.current)
        );
        let msg = self
            .banner
            .as_ref()
            .map(|(m, _)| m.as_str())
            .unwrap_or(&default_msg);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            msg,
            self.list.visible_len(),
            LANGUAGES.len(),
        );
        if right.is_none() && rows.height > 4 {
            rows.height = rows.height.saturating_sub(1);
            let pill_y = rows.bottom();
            self.draw_translate_pill(ctx.buf, rows.x, pill_y, rows.width, ctx.design);
            if let Some((src, out_text)) = &self.translated
                && rows.height > 3
            {
                rows.height = rows.height.saturating_sub(1);
                let preview_y = rows.bottom();
                let first = out_text.lines().next().unwrap_or("");
                put_trunc(
                    ctx.buf,
                    rows.x,
                    preview_y,
                    &format!("{src} → {}: {first}", name_of(&self.current)),
                    rows.width,
                    Style::default().fg(ctx.design.color(ColorRole::Ok)),
                );
            }
        }
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);

        if let Some(right_area) = right {
            self.draw_workbench(ctx.buf, right_area, ctx.design);
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        {
            self.translate_clipboard_now();
            return Outcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => {
                self.select_current();
                return Outcome::Consumed;
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(_) => {
                self.select_current();
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let btn = self.translate_btn.get();
        if btn.width > 0 && btn.height > 0 && btn.contains(Position { x, y }) {
            return Some(HIT_TRANSLATE);
        }
        let (left, _) = Self::split_panes(area);
        let (_, mut rows) = Self::left_layout(left);
        if area.width < super::DUAL_PANE_MIN_WIDTH && rows.height > 4 {
            rows.height = rows.height.saturating_sub(1);
            if self.translated.is_some() && rows.height > 3 {
                rows.height = rows.height.saturating_sub(1);
            }
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(r) = row {
            if r == HIT_TRANSLATE {
                self.translate_clipboard_now();
                return Outcome::Consumed;
            }
            self.list.set_cursor(r);
            self.select_current();
            return Outcome::Consumed;
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("translate.studio")
    }
}

pub fn menu_config(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = TranslateScreen::new();
    crate::run_screen(terminal, design, "Translate", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_contains_core_set() {
        assert_eq!(name_of("en"), "English");
        assert_eq!(name_of("es"), "Spanish");
        assert_eq!(name_of("fr"), "French");
    }

    #[test]
    fn unknown_language_code_falls_back_to_english() {
        assert_eq!(name_of("xx"), "English");
    }
}
