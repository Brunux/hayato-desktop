//! The command table and the two ways a name reaches a command.
//!
//! Every surface the framework grows is a row here, and the row is what
//! `--help`, `--symlinks` and the `check-system.sh` witness read: one source of
//! truth, so a command that exists but is not listed (or a symlink that points
//! nowhere) is a test failure rather than a customer finding.
//!
//! Resolution has two entry points, and they must agree:
//!
//! - `hayato menu files` - the words after the binary name, longest match first,
//!   so `menu` does not swallow `menu files`.
//! - a symlink called `hayato-files` (or `hayato-menu`) - the file name, which is
//!   how the ~64 old paths keep working once each surface is ported.

use std::path::Path;

/// The window shape a command asks the host for (plan-ui §3).
///
/// The shape is data in the table, not a decision each command makes: the host
/// derives the `foot` app-id and the floating geometry from it, and sway's rules
/// can key on the same three names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A floating, centred menu sized for a list.
    Pick,
    /// A floating, centred window sized for a task (log, gauge, progress).
    Task,
    /// A larger floating, centred workspace window (live QR deck, multi-pane studio).
    Wide,
    /// Fullscreen (the installer, the greeter).
    Full,
    /// No window at all: a command whose whole job is to write a file and
    /// print a line (`theme set`). A menu still gets a canvas; this one would
    /// be a terminal that opens and closes around a `mv`.
    Direct,
}

impl Shape {
    /// The app-id suffix (`foot --app-id=hayato-pick`). This is the name
    /// `config.d/` and `swaymsg` criteria use.
    pub fn slug(self) -> &'static str {
        match self {
            Shape::Pick => "pick",
            Shape::Task => "task",
            Shape::Wide => "wide",
            Shape::Full => "full",
            Shape::Direct => "direct",
        }
    }

    pub fn app_id(self) -> String {
        format!("hayato-{}", self.slug())
    }

    /// The shape's size hints, for the document that owns window geometry
    /// (`uis/sway/files/etc/sway/config.d/18-framework-windows.conf`). Sway
    /// applies them when it maps the surface; a `swaymsg` from this process
    /// would race that mapping (measured: the child runs before foot's first
    /// commit, so the criteria matched no node and the window stayed tiled).
    pub fn size(self) -> Option<(u16, u16)> {
        match self {
            Shape::Pick => Some((900, 600)),
            Shape::Task => Some((940, 620)),
            Shape::Wide => Some((1120, 780)),
            Shape::Full | Shape::Direct => None,
        }
    }
}

/// One row of the table.
#[derive(Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// The canonical words, as `hayato <path>` spells them.
    pub path: &'static str,
    /// The one line `--help` prints.
    pub summary: &'static str,
    pub shape: Shape,
    /// Every name that resolves to this command, including ones whose old
    /// script still owns the name (they are installed by the spec that ports
    /// it, and `--symlinks` reports only the installed ones).
    pub symlinks: &'static [&'static str],
    /// The subset of `symlinks` that resolves to the binary on an image
    /// **today**: a link in `/usr/bin` the installer creates, or one in
    /// `~/.config/bin` the UI tree carries in place of the script it replaced.
    /// A name whose surface has not been ported stays out of this list so the
    /// old script keeps working; `check-system.sh` validates exactly this set.
    pub installed: &'static [&'static str],
}

