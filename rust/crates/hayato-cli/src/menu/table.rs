//! The root menu as data (spec 007).
//!
//! Every row of the old `bar-menu` is a [`MenuItem`] here, in the same groups
//! and the same order, and every row's action is an [`Action`] value: a
//! program and an argument list, or one of the few things the menu does
//! itself. There is no shell anywhere in this file, which is the point - a row
//! cannot invent a command line, so a label, a detail or a future row from a
//! data file can never become `; rm -rf ~` (`action_never_contains_a_shell_metacharacter`).
//!
//! `check-system.sh` compares the keys below with the rows `bar-menu` had
//! (`hayato menu --rows`), so a row that goes missing is a failed check, not a
//! documented keypress that answers nothing.

use hayato_ui::Glyph;

/// The menu's groups, in the order they are drawn. `Open` is the unnamed first
/// block of `bar-menu` (what opens something); the others are its headings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Open,
    Ai,
    System,
    Session,
}

impl Group {
    /// The word the row shows in its value column, which is also something a
    /// person can type: `system` narrows the menu to the system group.
    pub fn label(self) -> &'static str {
        match self {
            Group::Open => "open",
            Group::Ai => "ai",
            Group::System => "system",
            Group::Session => "session",
        }
    }

    /// The section heading above the group. The row's value column keeps the
    /// lowercase word a person can type; the heading is the name a person reads.
    pub fn heading(self) -> &'static str {
        match self {
            Group::Open => "Open",
            Group::Ai => "AI",
            Group::System => "System",
            Group::Session => "Session",
        }
    }

    /// The order the headings appear in, so a surface that appends rows can
    /// keep each group contiguous (the palette appends themes to `System`).
    pub fn order(self) -> u8 {
        match self {
            Group::Open => 0,
            Group::Ai => 1,
            Group::System => 2,
            Group::Session => 3,
        }
    }
}

/// One argument of a program a row runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    Lit(&'static str),
    /// The person's home directory, filled in when the row runs. It is a value
    /// and not `"$HOME"`, because nothing here expands a variable.
    Home,
}

