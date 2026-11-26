//! Audio output, microphones & per-app mixer (`bar-audio`, `hayato menu audio`).
//!
//! Displays a live Output & Microphone hero card with `draw_gauge` volume bars
//! and `[DEFAULT]` / `[MUTED]` badges, lists all PipeWire/PulseAudio sinks,
//! microphones, and active per-application audio streams (`sink-inputs`), and
//! supports inline `Left`/`Right` (`-`/`+`) volume adjustment, `Ctrl+M`/`Ctrl+I`
//! mute toggling, default sink switching, and per-app output routing — all
//! natively in-window without spawning `pavucontrol`.

use std::cell::Cell;
use std::process::{Command, Stdio};

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    ActionItem, ActionStrip, ActionStripEvent, BodyCtx, ColorRole, Design, Glyph, Header, KeyHint,
    ListAction, ListState, Outcome, Row, Screen, TextStyle, draw_badge, draw_card, draw_filter_bar,
    draw_gauge,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["pactl"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SinkInput {
    pub id: String,
    pub sink_index: String,
    pub app_name: String,
    pub media_name: String,
    pub volume: u8,
    pub muted: bool,
}

/// Parse `pactl list sink-inputs` output into per-application streams.
pub fn parse_sink_inputs(output: &str) -> Vec<SinkInput> {
    let mut inputs = Vec::new();
    let mut id = String::new();
    let mut sink_index = String::new();
    let mut app_name = String::new();
    let mut media_name = String::new();
    let mut muted = false;
    let mut volume = 100u8;

    let flush = |inputs: &mut Vec<SinkInput>,
                 id: &mut String,
                 sink_index: &mut String,
                 app_name: &mut String,
                 media_name: &mut String,
                 muted: &mut bool,
                 volume: &mut u8| {
        if !id.is_empty() {
            let app = if app_name.is_empty() {
                format!("Stream #{}", id)
            } else {
                std::mem::take(app_name)
            };
            inputs.push(SinkInput {
                id: std::mem::take(id),
                sink_index: std::mem::take(sink_index),
                app_name: app,
                media_name: std::mem::take(media_name),
                volume: *volume,
                muted: *muted,
            });
            *muted = false;
            *volume = 100;
        }
    };

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if let Some(rest) = raw_line.strip_prefix("Sink Input #") {
            flush(
                &mut inputs,
                &mut id,
                &mut sink_index,
                &mut app_name,
                &mut media_name,
                &mut muted,
                &mut volume,
            );
            id = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Sink: ") {
            sink_index = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Mute: ") {
            muted = rest.trim() == "yes";
        } else if line.starts_with("Volume: ")
            && let Some(pct) = parse_volume_percent(line)
        {
            volume = pct;
        } else if let Some(rest) = line.strip_prefix("application.name = ") {
            app_name = rest.trim().trim_matches('"').to_string();
        } else if let Some(rest) = line.strip_prefix("media.name = ") {
            media_name = rest.trim().trim_matches('"').to_string();
        }
    }
    flush(
        &mut inputs,
        &mut id,
        &mut sink_index,
        &mut app_name,
        &mut media_name,
        &mut muted,
        &mut volume,
    );
    inputs
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sink {
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub volume: u8,
    pub muted: bool,
}

/// Extract the first `NN%` token from a `pactl` volume line.
pub fn parse_volume_percent(line: &str) -> Option<u8> {
    for part in line.split('/') {
        let trimmed = part.trim();
        if let Some(num) = trimmed.strip_suffix('%')
            && let Ok(v) = num.trim().parse::<u16>()
        {
            return Some(v.min(100) as u8);
        }
    }
    None
}

/// Parse `pactl list sinks` output into structured [`Sink`] records.
pub fn parse_sinks(output: &str, default: &str) -> Vec<Sink> {
    let mut sinks = Vec::new();
    let mut name = String::new();
    let mut description = String::new();
    let mut muted = false;
    let mut volume = 0u8;

    let flush = |sinks: &mut Vec<Sink>,
                 name: &mut String,
                 description: &mut String,
                 muted: &mut bool,
                 volume: &mut u8| {
        if !name.is_empty() {
            let desc = if description.is_empty() {
                name.clone()
            } else {
                std::mem::take(description)
            };
            let n = std::mem::take(name);
            let is_default = n == default;
            sinks.push(Sink {
                name: n,
                description: desc,
                is_default,
                volume: *volume,
                muted: *muted,
            });
            *muted = false;
            *volume = 0;
        }
    };

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if raw_line.starts_with("Sink #") {
            flush(
                &mut sinks,
                &mut name,
                &mut description,
                &mut muted,
                &mut volume,
            );
        } else if let Some(rest) = line.strip_prefix("Name: ") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Description: ") {
            description = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Mute: ") {
            muted = rest.trim() == "yes";
        } else if line.starts_with("Volume:") && !line.starts_with("Volume: ") {
            // Base volume or sub-line; ignore
        } else if line.starts_with("Volume: ")
            && let Some(pct) = parse_volume_percent(line)
        {
            volume = pct;
        }
    }
    flush(
        &mut sinks,
        &mut name,
        &mut description,
        &mut muted,
        &mut volume,
    );
    sinks.sort_by(|a, b| b.is_default.cmp(&a.is_default));
    sinks
}

