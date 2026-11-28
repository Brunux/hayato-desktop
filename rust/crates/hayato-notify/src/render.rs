#![allow(unknown_lints)]
#![allow(clippy::chunks_exact_to_as_chunks)]

use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};

use crate::font::FontRenderer;
use crate::model::{Notification, Rect, Urgency};

pub const OSD_WIDTH: f32 = 280.0;
pub const OSD_HEIGHT: f32 = 44.0;
pub const CARD_SPACING: f32 = 8.0;
pub const CORNER_RADIUS: f32 = 3.0;
const HEADER_H: f32 = 22.0;
const OVERFLOW_PILL_H: f32 = 22.0;
const ACTION_BTN_H: f32 = 26.0;
const ACTION_BTN_GAP: f32 = 6.0;
const ACTION_ROW_GAP: f32 = 6.0;
const CONTENT_LEFT_PAD: f32 = 14.0;
const CONTENT_RIGHT_PAD: f32 = 12.0;
const PROMPT_INDENT: f32 = 13.0;
const PROGRESS_BAR_H: f32 = 6.0;
const OSD_SEGMENTS: usize = 16;
const CARD_SEGMENTS: usize = 20;
const SEGMENT_GAP: f32 = 2.0;
const CLOSE_HIT_SIZE: f32 = 24.0;

fn with_alpha(c: Color, alpha: f32) -> Color {
    let a = (c.alpha() * alpha).clamp(0.0, 1.0);
    Color::from_rgba(c.red(), c.green(), c.blue(), a).unwrap_or(c)
}

pub struct CardTheme {
    pub bg: Color,
    pub bg_elevated: Color,
    pub header_bg: Color,
    pub border_subtle: Color,
    pub accent_text: Color,
    pub text_body: Color,
    pub text_dim: Color,
    pub accent: Color,
    #[allow(dead_code)]
    pub accent_alt: Color,
    pub danger: Color,
    pub danger_text: Color,
    pub danger_bg: Color,
    #[allow(dead_code)]
    pub warn: Color,
    pub warn_text: Color,
    #[allow(dead_code)]
    pub ok: Color,
    #[allow(dead_code)]
    pub ok_text: Color,
    pub track_bg: Color,
    pub btn_bg: Color,
    pub btn_fg: Color,
    pub btn_bg_hover: Color,
    pub btn_fg_hover: Color,
    pub btn_bevel_hover: Color,
}

impl Default for CardTheme {
    fn default() -> Self {
        Self {
            bg: Color::from_rgba8(31, 35, 53, 246),
            bg_elevated: Color::from_rgba8(40, 45, 65, 252),
            header_bg: Color::from_rgba8(24, 27, 41, 248),
            border_subtle: Color::from_rgba8(74, 83, 118, 235),
            accent_text: Color::from_rgba8(122, 162, 247, 255),
            text_body: Color::from_rgba8(192, 202, 245, 250),
            text_dim: Color::from_rgba8(133, 143, 184, 255),
            accent: Color::from_rgba8(122, 162, 247, 255),
            accent_alt: Color::from_rgba8(187, 154, 247, 255),
            danger: Color::from_rgba8(247, 118, 142, 255),
            danger_text: Color::from_rgba8(247, 148, 166, 255),
            danger_bg: Color::from_rgba8(66, 33, 46, 235),
            warn: Color::from_rgba8(224, 175, 104, 255),
            warn_text: Color::from_rgba8(224, 175, 104, 255),
            ok: Color::from_rgba8(158, 206, 106, 255),
            ok_text: Color::from_rgba8(158, 206, 106, 255),
            track_bg: Color::from_rgba8(53, 58, 80, 220),
            btn_bg: Color::from_rgba8(44, 48, 69, 245),
            btn_fg: Color::from_rgba8(192, 202, 245, 255),
            btn_bg_hover: Color::from_rgba8(122, 162, 247, 255),
            btn_fg_hover: Color::from_rgba8(26, 27, 38, 255),
            btn_bevel_hover: Color::from_rgba8(92, 122, 185, 255),
        }
    }
}

impl CardTheme {
    pub fn from_active() -> Self {
        let paths = hayato_theme::Paths::from_env();
        if let Some(slug) = hayato_theme::store::active(&paths) {
            if let Some(text) = hayato_theme::builtin::get(&slug) {
                if let Ok(theme) = hayato_theme::Theme::parse(text) {
                    return Self::from_theme(&theme);
                }
            }
        }
        Self::fallback()
    }

    /// The card palette before a theme is chosen: Tokyo Night, the image's
    /// default, never the theme the previous release used.
    pub fn fallback() -> Self {
        if let Ok(theme) = hayato_theme::Theme::parse(hayato_theme::builtin::TOKYO_NIGHT) {
            return Self::from_theme(&theme);
        }
        Self::default()
    }

