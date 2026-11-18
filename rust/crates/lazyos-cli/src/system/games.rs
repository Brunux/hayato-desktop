//! The game library: a Ratatui list over `~/Games` and the LazyOS HTML games.
//!
//! The library is the framework's own screen, so it looks and navigates like
//! every other LazyOS menu at any window size or fullscreen. The GTK + WebKit
//! binary `lazyos-games` is only the host that plays one HTML game; this screen
//! starts it (or RetroArch) and closes, and the host returns here when the game
//! ends.
//!
//! Left/Right are the tabs (All games, Recent, Favourites, each console,
//! LazyOS games); Up/Down move the list; Enter plays; `m` opens the manual;
//! Esc closes. That is the same everywhere, and `lazyos gamepad` injects
//! exactly those keys.
//!
//! The manual is part of this screen rather than a second window: it is the
//! answer to "where do I put a ROM and how do I run it", and the platform table
//! in it is built from the same `platform::PLATFORMS` the scan uses, so the
//! extensions and core names cannot drift from what Enter actually runs.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use lazyos_games_core::launch;
use lazyos_games_core::library::{
    self, Game, GameKind, Library, PublicPlatform, ScanOptions, State,
};
use lazyos_games_core::paths;
use lazyos_games_core::platform::{self, PLATFORMS};
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card, draw_filter_bar, draw_scrollbar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub struct GamesScreen {
    options: ScanOptions,
    state: State,
    library: Library,
    tabs: Vec<PublicPlatform>,
    tab: usize,
    games: Vec<Game>,
    list: ListState,
    mode: Mode,
    header: Header,
    keybar: Vec<KeyHint>,
    manual_keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    note: Option<String>,
}

/// What the screen is showing. One enum, one owner for each state's data
/// (AGENTS rule 12): the manual keeps its blocks, its wrapped lines and its
/// scroll position, and the list keeps its own.
enum Mode {
    Library,
    Manual(Manual),
}

/// The manual's own state: the text as written, the same text wrapped for the
/// current window width, and where the person has scrolled to.
struct Manual {
    blocks: Vec<Block>,
    lines: Vec<ManualLine>,
    width: u16,
    height: u16,
    scroll: u16,
}

/// One unwrapped piece of the manual. Wrapping depends on the window, so the
/// text is kept as blocks and re-flowed only when the width changes.
enum Block {
    Section(&'static str),
    Text(String),
    Code(String),
}

impl Block {
    fn text(&self) -> &str {
        match self {
            Block::Section(title) => title,
            Block::Text(text) | Block::Code(text) => text,
        }
    }
}

/// One drawn line of the manual: the text and the design token it asks for.
struct ManualLine {
    text: String,
    style: TextStyle,
}

impl Manual {
    fn new(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            lines: Vec::new(),
            width: 0,
            height: 0,
            scroll: 0,
        }
    }

    /// Wrap for this width. Called from `draw`, so a resize or fullscreen
    /// re-flows instead of clipping, and the cost is paid once per width.
    fn reflow(&mut self, width: u16) {
        self.width = width;
        let width = usize::from(width);
        let mut lines = Vec::new();
        for (index, block) in self.blocks.iter().enumerate() {
            if index > 0 {
                lines.push(ManualLine {
                    text: String::new(),
                    style: TextStyle::Body,
                });
            }
            let style = match block {
                Block::Section(_) => TextStyle::Title,
                Block::Text(_) => TextStyle::Body,
                Block::Code(_) => TextStyle::Code,
            };
            for text in wrap(block.text(), width) {
                lines.push(ManualLine { text, style });
            }
        }
        self.lines = lines;
        self.clamp();
    }

    fn max_scroll(&self) -> u16 {
        self.lines.len().saturating_sub(usize::from(self.height)) as u16
    }

    fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn scroll_by(&mut self, step: i32) {
        let next = i64::from(self.scroll) + i64::from(step);
        self.scroll = next.clamp(0, i64::from(self.max_scroll())) as u16;
    }

    fn home(&mut self) {
        self.scroll = 0;
    }

    fn end(&mut self) {
        self.scroll = self.max_scroll();
    }
}