/// Where a row's program comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    /// A session script, run from `~/.config/bin` by its full path - the same
    /// file `bar-menu` ran with `exec "$dir/…"`, so a script that is a symlink
    /// to `hayato` today and a shell script tomorrow is the same row.
    Session(&'static str),
    /// A program from `PATH` (`foot`, `hayato`, `favnyr`).
    System(&'static str),
    /// This binary with these words (`hayato theme next`), by the path the
    /// process was started from rather than by name.
    Hayato,
}

/// A thing the Power list does to the session.
///
/// The last three end it, and they are only reachable through a
/// [`hayato_ui::Confirm`] whose first answer is Cancel (spec 007: a stray Enter
/// runs nothing). [`Lock`](Self::Lock), [`Suspend`](Self::Suspend) and
/// [`Hibernate`](Self::Hibernate) act on Enter, the way `hayato-power` ran them:
/// the list is already the gate, and the lock is what makes a suspend safe.
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
    /// The question a session-ending row asks. Only those three reach a
    /// confirm; the rest answer something true for the same list.
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

    /// The answer that does it, after `Cancel`.
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

    /// Whether the row ends the session, i.e. whether it needs the confirm.
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
    /// Start a program detached from the window, which closes.
    Run {
        program: Program,
        args: &'static [Arg],
    },
    /// 32 random hex characters onto the clipboard through the secret card
    /// (spec 006), so the value never reaches a notification.
    Password,
    /// The Ask strip: a question, in this window.
    Ask,
    /// The three session-ending rows, in this window, each behind a confirm.
    Power,
    /// One of those three; constructed only by the power list's confirm.
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

use Group::{Ai, Open, Session, System};

/// The root menu: `bar-menu`'s rows, groups and order, plus the Ask row.
pub const ROOT: &[MenuItem] = &[
    item(
        "terminal",
        "Terminal",
        "foot",
        Glyph::Terminal,
        Open,
        Action::Run {
            program: Program::System("foot"),
            args: &[],
        },
    ),
    item(
        "files",
        "Files",
        "browse the filesystem",
        Glyph::Folder,
        Open,
        session("hayato-files"),
    ),
    item(
        "files-window",
        "Files (window)",
        "favnyr: folders, drag and drop, trash",
        Glyph::Window,
        Open,
        Action::Run {
            program: Program::System("favnyr"),
            args: &[Arg::Home],
        },
    ),
    item(
        "notes",
        "Notes",
        "quick notes, ink notes, sign a PDF",
        Glyph::File,
        Open,
        session("hayato-notes"),
    ),
    item(
        "grep",
        "Search inside files",
        "the text in them, with ripgrep",
        Glyph::Search,
        Open,
        session("hayato-grep"),
    ),
    item(
        "places",
        "Places",
        "home, disks, the configuration",
        Glyph::Disk,
        Open,
        session("hayato-places"),
    ),
    item(
        "eject",
        "Eject drive",
        "safely unmount a USB stick or SD card",
        Glyph::Undo,
        Open,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("menu"), Arg::Lit("places"), Arg::Lit("--eject")],
        },
    ),
    item(
        "camera",
        "Camera",
        "look through the webcam, take a photo",
        Glyph::Camera,
        Open,
        session("hayato-camera"),
    ),
    item(
        "games",
        "Games",
        "retro and Hayato games",
        Glyph::Play,
        Open,
        Action::Run {
            program: Program::Hayato,
            args: &[Arg::Lit("menu"), Arg::Lit("games")],
        },
    ),
    item(
        "download",
        "Download",
        "a link, into ~/Downloads",
        Glyph::Download,
        Open,
        session("hayato-download"),
    ),
    item(
        "settings",
        "Settings",
        "Wi-Fi, Bluetooth, night light, tablet mode, rotation",
        Glyph::Gear,
        Open,
        session("hayato-settings"),
    ),
    item(
        "learn",
        "Learn",
        "the manual, with search",
        Glyph::Book,
        Open,
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
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("keys")],
        },
    ),
    item(
        "apps",
        "Applications",
        "desktop entries, icons, frecency",
        Glyph::Desktop,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("apps")],
        },
    ),
    item(
        "window",
        "Window switcher",
        "focus or close any window",
        Glyph::Window,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("window")],
        },
    ),
    item(
        "run",
        "Run command",
        "PATH completion, Shift+Enter in foot",
        Glyph::Terminal,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("run")],
        },
    ),
    item(
        "calc",
        "Calculator",
        "type it, Enter copies the result",
        Glyph::Calculator,
        Open,
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
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("emoji")],
        },
    ),
    item(
        "clip",
        "Clipboard",
        "what was copied, back in one pick",
        Glyph::Clipboard,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("clip")],
        },
    ),
    item(
        "lens",
        "Lens",
        "capture screen region to text, link, translate, or note",
        Glyph::Eye,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("lens")],
        },
    ),
    item(
        "drop",
        "Drop",
        "share or receive files over Wi-Fi via QR code",
        Glyph::Qr,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("drop")],
        },
    ),
    item(
        "password",
        "Password",
        "32 random characters, into the clipboard",
        Glyph::Key,
        Open,
        Action::Password,
    ),
    item(
        "ssh",
        "SSH",
        "remote login status",
        Glyph::Server,
        Open,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("ssh")],
        },
    ),
    // The AI group opens with the two actions an agent is for: a session, and
    // one question. Both follow the default the Agents menu marks with `●`.
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
    // The model that runs here (spec 016): no network, no account, and the
    // flags it was measured into. Alt+Ctrl+M opens the same window.
    item(
        "model",
        "Local model",
        "a 1.7B model, offline, on four cores",
        Glyph::Chip,
        Ai,
        session("hayato-model"),
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
        "switch sink, mute, pavucontrol",
        Glyph::Volume,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("audio")],
        },
    ),
    item(
        "media",
        "Media",
        "play, pause, next, copy track",
        Glyph::Music,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("media")],
        },
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
        "vnc",
        "Share this screen",
        "read-only, over ssh",
        Glyph::Eye,
        System,
        session("hayato-vnc"),
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
        "display",
        "Display arrangement",
        "the external monitor",
        Glyph::Display,
        System,
        session("hayato-display"),
    ),
    // The one row here that rewrites a disk. Like Power it asks again where it
    // lands: `hayato-installer` confirms and then wants the disk's name typed.
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
        "pkg",
        "Packages",
        "search, then install one",
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
        "profile export and restore, diagnostics",
        Glyph::Database,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("dev")],
        },
    ),
    item(
        "capture",
        "Capture",
        "screenshot, annotate, OCR, QR, record",
        Glyph::Capture,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("capture")],
        },
    ),
    item(
        "qr",
        "QR code",
        "Wi-Fi network or any text",
        Glyph::Qr,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("qr")],
        },
    ),
    item(
        "colour",
        "Colour",
        "pick a pixel, see the palette",
        Glyph::Brush,
        System,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("colour")],
        },
    ),
    item(
        "theme",
        "Themes",
        "browse and preview color palettes and wallpapers",
        Glyph::Theme,
        System,
        session("hayato-theme-picker"),
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
    item(
        "remind",
        "Reminders",
        "write it now, be told later",
        Glyph::Clock,
        Session,
        Action::Run {
            program: Program::System("hayato"),
            args: &[Arg::Lit("menu"), Arg::Lit("remind")],
        },
    ),
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
    // Log out, reboot and shut down are one row that opens their own list,
    // each behind a confirm: a second, unconfirmed copy here is how a stray
    // Enter reboots the machine (`bar-menu` said the same about hayato-power).
    item(
        "power",
        "Power",
        "log out, reboot, shut down - each asks first",
        Glyph::Power,
        Session,
        Action::Power,
    ),
];

/// The Power row's list, in `hayato-power`'s order: the three that act at once,
/// then the three that ask. Choosing a session-ending row opens the confirm;
/// the confirm's second answer is the only way to its [`Action::Session`].
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

/// The palette's own rows, beyond every root-menu row: the framework commands
/// that run without a question (the theme keys) and one row per built-in theme
/// (built at run time from `hayato_theme::builtin`). The `.desktop` launcher
/// entries join in spec 012, which owns their parsing and launch path.
pub const PALETTE_EXTRA: &[MenuItem] = &[
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
];

/// The `bar-menu` row keys, verbatim from the script this menu replaces (its
/// last version, before it became a symlink). The table must keep every one:
/// `the_root_menu_keeps_every_bar_menu_row` compares them, and
/// `check-system.sh` compares the same list with `hayato menu --rows` on the
/// device.
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
