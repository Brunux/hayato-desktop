//! Hayato first-boot Mission Briefing (`hayato menu welcome` / `hayato-welcome`).
//!
//! A retro-futuristic, Ratatui-native 4-tab welcome experience:
//! - `[1] BOOT`: animated half-block pixel wordmark, scanline boot sequence,
//!   live machine telemetry, and Hayato's core ethos.
//! - `[2] HOTKEYS`: visual keycap grid of the essential Sway & desktop chords.
//! - `[3] TOOLS`: flagship studios and Wave C superpowers (Lens, Drop, Local AI,
//!   Smart Clipboard, Eject Drive, Games & Ink Notes).
//! - `[4] LAUNCHPAD`: interactive quick-launch actions (`Open Learn`, `Pick Theme`,
//!   `Open Settings`, `Enter Desktop`).

use std::fs;
use std::time::Duration;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Density, Design, Glyph, Header, KeyHint, Outcome, Screen, TextStyle,
    draw_badge, draw_card,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::system::spawn_detached_argv;

/// Hit-test base offset for Launchpad buttons (tabs use `0..4`).
const LAUNCH_HIT_BASE: usize = 100;

/// Number of animation ticks for the initial boot reveal.
const REVEAL_TICKS: u32 = 12;

/// 5x8 pixel-art bitmaps matching the Plymouth boot splash & greeter (`H`, `a`, `y`, `a`, `t`, `o`).
/// Each byte encodes 5 columns in bits 4..=0 (bit 4 = leftmost pixel).
const PIXEL_LETTERS: [[u8; 8]; 6] = [
    // H
    [
        0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001, 0b00000,
    ],
    // a
    [
        0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0b00000,
    ],
    // y
    [
        0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
    ],
    // a
    [
        0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0b00000,
    ],
    // t
    [
        0b00000, 0b00100, 0b11111, 0b00100, 0b00100, 0b00100, 0b00011, 0b00000,
    ],
    // o
    [
        0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000,
    ],
];

/// 5x8 decode/glitch glyphs (`0`, `1`, `#`, `/`, `=`, `+`).
const PIXEL_GLITCHES: [[u8; 8]; 6] = [
    // 0
    [
        0b00000, 0b01110, 0b10011, 0b10101, 0b11001, 0b01110, 0b00000, 0b00000,
    ],
    // 1
    [
        0b00000, 0b00100, 0b01100, 0b00100, 0b00100, 0b01110, 0b00000, 0b00000,
    ],
    // #
    [
        0b00000, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b00000, 0b00000,
    ],
    // /
    [
        0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000, 0b00000,
    ],
    // =
    [
        0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
    ],
    // +
    [
        0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000, 0b00000,
    ],
];

/// Pixel underscore cursor (`_`) rendered in the 4-row half-block grid.
const PIXEL_CURSOR: [u8; 8] = [
    0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111, 0b11111,
];

/// Compact cyber-eagle crest drawn beside the wordmark on wide terminals.
const EAGLE_CREST: [&str; 4] = [
    r"   __/\__   ",
    r"  /< oo >\  ",
    r"  \ \==/ /  ",
    r"   \/  \/   ",
];

/// Progressive boot-sequence lines revealed on Tab 1.
const BOOT_LINES: [&str; 4] = [
    "SwayFX Wayland compositor & statusline online",
    "Local AI (Qwen3.5-0.8B) & Hayato Lens armed",
    "Hayato Drop LAN share & smart clipboard ready",
    "Mission briefing synced — press Tab or 1-4 to explore",
];

/// Core philosophy banner.
const ETHOS_LINE: &str = "Keyboard-first · Zero bloat · Local AI · Yours";

/// Essential shortcuts shown in Tab 2 (`HOTKEYS`).
struct HotkeyEntry {
    keys: &'static str,
    desc: &'static str,
}

const HOTKEYS: &[HotkeyEntry] = &[
    HotkeyEntry {
        keys: "Alt+Space",
        desc: "Root menu: apps, tools, settings, power",
    },
    HotkeyEntry {
        keys: "Search / Alt+D",
        desc: "App launcher: type to filter, Enter opens",
    },
    HotkeyEntry {
        keys: "Alt+Return",
        desc: "Open a foot terminal",
    },
    HotkeyEntry {
        keys: "Alt+Q",
        desc: "Close the focused window",
    },
    HotkeyEntry {
        keys: "Alt+Ctrl+F",
        desc: "Browse files one directory per step",
    },
    HotkeyEntry {
        keys: "Alt+Ctrl+V",
        desc: "Smart clipboard (Ctrl+T clean / JSON)",
    },
    HotkeyEntry {
        keys: "Alt+Ctrl+Shift+L",
        desc: "Hayato Lens (local Circle-to-Search)",
    },
    HotkeyEntry {
        keys: "Alt+Ctrl+K",
        desc: "Every key binding, live from Sway",
    },
    HotkeyEntry {
        keys: "Alt+Escape",
        desc: "Power menu: lock, sleep, reboot, off",
    },
    HotkeyEntry {
        keys: "Alt+Shift+M",
        desc: "Toggle microphone privacy mute",
    },
];

/// Flagship studios & Wave C superpowers shown in Tab 3 (`TOOLS`).
struct ToolEntry {
    glyph: Glyph,
    name: &'static str,
    badge: &'static str,
    desc: &'static str,
}

