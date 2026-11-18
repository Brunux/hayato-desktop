//! The GTK window: a WebKit view, the library page, and the bridge.
//!
//! There are two modes. `Library` shows the grid and, when a game is chosen,
//! starts a separate `--play` process and exits. `Game` is that process: it
//! shows one web game fullscreen and, when it closes, `exec`s the library back.
//! The two are never alive together, which is the point on a 4 GB machine.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    HardwareAccelerationPolicy, NetworkSession, Settings, UserContentInjectedFrames,
    UserContentManager, UserScript, UserScriptInjectionTime, WebView,
};

use crate::library::{self, GameKind, ScanOptions, State};
use crate::scheme::Handler;
use crate::{launch, paths};

const APP_ID: &str = "org.lazyos.Games";
const HOST_JS: &str = include_str!("host.js");

#[derive(Clone)]
enum Mode {
    Library,
    Game,
}

struct Ctx {
    options: ScanOptions,
    state: Mutex<State>,
    handler: Arc<Handler>,
}

impl Ctx {
    fn new() -> Self {
        let options = ScanOptions::from_env();
        // RetroArch writes saves, states and screenshots into these; creating
        // them here means the first game does not depend on the emulator
        // creating a parent directory it only knows as a string.
        for directory in [
            paths::data_dir().join("saves"),
            paths::state_dir().join("states"),
            paths::data_dir().join("shots"),
        ] {
            let _ = std::fs::create_dir_all(directory);
        }
        let state = library::load_state(&options.state_path());
        let library = library::scan(&options, &state);
        let handler = Handler::new(library, options.system_dir.clone(), paths::theme_css());
        Self {
            options,
            state: Mutex::new(state),
            handler,
        }
    }

    fn save_state(&self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Err(error) = library::save_state(&self.options.state_path(), &state) {
            eprintln!("lazyos-games: cannot save the library state: {error}");
        }
    }
}

pub fn run_library() -> std::process::ExitCode {
    if let Err(error) = gtk::init() {
        eprintln!("lazyos-games: GTK could not start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let ctx = Arc::new(Ctx::new());
    let app = application();
    let library_ctx = Arc::clone(&ctx);
    app.connect_activate(move |app| present_library(app, Arc::clone(&library_ctx)));
    app.run_with_args::<&str>(&[]);
    std::process::ExitCode::SUCCESS
}

pub fn run_play(id: &str) -> std::process::ExitCode {
    let ctx = Arc::new(Ctx::new());
    let game = {
        let library = ctx.handler.library();
        library.find(id).cloned()
    };
    let Some(game) = game else {
        eprintln!("lazyos-games: no game with id {id}");
        return std::process::ExitCode::FAILURE;
    };

    match game.kind {
        GameKind::Retro => {
            let libretro_dir = ctx.handler.library().libretro_dir.clone();
            if let Err(error) = launch::run_retro(&game, &libretro_dir) {
                eprintln!("lazyos-games: could not start {}: {error}", game.title);
            }
            launch::return_to_library()
        }
        GameKind::Web => {
            let Some(uri) = launch::web_uri(&game) else {
                eprintln!("lazyos-games: {} has no entry page", game.title);
                launch::return_to_library()
            };
            if let Err(error) = gtk::init() {
                eprintln!("lazyos-games: GTK could not start: {error}");
                launch::return_to_library();
            }
            let app = application();
            let title = game.title.clone();
            let game_ctx = Arc::clone(&ctx);
            app.connect_activate(move |app| {
                present_game(app, Arc::clone(&game_ctx), uri.clone(), title.clone())
            });
            app.run_with_args::<&str>(&[]);
            launch::return_to_library()
        }
    }
}

fn application() -> gtk::Application {
    gtk::Application::builder()
        .application_id(APP_ID)
        // The library and the game it starts overlap for a moment; a unique
        // application would make the second one activate the first and exit.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build()
}

fn present_library(app: &gtk::Application, ctx: Arc<Ctx>) {
    let window = build_window(app, "Games");
    let webview = build_webview(app, &window, ctx, Mode::Library);
    finish_window(&window, &webview, app, Mode::Library);
    webview.load_uri("lazyos-game://library/index.html");
    install_gamepad(app, &webview, &Mode::Library);
}

fn present_game(app: &gtk::Application, ctx: Arc<Ctx>, uri: String, title: String) {
    let window = build_window(app, &title);
    let webview = build_webview(app, &window, ctx, Mode::Game);
    finish_window(&window, &webview, app, Mode::Game);
    webview.load_uri(&uri);
    install_gamepad(app, &webview, &Mode::Game);
}

fn build_window(app: &gtk::Application, title: &str) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .default_width(1280)
        .default_height(720)
        .build();
    paint_window(&window);
    window
}

/// The window and the web view are painted in the active palette before the
/// page has loaded, so a slow first frame is not a white rectangle in a base03
/// session.
fn paint_window(_window: &gtk::ApplicationWindow) {
    let background = lazyos_theme::Theme::parse(lazyos_theme::builtin::SOLARIZED)
        .map(|theme| theme.colors.background)
        .unwrap_or(lazyos_theme::Hex(lazyos_theme::BASE03));
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        "window {{ background-color: {}; }}",
        background.hashed()
    ));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// The theme the session is actually using, for the frame before the page