impl GamesScreen {
    pub fn new() -> Self {
        let options = ScanOptions::from_env();
        let state = library::load_state(&options.state_path());
        let library = library::scan(&options, &state);
        let tabs = library.public().platforms;
        let tab = tabs
            .iter()
            .position(|platform| platform.id == state.filter)
            .unwrap_or(0);
        let mut screen = Self {
            options,
            state,
            library,
            tabs,
            tab,
            games: Vec::new(),
            list: ListState::new().with_empty("No games in this view"),
            mode: Mode::Library,
            header: Header::new("LazyOS").subtitle("Games"),
            keybar: vec![
                KeyHint::new("Enter", "play"),
                KeyHint::new("←/→", "platform"),
                KeyHint::new("f", "favourite"),
                KeyHint::new("s", "rescan"),
                KeyHint::new("m", "manual"),
                KeyHint::new("Esc", "close"),
            ],
            manual_keybar: vec![
                KeyHint::new("↑/↓", "scroll"),
                KeyHint::new("PgUp/PgDn", "page"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            note: None,
        };
        screen.rebuild();
        screen.list.set_cursor(screen.state.index);
        screen
    }

    fn tab_id(&self) -> &str {
        self.tabs
            .get(self.tab)
            .map(|platform| platform.id.as_str())
            .unwrap_or("all")
    }

    fn rebuild(&mut self) {
        let tab = self.tab_id();
        let mut games: Vec<Game> = self
            .library
            .games
            .iter()
            .filter(|game| match tab {
                "all" => true,
                "recent" => game.last > 0,
                "favorites" => game.favorite,
                "web" => game.kind == GameKind::Web,
                platform => game.platform == platform,
            })
            .cloned()
            .collect();
        if tab == "recent" {
            games.sort_by(|a, b| b.last.cmp(&a.last));
        }

        let rows: Vec<Row> = games
            .iter()
            .map(|game| {
                let mut detail = game.platform_title.clone();
                if game.save {
                    detail.push_str(" · save");
                }
                if game.kind == GameKind::Retro {
                    if let Some(core) = game.core.as_deref() {
                        if !self.library.core_present(core) {
                            detail.push_str(" · core missing");
                        }
                    }
                }
                if game.favorite {
                    detail.push_str(" · ★");
                }
                Row::new(game.title.clone())
                    .glyph(if game.kind == GameKind::Web {
                        Glyph::Window
                    } else {
                        Glyph::Play
                    })
                    .detail(detail)
                    .value(game.id.clone())
            })
            .collect();

        self.games = games;
        self.list.update_rows(rows);
    }

    fn save_view(&mut self) {
        self.state.filter = self.tab_id().to_string();
        self.state.index = self.list.selected_index().unwrap_or(0);
        if let Err(error) = library::save_state(&self.options.state_path(), &self.state) {
            self.note = Some(format!("Cannot save the library state: {error}"));
        }
    }

    fn cycle_tab(&mut self, step: isize) {
        if self.tabs.is_empty() {
            return;
        }
        let count = self.tabs.len() as isize;
        self.tab = (self.tab as isize + step).rem_euclid(count) as usize;
        self.list.set_filter("");
        self.rebuild();
        self.save_view();
        self.note = None;
    }

    fn reload(&mut self) {
        self.library = library::scan(&self.options, &self.state);
        self.tabs = self.library.public().platforms;
        self.tab = self
            .tabs
            .iter()
            .position(|platform| platform.id == self.state.filter)
            .unwrap_or(0);
        self.rebuild();
        self.note = Some("Rescanned the game folders".to_string());
    }

    fn toggle_favorite(&mut self) {
        let Some(index) = self.list.selected_index() else {
            return;
        };
        let Some(game) = self.games.get(index).cloned() else {
            return;
        };
        let on = !game.favorite;
        self.library.set_favorite(&game.id, on);
        if on {
            self.state.favorites.insert(game.id.clone());
        } else {
            self.state.favorites.remove(&game.id);
        }
        self.save_view();
        self.rebuild();
        self.note = Some(if on {
            "Favourite added".to_string()
        } else {
            "Favourite removed".to_string()
        });
    }

    /// Open the manual. The table comes from the platform list the scan uses
    /// and from the library's own core check, so a missing core is visible here
    /// before the person puts a ROM on the shelf.
    fn open_manual(&mut self) {
        let blocks = manual_blocks(&self.options.roots, &|core| self.library.core_present(core));
        self.mode = Mode::Manual(Manual::new(blocks));
    }

    fn activate(&mut self, index: usize) -> Outcome {
        let Some(game) = self.games.get(index).cloned() else {
            return Outcome::Continue;
        };
        if game.kind == GameKind::Retro {
            if let Some(core) = game.core.as_deref() {
                if !self.library.core_present(core) {
                    self.note = Some(format!("Core not installed: {core}"));
                    return Outcome::Consumed;
                }
            }
            if !self.library.retroarch {
                self.note = Some("RetroArch is not installed".to_string());
                return Outcome::Consumed;
            }
        }
        self.save_view();
        match launch::spawn_play(&game.id) {
            Ok(()) => Outcome::Done(lazyos_ui::app::exit::OK),
            Err(error) => {
                self.note = Some(format!("Cannot start {}: {error}", game.title));
                Outcome::Consumed
            }
        }
    }

    fn draw_tabs(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        let mut x = area.x;
        for (index, tab) in self.tabs.iter().enumerate() {
            let label = format!(" {} {} ", tab.title, tab.count);
            let width = UnicodeWidthStr::width(label.as_str()) as u16;
            if x + width > area.right() {
                break;
            }
            let style = if index == self.tab {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(design.color(ColorRole::Selection))
            } else {
                design.style(TextStyle::Dim)
            };
            buf.set_string(x, area.y, &label, style);
            x += width + 1;
        }
    }

    fn draw_library(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Games"), true);
        let inner = Rect {
            x: ctx.area.x + 2,
            y: ctx.area.y + 1,
            width: ctx.area.width.saturating_sub(4),
            height: ctx.area.height.saturating_sub(2),
        };
        if inner.height < 4 {
            return;
        }
        let tabs = Rect { height: 1, ..inner };
        self.draw_tabs(ctx.buf, tabs, ctx.design);
        let filter = Rect {
            y: inner.y + 2,
            height: 1,
            ..inner
        };
        let note = self
            .note
            .as_deref()
            .unwrap_or("Enter play · ←/→ platform · f favourite · s rescan · m manual · Esc close");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            note,
            self.list.len(),
            self.list.len(),
        );
        let rows = Rect {
            y: inner.y + 4,
            height: inner.height.saturating_sub(4),
            ..inner
        };
        self.rows_height.set(rows.height);
        self.list.render(rows, ctx.buf, ctx.design, true);
    }
}

/// The manual, drawn over the whole card. The list is not visible behind it:
/// a modal replaces the body, it never floats over a live list (AGENTS rule 11).
fn draw_manual(ctx: &mut BodyCtx, manual: &mut Manual) {
    draw_card(ctx.buf, ctx.area, ctx.design, Some("Games · manual"), true);
    let inner = Rect {
        x: ctx.area.x + 2,
        y: ctx.area.y + 1,
        width: ctx.area.width.saturating_sub(4),
        height: ctx.area.height.saturating_sub(2),
    };
    if inner.width < 12 || inner.height < 2 {
        return;
    }
    manual.height = inner.height;
    // Two columns stay free on the right: one for the card's inner padding and
    // one for the scrollbar, so no wrapped line ends under either.
    let text_width = inner.width.saturating_sub(2);
    if manual.width != text_width {
        manual.reflow(text_width);
    }
    manual.clamp();
    for (offset, line) in manual
        .lines
        .iter()
        .skip(usize::from(manual.scroll))
        .take(usize::from(inner.height))
        .enumerate()
    {
        if line.text.is_empty() {
            continue;
        }
        let style = ctx.design.style(line.style);
        ctx.buf
            .set_string(inner.x, inner.y + offset as u16, &line.text, style);
    }
    draw_scrollbar(
        ctx.buf,
        inner,
        ctx.design,
        usize::from(manual.scroll),
        usize::from(inner.height),
        manual.lines.len(),
        true,
    );
}

impl Screen for GamesScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        if let Mode::Manual(manual) = &mut self.mode {
            draw_manual(ctx, manual);
            return;
        }
        self.draw_library(ctx);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if matches!(self.mode, Mode::Manual(_)) {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('q')
            ) {
                self.mode = Mode::Library;
                return Outcome::Consumed;
            }
            if let Mode::Manual(manual) = &mut self.mode {
                let page = i32::from(manual.height.saturating_sub(1)).max(1);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => manual.scroll_by(-1),
                    KeyCode::Down | KeyCode::Char('j') => manual.scroll_by(1),
                    KeyCode::PageUp => manual.scroll_by(-page),
                    KeyCode::PageDown => manual.scroll_by(page),
                    KeyCode::Home => manual.home(),
                    KeyCode::End => manual.end(),
                    _ => {}
                }
            }
            return Outcome::Consumed;
        }
        match key.code {
            KeyCode::Left => {
                self.cycle_tab(-1);
                return Outcome::Consumed;
            }
            KeyCode::Right => {
                self.cycle_tab(1);
                return Outcome::Consumed;
            }
            KeyCode::Esc => {
                if !self.list.filter().is_empty() {
                    self.list.set_filter("");
                    return Outcome::Consumed;
                }
                return Outcome::Done(lazyos_ui::app::exit::ESCAPE);
            }
            KeyCode::Char('f')
                if self.list.filter().is_empty()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.toggle_favorite();
                return Outcome::Consumed;
            }
            KeyCode::Char('s')
                if self.list.filter().is_empty()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.reload();
                return Outcome::Consumed;
            }
            KeyCode::Char('m')
                if self.list.filter().is_empty()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.open_manual();
                return Outcome::Consumed;
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
                return Outcome::Consumed;
            }
            _ => {}
        }
        match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        }
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if matches!(self.mode, Mode::Manual(_)) {
            return Outcome::Continue;
        }
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if matches!(self.mode, Mode::Manual(_)) {
            return None;
        }
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let rows = Rect {
            y: inner.y + 4,
            height: inner.height.saturating_sub(4),
            ..inner
        };
        self.list.hit_row(rows, x, y, design)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(match self.mode {
            Mode::Library => &self.keybar,
            Mode::Manual(_) => &self.manual_keybar,
        })
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            Mode::Library => "list",
            Mode::Manual(_) => "manual",
        })
    }

    /// The Pick shape's window. A library opened by the game host comes back
    /// through `foot` with no size of its own, so the screen asks for the same
    /// 900×600 the framework's host would have given it.
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((900, 600))
    }
}