const TOOLS: &[ToolEntry] = &[
    ToolEntry {
        glyph: Glyph::Crop,
        name: "Hayato Lens",
        badge: "OCR + QR",
        desc: "Local Circle-to-Search: grab any region to copy, open URL, translate or save note",
    },
    ToolEntry {
        glyph: Glyph::Qr,
        name: "Hayato Drop",
        badge: "LAN QR",
        desc: "Share or receive files over local Wi-Fi via an instant scannable QR code",
    },
    ToolEntry {
        glyph: Glyph::Sparkle,
        name: "Local AI",
        badge: "OFFLINE",
        desc: "Private Qwen3.5-0.8B Ask & Translate running locally with zero cloud account",
    },
    ToolEntry {
        glyph: Glyph::Clipboard,
        name: "Smart Clipboard",
        badge: "CTRL+T",
        desc: "History by kind with one-key tracking-param URL stripping & JSON formatting",
    },
    ToolEntry {
        glyph: Glyph::Disk,
        name: "Eject Drive",
        badge: "PLACES",
        desc: "Safe USB stick & SD card unmount + power-off straight from Places or the bar",
    },
    ToolEntry {
        glyph: Glyph::Book,
        name: "Games & Ink Notes",
        badge: "STUDIO",
        desc: "Retro ROM & HTML game library, Markdown vault, and xournalpp stylus ink notes",
    },
];

/// The four interactive tabs of the Mission Briefing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeTab {
    Boot = 0,
    Hotkeys = 1,
    Tools = 2,
    Launchpad = 3,
}

impl WelcomeTab {
    const ALL: [WelcomeTab; 4] = [
        WelcomeTab::Boot,
        WelcomeTab::Hotkeys,
        WelcomeTab::Tools,
        WelcomeTab::Launchpad,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Self {
        match index % 4 {
            0 => Self::Boot,
            1 => Self::Hotkeys,
            2 => Self::Tools,
            _ => Self::Launchpad,
        }
    }

    pub fn next(self) -> Self {
        Self::from_index(self.index() + 1)
    }

    pub fn prev(self) -> Self {
        Self::from_index(self.index() + 3)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Boot => "[1] BOOT",
            Self::Hotkeys => "[2] HOTKEYS",
            Self::Tools => "[3] TOOLS",
            Self::Launchpad => "[4] LAUNCHPAD",
        }
    }

    pub fn phase_name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Hotkeys => "hotkeys",
            Self::Tools => "tools",
            Self::Launchpad => "launchpad",
        }
    }
}

/// Action triggered from Tab 4 (`LAUNCHPAD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchAction {
    Learn,
    Theme,
    Settings,
    EnterDesktop,
}

impl LaunchAction {
    const ALL: [LaunchAction; 4] = [
        LaunchAction::Learn,
        LaunchAction::Theme,
        LaunchAction::Settings,
        LaunchAction::EnterDesktop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Learn => "[ Open Learn ]",
            Self::Theme => "[ Pick Theme ]",
            Self::Settings => "[ Open Settings ]",
            Self::EnterDesktop => "[ Enter Desktop ]",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Learn => "Read the built-in Hayato manual and keyboard guides",
            Self::Theme => "Live-preview colour palettes, blur, corners and wallpapers",
            Self::Settings => "Configure Wi-Fi, display, touchpad, local AI and privacy",
            Self::EnterDesktop => "Dismiss this briefing and start working",
        }
    }

    pub fn glyph(self) -> Glyph {
        match self {
            Self::Learn => Glyph::Book,
            Self::Theme => Glyph::Theme,
            Self::Settings => Glyph::Gear,
            Self::EnterDesktop => Glyph::Desktop,
        }
    }

    pub fn subcommand(self) -> Option<&'static str> {
        match self {
            Self::Learn => Some("learn"),
            Self::Theme => Some("theme"),
            Self::Settings => Some("settings"),
            Self::EnterDesktop => None,
        }
    }
}

/// Pre-formatted machine telemetry read once at startup (zero per-frame IO or allocation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Telemetry {
    pub hostname: String,
    pub board: String,
    pub kernel: String,
    pub memory: String,
    pub theme: String,
    pub version: String,
}

impl Telemetry {
    pub fn from_machine() -> Self {
        let hostname = fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "hayato".to_string());

        let board = read_board_name();

        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "linux-arm64".to_string());

        let memory = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| parse_mem_total(&s))
            .unwrap_or_else(|| "unknown".to_string());

        let theme_paths = hayato_theme::Paths::from_env();
        let theme =
            hayato_theme::store::active(&theme_paths).unwrap_or_else(|| "tokyo-night".to_string());

        let version = format!("v{}", crate::version::version());

        Self {
            hostname,
            board,
            kernel,
            memory,
            theme,
            version,
        }
    }
}

fn read_board_name() -> String {
    if let Ok(raw) = fs::read_to_string("/etc/hayato/device") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(raw) = fs::read_to_string("/etc/hayato/deviceinfo") {
        for key in ["DEVICE_NAME=", "BOARD=", "CODENAME="] {
            if let Some(line) = raw.lines().find(|l| l.starts_with(key)) {
                let val = line[key.len()..]
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                if !val.is_empty() {
                    return val.to_string();
                }
            }
        }
    }
    if let Ok(raw) = fs::read_to_string("/sys/firmware/devicetree/base/model") {
        let trimmed = raw.trim_end_matches('\0').trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    std::env::consts::ARCH.to_string()
}

fn parse_mem_total(meminfo: &str) -> Option<String> {
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            if kb > 0 {
                let gib = kb as f64 / 1024.0 / 1024.0;
                return Some(format!("{gib:.1} GiB RAM"));
            }
        }
    }
    None
}

fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Draw a 5x8 half-block pixel glyph (4 terminal rows x 5 terminal columns).
fn draw_pixel_glyph(buf: &mut Buffer, x: u16, y: u16, bitmap: &[u8; 8], style: Style) {
    for row in 0..4u16 {
        let py = y.saturating_add(row);
        if py >= buf.area.bottom() {
            break;
        }
        let top_bits = bitmap[(row * 2) as usize];
        let bot_bits = bitmap[(row * 2 + 1) as usize];
        for col in 0..5u16 {
            let px = x.saturating_add(col);
            if px >= buf.area.right() {
                break;
            }
            let mask = 1u8 << (4 - col);
            let top = (top_bits & mask) != 0;
            let bot = (bot_bits & mask) != 0;
            let ch = match (top, bot) {
                (true, true) => "\u{2588}",
                (true, false) => "\u{2580}",
                (false, true) => "\u{2584}",
                (false, false) => continue,
            };
            if let Some(cell) = buf.cell_mut((px, py)) {
                cell.set_symbol(ch);
                cell.set_style(style);
            }
        }
    }
}

