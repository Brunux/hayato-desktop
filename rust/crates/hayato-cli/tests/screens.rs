//! The screens that have no golden file yet, rendered at both session grids.
//!
//! A screen that draws outside its area, forgets its header or panics on an
//! empty state is a bug the unit tests cannot see. These tests build each
//! screen with an isolated home (no notes, no history, no games), draw it at
//! 80×24 and 152×45, and either compare a golden file or assert the shell's
//! one header rule. Screens whose rows depend on the machine (a media player,
//! the installed desktop entries, the manual pages, the game cores) are smoke
//! renders: the layout is checked, the exact rows are not promised.
//!
//! `UPDATE_SNAPSHOTS=1` regenerates the golden files; read the diff.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hayato_cli::greeter::ipc::Backend;
use hayato_cli::greeter::screen::{GreeterScreen, GreeterSetup};
use hayato_cli::greeter::{
    GreeterConfig, GreeterState, SessionEntry, SessionKind, UserEntry, load_theme_by_slug,
};
use hayato_cli::installer::deviceinfo::DeviceInfo;
use hayato_cli::installer::preflight::{Preflight, TargetCandidate, ToolStatus};
use hayato_cli::installer::ui::InstallerScreen;
use hayato_cli::padmap::Bindings;
use hayato_cli::system::{
    about::AboutScreen, apps::AppsScreen, audio::AudioScreen, bluetooth::BluetoothScreen,
    calc::CalcScreen, capture::CaptureScreen, clean::CleanScreen, clip::ClipScreen,
    colour::ColourScreen, dev::DevScreen, display::DisplayScreen, display::Output as DisplayOutput,
    emoji::EmojiScreen, games::GamesScreen, keys::KeysScreen, kill::KillScreen, learn::LearnScreen,
    media::MediaScreen, net::NetScreen, notify::NotifyScreen, pad::ControllerScreen,
    pkg::PkgScreen, qr::QrScreen, remind::RemindScreen, run::RunScreen, settings::SettingsScreen,
    setup::SetupScreen, theme_picker::ThemePickerScreen, window::WindowScreen,
};
use hayato_ui::{App, Density, Design, Header, Screen};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const SIZES: [(u16, u16); 2] = [(80, 24), (152, 45)];

/// A home with nothing in it: no history, no notes, no games, no sway config.
/// One per test binary, because the environment is process-global.
fn isolated() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("hayato-screens-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "home",
            "home/Games",
            "home/Documents/Notes",
            "config",
            "state",
            "data",
            "run",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        // SAFETY: this test binary has one thread that reads the environment
        // for the first time here; the other tests in this file call
        // `isolated()` before constructing a screen.
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
            std::env::set_var("XDG_STATE_HOME", root.join("state"));
            std::env::set_var("XDG_DATA_HOME", root.join("data"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("run"));
        }
        root
    })
}

fn render(screen: &mut dyn Screen, width: u16, height: u16) -> String {
    let mut app = App::new(Header::new("Hayato"))
        .design(Design::solarized().with_reduced_motion(true))
        .screen(screen);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::with_capacity(usize::from(width) * usize::from(height + 1));
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        out.push('\n');
    }
    out
}

fn snapshot(name: &str, rendered: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&path, rendered).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}: run UPDATE_SNAPSHOTS=1 cargo test -p hayato-cli",
            path.display()
        )
    });
    assert_eq!(rendered, expected, "snapshot {} changed", path.display());
}

/// A deterministic screen: its rows are the same on every machine with an
/// isolated home.
fn golden(name: &str, mut make: impl FnMut() -> Box<dyn Screen>) {
    isolated();
    let root = isolated().display().to_string();
    let home = isolated().join("home").display().to_string();
    for (width, height) in SIZES {
        let mut screen = make();
        let rendered = render(screen.as_mut(), width, height)
            .replace(&home, "~")
            .replace(&root, "<tmp>");
        snapshot(&format!("{name}-{width}x{height}"), &rendered);
    }
}