    pub fn from_theme(theme: &hayato_theme::Theme) -> Self {
        use hayato_theme::contrast::{luminance, ratio, readable};
        use hayato_theme::{Hex, Rgb};

        const BLACK: Rgb = Rgb::new(0, 0, 0);
        const WHITE: Rgb = Rgb::new(255, 255, 255);
        let extreme = |against: Rgb| -> Rgb {
            if ratio(WHITE, against) >= ratio(BLACK, against) {
                WHITE
            } else {
                BLACK
            }
        };

        let r = theme.roles();
        let to_color = |c: Rgb, a: u8| Color::from_rgba8(c.r, c.g, c.b, a);

        let bg_rgb = r.surface.0;
        let is_light = luminance(bg_rgb) > 0.5;

        // Card hover lift: brighter on dark surfaces, crisper paper on light ones.
        let elev_rgb = if is_light {
            r.surface.mix(Hex(WHITE), 0.55).0
        } else {
            r.surface.mix(r.text, 0.055).0
        };

        // Recessed header strip: darker on dark surfaces, gently tinted on light ones.
        let hdr_rgb = if is_light {
            r.surface.mix(r.text, 0.06).0
        } else {
            r.surface.mix(Hex(BLACK), 0.22).0
        };

        // Neutral structural border clearing 3.0:1 against both rest & hovered surfaces.
        let border_base = r.surface.mix(r.text, 0.28).0;
        let border_rgb = readable(border_base, r.text.0, bg_rgb, 3.0);
        let border_rgb = readable(border_rgb, r.text.0, elev_rgb, 3.0);

        // Body and muted text clearing 4.5:1 on card, hover, and recessed header surfaces.
        let body_rgb = readable(r.text.0, extreme(elev_rgb), elev_rgb, 4.5);
        let body_rgb = readable(body_rgb, extreme(bg_rgb), bg_rgb, 4.5);
        let body_rgb = readable(body_rgb, extreme(hdr_rgb), hdr_rgb, 4.5);

        let muted_rgb = readable(r.dim.0, body_rgb, hdr_rgb, 4.5);
        let muted_rgb = readable(muted_rgb, body_rgb, elev_rgb, 4.5);
        let muted_rgb = readable(muted_rgb, body_rgb, bg_rgb, 4.5);

        // Accent: 3.0:1 for glyphs/bars, 4.5:1 for titles, tags, and OSD percentages.
        let accent_ui_rgb = readable(r.accent.0, body_rgb, elev_rgb, 3.0);
        let accent_txt_rgb = readable(r.accent.0, body_rgb, hdr_rgb, 4.5);
        let accent_txt_rgb = readable(accent_txt_rgb, body_rgb, elev_rgb, 4.5);
        let accent_txt_rgb = readable(accent_txt_rgb, body_rgb, bg_rgb, 4.5);

        // Warning & critical roles lifted against header strip and danger pill fills too.
        let warn_txt_rgb = readable(r.warn.0, body_rgb, hdr_rgb, 4.5);
        let warn_txt_rgb = readable(warn_txt_rgb, body_rgb, bg_rgb, 4.5);

        let danger_ui_rgb = readable(r.danger.0, body_rgb, elev_rgb, 3.0);
        let danger_hdr_bg = Hex(bg_rgb).mix(r.danger_bg, 0.78).0;
        let danger_txt_rgb = readable(r.danger.0, extreme(r.danger_bg.0), r.danger_bg.0, 4.5);
        let danger_txt_rgb = readable(danger_txt_rgb, body_rgb, danger_hdr_bg, 4.5);
        let danger_txt_rgb = readable(danger_txt_rgb, body_rgb, elev_rgb, 4.5);
        let danger_txt_rgb = readable(danger_txt_rgb, body_rgb, bg_rgb, 4.5);

        // Action keycaps: calm raised neutral at rest, solid accent + inverted ink on hover.
        let btn_bg_rgb = if is_light {
            r.surface.mix(Hex(WHITE), 0.70).0
        } else {
            r.surface.mix(r.text, 0.08).0
        };
        let btn_fg_rgb = readable(body_rgb, extreme(btn_bg_rgb), btn_bg_rgb, 4.5);
        let btn_hov_rgb = r.accent.0;
        let btn_fg_hov_rgb = readable(
            theme.colors.background.0,
            extreme(btn_hov_rgb),
            btn_hov_rgb,
            4.5,
        );
        let btn_bevel_hov_rgb = Hex(btn_hov_rgb).mix(Hex(BLACK), 0.25).0;

        let track_rgb = r.surface.mix(r.text, 0.14).0;

        Self {
            bg: to_color(bg_rgb, 246),
            bg_elevated: to_color(elev_rgb, 252),
            header_bg: to_color(hdr_rgb, 248),
            border_subtle: to_color(border_rgb, 235),
            accent_text: to_color(accent_txt_rgb, 255),
            text_body: to_color(body_rgb, 250),
            text_dim: to_color(muted_rgb, 255),
            accent: to_color(accent_ui_rgb, 255),
            accent_alt: to_color(r.accent_alt.0, 255),
            danger: to_color(danger_ui_rgb, 255),
            danger_text: to_color(danger_txt_rgb, 255),
            danger_bg: to_color(r.danger_bg.0, 235),
            warn: to_color(r.warn.0, 255),
            warn_text: to_color(warn_txt_rgb, 255),
            ok: to_color(r.ok.0, 255),
            ok_text: to_color(r.ok_text.0, 255),
            track_bg: to_color(track_rgb, 220),
            btn_bg: to_color(btn_bg_rgb, 245),
            btn_fg: to_color(btn_fg_rgb, 255),
            btn_bg_hover: to_color(btn_hov_rgb, 255),
            btn_fg_hover: to_color(btn_fg_hov_rgb, 255),
            btn_bevel_hover: to_color(btn_bevel_hov_rgb, 255),
        }
    }
}

/// Format the bracketed Cyber-HUD header tag: `[ SYS // INFO ]`, `[ ALERT // CRIT ]`,
/// `[ APP // SYNC ]`, etc.
pub fn format_header_tag(
    app_name: &str,
    urgency: Urgency,
    has_progress: bool,
    has_open_path: bool,
) -> String {
    let trimmed = app_name.trim();
    let source = if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("hayato")
        || trimmed.eq_ignore_ascii_case("notify-send")
    {
        if urgency == Urgency::Critical {
            "ALERT".to_string()
        } else {
            "SYS".to_string()
        }
    } else {
        let cleaned: String = trimmed
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(10)
            .collect::<String>()
            .to_ascii_uppercase();
        if cleaned.is_empty() {
            "APP".to_string()
        } else {
            cleaned
        }
    };

    let level = match urgency {
        Urgency::Critical => "CRIT",
        Urgency::Low => "LOW",
        Urgency::Normal => {
            if has_progress {
                "SYNC"
            } else if has_open_path {
                "FILE"
            } else {
                "INFO"
            }
        }
    };

    format!("[ {source} // {level} ]")
}

pub struct LayoutMetrics {
    pub total_width: f32,
    pub total_height: f32,
    #[allow(dead_code)]
    pub hidden_count: usize,
}

struct ActionChip {
    key: String,
    label: String,
    width: f32,
}

struct CardLayout {
    body_lines: Vec<String>,
    progress_y: Option<f32>,
    actions_y: Option<f32>,
    action_rows: Vec<Vec<ActionChip>>,
    height: f32,
}

pub struct Renderer {
    pub font: FontRenderer,
    pub theme: CardTheme,
    pub card_width: f32,
    pub max_visible: usize,
    pub reduced_motion: bool,
}

impl Renderer {
    pub fn with_layout(card_width: f32, max_visible: usize) -> Self {
        Self {
            font: FontRenderer::new(),
            theme: CardTheme::from_active(),
            card_width,
            max_visible: max_visible.max(1),
            reduced_motion: crate::model::reduced_motion_enabled(),
        }
    }

