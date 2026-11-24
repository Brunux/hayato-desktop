//! The design system's vocabulary: roles, spaces, text styles and density.
//!
//! A widget asks for `ColorRole::Accent` and `TextStyle::Dim`, never for a hex
//! value or a magic padding number. That is what makes the root menu, the
//! installer and the AI surface look like one session instead of thirty
//! decisions, and it is what lets the theme engine (spec 004) change every
//! surface by changing one palette.
//!
//! The rules the tests enforce:
//!
//! - a colour comes from the palette (`Color::Rgb` appears in this file only);
//! - the density is decided once, from the session, never per widget;
//! - a role is added here deliberately - a widget that "just needs a slightly
//!   different grey" is a token, not a local constant.

use hayato_theme::contrast::ratio;
use hayato_theme::{Palette, Rgb};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

/// What a surface is asking for, semantically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRole {
    /// The window behind everything.
    Bg,
    /// A card, a bar, the selected row's fill.
    Surface,
    /// A second surface on top of `Surface`.
    SurfaceAlt,
    /// A card or list boundary: cleared to 3:1 against the background.
    Border,
    /// Regular text.
    Text,
    /// The theme's own quiet grey, for decoration only. Widgets ask for
    /// [`TextStyle::Dim`], which is `Muted`; this is the raw palette value.
    Dim,
    /// Secondary text that carries meaning (a size, a date, a hint): cleared to
    /// 4.5:1 against the background.
    Muted,
    /// The one colour that marks "this is Hayato" and the current focus.
    Accent,
    /// A second accent for information that is not the focus.
    AccentAlt,
    Ok,
    Warn,
    Danger,
    /// The fill of the selected row: the theme's own tint. The 3:1 cue that
    /// marks the row is the accent bar and the bold label, not the fill.
    Selection,
    /// The selected row's label: cleared to 4.5:1 against [`Selection`].
    SelectionText,
    /// The selected row's detail: cleared to 4.5:1 against [`Selection`].
    SelectionDim,
    /// The letters on a key chip: cleared to 4.5:1 against [`Accent`].
    KeyText,
}

impl ColorRole {
    #[allow(non_upper_case_globals)]
    pub const Warning: Self = Self::Warn;
    #[allow(non_upper_case_globals)]
    pub const Success: Self = Self::Ok;
    #[allow(non_upper_case_globals)]
    pub const Faint: Self = Self::SurfaceAlt;
}

/// The spacing scale. Four steps, no arbitrary numbers: on this 1366 px panel
/// one cell is ~8 px, so the scale is in cells and the density multiplies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Zero,
    One,
    Two,
    Three,
}

impl Space {
    pub const fn cells(self) -> u16 {
        match self {
            Space::Zero => 0,
            Space::One => 1,
            Space::Two => 2,
            Space::Three => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    /// The session's rounded corners (`╭╮╰╯`).
    Rounded,
    /// No frame at all, for a body that owns its whole area.
    None,
}

/// The text styles a widget can ask for, by job rather than by attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// A window or card title.
    Title,
    Body,
    /// Secondary text that carries meaning; `Muted`, not the raw `Dim`.
    Dim,
    /// The selected row's label.
    Selected,
    /// The selected row's detail.
    SelectedDim,
    /// A command, a path, a number worth copying.
    Code,
    /// A key in the key bar.
    Key,
    /// The error card's summary.
    Danger,
}

impl TextStyle {
    #[allow(non_upper_case_globals)]
    pub const Caption: Self = Self::Dim;
    #[allow(non_upper_case_globals)]
    pub const Subtitle: Self = Self::Title;
}

/// How much room a row and the frame's bands get.
///
/// The tablet is folded over the keyboard: the touch target has to be a finger,
/// not a mouse pointer, so a row takes two lines and the bands grow. It is read
/// once from the session (`$XDG_RUNTIME_DIR/hayato-tablet`, the directory
/// `tablet-mode` keeps) - never per widget, which is how a window ends up with
/// half its rows touch-sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Normal,
    Tablet,
}