/// The manual's text. It is written as blocks, not lines, because the window
/// decides where a line ends; the platform table is generated so it can never
/// disagree with the scan.
fn manual_blocks(roots: &[PathBuf], core_present: &dyn Fn(&str) -> bool) -> Vec<Block> {
    let scanned = if roots.is_empty() {
        "~/Games".to_string()
    } else {
        roots
            .iter()
            .map(|root| pretty_root(root))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut blocks = vec![
        Block::Section("Where ROMs go"),
        Block::Text(
            "Put a ROM in ~/Games, in any folder you like: the file extension chooses \
             the platform, not the folder name. The scan walks every subfolder, skips \
             hidden ones and art, bios, covers, saves, states, system and web, and \
             ignores a file smaller than the platform's floor — that is what keeps a \
             README.md off the Genesis shelf."
                .to_string(),
        ),
        Block::Text(format!(
            "This session scans: {scanned}\n\
             Somewhere else: LAZYOS_GAMES_ROM_DIRS=/media/usb/games:~/MoreGames"
        )),
        Block::Text(
            "A .zip is not unpacked: unzip it first — except Neo Geo, whose \
             romsets stay zipped and are read as they are. Put those in \
             ~/Games/neogeo/ (mvs/, aes/ and fbneo/ work too) with the BIOS \
             neogeo.zip in ~/Games/bios/. A .zip anywhere else is not a game. \
             For a multi-disc PlayStation game, list the .cue files in an .m3u \
             and select that."
                .to_string(),
        ),
        Block::Section("Platforms"),
        Block::Text(
            "The extension picks the row; the core named beside it must be installed.".to_string(),
        ),
    ];
    for row in platform_rows(core_present) {
        blocks.push(Block::Code(row));
    }
    blocks.extend([
        Block::Text(
            "Nintendo 64 is experimental: it runs at 640×480 and some games do not \
             reach full speed. A row whose core is missing says so here, and Enter on \
             the game explains it again."
                .to_string(),
        ),
        Block::Section("Firmware"),
        Block::Text(
            "Firmware goes in ~/Games/bios/ with the exact name the core expects. \
             LazyOS ships none of it: a BIOS is copyrighted."
                .to_string(),
        ),
        Block::Code(bios_rows().join("\n")),
        Block::Section("Play a game"),
        Block::Text(
            "1. Copy the ROM into ~/Games (a Neo Geo romset stays zipped, in neogeo/).\n\
             2. Press s here to rescan, or reopen the library.\n\
             3. ← / → choose a tab, ↑ / ↓ a game, Enter plays it.\n\
             4. Esc in the game comes back here; F1 opens RetroArch's menu."
                .to_string(),
        ),
        Block::Section("Keys"),
        Block::Code(
            "← / → — previous / next tab: All games, Recent, Favourites, consoles\n\
             ↑ / ↓ — move through the games in the tab\n\
             Enter — play the selected game\n\
             type — filter the tab by title or platform\n\
             f — favourite / unfavourite\n\
             s — rescan ~/Games\n\
             m — this manual\n\
             Esc — clear the filter, then close the library\n\
             Alt+F — fullscreen on or off\n\
             F1 — RetroArch's menu, inside a game\n\
             Esc — leave a game and come back here"
                .to_string(),
        ),
        Block::Text(
            "A gamepad is optional: lazyos gamepad maps a USB or Bluetooth pad to the \
             same arrows, Enter and Escape in every LazyOS window, this one included."
                .to_string(),
        ),
        Block::Section("Saves"),
        Block::Text(
            "Battery saves live in ~/.local/share/lazyos/games/saves and save states in \
             ~/.local/state/lazyos/games/states. Both load and save automatically, so a \
             game continues where you left it, and a row with a save says so."
                .to_string(),
        ),
        Block::Section("When something does not work"),
        Block::Code(
            "Core not installed — sudo apt install libretro-<core>\n\
             RetroArch is missing — scripts/build-retroarch.sh, then core/scripts/install-retroarch.sh\n\
             PS1 asks for a BIOS — put scph5501.bin in ~/Games/bios/\n\
             Neo Geo asks for BIOS — put neogeo.zip (the MVS set) in ~/Games/bios/\n\
             N64 is slow — lower the core resolution in F1 (experimental)\n\
             Blank window, bad GPU — LAZYOS_GAMES_NO_ACCEL=1\n\
             No sound — RetroArch uses the ALSA default PCM; check pactl info\n\
             A ROM does not appear — check its extension and the size floor, then press s"
                .to_string(),
        ),
        Block::Section("LazyOS HTML games"),
        Block::Text(
            "A LazyOS game is a folder with game.toml and index.html in \
             ~/.local/share/lazyos/games/web/<name>/. It plays in the GTK + WebKitGTK \
             host, fullscreen; Ctrl+Q or Select+Start leaves it. Snake is the sample."
                .to_string(),
        ),
    ]);
    blocks
}

/// One line per platform, from the same table the scan uses. The core column
/// says "(core missing)" when the .so is not installed, which is the answer to
/// half of the "why will it not start" questions.
///
/// Columns are separated by an em dash rather than padded with spaces: the
/// manual wraps to the window, and wrapping collapses runs of spaces, which
/// would turn a padded table into a ragged one.
fn platform_rows(core_present: &dyn Fn(&str) -> bool) -> Vec<String> {
    PLATFORMS
        .iter()
        .map(|row| {
            let title = if row.id == "n64" {
                format!("{} (experimental)", row.title)
            } else {
                row.title.to_string()
            };
            let mut extensions = row.extensions.join(", ");
            if let Some(folders) = row.folders {
                extensions.push_str(&format!(" (in {}/)", folders[0]));
            }
            let mut line = format!("{title} — {extensions} — {}", row.core);
            if !core_present(row.core) {
                line.push_str("   (core missing)");
            }
            line
        })
        .collect()
}

/// The firmware line per platform, from the same table: a platform that needs
/// a BIOS names it once, in `Platform::bios`.
fn bios_rows() -> Vec<String> {
    PLATFORMS
        .iter()
        .filter_map(|row| row.bios.map(|bios| format!("{} — {bios}", row.title)))
        .collect()
}

/// `~/Games` for a path under the home directory, the absolute path otherwise.
fn pretty_root(path: &Path) -> String {
    let home = paths::home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Word-wrap `text` to `width` display columns, keeping the explicit newlines
/// (the steps and the key table rely on them). A word wider than the line is
/// hard-broken rather than drawn past the card.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut used = 0usize;
        for word in paragraph.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);
            if used > 0 && used + 1 + word_width <= width {
                line.push(' ');
                line.push_str(word);
                used += 1 + word_width;
                continue;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
            }
            if word_width <= width {
                line.push_str(word);
                used = word_width;
                continue;
            }
            // A word longer than the line: break it by display width.
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for character in word.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(1);
                if chunk_width + character_width > width {
                    out.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(character);
                chunk_width += character_width;
            }
            line = chunk;
            used = chunk_width;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The `--rows` answer `check-desktop.sh` reads: one game per line, no window.
pub fn rows() -> Vec<String> {
    let options = ScanOptions::from_env();
    let state = library::load_state(&options.state_path());
    let library = library::scan(&options, &state);
    library
        .games
        .iter()
        .map(|game| {
            format!(
                "{}\t{}\t{}\t{}",
                game.id,
                game.title,
                game.platform,
                game.core.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

/// `lazyos menu games`: the window.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let mut screen = GamesScreen::new();
    crate::run_screen(terminal, design, "Games", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual_text(present: &dyn Fn(&str) -> bool) -> String {
        manual_blocks(&[PathBuf::from("/home/x/Games")], present)
            .iter()
            .map(Block::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_manual_lists_every_platform_and_its_core() {
        let text = manual_text(&|_| true);
        for row in platform::PLATFORMS {
            assert!(text.contains(row.title), "{} is missing", row.title);
            assert!(text.contains(row.core), "{} is missing", row.core);
            for extension in row.extensions {
                assert!(
                    text.contains(extension),
                    "{} is missing from {}",
                    extension,
                    row.title
                );
            }
        }
        assert!(!text.contains("core missing"));
    }

    #[test]
    fn a_missing_core_is_named_in_the_table() {
        let text = manual_text(&|_| false);
        assert_eq!(
            text.matches("(core missing)").count(),
            platform::PLATFORMS.len()
        );
    }

    #[test]
    fn the_manual_answers_where_and_how() {
        let text = manual_text(&|_| true);
        assert!(text.contains("/home/x/Games"), "the scan root is not shown");
        assert!(text.contains("~/Games/bios/"));
        assert!(text.contains("scph5501.bin"));
        assert!(text.contains("neogeo.zip"));
        assert!(text.contains("~/Games/neogeo/"));
        assert!(text.contains("Press s here to rescan"));
        assert!(text.contains("Enter plays it"));
        assert!(text.contains("~/.local/share/lazyos/games/saves"));
    }

    #[test]
    fn a_paragraph_wraps_at_the_width() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("one\ntwo", 20), vec!["one", "two"]);
        assert_eq!(wrap("", 20), vec![""]);
        // A word wider than the line is broken, never drawn past the card
        // (the floor is 8 columns, so a narrower card still gets whole words).
        assert_eq!(wrap("abcdefghij", 8), vec!["abcdefgh", "ij"]);
    }

    #[test]
    fn m_opens_the_manual_and_escape_goes_back() {
        let mut screen = GamesScreen::new();
        let design = Design::for_session(None);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        assert!(matches!(screen.mode, Mode::Library));
        assert_eq!(screen.phase(), Some("list"));

        screen.key(key(KeyCode::Char('m')), &design);
        assert!(matches!(screen.mode, Mode::Manual(_)));
        assert_eq!(screen.phase(), Some("manual"));

        // Scrolling consumes arrows without touching the list.
        let before = screen.list.selected_index();
        screen.key(key(KeyCode::Down), &design);
        assert_eq!(screen.list.selected_index(), before);

        screen.key(key(KeyCode::Esc), &design);
        assert!(matches!(screen.mode, Mode::Library));
        assert_eq!(screen.phase(), Some("list"));
    }
}
