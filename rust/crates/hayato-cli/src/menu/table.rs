//! The root menu as data (spec 007 + 2026 IA reorganization).
//!
//! Every row is a [`MenuItem`] in one of six non-overlapping groups (`Workspace`,
//! `Create, Capture & Share`, `Utilities`, `AI`, `System`, `Session`), and every
//! row's action is an [`Action`] value: a program and an argument list, or one of
//! the few things the menu does itself. There is no shell anywhere in this file
//! (`action_never_contains_a_shell_metacharacter`).
//!
//! Granular configuration and legacy script entries live in [`PALETTE_EXTRA`] so
//! `hayato menu palette` keeps 100% flat searchability while `Alt+Space` stays
//! focused and zero-overlap.

use hayato_ui::Glyph;

/// The menu's groups, in the order they are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Workspace,
    Capture,
    Utilities,
    Ai,
    System,
    Session,
}

impl Group {
    /// Lowercase word shown in the value column and matched by typing.
    pub fn label(self) -> &'static str {
        match self {
            Group::Workspace => "workspace",
            Group::Capture => "capture",
            Group::Utilities => "utilities",
            Group::Ai => "ai",
            Group::System => "system",
            Group::Session => "session",
        }
    }

    /// Section heading rendered above the group.
    pub fn heading(self) -> &'static str {
        match self {
            Group::Workspace => "Workspace",
            Group::Capture => "Create, Capture & Share",
            Group::Utilities => "Utilities",
            Group::Ai => "AI",
            Group::System => "System",
            Group::Session => "Session",
        }
    }

    /// Order the headings appear in so surfaces appending rows keep groups contiguous.
    pub fn order(self) -> u8 {
        match self {
            Group::Workspace => 0,
            Group::Capture => 1,
            Group::Utilities => 2,
            Group::Ai => 3,
            Group::System => 4,
            Group::Session => 5,
        }
    }
}

/// One argument of a program a row runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    Lit(&'static str),
    /// The person's home directory, filled in when the row runs.
    Home,
}

/// Where a row's program comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    /// A session script, run from `~/.config/bin` by its full path.
    Session(&'static str),
    /// A program from `PATH` (`foot`, `hayato`, `favnyr`).
    System(&'static str),
    /// This binary with these words, by the path the process was started from.
    Hayato,
}

/// A thing the Power list does to the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    Lock,
    Suspend,
    Hibernate,
    Logout,
    Reboot,
    Poweroff,
}

impl SessionAction {
    pub fn question(self) -> &'static str {
        match self {
            SessionAction::Lock => "Lock the screen?",
            SessionAction::Suspend => "Suspend the machine?",
            SessionAction::Hibernate => "Hibernate the machine?",
            SessionAction::Logout => "Log out of the session?",
            SessionAction::Reboot => "Reboot the machine?",
            SessionAction::Poweroff => "Shut the machine down?",
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            SessionAction::Lock => "Lock",
            SessionAction::Suspend => "Suspend",
            SessionAction::Hibernate => "Hibernate",
            SessionAction::Logout => "Log out",
            SessionAction::Reboot => "Reboot",
            SessionAction::Poweroff => "Shut down",
        }
    }

    pub fn ends_session(self) -> bool {
        matches!(
            self,
            SessionAction::Logout | SessionAction::Reboot | SessionAction::Poweroff
        )
    }
}

/// What choosing a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Run {
        program: Program,
        args: &'static [Arg],
    },
    Password,
    Ask,
    Power,
    Session(SessionAction),
}

/// A row of the root menu (or of the palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// Stable, unique, and the frecency key: renaming a label keeps its history.
    pub key: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub glyph: Glyph,
    pub group: Group,
    pub action: Action,
}

const fn session(name: &'static str) -> Action {
    Action::Run {
        program: Program::Session(name),
        args: &[],
    }
}

const fn item(
    key: &'static str,
    label: &'static str,
    detail: &'static str,
    glyph: Glyph,
    group: Group,
    action: Action,
) -> MenuItem {
    MenuItem {
        key,
        label,
        detail,
        glyph,
        group,
        action,
    }
}

use Group::{Ai, Capture, Session, System, Utilities, Workspace};