    pub fn add_rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.quad_to(x + w, y, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.quad_to(x + w, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.quad_to(x, y + h, x, y + h - r);
        pb.line_to(x, y + r);
        pb.quad_to(x, y, x + r, y);
        pb.close();
    }

    fn add_top_rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let r = r.min(w * 0.5).min(h).max(0.0);
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.quad_to(x + w, y, x + w, y + r);
        pb.line_to(x + w, y + h);
        pb.line_to(x, y + h);
        pb.line_to(x, y + r);
        pb.quad_to(x, y, x + r, y);
        pb.close();
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_segmented_bar(
        &self,
        pixmap: &mut PixmapMut,
        transform: Transform,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        segments: usize,
        pct: u8,
        muted: bool,
        fill_color: Color,
        alpha: f32,
    ) {
        let segments = segments.max(1);
        let total_gap = SEGMENT_GAP * (segments - 1) as f32;
        let seg_w = ((width - total_gap) / segments as f32).max(2.0);
        let filled = if muted || pct == 0 {
            0
        } else {
            let raw = ((pct.min(100) as f32 / 100.0) * segments as f32).round() as usize;
            raw.clamp(1, segments)
        };

        let mut empty_paint = Paint::default();
        empty_paint.set_color(with_alpha(self.theme.track_bg, alpha));
        empty_paint.anti_alias = true;

        let mut fill_paint = Paint::default();
        fill_paint.set_color(with_alpha(fill_color, alpha));
        fill_paint.anti_alias = true;

        for i in 0..segments {
            let sx = x + i as f32 * (seg_w + SEGMENT_GAP);
            if sx >= x + width {
                break;
            }
            let sw = seg_w.min(x + width - sx);
            let mut pb = PathBuilder::new();
            Self::add_rounded_rect(&mut pb, sx, y, sw, height, 1.0);
            if let Some(path) = pb.finish() {
                let paint = if i < filled {
                    &fill_paint
                } else {
                    &empty_paint
                };
                pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
            }
        }
    }

    #[allow(dead_code)]
    pub fn add_circle(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32) {
        Self::add_rounded_rect(pb, cx - r, cy - r, r * 2.0, r * 2.0, r);
    }

    fn plan_card_layout(&self, notif: &Notification, card_w: f32) -> CardLayout {
        let text_max_width = (card_w - CONTENT_LEFT_PAD - CONTENT_RIGHT_PAD).max(80.0);
        let body_max_width = (text_max_width - PROMPT_INDENT).max(60.0);
        let body_lines = if notif.body.is_empty() {
            Vec::new()
        } else {
            self.font
                .wrap_text(&notif.body, 11.5, body_max_width, 4, false)
        };

        // Plan keycap-style `[ Label ]` action button rows so buttons never overflow.
        let mut action_rows: Vec<Vec<ActionChip>> = Vec::new();
        if !notif.actions.is_empty() {
            let mut cur_row: Vec<ActionChip> = Vec::new();
            let mut cur_row_w = 0.0f32;
            let bracket_extra = self.font.measure_width("[  ]", 10.0, true);
            for action in &notif.actions {
                let max_inner_w = (text_max_width - bracket_extra - 20.0).max(32.0);
                let raw = self
                    .font
                    .truncate_text(&action.label, 10.0, max_inner_w, true);
                let label = format!("[ {raw} ]");
                let btn_w =
                    (self.font.measure_width(&label, 10.0, true) + 18.0).min(text_max_width);
                let needed = if cur_row.is_empty() {
                    btn_w
                } else {
                    cur_row_w + ACTION_BTN_GAP + btn_w
                };
                if !cur_row.is_empty() && needed > text_max_width {
                    action_rows.push(std::mem::take(&mut cur_row));
                    cur_row_w = 0.0;
                }
                if !cur_row.is_empty() {
                    cur_row_w += ACTION_BTN_GAP;
                }
                cur_row_w += btn_w;
                cur_row.push(ActionChip {
                    key: action.key.clone(),
                    label,
                    width: btn_w,
                });
            }
            if !cur_row.is_empty() {
                action_rows.push(cur_row);
            }
        }

        // Top terminal header strip occupies 0..HEADER_H.
        // Summary baseline sits at HEADER_H + 17.0.
        let mut cursor_y = HEADER_H + 21.0;
        if !body_lines.is_empty() {
            cursor_y += 4.0 + (body_lines.len() as f32 * 15.5);
        }

        let progress_y = if notif.progress.is_some() {
            let py = cursor_y + 7.0;
            cursor_y = py + PROGRESS_BAR_H + 6.0;
            Some(py)
        } else {
            None
        };

        let actions_y = if !action_rows.is_empty() {
            let ay = cursor_y + 6.0;
            let rows_h = action_rows.len() as f32 * ACTION_BTN_H
                + action_rows.len().saturating_sub(1) as f32 * ACTION_ROW_GAP;
            cursor_y = ay + rows_h;
            Some(ay)
        } else {
            None
        };

        let bottom_pad = 10.0f32;
        let height = (cursor_y + bottom_pad).max(52.0);

        CardLayout {
            body_lines,
            progress_y,
            actions_y,
            action_rows,
            height,
        }
    }

    pub fn compute_layout(&self, notifications: &mut [Notification]) -> LayoutMetrics {
        let start = notifications.len().saturating_sub(self.max_visible);
        let hidden_count = start;
        let mut max_w = 0.0f32;
        for notif in notifications[start..].iter() {
            let w = if notif.is_osd() {
                OSD_WIDTH
            } else {
                self.card_width
            };
            if w > max_w {
                max_w = w;
            }
        }
        if max_w == 0.0 {
            max_w = self.card_width;
        }

        let mut cur_y = 0.0f32;
        for (index, notif) in notifications.iter_mut().enumerate() {
            if index < start {
                notif.card_y = 0.0;
                notif.card_height = 0.0;
                notif.card_bounds = Rect::new(0.0, 0.0, 0.0, 0.0);
                notif.action_rects.clear();
                notif.close_rect = None;
                notif.set_hovered(false);
                notif.hovered_close = false;
                notif.hovered_action = None;
                continue;
            }
            let card_w = if notif.is_osd() {
                OSD_WIDTH
            } else {
                self.card_width
            };
            let card_x = max_w - card_w;

            if notif.is_osd() {
                let height = OSD_HEIGHT;
                notif.card_y = cur_y;
                notif.card_height = height;
                notif.card_bounds = Rect::new(card_x, cur_y, card_w, height);
                cur_y += height + CARD_SPACING;
                continue;
            }

            let plan = self.plan_card_layout(notif, card_w);
            notif.card_y = cur_y;
            notif.card_height = plan.height;
            notif.card_bounds = Rect::new(card_x, cur_y, card_w, plan.height);

            cur_y += plan.height + CARD_SPACING;
        }

        if hidden_count > 0 {
            cur_y += OVERFLOW_PILL_H + 2.0;
        }

        LayoutMetrics {
            total_width: max_w,
            total_height: cur_y.max(1.0),
            hidden_count,
        }
    }

    pub fn render(&self, pixmap: &mut PixmapMut, notifications: &mut [Notification]) {
        pixmap.fill(Color::TRANSPARENT);

        let hidden_count = notifications.len().saturating_sub(self.max_visible);
        let mut last_bottom = 0.0f32;
        let max_w = pixmap.width() as f32;

        for notif in notifications.iter_mut() {
            if notif.card_bounds.w <= 0.0 || notif.card_bounds.h <= 0.0 {
                continue;
            }
            self.render_card(pixmap, notif);
            let bottom = notif.card_bounds.y + notif.card_bounds.h;
            if bottom > last_bottom {
                last_bottom = bottom;
            }
        }

        if hidden_count > 0 {
            self.render_overflow_pill(pixmap, max_w, last_bottom + CARD_SPACING, hidden_count);
        }
    }

    fn render_overflow_pill(&self, pixmap: &mut PixmapMut, max_w: f32, y: f32, hidden: usize) {
        let label = format!("[ +{hidden} queued ]");
        let text_w = self.font.measure_width(&label, 10.0, true);
        let pill_w = text_w + 18.0;
        let pill_h = OVERFLOW_PILL_H;
        let pill_x = (max_w - pill_w).max(0.0);

        let mut pb = PathBuilder::new();
        Self::add_rounded_rect(&mut pb, pill_x, y, pill_w, pill_h, CORNER_RADIUS);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(self.theme.header_bg);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let mut bpb = PathBuilder::new();
        Self::add_rounded_rect(
            &mut bpb,
            pill_x + 0.5,
            y + 0.5,
            (pill_w - 1.0).max(1.0),
            (pill_h - 1.0).max(1.0),
            CORNER_RADIUS,
        );
        if let Some(bpath) = bpb.finish() {
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut bpaint = Paint::default();
            bpaint.set_color(self.theme.border_subtle);
            bpaint.anti_alias = true;
            pixmap.stroke_path(&bpath, &bpaint, &stroke, Transform::identity(), None);
        }

        self.font.draw_text(
            pixmap,
            &label,
            pill_x + 9.0,
            y + 14.5,
            10.0,
            self.theme.text_body,
            true,
        );
    }

    pub fn resolve_glyph(icon: &str, app: &str, summary: &str, urgency: Urgency) -> &'static str {
        let icon_lower = icon.to_lowercase();
        let app_lower = app.to_lowercase();
        let summary_lower = summary.to_lowercase();

        if icon_lower.contains("muted") || summary_lower.contains("muted") {
            "\u{f466}" // oct-mute
        } else if icon_lower.contains("volume")
            || app_lower.contains("volume")
            || summary_lower.contains("volume")
        {
            hayato_ui::Glyph::Volume.cell()
        } else if icon_lower.contains("brightness")
            || app_lower.contains("brightness")
            || summary_lower.contains("brightness")
        {
            hayato_ui::Glyph::Brightness.cell()
        } else if icon_lower.contains("wifi")
            || icon_lower.contains("network")
            || app_lower.contains("wifi")
            || summary_lower.contains("wifi")
        {
            hayato_ui::Glyph::Wifi.cell()
        } else if icon_lower.contains("bluetooth")
            || app_lower.contains("bluetooth")
            || summary_lower.contains("bluetooth")
        {
            hayato_ui::Glyph::Bluetooth.cell()
        } else if icon_lower.contains("battery")
            || app_lower.contains("battery")
            || summary_lower.contains("battery")
            || summary_lower.contains("power")
        {
            hayato_ui::Glyph::Battery.cell()
        } else if icon_lower.contains("camera")
            || app_lower.contains("camera")
            || summary_lower.contains("capture")
            || summary_lower.contains("screenshot")
        {
            hayato_ui::Glyph::Image.cell()
        } else if icon_lower.contains("download") || app_lower.contains("download") {
            hayato_ui::Glyph::Download.cell()
        } else if icon_lower.contains("terminal")
            || app_lower.contains("terminal")
            || app_lower.contains("foot")
        {
            hayato_ui::Glyph::Terminal.cell()
        } else if icon_lower.contains("music")
            || icon_lower.contains("media")
            || app_lower.contains("music")
            || app_lower.contains("player")
        {
            hayato_ui::Glyph::Music.cell()
        } else if icon_lower.contains("mic")
            || icon_lower.contains("record")
            || app_lower.contains("dictat")
            || summary_lower.contains("dictat")
        {
            hayato_ui::Glyph::Mic.cell()
        } else if icon_lower.contains("qr") || summary_lower.contains("qr") {
            hayato_ui::Glyph::Qr.cell()
        } else if icon_lower.contains("settings") || app_lower.contains("settings") {
            hayato_ui::Glyph::Gear.cell()
        } else if urgency == Urgency::Critical
            || icon_lower.contains("error")
            || summary_lower.contains("error")
        {
            hayato_ui::Glyph::Warning.cell()
        } else {
            hayato_ui::Glyph::Info.cell()
        }
    }