/// The interactive 4-tab Hayato Welcome / Mission Briefing screen.
pub struct WelcomeScreen {
    tab: WelcomeTab,
    telemetry: Telemetry,
    reduced_motion: bool,
    tick: u32,
    launchpad_idx: usize,
    pub pending_launch: Option<LaunchAction>,
    header: Header,
    keybar_browse: Vec<KeyHint>,
    keybar_launchpad: Vec<KeyHint>,
}

impl WelcomeScreen {
    pub fn new(design: &Design) -> Self {
        Self::from_telemetry(Telemetry::from_machine(), design.reduced_motion())
    }

    pub fn from_telemetry(telemetry: Telemetry, reduced_motion: bool) -> Self {
        let initial_tick = if reduced_motion { REVEAL_TICKS } else { 0 };
        let header = Header::new("Hayato")
            .subtitle("Mission Briefing")
            .status(telemetry.version.clone());
        Self {
            tab: WelcomeTab::Boot,
            telemetry,
            reduced_motion,
            tick: initial_tick,
            launchpad_idx: 0,
            pending_launch: None,
            header,
            keybar_browse: vec![
                KeyHint::new("\u{2190}\u{2192}/Tab", "switch tab"),
                KeyHint::new("1-4", "jump"),
                KeyHint::new("Enter", "next"),
                KeyHint::new("Esc", "enter desktop"),
            ],
            keybar_launchpad: vec![
                KeyHint::new("\u{2191}\u{2193}", "select"),
                KeyHint::new("Enter", "launch"),
                KeyHint::new("\u{2190}\u{2192}/Tab", "switch tab"),
                KeyHint::new("Esc", "enter desktop"),
            ],
        }
    }

    pub fn tab(&self) -> WelcomeTab {
        self.tab
    }

    pub fn launchpad_index(&self) -> usize {
        self.launchpad_idx
    }

    pub fn reveal_done(&self) -> bool {
        self.reduced_motion || self.tick >= REVEAL_TICKS
    }

    fn visible_boot_lines(&self) -> usize {
        if self.reduced_motion {
            BOOT_LINES.len()
        } else {
            ((self.tick / 2) as usize + 1).min(BOOT_LINES.len())
        }
    }

    fn cursor_visible(&self) -> bool {
        self.reduced_motion || (self.tick / 5).is_multiple_of(2)
    }

    /// Compute `(x_start, x_end)` column bounds for each of the 4 tabs on `tab_y`.
    fn tab_bounds(area: Rect) -> [(u16, u16); 4] {
        let mut bounds = [(0u16, 0u16); 4];
        let mut x = area.x;
        for (i, tab) in WelcomeTab::ALL.iter().enumerate() {
            let w = (tab.label().len() as u16) + 2; // 1 space padding on each side
            let end = x.saturating_add(w).min(area.right());
            bounds[i] = (x, end);
            x = end.saturating_add(1);
        }
        bounds
    }

    /// Compute the launchpad button vertical region `(start_y, stride)` for a given body area & density.
    fn launchpad_layout(area: Rect, density: Density) -> (u16, u16) {
        let tab_h = Self::tab_bar_height(density);
        let content_y = area.y.saturating_add(tab_h);
        let content_h = area.height.saturating_sub(tab_h);
        // Card border (1) + intro line (1) + blank (1) = 3 lines before first button,
        // or 2 lines if height is tight.
        let header_rows = if content_h >= 15 { 3 } else { 2 };
        let start_y = content_y.saturating_add(header_rows);
        let stride = if density == Density::Tablet || content_h >= 20 {
            3
        } else {
            2
        };
        (start_y, stride)
    }

    fn tab_bar_height(density: Density) -> u16 {
        match density {
            Density::Normal => 2,
            Density::Tablet => 3,
        }
    }

    fn draw_tab_bar(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        let y = area.y;
        let bounds = Self::tab_bounds(area);
        let active_style = Style::default()
            .fg(design.color(ColorRole::KeyText))
            .bg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let inactive_style = Style::default()
            .fg(design.color(ColorRole::Muted))
            .bg(design.color(ColorRole::Surface));

        for (i, tab) in WelcomeTab::ALL.iter().enumerate() {
            let (x0, x1) = bounds[i];
            if x0 >= area.right() || x1 <= x0 {
                continue;
            }
            let style = if *tab == self.tab {
                active_style
            } else {
                inactive_style
            };
            let w = x1 - x0;
            let mut x = put(buf, x0, y, " ", w, style);
            x = put_trunc(buf, x, y, tab.label(), x1.saturating_sub(x), style);
            if x < x1 {
                put(buf, x, y, " ", x1 - x, style);
            }
        }

        // Right-aligned live status badge when there is room.
        let tabs_right = bounds[3].1;
        let (status_text, status_role) = if self.reveal_done() {
            ("ONLINE", ColorRole::Ok)
        } else {
            ("BOOTING", ColorRole::AccentAlt)
        };
        let badge_w = status_text.len() as u16 + 2;
        if area.right() >= tabs_right.saturating_add(badge_w + 2) {
            let bx = area.right().saturating_sub(badge_w);
            draw_badge(buf, bx, y, design, status_text, status_role);
        }

        // Horizontal separator rule under tabs.
        let rule_y = y.saturating_add(Self::tab_bar_height(design.density()) - 1);
        if rule_y < area.bottom() {
            let rule_style = Style::default().fg(design.color(ColorRole::Border));
            for cx in area.x..area.right() {
                if let Some(cell) = buf.cell_mut((cx, rule_y)) {
                    cell.set_symbol("\u{2500}");
                    cell.set_style(rule_style);
                }
            }
        }
    }

