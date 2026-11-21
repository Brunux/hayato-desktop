//! Native `hayato-greeter` over `greetd` IPC (spec 021).
//!
//! Replaces `tuigreet` 0.9.1 with a full framework TUI running fullscreen in
//! `foot` inside a minimal `sway` greeter compositor (or directly on VT7 TTY as
//! an automatic fallback). Shares `hayato-theme` palettes, generated wallpapers,
//! Nerd Font glyphs, and `hayato-ui::App` terminal hygiene.

pub mod ipc;
pub mod screen;
pub mod settings;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hayato_theme::{SURFACES, Theme, builtin, store, wallpaper};
use hayato_ui::{App, Design, Header};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde::{Deserialize, Serialize};

use crate::Failure;
use crate::command::Resolved;
use crate::term;

pub const DEFAULT_STATE_DIR: &str = "/var/lib/hayato/greeter";
pub const DEFAULT_CONFIG_FILE: &str = "/etc/hayato/greeter.toml";
pub const DEFAULT_GREETING: &str = "light. fast. fun.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserEntry {
    pub username: String,
    pub display_name: String,
    pub home: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Wayland,
    X11,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Wayland => "wayland",
            SessionKind::X11 => "x11",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Wayland => "Wayland",
            SessionKind::X11 => "X11",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub exec: Vec<String>,
    pub kind: SessionKind,
    pub desktop_names: Vec<String>,
    pub comment: Option<String>,
}

impl SessionEntry {
    pub fn env_vars(&self) -> Vec<String> {
        let mut env = vec![format!("XDG_SESSION_TYPE={}", self.kind.as_str())];
        if !self.desktop_names.is_empty() {
            env.push(format!(
                "XDG_CURRENT_DESKTOP={}",
                self.desktop_names.join(":")
            ));
            if let Some(first) = self.desktop_names.first() {
                env.push(format!("XDG_SESSION_DESKTOP={first}"));
            }
        }
        env
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GreeterState {
    #[serde(default)]
    pub last_user: Option<String>,
    #[serde(default)]
    pub last_session: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
}

impl GreeterState {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let Some(parent) = path.parent() else {
            return Err("invalid greeter state path".into());
        };
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        let tmp = parent.join(".state.json.new");
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        Ok(())
    }
}

/// Discover human login users (`UID >= 1000 && UID < 65534`) from `/etc/passwd`.
pub fn parse_passwd_users(content: &str) -> Vec<UserEntry> {
    let mut users = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 7 {
            continue;
        }
        let username = parts[0].trim();
        let Ok(uid) = parts[2].trim().parse::<u32>() else {
            continue;
        };
        if username.is_empty() || !(1000..65534).contains(&uid) {
            continue;
        }
        let shell = parts[6].trim();
        if shell.is_empty()
            || shell.ends_with("/nologin")
            || shell.ends_with("/false")
            || shell == "/bin/sync"
            || shell.ends_with("/halt")
            || shell.ends_with("/shutdown")
        {
            continue;
        }
        let gecos = parts[4].split(',').next().unwrap_or("").trim();
        let display_name = if gecos.is_empty() {
            username.to_string()
        } else {
            gecos.to_string()
        };
        users.push(UserEntry {
            username: username.to_string(),
            display_name,
            home: parts[5].trim().to_string(),
        });
    }
    users
}

pub fn discover_users() -> Vec<UserEntry> {
    let Ok(passwd) = fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };
    parse_passwd_users(&passwd)
}

