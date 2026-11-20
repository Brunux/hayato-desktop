//! The session palette, by role.
//!
//! Spec 001 ships the roles the frame needs; spec 004 (`theme.toml` and its
//! generators) turns this into the engine that owns the files. The point of the
//! split is that a widget asks for `palette.accent`, never for a hex value, so a
//! second theme is data rather than 200 edits.
//!
//! The values are Tokyo Night, the session's default theme: the framework must
//! look like the session it replaces (`~/.config/foot/foot.ini`, `mako/config`,
//! the GTK overrides).

pub mod builtin;
pub mod contrast;
pub mod render;
pub mod store;
pub mod theme;
pub mod wallpaper;

pub use store::{Outcome, Paths, SURFACES, Surface};
pub use theme::{Hex, Motion, Roles, Theme};
pub use wallpaper::{
    DEFAULT_HEIGHT, DEFAULT_WIDTH, Wallpaper, generate_thumbnail, generate_wallpaper,
    get_or_generate,
};

/// One colour, independent of any terminal library (the UI crate maps it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Solarized dark's base and accent colours.
pub const BASE03: Rgb = Rgb::new(0x00, 0x2b, 0x36);
pub const BASE02: Rgb = Rgb::new(0x07, 0x36, 0x42);
pub const BASE01: Rgb = Rgb::new(0x58, 0x6e, 0x75);
pub const BASE00: Rgb = Rgb::new(0x65, 0x7b, 0x83);
pub const BASE0: Rgb = Rgb::new(0x83, 0x94, 0x96);
pub const BASE1: Rgb = Rgb::new(0x93, 0xa1, 0xa1);
pub const CYAN: Rgb = Rgb::new(0x2a, 0xa1, 0x98);
pub const BLUE: Rgb = Rgb::new(0x26, 0x8b, 0xd2);
pub const GREEN: Rgb = Rgb::new(0x85, 0x99, 0x00);
pub const YELLOW: Rgb = Rgb::new(0xb5, 0x89, 0x00);
pub const ORANGE: Rgb = Rgb::new(0xcb, 0x4b, 0x16);
pub const RED: Rgb = Rgb::new(0xdc, 0x32, 0x2f);

/// Tokyo Night's background, surface and accent colours.
pub const TN_BG: Rgb = Rgb::new(0x1a, 0x1b, 0x26);
pub const TN_SURFACE: Rgb = Rgb::new(0x24, 0x28, 0x3b);
pub const TN_SURFACE_ALT: Rgb = Rgb::new(0x29, 0x2e, 0x42);
pub const TN_TEXT: Rgb = Rgb::new(0xc0, 0xca, 0xf5);
pub const TN_DIM: Rgb = Rgb::new(0x56, 0x5f, 0x89);
pub const TN_BLUE: Rgb = Rgb::new(0x7a, 0xa2, 0xf7);
pub const TN_CYAN: Rgb = Rgb::new(0x7d, 0xcf, 0xff);
pub const TN_GREEN: Rgb = Rgb::new(0x9e, 0xce, 0x6a);
pub const TN_YELLOW: Rgb = Rgb::new(0xe0, 0xaf, 0x68);
pub const TN_RED: Rgb = Rgb::new(0xf7, 0x76, 0x8e);

/// The roles a surface asks for. Adding a role is a deliberate act: a widget
/// that needs "a slightly different grey" is a token that belongs here, not a
/// colour in the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Rgb,
    pub surface: Rgb,
    pub surface_alt: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub accent_alt: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
    pub danger: Rgb,
    pub selection: Rgb,
}

impl Palette {
    pub const fn tokyo_night() -> Self {
        Self {
            bg: TN_BG,
            surface: TN_SURFACE,
            surface_alt: TN_SURFACE_ALT,
            text: TN_TEXT,
            dim: TN_DIM,
            accent: TN_BLUE,
            accent_alt: TN_CYAN,
            ok: TN_GREEN,
            warn: TN_YELLOW,
            danger: TN_RED,
            selection: TN_SURFACE_ALT,
        }
    }

    pub const fn solarized_dark() -> Self {
        Self {
            bg: BASE03,
            surface: BASE02,
            surface_alt: BASE01,
            text: BASE1,
            dim: BASE01,
            accent: CYAN,
            accent_alt: BLUE,
            ok: GREEN,
            warn: YELLOW,
            danger: RED,
            selection: BASE02,
        }
    }

    /// The session's active palette: reads the active theme slug from state
    /// and parses its definition from built-ins (or the Tokyo Night fallback).
    pub fn for_session(paths: &Paths) -> Self {
        if let Some(slug) = store::active(paths)
            && let Some(text) = builtin::get(&slug)
            && let Ok(theme) = Theme::parse(text)
        {
            return theme.palette();
        }
        Self::tokyo_night()
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokyo_night_is_the_default() {
        assert_eq!(Palette::default(), Palette::tokyo_night());
    }

    #[test]
    fn the_roles_the_frame_needs_are_distinct() {
        let p = Palette::solarized_dark();
        // An error card on a background of the same colour is the bug this
        // catches: every surface role must be visible against `bg`.
        assert_ne!(p.bg, p.text);
        assert_ne!(p.bg, p.danger);
        assert_ne!(p.bg, p.accent);
        assert_ne!(p.text, p.dim);
    }
}