/// loads. Solarized Dark is the fallback only when the state file or the
/// built-in table cannot answer.
fn active_theme() -> lazyos_theme::Theme {
    let paths = lazyos_theme::Paths::from_env();
    lazyos_theme::store::active(&paths)
        .and_then(|slug| lazyos_theme::builtin::get(&slug))
        .and_then(|text| lazyos_theme::Theme::parse(text).ok())
        .unwrap_or_else(|| {
            lazyos_theme::Theme::parse(lazyos_theme::builtin::SOLARIZED)
                .expect("the built-in Solarized theme parses")
        })
}

fn build_webview(
    app: &gtk::Application,
    window: &gtk::ApplicationWindow,
    ctx: Arc<Ctx>,
    mode: Mode,
) -> WebView {
    let debug = std::env::var_os("LAZYOS_GAMES_DEBUG").is_some();

    let settings = Settings::new();
    settings.set_enable_javascript(true);
    settings.set_enable_webgl(true);
    settings.set_enable_webaudio(true);
    settings.set_enable_media(true);
    settings.set_enable_fullscreen(true);
    settings.set_enable_developer_extras(debug);
    settings.set_enable_write_console_messages_to_stdout(debug);
    if std::env::var_os("LAZYOS_GAMES_NO_ACCEL").is_some() {
        settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
    }

    let data = paths::data_dir();
    let cache = paths::cache_dir();
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    let network = NetworkSession::new(data.to_str(), cache.to_str());

    let manager = UserContentManager::new();
    manager.register_script_message_handler("lazyos", None);
    let script = UserScript::new(
        HOST_JS,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    manager.add_script(&script);

    let webview = WebView::builder()
        .network_session(&network)
        .user_content_manager(&manager)
        .settings(&settings)
        .build();

    let background = active_theme().colors.background.0;
    webview.set_background_color(&gtk::gdk::RGBA::new(
        f32::from(background.r) / 255.0,
        f32::from(background.g) / 255.0,
        f32::from(background.b) / 255.0,
        1.0,
    ));

    if let Some(context) = webview.web_context() {
        ctx.handler.install(&context);
    } else {
        eprintln!("lazyos-games: the web view has no context; the game scheme is unavailable");
    }

    connect_bridge(&manager, app, &webview, ctx, mode.clone());
    window.set_child(Some(&webview));
    webview
}

/// The menu's controller listener. It reads `/dev/input/event*` directly
/// through evdev on its own thread: WebKit's Gamepad API is what the games
/// use, but the menu must work with a USB or Bluetooth pad even when the web
/// engine does not see it. The thread sends actions to this window's main
/// loop; nothing is a daemon, and a slow device scan cannot delay the frame.
enum PadMessage {
    Action(&'static str),
    Connected(String),
    Quit,
}

struct Pad {
    device: evdev::Device,
    x: (i32, i32),
    y: (i32, i32),
}

fn install_gamepad(app: &gtk::Application, webview: &WebView, mode: &Mode) {
    let library = matches!(mode, Mode::Library);
    let (sender, receiver) = std::sync::mpsc::channel::<PadMessage>();

    std::thread::spawn(move || {
        let mut pads: Vec<Pad> = Vec::new();
        let mut known: BTreeSet<PathBuf> = BTreeSet::new();
        let mut last_scan = Instant::now() - Duration::from_secs(5);
        let mut stick = (0.0_f32, 0.0_f32);
        let mut direction = "";
        let mut next = Instant::now();
        let mut combo = (false, false);

        loop {
            if last_scan.elapsed() >= Duration::from_secs(2) {
                last_scan = Instant::now();
                for (path, device) in evdev::enumerate() {
                    if !known.insert(path) {
                        continue;
                    }
                    let gamepad = device
                        .supported_keys()
                        .map(|keys| keys.contains(evdev::KeyCode::BTN_SOUTH))
                        .unwrap_or(false);
                    if !gamepad {
                        continue;
                    }
                    let mut x = (-32768, 32767);
                    let mut y = (-32768, 32767);
                    if let Ok(info) = device.get_absinfo() {
                        for (axis, abs) in info {
                            match axis {
                                evdev::AbsoluteAxisCode::ABS_X => {
                                    x = (abs.minimum(), abs.maximum());
                                }
                                evdev::AbsoluteAxisCode::ABS_Y => {
                                    y = (abs.minimum(), abs.maximum());
                                }
                                _ => {}
                            }
                        }
                    }
                    let name = device.name().unwrap_or("gamepad").to_string();
                    if sender.send(PadMessage::Connected(name)).is_err() {
                        return;
                    }
                    pads.push(Pad { device, x, y });
                }
            }

            for pad in &mut pads {
                let Ok(events) = pad.device.fetch_events() else {
                    continue;
                };
                for event in events {
                    match event.destructure() {
                        evdev::EventSummary::Key(_, key, 1) => {
                            // Select+Start is the way out, not "play"; do not
                            // forward the Start that completes the combo.
                            let combo_start = key == evdev::KeyCode::BTN_START && combo.0;
                            if library
                                && !combo_start
                                && let Some(action) = key_action(key)
                                && sender.send(PadMessage::Action(action)).is_err()
                            {
                                return;
                            }
                            match key {
                                evdev::KeyCode::BTN_SELECT => combo.0 = true,
                                evdev::KeyCode::BTN_START => combo.1 = true,
                                _ => {}
                            }
                            if combo.0 && combo.1 {
                                let _ = sender.send(PadMessage::Quit);
                            }
                        }
                        evdev::EventSummary::Key(_, key, 0) => match key {
                            evdev::KeyCode::BTN_SELECT => combo.0 = false,
                            evdev::KeyCode::BTN_START => combo.1 = false,
                            _ => {}
                        },
                        evdev::EventSummary::AbsoluteAxis(_, axis, value) => match axis {
                            evdev::AbsoluteAxisCode::ABS_X => stick.0 = normalise(value, pad.x),
                            evdev::AbsoluteAxisCode::ABS_Y => stick.1 = normalise(value, pad.y),
                            evdev::AbsoluteAxisCode::ABS_HAT0X if library && value != 0 => {
                                let action = if value < 0 { "left" } else { "right" };
                                if sender.send(PadMessage::Action(action)).is_err() {
                                    return;
                                }
                            }
                            evdev::AbsoluteAxisCode::ABS_HAT0Y if library && value != 0 => {
                                let action = if value < 0 { "up" } else { "down" };
                                if sender.send(PadMessage::Action(action)).is_err() {
                                    return;
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }

            if library {
                let now = Instant::now();
                let dir = if stick.0 < -0.55 {
                    "left"
                } else if stick.0 > 0.55 {
                    "right"
                } else if stick.1 < -0.55 {
                    "up"
                } else if stick.1 > 0.55 {
                    "down"
                } else {
                    ""
                };
                if dir != direction {
                    direction = dir;
                    next = now + Duration::from_millis(260);
                    if !dir.is_empty() && sender.send(PadMessage::Action(dir)).is_err() {
                        return;
                    }
                } else if !dir.is_empty() && now >= next {
                    next = now + Duration::from_millis(170);
                    if sender.send(PadMessage::Action(dir)).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    });

    let app = app.clone();
    let webview = webview.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(message) = receiver.try_recv() {
            match message {
                PadMessage::Action(action) => gamepad_call(&webview, action),
                PadMessage::Connected(name) => {
                    let script = format!(
                        "window.LazyOSGamepadConnected && window.LazyOSGamepadConnected({})",
                        serde_json::to_string(&name).unwrap_or_default()
                    );
                    webview.evaluate_javascript(
                        &script,
                        None,
                        None,
                        None::<&gtk::gio::Cancellable>,
                        |_| {},
                    );
                }
                PadMessage::Quit => {
                    app.quit();
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn normalise(value: i32, (min, max): (i32, i32)) -> f32 {
    if max <= min {
        return 0.0;
    }
    (2.0 * (value - min) as f32 / (max - min) as f32) - 1.0
}

fn key_action(key: evdev::KeyCode) -> Option<&'static str> {
    match key {
        evdev::KeyCode::BTN_DPAD_LEFT => Some("left"),
        evdev::KeyCode::BTN_DPAD_RIGHT => Some("right"),
        evdev::KeyCode::BTN_DPAD_UP => Some("up"),
        evdev::KeyCode::BTN_DPAD_DOWN => Some("down"),
        evdev::KeyCode::BTN_SOUTH => Some("a"),
        evdev::KeyCode::BTN_EAST => Some("b"),
        evdev::KeyCode::BTN_NORTH => Some("y"),
        evdev::KeyCode::BTN_WEST => Some("x"),
        evdev::KeyCode::BTN_TL => Some("l1"),
        evdev::KeyCode::BTN_TR => Some("r1"),
        evdev::KeyCode::BTN_START => Some("start"),
        _ => None,
    }
}

fn gamepad_call(webview: &WebView, action: &str) {
    let script = format!(
        "window.LazyOSGamepad && window.LazyOSGamepad({})",
        serde_json::to_string(action).unwrap_or_default()
    );
    webview.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
}

fn finish_window(
    window: &gtk::ApplicationWindow,
    webview: &WebView,
    app: &gtk::Application,
    mode: Mode,
) {
    install_keys(window, app, &mode);
    window.present();
    if std::env::var_os("LAZYOS_GAMES_WINDOWED").is_none() {
        window.fullscreen();
    }
    webview.grab_focus();
}

fn install_keys(window: &gtk::ApplicationWindow, app: &gtk::Application, mode: &Mode) {
    let controller = gtk::EventControllerKey::new();
    let app = app.clone();
    let key_window = window.clone();
    let library = matches!(mode, Mode::Library);
    controller.connect_key_pressed(move |_, key, _, state| {
        if key == gtk::gdk::Key::F11 {
            if key_window.is_fullscreen() {
                key_window.unfullscreen();
            } else {
                key_window.fullscreen();
            }
            return glib::Propagation::Stop;
        }
        let control = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if control && (key == gtk::gdk::Key::q || key == gtk::gdk::Key::Q) {
            app.quit();
            return glib::Propagation::Stop;
        }
        // In a game, Escape belongs to the game. In the library it is the way
        // out when the page's own key handling is the thing that broke.
        if library && key == gtk::gdk::Key::Escape {
            app.quit();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

#[derive(serde::Deserialize)]
struct Message {
    cmd: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    value: Option<bool>,
}

fn connect_bridge(
    manager: &UserContentManager,
    app: &gtk::Application,
    webview: &WebView,
    ctx: Arc<Ctx>,
    mode: Mode,
) {
    let app = app.clone();
    let webview = webview.clone();
    manager.connect_script_message_received(Some("lazyos"), move |_, value| {
        let Some(text) = value.to_json(0) else {
            return;
        };
        let Ok(message) = serde_json::from_str::<Message>(text.as_str()) else {
            return;
        };
        match mode {
            Mode::Library => handle_library(&app, &webview, &ctx, message),
            // A game can leave; it cannot launch another game or rewrite the
            // library, even if its own JavaScript asks.
            Mode::Game => {
                if matches!(message.cmd.as_str(), "quit" | "back") {
                    app.quit();
                }
            }
        }
    });
}

fn handle_library(app: &gtk::Application, webview: &WebView, ctx: &Arc<Ctx>, message: Message) {
    match message.cmd.as_str() {
        "play" => {
            let Some(id) = message.id else {
                return;
            };
            let runnable = {
                let library = ctx.handler.library();
                library.find(&id).is_some_and(|game| {
                    game.kind == GameKind::Web
                        || game
                            .core
                            .as_deref()
                            .is_some_and(|core| library.core_present(core))
                })
            };
            if !runnable {
                return;
            }
            let now = library::now();
            {
                let mut state = ctx
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                state.last.insert(id.clone(), now);
            }
            {
                let mut library = ctx.handler.library();
                library.mark_played(&id, now);
            }
            ctx.handler.refresh();
            ctx.save_state();
            match launch::spawn_play(&id) {
                Ok(()) => app.quit(),
                Err(error) => {
                    eprintln!("lazyos-games: cannot start {id}: {error}");
                    let message = format!("Cannot start {id}: {error}");
                    let script = format!(
                        "window.LazyOSError && window.LazyOSError({})",
                        serde_json::to_string(&message).unwrap_or_default()
                    );
                    webview.evaluate_javascript(
                        &script,
                        None,
                        None,
                        None::<&gtk::gio::Cancellable>,
                        |_| {},
                    );
                }
            }
        }
        "favorite" => {
            let Some(id) = message.id else {
                return;
            };
            let on = message.value.unwrap_or(false);
            {
                let mut state = ctx
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if on {
                    state.favorites.insert(id.clone());
                } else {
                    state.favorites.remove(&id);
                }
            }
            {
                let mut library = ctx.handler.library();
                library.set_favorite(&id, on);
            }
            ctx.handler.refresh();
            ctx.save_state();
        }
        "rescan" => {
            let state = ctx
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            let library = library::scan(&ctx.options, &state);
            {
                *ctx.handler.library() = library;
            }
            ctx.handler.refresh();
            webview.reload();
        }
        "reveal" => {
            let Some(id) = message.id else {
                return;
            };
            let game = ctx.handler.library().find(&id).cloned();
            let Some(game) = game else {
                return;
            };
            if let Err(error) = launch::reveal(&game) {
                eprintln!("lazyos-games: cannot open the folder: {error}");
            }
        }
        "quit" | "back" => app.quit(),
        _ => {}
    }
}