/// The table. Adding a command here is what makes it callable by name, by
/// symlink and visible in `--help` at once.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        // The root menu (spec 007). `bar-menu` is the name Alt+Space and the
        // bar's menu button have always run; it is a link to this binary in the
        // UI tree (~/.config/bin), so the binding and the bar did not change.
        path: "menu",
        summary: "the session menu: open, ai, system, session (spec 007)",
        shape: Shape::Pick,
        symlinks: &["hayato-menu", "bar-menu"],
        installed: &["hayato-menu", "bar-menu"],
    },
    CommandSpec {
        // The session's controller mapper: a USB or Bluetooth pad as arrows,
        // Enter and Escape for the framework's own `hayato-*` windows. The
        // Games library reads the pad itself; games keep their own input.
        path: "gamepad",
        summary: "translate a gamepad to arrows, Enter and Escape for the framework windows",
        shape: Shape::Direct,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // `hayato-power`: lock, suspend, hibernate, log out, reboot, shut down.
        // The first three act at once and the last three ask with Cancel
        // first, which is the confirm `--power ACTION` also opens (spec 010).
        // It stays one implementation: `menu power`, `menu --power` and
        // `menu --power ACTION` are the same screen, entered at a different
        // row.
        path: "menu power",
        summary: "lock, sleep, hibernate, log out, reboot or shut down (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-power"],
        installed: &["hayato-power"],
    },
    CommandSpec {
        // `hayato-format`: only disks with nothing mounted, never a built-in one
        // and never the disk this system runs from. The list is the safety;
        // the confirm with Cancel selected is the last gate (spec 010).
        path: "menu format",
        summary: "erase a stick or a card and make it one filesystem (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-format"],
        installed: &["hayato-format"],
    },
    CommandSpec {
        // `hayato-swap`: the same candidate list as Format a disk, then the size
        // and a confirm. The fstab block is replaced, not appended, and
        // activation is verified by reading the kernel's swap list back.
        path: "menu swap",
        summary: "prepare a card as auxiliary swap, label hayato_swap (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-swap"],
        installed: &["hayato-swap"],
    },
    CommandSpec {
        // `hayato-display`: the external monitor arrangement (position, resolution,
        // scale), saved to config.d/40-display.conf, with wdisplays as a row and
        // --panel-on as a blind key (spec 010).
        path: "menu display",
        summary: "arrange the external monitor: position, resolution, scale (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-display"],
        installed: &["hayato-display"],
    },
    CommandSpec {
        // `hayato-tz`: timedatectl list-timezones (400-odd rows, filtered) plus the
        // network-time switch. Restarts waybar so the bar clock updates.
        path: "menu tz",
        summary: "set the time zone and network time sync (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-tz"],
        installed: &["hayato-tz"],
    },
    CommandSpec {
        // `hayato-dns`: point the active connection at a resolver (Cloudflare,
        // Google, Quad9, AdGuard) or back at DHCP. The change is applied by
        // modifying the connection and bringing it up again; the read-back
        // shows what NetworkManager is actually using.
        path: "menu dns",
        summary: "set DNS resolvers for the active connection (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-dns"],
        installed: &["hayato-dns"],
    },
    CommandSpec {
        // `hayato-vnc`: wayvnc on loopback, read-only by default and with
        // remote control as the explicit second start, nothing resident. The
        // tunnel command is copied for the other machine. The bar's icon is a
        // state file, not a process.
        path: "menu vnc",
        summary: "share this screen read-only or with remote control over ssh (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-vnc"],
        installed: &["hayato-vnc"],
    },
    CommandSpec {
        // `hayato-settings`: the radio switches (Wi-Fi, Bluetooth, night light,
        // DND, touchpad, tablet mode, rotation, automount, audio, media),
        // each showing real state from the same probes as today. Delegates to
        // the existing pickers/panels (bar-net, bar-bt, bar-audio, bar-power,
        // hayato-display, hayato-vnc, etc.) instead of duplicating them.
        path: "menu settings",
        summary: "quick settings: Wi-Fi, BT, night light, DND, touchpad, tablet mode, display, automount, keyboard, translate, audio, media, battery (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-settings"],
        installed: &["hayato-settings"],
    },
    CommandSpec {
        // `hayato-ssh`: remote login status (ssh.service enabled/running).
        // The image runs openssh-server with ssh.service enabled on every boot.
        // This menu shows the status and the commands to change it.
        path: "menu ssh",
        summary: "remote login status: ssh.service enabled/running (spec 010)",
        shape: Shape::Pick,
        symlinks: &["hayato-ssh"],
        installed: &["hayato-ssh"],
    },
    CommandSpec {
        // Every row the menu has, plus the commands that need no question and
        // the built-in themes, under the same ranking. Spec 012 adds the
        // `.desktop` entries and gives it a key.
        path: "menu palette",
        summary: "the command palette: every row, ranked as you type (spec 007)",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The Ask strip on its own (the launcher entry): the prompt, with the
        // words after `ask` already typed.
        path: "ask",
        summary: "one question to the default agent, answered in the window (spec 007)",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The palette. `theme set` writes files and prints what it did, so it
        // never opens a window: the cycle keys run it from the session and a
        // window would steal the focus for a `mv`.
        path: "theme",
        summary: "list, set or check the session palette (spec 004)",
        shape: Shape::Direct,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // A card from the command line (spec 006). It writes nothing and opens
        // no window: the notification daemon shows it, so it runs where it was
        // called, like `theme`.
        path: "notify",
        summary: "send a notification card: send, say, progress (spec 006)",
        shape: Shape::Direct,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The Quake console (Alt+`): a terminal that drops from the top of the
        // screen, holding the default agent. Sway's scratchpad keeps the
        // window alive between toggles, so this command opens no window of its
        // own and has nothing to draw.
        path: "quake",
        summary: "toggle the drop-down agent console (Alt+`)",
        shape: Shape::Direct,
        symlinks: &["hayato-quake"],
        installed: &["hayato-quake"],
    },
    CommandSpec {
        // Notes and documents: quick Markdown notes in ~/Documents/Notes,
        // ink notes and PDF signing through xournalpp. EPUB stays in Files
        // (foliate), because reading is not note-taking.
        path: "menu notes",
        summary: "quick notes, ink notes and PDF signing (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-notes"],
        installed: &["hayato-notes"],
    },
    CommandSpec {
        // The file list (spec 009). `hayato-files` is a link to this binary in
        // the UI tree, so Alt+Ctrl+F, Alt+Search (`--search`), the launcher
        // entry and the scripts that open a folder did not change.
        path: "menu files",
        summary: "browse a folder with a preview, or --search the newest files (spec 009)",
        shape: Shape::Pick,
        symlinks: &["hayato-files"],
        installed: &["hayato-files"],
    },
    CommandSpec {
        // The uncapped walk with a preview (Search+Shift), where fzf was.
        path: "menu find",
        summary: "every file under a folder, filtered while it walks (spec 009)",
        shape: Shape::Pick,
        symlinks: &["hayato-find"],
        installed: &["hayato-find"],
    },
    CommandSpec {
        // ripgrep over the home folder (Alt+Ctrl+G); a match opens at its line.
        path: "menu grep",
        summary: "search inside the files; a match opens at its line (spec 009)",
        shape: Shape::Pick,
        symlinks: &["hayato-grep"],
        installed: &["hayato-grep"],
    },
    CommandSpec {
        // The opener table applied to one path: what a double click in favnyr
        // (org.hayato.view.desktop), xdg-open and every list run. It starts
        // the viewer directly when ready, or launches `menu open` for intents.
        path: "open",
        summary: "open a file with the program that fits it (spec 009)",
        shape: Shape::Direct,
        symlinks: &["hayato-view"],
        installed: &["hayato-view"],
    },
    CommandSpec {
        path: "menu open",
        summary: "smart intent router for installers, archives, and unhandled files",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The desktop as MCP tools for an agent (spec 008): a stdio server an
        // OpenCode session starts and ends, so it opens no window and keeps
        // nothing running. `--tools` prints the manifest check-system reads.
        path: "mcp",
        summary: "the desktop's tools for an agent, over stdio; --tools prints the manifest (spec 008)",
        shape: Shape::Direct,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // Ask a running task to stop, the way its window's `s` does: only a
        // task whose record says it is stoppable (spec 005), and never by
        // signalling a process this command does not own.
        path: "task stop",
        summary: "ask a running, stoppable task to stop (spec 005/008)",
        shape: Shape::Direct,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The task surface: reattach to a slow action that outlives a toast.
        path: "task attach",
        summary: "reattach to a running or finished task (spec 005)",
        shape: Shape::Task,
        symlinks: &["hayato-task"],
        installed: &["hayato-task"],
    },
    CommandSpec {
        // The game library: a Ratatui list over ~/Games and the Hayato HTML
        // games. `hayato-games` is only the GTK + WebKit host for one web game.
        path: "menu games",
        summary: "the game library: retro ROMs and Hayato HTML games",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // The controller: what each pad button does, saved where the mapper
        // reads it. The row is also reachable from Settings → Input.
        path: "menu controller",
        summary: "the gamepad: what each button does, and how a pad pairs",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // Media: player state and transport (spec 011).
        path: "menu media",
        summary: "what is playing, and the transport keys (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-media"],
        installed: &["hayato-media"],
    },
    CommandSpec {
        // Notify history: mako's history with dismiss/clear (spec 011).
        path: "menu notify",
        summary: "the notification history and do-not-disturb (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-notify-history"],
        installed: &["hayato-notify-history"],
    },
    CommandSpec {
        // Clipboard: cliphist rows, copy back, save image, clear (spec 011).
        path: "menu clip",
        summary: "clipboard history by kind, copy back, save image, clear (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-clip"],
        installed: &["hayato-clip"],
    },
    CommandSpec {
        // Emoji: the emojis.tsv table, insert through wtype (spec 011).
        path: "menu emoji",
        summary: "the emoji table, fuzzy filter, insert at cursor (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-emoji"],
        installed: &["hayato-emoji"],
    },
    CommandSpec {
        // Capture: screenshots, OCR, QR, recording (spec 011).
        path: "menu capture",
        summary: "screenshot, annotate, OCR, QR, record (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-capture"],
        installed: &["hayato-capture"],
    },
    CommandSpec {
        // Studio: screen recording, webcam video, photo snap, and live HUD.
        path: "menu studio",
        summary: "screen record, webcam video, photo snap, and live HUD",
        shape: Shape::Task,
        symlinks: &["hayato-studio"],
        installed: &["hayato-studio"],
    },
    CommandSpec {
        // QR: Wi-Fi payload or any text (spec 011).
        path: "menu qr",
        summary: "the Wi-Fi QR or any text, rendered in-window (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-qr"],
        installed: &["hayato-qr"],
    },
    CommandSpec {
        // Lens: local Circle-to-Search (OCR + QR + entity actions).
        path: "menu lens",
        summary: "capture screen region to OCR, URL, translate, note, or QR",
        shape: Shape::Pick,
        symlinks: &["hayato-lens"],
        installed: &["hayato-lens"],
    },
    CommandSpec {
        // Drop: ephemeral LAN nearby share via in-TUI QR code.
        path: "menu drop",
        summary: "share or receive files over Wi-Fi via a scannable QR code",
        shape: Shape::Pick,
        symlinks: &["hayato-drop"],
        installed: &["hayato-drop"],
    },
    CommandSpec {
        // Share: OS-wide Share Sheet (Phone QR, LocalSend v2, offline QR, clipboard, notes).
        path: "share",
        summary: "system share sheet: Phone QR, LocalSend, offline QR, clipboard, notes",
        shape: Shape::Pick,
        symlinks: &["hayato-share"],
        installed: &["hayato-share"],
    },
    CommandSpec {
        // Connect: opt-in zero-daemon encrypted phone-to-computer workspace (TLS + WSS).
        path: "connect",
        summary: "encrypted LAN phone-to-computer hub: pairing QR, clipboard, files, links",
        shape: Shape::Wide,
        symlinks: &["hayato-connect"],
        installed: &["hayato-connect"],
    },
    CommandSpec {
        // Remind: title → note → when flow (spec 011).
        path: "menu remind",
        summary: "write it now, be told later, cancel from the list (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-remind"],
        installed: &["hayato-remind"],
    },
    CommandSpec {
        // Calc: expression parser, history, copy (spec 011).
        path: "menu calc",
        summary: "an expression parser, not a dmenu prompt (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-calc"],
        installed: &["hayato-calc"],
    },
    CommandSpec {
        // Colour: the hex of a pixel and the palette (spec 011).
        path: "menu colour",
        summary: "pick a pixel and show the palette (spec 011)",
        shape: Shape::Pick,
        symlinks: &["hayato-color"],
        installed: &["hayato-color"],
    },
    CommandSpec {
        // Audio output picker (spec 011): the bar's sink list.
        path: "menu audio",
        summary: "switch the default sink, mute, and per-app audio mixer (spec 011)",
        shape: Shape::Pick,
        symlinks: &["bar-audio"],
        installed: &["bar-audio"],
    },
    CommandSpec {
        // Bluetooth picker (spec 011): the bar's device list.
        path: "menu bluetooth",
        summary: "scan, connect, disconnect, bluetoothctl (spec 011)",
        shape: Shape::Pick,
        symlinks: &["bar-bt"],
        installed: &["bar-bt"],
    },
    CommandSpec {
        // Launcher (spec 012): .desktop entries with icons, frecency, Terminal=true → foot.
        path: "menu apps",
        summary: "desktop entries with icons, frecency, Terminal=true → foot (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-apps"],
        installed: &["hayato-apps"],
    },
    CommandSpec {
        // Window switcher (spec 012): sway IPC tree, focus/close.
        path: "menu window",
        summary: "focus or close any window, from sway IPC (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-window"],
        installed: &["hayato-window"],
    },
    CommandSpec {
        // Run prompt (spec 012): PATH completion, Shift+Enter → foot.
        path: "menu run",
        summary: "PATH commands with completion, Shift+Enter in foot (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-run"],
        installed: &["hayato-run"],
    },
    CommandSpec {
        // Learn: manual pages + apropos (spec 012).
        path: "menu learn",
        summary: "the manual pages and apropos, rendered in the window (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-learn"],
        installed: &["hayato-learn"],
    },
    CommandSpec {
        // Keys: parsed sway config (spec 012).
        path: "menu keys",
        summary: "every binding from the sway config, comments as descriptions (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-keys"],
        installed: &["hayato-keys"],
    },
    CommandSpec {
        // Pkg: apt search + install (spec 012).
        path: "menu pkg",
        summary: "search packages and install with apt (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-pkg"],
        installed: &["hayato-pkg"],
    },
    CommandSpec {
        // Dev: profile export/restore/report (spec 012).
        path: "menu dev",
        summary: "profile export, restore and diagnostics bundle (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-dev"],
        installed: &["hayato-dev"],
    },
    CommandSpec {
        // About: fastfetch facts, revision, logs (spec 012).
        path: "menu about",
        summary: "system facts, revision, errors, boot time, devices (spec 012)",
        shape: Shape::Pick,
        symlinks: &["hayato-about"],
        installed: &["hayato-about"],
    },
    CommandSpec {
        // Wi-Fi network manager (spec 017): scan, signal, connect.
        path: "menu net",
        summary: "scan Wi-Fi networks, signal gauge, connect (spec 017)",
        shape: Shape::Pick,
        symlinks: &["bar-net"],
        installed: &["bar-net"],
    },
    CommandSpec {
        // AI agents selector (spec 017): agents and accounts.
        path: "menu agents",
        summary: "AI agent selector and accounts (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-agents"],
        installed: &["hayato-agents"],
    },
    CommandSpec {
        // Places & bookmarks (spec 017): folders, disks, mount.
        path: "menu places",
        summary: "user directories, mounted and mountable disks (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-places"],
        installed: &["hayato-places"],
    },
    CommandSpec {
        // Disk cleaner (spec 017): apt, journal, thumbnails, trash.
        path: "menu clean",
        summary: "reclaim disk space from caches, trash and journal (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-clean"],
        installed: &["hayato-clean"],
    },
    CommandSpec {
        // Process killer (spec 017): user processes, memory, SIGTERM.
        path: "menu kill",
        summary: "user processes with PID and memory, signal with SIGTERM (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-kill"],
        installed: &["hayato-kill"],
    },
    CommandSpec {
        // Media downloader (spec 017): yt-dlp into ~/Downloads.
        path: "menu download",
        summary: "download video or audio from link with yt-dlp (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-download"],
        installed: &["hayato-download"],
    },
    CommandSpec {
        // Keyboard layout (spec 017): switch layout via Sway IPC.
        path: "menu layout",
        summary: "switch keyboard layout via Sway IPC (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-layout"],
        installed: &["hayato-layout"],
    },
    CommandSpec {
        // Contextual file actions (spec 017): du, pack, unpack, convert.
        path: "menu file-actions",
        summary: "contextual file actions: du, compress, extract, convert (spec 017)",
        shape: Shape::Pick,
        symlinks: &["hayato-file-actions"],
        installed: &["hayato-file-actions"],
    },
    CommandSpec {
        // Translation (spec 017): translate selection or clipboard.
        path: "menu translate",
        summary: "translate selected text or clipboard via Google/MyMemory (spec 017)",
        shape: Shape::Pick,
        symlinks: &["translate"],
        installed: &["translate"],
    },
    CommandSpec {
        // Speech dictation (spec 017): toggle speech-to-text.
        path: "menu dictate",
        summary: "speech-to-text dictation with whisper-cli and wtype (spec 017)",
        shape: Shape::Direct,
        symlinks: &["dictate"],
        installed: &["dictate"],
    },
    CommandSpec {
        // Interactive theme and appearance selector (hayato-theme-picker).
        path: "menu theme",
        summary: "interactive theme and appearance selector with live preview",
        shape: Shape::Task,
        symlinks: &["hayato-theme-picker"],
        installed: &["hayato-theme-picker"],
    },
    CommandSpec {
        // `hayato installer`: the full TUI installer wizard (spec 018).
        // `hayato-install-run` and `hayato-install-log` stay as aliases for the
        // headless halves; the old install menu is the installer itself now.
        path: "installer",
        summary: "install Hayato to internal storage: pre-flight, user, timezone, safety (spec 018)",
        shape: Shape::Full,
        symlinks: &[
            "hayato-installer",
            "hayato-install-run",
            "hayato-install-log",
        ],
        installed: &[
            "hayato-installer",
            "hayato-install-run",
            "hayato-install-log",
        ],
    },
    CommandSpec {
        // `hayato greeter`: themed login greeter over greetd IPC (spec 021).
        path: "greeter",
        summary: "themed login greeter over greetd IPC (spec 021)",
        shape: Shape::Full,
        symlinks: &["hayato-greeter"],
        installed: &["hayato-greeter"],
    },
    CommandSpec {
        // `hayato menu greeter`: configure greeter defaults (spec 021).
        path: "menu greeter",
        summary: "configure greeter defaults: theme, user pills, clock, sessions (spec 021)",
        shape: Shape::Task,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // `hayato menu software`: intent-first Debian APT software catalog,
        // updates, and cleanup (replaces hunting for raw package names).
        path: "menu software",
        summary: "discover apps by task, inspect APT details, updates, and cleanup",
        shape: Shape::Task,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // `hayato menu vpn`: native WireGuard & OpenVPN client over NetworkManager.
        path: "menu vpn",
        summary: "WireGuard & OpenVPN client: import .conf/.ovpn, connect, autoconnect",
        shape: Shape::Pick,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // `hayato menu setup`: 8-step Personal Setup Assistant (optional,
        // recoverable, resumable). Persists progress in
        // `$XDG_STATE_HOME/hayato/setup.json`.
        path: "menu setup",
        summary: "Personal Setup Assistant: 8-step guided first-run and recovery setup",
        shape: Shape::Task,
        symlinks: &[],
        installed: &[],
    },
    CommandSpec {
        // `hayato menu welcome`: launches the Personal Setup Assistant
        // (`--briefing` opens the legacy 4-tab Mission Briefing).
        // Intentionally has no installed symlink because `~/.config/bin/hayato-welcome`
        // is the wrapper script that handles `--print` and the `/etc/machine-id` marker.
        path: "menu welcome",
        summary: "first-boot Personal Setup Assistant (or --briefing for Mission Briefing)",
        shape: Shape::Task,
        symlinks: &[],
        installed: &[],
    },
];