/// The curated, zero-overlap `Alt+Space` root menu.
pub const ROOT: &[MenuItem] = &[
    // ── Workspace ──────────────────────────────────────────────────────────
    item(
        "terminal",
        "Terminal",
        "foot",
        Glyph::Terminal,
        Workspace,
        Action::Run {
            program: Program::System("foot"),
            args: &[],
        },
    ),
    item(
        "files",
        "Files",
        "browse folders with preview",
        Glyph::Folder,
        Workspace,
        session("hayato-files"),
    ),
    item(
        "files-window",
        "Files (window)",
        "folders, drag and drop, trash, devices",
        Glyph::Window,
        Workspace,
        Action::Run {
            program: Program::System("hayato-filer"),
            args: &[Arg::Home],
        },
    ),
    item(
        "notes",
        "Notes",
        "quick notes, ink notes, sign a PDF",
        Glyph::File,
        Workspace,
        session("hayato-notes"),
    ),
    item(
        "grep",
        "Search inside files",
        "full-text ripgrep over home",
        Glyph::Search,
        Workspace,
        session("hayato-grep"),
    ),
    item(
        "places",
        "Places & Drives",
        "bookmarks, USB/SD mount, eject, format",
        Glyph::Disk,
        Workspace,
        session("hayato-places"),
    ),
    item(
        "download",
        "Download",
        "video or audio link → ~/Downloads",
        Glyph::Download,
        Workspace,
        session("hayato-download"),
    ),
    item(
        "cassette",
        "Cassette",
        "music player: local library & Spotify",
        Glyph::Music,
        Workspace,
        Action::Run {
            program: Program::System("hayato-cassette"),
            args: &[Arg::Lit("--toggle")],
        },
    ),
    item(
        "games",
        "Games",
        "retro ROMs & Hayato games",
        Glyph::Play,
        Workspace,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("menu"), Arg::Lit("games")],
        },
    ),
    // ── Create, Capture & Share ────────────────────────────────────────────
    item(
        "capture",
        "Screenshot",
        "snap screen, area or window, annotate",
        Glyph::Capture,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("capture")],
        },
    ),
    item(
        "studio",
        "Studio",
        "screen record, webcam video & photo snap",
        Glyph::Video,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("studio")],
        },
    ),
    item(
        "lens",
        "Lens",
        "read text (OCR), scan QR/link or translate",
        Glyph::Eye,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("lens"), Arg::Lit("--detach")],
        },
    ),
    item(
        "share",
        "Share",
        "send/receive files, links, clipboard & Wi-Fi QR",
        Glyph::Qr,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("share")],
        },
    ),
    // ── Utilities ──────────────────────────────────────────────────────────
    item(
        "clip",
        "Clipboard",
        "paste history & saved images",
        Glyph::Clipboard,
        Utilities,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("clip")],
        },
    ),
    item(
        "calc",
        "Calculator",
        "live math, Enter copies the result",
        Glyph::Calculator,
        Utilities,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("calc")],
        },
    ),
    item(
        "emoji",
        "Emoji",
        "type a name, insert the glyph",
        Glyph::Smile,
        Utilities,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("emoji")],
        },
    ),
    item(
        "colour",
        "Colour",
        "pick a pixel hex & inspect palette",
        Glyph::Brush,
        Utilities,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("colour")],
        },
    ),
    item(
        "remind",
        "Reminders",
        "write it now, be told later",
        Glyph::Clock,
        Utilities,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("remind")],
        },
    ),
    item(
        "password",
        "Password",
        "32 random characters → clipboard",
        Glyph::Key,
        Utilities,
        Action::Password,
    ),
    // ── AI ─────────────────────────────────────────────────────────────────
    item(
        "agent",
        "Agent session",
        "the default agent, in ~/Work",
        Glyph::Terminal,
        Ai,
        Action::Run {
            program: Program::Session("hayato-agents"),
            args: &[Arg::Lit("--default")],
        },
    ),
    item(
        "ask",
        "Ask",
        "one question, the answer here",
        Glyph::Lightbulb,
        Ai,
        Action::Ask,
    ),
    item(
        "agents",
        "Agents",
        "Claude Code, Codex and the others",
        Glyph::People,
        Ai,
        session("hayato-agents"),
    ),
    item(
        "dictate",
        "Dictation",
        "speak, and it is typed",
        Glyph::Mic,
        Ai,
        session("dictate"),
    ),
    item(
        "model",
        "Local model",
        "a 1.7B model, offline, on four cores",
        Glyph::Chip,
        Ai,
        session("hayato-model"),
    ),
    // ── System ─────────────────────────────────────────────────────────────
    item(
        "settings",
        "Settings",
        "network, display, input, power, system",
        Glyph::Gear,
        System,
        session("hayato-settings"),
    ),
    item(
        "wifi",
        "Wi-Fi",
        "pick a network",
        Glyph::Wifi,
        System,
        session("bar-net"),
    ),
    item(
        "bt",
        "Bluetooth",
        "scan, connect, disconnect",
        Glyph::Bluetooth,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("bluetooth")],
        },
    ),
    item(
        "audio",
        "Audio output",
        "switch sink, mute, app mixer",
        Glyph::Volume,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("audio")],
        },
    ),
    item(
        "theme",
        "Themes",
        "browse and preview palettes & wallpapers",
        Glyph::Theme,
        System,
        session("hayato-theme-picker"),
    ),
    item(
        "notify",
        "Notifications",
        "history and do-not-disturb",
        Glyph::Bell,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("notify")],
        },
    ),
    item(
        "pkg",
        "Packages",
        "search, then install via apt",
        Glyph::Box,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("pkg")],
        },
    ),
    item(
        "dev",
        "Developer",
        "profile export/restore & diagnostics",
        Glyph::Database,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("dev")],
        },
    ),
    item(
        "learn",
        "Learn",
        "the manual, with search",
        Glyph::Book,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("learn")],
        },
    ),
    item(
        "keys",
        "Keybindings",
        "every binding, from the configuration",
        Glyph::Keyboard,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("keys")],
        },
    ),
    item(
        "about",
        "About",
        "kernel, uptime, memory, battery",
        Glyph::Info,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("about")],
        },
    ),
    // ── Session ────────────────────────────────────────────────────────────
    item(
        "lock",
        "Lock",
        "the session lock",
        Glyph::Lock,
        Session,
        session("sway-lock"),
    ),
    item(
        "screensaver",
        "Screensaver",
        "the idle screen, now",
        Glyph::Display,
        Session,
        session("hayato-screensaver"),
    ),
    item(
        "power",
        "Power",
        "suspend, hibernate, log out, reboot, shut down",
        Glyph::Power,
        Session,
        Action::Power,
    ),
];