impl Density {
    /// The number of lines one row occupies.
    pub const fn row_stride(self) -> u16 {
        match self {
            Density::Normal => 1,
            Density::Tablet => 2,
        }
    }

    /// Height of the header band, in lines.
    pub const fn header_height(self) -> u16 {
        match self {
            Density::Normal => 2,
            Density::Tablet => 3,
        }
    }
}

/// The theme's colour in the terminal library's type.
pub fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// The palette plus the decisions derived from it.
///
/// The derived colours are computed once, when the palette is read, and then
/// copied into every cell: a frame never runs a contrast calculation. They are
/// the readable variants of the theme's own roles, so a theme file keeps saying
/// `dim` and `surface_alt` while the UI draws what a person can read on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Design {
    palette: Palette,
    density: Density,
    reduced_motion: bool,
    muted: Rgb,
    border: Rgb,
    ok: Rgb,
    warn: Rgb,
    danger: Rgb,
    selection: Rgb,
    selection_text: Rgb,
    selection_dim: Rgb,
    key_text: Rgb,
}

/// The extremes, for the one role that has to move away from a colour that is
/// neither the background nor the text (a key chip on the accent).
const BLACK: Rgb = Rgb::new(0, 0, 0);
const WHITE: Rgb = Rgb::new(255, 255, 255);

/// Blend `from` toward `toward`; `t` is clamped to 0..=1.
fn mix(from: Rgb, toward: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Rgb::new(
        channel(from.r, toward.r),
        channel(from.g, toward.g),
        channel(from.b, toward.b),
    )
}

/// The endpoint that is further from `against` in contrast, used when a role
/// has to move away from a fill rather than toward the theme's text.
fn extreme(against: Rgb) -> Rgb {
    if ratio(WHITE, against) >= ratio(BLACK, against) {
        WHITE
    } else {
        BLACK
    }
}

/// `from`, or the smallest step from `from` toward `toward` that clears
/// `floor` against `against`. A theme that already passes is unchanged.
fn readable(from: Rgb, toward: Rgb, against: Rgb, floor: f32) -> Rgb {
    if ratio(from, against) >= floor {
        return from;
    }
    if ratio(toward, against) < floor {
        return toward;
    }
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let middle = (low + high) / 2.0;
        if ratio(mix(from, toward, middle), against) >= floor {
            high = middle;
        } else {
            low = middle;
        }
    }
    mix(from, toward, high)
}

/// The readable roles a palette implies, computed once.
struct Derived {
    muted: Rgb,
    border: Rgb,
    ok: Rgb,
    warn: Rgb,
    danger: Rgb,
    selection: Rgb,
    selection_text: Rgb,
    selection_dim: Rgb,
    key_text: Rgb,
}

impl Derived {
    fn of(palette: &Palette) -> Self {
        let muted = readable(palette.dim, palette.text, palette.bg, 4.5);
        let border = readable(palette.surface_alt, palette.text, palette.bg, 3.0);
        // Status text is the one string a person must read: an error summary,
        // "Stopped", "connected". Solarized's own red is 3.25:1 on its
        // background and Nord's 3.05, so a theme that states a quiet status
        // colour still gets a readable one. These are foreground roles only;
        // a tinted fill is `danger_bg`, which is not derived.
        let ok = readable(palette.ok, palette.text, palette.bg, 4.5);
        let warn = readable(palette.warn, palette.text, palette.bg, 4.5);
        let danger = readable(palette.danger, palette.text, palette.bg, 4.5);
        // The fill stays the theme's: lifting it to 3:1 against the background
        // would make the theme's own text unreadable on it (solarized's text
        // fell to 1.9:1). The 3:1 cue is the accent bar and the bold label;
        // the fill is a tint that says "this row", not the only signal.
        let selection = palette.selection;
        Self {
            muted,
            border,
            ok,
            warn,
            danger,
            selection,
            selection_text: readable(palette.text, extreme(selection), selection, 4.5),
            selection_dim: readable(muted, palette.text, selection, 4.5),
            key_text: readable(palette.bg, extreme(palette.accent), palette.accent, 4.5),
        }
    }
}

fn reduced_motion_file() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("hayato").join("reduced-motion")
}