/// Parse `pactl list sources` output into structured [`Sink`] records,
/// excluding `.monitor` loopback sources so only real microphones are shown.
pub fn parse_sources(output: &str, default: &str) -> Vec<Sink> {
    let mut sources = Vec::new();
    let mut name = String::new();
    let mut description = String::new();
    let mut muted = false;
    let mut volume = 0u8;

    let flush = |sources: &mut Vec<Sink>,
                 name: &mut String,
                 description: &mut String,
                 muted: &mut bool,
                 volume: &mut u8| {
        if !name.is_empty() && !name.ends_with(".monitor") {
            let desc = if description.is_empty() {
                name.clone()
            } else {
                std::mem::take(description)
            };
            let n = std::mem::take(name);
            let is_default = n == default;
            sources.push(Sink {
                name: n,
                description: desc,
                is_default,
                volume: *volume,
                muted: *muted,
            });
        } else {
            name.clear();
            description.clear();
        }
        *muted = false;
        *volume = 0;
    };

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if raw_line.starts_with("Source #") {
            flush(
                &mut sources,
                &mut name,
                &mut description,
                &mut muted,
                &mut volume,
            );
        } else if let Some(rest) = line.strip_prefix("Name: ") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Description: ") {
            description = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Mute: ") {
            muted = rest.trim() == "yes";
        } else if line.starts_with("Volume: ")
            && let Some(pct) = parse_volume_percent(line)
        {
            volume = pct;
        }
    }
    flush(
        &mut sources,
        &mut name,
        &mut description,
        &mut muted,
        &mut volume,
    );
    sources.sort_by(|a, b| b.is_default.cmp(&a.is_default));
    sources
}

/// The audio screen.
pub struct AudioScreen {
    sinks: Vec<Sink>,
    sources: Vec<Sink>,
    inputs: Vec<SinkInput>,
    default_sink: String,
    default_source: String,
    source_volume: u8,
    source_muted: bool,
    status: String,
    strip: ActionStrip,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    keys: Vec<String>,
}