/// Parse one `.desktop` entry from a Wayland or X11 session directory.
pub fn parse_desktop_session(content: &str, kind: SessionKind) -> Option<SessionEntry> {
    let mut in_entry = false;
    let mut name = String::new();
    let mut exec_raw = String::new();
    let mut desktop_names = Vec::new();
    let mut comment = None;
    let mut no_display = false;
    let mut hidden = false;
    let mut try_exec = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Name" if name.is_empty() => name = value.to_string(),
            "Exec" => exec_raw = value.to_string(),
            "Comment" if comment.is_none() => comment = Some(value.to_string()),
            "DesktopNames" => {
                desktop_names = value
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "TryExec" => try_exec = Some(value.to_string()),
            "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    if no_display || hidden || name.is_empty() || exec_raw.is_empty() {
        return None;
    }
    if let Some(bin) = try_exec {
        if !command_exists(&bin) {
            return None;
        }
    }

    let exec = split_exec(&exec_raw);
    if exec.is_empty() {
        return None;
    }

    Some(SessionEntry {
        name,
        exec,
        kind,
        desktop_names,
        comment,
    })
}

fn split_exec(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;

    for ch in raw.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    push_exec_token(&mut out, std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        push_exec_token(&mut out, current);
    }
    out
}

fn push_exec_token(out: &mut Vec<String>, token: String) {
    match token.as_str() {
        "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" => {}
        _ => out.push(token),
    }
}

pub fn discover_sessions() -> Vec<SessionEntry> {
    let dirs = [
        (
            Path::new("/usr/share/wayland-sessions"),
            SessionKind::Wayland,
        ),
        (
            Path::new("/usr/local/share/wayland-sessions"),
            SessionKind::Wayland,
        ),
        (Path::new("/usr/share/xsessions"), SessionKind::X11),
        (Path::new("/usr/local/share/xsessions"), SessionKind::X11),
    ];

    let mut sessions = Vec::new();
    for (dir, kind) in dirs {
        let Ok(read_dir) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Some(entry) = parse_desktop_session(&content, kind) {
                    if !sessions.iter().any(|existing: &SessionEntry| {
                        existing.name == entry.name && existing.kind == entry.kind
                    }) {
                        sessions.push(entry);
                    }
                }
            }
        }
    }

    if sessions.is_empty() {
        sessions.push(SessionEntry {
            name: "Sway".into(),
            exec: vec!["sway".into()],
            kind: SessionKind::Wayland,
            desktop_names: vec!["sway".into(), "wlroots".into(), "swayfx".into()],
            comment: Some("Hayato Wayland session".into()),
        });
    }
    sessions
}

fn command_exists(bin: &str) -> bool {
    let path = Path::new(bin);
    if path.is_absolute() {
        return path.is_file();
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(bin).is_file())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreeterConfig {
    pub follow_user_theme: bool,
    pub theme: Option<String>,
    pub greeting: Option<String>,
    pub default_session: Option<String>,
    pub show_user_pills: bool,
    pub remember_user: bool,
    pub remember_session: bool,
    pub clock_24h: bool,
    pub show_power_actions: bool,
}

impl Default for GreeterConfig {
    fn default() -> Self {
        Self {
            follow_user_theme: true,
            theme: None,
            greeting: None,
            default_session: None,
            show_user_pills: true,
            remember_user: true,
            remember_session: true,
            clock_24h: true,
            show_power_actions: true,
        }
    }
}

fn parse_bool_val(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse_greeter_config(text: &str) -> GreeterConfig {
    let mut cfg = GreeterConfig::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'').trim();
        if val.is_empty() {
            continue;
        }
        match key {
            "follow_user_theme" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.follow_user_theme = b;
                }
            }
            "theme" => cfg.theme = Some(val.to_string()),
            "greeting" => cfg.greeting = Some(val.to_string()),
            "default_session" | "session" => cfg.default_session = Some(val.to_string()),
            "show_user_pills" | "user_pills" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.show_user_pills = b;
                }
            }
            "remember_user" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.remember_user = b;
                }
            }
            "remember_session" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.remember_session = b;
                }
            }
            "clock_24h" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.clock_24h = b;
                }
            }
            "show_power_actions" | "power_actions" => {
                if let Some(b) = parse_bool_val(val) {
                    cfg.show_power_actions = b;
                }
            }
            _ => {}
        }
    }
    cfg
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Format [`GreeterConfig`] as clean TOML suitable for `/etc/hayato/greeter.toml`.
pub fn format_greeter_config(cfg: &GreeterConfig) -> String {
    let mut out = String::from(
        "# Hayato Greeter configuration (managed by `hayato menu greeter`)\n\
         # Changes apply to `/usr/bin/hayato-greeter --session` and `hayato greeter --preview`.\n\n",
    );
    out.push_str(&format!("follow_user_theme = {}\n", cfg.follow_user_theme));
    if let Some(theme) = cfg.theme.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("theme = \"{}\"\n", toml_escape(theme.trim())));
    } else {
        out.push_str("# theme = \"tokyo-night\"\n");
    }
    if let Some(greeting) = cfg.greeting.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!(
            "greeting = \"{}\"\n",
            toml_escape(greeting.trim())
        ));
    } else {
        out.push_str("# greeting = \"light. fast. fun.\"\n");
    }
    if let Some(session) = cfg
        .default_session
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        out.push_str(&format!(
            "default_session = \"{}\"\n",
            toml_escape(session.trim())
        ));
    } else {
        out.push_str("# default_session = \"Sway\"\n");
    }
    out.push_str(&format!("show_user_pills = {}\n", cfg.show_user_pills));
    out.push_str(&format!("remember_user = {}\n", cfg.remember_user));
    out.push_str(&format!("remember_session = {}\n", cfg.remember_session));
    out.push_str(&format!("clock_24h = {}\n", cfg.clock_24h));
    out.push_str(&format!(
        "show_power_actions = {}\n",
        cfg.show_power_actions
    ));
    out
}

pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("HAYATO_GREETER_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from(DEFAULT_CONFIG_FILE)
}

pub fn load_greeter_config(path: &Path) -> GreeterConfig {
    fs::read_to_string(path)
        .map(|s| parse_greeter_config(&s))
        .unwrap_or_default()
}

/// Save [`GreeterConfig`] to `path`, using passwordless `sudo -n` if `path` is
/// system-owned (such as `/etc/hayato/greeter.toml`).
pub fn save_greeter_config(path: &Path, cfg: &GreeterConfig) -> Result<(), String> {
    let body = format_greeter_config(cfg);
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_ok() {
            let tmp = parent.join(".greeter.toml.new");
            if fs::write(&tmp, &body).is_ok() {
                if fs::rename(&tmp, path).is_ok() {
                    return Ok(());
                }
                let _ = fs::remove_file(&tmp);
            }
        }
    }

    let tmp_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let tmp_path = tmp_dir.join(format!("hayato-greeter-{}.toml", std::process::id()));
    let _ = fs::remove_file(&tmp_path);
    {
        use std::io::Write as _;
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| format!("create temp config: {e}"))?;
        tmp.write_all(body.as_bytes())
            .map_err(|e| format!("write temp config: {e}"))?;
        tmp.flush().map_err(|e| format!("flush temp config: {e}"))?;
    }

    let status = Command::new("sudo")
        .args([
            "-n", "install", "-D", "-m", "0644", "-o", "root", "-g", "root",
        ])
        .arg(&tmp_path)
        .arg(path)
        .status();
    let _ = fs::remove_file(&tmp_path);
    let status = status.map_err(|e| format!("invoke sudo: {e}"))?;

    if !status.success() {
        return Err(format!(
            "cannot write {} (sudo -n exited with {})",
            path.display(),
            status
        ));
    }
    Ok(())
}

pub fn load_theme_by_slug(slug: &str) -> Option<Theme> {
    if let Some(toml) = builtin::get(slug) {
        if let Ok(theme) = Theme::parse(toml) {
            return Some(theme);
        }
    }
    None
}

