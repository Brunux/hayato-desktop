#![allow(unknown_lints)]
#![allow(clippy::chunks_exact_to_as_chunks)]

use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};

use crate::font::FontRenderer;
use crate::model::{Notification, Rect, Urgency};

pub const OSD_WIDTH: f32 = 280.0;
pub const OSD_HEIGHT: f32 = 44.0;
pub const CARD_SPACING: f32 = 8.0;
pub const CORNER_RADIUS: f32 = 6.0;
const OVERFLOW_PILL_H: f32 = 22.0;
const ACTION_BTN_H: f32 = 26.0;
const ACTION_BTN_GAP: f32 = 6.0;
const ACTION_ROW_GAP: f32 = 6.0;
const CONTENT_LEFT_PAD: f32 = 38.0;
const CONTENT_RIGHT_PAD: f32 = 14.0;
const CLOSE_HIT_SIZE: f32 = 28.0;

fn with_alpha(c: Color, alpha: f32) -> Color {
    let a = (c.alpha() * alpha).clamp(0.0, 1.0);
    Color::from_rgba(c.red(), c.green(), c.blue(), a).unwrap_or(c)
}

pub struct CardTheme {
    pub bg: Color,
    pub bg_elevated: Color,
    pub border_subtle: Color,
    pub text_bright: Color,
    pub text_body: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub danger: Color,
    pub danger_text: Color,
    pub danger_bg: Color,
    #[allow(dead_code)]
    pub warn: Color,
    #[allow(dead_code)]
    pub ok: Color,
    pub track_bg: Color,
    pub btn_bg: Color,
    pub btn_bg_hover: Color,
}

