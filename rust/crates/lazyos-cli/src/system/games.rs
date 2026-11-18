//! The game library: a shelf over `~/Games` and the LazyOS HTML games.
//!
//! The screen is the framework's own, so it navigates like every other LazyOS
//! menu at any window size or fullscreen. It is a master-detail surface: the
//! rail picks a platform, the shelf (or the dense list) picks a game, and the
//! hero on the right shows the one under the cursor — its own generated
//! cartridge art, its core, its size, its folder and when it was last played.
//!
//! Left/Right are the tabs; Up/Down move the shelf; Enter plays; `o` opens the
//! four options; `m` the manual; `r` the game's folder in Files; `f` favourites;
//! `s` rescans; Esc closes. That is the same everywhere, and `lazyos gamepad`
//! injects exactly those keys.
//!
//! The GTK + WebKit binary `lazyos-games` is only the host that plays one HTML
//! game; this screen starts it (or RetroArch) and closes, and the host returns
//! here when the game ends.

mod art;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lazyos_games_core::launch;
use lazyos_games_core::library::{
    self, Game, GameKind, Layout, Library, Options, PublicPlatform, ScanOptions, Sort, State,
};
use lazyos_games_core::paths;
use lazyos_games_core::platform::{self, PLATFORMS};
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, centered, draw_card, draw_card_with, draw_filter_bar, draw_scrollbar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use tachyonfx::{Effect, EffectTimer, Interpolation, Motion as Sweep, fx};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The window the shelf wants. The screen declares it, so a library opened by
/// the game host grows to it; the list is narrower because it has no hero.
const SHELF_SIZE: (u16, u16) = (1200, 780);
const LIST_SIZE: (u16, u16) = (1000, 640);
/// Below this the hero would squeeze the shelf, so the shelf takes the window.
const HERO_MIN_WIDTH: u16 = 100;
/// A shelf card: the cartridge, the title, the detail and the path.
const CARD_HEIGHT: u16 = 3;
const OPTION_ROWS: usize = 4;

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
    options_keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    note: Option<String>,
    /// First visible shelf card, so the cursor stays on screen.
    scroll: Cell<usize>,
    /// The selection flash: a short sweep over the card that just got focus.
    flash: Option<Flash>,
    flash_at: Option<Instant>,
}

/// What the screen is showing. One enum, one owner for each state's data
/// (AGENTS rule 12).
enum Mode {
    Library,
    Manual(Manual),
    Options(OptionsState),
}

/// The options card's cursor: four rows, no data of its own.
struct OptionsState {
    row: usize,
}