/// Resolve active theme slug, parsed [`Theme`], and greeting string.
///
/// Priority when `follow_user_theme = true` (default):
/// 1. Explicit CLI `--theme` or `HAYATO_GREETER_THEME`
/// 2. Last user's `~/.local/state/hayato/theme/active` (or `$HOME` in preview)
/// 3. Greeter's own remembered state (`state.theme`)
/// 4. `/etc/hayato/greeter.toml` `theme = "..."`
/// 5. Built-in `"tokyo-night"`
///
/// When `follow_user_theme = false`, step 4 precedes steps 2 and 3.
pub fn resolve_theme_and_greeting(
    cli_theme: Option<&str>,
    user_home: Option<&Path>,
    state: &GreeterState,
    config_file: &Path,
) -> (String, Theme, String) {
    let cfg = load_greeter_config(config_file);
    let greeting = cfg
        .greeting
        .clone()
        .unwrap_or_else(|| DEFAULT_GREETING.to_string());

    let env_theme = std::env::var("HAYATO_GREETER_THEME").ok();
    let user_active = user_home.and_then(|h| {
        let p = h.join(".local/state/hayato/theme/active");
        fs::read_to_string(p).ok().map(|s| s.trim().to_string())
    });

    let candidates = if cfg.follow_user_theme {
        [
            cli_theme.map(str::to_string),
            env_theme,
            user_active,
            state.theme.clone(),
            cfg.theme,
        ]
    } else {
        [
            cli_theme.map(str::to_string),
            env_theme,
            cfg.theme,
            state.theme.clone(),
            user_active,
        ]
    };

    for slug in candidates.into_iter().flatten() {
        let trimmed = slug.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(theme) = load_theme_by_slug(trimmed) {
            return (trimmed.to_string(), theme, greeting);
        }
    }

    let fallback = Theme::parse(builtin::get("tokyo-night").expect("tokyo-night builtin exists"))
        .expect("tokyo-night theme parses");
    ("tokyo-night".into(), fallback, greeting)
}

fn read_system_xkb_layout() -> String {
    let Ok(content) = fs::read_to_string("/etc/default/keyboard") else {
        return "us".into();
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("XKBLAYOUT=") {
            let layout = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !layout.is_empty()
                && layout
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ',' || c == '-' || c == '_')
            {
                return layout.to_string();
            }
        }
    }
    "us".into()
}

pub fn render_greeter_foot_ini(theme: &Theme, slug: &str) -> Result<String, String> {
    let foot_surface = SURFACES
        .iter()
        .find(|s| s.name == "foot")
        .ok_or_else(|| "foot template missing from hayato-theme".to_string())?;
    let mut rendered = store::render_surface(foot_surface, theme, slug, "/var/lib/hayato/greeter")?;
    rendered = rendered.replace("pad=6x6", "pad=16x16");
    rendered = rendered.replace("alpha=0.87", "alpha=0.88\nalpha-mode=all");
    rendered = rendered.replace("size=11", "size=13");
    rendered = rendered.replace("size=12", "size=14");
    Ok(rendered)
}

pub fn render_greeter_sway_conf(
    foot_ini: &Path,
    wallpaper_path: Option<&Path>,
    theme: &Theme,
    exe: &str,
) -> String {
    let xkb_layout = read_system_xkb_layout();
    let palette = theme.palette();
    let bg_hex = format!(
        "#{:02x}{:02x}{:02x}",
        palette.bg.r, palette.bg.g, palette.bg.b
    );

    let bg_line = match wallpaper_path {
        Some(wp) => format!("output * bg \"{}\" fill \"{}\"", wp.display(), bg_hex),
        None => format!("output * bg \"{}\" solid_color", bg_hex),
    };

    format!(
        "# Generated by hayato-greeter\n\
{bg_line}\n\
default_border none\n\
for_window [app_id=\"hayato-greeter\"] fullscreen enable, border none\n\
input type:touchpad {{\n\
    tap enabled\n\
    natural_scroll enabled\n\
}}\n\
input type:keyboard {{\n\
    xkb_layout {xkb_layout}\n\
}}\n\
bindsym XF86MonBrightnessUp exec brightnessctl set +5%\n\
bindsym XF86MonBrightnessDown exec brightnessctl set 5%-\n\
exec \"HAYATO_UI=1 foot --config='{}' --app-id=hayato-greeter '{}' greeter --ui ; swaymsg exit\"\n",
        foot_ini.display(),
        exe
    )
}