    fn render_osd_card(&self, pixmap: &mut PixmapMut, notif: &mut Notification) {
        let card_x = notif.card_bounds.x;
        let card_y = notif.card_bounds.y;
        let card_w = notif.card_bounds.w;
        let card_h = notif.card_bounds.h;

        notif.close_rect = None;
        notif.action_rects.clear();

        let (scale, alpha) = notif.animation_factors(self.reduced_motion);
        let cx = card_x + card_w * 0.5;
        let cy = card_y + card_h * 0.5;
        let transform = Transform::from_translate(-cx, -cy)
            .post_scale(scale, scale)
            .post_translate(cx, cy);

        let map_x = |x: f32| cx + (x - cx) * scale;
        let map_y = |y: f32| cy + (y - cy) * scale;

        let is_muted = notif.body.to_lowercase().contains("mute")
            || notif.app_icon.contains("muted")
            || notif.summary.to_lowercase().contains("mute");

        let is_brightness = notif.summary.to_lowercase().contains("brightness")
            || notif.app_icon.contains("brightness");

        let icon_glyph = if is_brightness {
            hayato_ui::Glyph::Brightness.cell()
        } else if is_muted {
            "\u{f466}" // oct-mute
        } else {
            hayato_ui::Glyph::Volume.cell()
        };

        let icon_color = self.theme.accent;

        // 1. Background
        let mut bg_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut bg_builder,
            card_x,
            card_y,
            card_w,
            card_h,
            CORNER_RADIUS,
        );
        if let Some(bg_path) = bg_builder.finish() {
            let mut paint = Paint::default();
            paint.set_color(with_alpha(self.theme.bg, alpha));
            paint.anti_alias = true;
            pixmap.fill_path(&bg_path, &paint, FillRule::Winding, transform, None);
        }

