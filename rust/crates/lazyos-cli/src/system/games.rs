//! The game library: a Ratatui list over `~/Games` and the LazyOS HTML games.
//!
//! The library is the framework's own screen, so it looks and navigates like
//! every other LazyOS menu at any window size or fullscreen. The GTK + WebKit
//! binary `lazyos-games` is only the host that plays one HTML game; this screen
//! starts it (or RetroArch) and closes, and the host returns here when the game
//! ends.
//!
//! Left/Right are the tabs (All games, Recent, Favourites, each console,
//! LazyOS games); Up/Down move the list; Enter plays; Esc closes. That is the
//! same everywhere, and `lazyos gamepad` injects exactly those keys.

use std::cell::Cell;

use lazyos_games_core::launch;
use lazyos_games_core::library::{
    self, Game, GameKind, Library, PublicPlatform, ScanOptions, State,
};
use lazyos_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, ListAction, ListState, Outcome, Row,
    Screen, TextStyle, draw_card, draw_filter_bar,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

pub struct GamesScreen {
    options: ScanOptions,
    state: State,
    library: Library,
    tabs: Vec<PublicPlatform>,
    tab: usize,
    games: Vec<Game>,
    list: ListState,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    note: Option<String>,
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
            header: Header::new("LazyOS").subtitle("Games"),
            keybar: vec![
                KeyHint::new("Enter", "play"),
                KeyHint::new("←/→", "platform"),
                KeyHint::new("f", "favourite"),
                KeyHint::new("s", "rescan"),
                KeyHint::new("Esc", "close"),
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
}

impl Screen for GamesScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
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
            .unwrap_or("Enter play · ←/→ platform · f favourite · s rescan · Esc close");
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

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
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
        if let Some(row) = row {
            self.list.set_cursor(row);
            if let Some(index) = self.list.selected_index() {
                return self.activate(index);
            }
        }
        Outcome::Continue
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
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
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some("games")
    }

    /// The Pick shape's window. A library opened by the game host comes back
    /// through `foot` with no size of its own, so the screen asks for the same
    /// 900×600 the framework's host would have given it.
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((900, 600))
    }
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