#[derive(Clone, Debug)]
pub struct PreparedSession {
    pub foot_ini: PathBuf,
    pub sway_conf: PathBuf,
    pub wallpaper: Option<PathBuf>,
    pub theme_slug: String,
}

pub fn prepare_session_assets(
    runtime_dir: &Path,
    theme_slug: &str,
    theme: &Theme,
    exe: &str,
) -> Result<PreparedSession, String> {
    fs::create_dir_all(runtime_dir)
        .map_err(|e| format!("create {}: {e}", runtime_dir.display()))?;

    let foot_ini_path = runtime_dir.join("foot.ini");
    let foot_body = render_greeter_foot_ini(theme, theme_slug)?;
    fs::write(&foot_ini_path, foot_body)
        .map_err(|e| format!("write {}: {e}", foot_ini_path.display()))?;

    let wallpaper_path = runtime_dir.join(format!("wallpaper-{theme_slug}.png"));
    let wp = match wallpaper::generate_wallpaper(
        theme,
        theme_slug,
        wallpaper::DEFAULT_WIDTH,
        wallpaper::DEFAULT_HEIGHT,
    ) {
        Ok(wp) => {
            if fs::write(&wallpaper_path, &wp.data).is_ok() {
                Some(wallpaper_path)
            } else {
                None
            }
        }
        Err(_) => None,
    };

    let sway_conf_path = runtime_dir.join("sway.conf");
    let sway_body = render_greeter_sway_conf(&foot_ini_path, wp.as_deref(), theme, exe);
    fs::write(&sway_conf_path, sway_body)
        .map_err(|e| format!("write {}: {e}", sway_conf_path.display()))?;

    Ok(PreparedSession {
        foot_ini: foot_ini_path,
        sway_conf: sway_conf_path,
        wallpaper: wp,
        theme_slug: theme_slug.to_string(),
    })
}

fn default_state_path() -> PathBuf {
    if let Ok(p) = std::env::var("HAYATO_GREETER_STATE") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    Path::new(DEFAULT_STATE_DIR).join("state.json")
}

fn default_runtime_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("hayato-greeter");
        }
    }
    PathBuf::from("/tmp").join(format!("hayato-greeter-{}", std::process::id()))
}

fn resolve_user_home(users: &[UserEntry], state: &GreeterState) -> Option<PathBuf> {
    if let Some(last) = state.last_user.as_deref() {
        if let Some(u) = users.iter().find(|u| u.username == last) {
            return Some(PathBuf::from(&u.home));
        }
    }
    if users.len() == 1 {
        return Some(PathBuf::from(&users[0].home));
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() && home != "/" && !home.contains("greetd") {
            return Some(PathBuf::from(home));
        }
    }
    None
}

fn cli_flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().map(String::as_str);
        }
        if let Some(rest) = arg.strip_prefix(&format!("{name}=")) {
            return Some(rest);
        }
    }
    None
}

/// Handle headless or VT-session greeter flags (`--check`, `--prepare-session`,
/// `--session`) before the normal desktop window planner runs.
pub fn try_query(resolved: &Resolved) -> Option<i32> {
    if resolved.spec.path != "greeter" {
        return None;
    }
    let args = &resolved.args;
    if args.iter().any(|a| a == "--check") {
        return Some(run_check(args));
    }
    if args.iter().any(|a| a == "--prepare-session") {
        return Some(run_prepare_session(args));
    }
    if args.iter().any(|a| a == "--session") {
        return Some(run_greetd_session(args));
    }
    None
}