/// A screen whose rows depend on the machine: check the layout and the one
/// header rule, not the exact rows.
fn smoke(name: &str, mut make: impl FnMut() -> Box<dyn Screen>) {
    isolated();
    for (width, height) in SIZES {
        let mut screen = make();
        let rendered = render(screen.as_mut(), width, height);
        let first = rendered.lines().next().unwrap_or_default();
        assert!(
            first.contains("Hayato"),
            "{name} at {width}x{height} has no product header: {first:?}"
        );
        assert_eq!(
            rendered.lines().count(),
            usize::from(height),
            "{name} at {width}x{height} drew a different number of lines"
        );
    }
}

#[test]
fn capture_is_a_list_of_actions() {
    // The rows name the folder they save to; a smoke render checks the layout
    // without pinning the runner's temp path into a golden file.
    smoke("capture", || Box::new(CaptureScreen::new()));
}

#[test]
fn qr_is_a_menu_of_two_choices() {
    // The first row names the Wi-Fi network this machine is on; smoke.
    smoke("qr", || Box::new(QrScreen::new()));
}

#[test]
fn reminders_are_empty_and_say_so() {
    golden("remind", || Box::new(RemindScreen::new()));
}

#[test]
fn calculator_starts_with_an_empty_history() {
    golden("calc", || Box::new(CalcScreen::new()));
}

#[test]
fn colour_with_no_theme_file_still_renders() {
    smoke("colour", || Box::new(ColourScreen::new()));
}

#[test]
fn developer_is_a_list_of_tools() {
    golden("dev", || Box::new(DevScreen::new()));
}

#[test]
fn emoji_without_the_font_pin_still_renders() {
    smoke("emoji", || Box::new(EmojiScreen::new()));
}

#[test]
fn notifications_are_empty_with_no_history() {
    golden("notify", || Box::new(NotifyScreen::new()));
}

#[test]
fn clipboard_is_empty_with_no_history() {
    golden("clip", || Box::new(ClipScreen::new()));
}

#[test]
fn keys_with_no_sway_config_still_renders() {
    smoke("keys", || Box::new(KeysScreen::new()));
}

#[test]
fn media_with_no_player_still_renders() {
    smoke("media", || Box::new(MediaScreen::new()));
}

#[test]
fn applications_with_no_entries_still_render() {
    smoke("apps", || Box::new(AppsScreen::new()));
}

#[test]
fn learn_with_the_manual_missing_still_renders() {
    smoke("learn", || Box::new(LearnScreen::new()));
}

#[test]
fn games_with_an_empty_library_still_renders() {
    smoke("games", || Box::new(GamesScreen::new()));
}

#[test]
fn about_with_the_machines_own_facts_still_renders() {
    smoke("about", || Box::new(AboutScreen::new()));
}

#[test]
fn free_space_while_calculating_still_renders() {
    smoke("clean", || Box::new(CleanScreen::new()));
}

#[test]
fn audio_with_no_sink_still_renders() {
    smoke("audio", || Box::new(AudioScreen::new()));
}

#[test]
fn bluetooth_with_no_adapter_still_renders() {
    smoke("bluetooth", || Box::new(BluetoothScreen::new()));
}

#[test]
fn network_with_no_radio_still_renders() {
    smoke("net", || Box::new(NetScreen::new()));
}

#[test]
fn window_switcher_with_no_windows_still_renders() {
    smoke("window", || Box::new(WindowScreen::new()));
}

#[test]
fn run_with_the_path_still_renders() {
    smoke("run", || Box::new(RunScreen::new()));
}

#[test]
fn kill_with_the_process_table_still_renders() {
    smoke("kill", || Box::new(KillScreen::new()));
}

#[test]
fn controller_with_no_pad_still_renders() {
    smoke("controller", || {
        Box::new(ControllerScreen::new(Bindings::default(), None, Vec::new()))
    });
}

#[test]
fn packages_before_a_search_still_renders() {
    smoke("packages", || Box::new(PkgScreen::new()));
}

