//! The GTK host: one web game, fullscreen, then back to the TUI library.
//!
//! The library itself is a Ratatui screen in `hayato-cli`. This binary exists
//! only for the part a terminal cannot do: a WebKitGTK window that gives an
//! HTML/CSS/JS game its own origin, Gamepad API, storage and fullscreen space.
//! Retro games never reach this code: `hayato-games --play retro:…` runs
//! RetroArch and returns, with no GTK initialized at all.

use std::sync::Arc;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    HardwareAccelerationPolicy, NetworkSession, Settings, UserContentInjectedFrames,
    UserContentManager, UserScript, UserScriptInjectionTime, WebView,
};

use hayato_games_core::library::{self, GameKind, ScanOptions};
use hayato_games_core::{launch, paths};

use crate::scheme::Handler;

const APP_ID: &str = "org.hayato.Games";
const HOST_JS: &str = include_str!("host.js");

pub fn run_play(id: &str) -> std::process::ExitCode {
    let options = ScanOptions::from_env();
    let state = library::load_state(&options.state_path());
    let library = library::scan(&options, &state);
    let Some(game) = library.find(id).cloned() else {
        eprintln!("hayato-games: no game with id {id}");
        return std::process::ExitCode::FAILURE;
    };

    match game.kind {
        GameKind::Retro => {
            let libretro_dir = library.libretro_dir.clone();
            if let Err(error) = launch::run_retro(&game, &libretro_dir) {
                eprintln!("hayato-games: could not start {}: {error}", game.title);
            }
            launch::return_to_library()
        }
        GameKind::Web => {
            let Some(uri) = launch::web_uri(&game) else {
                eprintln!("hayato-games: {} has no entry page", game.title);
                launch::return_to_library()
            };
            if let Err(error) = gtk::init() {
                eprintln!("hayato-games: GTK could not start: {error}");
                launch::return_to_library();
            }
            let handler = Handler::new(library, options.system_dir.clone(), paths::theme_css());
            let title = game.title.clone();
            let app = application();
            let game_handler = Arc::clone(&handler);
            let game_uri = uri.clone();
            let game_title = title.clone();
            app.connect_activate(move |app| {
                present_game(
                    app,
                    Arc::clone(&game_handler),
                    game_uri.clone(),
                    game_title.clone(),
                )
            });
            app.run_with_args::<&str>(&[]);
            launch::return_to_library()
        }
    }
}

fn application() -> gtk::Application {
    gtk::Application::builder()
        .application_id(APP_ID)
        // The host and the library are never alive together, but a second
        // `--play` from a desktop entry must not activate the first one and
        // exit without showing its game.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build()
}

fn present_game(app: &gtk::Application, handler: Arc<Handler>, uri: String, title: String) {
    let window = build_window(app, &title);
    let webview = build_webview(app, &window, handler);
    finish_window(&window, &webview, app);
    webview.load_uri(&uri);
}

fn build_window(app: &gtk::Application, title: &str) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .default_width(1280)
        .default_height(720)
        .build();
    paint_window();
    window
}

/// The window and the web view are painted in the active palette before the
/// page has loaded, so a slow first frame is not a white rectangle in a base03
/// session.
fn paint_window() {
    let background = active_theme().colors.background;
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
/// loads. Tokyo Night, the image's default, is the fallback only when the
/// state file or the built-in table cannot answer.
fn active_theme() -> hayato_theme::Theme {
    let paths = hayato_theme::Paths::from_env();
    hayato_theme::store::active(&paths)
        .and_then(|slug| hayato_theme::builtin::get(&slug))
        .and_then(|text| hayato_theme::Theme::parse(text).ok())
        .unwrap_or_else(fallback_theme)
}

fn fallback_theme() -> hayato_theme::Theme {
    hayato_theme::Theme::parse(hayato_theme::builtin::TOKYO_NIGHT)
        .expect("the built-in Tokyo Night theme parses")
}

fn build_webview(
    app: &gtk::Application,
    window: &gtk::ApplicationWindow,
    handler: Arc<Handler>,
) -> WebView {
    let debug = std::env::var_os("HAYATO_GAMES_DEBUG").is_some();

    let settings = Settings::new();
    settings.set_enable_javascript(true);
    settings.set_enable_webgl(true);
    settings.set_enable_webaudio(true);
    settings.set_enable_media(true);
    settings.set_enable_fullscreen(true);
    settings.set_enable_developer_extras(debug);
    settings.set_enable_write_console_messages_to_stdout(debug);
    if std::env::var_os("HAYATO_GAMES_NO_ACCEL").is_some() {
        settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);
    }

    let data = paths::data_dir();
    let cache = paths::cache_dir();
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    let network = NetworkSession::new(data.to_str(), cache.to_str());

    let manager = UserContentManager::new();
    manager.register_script_message_handler("hayato", None);
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
        handler.install(&context);
    } else {
        eprintln!("hayato-games: the web view has no context; the game scheme is unavailable");
    }

    connect_bridge(&manager, app);
    window.set_child(Some(&webview));
    webview
}

fn finish_window(window: &gtk::ApplicationWindow, webview: &WebView, app: &gtk::Application) {
    install_keys(window, app);
    window.present();
    if std::env::var_os("HAYATO_GAMES_WINDOWED").is_none() {
        window.fullscreen();
    }
    webview.grab_focus();
}

fn install_keys(window: &gtk::ApplicationWindow, app: &gtk::Application) {
    let controller = gtk::EventControllerKey::new();
    let app = app.clone();
    let key_window = window.clone();
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
        // Escape belongs to the game. The host script's Select+Start and
        // Ctrl+Q are the way out.
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

#[derive(serde::Deserialize)]
struct Message {
    cmd: String,
}

fn connect_bridge(manager: &UserContentManager, app: &gtk::Application) {
    let app = app.clone();
    manager.connect_script_message_received(Some("hayato"), move |_, value| {
        let Some(text) = value.to_json(0) else {
            return;
        };
        let Ok(message) = serde_json::from_str::<Message>(text.as_str()) else {
            return;
        };
        if matches!(message.cmd.as_str(), "quit" | "back") {
            app.quit();
        }
    });
}

#[cfg(test)]
mod tests {
    /// The frame before the page loads must wear the image's default (Tokyo
    /// Night), not the theme the previous release used: a fresh host has no
    /// active-theme state until the first session writes it.
    #[test]
    fn the_fallback_theme_is_tokyo_night() {
        let expected = hayato_theme::Theme::parse(hayato_theme::builtin::TOKYO_NIGHT)
            .expect("the built-in Tokyo Night theme parses");
        let fallback = super::fallback_theme();
        assert_eq!(fallback.roles(), expected.roles());
    }
}
