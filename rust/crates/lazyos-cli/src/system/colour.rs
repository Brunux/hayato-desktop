//! Colour: pick a pixel and show the palette (`rofi-color`).
//!
//! The old script used `slurp -p` to pick a 1x1 region, `grim` to capture it,
//! and `ffmpeg` to read the raw pixel value. It then showed the hex, RGB, and
//! the solarized palette as swatch rows.

#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::vec_init_then_push)]
use std::cell::Cell;

use lazyos_theme::Rgb;
use lazyos_ui::{
    BodyCtx, Card, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, Urgency, draw_badge, draw_card, draw_filter_bar, draw_gauge,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::system::{missing, out};

const NEEDED: &[&str] = &["slurp", "grim", "ffmpeg"];

/// The colour screen: dual-pane truecolor swatch inspector + searchable palette list.
pub struct ColourScreen {
    picked: Option<Colour>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    status_note: Option<String>,
}

#[derive(Clone, Debug)]
struct Colour {
    hex: String,
    rgb: (u8, u8, u8),
}

impl Colour {
    fn from_hex(hex: &str) -> Option<Self> {
        let raw = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
        if raw.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&raw[0..2], 16).ok()?;
        let g = u8::from_str_radix(&raw[2..4], 16).ok()?;
        let b = u8::from_str_radix(&raw[4..6], 16).ok()?;
        Some(Self {
            hex: format!("#{:02X}{:02X}{:02X}", r, g, b),
            rgb: (r, g, b),
        })
    }

    fn relative_luminance(&self) -> f64 {
        fn channel(c: u8) -> f64 {
            let s = c as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.rgb.0) + 0.7152 * channel(self.rgb.1) + 0.0722 * channel(self.rgb.2)
    }

    fn contrast_ratio(l1: f64, l2: f64) -> f64 {
        let (bright, dark) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (bright + 0.05) / (dark + 0.05)
    }
}

impl ColourScreen {
    pub fn new() -> Self {
        let list = ListState::new().with_empty("pick a pixel to see its colour");
        Self {
            picked: None,
            list,
            header: Header::new("LazyOS").subtitle("Colour"),
            keybar: vec![
                KeyHint::new("Enter", "copy / pick"),
                KeyHint::new("type", "filter"),
                KeyHint::new("Esc", "close"),
            ],
            rows_height: Cell::new(20),
            status_note: None,
        }
    }

    fn selected_colour(&self) -> Colour {
        if let Some(idx) = self.list.selected_index() {
            if let Some(row) = self.list.rows().get(idx) {
                if let Some(val) = row.value.as_deref() {
                    if let Some(hex) = val.strip_prefix("hex:") {
                        if let Some(c) = Colour::from_hex(hex) {
                            return c;
                        }
                    }
                }
            }
        }
        self.picked.clone().unwrap_or(Colour {
            hex: "#2AA198".to_string(),
            rgb: (0x2a, 0xa1, 0x98),
        })
    }