impl Default for AudioScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            keys: Vec::new(),
            sinks: Vec::new(),
            sources: Vec::new(),
            inputs: Vec::new(),
            default_sink: String::new(),
            default_source: String::new(),
            source_volume: 0,
            source_muted: false,
            status: "Enter switches/routes \u{00b7} \u{2190}/\u{2192} volume \u{00b1}5% \u{00b7} Ctrl+M mute"
                .to_string(),
            strip: ActionStrip::default(),
            list: ListState::new(),
            header: Header::new("Hayato").subtitle("Audio & Per-App Mixer"),
            keybar: vec![
                KeyHint::new("Enter", "switch/route"),
                KeyHint::new("\u{2190}/\u{2192}", "volume \u{00b1}5%"),
                KeyHint::new("Tab", "actions"),
                KeyHint::new("Ctrl+M", "mute"),
                KeyHint::new("Ctrl+I", "mute mic"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(16),
        };
        screen.reload(false);
        screen
    }

    fn reload(&mut self, preserve: bool) {
        self.default_sink = out(&["pactl", "get-default-sink"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let raw_sinks = out(&["pactl", "list", "sinks"]).unwrap_or_default();
        self.sinks = parse_sinks(&raw_sinks, &self.default_sink);
        self.default_source = out(&["pactl", "get-default-source"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let raw_sources = out(&["pactl", "list", "sources"]).unwrap_or_default();
        self.sources = parse_sources(&raw_sources, &self.default_source);
        let raw_inputs = out(&["pactl", "list", "sink-inputs"]).unwrap_or_default();
        self.inputs = parse_sink_inputs(&raw_inputs);
        self.source_volume = out(&["pactl", "get-source-volume", "@DEFAULT_SOURCE@"])
            .and_then(|s| parse_volume_percent(&s))
            .unwrap_or(0);
        self.source_muted = out(&["pactl", "get-source-mute", "@DEFAULT_SOURCE@"])
            .is_some_and(|s| s.contains("yes"));
        self.build_rows(preserve);
        self.header = Header::new("Hayato")
            .subtitle("Audio & Per-App Mixer")
            .status(self.status.clone());
    }

    fn build_rows(&mut self, preserve: bool) {
        let out_muted = self
            .sinks
            .iter()
            .find(|s| s.is_default)
            .is_some_and(|s| s.muted);
        let out_label = if out_muted {
            "Unmute output"
        } else {
            "Mute output"
        };
        let mic_label = if self.source_muted {
            "Unmute mic"
        } else {
            "Mute mic"
        };
        self.strip.set_items(vec![
            ActionItem::new("mute-sink", "^M", out_label).glyph(Glyph::Volume),
            ActionItem::new("mute-source", "^I", mic_label).glyph(Glyph::Mic),
            ActionItem::new("refresh", "^R", "Refresh streams").glyph(Glyph::Sync),
        ]);

        let mut rows = Vec::new();
        let mut keys = Vec::new();
        if self.sinks.is_empty() {
            rows.push(
                Row::new("No audio outputs")
                    .glyph(Glyph::Volume)
                    .detail("PipeWire / PulseAudio reported no sinks"),
            );
            keys.push(String::new());
        } else {
            for s in &self.sinks {
                let glyph = if s.is_default {
                    Glyph::Check
                } else {
                    Glyph::Volume
                };
                let badge = if s.muted {
                    format!("muted ({:>3}%)", s.volume)
                } else if s.is_default {
                    format!("\u{25cf} {:>3}%", s.volume)
                } else {
                    format!("{:>3}%", s.volume)
                };
                keys.push(format!("sink:{}", s.name));
                rows.push(
                    Row::new(s.description.clone())
                        .glyph(glyph)
                        .detail(s.name.clone())
                        .value(badge),
                );
            }
        }

        if !self.sources.is_empty() {
            rows.push(Row::heading("Microphones (Inputs)"));
            keys.push(String::new());
            for src in &self.sources {
                let glyph = if src.is_default {
                    Glyph::Check
                } else {
                    Glyph::Mic
                };
                let badge = if src.muted {
                    format!("muted ({:>3}%)", src.volume)
                } else if src.is_default {
                    format!("\u{25cf} {:>3}%", src.volume)
                } else {
                    format!("{:>3}%", src.volume)
                };
                keys.push(format!("source:{}", src.name));
                rows.push(
                    Row::new(src.description.clone())
                        .glyph(glyph)
                        .detail(src.name.clone())
                        .value(badge),
                );
            }
        }

        rows.push(Row::heading("Applications (Per-App Mixer)"));
        keys.push(String::new());
        if self.inputs.is_empty() {
            rows.push(
                Row::new("No apps playing audio")
                    .glyph(Glyph::Sliders)
                    .detail("Active streams appear here · \u{2190}/\u{2192} volume · Enter routes"),
            );
            keys.push(String::new());
        } else {
            for app in &self.inputs {
                let badge = if app.muted {
                    format!("muted ({:>3}%)", app.volume)
                } else {
                    format!("\u{25cf} {:>3}%", app.volume)
                };
                let detail = if app.media_name.is_empty() {
                    format!("stream #{} · Enter cycles output", app.id)
                } else {
                    app.media_name.clone()
                };
                keys.push(format!("app:{}", app.id));
                rows.push(
                    Row::new(app.app_name.clone())
                        .glyph(Glyph::Sliders)
                        .detail(detail)
                        .value(badge),
                );
            }
        }

        self.keys = keys;
        if preserve {
            self.list.update_rows(rows);
        } else {
            self.list.set_rows(rows);
        }
    }

    fn target_sink(&self) -> String {
        if let Some(idx) = self.list.selected_index()
            && let Some(key) = self.keys.get(idx)
            && let Some(name) = key.strip_prefix("sink:")
        {
            return name.to_string();
        }
        if !self.default_sink.is_empty() {
            self.default_sink.clone()
        } else {
            "@DEFAULT_SINK@".to_string()
        }
    }

    fn adjust_volume(&mut self, delta: i8) {
        if let Some(idx) = self.list.selected_index()
            && let Some(key) = self.keys.get(idx)
        {
            if let Some(app_id) = key.strip_prefix("app:") {
                let current = self
                    .inputs
                    .iter()
                    .find(|a| a.id == app_id)
                    .map(|a| a.volume)
                    .unwrap_or(100);
                let next = (current as i16 + delta as i16).clamp(0, 100) as u8;
                let arg = format!("{next}%");
                let _ = Command::new("pactl")
                    .args(["set-sink-input-volume", app_id, &arg])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let name = self
                    .inputs
                    .iter()
                    .find(|a| a.id == app_id)
                    .map(|a| a.app_name.clone())
                    .unwrap_or_else(|| format!("Stream #{app_id}"));
                self.status = format!("{name} volume: {next}%");
                self.reload(true);
                return;
            }
            if let Some(src_name) = key.strip_prefix("source:") {
                let current = self
                    .sources
                    .iter()
                    .find(|s| s.name == src_name)
                    .map(|s| s.volume)
                    .unwrap_or(self.source_volume);
                let next = (current as i16 + delta as i16).clamp(0, 100) as u8;
                let arg = format!("{next}%");
                let _ = Command::new("pactl")
                    .args(["set-source-volume", src_name, &arg])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = format!("Microphone volume: {next}%");
                self.reload(true);
                return;
            }
        }
        let target = self.target_sink();
        let current = self
            .sinks
            .iter()
            .find(|s| s.name == target || (target == "@DEFAULT_SINK@" && s.is_default))
            .map(|s| s.volume)
            .unwrap_or(50);
        let next = (current as i16 + delta as i16).clamp(0, 100) as u8;
        let arg = format!("{next}%");
        let _ = Command::new("pactl")
            .args(["set-sink-volume", &target, &arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        self.status = format!("Output volume: {next}%");
        self.reload(true);
    }

    fn toggle_selected_mute(&mut self) {
        if let Some(idx) = self.list.selected_index()
            && let Some(key) = self.keys.get(idx).map(String::as_str)
        {
            if let Some(app_id) = key.strip_prefix("app:") {
                let _ = Command::new("pactl")
                    .args(["set-sink-input-mute", app_id, "toggle"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = format!("Toggled app stream #{app_id} mute");
                self.reload(true);
                return;
            }
            if key == "mute-source" || key.starts_with("source:") {
                let target_src = key
                    .strip_prefix("source:")
                    .filter(|s| !s.is_empty())
                    .unwrap_or("@DEFAULT_SOURCE@")
                    .to_string();
                let _ = Command::new("pactl")
                    .args(["set-source-mute", &target_src, "toggle"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = "Toggled microphone mute".to_string();
                self.reload(true);
                return;
            }
        }
        let target = self.target_sink();
        let _ = Command::new("pactl")
            .args(["set-sink-mute", &target, "toggle"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        self.status = "Toggled output mute".to_string();
        self.reload(true);
    }

    fn layout(area: Rect) -> (Rect, Rect, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let hero = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 3.min(inner.height),
        };
        let strip = Rect {
            x: inner.x,
            y: inner.y.saturating_add(3).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(3)),
        };
        let filter = Rect {
            x: inner.x,
            y: inner.y.saturating_add(5).min(inner.bottom()),
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(5)),
        };
        let rows_y = inner.y.saturating_add(7).min(inner.bottom());
        let rows = Rect {
            x: inner.x,
            y: rows_y,
            width: inner.width,
            height: inner.bottom().saturating_sub(rows_y),
        };
        (hero, strip, filter, rows)
    }

    fn draw_hero(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        if area.height == 0 || area.width < 20 {
            return;
        }
        let def = self
            .sinks
            .iter()
            .find(|s| s.is_default)
            .or(self.sinks.first());
        let y0 = area.y;
        if let Some(sink) = def {
            let (badge_text, badge_role) = if sink.muted {
                ("MUTED", ColorRole::Warn)
            } else {
                ("OUTPUT", ColorRole::Ok)
            };
            let mut x = draw_badge(buf, area.x, y0, design, badge_text, badge_role);
            x = put(
                buf,
                x,
                y0,
                "  ",
                area.right().saturating_sub(x),
                design.style(TextStyle::Body),
            );
            let gauge_space = 22u16.min(area.width / 2);
            let _ = put_trunc(
                buf,
                x,
                y0,
                &sink.description,
                area.right().saturating_sub(x + gauge_space + 2),
                design.style(TextStyle::Title),
            );
            if area.width >= 36 {
                let gx = area.right().saturating_sub(gauge_space);
                let gw = gauge_space.saturating_sub(6);
                let pct = format!(" {:>3}%", sink.volume);
                draw_gauge(
                    buf,
                    Rect {
                        x: gx,
                        y: y0,
                        width: gw,
                        height: 1,
                    },
                    design,
                    sink.volume as f64 / 100.0,
                    &pct,
                );
            }
        } else {
            let x = draw_badge(buf, area.x, y0, design, "NO OUTPUT", ColorRole::Warn);
            let _ = put_trunc(
                buf,
                x,
                y0,
                "  No audio sink detected",
                area.right().saturating_sub(x),
                design.style(TextStyle::Dim),
            );
        }

        if area.height >= 2 {
            let y1 = area.y + 1;
            let (mic_badge, mic_role) = if self.source_muted {
                ("MIC MUTED", ColorRole::Warn)
            } else {
                ("INPUT", ColorRole::Accent)
            };
            let mut x = draw_badge(buf, area.x, y1, design, mic_badge, mic_role);
            x = put(
                buf,
                x,
                y1,
                "  ",
                area.right().saturating_sub(x),
                design.style(TextStyle::Body),
            );
            let mic_name = self
                .sources
                .iter()
                .find(|s| s.is_default)
                .or(self.sources.first())
                .map(|s| s.description.as_str())
                .unwrap_or("Default microphone");
            let gauge_space = 22u16.min(area.width / 2);
            let _ = put_trunc(
                buf,
                x,
                y1,
                mic_name,
                area.right().saturating_sub(x + gauge_space + 2),
                design.style(TextStyle::Dim),
            );
            if area.width >= 36 {
                let gx = area.right().saturating_sub(gauge_space);
                let gw = gauge_space.saturating_sub(6);
                let pct = format!(" {:>3}%", self.source_volume);
                draw_gauge(
                    buf,
                    Rect {
                        x: gx,
                        y: y1,
                        width: gw,
                        height: 1,
                    },
                    design,
                    self.source_volume as f64 / 100.0,
                    &pct,
                );
            }
        }

        if area.height >= 3 {
            let rule_y = area.y + 2;
            let rule_style = Style::default().fg(design.color(ColorRole::SurfaceAlt));
            for col in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((col, rule_y)) {
                    cell.set_symbol("\u{2500}").set_style(rule_style);
                }
            }
        }
    }
}

impl Screen for AudioScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((840, 500))
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Outputs"), true);
        let (hero, strip, filter, rows) = Self::layout(ctx.area);
        self.draw_hero(ctx.buf, hero, ctx.design);
        self.strip.render(strip, ctx.buf, ctx.design);
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            &self.status,
            self.list.visible_len(),
            self.list.len(),
        );
        self.rows_height.set(rows.height);
        self.list
            .render(rows, ctx.buf, ctx.design, !self.strip.is_focused());
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match self.strip.handle(key) {
            ActionStripEvent::Activated(id) => return self.run_strip_action(id),
            ActionStripEvent::Consumed => return Outcome::Consumed,
            ActionStripEvent::Ignored => {}
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('m') | KeyCode::Char('M') => {
                    self.toggle_selected_mute();
                    return Outcome::Consumed;
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    return self.run_strip_action("mute-source");
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    return self.run_strip_action("refresh");
                }
                _ => {}
            }
        }
        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(hayato_ui::app::exit::ESCAPE);
            }
            KeyCode::Left | KeyCode::Char('-') => {
                self.adjust_volume(-5);
                return Outcome::Consumed;
            }
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_volume(5);
                return Outcome::Consumed;
            }
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_index() {
                    return self.activate(index);
                }
            }
            _ => {}
        }
        let typed = matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace)
            && !key.modifiers.contains(KeyModifiers::CONTROL);
        if typed {
            self.list.type_filter(key);
            return Outcome::Consumed;
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            if row >= 10_000 {
                let strip_idx = row - 10_000;
                if let Some(item) = self.strip.items().get(strip_idx) {
                    return self.run_strip_action(item.id);
                }
                return Outcome::Consumed;
            }
            self.strip.set_focused(false);
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, strip, _, rows) = Self::layout(area);
        if let Some(idx) = self.strip.hit_index(strip, x, y) {
            return Some(10_000 + idx);
        }
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }
}