    fn draw_boot_tab(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let accent = Style::default().fg(design.color(ColorRole::Accent));
        let accent_alt = Style::default().fg(design.color(ColorRole::AccentAlt));
        let cursor_style = Style::default().fg(design.color(ColorRole::Warn));
        let ok_style = Style::default()
            .fg(design.color(ColorRole::Ok))
            .add_modifier(Modifier::BOLD);
        let body_style = design.style(TextStyle::Body);
        let dim_style = design.style(TextStyle::Dim);
        let title_style = design.style(TextStyle::Title);

        let mut y = area.y;

        // 1. Cyber-banner: 4-row half-block pixel wordmark on >= 52x15, compact 1-line otherwise.
        if area.width >= 52 && area.height >= 15 {
            let wordmark_w: u16 = 6 * 6 + 5; // 6 letters (5 + 1 gap) + 5-col cursor = 41
            let show_crest = area.width >= wordmark_w + 18;
            let total_w = if show_crest {
                wordmark_w + 16
            } else {
                wordmark_w
            };
            let start_x = area
                .x
                .saturating_add(area.width.saturating_sub(total_w) / 2);

            let mut lx = start_x;
            if show_crest {
                for (row_i, line) in EAGLE_CREST.iter().enumerate() {
                    put(
                        buf,
                        lx,
                        y.saturating_add(row_i as u16),
                        line,
                        14,
                        accent_alt,
                    );
                }
                lx = lx.saturating_add(15);
            }

            for (idx, letter) in PIXEL_LETTERS.iter().enumerate() {
                let (bitmap, style) = if self.reduced_motion {
                    (letter, accent)
                } else if self.tick < (idx as u32 * 2 + 2) {
                    let g = (mix64((idx as u64 + 1) ^ (self.tick as u64)) as usize)
                        % PIXEL_GLITCHES.len();
                    (&PIXEL_GLITCHES[g], accent_alt)
                } else {
                    // Subtle periodic single-letter glitch pulse after boot reveal.
                    let cycle = self.tick % 48;
                    let bucket = (self.tick / 48) as u64;
                    let h = mix64(0x4841_5941_544f ^ bucket.wrapping_mul(0x9e37_79b9_7f4a_7c15));
                    let target = (h as usize) % PIXEL_LETTERS.len();
                    if self.tick >= REVEAL_TICKS + 12 && idx == target && cycle < 2 {
                        let g = (mix64(h ^ (self.tick as u64)) as usize) % PIXEL_GLITCHES.len();
                        (&PIXEL_GLITCHES[g], accent_alt)
                    } else {
                        (letter, accent)
                    }
                };
                draw_pixel_glyph(buf, lx, y, bitmap, style);
                lx = lx.saturating_add(6);
            }

            if self.cursor_visible() {
                draw_pixel_glyph(buf, lx, y, &PIXEL_CURSOR, cursor_style);
            }
            y = y.saturating_add(4);

            // Tagline centred directly under the banner.
            let tagline = ":: light. fast. fun. ::";
            let tx = area
                .x
                .saturating_add(area.width.saturating_sub(tagline.len() as u16) / 2);
            put(buf, tx, y, tagline, area.width, accent_alt);
            y = y.saturating_add(2);
        } else {
            let mut x = put(buf, area.x + 1, y, "> Hayato", area.width, title_style);
            if self.cursor_visible() {
                x = put(buf, x, y, " _", 2, cursor_style);
            }
            put_trunc(
                buf,
                x.saturating_add(2),
                y,
                ":: light. fast. fun. ::",
                area.right().saturating_sub(x + 2),
                accent_alt,
            );
            y = y.saturating_add(2);
        }

        // 2. Progressive boot-sequence reveal lines.
        let visible_lines = self.visible_boot_lines();
        for line in BOOT_LINES.iter().take(visible_lines) {
            if y >= area.bottom().saturating_sub(5) {
                break;
            }
            let x = put(buf, area.x + 1, y, "[ OK ] ", 7, ok_style);
            put_trunc(
                buf,
                x,
                y,
                line,
                area.right().saturating_sub(x + 1),
                body_style,
            );
            y = y.saturating_add(1);
        }
        if !self.reveal_done() && y < area.bottom().saturating_sub(5) {
            put(
                buf,
                area.x + 1,
                y,
                "[ .. ] initializing subsystem",
                area.width.saturating_sub(2),
                dim_style,
            );
        }

        // Leave a breathing line if space allows before the telemetry card.
        let remaining = area.bottom().saturating_sub(y);
        if remaining >= 8 {
            y = y.saturating_add(1);
        }

        // 3. Live machine telemetry card + ethos strip.
        let ethos_h: u16 = if area.bottom().saturating_sub(y) >= 7 {
            2
        } else {
            1
        };
        let card_h = area.bottom().saturating_sub(y).saturating_sub(ethos_h);
        if card_h >= 4 {
            let card_area = Rect::new(area.x, y, area.width, card_h);
            let inner = draw_card(buf, card_area, design, Some("TELEMETRY"), false);
            let pairs = [
                ("HOST", self.telemetry.hostname.as_str()),
                ("BOARD", self.telemetry.board.as_str()),
                ("KERNEL", self.telemetry.kernel.as_str()),
                ("MEMORY", self.telemetry.memory.as_str()),
                ("THEME", self.telemetry.theme.as_str()),
                ("BUILD", self.telemetry.version.as_str()),
            ];
            let key_style = Style::default()
                .fg(design.color(ColorRole::AccentAlt))
                .add_modifier(Modifier::BOLD);
            let two_col = inner.width >= 56;
            if two_col {
                let col_w = inner.width / 2;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    let row = (i / 2) as u16;
                    if row >= inner.height {
                        break;
                    }
                    let col = (i % 2) as u16;
                    let cx = inner.x.saturating_add(col * col_w).saturating_add(1);
                    let cy = inner.y.saturating_add(row);
                    let vx = put(buf, cx, cy, k, 7, key_style);
                    let label_end = cx.saturating_add(8);
                    let _ = vx;
                    let max_v = cx.saturating_add(col_w).saturating_sub(label_end + 1);
                    put_trunc(buf, label_end, cy, v, max_v, body_style);
                }
            } else {
                for (i, (k, v)) in pairs.iter().enumerate() {
                    let row = i as u16;
                    if row >= inner.height {
                        break;
                    }
                    let cy = inner.y.saturating_add(row);
                    put(buf, inner.x + 1, cy, k, 7, key_style);
                    put_trunc(
                        buf,
                        inner.x + 9,
                        cy,
                        v,
                        inner.width.saturating_sub(10),
                        body_style,
                    );
                }
            }
            y = card_area.bottom();
        }