    fn pick_pixel() -> Option<Colour> {
        // slurp -p picks a point and outputs "x,y"
        let point = out(&["slurp", "-p"])?;
        let point = point.trim();
        if point.is_empty() {
            return None;
        }
        // grim -g "x,y 1x1" captures a 1x1 pixel at that point
        let tmp = std::env::temp_dir().join(format!("colour-{}.ppm", std::process::id()));
        let geom = format!("{} 1x1", point);
        if out(&["grim", "-g", &geom, "-t", "ppm", &tmp.to_string_lossy()]).is_none() {
            return None;
        }
        // ffmpeg reads the single pixel
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "quiet",
                "-f",
                "image2pipe",
                "-i",
                "-",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ])
            .stdin(std::process::Stdio::from(std::fs::File::open(&tmp).ok()?))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?
            .wait_with_output()
            .ok()?;
        let _ = std::fs::remove_file(&tmp);
        if output.stdout.len() < 3 {
            return None;
        }
        let (r, g, b) = (output.stdout[0], output.stdout[1], output.stdout[2]);
        let hex = format!("#{:02x}{:02x}{:02x}", r, g, b);
        Some(Colour {
            hex: hex.to_uppercase(),
            rgb: (r, g, b),
        })
    }

    fn pick_detached() {
        let script = r#"sleep 0.2
pos=$(slurp -p -f '%x,%y' 2>/dev/null) || exit 0
[ -n "$pos" ] || exit 0
rgb=$(grim -g "$pos 1x1" - 2>/dev/null | ffmpeg -v error -i pipe:0 -frames:v 1 -f rawvideo -pix_fmt rgb24 pipe:1 2>/dev/null | od -An -tu1) || rgb=
set -- ${rgb:-}
if [ $# -lt 3 ]; then
    notify-send -a LazyOS -u critical -i dialog-error "Colour" "That pixel could not be read"
    exit 1
fi
hex=$(printf '#%02x%02x%02x' "$1" "$2" "$3")
printf '%s' "$hex" | wl-copy
notify-send -a LazyOS -i color-picker "Colour" "$hex copied to the clipboard (${pos%%,*}, ${pos#*,})"
"#;
        crate::system::spawn_detached(script);
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        rows.push(
            Row::new("Pick a pixel")
                .glyph(Glyph::Brush)
                .detail("the colour under the pointer, to the clipboard")
                .value("pick"),
        );
        if let Some(c) = &self.picked {
            rows.push(
                Row::new(format!("Picked: {}", c.hex))
                    .glyph(Glyph::Brush)
                    .detail(format!(
                        "RGB({}, {}, {}) · Enter copies",
                        c.rgb.0, c.rgb.1, c.rgb.2
                    ))
                    .value(format!("hex:{}", c.hex)),
            );
        }
        // Solarized palette swatches
        const SOLARIZED: &[(&str, &str, &str)] = &[
            ("base03", "#002b36", "background"),
            ("base02", "#073642", "background highlights"),
            ("base01", "#586e75", "comments / optional emphasis"),
            ("base00", "#657b83", "comments"),
            ("base0", "#839496", "body text / default code"),
            ("base1", "#93a1a1", "emphasized text"),
            ("base2", "#eee8d5", "background highlights"),
            ("base3", "#fdf6e3", "background"),
            ("yellow", "#b58900", "warning"),
            ("orange", "#cb4b16", "error / orange"),
            ("red", "#dc322f", "error"),
            ("magenta", "#d33682", "magenta"),
            ("violet", "#6c71c4", "violet"),
            ("blue", "#268bd2", "blue / links"),
            ("cyan", "#2aa198", "accent / cyan"),
            ("green", "#859900", "success"),
        ];
        for (name, hex, role) in SOLARIZED {
            rows.push(
                Row::new(format!("{}  {}", name, hex.to_uppercase()))
                    .glyph(Glyph::Brush)
                    .detail(format!("{role} · Enter copies"))
                    .value(format!("hex:{}", hex)),
            );
        }
        self.list.set_rows(rows);
    }
}