/// A command the table resolved, with the arguments that belong to it.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    pub spec: &'static CommandSpec,
    pub args: Vec<String>,
}

impl Resolved {
    /// The words to pass to a child process so it resolves to the same command
    /// **without** the symlink: the host must hand `foot` the canonical path,
    /// not the name it was called by.
    pub fn words(&self) -> Vec<&'static str> {
        self.spec.path.split(' ').collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The binary was called with no command at all.
    NoCommand,
    /// A name nothing in the table answers to.
    Unknown(String),
}

/// Resolve `argv[0]` (which may be a symlink name) and the words after it.
pub fn resolve(argv0: &str, args: &[String]) -> Result<Resolved, ResolveError> {
    let called_as = basename(argv0);

    // 1. The file name wins: a symlink is an explicit name for one command, and
    //    `hayato` itself is not in any row's `symlinks`.
    for spec in COMMANDS {
        if spec.symlinks.contains(&called_as.as_str()) {
            return Ok(Resolved {
                spec,
                args: args.to_vec(),
            });
        }
    }

    // 2. The words after `hayato`, longest match first (`menu files` before
    //    `menu`, so a sub-surface is not swallowed by its parent).
    if !args.is_empty() {
        let mut best: Option<(usize, &'static CommandSpec)> = None;
        for spec in COMMANDS {
            let words: Vec<&str> = spec.path.split(' ').collect();
            if words.len() <= args.len() && args[..words.len()] == words[..] {
                match best {
                    Some((len, _)) if len >= words.len() => {}
                    _ => best = Some((words.len(), spec)),
                }
            }
        }
        if let Some((len, spec)) = best {
            return Ok(Resolved {
                spec,
                args: args[len..].to_vec(),
            });
        }
        return Err(ResolveError::Unknown(args.join(" ")));
    }

    Err(ResolveError::NoCommand)
}

fn basename(argv0: &str) -> String {
    Path::new(argv0)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| argv0.to_string())
}

