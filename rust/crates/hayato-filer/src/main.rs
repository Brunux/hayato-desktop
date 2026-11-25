//! `hayato-filer` — Minimalistic native Rust GTK4 file browser for Hayato OS.

mod fs;
mod state;
mod theme;
mod tools;
mod ui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk4::gdk::Display;
use gtk4::gio::ApplicationFlags;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};

const VERSION: &str = include_str!("../../../../VERSION");
const APP_ID: &str = "org.hayato.Filer";
static THEME_RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigusr1(_: libc::c_int) {
    THEME_RELOAD.store(true, Ordering::Relaxed);
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut initial: Option<PathBuf> = None;

    for arg in &args {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("{}", VERSION.trim());
                return glib::ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("hayato-filer {} — Hayato OS file browser", VERSION.trim());
                println!("usage: hayato-filer [PATH | file://URI]");
                println!("  --self-test   verify theme CSS, Trash & device parsers headlessly");
                println!("  --version     print version");
                return glib::ExitCode::SUCCESS;
            }
            "--self-test" => {
                let (css, _) = theme::load_css_string();
                assert!(!css.is_empty(), "filer CSS failed to render");
                let st = state::AppState::new(state::home_dir());
                assert!(st.history.len() == 1);
                println!("ok\thayato-filer {}\tcss={}B", VERSION.trim(), css.len());
                return glib::ExitCode::SUCCESS;
            }
            "--new-window" => {}
            other if !other.starts_with('-') => {
                initial = Some(resolve_initial_path(other));
            }
            _ => {}
        }
    }

    let app = Application::builder()
        .application_id(APP_ID)
        .flags(ApplicationFlags::HANDLES_OPEN | ApplicationFlags::NON_UNIQUE)
        .build();

    let start_dir = initial.unwrap_or_else(state::home_dir);
    let start_dir_open = start_dir.clone();

    app.connect_startup(move |_| {
        let provider = CssProvider::new();
        theme::apply_theme(&provider);
        if let Some(display) = Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        // Live SIGUSR1 theme reload when `hayato theme set` runs
        unsafe {
            libc::signal(libc::SIGUSR1, on_sigusr1 as *const () as libc::sighandler_t);
        }
        let p_reload = provider.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if THEME_RELOAD.swap(false, Ordering::Relaxed) {
                theme::apply_theme(&p_reload);
            }
            glib::ControlFlow::Continue
        });
    });

    app.connect_activate(move |app| {
        let win = ui::FilerWindow::build(app, start_dir.clone());
        win.window.present();
    });

    app.connect_open(move |app, files, _| {
        let dir = files
            .first()
            .and_then(|f| f.path())
            .map(|p| {
                if p.is_file() {
                    p.parent()
                        .map(PathBuf::from)
                        .unwrap_or_else(state::home_dir)
                } else {
                    p
                }
            })
            .unwrap_or_else(|| start_dir_open.clone());
        let win = ui::FilerWindow::build(app, dir);
        win.window.present();
    });

    app.run_with_args::<&str>(&[])
}

fn resolve_initial_path(raw: &str) -> PathBuf {
    let p = if let Some(rest) = raw.strip_prefix("file://") {
        PathBuf::from(fs::trash::percent_decode(rest))
    } else if let Some(rest) = raw.strip_prefix("~/") {
        state::home_dir().join(rest)
    } else if raw == "~" {
        state::home_dir()
    } else {
        PathBuf::from(raw)
    };
    if p.is_file() {
        p.parent()
            .map(PathBuf::from)
            .unwrap_or_else(state::home_dir)
    } else if p.is_dir() {
        p
    } else {
        state::home_dir()
    }
}
