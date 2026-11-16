//! Integration tests for `lazyos greeter` / `lazyos-greeter` (spec 021).

use std::process::Command;

use lazyos_cli::command::resolve;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lazyos")
}

#[test]
fn greeter_command_and_symlink_resolve_to_full_shape() {
    let by_cmd = resolve("lazyos", &["greeter".into()]).unwrap();
    assert_eq!(by_cmd.spec.path, "greeter");
    assert_eq!(by_cmd.spec.shape, lazyos_cli::command::Shape::Full);

    let by_link = resolve("/usr/bin/lazyos-greeter", &["--session".into()]).unwrap();
    assert_eq!(by_link.spec.path, "greeter");
    assert_eq!(by_link.args, vec!["--session"]);
}

#[test]
fn greeter_is_listed_in_help_and_symlinks() {
    let help = Command::new(bin()).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("greeter"), "{help_text}");

    let symlinks = Command::new(bin()).arg("--symlinks").output().unwrap();
    assert!(symlinks.status.success());
    let links_text = String::from_utf8_lossy(&symlinks.stdout);
    assert!(
        links_text.contains("lazyos-greeter\tgreeter"),
        "{links_text}"
    );
}

#[test]
fn greeter_check_reports_theme_sessions_and_users() {
    let out = Command::new(bin())
        .args(["greeter", "--check", "--theme", "dracula"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("theme\tdracula"), "{stdout}");
    assert!(stdout.contains("sessions\t"), "{stdout}");
    assert!(stdout.contains("greetd_sock\t"), "{stdout}");
}

#[test]
fn prepare_session_writes_valid_foot_and_sway_configs() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .args([
            "greeter",
            "--prepare-session",
            "--dir",
            &tmp.path().display().to_string(),
            "--theme",
            "nord",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);

    let foot_ini = tmp.path().join("foot.ini");
    let sway_conf = tmp.path().join("sway.conf");
    assert!(foot_ini.is_file());
    assert!(sway_conf.is_file());

    // Validate generated foot.ini against foot itself if installed in the environment.
    if let Ok(check) = Command::new("foot")
        .args(["--check-config", "-c", &foot_ini.display().to_string()])
        .output()
    {
        assert!(
            check.status.success(),
            "foot --check-config failed: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

#[test]
fn menu_greeter_resolves_and_is_listed_in_settings_rows() {
    let by_cmd = resolve("lazyos", &["menu".into(), "greeter".into()]).unwrap();
    assert_eq!(by_cmd.spec.path, "menu greeter");
    assert_eq!(by_cmd.spec.shape, lazyos_cli::command::Shape::Task);

    let out = Command::new(bin())
        .args(["menu", "settings", "--rows"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("greeter\t"), "{stdout}");
}

#[test]
fn greeter_check_honors_custom_greeter_toml_config() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("greeter.toml");
    std::fs::write(
        &cfg_path,
        "follow_user_theme = false\ntheme = \"nord\"\nshow_user_pills = false\nclock_24h = false\n",
    )
    .unwrap();

    let out = Command::new(bin())
        .args(["greeter", "--check"])
        .env("LAZYOS_GREETER_CONFIG", &cfg_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("theme\tnord"), "{stdout}");
    assert!(stdout.contains("follow_user_theme\tfalse"), "{stdout}");
    assert!(stdout.contains("user_pills\tfalse"), "{stdout}");
    assert!(stdout.contains("clock_24h\tfalse"), "{stdout}");
}