impl AudioScreen {
    fn run_strip_action(&mut self, id: &'static str) -> Outcome {
        match id {
            "refresh" => {
                self.status = "Refreshed audio devices and app streams".to_string();
                self.reload(true);
                Outcome::Consumed
            }
            "mute-sink" => {
                let _ = Command::new("pactl")
                    .args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = "Toggled default output mute".to_string();
                self.reload(true);
                Outcome::Consumed
            }
            "mute-source" => {
                let _ = Command::new("pactl")
                    .args(["set-source-mute", "@DEFAULT_SOURCE@", "toggle"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = "Toggled default microphone mute".to_string();
                self.reload(true);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let rows = self.list.rows();
        if index >= rows.len() {
            return Outcome::Continue;
        }
        let key = self.keys.get(index).cloned().unwrap_or_default();
        if key.is_empty() || key == "-" {
            return Outcome::Continue;
        }
        if let Some(app_id) = key.strip_prefix("app:") {
            if self.sinks.len() > 1 {
                // Move this app stream to the next non-default (or next) sink.
                let next_sink = self
                    .sinks
                    .iter()
                    .find(|s| !s.is_default)
                    .or(self.sinks.first());
                if let Some(sink) = next_sink {
                    let sink_name = sink.name.clone();
                    let sink_desc = sink.description.clone();
                    let _ = Command::new("pactl")
                        .args(["move-sink-input", app_id, &sink_name])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    self.status = format!("Routed stream #{app_id} to {sink_desc}");
                    self.reload(true);
                }
            } else {
                let _ = Command::new("pactl")
                    .args(["set-sink-input-mute", app_id, "toggle"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                self.status = format!("Toggled stream #{app_id} mute");
                self.reload(true);
            }
            return Outcome::Consumed;
        }
        if let Some(name) = key.strip_prefix("source:") {
            let _ = Command::new("pactl")
                .args(["set-default-source", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let desc = self
                .sources
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.description.as_str())
                .unwrap_or(name);
            self.status = format!("Switched microphone to {desc}");
            self.reload(true);
            return Outcome::Consumed;
        }
        if let Some(name) = key.strip_prefix("sink:") {
            let _ = Command::new("pactl")
                .args(["set-default-sink", name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Ok(output) = Command::new("pactl")
                .args(["list", "short", "sink-inputs"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Some(id) = line.split_whitespace().next() {
                        let _ = Command::new("pactl")
                            .args(["move-sink-input", id, name])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                }
            }
            let desc = self
                .sinks
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.description.clone())
                .unwrap_or_else(|| name.to_string());
            self.status = format!("Default output: {desc}");
            self.reload(true);
            Outcome::Consumed
        } else {
            match key.as_str() {
                "refresh" => {
                    self.status = "Refreshed audio devices and app streams".to_string();
                    self.reload(true);
                    Outcome::Consumed
                }
                "mute-sink" => {
                    let _ = Command::new("pactl")
                        .args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    self.status = "Toggled default output mute".to_string();
                    self.reload(true);
                    Outcome::Consumed
                }
                "mute-source" => {
                    let _ = Command::new("pactl")
                        .args(["set-source-mute", "@DEFAULT_SOURCE@", "toggle"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    self.status = "Toggled default microphone mute".to_string();
                    self.reload(true);
                    Outcome::Consumed
                }
                _ => Outcome::Continue,
            }
        }
    }
}

/// `hayato menu audio` (`bar-audio`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Audio output",
            format!("{said} (pactl)"),
        ));
    }
    let mut screen = AudioScreen::new();
    crate::run_screen(terminal, design, "Audio output", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_needed_tools() {
        assert!(NEEDED.contains(&"pactl"));
        assert!(!NEEDED.contains(&"pavucontrol"));
    }

    #[test]
    fn parses_pactl_sinks_volume_and_mute() {
        let raw = "\
Sink #47
\tState: RUNNING
\tName: alsa_output.platform-sound.HiFi__hw_sofmt8192_0__sink
\tDescription: Speaker
\tMute: no
\tVolume: front-left: 42598 /  65% / -11.22 dB,   front-right: 42598 /  65% / -11.22 dB
Sink #52
\tState: SUSPENDED
\tName: bluez_output.AA_BB_CC.1
\tDescription: Headphones
\tMute: yes
\tVolume: front-left: 19660 /  30% / -31.37 dB
";
        let sinks = parse_sinks(raw, "bluez_output.AA_BB_CC.1");
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].description, "Headphones");
        assert!(sinks[0].is_default);
        assert!(sinks[0].muted);
        assert_eq!(sinks[0].volume, 30);
        assert_eq!(sinks[1].description, "Speaker");
        assert!(!sinks[1].is_default);
        assert!(!sinks[1].muted);
        assert_eq!(sinks[1].volume, 65);
    }

    #[test]
    fn parses_pactl_sources_and_skips_monitor_loopbacks() {
        let raw = "\
Source #10
\tName: alsa_output.platform-sound.monitor
\tDescription: Monitor of Speaker
\tMute: no
\tVolume: front-left: 65536 / 100% / 0.00 dB
Source #11
\tName: alsa_input.platform-sound.HiFi__Mic__source
\tDescription: Internal Microphone
\tMute: no
\tVolume: front-left: 52428 /  80% / -5.80 dB
Source #12
\tName: bluez_input.AA_BB_CC.0
\tDescription: Headset Mic
\tMute: yes
\tVolume: front-left: 32768 /  50% / -18.06 dB
";
        let sources = parse_sources(raw, "bluez_input.AA_BB_CC.0");
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].description, "Headset Mic");
        assert!(sources[0].is_default);
        assert!(sources[0].muted);
        assert_eq!(sources[0].volume, 50);
        assert_eq!(sources[1].description, "Internal Microphone");
        assert!(!sources[1].is_default);
        assert_eq!(sources[1].volume, 80);
    }

    #[test]
    fn parses_pactl_sink_inputs_for_per_app_mixer() {
        let raw = "\
Sink Input #42
\tSink: 51
\tMute: no
\tVolume: front-left: 52428 /  80% / -5.80 dB
\tProperties:
\t\tapplication.name = \"Firefox\"
\t\tmedia.name = \"Lofi Beats\"
Sink Input #43
\tSink: 52
\tMute: yes
\tVolume: front-left: 32768 /  50% / -18.06 dB
\tProperties:
\t\tapplication.name = \"mpv\"
";
        let inputs = parse_sink_inputs(raw);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].id, "42");
        assert_eq!(inputs[0].app_name, "Firefox");
        assert_eq!(inputs[0].media_name, "Lofi Beats");
        assert_eq!(inputs[0].volume, 80);
        assert!(!inputs[0].muted);

        assert_eq!(inputs[1].id, "43");
        assert_eq!(inputs[1].app_name, "mpv");
        assert_eq!(inputs[1].volume, 50);
        assert!(inputs[1].muted);
    }
}
