use fontdue::{Font, FontSettings};
use tiny_skia::{Color, PixmapMut};

pub struct FontRenderer {
    regular: Option<Font>,
    bold: Option<Font>,
    symbols: Option<Font>,
    symbols_fa_solid: Option<Font>,
    symbols_fa_brands: Option<Font>,
}

impl FontRenderer {
    pub fn new() -> Self {
        let regular = Self::load_font(&[
            "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-SemiBold.ttf",
            "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/adwaita/AdwaitaSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]);

        let bold = Self::load_font(&[
            "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Bold.ttf",
            "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-SemiBold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/adwaita/AdwaitaSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        ]);

        let symbols = Self::load_font(&[
            "/usr/share/fonts/truetype/nerd-fonts-symbols/SymbolsNerdFontMono-Regular.ttf",
            "/usr/share/fonts/truetype/nerd-fonts-symbols/SymbolsNerdFont-Regular.ttf",
        ]);

        let symbols_fa_solid = Self::load_font(&[
            "/usr/share/fonts/truetype/font-awesome/fa-6.7.2-solid-900.ttf",
            "/usr/share/fonts/truetype/font-awesome/fa-solid-900.ttf",
        ]);

        let symbols_fa_brands = Self::load_font(&[
            "/usr/share/fonts/truetype/font-awesome/fa-6.7.2-brands-400.ttf",
            "/usr/share/fonts/truetype/font-awesome/fa-brands-400.ttf",
        ]);

        Self {
            regular,
            bold,
            symbols,
            symbols_fa_solid,
            symbols_fa_brands,
        }
    }

    fn load_font(paths: &[&str]) -> Option<Font> {
        for path in paths {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                    #[cfg(test)]
                    println!("Successfully loaded font from: {path}");
                    return Some(font);
                }
            }
        }
        None
    }

    pub fn pick_font(&self, ch: char, bold: bool) -> Option<&Font> {
        // For private-use codepoints (Nerd Font Octicons / Font Awesome 6),
        // check symbol faces in the same precedence order as foot.ini.
        if ch >= '\u{e000}' {
            for face in [
                &self.symbols,
                &self.symbols_fa_solid,
                &self.symbols_fa_brands,
            ] {
                if let Some(f) = face.as_ref() {
                    if f.lookup_glyph_index(ch) != 0 {
                        return Some(f);
                    }
                }
            }
        }

        // For console text characters, use JetBrains Mono (bold or regular)
        if bold {
            if let Some(ref f) = self.bold {
                if f.lookup_glyph_index(ch) != 0 {
                    return Some(f);
                }
            }
        }
        if let Some(ref f) = self.regular {
            if f.lookup_glyph_index(ch) != 0 {
                return Some(f);
            }
        }

        for face in [
            &self.symbols,
            &self.symbols_fa_solid,
            &self.symbols_fa_brands,
        ] {
            if let Some(f) = face.as_ref() {
                if f.lookup_glyph_index(ch) != 0 {
                    return Some(f);
                }
            }
        }

        self.regular
            .as_ref()
            .or(self.bold.as_ref())
            .or(self.symbols.as_ref())
            .or(self.symbols_fa_solid.as_ref())
            .or(self.symbols_fa_brands.as_ref())
    }

    pub fn measure_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        let mut width = 0.0f32;
        for ch in text.chars() {
            if let Some(font) = self.pick_font(ch, bold) {
                let metrics = font.metrics(ch, size);
                width += metrics.advance_width;
            } else {
                width += size * 0.5;
            }
        }
        width
    }

    pub fn truncate_text(&self, text: &str, size: f32, max_width: f32, bold: bool) -> String {
        if max_width <= 0.0 || text.is_empty() {
            return String::new();
        }
        if self.measure_width(text, size, bold) <= max_width {
            return text.to_string();
        }
        let ellipsis = "…";
        let el_width = self.measure_width(ellipsis, size, bold);
        if el_width >= max_width {
            return ellipsis.to_string();
        }
        let mut out = String::new();
        let mut cur_w = 0.0f32;
        for ch in text.chars() {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            let w = self.measure_width(s, size, bold);
            if cur_w + w + el_width > max_width {
                break;
            }
            out.push(ch);
            cur_w += w;
        }
        out.trim_end().to_string() + ellipsis
    }

    pub fn wrap_text(
        &self,
        text: &str,
        size: f32,
        max_width: f32,
        max_lines: usize,
        bold: bool,
    ) -> Vec<String> {
        if max_lines == 0 || max_width <= 0.0 {
            return Vec::new();
        }
        let mut lines = Vec::new();
        let mut truncated = false;
        let space_width = self.measure_width(" ", size, bold);

        'outer: for paragraph in text.lines() {
            let mut cur_line = String::new();
            let mut cur_width = 0.0f32;

            for word in paragraph.split_whitespace() {
                if lines.len() >= max_lines {
                    truncated = true;
                    break 'outer;
                }
                let word_width = self.measure_width(word, size, bold);

                if cur_line.is_empty() {
                    if word_width <= max_width {
                        cur_line.push_str(word);
                        cur_width = word_width;
                    } else {
                        // Break long unbroken tokens (file paths, URLs) cleanly across lines.
                        for ch in word.chars() {
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            let cw = self.measure_width(s, size, bold);
                            if !cur_line.is_empty() && cur_width + cw > max_width {
                                lines.push(std::mem::take(&mut cur_line));
                                cur_width = 0.0;
                                if lines.len() >= max_lines {
                                    truncated = true;
                                    break 'outer;
                                }
                            }
                            cur_line.push(ch);
                            cur_width += cw;
                        }
                    }
                } else if cur_width + space_width + word_width <= max_width {
                    cur_line.push(' ');
                    cur_line.push_str(word);
                    cur_width += space_width + word_width;
                } else {
                    lines.push(std::mem::take(&mut cur_line));
                    cur_width = 0.0;
                    if lines.len() >= max_lines {
                        truncated = true;
                        break 'outer;
                    }
                    if word_width <= max_width {
                        cur_line.push_str(word);
                        cur_width = word_width;
                    } else {
                        for ch in word.chars() {
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            let cw = self.measure_width(s, size, bold);
                            if !cur_line.is_empty() && cur_width + cw > max_width {
                                lines.push(std::mem::take(&mut cur_line));
                                cur_width = 0.0;
                                if lines.len() >= max_lines {
                                    truncated = true;
                                    break 'outer;
                                }
                            }
                            cur_line.push(ch);
                            cur_width += cw;
                        }
                    }
                }
            }

            if !cur_line.is_empty() {
                if lines.len() < max_lines {
                    lines.push(cur_line);
                } else {
                    truncated = true;
                    break 'outer;
                }
            }
        }

        if truncated {
            if let Some(last) = lines.last_mut() {
                let ellipsis = "…";
                let el_width = self.measure_width(ellipsis, size, bold);
                while !last.is_empty()
                    && self.measure_width(last, size, bold) + el_width > max_width
                {
                    last.pop();
                }
                let trimmed = last.trim_end().to_string();
                *last = trimmed + ellipsis;
            }
        }

        lines
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        pixmap: &mut PixmapMut,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        bold: bool,
    ) -> f32 {
        let mut cur_x = x;
        let p_width = pixmap.width() as i32;
        let p_height = pixmap.height() as i32;

        for ch in text.chars() {
            let Some(font) = self.pick_font(ch, bold) else {
                cur_x += size * 0.5;
                continue;
            };

            let (metrics, bitmap) = font.rasterize(ch, size);
            let gx = (cur_x + metrics.xmin as f32).round() as i32;
            let gy = (y - metrics.height as f32 - metrics.ymin as f32).round() as i32;

            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || px >= p_width || py < 0 || py >= p_height {
                        continue;
                    }

                    let alpha_coverage = bitmap[row * metrics.width + col] as f32 / 255.0;
                    if alpha_coverage <= 0.001 {
                        continue;
                    }

                    let pixel_index = (py as usize * p_width as usize + px as usize) * 4;
                    let slice = &mut pixmap.data_mut()[pixel_index..pixel_index + 4];

                    // Premultiplied alpha blending:
                    let src_a = color.alpha() * alpha_coverage;
                    let src_r = color.red() * src_a;
                    let src_g = color.green() * src_a;
                    let src_b = color.blue() * src_a;

                    let dst_r = slice[0] as f32 / 255.0;
                    let dst_g = slice[1] as f32 / 255.0;
                    let dst_b = slice[2] as f32 / 255.0;
                    let dst_a = slice[3] as f32 / 255.0;

                    let out_a = src_a + dst_a * (1.0 - src_a);
                    if out_a > 0.0 {
                        let out_r = (src_r + dst_r * (1.0 - src_a)).min(1.0);
                        let out_g = (src_g + dst_g * (1.0 - src_a)).min(1.0);
                        let out_b = (src_b + dst_b * (1.0 - src_a)).min(1.0);

                        slice[0] = (out_r * 255.0).round() as u8;
                        slice[1] = (out_g * 255.0).round() as u8;
                        slice[2] = (out_b * 255.0).round() as u8;
                        slice[3] = (out_a * 255.0).round() as u8;
                    }
                }
            }