fn reduced_motion_in(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| matches!(text.trim(), "1" | "yes" | "true"))
        .unwrap_or(false)
}

fn reduced_motion_requested() -> bool {
    matches!(std::env::var("HAYATO_REDUCED_MOTION"), Ok(v) if v == "1")
        || reduced_motion_in(&reduced_motion_file())
}

impl Design {
    pub fn new(palette: Palette, density: Density, reduced_motion: bool) -> Self {
        let derived = Derived::of(&palette);
        Self {
            palette,
            density,
            reduced_motion,
            muted: derived.muted,
            border: derived.border,
            ok: derived.ok,
            warn: derived.warn,
            danger: derived.danger,
            selection: derived.selection,
            selection_text: derived.selection_text,
            selection_dim: derived.selection_dim,
            key_text: derived.key_text,
        }
    }

    pub fn solarized() -> Self {
        Self::new(Palette::solarized_dark(), Density::Normal, false)
    }

    /// The session's design: the active theme's palette, the density the tablet state asks for,
    /// and reduced motion from `HAYATO_REDUCED_MOTION=1` or the saved
    /// `~/.config/hayato/reduced-motion` setting.
    pub fn for_session(runtime_dir: Option<&str>) -> Self {
        let paths = hayato_theme::Paths::from_env();
        let palette = Palette::for_session(&paths);
        Self::new(
            palette,
            Self::detect_density(runtime_dir),
            reduced_motion_requested(),
        )
    }

    pub fn with_palette(mut self, palette: Palette) -> Self {
        let derived = Derived::of(&palette);
        self.palette = palette;
        self.muted = derived.muted;
        self.border = derived.border;
        self.ok = derived.ok;
        self.warn = derived.warn;
        self.danger = derived.danger;
        self.selection = derived.selection;
        self.selection_text = derived.selection_text;
        self.selection_dim = derived.selection_dim;
        self.key_text = derived.key_text;
        self
    }

    /// Tablet mode keeps its state directory while it is on, so its presence is
    /// the signal; a session that is not rooted in an XDG runtime directory gets
    /// the normal density instead of guessing.
    pub fn detect_density(runtime_dir: Option<&str>) -> Density {
        match runtime_dir {
            Some(dir) if std::path::Path::new(dir).join("hayato-tablet").is_dir() => {
                Density::Tablet
            }
            _ => Density::Normal,
        }
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub fn density(&self) -> Density {
        self.density
    }

    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    pub fn with_density(mut self, density: Density) -> Self {
        self.density = density;
        self
    }

    pub fn with_reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }

    fn rgb_for(&self, role: ColorRole) -> Rgb {
        let p = &self.palette;
        match role {
            ColorRole::Bg => p.bg,
            ColorRole::Surface => p.surface,
            ColorRole::SurfaceAlt => p.surface_alt,
            ColorRole::Border => self.border,
            ColorRole::Text => p.text,
            ColorRole::Dim => p.dim,
            ColorRole::Muted => self.muted,
            ColorRole::Accent => p.accent,
            ColorRole::AccentAlt => p.accent_alt,
            ColorRole::Ok => self.ok,
            ColorRole::Warn => self.warn,
            ColorRole::Danger => self.danger,
            ColorRole::Selection => self.selection,
            ColorRole::SelectionText => self.selection_text,
            ColorRole::SelectionDim => self.selection_dim,
            ColorRole::KeyText => self.key_text,
        }
    }

    pub fn color(&self, role: ColorRole) -> Color {
        color(self.rgb_for(role))
    }

    /// Precompute a fixed-size colour ramp between two semantic roles.
    /// Keeps `Color::Rgb` strictly inside `design.rs` while enabling zero-alloc
    /// spectrum gradients and beat-reactive border pulses.
    pub fn ramp<const N: usize>(&self, from: ColorRole, to: ColorRole) -> [Color; N] {
        let a = self.rgb_for(from);
        let b = self.rgb_for(to);
        std::array::from_fn(|i| {
            let t = if N <= 1 {
                0.0
            } else {
                i as f32 / (N - 1) as f32
            };
            color(mix(a, b, t))
        })
    }

