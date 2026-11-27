//! `hayato-cassette` (alias `hayato-music`) — terminal-native Cyberdeck music
//! player for Hayato OS.

mod art;
mod screen;
mod sway_ipc;

use std::io::{self, IsTerminal};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};

use hayato_music_core::{NowPlayingFile, PlayState, format_time, load_config, runtime_dir};
use hayato_ui::{App, Design, Header};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use screen::{CassetteScreen, THEME_RELOAD_FLAG};

const VERSION: &str = include_str!("../../../../VERSION");
static SAVED_STDERR_FD: AtomicI32 = AtomicI32::new(-1);

fn redirect_stderr_to_log() {
    let _ = std::fs::create_dir_all(runtime_dir());
    let log_path = runtime_dir().join("cassette.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .or_else(|_| std::fs::OpenOptions::new().write(true).open("/dev/null"));
    let Ok(file) = file else {
        return;
    };
    unsafe {
        if SAVED_STDERR_FD.load(Ordering::Relaxed) < 0 {
            let saved = libc::dup(2);
            if saved >= 0 {
                let _ = libc::fcntl(saved, libc::F_SETFD, libc::FD_CLOEXEC);
                SAVED_STDERR_FD.store(saved, Ordering::Relaxed);
            }
        }
        let _ = libc::dup2(file.as_raw_fd(), 2);
    }
}

fn restore_stderr_fd() {
    let saved = SAVED_STDERR_FD.swap(-1, Ordering::Relaxed);
    if saved >= 0 {
        unsafe {
            let _ = libc::dup2(saved, 2);
            let _ = libc::close(saved);
        }
    }
}

extern "C" fn handle_sigusr1(_: libc::c_int) {
    THEME_RELOAD_FLAG.store(true, Ordering::Relaxed);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("{}", VERSION.trim());
        return;
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "hayato-cassette {} — Hayato OS Cyberdeck music player",
            VERSION.trim()
        );
        println!();
        println!("usage: hayato-cassette [--toggle | --bar | --mini | --play FILE]");
        return;
    }
    if args.iter().any(|a| a == "--bar") {
        println!("{}", bar_json());
        return;
    }
    if args.iter().any(|a| a == "--toggle") {
        toggle_window();
        return;
    }

    let start_mini = args.iter().any(|a| a == "--mini");
    let play_file = args
        .windows(2)
        .find(|w| w[0] == "--play")
        .map(|w| PathBuf::from(&w[1]))
        .or_else(|| args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from));

    // If invoked without a TTY (e.g. from root menu or keybinding), spawn foot
    if !io::stdout().is_terminal() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        spawn_in_foot(start_mini, play_file.as_deref());
        return;
    }

    // Install SIGUSR1 handler for live theme reload
    unsafe {
        libc::signal(libc::SIGUSR1, handle_sigusr1 as *const () as usize);
    }

    let pid_file = runtime_dir().join("hayato-cassette.pid");
    let _ = std::fs::create_dir_all(runtime_dir());
    let _ = std::fs::write(&pid_file, std::process::id().to_string());

    let code = run_tui(play_file, start_mini);

    NowPlayingFile::clear();
    let _ = std::fs::remove_file(runtime_dir().join("mode"));
    let _ = std::fs::remove_file(&pid_file);
    std::process::exit(code);
}

fn run_tui(play_file: Option<PathBuf>, start_mini: bool) -> i32 {
    if enable_raw_mode().is_err() {
        return 3;
    }
    redirect_stderr_to_log();
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    );

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut out = io::stdout();
        let _ = execute!(
            out,
            DisableFocusChange,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        restore_stderr_fd();
        default_hook(info);
    }));

    let Ok(mut terminal) = Terminal::new(CrosstermBackend::new(io::stdout())) else {
        let _ = disable_raw_mode();
        restore_stderr_fd();
        return 3;
    };

    let design = Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
    let mut screen = CassetteScreen::new(play_file, start_mini, &design);
    let code = App::new(Header::new("Hayato").subtitle("Cassette"))
        .design(design)
        .screen(&mut screen)
        .run(&mut terminal)
        .unwrap_or(3);

    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(
        out,
        DisableFocusChange,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    restore_stderr_fd();
    code
}

fn is_mini_state() -> bool {
    let mode_path = runtime_dir().join("mode");
    if let Ok(text) = std::fs::read_to_string(mode_path) {
        return text.trim() == "mini";
    }
    load_config().mini_mode
}

fn toggle_window() {
    let mini = is_mini_state();
    match sway_ipc::window_state() {
        sway_ipc::WindowState::Visible => {
            let _ = sway_ipc::hide_to_scratchpad();
        }
        sway_ipc::WindowState::Hidden => {
            let _ = sway_ipc::show_from_scratchpad(mini);
        }
        sway_ipc::WindowState::Absent => {
            spawn_in_foot(mini, None);
        }
    }
}

fn spawn_in_foot(mini: bool, play_file: Option<&std::path::Path>) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("hayato-cassette"));
    let mut cmd = Command::new("foot");
    cmd.args([
        "--app-id",
        sway_ipc::APP_ID,
        "--window-size-pixels",
        if mini { "560x300" } else { "1120x680" },
        "--",
    ])
    .arg(exe);
    if mini {
        cmd.arg("--mini");
    }
    if let Some(p) = play_file {
        cmd.arg("--play").arg(p);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.spawn();
}

/// Waybar `custom/cassette` JSON payload (`<1 ms`, zero D-Bus).
fn bar_json() -> String {
    let Some(now) = NowPlayingFile::read_live() else {
        return r#"{"class":"empty"}"#.to_string();
    };
    if now.state == PlayState::Stopped || now.title.is_empty() {
        return r#"{"class":"empty"}"#.to_string();
    }
    let state_word = if now.state == PlayState::Playing {
        "PLAYING"
    } else {
        "PAUSED"
    };
    let icon = if now.state == PlayState::Playing {
        "\u{f001}"
    } else {
        "\u{f04c}"
    };
    let short_title = if now.title.chars().count() > 20 {
        format!("{}…", now.title.chars().take(19).collect::<String>())
    } else {
        now.title.clone()
    };
    let text = format!("{icon} {short_title}");
    let tooltip = format!(
        "<span alpha='50%'>╭─</span> <b>CASSETTE // {state_word}</b>\n\
         <span alpha='45%'>│</span> <span alpha='60%'>TRK</span>  {}\n\
         <span alpha='45%'>│</span> <span alpha='60%'>ART</span>  {}\n\
         <span alpha='45%'>│</span> <span alpha='60%'>SRC</span>  {} · {} / {}\n\
         <span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] toggle · [RMB] pause · [SCR] prev/next</span>",
        escape_pango(&now.title),
        escape_pango(&now.artist),
        escape_pango(&now.codec_label),
        format_time(now.position_secs),
        format_time(now.duration_secs),
    );
    let class = if now.state == PlayState::Playing {
        "playing"
    } else {
        "paused"
    };
    serde_json::json!({
        "text": text,
        "tooltip": tooltip,
        "class": class,
    })
    .to_string()
}

fn escape_pango(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