/// The `name<TAB>command` lines `--symlinks` prints and `check-system.sh`
/// reads: only the links the installer creates today.
pub fn installed_symlinks() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for spec in COMMANDS {
        for name in spec.installed {
            out.push((*name, spec.path));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn dispatch_resolves_symlink_names() {
        // The exact pair spec 001 names: the old script name and the new one.
        let by_symlink = resolve("hayato-files", &[]).unwrap();
        let by_words = resolve("hayato", &args(&["menu", "files"])).unwrap();
        assert_eq!(by_symlink.spec.path, by_words.spec.path);
        assert_eq!(by_symlink.spec.path, "menu files");
        assert_eq!(
            resolve("/usr/bin/hayato-menu", &[]).unwrap().spec.path,
            "menu"
        );
    }

    #[test]
    fn the_power_list_is_one_command_reached_two_ways() {
        // `hayato-power` and `hayato menu power` are the same handler, and the
        // words win over the link so both keep working while the link is
        // being installed.
        assert_eq!(
            resolve("hayato-power", &[]).unwrap().spec.path,
            "menu power"
        );
        assert_eq!(
            resolve("hayato", &args(&["menu", "power"]))
                .unwrap()
                .spec
                .path,
            "menu power"
        );
        // It is a link to this binary and not a row of the root menu: the
        // root menu owns `lock`, `suspend` and `hibernate` already.
        assert!(
            COMMANDS
                .iter()
                .any(|spec| spec.path == "menu power" && spec.symlinks.contains(&"hayato-power"))
        );
    }

    #[test]
    fn dispatch_takes_the_longest_match_and_keeps_the_rest() {
        let r = resolve("hayato", &args(&["menu", "files", "Pictures"])).unwrap();
        assert_eq!(r.spec.path, "menu files");
        assert_eq!(r.args, args(&["Pictures"]));
        assert_eq!(
            resolve("hayato", &args(&["menu", "bogus"])).unwrap().args,
            args(&["bogus"])
        );
    }

    #[test]
    fn dispatch_rejects_unknown() {
        assert_eq!(
            resolve("hayato", &args(&["nope"])),
            Err(ResolveError::Unknown("nope".into()))
        );
        assert_eq!(resolve("hayato", &[]), Err(ResolveError::NoCommand));
        // A symlink name that is not in the table is not a command either.
        assert_eq!(
            resolve("unknown-tool", &args(&["menu"])),
            Ok(Resolved {
                spec: &COMMANDS[0],
                args: vec![]
            })
        );
    }

    #[test]
    fn the_table_has_no_duplicate_names() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in COMMANDS {
            assert!(
                seen.insert(spec.path),
                "duplicate command path {}",
                spec.path
            );
            for name in spec.symlinks {
                assert!(seen.insert(*name), "duplicate name {name}");
            }
            assert!(
                spec.installed.iter().all(|n| spec.symlinks.contains(n)),
                "{} installs a name it does not declare",
                spec.path
            );
        }
    }

    #[test]
    fn every_installed_symlink_names_a_command_help_lists() {
        for (name, path) in installed_symlinks() {
            assert!(!name.is_empty());
            assert!(
                COMMANDS.iter().any(|c| c.path == path),
                "{name} points at an unknown command {path}"
            );
        }
    }

    #[test]
    fn shapes_carry_their_own_app_id_and_size() {
        assert_eq!(Shape::Pick.app_id(), "hayato-pick");
        assert_eq!(Shape::Task.app_id(), "hayato-task");
        assert_eq!(Shape::Wide.app_id(), "hayato-wide");
        assert_eq!(Shape::Full.app_id(), "hayato-full");
        assert_eq!(Shape::Wide.size(), Some((1120, 780)));
        // The size is what the sway drop-in repeats for the same app-id; a
        // shape without one owns the whole screen.
        assert_eq!(Shape::Task.size(), Some((940, 620)));
        assert_eq!(Shape::Full.size(), None);
    }

    #[test]
    fn the_child_is_given_the_canonical_words_not_the_symlink() {
        let r = resolve("hayato-files", &args(&["--search"])).unwrap();
        assert_eq!(r.words(), vec!["menu", "files"]);
        assert_eq!(r.args, args(&["--search"]));
    }
}