    pub fn style(&self, kind: TextStyle) -> Style {
        match kind {
            TextStyle::Title => Style::default()
                .fg(self.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
            TextStyle::Body => Style::default()
                .fg(self.color(ColorRole::Text))
                .bg(self.color(ColorRole::Bg)),
            TextStyle::Dim => Style::default()
                .fg(self.color(ColorRole::Muted))
                .bg(self.color(ColorRole::Bg)),
            TextStyle::Selected => Style::default()
                .fg(self.color(ColorRole::SelectionText))
                .add_modifier(Modifier::BOLD),
            TextStyle::SelectedDim => Style::default().fg(self.color(ColorRole::SelectionDim)),
            TextStyle::Code => Style::default().fg(self.color(ColorRole::AccentAlt)),
            TextStyle::Key => Style::default()
                .fg(self.color(ColorRole::KeyText))
                .bg(self.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
            TextStyle::Danger => Style::default()
                .fg(self.color(ColorRole::Danger))
                .add_modifier(Modifier::BOLD),
        }
    }

    /// The padding inside a card, in cells: the density widens it because a
    /// finger needs more room than a pointer.
    pub fn padding(&self, space: Space) -> u16 {
        let extra = match self.density {
            Density::Normal => 0,
            Density::Tablet => 1,
        };
        space.cells() + extra
    }

    pub fn header_height(&self) -> u16 {
        self.density.header_height()
    }

    pub fn row_stride(&self) -> u16 {
        self.density.row_stride()
    }

    /// A framed surface (a card, a list, the body).
    pub fn block(&self, border: Border) -> Block<'static> {
        let pad = self.padding(Space::One);
        let mut block = Block::default()
            .style(
                Style::default()
                    .bg(self.color(ColorRole::Bg))
                    .fg(self.color(ColorRole::Text)),
            )
            .padding(Padding::new(pad, pad, 0, 0));
        if border == Border::Rounded {
            block = block
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(self.color(ColorRole::Border)));
        }
        block
    }
}