            cur_x += metrics.advance_width;
        }

        cur_x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_picking() {
        let renderer = FontRenderer::new();

        // If system fonts are present in the environment (e.g. on the Hayato target device),
        // verify regular, bold, and symbol font picking. In minimal CI runner environments
        // lacking these specific font packages, verify that picking and measuring fall back
        // gracefully without panicking.
        if let Some(ref reg) = renderer.regular {
            let font_a = renderer.pick_font('A', false);
            assert!(font_a.is_some());
            assert_ne!(reg.lookup_glyph_index('A'), 0);
        }

        if renderer.symbols.is_some() {
            let icon_font = renderer.pick_font('\u{f485}', false);
            assert!(icon_font.is_some());
        }
        if let Some(ref fa) = renderer.symbols_fa_solid {
            assert_ne!(fa.lookup_glyph_index('\u{f00d}'), 0);
            assert!(renderer.pick_font('\u{f00d}', false).is_some());
        }
        if let Some(ref fa_b) = renderer.symbols_fa_brands {
            assert_ne!(fa_b.lookup_glyph_index('\u{f293}'), 0);
            assert!(renderer.pick_font('\u{f293}', false).is_some());
        }

        // Measure width and font picking must always succeed without panicking
        let width = renderer.measure_width("Hayato Notification", 12.0, false);
        assert!(width > 0.0);
    }

    #[test]
    fn wrap_breaks_long_paths_and_avoids_false_ellipsis() {
        let renderer = FontRenderer::new();
        let exact_four = "line one\nline two\nline three\nline four";
        let wrapped = renderer.wrap_text(exact_four, 12.0, 260.0, 4, false);
        assert_eq!(wrapped.len(), 4);
        assert!(
            !wrapped[3].ends_with('…'),
            "exact 4-line text must not get a false ellipsis: {:?}",
            wrapped
        );

        let five_lines = "line one\nline two\nline three\nline four\nline five";
        let truncated = renderer.wrap_text(five_lines, 12.0, 260.0, 4, false);
        assert_eq!(truncated.len(), 4);
        assert!(truncated[3].ends_with('…'));

        let long_path =
            "/home/brunux/Pictures/Screenshots/hayato-very-long-unbroken-filename-2026-11-21.png";
        let path_lines = renderer.wrap_text(long_path, 12.0, 140.0, 4, false);
        assert!(path_lines.len() >= 2);
        for line in &path_lines {
            assert!(
                renderer.measure_width(line, 12.0, false) <= 145.0,
                "wrapped line overflowed max width: {line}"
            );
        }

        let short_sum = renderer.truncate_text(
            "A very long notification summary that would hit close",
            12.0,
            100.0,
            false,
        );
        assert!(short_sum.ends_with('…'));
        assert!(renderer.measure_width(&short_sum, 12.0, false) <= 102.0);
    }
}