/// The Power row's list: three immediate actions, then three confirmed session-enders.
pub const POWER: &[MenuItem] = &[
    item(
        "power-lock",
        "Lock",
        "the session lock, now",
        Glyph::Lock,
        Session,
        Action::Session(SessionAction::Lock),
    ),
    item(
        "power-suspend",
        "Suspend",
        "lock, then sleep",
        Glyph::Moon,
        Session,
        Action::Session(SessionAction::Suspend),
    ),
    item(
        "power-hibernate",
        "Hibernate",
        "to a disk swap, if there is one",
        Glyph::Save,
        Session,
        Action::Session(SessionAction::Hibernate),
    ),
    item(
        "logout",
        "Log out",
        "back to the greeter",
        Glyph::Logout,
        Session,
        Action::Session(SessionAction::Logout),
    ),
    item(
        "reboot",
        "Reboot",
        "asks first",
        Glyph::Reboot,
        Session,
        Action::Session(SessionAction::Reboot),
    ),
    item(
        "poweroff",
        "Shut down",
        "asks first",
        Glyph::Power,
        Session,
        Action::Session(SessionAction::Poweroff),
    ),
];

/// Granular, power-user, and compatibility rows exposed in `hayato menu palette`
/// so nothing is ever lost while keeping `Alt+Space` crisp and zero-overlap.
pub const PALETTE_EXTRA: &[MenuItem] = &[
    item(
        "eject",
        "Eject drive",
        "safely unmount a USB stick or SD card",
        Glyph::Undo,
        Workspace,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("menu"), Arg::Lit("places"), Arg::Lit("--eject")],
        },
    ),
    item(
        "apps",
        "Applications",
        "desktop entries, icons, frecency (Alt+D)",
        Glyph::Desktop,
        Workspace,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("apps")],
        },
    ),
    item(
        "window",
        "Window switcher",
        "focus or close any window (Alt+Tab)",
        Glyph::Window,
        Workspace,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("window")],
        },
    ),
    item(
        "run",
        "Run command",
        "PATH completion, Shift+Enter in foot (Alt+Shift+D)",
        Glyph::Terminal,
        Workspace,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("run")],
        },
    ),
    item(
        "camera",
        "Camera (Studio Photo)",
        "open Studio directly in webcam photo mode",
        Glyph::Camera,
        Capture,
        session("hayato-camera"),
    ),
    item(
        "drop",
        "Drop (Receive from Phone)",
        "open Share directly in Receive mode",
        Glyph::Download,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("drop")],
        },
    ),
    item(
        "qr",
        "QR code generator",
        "Wi-Fi network or custom text QR",
        Glyph::Qr,
        Capture,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("qr")],
        },
    ),
    item(
        "media",
        "Media controls (MPRIS)",
        "play, pause, next, copy track",
        Glyph::Music,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("media")],
        },
    ),
    item(
        "vnc",
        "Remote Desktop (VNC)",
        "read-only or remote control over SSH",
        Glyph::Eye,
        System,
        session("hayato-vnc"),
    ),
    item(
        "ssh",
        "SSH",
        "remote login status",
        Glyph::Server,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("ssh")],
        },
    ),
    item(
        "dns",
        "DNS",
        "the resolver this connection uses",
        Glyph::Cloud,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("dns")],
        },
    ),
    item(
        "tz",
        "Time zone",
        "and network time",
        Glyph::Clock,
        System,
        session("hayato-tz"),
    ),
    item(
        "display",
        "Display arrangement",
        "the external monitor",
        Glyph::Display,
        System,
        session("hayato-display"),
    ),
    item(
        "clean",
        "Free space",
        "what is using it, and clean up",
        Glyph::Trash,
        System,
        session("hayato-clean"),
    ),
    item(
        "swap",
        "Auxiliary swap",
        "a card as more memory, on every boot",
        Glyph::Chip,
        System,
        session("hayato-swap"),
    ),
    item(
        "format",
        "Format a disk",
        "erase a stick or a card",
        Glyph::Disk,
        System,
        session("hayato-format"),
    ),
    item(
        "install",
        "Install Hayato",
        "copy this system to the internal disk - asks twice",
        Glyph::Save,
        System,
        session("hayato-installer"),
    ),
    item(
        "kill",
        "End a process",
        "the heaviest twenty",
        Glyph::Bug,
        System,
        session("hayato-kill"),
    ),
    item(
        "theme-picker",
        "Theme Selector",
        "browse themes with live palette & wallpaper preview",
        Glyph::Theme,
        System,
        session("hayato-theme-picker"),
    ),
    item(
        "theme-next",
        "Next theme",
        "the palette after this one (Alt+Shift+T)",
        Glyph::Theme,
        System,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("theme"), Arg::Lit("next")],
        },
    ),
    item(
        "wallpaper-next",
        "Next wallpaper",
        "of the active theme (Alt+Shift+W)",
        Glyph::Image,
        System,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("theme"), Arg::Lit("wallpaper"), Arg::Lit("next")],
        },
    ),
    item(
        "setup",
        "Setup Assistant",
        "guided 8-step setup: security, network, hardware, software, privacy, backup",
        Glyph::Sparkle,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("setup")],
        },
    ),
    item(
        "vpn",
        "VPN",
        "WireGuard & OpenVPN tunnels: import, connect, autoconnect",
        Glyph::Shield,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("vpn")],
        },
    ),
    item(
        "greeter-settings",
        "Greeter settings",
        "login defaults, user pills, clock, sessions",
        Glyph::Gear,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("greeter")],
        },
    ),
    item(
        "suspend",
        "Suspend",
        "sleep until asked",
        Glyph::Pause,
        Session,
        session("sway-suspend"),
    ),
    item(
        "hibernate",
        "Hibernate",
        "needs a disk swap and resume=",
        Glyph::Moon,
        Session,
        session("sway-hibernate"),
    ),
];

/// Every legacy `bar-menu` row key; each must remain reachable in `ROOT` or `PALETTE_EXTRA`.
pub const BAR_MENU_KEYS: &[&str] = &[
    "terminal",
    "files",
    "files-window",
    "grep",
    "places",
    "camera",
    "download",
    "settings",
    "learn",
    "keys",
    "apps",
    "window",
    "run",
    "calc",
    "emoji",
    "clip",
    "password",
    "ssh",
    "agent",
    "agents",
    "dictate",
    "model",
    "wifi",
    "dns",
    "bt",
    "audio",
    "media",
    "notify",
    "vnc",
    "tz",
    "clean",
    "swap",
    "install",
    "kill",
    "pkg",
    "dev",
    "capture",
    "qr",
    "colour",
    "about",
    "remind",
    "lock",
    "screensaver",
    "suspend",
    "hibernate",
    "power",
];