        // 4. Core ethos strip at the bottom of the Boot view.
        let ethos_y = area.bottom().saturating_sub(1).max(y);
        if ethos_y < area.bottom() {
            let prompt_w = ETHOS_LINE.len() as u16 + 2;
            let ex = area
                .x
                .saturating_add(area.width.saturating_sub(prompt_w) / 2);
            let x = put(buf, ex, ethos_y, "> ", 2, cursor_style);
            put_trunc(
                buf,
                x,
                ethos_y,
                ETHOS_LINE,
                area.right().saturating_sub(x),
                title_style,
            );
        }
    }

    fn draw_hotkeys_tab(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.height < 4 || area.width < 20 {
            return;
        }
        let footer_h: u16 = if area.height >= 12 { 1 } else { 0 };
        let card_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(footer_h),
        );
        let inner = draw_card(buf, card_area, design, Some("ESSENTIAL SHORTCUTS"), true);

        let keycap_style = Style::default()
            .fg(design.color(ColorRole::Accent))
            .bg(design.color(ColorRole::SurfaceAlt))
            .add_modifier(Modifier::BOLD);
        let desc_style = design.style(TextStyle::Body);
        let dim_style = design.style(TextStyle::Dim);

        let two_col = inner.width >= 72;
        if two_col {
            let col_w = inner.width / 2;
            let rows_per_col = HOTKEYS.len().div_ceil(2);
            let stride: u16 = if inner.height >= (rows_per_col as u16) * 3 {
                3
            } else if inner.height >= (rows_per_col as u16) * 2 {
                2
            } else {
                1
            };
            for (i, hk) in HOTKEYS.iter().enumerate() {
                let col = if i < rows_per_col { 0u16 } else { 1u16 };
                let row_i = if i < rows_per_col {
                    i
                } else {
                    i - rows_per_col
                };
                let cy = inner.y.saturating_add((row_i as u16) * stride);
                if cy >= inner.bottom() {
                    continue;
                }
                let cx = inner.x.saturating_add(col * col_w).saturating_add(1);
                let col_right = inner.x.saturating_add((col + 1) * col_w).saturating_sub(1);
                let mut x = put(buf, cx, cy, " ", 1, keycap_style);
                x = put_trunc(
                    buf,
                    x,
                    cy,
                    hk.keys,
                    col_right.saturating_sub(x),
                    keycap_style,
                );
                if x < col_right {
                    x = put(buf, x, cy, " ", 1, keycap_style);
                }
                if stride >= 2 && cy + 1 < inner.bottom() {
                    put_trunc(
                        buf,
                        cx,
                        cy + 1,
                        hk.desc,
                        col_right.saturating_sub(cx),
                        desc_style,
                    );
                } else if x + 1 < col_right {
                    put_trunc(
                        buf,
                        x + 1,
                        cy,
                        hk.desc,
                        col_right.saturating_sub(x + 1),
                        desc_style,
                    );
                }
            }
        } else {
            for (i, hk) in HOTKEYS.iter().enumerate() {
                let cy = inner.y.saturating_add(i as u16);
                if cy >= inner.bottom() {
                    break;
                }
                let cx = inner.x.saturating_add(1);
                let mut x = put(buf, cx, cy, " ", 1, keycap_style);
                x = put_trunc(buf, x, cy, hk.keys, 18, keycap_style);
                x = put(buf, x, cy, " ", 1, keycap_style);
                let desc_x = cx.saturating_add(19).max(x + 1);
                put_trunc(
                    buf,
                    desc_x,
                    cy,
                    hk.desc,
                    inner.right().saturating_sub(desc_x + 1),
                    desc_style,
                );
            }
        }

        if footer_h > 0 {
            let fy = area.bottom().saturating_sub(1);
            put_trunc(
                buf,
                area.x + 1,
                fy,
                "Tip: press Alt+Ctrl+K anytime for the live searchable cheatsheet.",
                area.width.saturating_sub(2),
                dim_style,
            );
        }
    }

    fn draw_tools_tab(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        if area.height < 4 || area.width < 20 {
            return;
        }
        let card_area = Rect::new(area.x, area.y, area.width, area.height);
        let inner = draw_card(
            buf,
            card_area,
            design,
            Some("FLAGSHIP STUDIOS & WAVE C SUPERPOWERS"),
            true,
        );

        let name_style = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);
        let glyph_style = Style::default().fg(design.color(ColorRole::AccentAlt));
        let desc_style = design.style(TextStyle::Body);
        let dim_style = design.style(TextStyle::Dim);

        let two_col = inner.width >= 72 && inner.height >= 12;
        if two_col {
            let col_w = inner.width / 2;
            let row_h = (inner.height / 3).max(4);
            for (i, tool) in TOOLS.iter().enumerate() {
                let col = (i % 2) as u16;
                let row = (i / 2) as u16;
                let tx = inner.x.saturating_add(col * col_w).saturating_add(1);
                let ty = inner.y.saturating_add(row * row_h);
                if ty >= inner.bottom() {
                    break;
                }
                let max_w = col_w.saturating_sub(2);
                let x = put(buf, tx, ty, tool.glyph.cell(), 2, glyph_style);
                let x = put(
                    buf,
                    x + 1,
                    ty,
                    tool.name,
                    max_w.saturating_sub(3),
                    name_style,
                );
                if x + 2 + tool.badge.len() as u16 + 2 <= tx + max_w {
                    draw_badge(buf, x + 2, ty, design, tool.badge, ColorRole::AccentAlt);
                }

                // Wrap description neatly into up to 2 lines without allocating.
                let desc_y = ty.saturating_add(1);
                if desc_y < inner.bottom() {
                    let (line1, line2) = split_two_lines(tool.desc, max_w as usize);
                    put_trunc(buf, tx, desc_y, line1, max_w, desc_style);
                    if !line2.is_empty() && desc_y + 1 < inner.bottom() && row_h >= 3 {
                        put_trunc(buf, tx, desc_y + 1, line2, max_w, dim_style);
                    }
                }
            }
        } else {
            let stride: u16 = if inner.height >= (TOOLS.len() as u16) * 2 {
                2
            } else {
                1
            };
            for (i, tool) in TOOLS.iter().enumerate() {
                let ty = inner.y.saturating_add((i as u16) * stride);
                if ty >= inner.bottom() {
                    break;
                }
                let tx = inner.x.saturating_add(1);
                let max_w = inner.width.saturating_sub(2);
                let x = put(buf, tx, ty, tool.glyph.cell(), 2, glyph_style);
                let x = put(
                    buf,
                    x + 1,
                    ty,
                    tool.name,
                    max_w.saturating_sub(3),
                    name_style,
                );
                if stride >= 2 && ty + 1 < inner.bottom() {
                    if x + 2 + tool.badge.len() as u16 + 2 <= tx + max_w {
                        draw_badge(buf, x + 2, ty, design, tool.badge, ColorRole::AccentAlt);
                    }
                    put_trunc(buf, tx, ty + 1, tool.desc, max_w, desc_style);
                } else if x + 2 < inner.right() {
                    put_trunc(
                        buf,
                        x + 2,
                        ty,
                        tool.desc,
                        inner.right().saturating_sub(x + 3),
                        desc_style,
                    );
                }
            }
        }
    }

    fn draw_launchpad_tab(&self, buf: &mut Buffer, full_area: Rect, area: Rect, design: &Design) {
        if area.height < 4 || area.width < 20 {
            return;
        }
        let footer_h: u16 = if area.height >= 14 { 1 } else { 0 };
        let card_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(footer_h),
        );
        let inner = draw_card(buf, card_area, design, Some("MISSION LAUNCHPAD"), true);

        let dim_style = design.style(TextStyle::Dim);
        let body_style = design.style(TextStyle::Body);

        put_trunc(
            buf,
            inner.x + 1,
            inner.y,
            "Choose your first stop — press Enter or click any action:",
            inner.width.saturating_sub(2),
            dim_style,
        );

        let (start_y, stride) = Self::launchpad_layout(full_area, design.density());

        for (i, action) in LaunchAction::ALL.iter().enumerate() {
            let row_y = start_y.saturating_add((i as u16) * stride);
            if row_y >= inner.bottom() {
                break;
            }
            let selected = i == self.launchpad_idx;
            let row_h = if stride >= 3 && row_y + 1 < inner.bottom() {
                2
            } else {
                1
            };
            let row_rect = Rect::new(inner.x, row_y, inner.width, row_h);

            if selected {
                let fill = Style::default().bg(design.color(ColorRole::Selection));
                buf.set_style(row_rect, fill);
                let bar_style = Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(design.color(ColorRole::Selection));
                for dy in 0..row_h {
                    put(buf, inner.x, row_y + dy, "\u{258e}", 1, bar_style);
                }
            }

            let label_style = if selected {
                design
                    .style(TextStyle::Selected)
                    .bg(design.color(ColorRole::Selection))
            } else {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            };
            let detail_style = if selected {
                design
                    .style(TextStyle::SelectedDim)
                    .bg(design.color(ColorRole::Selection))
            } else {
                body_style
            };
            let glyph_style = if selected {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(design.color(ColorRole::Selection))
            } else {
                Style::default().fg(design.color(ColorRole::AccentAlt))
            };

            let x = put(
                buf,
                inner.x + 2,
                row_y,
                action.glyph().cell(),
                2,
                glyph_style,
            );
            let x = put(
                buf,
                x + 1,
                row_y,
                action.label(),
                inner.right().saturating_sub(x + 2),
                label_style,
            );

            if row_h >= 2 {
                put_trunc(
                    buf,
                    inner.x + 5,
                    row_y + 1,
                    action.detail(),
                    inner.width.saturating_sub(6),
                    detail_style,
                );
            } else if x + 2 < inner.right() {
                put_trunc(
                    buf,
                    x + 2,
                    row_y,
                    action.detail(),
                    inner.right().saturating_sub(x + 3),
                    detail_style,
                );
            }

            if selected && inner.width >= 36 {
                let hint = "[Enter]";
                let hx = inner.right().saturating_sub(hint.len() as u16 + 1);
                if hx > x + 1 {
                    let hint_style = Style::default()
                        .fg(design.color(ColorRole::Accent))
                        .bg(design.color(ColorRole::Selection))
                        .add_modifier(Modifier::BOLD);
                    put(buf, hx, row_y, hint, hint.len() as u16, hint_style);
                }
            }
        }

        if footer_h > 0 {
            let fy = area.bottom().saturating_sub(1);
            put_trunc(
                buf,
                area.x + 1,
                fy,
                "Appears once per install. Reopen anytime via `hayato menu welcome`.",
                area.width.saturating_sub(2),
                dim_style,
            );
        }
    }
}