impl Screen for ColourScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Colour"), true);
        let (left_pane, filter, rows) = Self::layout(ctx.area);

        if let Some(lp) = left_pane {
            let c = self.selected_colour();
            let swatch_style = Style::default().fg(lazyos_ui::design::color(Rgb::new(
                c.rgb.0, c.rgb.1, c.rgb.2,
            )));
            let swatch_line = "\u{2588}".repeat(lp.width.saturating_sub(2).min(24) as usize);
            for dy in 0..4.min(lp.height) {
                ctx.buf
                    .set_string(lp.x, lp.y + dy, &swatch_line, swatch_style);
            }

            if lp.height >= 6 {
                let mut x = draw_badge(
                    ctx.buf,
                    lp.x,
                    lp.y + 5,
                    ctx.design,
                    &c.hex,
                    ColorRole::Accent,
                );
                x += 1;
                ctx.buf.set_string(
                    x,
                    lp.y + 5,
                    format!("rgb({},{},{})", c.rgb.0, c.rgb.1, c.rgb.2),
                    ctx.design.style(TextStyle::Dim),
                );
            }

            let bar_w = lp.width.saturating_sub(8);
            if lp.height >= 10 && bar_w >= 6 {
                let r_val = format!("{:>3}", c.rgb.0);
                let g_val = format!("{:>3}", c.rgb.1);
                let b_val = format!("{:>3}", c.rgb.2);
                ctx.buf
                    .set_string(lp.x, lp.y + 7, "R", ctx.design.style(TextStyle::Title));
                draw_gauge(
                    ctx.buf,
                    Rect {
                        x: lp.x + 2,
                        y: lp.y + 7,
                        width: bar_w,
                        height: 1,
                    },
                    ctx.design,
                    c.rgb.0 as f64 / 255.0,
                    &r_val,
                );

                ctx.buf
                    .set_string(lp.x, lp.y + 8, "G", ctx.design.style(TextStyle::Title));
                draw_gauge(
                    ctx.buf,
                    Rect {
                        x: lp.x + 2,
                        y: lp.y + 8,
                        width: bar_w,
                        height: 1,
                    },
                    ctx.design,
                    c.rgb.1 as f64 / 255.0,
                    &g_val,
                );

                ctx.buf
                    .set_string(lp.x, lp.y + 9, "B", ctx.design.style(TextStyle::Title));
                draw_gauge(
                    ctx.buf,
                    Rect {
                        x: lp.x + 2,
                        y: lp.y + 9,
                        width: bar_w,
                        height: 1,
                    },
                    ctx.design,
                    c.rgb.2 as f64 / 255.0,
                    &b_val,
                );
            }

            if lp.height >= 13 {
                let lum = c.relative_luminance();
                let vs_white = Colour::contrast_ratio(lum, 1.0);
                let vs_black = Colour::contrast_ratio(lum, 0.0);
                ctx.buf.set_string(
                    lp.x,
                    lp.y + 11,
                    format!("WCAG vs #FFF: {vs_white:.1}:1"),
                    ctx.design.style(TextStyle::Dim),
                );
                ctx.buf.set_string(
                    lp.x,
                    lp.y + 12,
                    format!("WCAG vs #000: {vs_black:.1}:1"),
                    ctx.design.style(TextStyle::Dim),
                );
            }
        }

        let note = self
            .status_note
            .as_deref()
            .unwrap_or("Enter copies or picks a pixel · type to filter");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            note,
            self.list.len(),
            self.list.rows().len(),
        );
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        let rows_height = self.rows_height.get();
        match key.code {
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Continue;
                }
                return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
            }
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_index() {
                    return self.activate(index);
                }
            }
            KeyCode::Char(_) | KeyCode::Backspace
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.list.type_filter(key) =>
            {
                return Outcome::Continue;
            }
            _ => {}
        }
        match self.list.handle(key, rows_height, design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let (_, _, rows) = Self::layout(area);
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((780, 460))
    }
}

impl ColourScreen {
    fn layout(area: Rect) -> (Option<Rect>, Rect, Rect) {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        if inner.width >= 60 && inner.height >= 10 {
            let left_w = 26;
            let left = Rect {
                x: inner.x,
                y: inner.y,
                width: left_w,
                height: inner.height,
            };
            let right = Rect {
                x: inner.x + left_w + 2,
                y: inner.y,
                width: inner.width.saturating_sub(left_w + 2),
                height: inner.height,
            };
            let filter = Rect {
                height: 1.min(right.height),
                ..right
            };
            let rows = Rect {
                y: right.y + 2,
                height: right.height.saturating_sub(2),
                ..right
            };
            (Some(left), filter, rows)
        } else {
            let (filter, rows) = super::pick::list_layout(area);
            (None, filter, rows)
        }
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let rows = self.list.rows();
        let Some(row) = rows.get(index) else {
            return Outcome::Continue;
        };
        let Some(key) = row.value.as_deref() else {
            return Outcome::Continue;
        };
        if key == "pick" {
            Self::pick_detached();
            Outcome::Done(lazyos_ui::app::exit::OK)
        } else if let Some(hex) = key.strip_prefix("hex:").map(|s| s.to_uppercase()) {
            let _ = std::process::Command::new("wl-copy")
                .arg(&hex)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            self.status_note = Some(format!("Copied {hex} to the clipboard"));
            Outcome::Continue
        } else {
            Outcome::Continue
        }
    }
}

/// `lazyos menu colour` (`rofi-color`): the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new(
            "Colour",
            format!("{said} (slurp, grim, ffmpeg)"),
        ));
    }
    let mut screen = ColourScreen::new();
    screen.build_rows();
    let code = crate::run_screen(terminal, design, "Colour", &mut screen)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_palette_is_complete() {
        let mut screen = ColourScreen::new();
        screen.build_rows();
        // 1 pick row + 16 solarized palette swatches = 17 rows
        assert_eq!(screen.list.rows().len(), 17);
        assert_eq!(screen.list.rows()[0].value.as_deref(), Some("pick"));
        assert_eq!(screen.list.rows()[1].value.as_deref(), Some("hex:#002b36"));
    }
}