impl Default for CardTheme {
    fn default() -> Self {
        Self {
            bg: Color::from_rgba8(36, 40, 59, 244), // surface: #24283b
            bg_elevated: Color::from_rgba8(41, 46, 66, 250),
            border_subtle: Color::from_rgba8(59, 66, 97, 230), // border: #3b4261
            text_bright: Color::from_rgba8(125, 207, 255, 255), // cyan: #7dcfff
            text_body: Color::from_rgba8(192, 202, 245, 245),  // text: #c0caf5
            text_dim: Color::from_rgba8(86, 95, 137, 255),
            accent: Color::from_rgba8(125, 207, 255, 255),
            danger: Color::from_rgba8(247, 118, 142, 255),
            danger_text: Color::from_rgba8(247, 118, 142, 255),
            danger_bg: Color::from_rgba8(72, 34, 48, 220),
            warn: Color::from_rgba8(224, 175, 104, 255),
            ok: Color::from_rgba8(158, 206, 106, 255),
            track_bg: Color::from_rgba8(41, 46, 66, 175),
            btn_bg: Color::from_rgba8(41, 46, 66, 215),
            btn_bg_hover: Color::from_rgba8(59, 66, 97, 245),
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
        let r = theme.roles();
        let to_color =
            |hex: hayato_theme::Hex, a: u8| Color::from_rgba8(hex.0.r, hex.0.g, hex.0.b, a);

        Self {
            bg: to_color(r.surface, 244),
            bg_elevated: to_color(r.hover, 250),
            border_subtle: to_color(r.border, 230),
            text_bright: to_color(r.accent, 255),
            // Use `r.text` (not `r.strong`) so body copy remains high-contrast
            // on light themes like Catppuccin Latte and Flexoki Light.
            text_body: to_color(r.text, 245),
            text_dim: to_color(r.dim, 255),
            accent: to_color(r.accent, 255),
            danger: to_color(r.danger, 255),
            danger_text: to_color(r.danger_text, 255),
            danger_bg: to_color(r.danger_bg, 225),
            warn: to_color(r.warn, 255),
            ok: to_color(r.ok, 255),
            track_bg: to_color(r.hover, 175),
            btn_bg: to_color(r.hover, 215),
            btn_bg_hover: to_color(r.border, 245),
        }
    }
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
    is_compact_single: bool,
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

    #[allow(dead_code)]
    pub fn add_circle(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32) {
        Self::add_rounded_rect(pb, cx - r, cy - r, r * 2.0, r * 2.0, r);
    }

    fn plan_card_layout(&self, notif: &Notification, card_w: f32) -> CardLayout {
        let text_max_width = (card_w - CONTENT_LEFT_PAD - CONTENT_RIGHT_PAD).max(80.0);
        let body_lines = if notif.body.is_empty() {
            Vec::new()
        } else {
            self.font
                .wrap_text(&notif.body, 12.0, text_max_width, 4, false)
        };

        // Plan action button rows so buttons never overflow off the left edge.
        let mut action_rows: Vec<Vec<ActionChip>> = Vec::new();
        if !notif.actions.is_empty() {
            let mut cur_row: Vec<ActionChip> = Vec::new();
            let mut cur_row_w = 0.0f32;
            for action in &notif.actions {
                let max_label_w = (text_max_width - 18.0).max(32.0);
                let label = self
                    .font
                    .truncate_text(&action.label, 10.5, max_label_w, true);
                let btn_w =
                    (self.font.measure_width(&label, 10.5, true) + 18.0).min(text_max_width);
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

        let is_compact_single =
            body_lines.is_empty() && notif.progress.is_none() && action_rows.is_empty();

        if is_compact_single {
            return CardLayout {
                is_compact_single: true,
                body_lines,
                progress_y: None,
                actions_y: None,
                action_rows,
                height: OSD_HEIGHT,
            };
        }

        // Top-aligned multi-part card:
        // Summary baseline at y + 20.0
        let mut cursor_y = 22.0f32;
        if !body_lines.is_empty() {
            cursor_y += 4.0 + (body_lines.len() as f32 * 16.0);
        }

        let progress_y = if notif.progress.is_some() {
            let py = cursor_y + 8.0;
            cursor_y = py + 4.0 + 4.0; // bar height (4) + breathing room (4)
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

        let bottom_pad = 12.0f32;
        let height = (cursor_y + bottom_pad).max(OSD_HEIGHT);

        CardLayout {
            is_compact_single: false,
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
        let label = format!("+{hidden} more");
        let text_w = self.font.measure_width(&label, 10.5, true);
        let pill_w = text_w + 18.0;
        let pill_h = OVERFLOW_PILL_H;
        let pill_x = (max_w - pill_w).max(0.0);

        let mut pb = PathBuilder::new();
        Self::add_rounded_rect(&mut pb, pill_x, y, pill_w, pill_h, pill_h * 0.5);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(self.theme.bg);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut bpaint = Paint::default();
            bpaint.set_color(self.theme.border_subtle);
            bpaint.anti_alias = true;
            pixmap.stroke_path(&path, &bpaint, &stroke, Transform::identity(), None);
        }

        self.font.draw_text(
            pixmap,
            &label,
            pill_x + 9.0,
            y + 14.5,
            10.5,
            self.theme.text_dim,
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

        // 3. Status checks
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

        let icon_color = if is_muted {
            self.theme.danger
        } else {
            self.theme.accent
        };

        self.font.draw_text(
            pixmap,
            icon_glyph,
            map_x(card_x + 12.0),
            map_y(card_y + 26.0),
            16.0 * scale,
            with_alpha(icon_color, alpha),
            false,
        );

        let content_x = card_x + 36.0;
        let right_margin = card_x + card_w - 14.0;

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
            "MUTED".to_string()
        } else if notif.progress.is_some() || notif.body.trim().ends_with('%') {
            format!("{effective_pct}%")
        } else if !notif.body.is_empty() {
            notif.body.clone()
        } else {
            "0%".to_string()
        };

        let val_w = self.font.measure_width(&val_str, 12.0 * scale, false);
        let val_color = if is_muted {
            self.theme.danger_text
        } else {
            self.theme.accent
        };

        let summary_max_w = (right_margin - content_x - val_w - 8.0).max(40.0);
        let summary = self
            .font
            .truncate_text(&notif.summary, 12.0, summary_max_w, false);

        self.font.draw_text(
            pixmap,
            &summary,
            map_x(content_x),
            map_y(card_y + 17.0),
            12.0 * scale,
            with_alpha(self.theme.text_bright, alpha),
            false,
        );

        self.font.draw_text(
            pixmap,
            &val_str,
            map_x(right_margin) - val_w,
            map_y(card_y + 17.0),
            12.0 * scale,
            with_alpha(val_color, alpha),
            false,
        );

        // 4. Level track (0% draws empty — no phantom 3px nub)
        let track_x = content_x;
        let track_y = card_y + 27.0;
        let track_w = right_margin - content_x;
        let track_h = 4.0f32;
        let track_r = 2.0f32;

        let mut track_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut track_builder,
            track_x,
            track_y,
            track_w,
            track_h,
            track_r,
        );
        if let Some(track_path) = track_builder.finish() {
            let mut paint = Paint::default();
            paint.set_color(with_alpha(self.theme.track_bg, alpha));
            paint.anti_alias = true;
            pixmap.fill_path(&track_path, &paint, FillRule::Winding, transform, None);
        }

        let fill_w = if is_muted || effective_pct == 0 {
            0.0
        } else {
            let fill_pct = (effective_pct as f32 / 100.0).clamp(0.0, 1.0);
            (track_w * fill_pct).max(3.0)
        };

        if fill_w > 0.0 {
            let mut fill_builder = PathBuilder::new();
            Self::add_rounded_rect(
                &mut fill_builder,
                track_x,
                track_y,
                fill_w,
                track_h,
                track_r,
            );
            if let Some(fill_path) = fill_builder.finish() {
                let mut paint = Paint::default();
                paint.set_color(with_alpha(icon_color, alpha));
                paint.anti_alias = true;
                pixmap.fill_path(&fill_path, &paint, FillRule::Winding, transform, None);
            }
        }
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

        // 2. Crisp 3px left urgency accent strip
        let strip_color = match notif.urgency {
            Urgency::Low => with_alpha(self.theme.border_subtle, 0.7),
            Urgency::Normal => self.theme.accent,
            Urgency::Critical => self.theme.danger,
        };
        let mut strip_builder = PathBuilder::new();
        Self::add_rounded_rect(
            &mut strip_builder,
            card_x + 3.0,
            card_y + 6.0,
            3.0,
            (card_h - 12.0).max(8.0),
            1.5,
        );
        if let Some(strip_path) = strip_builder.finish() {
            let mut paint = Paint::default();
            paint.set_color(with_alpha(strip_color, alpha));
            paint.anti_alias = true;
            pixmap.fill_path(&strip_path, &paint, FillRule::Winding, transform, None);
        }

        // 3. Border (Critical gets danger border; hovered file cards highlight accent)
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

        // 4. Icon glyph
        let glyph = Self::resolve_glyph(
            &notif.app_icon,
            &notif.app_name,
            &notif.summary,
            notif.urgency,
        );
        let glyph_color = match notif.urgency {
            Urgency::Low => self.theme.text_dim,
            Urgency::Normal => self.theme.accent,
            Urgency::Critical => self.theme.danger,
        };

        let icon_y = if plan.is_compact_single {
            card_y + 26.0
        } else {
            card_y + 23.0
        };

        self.font.draw_text(
            pixmap,
            glyph,
            map_x(card_x + 14.0),
            map_y(icon_y),
            16.0 * scale,
            with_alpha(glyph_color, alpha),
            false,
        );

        // 5. Close '×' button (sized cleanly to top-right, clamped above action buttons!)
        let close_x = card_x + card_w - 22.0;
        let close_y = if plan.is_compact_single {
            card_y + 26.0
        } else {
            card_y + 20.0
        };
        let hit_w = CLOSE_HIT_SIZE * scale;
        let mut hit_top = map_y(card_y + 2.0);
        let mut hit_h = (CLOSE_HIT_SIZE * scale).min(card_h * scale - 4.0);
        if let Some(ay) = plan.actions_y {
            let max_bottom = map_y(card_y + ay - 2.0);
            if hit_top + hit_h > max_bottom {
                hit_h = (max_bottom - hit_top).max(18.0 * scale);
            }
        } else if plan.is_compact_single {
            hit_top = map_y(card_y + (card_h - CLOSE_HIT_SIZE) * 0.5);
        }
        let close_rect = Rect::new(map_x(card_x + card_w - 32.0), hit_top, hit_w, hit_h);
        notif.close_rect = Some(close_rect);

        if notif.hovered_close {
            let pill_x = card_x + card_w - 28.0;
            let pill_y = close_y - 14.0;
            let mut pb = PathBuilder::new();
            Self::add_rounded_rect(&mut pb, pill_x, pill_y, 20.0, 20.0, 4.0);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(with_alpha(self.theme.danger_bg, alpha));
                paint.anti_alias = true;
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        }

        let close_color = if notif.hovered_close {
            self.theme.danger_text
        } else if notif.is_hovered {
            self.theme.text_body
        } else {
            self.theme.text_dim
        };
        self.font.draw_text(
            pixmap,
            "\u{f00d}", // Font Awesome xmark
            map_x(close_x),
            map_y(close_y),
            12.0 * scale,
            with_alpha(close_color, alpha),
            false,
        );

        // 6. Repeat badge pill (×N) placed cleanly left of the close button
        let content_x = card_x + CONTENT_LEFT_PAD;
        let mut summary_right = card_x + card_w - 34.0;
        let summary_baseline = if plan.is_compact_single {
            card_y + 26.0
        } else {
            card_y + 20.0
        };

        if notif.repeat_count > 1 {
            let label = format!("\u{00d7}{}", notif.repeat_count);
            let label_w = self.font.measure_width(&label, 10.0, true);
            let pill_w = label_w + 10.0;
            let pill_h = 16.0f32;
            let pill_x = summary_right - pill_w - 4.0;
            let pill_y = summary_baseline - 12.0;

            let mut pb = PathBuilder::new();
            Self::add_rounded_rect(&mut pb, pill_x, pill_y, pill_w, pill_h, 4.0);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(with_alpha(self.theme.btn_bg, alpha));
                paint.anti_alias = true;
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
            self.font.draw_text(
                pixmap,
                &label,
                map_x(pill_x + 5.0),
                map_y(summary_baseline - 0.5),
                10.0 * scale,
                with_alpha(self.theme.text_body, alpha),
                true,
            );
            summary_right = pill_x - 6.0;
        }

        // 7. Summary (truncated cleanly so it never collides with ×N or ×)
        let summary_max_w = (summary_right - content_x).max(40.0);
        let summary = self
            .font
            .truncate_text(&notif.summary, 12.0, summary_max_w, true);
        let summary_color = match notif.urgency {
            Urgency::Critical => self.theme.danger_text,
            _ => self.theme.text_bright,
        };
        self.font.draw_text(
            pixmap,
            &summary,
            map_x(content_x),
            map_y(summary_baseline),
            12.0 * scale,
            with_alpha(summary_color, alpha),
            true,
        );

        // 8. Body lines
        let mut body_y = summary_baseline + 4.0;
        for line in &plan.body_lines {
            body_y += 16.0;
            self.font.draw_text(
                pixmap,
                line,
                map_x(content_x),
                map_y(body_y),
                12.0 * scale,
                with_alpha(self.theme.text_body, alpha),
                false,
            );
        }

        // 9. Progress Bar (using planned non-overlapping Y)
        let text_max_width = (card_w - CONTENT_LEFT_PAD - CONTENT_RIGHT_PAD).max(80.0);
        if let (Some(pct), Some(py_off)) = (notif.progress, plan.progress_y) {
            let bar_y = card_y + py_off;
            let bar_x = content_x;
            let bar_w = text_max_width;
            let bar_h = 4.0f32;

            let mut track_builder = PathBuilder::new();
            Self::add_rounded_rect(&mut track_builder, bar_x, bar_y, bar_w, bar_h, 2.0);
            if let Some(track_path) = track_builder.finish() {
                let mut paint = Paint::default();
                paint.set_color(with_alpha(self.theme.track_bg, alpha));
                paint.anti_alias = true;
                pixmap.fill_path(&track_path, &paint, FillRule::Winding, transform, None);
            }

            if pct > 0 {
                let fill_pct = (pct as f32 / 100.0).clamp(0.0, 1.0);
                let fill_w = (bar_w * fill_pct).max(3.0);
                let fill_color = if let Some([r, g, b, a]) = notif.progress_color_override {
                    Color::from_rgba8(r, g, b, a)
                } else {
                    glyph_color
                };

                let mut fill_builder = PathBuilder::new();
                Self::add_rounded_rect(&mut fill_builder, bar_x, bar_y, fill_w, bar_h, 2.0);
                if let Some(fill_path) = fill_builder.finish() {
                    let mut paint = Paint::default();
                    paint.set_color(with_alpha(fill_color, alpha));
                    paint.anti_alias = true;
                    pixmap.fill_path(&fill_path, &paint, FillRule::Winding, transform, None);
                }
            }
        }

        // 10. Action buttons (wrapped & clamped inside card_bounds, never overlapping progress or close)
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
                        4.0,
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
                        pixmap.stroke_path(&btn_path, &bpaint, &stroke, transform, None);
                    }

                    let label_color = if is_btn_hovered {
                        self.theme.text_bright
                    } else {
                        self.theme.text_body
                    };

                    self.font.draw_text(
                        pixmap,
                        &chip.label,
                        map_x(btn_x + 9.0),
                        map_y(row_y + 17.0),
                        10.5 * scale,
                        with_alpha(label_color, alpha),
                        true,
                    );

                    btn_x -= ACTION_BTN_GAP;
                }
                row_y += ACTION_BTN_H + ACTION_ROW_GAP;
            }
        }

        // 11. Subtle 2px bottom timeout countdown hairline (freezes visibly on hover)
        if let Some(rem) = notif.remaining_fraction() {
            if rem > 0.01 {
                let inner_w = (card_w - 12.0).max(0.0);
                let line_w = inner_w * rem;
                let line_x = card_x + 6.0;
                let line_y = card_y + card_h - 3.0;
                let mut pb = PathBuilder::new();
                Self::add_rounded_rect(&mut pb, line_x, line_y, line_w, 2.0, 1.0);
                if let Some(path) = pb.finish() {
                    let mut paint = Paint::default();
                    let line_alpha = if notif.is_hovered { 0.65 } else { 0.35 };
                    paint.set_color(with_alpha(strip_color, alpha * line_alpha));
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
}