#[test]
fn the_theme_picker_lists_the_builtins_without_touching_the_terminal() {
    // `with_design_silent` is the same screen without the OSC preview, so the
    // snapshot can draw it and the test run cannot repaint a real terminal.
    golden("theme", || {
        Box::new(ThemePickerScreen::with_design_silent(
            &Design::solarized().with_reduced_motion(true),
        ))
    });
}

/// The installer's preflight, as the machine would report it: a board with a
/// target disk and every tool present.
fn preflight() -> Preflight {
    Preflight {
        device_info: DeviceInfo::default(),
        soc_model: "MediaTek MT8192".into(),
        total_ram: "4.0 GiB".into(),
        power_source: "Mains (USB-C)".into(),
        root_source: "/dev/mmcblk0p3".into(),
        target_disk: "/dev/sda".into(),
        target_size: "29.5 GB".into(),
        candidates: vec![TargetCandidate {
            path: "/dev/sda".into(),
            short_name: "sda".into(),
            size_human: "29.5 GB".into(),
            model: "USB DISK 3.0".into(),
            tran: Some("usb".into()),
            removable: true,
        }],
        is_safe: true,
        refusal_reason: None,
        installed_notice: None,
        tools: vec![ToolStatus {
            name: "parted",
            available: true,
            description: "GPT table generator",
        }],
    }
}

#[test]
fn the_installer_starts_on_its_first_step() {
    // The wizard reads the live locale, keymap, user and hostname, so the rows
    // are this machine's: the layout is checked, the exact words are not.
    smoke("installer", || {
        Box::new(InstallerScreen::from_preflight_isolated(preflight(), true))
    });
}

fn greeter() -> Box<dyn Screen> {
    let setup = GreeterSetup {
        users: vec![UserEntry {
            username: "brunux".into(),
            display_name: "Bruno".into(),
            home: "/home/brunux".into(),
        }],
        sessions: vec![
            SessionEntry {
                name: "Sway".into(),
                exec: vec!["sway".into()],
                kind: SessionKind::Wayland,
                desktop_names: vec!["sway".into()],
                comment: None,
            },
            SessionEntry {
                name: "Bash".into(),
                exec: vec!["/bin/bash".into()],
                kind: SessionKind::X11,
                desktop_names: vec![],
                comment: None,
            },
        ],
        state: GreeterState {
            last_user: Some("brunux".into()),
            ..Default::default()
        },
        state_path: None,
        auth_marker: None,
        theme_slug: "solarized".into(),
        theme: load_theme_by_slug("solarized").unwrap(),
        greeting: "light. fast. fun.".into(),
        backend: Backend::Preview,
        preview: true,
        density: Density::Normal,
        reduced_motion: true,
        config: GreeterConfig::default(),
    };
    let mut screen = GreeterScreen::new(setup);
    // The clock is the one thing that would move between two runs.
    screen.set_clock_for_test("18:30", "Sun, 15 Nov 2026", "hayato");
    Box::new(screen)
}

#[test]
fn the_greeter_preview_draws_a_login_at_both_grids() {
    golden("greeter", greeter);
}

#[test]
fn display_studio_draws_dual_monitor_topology_at_both_grids() {
    golden("display", || {
        Box::new(DisplayScreen::from_outputs(
            vec![
                DisplayOutput {
                    name: "eDP-1".into(),
                    x: 0,
                    y: 0,
                    width: 1366,
                    height: 768,
                    scale: 1.0,
                    mode_width: 1366,
                    mode_height: 768,
                    mode_refresh: 60000,
                    active: true,
                },
                DisplayOutput {
                    name: "DP-1".into(),
                    x: 1366,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: 1.0,
                    mode_width: 1920,
                    mode_height: 1080,
                    mode_refresh: 60000,
                    active: true,
                },
            ],
            vec!["1920x1080@60Hz".into(), "2560x1440@60Hz".into()],
        ))
    });
}

#[test]
fn settings_control_center_draws_master_detail_at_both_grids() {
    golden("settings", || Box::new(SettingsScreen::new_for_test()));
}

#[test]
fn the_setup_assistant_renders_at_both_grids() {
    golden("setup", || Box::new(SetupScreen::for_snapshot()));
}