/// The glyphs the widgets draw, as codepoints.
///
/// `hayato --glyphs` prints these and `check-system.sh` asks fontconfig for each
/// one: a box-drawing character that no font in foot's chain has is a hole in
/// the frame that logs nothing, which is the same class of bug as a missing icon
/// in the launcher. Keeping the list here means the check cannot drift from what
/// the widgets actually draw.
pub const GLYPHS: &[char] = &[
    '\u{2500}', // ─ horizontal
    '\u{2502}', // │ vertical
    '\u{256d}', // ╭
    '\u{256e}', // ╮
    '\u{256f}', // ╯
    '\u{2570}', // ╰
    '\u{2580}', // ▀ qr upper half block
    '\u{2582}', // ▂ signal bar 1
    '\u{2584}', // ▄ qr/signal lower half block
    '\u{2586}', // ▆ signal bar 3
    '\u{2588}', // █ gauge full / qr full block / signal bar 4
    '\u{258d}', // ▍ selection bar
    '\u{2591}', // ░ gauge empty
    '\u{2595}', // ▕ gauge end
    '\u{2026}', // … truncation
    '\u{00b7}', // · separator
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_motion_reads_the_saved_setting() {
        let dir =
            std::env::temp_dir().join(format!("hayato-reduced-motion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reduced-motion");
        std::fs::write(&path, "1\n").unwrap();
        assert!(reduced_motion_in(&path));
        std::fs::write(&path, "0\n").unwrap();
        assert!(!reduced_motion_in(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_role_maps_to_a_palette_colour() {
        let design = Design::solarized();
        let roles = [
            ColorRole::Bg,
            ColorRole::Surface,
            ColorRole::SurfaceAlt,
            ColorRole::Text,
            ColorRole::Dim,
            ColorRole::Accent,
            ColorRole::AccentAlt,
            ColorRole::Ok,
            ColorRole::Warn,
            ColorRole::Danger,
            ColorRole::Selection,
        ];
        // Solarized deliberately shares values between roles (selection is
        // base02, the same as a surface); what must hold is that nothing except
        // `Bg` maps to the background, and that the meaning-carrying roles are
        // distinguishable from each other.
        for role in roles {
            if role != ColorRole::Bg {
                assert_ne!(design.color(role), design.color(ColorRole::Bg), "{role:?}");
            }
        }
        let meaning = [
            ColorRole::Text,
            ColorRole::Dim,
            ColorRole::Accent,
            ColorRole::AccentAlt,
            ColorRole::Ok,
            ColorRole::Warn,
            ColorRole::Danger,
        ];
        for (i, a) in meaning.iter().enumerate() {
            for b in &meaning[i + 1..] {
                assert_ne!(design.color(*a), design.color(*b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn text_styles_are_readable_on_the_background() {
        let design = Design::solarized();
        // The three that carry meaning must not be the background colour: a
        // title or an error that disappears into the window is a blank row.
        for kind in [TextStyle::Title, TextStyle::Danger, TextStyle::Body] {
            assert_ne!(design.style(kind).fg, Some(design.color(ColorRole::Bg)));
        }
    }

    fn rgb(colour: Color) -> Rgb {
        match colour {
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
            other => panic!("not an RGB colour: {other:?}"),
        }
    }

    /// A theme file may keep a quiet `dim` and a flat `selection`; what the UI
    /// draws is derived, and every built-in has to clear the floor it is used
    /// at. A new theme that cannot be read fails here, with its slug.
    #[test]
    fn every_builtin_theme_clears_the_derived_floors() {
        for (slug, text) in hayato_theme::builtin::ALL {
            let theme = hayato_theme::Theme::parse(text).unwrap();
            let design = Design::new(theme.palette(), Density::Normal, true);
            // The selected row is marked by the accent bar (3:1) and the bold
            // label; its fill is a tint, and it only has to be distinct from
            // the background, not to carry the state alone.
            assert_ne!(
                design.color(ColorRole::Selection),
                design.color(ColorRole::Bg),
                "{slug}: the selection fill is the background"
            );
            for (role, against, floor) in [
                (ColorRole::Muted, ColorRole::Bg, 4.5),
                (ColorRole::Border, ColorRole::Bg, 3.0),
                (ColorRole::Ok, ColorRole::Bg, 4.5),
                (ColorRole::Warn, ColorRole::Bg, 4.5),
                (ColorRole::Danger, ColorRole::Bg, 4.5),
                (ColorRole::Accent, ColorRole::Bg, 3.0),
                (ColorRole::SelectionText, ColorRole::Selection, 4.5),
                (ColorRole::SelectionDim, ColorRole::Selection, 4.5),
                (ColorRole::KeyText, ColorRole::Accent, 4.5),
            ] {
                let value = ratio(rgb(design.color(role)), rgb(design.color(against)));
                assert!(
                    value >= floor,
                    "{slug}: {role:?} on {against:?} is {value:.2}, below {floor:.1}"
                );
            }
        }
    }

    #[test]
    fn dense_mode_grows_the_row_and_the_bands() {
        let normal = Design::solarized();
        let tablet = Design::solarized().with_density(Density::Tablet);
        assert_eq!(normal.row_stride(), 1);
        assert_eq!(tablet.row_stride(), 2);
        assert_eq!(tablet.header_height(), normal.header_height() + 1);
        assert_eq!(tablet.padding(Space::Two), normal.padding(Space::Two) + 1);
    }

    #[test]
    fn the_spacing_scale_is_four_steps() {
        assert_eq!(Space::Zero.cells(), 0);
        assert_eq!(Space::Three.cells(), 3);
        assert!(Space::Zero.cells() < Space::One.cells());
        assert!(Space::One.cells() < Space::Two.cells());
        assert!(Space::Two.cells() < Space::Three.cells());
    }

    #[test]
    fn a_missing_tablet_state_means_normal_density() {
        assert_eq!(Design::detect_density(None), Density::Normal);
        assert_eq!(
            Design::detect_density(Some("/nonexistent/hayato")),
            Density::Normal
        );
    }

    #[test]
    fn the_glyph_table_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for glyph in GLYPHS {
            assert!(seen.insert(*glyph), "duplicate glyph {glyph:?}");
        }
    }
}