/// Split a short static description into at most two lines at a word boundary without allocating.
fn split_two_lines(text: &str, max_width: usize) -> (&str, &str) {
    if text.len() <= max_width || max_width == 0 {
        return (text, "");
    }
    let slice = &text[..text.floor_char_boundary(max_width)];
    if let Some(pos) = slice.rfind(' ') {
        let first = text[..pos].trim_end();
        let rest = text[pos + 1..].trim_start();
        (first, rest)
    } else {
        (slice, &text[slice.len()..])
    }
}

impl Screen for WelcomeScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if ctx.area.width == 0 || ctx.area.height == 0 {
            return;
        }
        self.draw_tab_bar(ctx.buf, ctx.area, ctx.design);

        let tab_h = Self::tab_bar_height(ctx.design.density());
        let content = Rect::new(
            ctx.area.x,
            ctx.area.y.saturating_add(tab_h),
            ctx.area.width,
            ctx.area.height.saturating_sub(tab_h),
        );

        match self.tab {
            WelcomeTab::Boot => self.draw_boot_tab(ctx.buf, content, ctx.design),
            WelcomeTab::Hotkeys => self.draw_hotkeys_tab(ctx.buf, content, ctx.design),
            WelcomeTab::Tools => self.draw_tools_tab(ctx.buf, content, ctx.design),
            WelcomeTab::Launchpad => {
                self.draw_launchpad_tab(ctx.buf, ctx.area, content, ctx.design)
            }
        }
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.pending_launch = Some(LaunchAction::EnterDesktop);
                Outcome::Done(0)
            }
            KeyCode::Char('1') => {
                self.tab = WelcomeTab::Boot;
                Outcome::Consumed
            }
            KeyCode::Char('2') => {
                self.tab = WelcomeTab::Hotkeys;
                Outcome::Consumed
            }
            KeyCode::Char('3') => {
                self.tab = WelcomeTab::Tools;
                Outcome::Consumed
            }
            KeyCode::Char('4') => {
                self.tab = WelcomeTab::Launchpad;
                Outcome::Consumed
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.tab = self.tab.next();
                Outcome::Consumed
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.tab = self.tab.prev();
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') if self.tab == WelcomeTab::Launchpad => {
                self.launchpad_idx = (self.launchpad_idx + 1) % LaunchAction::ALL.len();
                Outcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') if self.tab == WelcomeTab::Launchpad => {
                self.launchpad_idx =
                    (self.launchpad_idx + LaunchAction::ALL.len() - 1) % LaunchAction::ALL.len();
                Outcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if self.tab == WelcomeTab::Launchpad {
                    self.pending_launch = Some(LaunchAction::ALL[self.launchpad_idx]);
                    Outcome::Done(0)
                } else {
                    self.tab = self.tab.next();
                    Outcome::Consumed
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if x < area.x || x >= area.right() || y < area.y || y >= area.bottom() {
            return None;
        }
        // Tab bar hit-test on the top row.
        if y == area.y {
            let bounds = Self::tab_bounds(area);
            for (i, (x0, x1)) in bounds.iter().enumerate() {
                if x >= *x0 && x < *x1 {
                    return Some(i);
                }
            }
            return None;
        }

        // Launchpad button hit-test when on Tab 4.
        if self.tab == WelcomeTab::Launchpad {
            let (start_y, stride) = Self::launchpad_layout(area, design.density());
            let row_h = if stride >= 3 { 2 } else { 1 };
            for i in 0..LaunchAction::ALL.len() {
                let ry = start_y.saturating_add((i as u16) * stride);
                if y >= ry && y < ry.saturating_add(row_h) {
                    return Some(LAUNCH_HIT_BASE + i);
                }
            }
        }
        None
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(id) = row else {
            return Outcome::Consumed;
        };
        if id < WelcomeTab::ALL.len() {
            self.tab = WelcomeTab::from_index(id);
            return Outcome::Consumed;
        }
        if id >= LAUNCH_HIT_BASE {
            let idx = id - LAUNCH_HIT_BASE;
            if idx < LaunchAction::ALL.len() {
                self.launchpad_idx = idx;
                self.pending_launch = Some(LaunchAction::ALL[idx]);
                return Outcome::Done(0);
            }
        }
        Outcome::Consumed
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(match self.tab {
            WelcomeTab::Launchpad => &self.keybar_launchpad,
            _ => &self.keybar_browse,
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.reduced_motion || self.tab != WelcomeTab::Boot {
            return None;
        }
        if self.tick < REVEAL_TICKS {
            Some(Duration::from_millis(80))
        } else {
            Some(Duration::from_millis(120))
        }
    }

    fn tick(&mut self) -> Outcome {
        if !self.reduced_motion {
            self.tick = self.tick.wrapping_add(1);
        }
        Outcome::Continue
    }

    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((880, 580))
    }

    fn phase(&self) -> Option<&'static str> {
        Some(self.tab.phase_name())
    }
}