fn run_check(args: &[String]) -> i32 {
    let users = discover_users();
    let sessions = discover_sessions();
    let state = GreeterState::load(&default_state_path());
    let cfg_path = default_config_path();
    let cfg = load_greeter_config(&cfg_path);
    let home = resolve_user_home(&users, &state);
    let cli_theme = cli_flag_value(args, "--theme");
    let (slug, _theme, greeting) =
        resolve_theme_and_greeting(cli_theme, home.as_deref(), &state, &cfg_path);
    let sock = std::env::var("GREETD_SOCK").unwrap_or_else(|_| "unset".into());

    println!("theme\t{slug}");
    println!("follow_user_theme\t{}", cfg.follow_user_theme);
    println!("user_pills\t{}", cfg.show_user_pills);
    println!("clock_24h\t{}", cfg.clock_24h);
    println!("greeting\t{greeting}");
    println!("users\t{}", users.len());
    for u in &users {
        println!("user\t{}\t{}\t{}", u.username, u.display_name, u.home);
    }
    println!("sessions\t{}", sessions.len());
    for s in &sessions {
        println!(
            "session\t{}\t{}\t{}",
            s.name,
            s.kind.as_str(),
            s.exec.join(" ")
        );
    }
    println!("greetd_sock\t{sock}");
    0
}

fn run_prepare_session(args: &[String]) -> i32 {
    let dir = cli_flag_value(args, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_runtime_dir);
    let users = discover_users();
    let state = GreeterState::load(&default_state_path());
    let cfg_path = default_config_path();
    let home = resolve_user_home(&users, &state);
    let cli_theme = cli_flag_value(args, "--theme");
    let (slug, theme, _greeting) =
        resolve_theme_and_greeting(cli_theme, home.as_deref(), &state, &cfg_path);
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/usr/bin/hayato".to_string());

    match prepare_session_assets(&dir, &slug, &theme, &exe) {
        Ok(prep) => {
            println!(
                "{}\t{}\t{}\t{}",
                prep.foot_ini.display(),
                prep.sway_conf.display(),
                prep.wallpaper
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "none".into()),
                prep.theme_slug
            );
            0
        }
        Err(e) => {
            eprintln!("hayato greeter --prepare-session: {e}");
            3
        }
    }
}

/// Entry point for `/etc/greetd/config.toml`:
/// `command = "/usr/bin/hayato-greeter --session"`.
///
/// Spawns minimal `sway` + `swaybg` wallpaper + fullscreen `foot` running
/// `hayato greeter --ui`. If `sway` or `foot` is absent or exits pre-auth (e.g.
/// headless VM without DRM), falls back directly to inline Ratatui TUI on VT7.
fn run_greetd_session(args: &[String]) -> i32 {
    let force_tty = args.iter().any(|a| a == "--tty");
    if !force_tty && command_exists("sway") && command_exists("foot") {
        let runtime_dir = default_runtime_dir();
        let users = discover_users();
        let state = GreeterState::load(&default_state_path());
        let cfg_path = default_config_path();
        let home = resolve_user_home(&users, &state);
        let cli_theme = cli_flag_value(args, "--theme");
        let (slug, theme, _greeting) =
            resolve_theme_and_greeting(cli_theme, home.as_deref(), &state, &cfg_path);
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/usr/bin/hayato".to_string());

        if let Ok(prep) = prepare_session_assets(&runtime_dir, &slug, &theme, &exe) {
            let marker = runtime_dir.join("authenticated");
            let _ = fs::remove_file(&marker);

            let status = Command::new("sway")
                .args(["-c", &prep.sway_conf.to_string_lossy()])
                .env("HAYATO_GREETER_AUTH_MARKER", &marker)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status();

            if marker.exists() {
                let _ = fs::remove_file(&marker);
                let _ = fs::remove_dir_all(&runtime_dir);
                return 0;
            }
            if let Ok(st) = status {
                if st.success() {
                    let _ = fs::remove_dir_all(&runtime_dir);
                    return 0;
                }
            }
        }
    }

    // Automatic VT7 fallback: draw directly on the active console.
    run_tty_fallback(args)
}

fn run_tty_fallback(args: &[String]) -> i32 {
    let session = match term::Session::enter() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hayato greeter: cannot initialize VT fallback terminal: {e}");
            return 3;
        }
    };
    let mut terminal = match Terminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("hayato greeter: cannot create terminal backend: {e}");
            return 3;
        }
    };
    let design = Design::for_session(std::env::var("XDG_RUNTIME_DIR").ok().as_deref());
    let code = match menu(args, &mut terminal, &design) {
        Ok(c) => c,
        Err(f) => {
            eprintln!("hayato greeter: {}: {}", f.summary, f.detail);
            3
        }
    };
    drop(session);
    code
}