/// One sweep of the accent colour over the card that just became the cursor.
struct Flash {
    effect: Effect,
    area: Rect,
    last: Instant,
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
                KeyHint::new("o", "options"),
                KeyHint::new("m", "manual"),
                KeyHint::new("f", "favourite"),
                KeyHint::new("s", "rescan"),
                KeyHint::new("Esc", "close"),
            ],
            manual_keybar: vec![
                KeyHint::new("↑/↓", "scroll"),
                KeyHint::new("PgUp/PgDn", "page"),
                KeyHint::new("Esc", "back"),
            ],
            options_keybar: vec![
                KeyHint::new("↑/↓", "row"),
                KeyHint::new("←/→", "change"),
                KeyHint::new("Esc", "back"),
            ],
            rows_height: Cell::new(20),
            note: None,
            scroll: Cell::new(0),
            flash: None,
            flash_at: None,
        };
        screen.rebuild();
        screen.select_saved();
        screen
    }

    fn tab_id(&self) -> &str {
        self.tabs
            .get(self.tab)
            .map(|platform| platform.id.as_str())
            .unwrap_or("all")
    }

    /// Rebuild the visible shelf: the tab, the options' filters, the options'
    /// order, and the rows the list layout and the filter work on.
    fn rebuild(&mut self) {
        let tab = self.tab_id();
        let hide_missing = self.state.options.hide_missing;
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
            .filter(|game| {
                !(hide_missing
                    && game.kind == GameKind::Retro
                    && game
                        .core
                        .as_deref()
                        .is_some_and(|core| !self.library.core_present(core)))
            })
            .cloned()
            .collect();
        self.state.options.sort.apply(&mut games);

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
            })
            .collect();

        self.games = games;
        self.list.update_rows(rows);
        self.scroll.set(0);
    }

    /// Put the cursor back on the game the state remembers, by id: a re-sort or
    /// a hidden row must not silently move it to a different game.
    fn select_saved(&mut self) {
        if !self.state.selected.is_empty()
            && let Some(index) = self
                .games
                .iter()
                .position(|game| game.id == self.state.selected)
        {
            self.list.set_cursor(index);
            return;
        }
        let index = self.state.index.min(self.games.len().saturating_sub(1));
        self.list.set_cursor(index);
    }

    fn selected_game(&self) -> Option<&Game> {
        self.list
            .selected_index()
            .and_then(|index| self.games.get(index))
    }

    fn save_view(&mut self) {
        self.state.filter = self.tab_id().to_string();
        self.state.index = self.list.selected_index().unwrap_or(0);
        self.state.selected = self
            .selected_game()
            .map(|game| game.id.clone())
            .unwrap_or_default();
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
        self.list.set_cursor(0);
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
        self.select_saved();
        self.note = Some("Rescanned the game folders".to_string());
    }

    fn toggle_favorite(&mut self) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let on = !game.favorite;
        self.library.set_favorite(&game.id, on);
        if on {
            self.state.favorites.insert(game.id.clone());
        } else {
            self.state.favorites.remove(&game.id);
        }
        self.state.selected = game.id.clone();
        self.rebuild_keep_selection();
        self.save_view();
        self.note = Some(if on {
            "Favourite added".to_string()
        } else {
            "Favourite removed".to_string()
        });
    }

    /// Rebuild and keep the cursor on the same game, by id. Used when the
    /// options change the order or the filter.
    fn rebuild_keep_selection(&mut self) {
        let selected = self.selected_game().map(|game| game.id.clone());
        self.list.set_filter("");
        self.rebuild();
        if let Some(id) = selected
            && let Some(index) = self.games.iter().position(|game| game.id == id)
        {
            self.list.set_cursor(index);
        }
    }

    fn open_manual(&mut self) {
        let blocks = manual_blocks(&self.options.roots, &|core| self.library.core_present(core));
        self.mode = Mode::Manual(Manual::new(blocks));
    }

    fn open_options(&mut self) {
        self.mode = Mode::Options(OptionsState { row: 0 });
    }

    /// Change one option, rebuild what it affects and persist it. The four
    /// rows are the whole settings surface, on purpose.
    fn cycle_option(&mut self, row: usize, step: isize) {
        match row {
            0 => self.state.options.sort = self.state.options.sort.next(step),
            1 => self.state.options.layout = self.state.options.layout.next(),
            2 => self.state.options.covers = !self.state.options.covers,
            3 => self.state.options.hide_missing = !self.state.options.hide_missing,
            _ => return,
        }
        self.rebuild_keep_selection();
        self.save_view();
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

    /// Open the selected game's folder in Files, in its own window. The shelf
    /// stays where it is, and the person decides in Files what to do.
    fn reveal(&mut self) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        match launch::reveal(&game) {
            Ok(()) => self.note = Some("Opened the folder in Files".to_string()),
            Err(error) => self.note = Some(format!("Cannot open Files: {error}")),
        }
    }

    /// The body's three pieces: the rail's row, the shelf and the hero. The
    /// hero only exists when the window is wide enough for both.
    fn content_rects(area: Rect) -> Option<(Rect, Rect, Option<Rect>)> {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        if inner.height < 6 || inner.width < 20 {
            return None;
        }
        let content = Rect {
            y: inner.y + 3,
            height: inner.height.saturating_sub(3),
            ..inner
        };
        if inner.width >= HERO_MIN_WIDTH {
            let hero_width = (inner.width / 3).clamp(34, 46);
            let shelf = Rect {
                width: inner.width.saturating_sub(hero_width + 1),
                ..content
            };
            let hero = Rect {
                x: shelf.right() + 1,
                width: hero_width,
                ..content
            };
            Some((inner, shelf, Some(hero)))
        } else {
            Some((inner, content, None))
        }
    }

    fn draw_rail(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        let missing = |tab: &PublicPlatform| {
            platform::PLATFORMS
                .iter()
                .find(|row| row.id == tab.id)
                .is_some_and(|row| !self.library.core_present(row.core))
        };
        let mut x = area.x;
        let mut start = 0;
        let total: u16 = self
            .tabs
            .iter()
            .map(|tab| {
                UnicodeWidthStr::width(rail_title(tab).as_str()) as u16
                    + 4
                    + u16::from(missing(tab))
            })
            .sum();
        if total > area.width {
            start = self.tab.saturating_sub(1);
        }
        for (index, tab) in self.tabs.iter().enumerate().skip(start) {
            let label = format!(" {} {} ", rail_title(tab), tab.count);
            let width = UnicodeWidthStr::width(label.as_str()) as u16 + u16::from(missing(tab));
            if x + width > area.right() {
                break;
            }
            let selected = index == self.tab;
            let background = design.color(ColorRole::Bg);
            if missing(tab) {
                buf.set_string(
                    x,
                    area.y,
                    "●",
                    Style::default()
                        .fg(design.color(ColorRole::Danger))
                        .bg(background),
                );
                x += 1;
            }
            let style = Style::default()
                .fg(design.color(if selected {
                    ColorRole::Accent
                } else {
                    ColorRole::Dim
                }))
                .bg(if selected {
                    design.color(ColorRole::Selection)
                } else {
                    background
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            buf.set_string(x, area.y, &label, style);
            x += UnicodeWidthStr::width(label.as_str()) as u16 + 1;
        }
    }

    /// The shelf: one rich card per game, three lines each, with the cursor's
    /// card flashed by a short accent sweep when it moves.
    fn draw_shelf(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect, design: &Design) {
        if area.height < CARD_HEIGHT {
            return;
        }
        let covers = self.state.options.covers;
        let visible = usize::from(area.height / CARD_HEIGHT).max(1);
        let cursor = self.list.cursor();
        let count = self.list.visible_len();
        let mut scroll = self.scroll.get();
        if cursor < scroll {
            scroll = cursor;
        }
        if cursor >= scroll + visible {
            scroll = cursor + 1 - visible;
        }
        scroll = scroll.min(count.saturating_sub(visible));
        self.scroll.set(scroll);

        let now = library::now();
        let mut selected_area = None;
        for position in scroll..count.min(scroll + visible) {
            let Some(index) = self.list.visible_index(position) else {
                continue;
            };
            let Some(game) = self.games.get(index) else {
                continue;
            };
            let card = Rect {
                x: area.x,
                y: area.y + (position - scroll) as u16 * CARD_HEIGHT,
                width: area.width,
                height: CARD_HEIGHT,
            };
            let selected = position == cursor;
            if selected {
                selected_area = Some(card);
            }
            let core_present = game
                .core
                .as_deref()
                .is_none_or(|core| self.library.core_present(core));
            draw_game_card(buf, card, design, game, selected, covers, core_present, now);
        }

        // The flash: created once the card that moved into focus is known, and
        // advanced by the elapsed time, not by a fixed tick.
        if let Some(started) = self.flash_at.take()
            && !design.reduced_motion()
            && let Some(card) = selected_area
        {
            self.flash = Some(Flash {
                effect: fx::sweep_in(
                    Sweep::LeftToRight,
                    6,
                    0,
                    design.color(ColorRole::Selection),
                    EffectTimer::from_ms(180, Interpolation::QuadOut),
                ),
                area: card,
                last: started,
            });
        }
        if let Some(flash) = self.flash.as_mut() {
            let now = Instant::now();
            let elapsed = now
                .saturating_duration_since(flash.last)
                .min(Duration::from_millis(60));
            flash.last = now;
            flash.effect.process(elapsed.into(), buf, flash.area);
            if flash.effect.done() {
                self.flash = None;
            }
        }
    }

    /// The hero: the selected game, big. The card's own title carries the full
    /// name; inside are the monogram, the facts and the play button.
    fn draw_hero(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        design: &Design,
        game: Option<&Game>,
    ) {
        let title = game
            .map(|game| art::fit(&game.title, usize::from(area.width.saturating_sub(4))))
            .unwrap_or_else(|| "Selected".to_string());
        draw_card_with(
            buf,
            area,
            design,
            Some(&title),
            ColorRole::Accent,
            design.style(TextStyle::Title),
        );
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let Some(game) = game else {
            let hint = "Put a ROM in ~/Games and press s";
            let y = inner.y + inner.height / 2;
            buf.set_string(
                inner.x,
                y.saturating_sub(1),
                &art::fit("No games in this view", usize::from(inner.width)),
                design.style(TextStyle::Title),
            );
            buf.set_string(
                inner.x,
                y,
                &art::fit(hint, usize::from(inner.width)),
                design.style(TextStyle::Dim),
            );
            return;
        };
        if inner.width < 12 || inner.height < 6 {
            return;
        }
        let mut y = inner.y;
        if self.state.options.covers && inner.height >= 12 {
            let scale = if inner.height >= 24 { 2 } else { 1 };
            art::draw_monogram(buf, inner.x, y, design, art::initial(&game.title), scale);
            y += 5 * scale + 1;
        }
        let text_width = usize::from(inner.width);
        let line = |buf: &mut ratatui::buffer::Buffer, text: &str, style: TextStyle, y: u16| {
            if y < inner.bottom() {
                buf.set_string(inner.x, y, &art::fit(text, text_width), design.style(style));
            }
        };
        // A web game's platform title is "LazyOS" and its kind is "LazyOS
        // game"; saying both reads like a stutter.
        let heading = match game.kind {
            GameKind::Web => "LazyOS game".to_string(),
            GameKind::Retro if is_arcade(&game.platform) => {
                format!("{} · arcade", game.platform_title)
            }
            GameKind::Retro => format!("{} · console", game.platform_title),
        };
        line(buf, &heading, TextStyle::Title, y);
        y += 1;
        let core = game.core.as_deref().unwrap_or("WebKitGTK");
        let size = if game.size > 0 {
            format!(" · {}", art::human_size(game.size))
        } else {
            String::new()
        };
        line(buf, &format!("{core}{size}"), TextStyle::Code, y);
        y += 1;
        let mut badges = Vec::new();
        if game.favorite {
            badges.push("★ favourite".to_string());
        }
        if game.save {
            badges.push("save".to_string());
        }
        badges.push(art::ago(game.last, library::now()));
        line(buf, &badges.join(" · "), TextStyle::Dim, y);
        y += 2;
        // The path is one line with an ellipsis: a path broken mid-name reads
        // as two paths, and `r` opens the folder when the rest matters.
        if let Some(path) = game_path(game) {
            line(buf, &path, TextStyle::Dim, y);
            y += 1;
        }
        if let Some(description) = game.description.as_deref() {
            y += 1;
            for wrapped in wrap(description, text_width).into_iter().take(3) {
                line(buf, &wrapped, TextStyle::Body, y);
                y += 1;
            }
        }
        // The play button follows the facts, and never leaves the card.
        if inner.height >= 8 {
            let button = "  ▶  PLAY  ";
            let width = UnicodeWidthStr::width(button) as u16;
            let button_y = (y + 1).min(inner.bottom().saturating_sub(1));
            if inner.width > width + 2 {
                let rect = Rect {
                    x: inner.x,
                    y: button_y,
                    width,
                    height: 1,
                };
                buf.set_style(rect, Style::default().bg(design.color(ColorRole::Accent)));
                buf.set_string(
                    rect.x,
                    rect.y,
                    button,
                    Style::default()
                        .fg(design.color(ColorRole::Bg))
                        .bg(design.color(ColorRole::Accent))
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    fn draw_library(&mut self, ctx: &mut BodyCtx) {
        draw_card(ctx.buf, ctx.area, ctx.design, Some("Games"), true);
        let Some((inner, shelf, hero)) = Self::content_rects(ctx.area) else {
            return;
        };
        self.draw_rail(ctx.buf, Rect { height: 1, ..inner }, ctx.design);
        let filter = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        let note = self
            .note
            .as_deref()
            .unwrap_or("Enter play · ←/→ platform · o options · m manual · f favourite · s rescan");
        draw_filter_bar(
            ctx.buf,
            filter,
            ctx.design,
            self.list.filter(),
            note,
            self.list.visible_len(),
            self.games.len(),
        );
        match self.state.options.layout {
            Layout::List => {
                self.rows_height.set(shelf.height);
                self.list.render(shelf, ctx.buf, ctx.design, true);
            }
            Layout::Shelf => {
                self.rows_height.set((shelf.height / CARD_HEIGHT).max(1));
                self.draw_shelf(ctx.buf, shelf, ctx.design);
            }
        }
        if let Some(hero) = hero {
            let game = self.selected_game().cloned();
            self.draw_hero(ctx.buf, hero, ctx.design, game.as_ref());
        }
    }

    fn options_key(&mut self, key: KeyEvent) -> Outcome {
        let row = match &self.mode {
            Mode::Options(state) => state.row,
            _ => return Outcome::Continue,
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Mode::Options(state) = &mut self.mode {
                    state.row = (row + OPTION_ROWS - 1) % OPTION_ROWS;
                }
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Mode::Options(state) = &mut self.mode {
                    state.row = (row + 1) % OPTION_ROWS;
                }
                Outcome::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.cycle_option(row, -1);
                Outcome::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                self.cycle_option(row, 1);
                Outcome::Consumed
            }
            KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => {
                self.mode = Mode::Library;
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    fn manual_key(&mut self, key: KeyEvent) -> Outcome {
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
        Outcome::Consumed
    }
}

/// One shelf card: a cartridge, the title with the platform on the right, the
/// core/size/save line, and the path. The selected card is filled with the
/// selection colour so the cursor reads at a glance.
#[allow(clippy::too_many_arguments)]
fn draw_game_card(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    design: &Design,
    game: &Game,
    selected: bool,
    covers: bool,
    core_present: bool,
    now: i64,
) {
    let background = if selected {
        design.color(ColorRole::Selection)
    } else {
        design.color(ColorRole::Bg)
    };
    buf.set_style(area, Style::default().bg(background));
    let accent = if selected {
        ColorRole::Accent
    } else {
        ColorRole::SurfaceAlt
    };
    buf.set_string(
        area.x,
        area.y,
        "▌",
        Style::default().fg(design.color(accent)).bg(background),
    );
    let mut text_x = area.x + 2;
    if covers {
        art::draw_cartridge(
            buf,
            text_x,
            area.y,
            design,
            art::initial(&game.title),
            selected,
        );
        text_x += 6;
    }
    let text_width = usize::from(area.right().saturating_sub(text_x + 1));
    let title_style = Style::default()
        .fg(design.color(if selected {
            ColorRole::Accent
        } else {
            ColorRole::Text
        }))
        .bg(background)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default()
        .fg(design.color(ColorRole::Dim))
        .bg(background);
    let right = short_platform(&game.platform, &game.platform_title);
    let right_width = UnicodeWidthStr::width(right.as_str());
    let star = if game.favorite { "★ " } else { "" };
    let title_width = text_width.saturating_sub(right_width + 2);
    buf.set_string(
        text_x,
        area.y,
        &art::fit(&format!("{star}{}", game.title), title_width.max(4)),
        title_style,
    );
    if right_width + 2 < text_width {
        buf.set_string(
            area.right().saturating_sub(1 + right_width as u16),
            area.y,
            &right,
            dim,
        );
    }
    let mut detail = game
        .core
        .clone()
        .unwrap_or_else(|| "LazyOS game".to_string());
    if game.size > 0 {
        detail.push_str(&format!(" · {}", art::human_size(game.size)));
    }
    if game.save {
        detail.push_str(" · save");
    }
    if game.last > 0 {
        detail.push_str(&format!(" · {}", art::ago(game.last, now)));
    }
    let detail_style = if core_present {
        dim
    } else {
        Style::default()
            .fg(design.color(ColorRole::Danger))
            .bg(background)
    };
    if !core_present {
        detail.push_str(" · core missing");
    }
    buf.set_string(
        text_x,
        area.y + 1,
        &art::fit(&detail, text_width),
        detail_style,
    );
    if let Some(path) = game_path(game) {
        buf.set_string(text_x, area.y + 2, &art::fit(&path, text_width), dim);
    }
}

/// `~/Games/neogeo/mslug.zip`: the folder a person would open in Files.
fn game_path(game: &Game) -> Option<String> {
    match (game.kind, game.rom.as_deref(), game.root.as_deref()) {
        (GameKind::Retro, Some(rom), _) => Some(pretty_path(rom)),
        (GameKind::Web, _, Some(root)) => Some(pretty_path(root)),
        _ => None,
    }
}

fn pretty_path(path: &Path) -> String {
    let home = paths::home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn is_arcade(platform: &str) -> bool {
    platform::PLATFORMS
        .iter()
        .find(|row| row.id == platform)
        .is_some_and(|row| row.folders.is_some())
}

/// The rail's label: the platform's own short name, not its table title. A
/// pill has one line, and "Nintendo Entertainment System" does not fit.
fn rail_title(tab: &PublicPlatform) -> String {
    match tab.id.as_str() {
        "all" => "All".to_string(),
        "recent" => "Recent".to_string(),
        "favorites" => "★".to_string(),
        "web" => "LazyOS".to_string(),
        id => id.to_ascii_uppercase(),
    }
}

/// The right side of a card: `SNES`, `NEOGEO`, `LazyOS`.
fn short_platform(id: &str, title: &str) -> String {
    match id {
        "web" => "LazyOS".to_string(),
        "" => title.to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// The four options and their current values, in the order the card shows them.
fn option_rows(options: &Options) -> [(&'static str, String); OPTION_ROWS] {
    [
        ("Sort", sort_label(options.sort).to_string()),
        ("Layout", layout_label(options.layout).to_string()),
        ("Covers", on_off(options.covers)),
        ("Hide missing cores", on_off(options.hide_missing)),
    ]
}

fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Title => "Title",
        Sort::Recent => "Recent",
        Sort::Platform => "Platform",
    }
}

fn layout_label(layout: Layout) -> &'static str {
    match layout {
        Layout::Shelf => "Shelf",
        Layout::List => "List",
    }
}

fn on_off(on: bool) -> String {
    if on { "On" } else { "Off" }.to_string()
}

/// The options card, on a cleared body: a modal replaces the body, it never
/// floats over the live shelf (AGENTS rule 11).
fn draw_options(ctx: &mut BodyCtx, state: &OptionsState, options: &Options) {
    ctx.buf.set_style(
        ctx.area,
        Style::default().bg(ctx.design.color(ColorRole::Bg)),
    );
    let card = centered(ctx.area, 50, 9);
    draw_card_with(
        ctx.buf,
        card,
        ctx.design,
        Some("Options"),
        ColorRole::Accent,
        ctx.design.style(TextStyle::Title),
    );
    let inner = Rect {
        x: card.x + 3,
        y: card.y + 2,
        width: card.width.saturating_sub(6),
        height: card.height.saturating_sub(4),
    };
    let rows = option_rows(options);
    for (index, (label, value)) in rows.iter().enumerate() {
        let y = inner.y + index as u16;
        if y >= inner.bottom() {
            break;
        }
        let selected = index == state.row;
        let row = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        let background = if selected {
            ctx.buf.set_style(
                row,
                Style::default().bg(ctx.design.color(ColorRole::Selection)),
            );
            ctx.design.color(ColorRole::Selection)
        } else {
            ctx.design.color(ColorRole::Bg)
        };
        let label_style = Style::default()
            .fg(ctx.design.color(if selected {
                ColorRole::Accent
            } else {
                ColorRole::Text
            }))
            .bg(background)
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let value_style = Style::default()
            .fg(ctx.design.color(ColorRole::AccentAlt))
            .bg(background);
        let arrow_style = Style::default()
            .fg(ctx.design.color(ColorRole::Dim))
            .bg(background);
        ctx.buf.set_string(inner.x, y, label, label_style);
        let arrows = "‹  ›";
        let arrows_x = inner
            .right()
            .saturating_sub(UnicodeWidthStr::width(arrows) as u16);
        let value_x = arrows_x.saturating_sub(UnicodeWidthStr::width(value.as_str()) as u16 + 2);
        ctx.buf.set_string(value_x, y, value, value_style);
        ctx.buf.set_string(arrows_x, y, arrows, arrow_style);
    }
    let hint = "↑/↓ row · ←/→ change · Esc back";
    let hint_y = card.bottom().saturating_sub(2);
    ctx.buf.set_string(
        card.x + 3,
        hint_y,
        &art::fit(hint, usize::from(card.width.saturating_sub(6))),
        ctx.design.style(TextStyle::Dim),
    );
}

/// The manual, drawn over the whole card.
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
        if let Mode::Options(state) = &self.mode {
            let options = &self.state.options;
            draw_options(ctx, state, options);
            return;
        }
        self.draw_library(ctx);
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match self.mode {
            Mode::Options(_) => return self.options_key(key),
            Mode::Manual(_) => return self.manual_key(key),
            Mode::Library => {}
        }
        let typing = self.list.filter().is_empty()
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
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
            KeyCode::Char('f') if typing => {
                self.toggle_favorite();
                return Outcome::Consumed;
            }
            KeyCode::Char('s') if typing => {
                self.reload();
                return Outcome::Consumed;
            }
            KeyCode::Char('m') if typing => {
                self.open_manual();
                return Outcome::Consumed;
            }
            KeyCode::Char('o') if typing => {
                self.open_options();
                return Outcome::Consumed;
            }
            KeyCode::Char('r') if typing => {
                self.reveal();
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
                self.scroll.set(0);
                return Outcome::Consumed;
            }
            _ => {}
        }
        // The cursor moved: flash the card that got it.
        let before = self.list.cursor();
        let outcome = match self.list.handle(key, self.rows_height.get(), design) {
            ListAction::Activated(index) => self.activate(index),
            _ => Outcome::Continue,
        };
        if self.list.cursor() != before {
            self.flash_at = Some(Instant::now());
        }
        outcome
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if !matches!(self.mode, Mode::Library) {
            return Outcome::Continue;
        }
        let Some(index) = row else {
            return Outcome::Continue;
        };
        if self.list.selected_index() == Some(index) {
            return self.activate(index);
        }
        if let Some(position) = self.list.position_of(index) {
            self.list.set_cursor(position);
            self.flash_at = Some(Instant::now());
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        if !matches!(self.mode, Mode::Library) {
            return None;
        }
        let (_, shelf, _) = Self::content_rects(area)?;
        if !shelf.contains(ratatui::layout::Position::new(x, y)) {
            return None;
        }
        match self.state.options.layout {
            Layout::List => self.list.hit_row(shelf, x, y, design),
            Layout::Shelf => {
                let position = self.scroll.get() + usize::from((y - shelf.y) / CARD_HEIGHT);
                self.list.visible_index(position)
            }
        }
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(match self.mode {
            Mode::Library => &self.keybar,
            Mode::Manual(_) => &self.manual_keybar,
            Mode::Options(_) => &self.options_keybar,
        })
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.flash.is_some() || self.flash_at.is_some() {
            Some(Duration::from_millis(16))
        } else {
            None
        }
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.mode {
            Mode::Library => "list",
            Mode::Manual(_) => "manual",
            Mode::Options(_) => "options",
        })
    }

    /// The shelf wants a wide window (the hero is the point); the dense list is
    /// narrower. The shell resizes the floating window when this changes.
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some(match self.state.options.layout {
            Layout::Shelf => SHELF_SIZE,
            Layout::List => LIST_SIZE,
        })
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
            "Put a ROM in the folder for its platform: the file extension chooses \
             the platform, not the folder name. The library creates one folder per \
             platform (and bios/) the first time it opens, and writes a README in \
             ~/Games. Hidden folders and art, bios, covers, saves, states, system \
             and web are skipped, and a file smaller than the platform's floor is \
             not a ROM — that is what keeps a README.md off the Genesis shelf."
                .to_string(),
        ),
        Block::Text(format!(
            "This session scans: {scanned}\n\
             Somewhere else: LAZYOS_GAMES_ROM_DIRS=/media/usb/games:~/MoreGames"
        )),
        Block::Text(
            "A .zip is not unpacked: unzip it first — except arcade and Neo Geo, \
             whose romsets stay zipped and are read as they are. Put those in \
             ~/Games/neogeo/, cps1/, cps2/, cps3/, pgm/ or arcade/, with the BIOS \
             zips in ~/Games/bios/. A .zip anywhere else is not a game. For a \
             multi-disc PlayStation game, list the .cue files in an .m3u."
                .to_string(),
        ),
        Block::Section("Platforms"),
        Block::Text(
            "The extension picks the row; a romset is claimed by the folder it is \
             in, and the core named beside it must be installed."
                .to_string(),
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
            "1. Copy the ROM into its folder (an arcade romset stays zipped).\n\
             2. Press s here to rescan, or reopen the library.\n\
             3. ← / → choose a platform, ↑ / ↓ a game, Enter plays it.\n\
             4. Esc in the game comes back here; F1 opens RetroArch's menu."
                .to_string(),
        ),
        Block::Section("Keys"),
        Block::Code(
            "← / → — previous / next platform\n\
             ↑ / ↓ — move through the games\n\
             Enter — play the selected game\n\
             type — filter the platform by title or platform\n\
             o — options: sort, layout, covers, hide missing cores\n\
             m — this manual\n\
             r — open the game's folder in Files\n\
             f — favourite / unfavourite\n\
             s — rescan ~/Games\n\
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
             A ROM does not appear — check its folder and extension, then press s"
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
/// says "(core missing)" when the .so is not installed.
///
/// Columns are separated by an em dash rather than padded with spaces: the
/// manual wraps to the window, and wrapping collapses runs of spaces.
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
    pretty_path(path)
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
    // The folders are the feature: a person opening Games for the first time
    // gets the shelf ready, one folder per platform, and a README that says
    // what goes where. A read-only home is a note, not a failure to open.
    if let Err(error) = library::ensure_layout(&screen.options) {
        screen.note = Some(format!("Cannot create the game folders: {error}"));
    }
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
        assert!(text.contains("cps1/"));
        assert!(text.contains("Press s here to rescan"));
        assert!(text.contains("Enter plays it"));
        assert!(text.contains("~/.local/share/lazyos/games/saves"));
        assert!(text.contains("r — open the game's folder in Files"));
        assert!(text.contains("o — options"));
    }

    #[test]
    fn a_paragraph_wraps_at_the_width() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("one\ntwo", 20), vec!["one", "two"]);
        assert_eq!(wrap("", 20), vec![""]);
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

        let before = screen.list.selected_index();
        screen.key(key(KeyCode::Down), &design);
        assert_eq!(screen.list.selected_index(), before);

        screen.key(key(KeyCode::Esc), &design);
        assert!(matches!(screen.mode, Mode::Library));
        assert_eq!(screen.phase(), Some("list"));
    }

    #[test]
    fn o_opens_the_options_and_escape_goes_back() {
        let mut screen = GamesScreen::new();
        let design = Design::for_session(None);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        screen.key(key(KeyCode::Char('o')), &design);
        assert!(matches!(screen.mode, Mode::Options(_)));
        assert_eq!(screen.phase(), Some("options"));
        assert_eq!(screen.keybar().map(<[KeyHint]>::len), Some(3));

        // Moving inside the card does not touch the shelf.
        let before = screen.list.selected_index();
        screen.key(key(KeyCode::Down), &design);
        assert_eq!(screen.list.selected_index(), before);

        screen.key(key(KeyCode::Esc), &design);
        assert!(matches!(screen.mode, Mode::Library));
    }

    #[test]
    fn the_options_are_four_and_name_the_current_value() {
        let rows = option_rows(&Options::default());
        assert_eq!(rows.len(), OPTION_ROWS);
        assert_eq!(rows[0], ("Sort", "Title".to_string()));
        assert_eq!(rows[1], ("Layout", "Shelf".to_string()));
        assert_eq!(rows[2], ("Covers", "On".to_string()));
        assert_eq!(rows[3], ("Hide missing cores", "Off".to_string()));
    }

    #[test]
    fn a_wide_window_gets_a_hero_and_a_narrow_one_does_not() {
        let (inner, shelf, hero) = GamesScreen::content_rects(Rect::new(0, 0, 140, 40)).unwrap();
        assert!(hero.is_some(), "a wide window must have a hero");
        let hero = hero.unwrap();
        assert!(
            shelf.right() < hero.x,
            "the shelf and the hero must not touch"
        );
        assert_eq!(shelf.y, hero.y);
        assert!(inner.contains(ratatui::layout::Position::new(shelf.x, shelf.y)));

        let (_, shelf, hero) = GamesScreen::content_rects(Rect::new(0, 0, 90, 30)).unwrap();
        assert!(hero.is_none(), "a narrow window keeps the shelf only");
        assert_eq!(shelf.width, 86);
    }

    #[test]
    fn the_shelf_renders_from_a_narrow_window_to_fullscreen() {
        let mut screen = all_tab();
        for (width, height) in [(90, 30), (150, 43), (170, 57)] {
            let text = render(&mut screen, width, height);
            if std::env::var_os("LAZYOS_GAMES_SHOW").is_some() {
                println!("--- {width}x{height} ---\n{text}");
            }
            // The frame is the same card the other screens use, and the rail is
            // there whether or not the library has games.
            assert!(text.contains("Games"), "no card at {width}x{height}");
            assert!(text.contains("All"), "no rail at {width}x{height}");
            assert!(
                text.contains("Enter play"),
                "no hint line at {width}x{height}"
            );
            if let Some(game) = screen.selected_game() {
                assert!(
                    text.contains(&art::fit(&game.title, 30)),
                    "the selected game is missing at {width}x{height}"
                );
                // The hero (and its button) only exists in a wide window.
                if width >= HERO_MIN_WIDTH {
                    assert!(text.contains("PLAY"), "no hero button at {width}x{height}");
                }
            } else {
                assert!(text.contains("No games in this view"));
            }
        }
    }

    /// A screen on the All tab, cursor at the top, no filter: the layout tests
    /// must not depend on the state file of the machine running them.
    fn all_tab() -> GamesScreen {
        let mut screen = GamesScreen::new();
        // The layout tests must not inherit the options of the machine running
        // them: this developer's library is on the list right now.
        screen.state.options = Options::default();
        screen.tab = 0;
        screen.state.filter = "all".to_string();
        screen.list.set_filter("");
        screen.rebuild();
        screen.list.set_cursor(0);
        screen
    }

    fn render(screen: &mut GamesScreen, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let design = Design::for_session(None);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                let mut ctx = BodyCtx {
                    area,
                    buf: frame.buffer_mut(),
                    design: &design,
                };
                screen.draw(&mut ctx);
            })
            .unwrap();
        lazyos_ui::screen(terminal.backend().buffer())
    }

    #[test]
    fn the_options_card_renders_and_names_the_four_rows() {
        let mut screen = all_tab();
        screen.mode = Mode::Options(OptionsState { row: 0 });
        for (width, height) in [(100, 30), (150, 43)] {
            let text = render(&mut screen, width, height);
            if std::env::var_os("LAZYOS_GAMES_SHOW").is_some() {
                println!("{text}");
            }
            assert!(
                text.contains("Options"),
                "no options card at {width}x{height}"
            );
            for row in [
                "Sort",
                "Title",
                "Layout",
                "Shelf",
                "Covers",
                "On",
                "Hide missing cores",
            ] {
                assert!(text.contains(row), "{row:?} is missing at {width}x{height}");
            }
            assert!(text.contains("Esc back"));
        }
    }

    #[test]
    fn the_list_layout_renders_the_same_games() {
        let mut screen = all_tab();
        screen.state.options.layout = Layout::List;
        screen.rebuild_keep_selection();
        let text = render(&mut screen, 100, 30);
        if std::env::var_os("LAZYOS_GAMES_SHOW").is_some() {
            println!("{text}");
        }
        assert!(text.contains("Games"));
        if let Some(game) = screen.selected_game() {
            assert!(text.contains(&game.title));
        }
    }

    #[test]
    fn the_rail_names_platforms_shortly() {
        let tab = |id: &str, title: &str| PublicPlatform {
            id: id.to_string(),
            title: title.to_string(),
            count: 1,
        };
        assert_eq!(rail_title(&tab("all", "All games")), "All");
        assert_eq!(rail_title(&tab("favorites", "Favourites")), "★");
        assert_eq!(rail_title(&tab("web", "LazyOS games")), "LazyOS");
        assert_eq!(rail_title(&tab("snes", "Super Nintendo")), "SNES");
        assert_eq!(rail_title(&tab("neogeo", "Neo Geo")), "NEOGEO");
    }
}