        // 2. Border
        let mut border_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut border_builder,
            card_x + 0.5,
            card_y + 0.5,
            card_w - 1.0,
            card_h - 1.0,
            CORNER_RADIUS,
        );
        if let Some(border_path) = border_builder.finish() {
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut paint = Paint::default();
            paint.set_color(with_alpha(self.theme.border_subtle, alpha));
            paint.anti_alias = true;
            pixmap.stroke_path(&border_path, &paint, &stroke, transform, None);
        }

        // 4. Icon glyph
        self.font.draw_text(
            pixmap,
            icon_glyph,
            map_x(card_x + 12.0),
            map_y(card_y + 26.0),
            15.0 * scale,
            with_alpha(icon_color, alpha),
            false,
        );

        let content_x = card_x + 34.0;
        let right_margin = card_x + card_w - 12.0;

        let effective_pct = if let Some(pct) = notif.progress {
            pct.min(100)
        } else if let Some(n) = notif
            .body
            .trim()
            .strip_suffix('%')
            .and_then(|s| s.trim().parse::<u16>().ok())
        {
            (n.min(100)) as u8
        } else {
            0
        };

        let val_str = if is_muted {
            "[ MUTED ]".to_string()
        } else if notif.progress.is_some() || notif.body.trim().ends_with('%') {
            format!("[ {effective_pct:>3}% ]")
        } else if !notif.body.is_empty() {
            format!("[ {} ]", notif.body.trim())
        } else {
            "[   0% ]".to_string()
        };

        let val_w = self.font.measure_width(&val_str, 10.5, true);
        let val_color = self.theme.accent_text;

        // Terminal chevron `> ` + uppercase HUD label
        let chevron = "> ";
        let chevron_w = self.font.measure_width(chevron, 11.5, true);
        self.font.draw_text(
            pixmap,
            chevron,
            map_x(content_x),
            map_y(card_y + 17.0),
            11.5 * scale,
            with_alpha(icon_color, alpha),
            true,
        );

        let summary_max_w = (right_margin - (content_x + chevron_w) - val_w - 6.0).max(36.0);
        let label_upper = notif.summary.to_ascii_uppercase();
        let summary = self
            .font
            .truncate_text(&label_upper, 11.5, summary_max_w, true);

        self.font.draw_text(
            pixmap,
            &summary,
            map_x(content_x + chevron_w),
            map_y(card_y + 17.0),
            11.5 * scale,
            with_alpha(self.theme.accent_text, alpha),
            true,
        );

        self.font.draw_text(
            pixmap,
            &val_str,
            map_x(right_margin - val_w),
            map_y(card_y + 17.0),
            10.5 * scale,
            with_alpha(val_color, alpha),
            true,
        );

        // 5. Segmented block level bar
        let track_x = content_x;
        let track_y = card_y + 26.0;
        let track_w = (right_margin - content_x).max(40.0);
        self.draw_segmented_bar(
            pixmap,
            transform,
            track_x,
            track_y,
            track_w,
            PROGRESS_BAR_H,
            OSD_SEGMENTS,
            effective_pct,
            is_muted,
            icon_color,
            alpha,
        );
    }

    fn render_card(&self, pixmap: &mut PixmapMut, notif: &mut Notification) {
        if notif.is_osd() {
            self.render_osd_card(pixmap, notif);
            return;
        }

        let card_x = notif.card_bounds.x;
        let card_y = notif.card_bounds.y;
        let card_w = notif.card_bounds.w;
        let card_h = notif.card_bounds.h;

        let plan = self.plan_card_layout(notif, card_w);

        let (scale, alpha) = notif.animation_factors(self.reduced_motion);
        let cx = card_x + card_w * 0.5;
        let cy = card_y + card_h * 0.5;
        let transform = Transform::from_translate(-cx, -cy)
            .post_scale(scale, scale)
            .post_translate(cx, cy);

        let map_x = |x: f32| cx + (x - cx) * scale;
        let map_y = |y: f32| cy + (y - cy) * scale;

        let glyph_color = match notif.urgency {
            Urgency::Low => self.theme.text_dim,
            Urgency::Normal => self.theme.accent,
            Urgency::Critical => self.theme.danger,
        };
        let tag_color = match notif.urgency {
            Urgency::Low => self.theme.text_dim,
            Urgency::Normal => self.theme.accent_text,
            Urgency::Critical => self.theme.danger_text,
        };

        // 1. Background
        let mut bg_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut bg_builder,
            card_x,
            card_y,
            card_w,
            card_h,
            CORNER_RADIUS,
        );
        if let Some(bg_path) = bg_builder.finish() {
            let mut paint = Paint::default();
            let bg_color = if notif.is_hovered {
                self.theme.bg_elevated
            } else {
                self.theme.bg
            };
            paint.set_color(with_alpha(bg_color, alpha));
            paint.anti_alias = true;
            pixmap.fill_path(&bg_path, &paint, FillRule::Winding, transform, None);
        }

        // 2. Top terminal header strip (recessed Cyber-HUD titlebar)
        let mut hdr_builder = PathBuilder::new();
        Self::add_top_rounded_rect(
            &mut hdr_builder,
            card_x,
            card_y,
            card_w,
            HEADER_H,
            CORNER_RADIUS,
        );
        if let Some(hdr_path) = hdr_builder.finish() {
            let mut paint = Paint::default();
            let hdr_fill = if notif.urgency == Urgency::Critical {
                with_alpha(self.theme.danger_bg, 0.78)
            } else {
                self.theme.header_bg
            };
            paint.set_color(with_alpha(hdr_fill, alpha));
            paint.anti_alias = true;
            pixmap.fill_path(&hdr_path, &paint, FillRule::Winding, transform, None);
        }

        // 1px header divider rule
        let mut rule_builder = PathBuilder::new();
        rule_builder.move_to(card_x + 1.0, card_y + HEADER_H);
        rule_builder.line_to(card_x + card_w - 1.0, card_y + HEADER_H);
        if let Some(rule_path) = rule_builder.finish() {
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut paint = Paint::default();
            let rule_color = if notif.urgency == Urgency::Critical {
                with_alpha(self.theme.danger, 0.60)
            } else {
                with_alpha(self.theme.border_subtle, 0.45)
            };
            paint.set_color(with_alpha(rule_color, alpha));
            paint.anti_alias = false;
            pixmap.stroke_path(&rule_path, &paint, &stroke, transform, None);
        }

        // 3. Outer border (Critical gets danger border; hovered file cards highlight accent)
        let border_color = if notif.urgency == Urgency::Critical {
            self.theme.danger
        } else if notif.is_hovered && notif.open_path.is_some() {
            self.theme.accent
        } else {
            self.theme.border_subtle
        };
        let mut border_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut border_builder,
            card_x + 0.5,
            card_y + 0.5,
            card_w - 1.0,
            card_h - 1.0,
            CORNER_RADIUS,
        );
        if let Some(border_path) = border_builder.finish() {
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut paint = Paint::default();
            paint.set_color(with_alpha(border_color, alpha));
            paint.anti_alias = true;
            pixmap.stroke_path(&border_path, &paint, &stroke, transform, None);
        }

        // 5. Header strip content: glyph + `[ <APP> // <LVL> ]` + right-aligned badges & close
        let glyph = Self::resolve_glyph(
            &notif.app_icon,
            &notif.app_name,
            &notif.summary,
            notif.urgency,
        );
        let header_baseline = card_y + 14.5;
        let content_x = card_x + CONTENT_LEFT_PAD;

        self.font.draw_text(
            pixmap,
            glyph,
            map_x(content_x),
            map_y(header_baseline + 0.5),
            11.0 * scale,
            with_alpha(glyph_color, alpha),
            false,
        );

        // Close `[×]` button lives strictly inside the top header strip (never collides with actions)
        let hit_w = CLOSE_HIT_SIZE * scale;
        let hit_h = HEADER_H * scale;
        let close_rect = Rect::new(
            map_x(card_x + card_w - CLOSE_HIT_SIZE - 2.0),
            map_y(card_y),
            hit_w,
            hit_h,
        );
        notif.close_rect = Some(close_rect);

        let pill_x = card_x + card_w - 23.0;
        let pill_y = card_y + 2.0;
        let pill_w = 20.0;
        let pill_h = HEADER_H - 4.0;
        if notif.hovered_close {
            let mut pb = PathBuilder::new();
            Self::add_rounded_rect(&mut pb, pill_x, pill_y, pill_w, pill_h, 2.0);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(with_alpha(self.theme.danger_bg, alpha));
                paint.anti_alias = true;
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
            let mut bpb = PathBuilder::new();
            Self::add_rounded_rect(
                &mut bpb,
                pill_x + 0.5,
                pill_y + 0.5,
                pill_w - 1.0,
                pill_h - 1.0,
                2.0,
            );
            if let Some(bpath) = bpb.finish() {
                let stroke = Stroke {
                    width: 1.0,
                    ..Default::default()
                };
                let mut bpaint = Paint::default();
                bpaint.set_color(with_alpha(self.theme.danger, alpha * 0.65));
                bpaint.anti_alias = true;
                pixmap.stroke_path(&bpath, &bpaint, &stroke, transform, None);
            }
        }

        let close_color = if notif.hovered_close {
            self.theme.danger_text
        } else if notif.is_hovered {
            self.theme.text_body
        } else {
            self.theme.text_dim
        };
        let close_glyph = "\u{f00d}";
        let close_glyph_w = self.font.measure_width(close_glyph, 10.5, false);
        self.font.draw_text(
            pixmap,
            close_glyph, // Font Awesome xmark
            map_x(pill_x + (pill_w - close_glyph_w) * 0.5),
            map_y(header_baseline + 0.5),
            10.5 * scale,
            with_alpha(close_color, alpha),
            false,
        );

        let mut header_right = card_x + card_w - 26.0;

        // Repeat badge `[×N]` in header strip
        if notif.repeat_count > 1 {
            let label = format!("[\u{00d7}{}]", notif.repeat_count);
            let label_w = self.font.measure_width(&label, 9.0, true);
            let badge_x = header_right - label_w - 4.0;
            self.font.draw_text(
                pixmap,
                &label,
                map_x(badge_x),
                map_y(header_baseline),
                9.0 * scale,
                with_alpha(self.theme.text_body, alpha),
                true,
            );
            header_right = badge_x - 6.0;
        }

        // Subtle `[HOLD]` indicator when hovering pauses a timed notification
        let tag_x = content_x + 15.0;
        let raw_tag = format_header_tag(
            &notif.app_name,
            notif.urgency,
            notif.progress.is_some(),
            notif.open_path.is_some(),
        );
        let tag_w = self.font.measure_width(&raw_tag, 9.5, true);
        if notif.is_hovered && notif.remaining_fraction().is_some() {
            let hold_label = "[HOLD]";
            let hold_w = self.font.measure_width(hold_label, 8.5, true);
            if header_right - hold_w - 8.0 > tag_x + tag_w {
                let hold_x = header_right - hold_w - 2.0;
                self.font.draw_text(
                    pixmap,
                    hold_label,
                    map_x(hold_x),
                    map_y(header_baseline),
                    8.5 * scale,
                    with_alpha(self.theme.warn_text, alpha),
                    true,
                );
                header_right = hold_x - 6.0;
            }
        }

        let tag_max_w = (header_right - tag_x).max(40.0);
        let tag_text = self.font.truncate_text(&raw_tag, 9.5, tag_max_w, true);
        self.font.draw_text(
            pixmap,
            &tag_text,
            map_x(tag_x),
            map_y(header_baseline),
            9.5 * scale,
            with_alpha(tag_color, alpha),
            true,
        );

        // 6. Summary line with terminal prompt chevron `> `
        let summary_baseline = card_y + HEADER_H + 17.0;
        let summary_right = card_x + card_w - CONTENT_RIGHT_PAD;
        self.font.draw_text(
            pixmap,
            ">",
            map_x(content_x),
            map_y(summary_baseline),
            12.0 * scale,
            with_alpha(glyph_color, alpha),
            true,
        );

        let text_x = content_x + PROMPT_INDENT;
        let summary_max_w = (summary_right - text_x).max(40.0);
        let summary = self
            .font
            .truncate_text(&notif.summary, 12.0, summary_max_w, true);
        let summary_color = match notif.urgency {
            Urgency::Critical => self.theme.danger_text,
            Urgency::Low => self.theme.text_body,
            Urgency::Normal => self.theme.accent_text,
        };
        self.font.draw_text(
            pixmap,
            &summary,
            map_x(text_x),
            map_y(summary_baseline),
            12.0 * scale,
            with_alpha(summary_color, alpha),
            true,
        );

        // 7. Body lines (indented cleanly under summary text)
        let mut body_y = summary_baseline + 4.0;
        for line in &plan.body_lines {
            body_y += 15.5;
            self.font.draw_text(
                pixmap,
                line,
                map_x(text_x),
                map_y(body_y),
                11.5 * scale,
                with_alpha(self.theme.text_body, alpha),
                false,
            );
        }

        // 8. Segmented Block Progress Bar + right-aligned monospace `%` readout
        let text_max_width = (card_w - CONTENT_LEFT_PAD - CONTENT_RIGHT_PAD).max(80.0);
        if let (Some(pct), Some(py_off)) = (notif.progress, plan.progress_y) {
            let bar_y = card_y + py_off;
            let pct_label = format!("{:>3}%", pct.min(100));
            let pct_w = self.font.measure_width(&pct_label, 10.0, true);
            let bar_x = text_x;
            let bar_w = (summary_right - bar_x - pct_w - 8.0).max(40.0);
            let fill_color = if let Some([r, g, b, a]) = notif.progress_color_override {
                Color::from_rgba8(r, g, b, a)
            } else {
                glyph_color
            };

            self.draw_segmented_bar(
                pixmap,
                transform,
                bar_x,
                bar_y,
                bar_w,
                PROGRESS_BAR_H,
                CARD_SEGMENTS,
                pct,
                false,
                fill_color,
                alpha,
            );

            self.font.draw_text(
                pixmap,
                &pct_label,
                map_x(summary_right - pct_w),
                map_y(bar_y + PROGRESS_BAR_H + 1.0),
                10.0 * scale,
                with_alpha(tag_color, alpha),
                true,
            );
        }

        // 9. Keycap-style `[ Label ]` action pills
        notif.action_rects.clear();
        if let Some(ay_off) = plan.actions_y {
            let mut row_y = card_y + ay_off;
            for row in &plan.action_rows {
                let mut btn_x = card_x + card_w - CONTENT_RIGHT_PAD;
                for chip in row.iter().rev() {
                    btn_x -= chip.width;
                    let rect = Rect::new(
                        map_x(btn_x),
                        map_y(row_y),
                        chip.width * scale,
                        ACTION_BTN_H * scale,
                    );
                    notif.action_rects.push((chip.key.clone(), rect));

                    let is_btn_hovered = notif.hovered_action.as_deref() == Some(&chip.key);

                    let mut btn_builder = PathBuilder::new();
                    Self::add_rounded_rect(
                        &mut btn_builder,
                        btn_x,
                        row_y,
                        chip.width,
                        ACTION_BTN_H,
                        CORNER_RADIUS,
                    );
                    if let Some(btn_path) = btn_builder.finish() {
                        let mut paint = Paint::default();
                        let btn_bg = if is_btn_hovered {
                            self.theme.btn_bg_hover
                        } else {
                            self.theme.btn_bg
                        };
                        paint.set_color(with_alpha(btn_bg, alpha));
                        paint.anti_alias = true;
                        pixmap.fill_path(&btn_path, &paint, FillRule::Winding, transform, None);

                        // Subtle tactile keycap bottom bevel inset inside the stroke
                        let mut bevel_pb = PathBuilder::new();
                        Self::add_rounded_rect(
                            &mut bevel_pb,
                            btn_x + 1.5,
                            row_y + ACTION_BTN_H - 2.5,
                            (chip.width - 3.0).max(2.0),
                            1.5,
                            0.5,
                        );
                        if let Some(bevel_path) = bevel_pb.finish() {
                            let mut bevel_paint = Paint::default();
                            let bevel_col = if is_btn_hovered {
                                self.theme.btn_bevel_hover
                            } else {
                                with_alpha(self.theme.border_subtle, 0.65)
                            };
                            bevel_paint.set_color(with_alpha(bevel_col, alpha));
                            bevel_paint.anti_alias = true;
                            pixmap.fill_path(
                                &bevel_path,
                                &bevel_paint,
                                FillRule::Winding,
                                transform,
                                None,
                            );
                        }
                    }

                    let mut btn_stroke_pb = PathBuilder::new();
                    Self::add_rounded_rect(
                        &mut btn_stroke_pb,
                        btn_x + 0.5,
                        row_y + 0.5,
                        (chip.width - 1.0).max(2.0),
                        (ACTION_BTN_H - 1.0).max(2.0),
                        CORNER_RADIUS,
                    );
                    if let Some(btn_stroke_path) = btn_stroke_pb.finish() {
                        let stroke = Stroke {
                            width: 1.0,
                            ..Default::default()
                        };
                        let mut bpaint = Paint::default();
                        let btn_border = if is_btn_hovered {
                            self.theme.accent
                        } else {
                            self.theme.border_subtle
                        };
                        bpaint.set_color(with_alpha(btn_border, alpha));
                        bpaint.anti_alias = true;
                        pixmap.stroke_path(&btn_stroke_path, &bpaint, &stroke, transform, None);
                    }

                    let label_color = if is_btn_hovered {
                        self.theme.btn_fg_hover
                    } else {
                        self.theme.btn_fg
                    };

                    self.font.draw_text(
                        pixmap,
                        &chip.label,
                        map_x(btn_x + 9.0),
                        map_y(row_y + 16.5),
                        10.0 * scale,
                        with_alpha(label_color, alpha),
                        true,
                    );

                    btn_x -= ACTION_BTN_GAP;
                }
                row_y += ACTION_BTN_H + ACTION_ROW_GAP;
            }
        }

        // 10. Subtle recessed 2px bottom TTL countdown rail (freezes visibly on hover)
        let _ = text_max_width;
        if let Some(rem) = notif.remaining_fraction() {
            if rem > 0.01 {
                let inner_w = (card_w - 16.0).max(0.0);
                let line_x = card_x + 8.0;
                let line_y = card_y + card_h - 3.5;

                // Recessed track
                let mut track_pb = PathBuilder::new();
                Self::add_rounded_rect(&mut track_pb, line_x, line_y, inner_w, 2.0, 1.0);
                if let Some(path) = track_pb.finish() {
                    let mut paint = Paint::default();
                    paint.set_color(with_alpha(self.theme.track_bg, alpha * 0.5));
                    paint.anti_alias = true;
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }

                // Active remaining TTL fill
                let line_w = inner_w * rem;
                let mut pb = PathBuilder::new();
                Self::add_rounded_rect(&mut pb, line_x, line_y, line_w, 2.0, 1.0);
                if let Some(path) = pb.finish() {
                    let mut paint = Paint::default();
                    let line_alpha = if notif.is_hovered { 0.8 } else { 0.5 };
                    let ttl_color = if notif.urgency == Urgency::Low {
                        self.theme.border_subtle
                    } else {
                        glyph_color
                    };
                    paint.set_color(with_alpha(ttl_color, alpha * line_alpha));
                    paint.anti_alias = true;
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActionItem, AnimState};
    use std::time::{Duration, Instant};

    fn card(id: u32) -> Notification {
        Notification {
            id,
            app_name: "hayato".to_string(),
            summary: format!("card {id}"),
            body: String::new(),
            app_icon: String::new(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            category: None,
            progress: None,
            sync_tag: None,
            open_path: None,
            repeat_count: 1,
            created_at: Instant::now(),
            timeout: Duration::from_secs(5),
            anim_state: AnimState::Active,
            card_y: 0.0,
            card_height: 0.0,
            card_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            action_rects: Vec::new(),
            close_rect: None,
            is_hovered: false,
            hovered_close: false,
            hovered_action: None,
            hovered_at: None,
            dismissing: false,
            progress_color_override: None,
        }
    }

    #[test]
    fn only_the_newest_cards_are_laid_out_and_overflow_counted() {
        let renderer = Renderer::with_layout(340.0, 2);
        let mut notifications = vec![card(1), card(2), card(3)];
        let metrics = renderer.compute_layout(&mut notifications);
        assert_eq!(notifications[0].card_bounds.w, 0.0);
        assert_eq!(metrics.hidden_count, 1);
        assert!(notifications[1].card_bounds.h > 0.0);
        assert!(notifications[2].card_bounds.h > 0.0);
        assert!(metrics.total_height > 0.0);
    }

    #[test]
    fn progress_bar_and_action_buttons_never_overlap_or_collide_with_close() {
        let renderer = Renderer::with_layout(340.0, 4);
        let mut n = card(1);
        n.summary = "Downloading very long package name that must truncate cleanly".to_string();
        n.body = "File /home/brunux/Downloads/hayato-google-asurada-v0.6.1.img.xz".to_string();
        n.progress = Some(65);
        n.repeat_count = 3;
        n.actions = vec![
            ActionItem {
                key: "cancel".to_string(),
                label: "Cancel Download".to_string(),
            },
            ActionItem {
                key: "open".to_string(),
                label: "Open Folder".to_string(),
            },
        ];

        let plan = renderer.plan_card_layout(&n, 340.0);
        let py = plan.progress_y.expect("progress y");
        let ay = plan.actions_y.expect("actions y");
        assert!(
            ay >= py + 8.0,
            "actions ({ay}) must sit cleanly below progress ({py})"
        );

        let mut list = vec![n];
        let metrics = renderer.compute_layout(&mut list);
        let mut data =
            vec![
                0u8;
                (metrics.total_width.ceil() as usize) * (metrics.total_height.ceil() as usize) * 4
            ];
        let mut pixmap = PixmapMut::from_bytes(
            &mut data,
            metrics.total_width.ceil() as u32,
            metrics.total_height.ceil() as u32,
        )
        .expect("pixmap");
        renderer.render(&mut pixmap, &mut list);

        let close = list[0].close_rect.expect("close rect");
        assert_eq!(list[0].action_rects.len(), 2);
        for (_, rect) in &list[0].action_rects {
            assert!(
                rect.x >= list[0].card_bounds.x,
                "action button must stay inside left card edge: {rect:?}"
            );
            assert!(
                close.y + close.h <= rect.y + 0.5,
                "close_rect ({close:?}) must never overlap action button ({rect:?})"
            );
        }
    }

    #[test]
    fn light_theme_body_text_is_readable_on_surface() {
        let latte = hayato_theme::Theme::parse(hayato_theme::builtin::CATPPUCCIN_LATTE)
            .expect("latte parses");
        let theme = CardTheme::from_theme(&latte);
        // On Catppuccin Latte, bg is light (~#e6e9ef) and text_body must be dark (~#4c4f69),
        // never matching the light surface!
        let bg_luma = 0.299 * theme.bg.red() + 0.587 * theme.bg.green() + 0.114 * theme.bg.blue();
        let fg_luma = 0.299 * theme.text_body.red()
            + 0.587 * theme.text_body.green()
            + 0.114 * theme.text_body.blue();
        assert!(
            (bg_luma - fg_luma).abs() > 0.35,
            "light theme body text must contrast strongly with surface (bg={bg_luma}, fg={fg_luma})"
        );
    }

    #[test]
    fn test_urgency_helpers() {
        assert_eq!(Urgency::from_u8(0), Urgency::Low);
        assert_eq!(Urgency::from_u8(1), Urgency::Normal);
        assert_eq!(Urgency::from_u8(2), Urgency::Critical);
        assert_eq!(Urgency::from_u8(99), Urgency::Normal);
        assert_eq!(Urgency::Low.to_str(), "low");
        assert_eq!(Urgency::Normal.to_str(), "normal");
        assert_eq!(Urgency::Critical.to_str(), "critical");
    }

    #[test]
    fn test_compute_layout_and_render_osd() {
        let renderer = Renderer::with_layout(340.0, 4);
        let mut notifications = vec![Notification {
            id: 2,
            app_name: "Hayato".to_string(),
            summary: "Volume".to_string(),
            body: "50%".to_string(),
            app_icon: "audio-volume-high".to_string(),
            actions: Vec::new(),
            urgency: Urgency::Low,
            category: None,
            progress: Some(50),
            sync_tag: Some("hayato-osd".to_string()),
            open_path: None,
            repeat_count: 1,
            created_at: Instant::now(),
            timeout: Duration::from_millis(1200),
            anim_state: AnimState::Active,
            card_y: 0.0,
            card_height: 0.0,
            card_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            action_rects: Vec::new(),
            close_rect: None,
            is_hovered: false,
            hovered_close: false,
            hovered_action: None,
            hovered_at: None,
            dismissing: false,
            progress_color_override: None,
        }];

        let metrics = renderer.compute_layout(&mut notifications);
        assert_eq!(notifications[0].card_height, 44.0);
        assert!(notifications[0].is_osd());

        let mut data =
            vec![
                0u8;
                (metrics.total_width.ceil() as usize) * (metrics.total_height.ceil() as usize) * 4
            ];
        let mut pixmap = PixmapMut::from_bytes(
            &mut data,
            metrics.total_width.ceil() as u32,
            metrics.total_height.ceil() as u32,
        )
        .expect("pixmap allocation");

        renderer.render(&mut pixmap, &mut notifications);
    }

    #[test]
    fn the_fallback_card_is_tokyo_night() {
        let tokyo = hayato_theme::Theme::parse(hayato_theme::builtin::TOKYO_NIGHT)
            .expect("the built-in Tokyo Night theme parses");
        let expected = CardTheme::from_theme(&tokyo);
        let fallback = CardTheme::fallback();
        assert_eq!(fallback.bg, expected.bg, "card background");
        assert_eq!(fallback.accent, expected.accent, "card accent");
        assert_eq!(fallback.text_body, expected.text_body, "card body text");
    }

    #[test]
    fn header_tags_follow_cyber_hud_grammar() {
        assert_eq!(
            format_header_tag("Hayato", Urgency::Normal, false, false),
            "[ SYS // INFO ]"
        );
        assert_eq!(
            format_header_tag("Hayato", Urgency::Critical, false, false),
            "[ ALERT // CRIT ]"
        );
        assert_eq!(
            format_header_tag("Hayato", Urgency::Normal, true, false),
            "[ SYS // SYNC ]"
        );
        assert_eq!(
            format_header_tag("Hayato", Urgency::Normal, false, true),
            "[ SYS // FILE ]"
        );
        assert_eq!(
            format_header_tag("Chromium", Urgency::Normal, false, false),
            "[ CHROMIUM // INFO ]"
        );
        assert_eq!(
            format_header_tag("", Urgency::Low, false, false),
            "[ SYS // LOW ]"
        );
    }

    fn as_rgb(c: Color) -> hayato_theme::Rgb {
        hayato_theme::Rgb::new(
            (c.red() * 255.0).round() as u8,
            (c.green() * 255.0).round() as u8,
            (c.blue() * 255.0).round() as u8,
        )
    }

    #[test]
    fn every_builtin_theme_produces_readable_card_theme() {
        use hayato_theme::contrast::ratio;

        for (slug, text) in hayato_theme::builtin::ALL {
            let parsed = hayato_theme::Theme::parse(text).expect("builtin theme parses");
            let t = CardTheme::from_theme(&parsed);
            let bg = as_rgb(t.bg);
            let elev = as_rgb(t.bg_elevated);
            let hdr = as_rgb(t.header_bg);
            let danger_bg = as_rgb(t.danger_bg);
            let danger_hdr = hayato_theme::Hex(bg)
                .mix(hayato_theme::Hex(danger_bg), 0.78)
                .0;

            for (name, fg, against, floor) in [
                (
                    "btn_fg_hover on btn_bg_hover",
                    t.btn_fg_hover,
                    as_rgb(t.btn_bg_hover),
                    4.5,
                ),
                ("btn_fg on btn_bg", t.btn_fg, as_rgb(t.btn_bg), 4.5),
                ("text_body on bg", t.text_body, bg, 4.5),
                ("text_body on bg_elevated", t.text_body, elev, 4.5),
                ("text_body on header_bg", t.text_body, hdr, 4.5),
                ("text_dim on header_bg", t.text_dim, hdr, 4.5),
                ("text_dim on bg", t.text_dim, bg, 4.5),
                ("accent_text on header_bg", t.accent_text, hdr, 4.5),
                ("accent_text on bg", t.accent_text, bg, 4.5),
                ("accent_text on bg_elevated", t.accent_text, elev, 4.5),
                ("warn_text on header_bg", t.warn_text, hdr, 4.5),
                ("danger_text on danger_bg", t.danger_text, danger_bg, 4.5),
                ("danger_text on danger_hdr", t.danger_text, danger_hdr, 4.5),
                ("border_subtle on bg", t.border_subtle, bg, 3.0),
                ("border_subtle on bg_elevated", t.border_subtle, elev, 3.0),
                ("accent on bg", t.accent, bg, 3.0),
                ("danger on bg", t.danger, bg, 3.0),
            ] {
                let r = ratio(as_rgb(fg), against);
                assert!(r >= floor, "{slug}: {name} contrast {r:.2} < {floor:.1}");
            }
        }
    }
}