/// Interactive TUI entrypoint invoked by `hayato-cli/src/lib.rs::execute`.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    if args.iter().any(|a| a == "settings" || a == "--settings") {
        return settings::menu(terminal, design);
    }

    let ui_mode = args.iter().any(|a| a == "--ui");
    let preview_flag = args.iter().any(|a| a == "--preview");
    let has_sock = std::env::var("GREETD_SOCK")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    // If launched interactively without `--ui` or `GREETD_SOCK`, default to safe preview mode.
    let preview = preview_flag || (!ui_mode && !has_sock);
    let cli_theme = cli_flag_value(args, "--theme");
    let state_path = default_state_path();
    let cfg_path = default_config_path();

    loop {
        let backend = ipc::Backend::from_env_or_preview(preview)
            .map_err(|err| Failure::new("greeter cannot reach greetd", err))?;

        let users = discover_users();
        let sessions = discover_sessions();
        let state = GreeterState::load(&state_path);
        let cfg = load_greeter_config(&cfg_path);
        let home = resolve_user_home(&users, &state);
        let (theme_slug, theme, greeting) =
            resolve_theme_and_greeting(cli_theme, home.as_deref(), &state, &cfg_path);

        let initial_design = Design::new(
            theme.palette(),
            design.density(),
            design.reduced_motion() || theme.motion != hayato_theme::Motion::Full,
        );

        let mut scr = screen::GreeterScreen::new(screen::GreeterSetup {
            users,
            sessions,
            state,
            state_path: (!preview).then(|| state_path.clone()),
            auth_marker: std::env::var_os("HAYATO_GREETER_AUTH_MARKER").map(PathBuf::from),
            theme_slug,
            theme,
            greeting,
            backend,
            preview,
            density: design.density(),
            reduced_motion: design.reduced_motion(),
            config: cfg,
        });

        let code = App::new(Header::default())
            .design(initial_design)
            .screen(&mut scr)
            .run(terminal)
            .map_err(|e| Failure::new("greeter terminal stopped answering", e.to_string()))?;

        return Ok(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_discovery_keeps_only_human_login_accounts() {
        let sample = "\
root:x:0:0:root:/root:/bin/bash
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin
_greetd:x:983:983::/var/lib/greetd:/usr/sbin/nologin
brunux:x:1000:1000:Bruno,,,:/home/brunux:/bin/bash
guest:x:1001:1001::/home/guest:/bin/false
nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin
alice:x:1002:1002:Alice:/home/alice:/usr/bin/zsh
";
        let users = parse_passwd_users(sample);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "brunux");
        assert_eq!(users[0].display_name, "Bruno");
        assert_eq!(users[1].username, "alice");
    }

    #[test]
    fn desktop_session_parser_extracts_exec_and_desktop_names() {
        let sway = "\
[Desktop Entry]
Name=Sway
Comment=An i3-compatible Wayland compositor
Exec=sway --unsupported-gpu
Type=Application
DesktopNames=sway;wlroots;swayfx
";
        let parsed = parse_desktop_session(sway, SessionKind::Wayland).unwrap();
        assert_eq!(parsed.name, "Sway");
        assert_eq!(parsed.exec, vec!["sway", "--unsupported-gpu"]);
        assert_eq!(parsed.desktop_names, vec!["sway", "wlroots", "swayfx"]);
        assert_eq!(
            parsed.env_vars(),
            vec![
                "XDG_SESSION_TYPE=wayland",
                "XDG_CURRENT_DESKTOP=sway:wlroots:swayfx",
                "XDG_SESSION_DESKTOP=sway",
            ]
        );
    }

    #[test]
    fn desktop_session_parser_skips_hidden_entries() {
        let hidden = "[Desktop Entry]\nName=Hidden\nExec=foo\nNoDisplay=true\n";
        assert!(parse_desktop_session(hidden, SessionKind::X11).is_none());
    }

    #[test]
    fn theme_resolution_follows_priority_and_falls_back_to_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home/brunux");
        let active_dir = home.join(".local/state/hayato/theme");
        fs::create_dir_all(&active_dir).unwrap();
        fs::write(active_dir.join("active"), "nord\n").unwrap();

        let cfg = tmp.path().join("greeter.toml");
        fs::write(&cfg, "theme = \"gruvbox\"\ngreeting = \"Welcome home\"\n").unwrap();

        let state = GreeterState {
            theme: Some("dracula".into()),
            ..Default::default()
        };

        // Explicit CLI override wins first.
        let (slug, _, greeting) =
            resolve_theme_and_greeting(Some("rose-pine"), Some(&home), &state, &cfg);
        assert_eq!(slug, "rose-pine");
        assert_eq!(greeting, "Welcome home");

        // Remembered user's active theme overrides greeter fallback & config.
        let (slug, _, _) = resolve_theme_and_greeting(None, Some(&home), &state, &cfg);
        assert_eq!(slug, "nord");

        // Greeter state overrides config when home has no active theme.
        let (slug, _, _) = resolve_theme_and_greeting(None, None, &state, &cfg);
        assert_eq!(slug, "dracula");

        // Config overrides the default fallback.
        let (slug, _, _) = resolve_theme_and_greeting(None, None, &GreeterState::default(), &cfg);
        assert_eq!(slug, "gruvbox");

        // Invalid slug falls back to the default theme.
        fs::write(&cfg, "theme = \"does-not-exist\"\n").unwrap();
        let (slug, _, _) = resolve_theme_and_greeting(None, None, &GreeterState::default(), &cfg);
        assert_eq!(slug, "tokyo-night");
    }

    #[test]
    fn state_save_and_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state/state.json");
        let state = GreeterState {
            last_user: Some("brunux".into()),
            last_session: Some("Sway".into()),
            theme: Some("catppuccin".into()),
        };
        state.save(&path).unwrap();
        assert_eq!(GreeterState::load(&path), state);
    }

    #[test]
    fn prepare_session_assets_writes_valid_configs() {
        let tmp = tempfile::tempdir().unwrap();
        let theme = load_theme_by_slug("solarized").unwrap();
        let prep =
            prepare_session_assets(tmp.path(), "solarized", &theme, "/usr/bin/hayato").unwrap();
        assert!(prep.foot_ini.is_file());
        assert!(prep.sway_conf.is_file());
        let foot_text = fs::read_to_string(&prep.foot_ini).unwrap();
        assert!(foot_text.contains("alpha-mode=all"));
        let sway_text = fs::read_to_string(&prep.sway_conf).unwrap();
        assert!(sway_text.contains("fullscreen enable"));
        assert!(sway_text.contains("hayato-greeter"));
    }

    #[test]
    fn greeter_config_format_save_and_parse_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("etc/hayato/greeter.toml");
        let cfg = GreeterConfig {
            follow_user_theme: false,
            theme: Some("nord".into()),
            greeting: Some("Welcome to Hayato".into()),
            default_session: Some("Sway".into()),
            show_user_pills: false,
            remember_user: false,
            remember_session: false,
            clock_24h: false,
            show_power_actions: false,
        };
        save_greeter_config(&cfg_path, &cfg).unwrap();
        let loaded = load_greeter_config(&cfg_path);
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn fixed_greeter_theme_ignores_user_active_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home/brunux");
        let active_dir = home.join(".local/state/hayato/theme");
        fs::create_dir_all(&active_dir).unwrap();
        fs::write(active_dir.join("active"), "nord\n").unwrap();

        let cfg = tmp.path().join("greeter.toml");
        fs::write(&cfg, "follow_user_theme = false\ntheme = \"gruvbox\"\n").unwrap();

        let state = GreeterState {
            theme: Some("dracula".into()),
            ..Default::default()
        };
        let (slug, _, _) = resolve_theme_and_greeting(None, Some(&home), &state, &cfg);
        assert_eq!(slug, "gruvbox");
    }
}