/// Run the interactive Welcome Mission Briefing (`hayato menu welcome`).
pub fn menu(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = WelcomeScreen::new(design);
    let code = crate::run_screen(terminal, design, "Mission Briefing", &mut screen)?;
    if let Some(action) = screen.pending_launch
        && let Some(sub) = action.subcommand()
    {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "hayato".to_string());
        spawn_detached_argv(&exe, &["menu", sub]);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn sample_telemetry() -> Telemetry {
        Telemetry {
            hostname: "hayato-cm3".to_string(),
            board: "google-hayato (MT8192)".to_string(),
            kernel: "7.2.7-hayato".to_string(),
            memory: "3.8 GiB RAM".to_string(),
            theme: "tokyo-night".to_string(),
            version: "v0.9.0".to_string(),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render_screen(
        screen: &mut WelcomeScreen,
        design: &Design,
        width: u16,
        height: u16,
    ) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let mut ctx = BodyCtx {
            area,
            buf: &mut buf,
            design,
        };
        screen.draw(&mut ctx);
        hayato_ui::screen(&buf)
    }

    #[test]
    fn tab_switching_with_keys_and_numbers() {
        let design = Design::solarized().with_reduced_motion(true);
        let mut screen = WelcomeScreen::from_telemetry(sample_telemetry(), true);
        assert_eq!(screen.tab(), WelcomeTab::Boot);
        assert_eq!(screen.phase(), Some("boot"));

        assert_eq!(screen.key(press(KeyCode::Tab), &design), Outcome::Consumed);
        assert_eq!(screen.tab(), WelcomeTab::Hotkeys);
        assert_eq!(screen.phase(), Some("hotkeys"));

        assert_eq!(
            screen.key(press(KeyCode::Right), &design),
            Outcome::Consumed
        );
        assert_eq!(screen.tab(), WelcomeTab::Tools);

        assert_eq!(
            screen.key(press(KeyCode::Char('4')), &design),
            Outcome::Consumed
        );
        assert_eq!(screen.tab(), WelcomeTab::Launchpad);

        assert_eq!(
            screen.key(press(KeyCode::BackTab), &design),
            Outcome::Consumed
        );
        assert_eq!(screen.tab(), WelcomeTab::Tools);

        assert_eq!(
            screen.key(press(KeyCode::Char('1')), &design),
            Outcome::Consumed
        );
        assert_eq!(screen.tab(), WelcomeTab::Boot);
    }

    #[test]
    fn reduced_motion_skips_animation_and_ticks() {
        let animated = WelcomeScreen::from_telemetry(sample_telemetry(), false);
        assert!(!animated.reveal_done());
        assert!(animated.needs_tick().is_some());

        let mut reduced = WelcomeScreen::from_telemetry(sample_telemetry(), true);
        assert!(reduced.reveal_done());
        assert!(reduced.needs_tick().is_none());
        assert_eq!(reduced.visible_boot_lines(), BOOT_LINES.len());

        let design = Design::solarized().with_reduced_motion(true);
        let text = render_screen(&mut reduced, &design, 80, 24);
        assert!(text.contains("ONLINE"));
        assert!(text.contains("SwayFX Wayland compositor"));
        assert!(text.contains(ETHOS_LINE));
    }

    #[test]
    fn renders_all_tabs_at_80x24_and_152x45() {
        let design = Design::solarized().with_reduced_motion(true);
        for (w, h) in [(80, 24), (152, 45)] {
            let mut screen = WelcomeScreen::from_telemetry(sample_telemetry(), true);

            // Tab 1: BOOT
            let boot = render_screen(&mut screen, &design, w, h);
            assert!(boot.contains("[1] BOOT"), "{w}x{h} missing tab 1");
            assert!(boot.contains("hayato-cm3"), "{w}x{h} missing hostname");
            assert!(boot.contains("google-hayato"), "{w}x{h} missing board");
            assert!(boot.contains(ETHOS_LINE), "{w}x{h} missing ethos");

            // Tab 2: HOTKEYS
            screen.key(press(KeyCode::Char('2')), &design);
            let hotkeys = render_screen(&mut screen, &design, w, h);
            assert!(hotkeys.contains("Alt+Space"), "{w}x{h} missing Alt+Space");
            assert!(
                hotkeys.contains("Alt+Ctrl+Shift+L"),
                "{w}x{h} missing Lens shortcut"
            );
            assert!(
                hotkeys.contains("Alt+Shift+M"),
                "{w}x{h} missing Mic mute shortcut"
            );

            // Tab 3: TOOLS
            screen.key(press(KeyCode::Char('3')), &design);
            let tools = render_screen(&mut screen, &design, w, h);
            assert!(tools.contains("Hayato Lens"), "{w}x{h} missing Lens");
            assert!(tools.contains("Hayato Drop"), "{w}x{h} missing Drop");
            assert!(tools.contains("Local AI"), "{w}x{h} missing Local AI");
            assert!(tools.contains("Eject Drive"), "{w}x{h} missing Eject Drive");

            // Tab 4: LAUNCHPAD
            screen.key(press(KeyCode::Char('4')), &design);
            let launch = render_screen(&mut screen, &design, w, h);
            assert!(launch.contains("[ Open Learn ]"), "{w}x{h} missing Learn");
            assert!(launch.contains("[ Pick Theme ]"), "{w}x{h} missing Theme");
            assert!(
                launch.contains("[ Open Settings ]"),
                "{w}x{h} missing Settings"
            );
            assert!(
                launch.contains("[ Enter Desktop ]"),
                "{w}x{h} missing Enter Desktop"
            );
        }
    }

    #[test]
    fn mouse_hit_and_click_on_tabs_and_launchpad() {
        let design = Design::solarized().with_reduced_motion(true);
        let mut screen = WelcomeScreen::from_telemetry(sample_telemetry(), true);
        let area = Rect::new(0, 0, 80, 21);

        // Click Tab 4 ("[4] LAUNCHPAD").
        let bounds = WelcomeScreen::tab_bounds(area);
        let (tx0, _) = bounds[3];
        let hit_tab = screen.hit(area, tx0 + 1, area.y, &design);
        assert_eq!(hit_tab, Some(3));
        assert_eq!(screen.click(hit_tab), Outcome::Consumed);
        assert_eq!(screen.tab(), WelcomeTab::Launchpad);

        // Click second Launchpad button ("[ Pick Theme ]").
        let (start_y, stride) = WelcomeScreen::launchpad_layout(area, design.density());
        let hit_btn = screen.hit(area, area.x + 4, start_y + stride, &design);
        assert_eq!(hit_btn, Some(LAUNCH_HIT_BASE + 1));
        assert_eq!(screen.click(hit_btn), Outcome::Done(0));
        assert_eq!(screen.pending_launch, Some(LaunchAction::Theme));
    }

    #[test]
    fn desired_size_matches_sway_window_rule() {
        let screen = WelcomeScreen::from_telemetry(sample_telemetry(), true);
        assert_eq!(screen.desired_size(), Some((880, 580)));
    }
}
