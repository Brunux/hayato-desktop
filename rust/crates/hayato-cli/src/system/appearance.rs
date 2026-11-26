//! Appearance: theme, wallpaper, bar and the compositor's window effects.
//!
//! One user file, `~/.config/hayato/appearance.conf`, is the source of truth
//! for the effects. It is rendered into
//! `~/.config/sway/config.d/41-appearance.conf` so the next login gets the
//! same values, and the same commands are sent through `swaymsg` so the
//! current session changes immediately. The bar is different: its position and
//! density already live in `~/.config/waybar/config.jsonc`, so this module
//! edits those lines and signals Waybar instead of adding a second copy.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hayato_ui::Design;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Sharp,
    Rounded,
    Soft,
}

impl Corners {
    fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "sharp" => Some(Self::Sharp),
            "rounded" => Some(Self::Rounded),
            "soft" => Some(Self::Soft),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Sharp => "sharp",
            Self::Rounded => "rounded",
            Self::Soft => "soft",
        }
    }

    fn label(self) -> &'static str {
        self.as_str()
    }

    fn next(self) -> Self {
        match self {
            Self::Sharp => Self::Rounded,
            Self::Rounded => Self::Soft,
            Self::Soft => Self::Sharp,
        }
    }

    fn radius(self) -> u8 {
        match self {
            Self::Sharp => 0,
            Self::Rounded => 6,
            Self::Soft => 12,
        }
    }

    fn smart(self) -> bool {
        !matches!(self, Self::Sharp)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blur {
    Off,
    Frosted,
    Xray,
}

impl Blur {
    fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "off" => Some(Self::Off),
            "frosted" => Some(Self::Frosted),
            "xray" => Some(Self::Xray),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Frosted => "frosted",
            Self::Xray => "xray",
        }
    }

    fn label(self) -> &'static str {
        self.as_str()
    }

    fn next(self) -> Self {
        match self {
            Self::Off => Self::Frosted,
            Self::Frosted => Self::Xray,
            Self::Xray => Self::Off,
        }
    }

    fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    fn xray(self) -> bool {
        matches!(self, Self::Xray)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Animation {
    Off,
    Calm,
    Normal,
}

impl Animation {
    fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "off" => Some(Self::Off),
            "calm" => Some(Self::Calm),
            "normal" => Some(Self::Normal),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Calm => "calm",
            Self::Normal => "normal",
        }
    }

    fn label(self) -> &'static str {
        self.as_str()
    }

    fn next(self) -> Self {
        match self {
            Self::Off => Self::Calm,
            Self::Calm => Self::Normal,
            Self::Normal => Self::Off,
        }
    }

    fn millis(self) -> u16 {
        match self {
            Self::Off => 0,
            Self::Calm => 80,
            Self::Normal => 150,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effects {
    pub corners: Corners,
    pub shadows: bool,
    pub blur: Blur,
    pub animation: Animation,
    /// An absolute image path chosen by the person, or `None` for the theme's
    /// preset wallpaper.
    pub custom_wallpaper: Option<String>,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            corners: Corners::Rounded,
            shadows: true,
            blur: Blur::Frosted,
            animation: Animation::Normal,
            custom_wallpaper: None,
        }
    }
}

fn config_home() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        })
}

fn appearance_path() -> PathBuf {
    config_home().join("hayato/appearance.conf")
}

fn dropin_path() -> PathBuf {
    config_home().join("sway/config.d/41-appearance.conf")
}

fn waybar_path() -> PathBuf {
    config_home().join("waybar/config.jsonc")
}

fn custom_dropin_path() -> PathBuf {
    config_home().join("sway/config.d/36-custom-wallpaper.conf")
}

fn data_home() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".local/share"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        })
}

fn state_home() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".local/state"))
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
        })
}

fn custom_wallpaper_dir() -> PathBuf {
    data_home().join("hayato/wallpapers")
}

fn parse_effects(text: &str) -> Effects {
    let mut effects = Effects::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "corners" => {
                if let Some(corners) = Corners::parse(value) {
                    effects.corners = corners;
                }
            }
            "shadows" => effects.shadows = matches!(value, "on" | "yes" | "true" | "1"),
            "blur" => {
                if let Some(blur) = Blur::parse(value) {
                    effects.blur = blur;
                }
            }
            "animations" => {
                if let Some(animation) = Animation::parse(value) {
                    effects.animation = animation;
                }
            }
            "wallpaper" => {
                effects.custom_wallpaper = if value.is_empty() || value == "theme" {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            _ => {}
        }
    }
    effects
}

fn effects_body(effects: &Effects) -> String {
    format!(
        "# Written by Settings -> Appearance.\n\
         corners={}\n\
         shadows={}\n\
         blur={}\n\
         animations={}\n\
         wallpaper={}\n",
        effects.corners.as_str(),
        if effects.shadows { "on" } else { "off" },
        effects.blur.as_str(),
        effects.animation.as_str(),
        effects.custom_wallpaper.as_deref().unwrap_or("theme")
    )
}

pub fn load() -> Effects {
    std::fs::read_to_string(appearance_path())
        .map(|text| parse_effects(&text))
        .unwrap_or_default()
}

fn write_atomic(path: &PathBuf, body: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension("new");
    std::fs::write(&temp, body).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

fn dropin_body(effects: &Effects) -> String {
    format!(
        "# Written by Settings -> Appearance. Delete this file to go back to\n\
         # the session defaults in /etc/sway/config.d/26-swayfx.conf.\n\
         corner_radius {}\n\
         smart_corner_radius {}\n\
         shadows {}\n\
         blur {}\n\
         blur_xray {}\n\
         animation_duration_ms {}\n",
        effects.corners.radius(),
        if effects.corners.smart() {
            "enable"
        } else {
            "disable"
        },
        if effects.shadows { "enable" } else { "disable" },
        if effects.blur.enabled() {
            "enable"
        } else {
            "disable"
        },
        if effects.blur.xray() {
            "enable"
        } else {
            "disable"
        },
        effects.animation.millis()
    )
}

fn live_commands(effects: &Effects) -> Vec<String> {
    vec![
        format!("corner_radius {}", effects.corners.radius()),
        format!(
            "smart_corner_radius {}",
            if effects.corners.smart() {
                "enable"
            } else {
                "disable"
            }
        ),
        format!(
            "shadows {}",
            if effects.shadows { "enable" } else { "disable" }
        ),
        format!(
            "blur {}",
            if effects.blur.enabled() {
                "enable"
            } else {
                "disable"
            }
        ),
        format!(
            "blur_xray {}",
            if effects.blur.xray() {
                "enable"
            } else {
                "disable"
            }
        ),
        format!("animation_duration_ms {}", effects.animation.millis()),
    ]
}

fn apply(effects: &Effects) -> Result<(), String> {
    write_atomic(&appearance_path(), &effects_body(effects))?;
    write_atomic(&dropin_path(), &dropin_body(effects))?;
    for command in live_commands(effects) {
        let _ = Command::new("swaymsg")
            .arg(&command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

fn change(update: impl FnOnce(&mut Effects)) -> Result<String, String> {
    let mut effects = load();
    update(&mut effects);
    apply(&effects)?;
    Ok(status())
}

pub fn status() -> String {
    let effects = load();
    format!(
        "{} corners · shadows {} · blur {} · animations {} · bar {} {}",
        effects.corners.label(),
        if effects.shadows { "on" } else { "off" },
        effects.blur.label(),
        effects.animation.label(),
        bar_position_label(),
        bar_density_label()
    )
}

pub fn corners_label() -> String {
    load().corners.label().to_string()
}

pub fn shadows_label() -> String {
    if load().shadows {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

pub fn blur_label() -> String {
    load().blur.label().to_string()
}

pub fn animation_label() -> String {
    load().animation.label().to_string()
}

pub fn cycle_corners() -> Result<String, String> {
    change(|effects| effects.corners = effects.corners.next())
}

pub fn toggle_shadows() -> Result<String, String> {
    change(|effects| effects.shadows = !effects.shadows)
}

pub fn cycle_blur() -> Result<String, String> {
    change(|effects| effects.blur = effects.blur.next())
}

pub fn cycle_animations() -> Result<String, String> {
    change(|effects| effects.animation = effects.animation.next())
}

pub fn theme_name() -> String {
    let paths = crate::theme::paths();
    let slug = hayato_theme::store::active(&paths).unwrap_or_else(|| "tokyo-night".to_string());
    crate::theme::theme(&slug)
        .map(|theme| theme.name)
        .unwrap_or(slug)
}

pub fn has_custom_wallpaper() -> bool {
    load().custom_wallpaper.is_some()
}

pub fn wallpaper_name() -> String {
    if let Some(path) = load().custom_wallpaper {
        return Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| format!("custom: {name}"))
            .unwrap_or(path);
    }
    let paths = crate::theme::paths();
    std::fs::read_to_string(paths.state.join("wallpaper"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "theme default".to_string())
}

fn is_picture(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        crate::files::open::family(name, false),
        crate::files::open::Family::Picture
    )
}

fn safe_extension(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let extension: String = extension
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if extension.is_empty() {
        "png".to_string()
    } else {
        extension
    }
}

fn custom_dropin_body(path: &Path) -> String {
    format!(
        "# Written by Settings -> Appearance -> Wallpaper. Delete this file to\n\
         # return to the theme's preset wallpaper.\n\
         output * bg \"{}\" fill\n",
        path.display()
    )
}

fn live_set_wallpaper(path: &Path) {
    if std::env::var("WAYLAND_DISPLAY").is_err() {
        return;
    }
    let _ = Command::new("swaymsg")
        .arg(format!("output * bg \"{}\" fill", path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn set_custom_wallpaper(source: &Path) -> Result<String, String> {
    if !is_picture(source) {
        return Err("Choose a picture (PNG, JPEG, WebP, GIF, BMP or TIFF)".to_string());
    }
    let directory = custom_wallpaper_dir();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let destination = directory.join(format!("custom-{stamp}.{}", safe_extension(source)));
    std::fs::copy(source, &destination)
        .map_err(|error| format!("cannot copy {}: {error}", source.display()))?;

    let mut effects = load();
    effects.custom_wallpaper = Some(destination.to_string_lossy().to_string());
    write_atomic(&appearance_path(), &effects_body(&effects))?;
    write_atomic(&custom_dropin_path(), &custom_dropin_body(&destination))?;
    live_set_wallpaper(&destination);
    Ok(format!(
        "Wallpaper: custom {}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
    ))
}

fn theme_wallpaper_path() -> PathBuf {
    let paths = crate::theme::paths();
    let name = std::fs::read_to_string(paths.state.join("wallpaper"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            let slug = hayato_theme::store::active(&paths)?;
            crate::theme::theme(&slug)
                .ok()
                .and_then(|t| t.wallpapers.into_iter().next())
        })
        .unwrap_or_else(|| "hayato.png".to_string());
    paths.home.join("Pictures/Wallpapers").join(name)
}

/// Removes the custom wallpaper drop-in and resets `wallpaper=theme` in
/// `appearance.conf` without running `swaymsg`.
pub fn clear_custom_wallpaper_files() {
    let mut effects = load();
    if effects.custom_wallpaper.is_some() {
        effects.custom_wallpaper = None;
        let _ = write_atomic(&appearance_path(), &effects_body(&effects));
    }
    let _ = std::fs::remove_file(custom_dropin_path());
}

pub fn clear_custom_wallpaper() -> Result<String, String> {
    clear_custom_wallpaper_files();
    let preset = theme_wallpaper_path();
    live_set_wallpaper(&preset);
    Ok(format!(
        "Wallpaper: {}",
        preset
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("theme preset")
    ))
}

pub fn cycle_wallpaper() -> Result<String, String> {
    clear_custom_wallpaper_files();
    let paths = crate::theme::paths();
    let (wallpaper, _) = crate::theme::cycle_wallpaper_for(&paths, None)?;
    Ok(format!("Wallpaper: {wallpaper}"))
}

pub fn pick_wallpaper(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<Option<String>, crate::Failure> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let pictures = home.join("Pictures");
    let start = if pictures.is_dir() {
        pictures
    } else {
        home.clone()
    };
    let setup = crate::files::browse::Setup {
        dir: start,
        mode: crate::files::browse::Mode::Browse,
        hidden: false,
        home: home.clone(),
        memory: state_home().join("hayato/files-last"),
        path_var: std::env::var("PATH").unwrap_or_default(),
    };
    let mut screen = crate::files::browse::FilesScreen::new(
        setup,
        Some(crate::files::preview::Previewer::for_session(&home)),
    );
    let _ = crate::run_screen(terminal, design, "Choose a wallpaper", &mut screen)?;
    match screen.chosen.take() {
        Some(crate::files::Chosen::Open(path)) => Ok(Some(match set_custom_wallpaper(&path) {
            Ok(message) => message,
            Err(reason) => reason,
        })),
        Some(crate::files::Chosen::Actions(_) | crate::files::Chosen::Run(_)) => Ok(None),
        None => Ok(None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPosition {
    Top,
    Bottom,
}

impl BarPosition {
    fn label(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarModules {
    Full,
    Compact,
    Minimal,
}

impl BarModules {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Minimal => "minimal",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Full => Self::Compact,
            Self::Compact => Self::Minimal,
            Self::Minimal => Self::Full,
        }
    }

    fn modules(self) -> &'static [&'static str] {
        match self {
            Self::Full => &[
                "privacy",
                "custom/rec",
                "custom/cassette",
                "custom/busy",
                "custom/keyboard-layout",
                "custom/vnc",
                "custom/drives",
                "custom/bluetooth",
                "network",
                "pulseaudio",
                "backlight",
                "idle_inhibitor",
                "battery",
                "custom/updates",
                "tray",
            ],
            Self::Compact => &[
                "privacy",
                "custom/rec",
                "custom/cassette",
                "custom/busy",
                "custom/bluetooth",
                "custom/vnc",
                "custom/drives",
                "network",
                "pulseaudio",
                "battery",
                "tray",
            ],
            Self::Minimal => &[
                "custom/rec",
                "custom/busy",
                "network",
                "pulseaudio",
                "battery",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarDensity {
    Comfortable,
    Compact,
}

impl BarDensity {
    fn label(self) -> &'static str {
        match self {
            Self::Comfortable => "comfortable",
            Self::Compact => "compact",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Comfortable => Self::Compact,
            Self::Compact => Self::Comfortable,
        }
    }

    fn height(self) -> u16 {
        match self {
            Self::Comfortable => 30,
            // 29 is the smallest height Waybar's modules accept on this bar;
            // 24 made Waybar clamp it and log a warning on every reload.
            Self::Compact => 29,
        }
    }

    fn spacing(self) -> u16 {
        match self {
            Self::Comfortable => 4,
            Self::Compact => 2,
        }
    }
}

fn bar_text() -> Result<String, String> {
    std::fs::read_to_string(waybar_path())
        .map_err(|error| format!("cannot read {}: {error}", waybar_path().display()))
}

pub fn bar_position() -> BarPosition {
    bar_text()
        .map(|text| bar_position_from(&text))
        .unwrap_or(BarPosition::Top)
}

pub fn bar_density() -> BarDensity {
    bar_text()
        .map(|text| bar_density_from(&text))
        .unwrap_or(BarDensity::Comfortable)
}

pub fn bar_position_label() -> String {
    bar_position().label().to_string()
}

pub fn bar_density_label() -> String {
    bar_density().label().to_string()
}

pub fn bar_modules_label() -> String {
    bar_text()
        .ok()
        .and_then(|text| bar_modules_from(&text))
        .map(|modules| modules.label().to_string())
        .unwrap_or_else(|| "custom".to_string())
}

fn modules_right(text: &str) -> Vec<String> {
    let mut inside = false;
    let mut modules = Vec::new();
    for line in text.lines() {
        if !inside {
            if line.contains("\"modules-right\":") {
                inside = true;
            }
            continue;
        }
        if line.trim_start().starts_with(']') {
            break;
        }
        for part in line.split('"').skip(1).step_by(2) {
            if !part.is_empty() {
                modules.push(part.to_string());
            }
        }
    }
    modules
}

fn bar_modules_from(text: &str) -> Option<BarModules> {
    let current = modules_right(text);
    let current: Vec<&str> = current.iter().map(String::as_str).collect();
    [BarModules::Full, BarModules::Compact, BarModules::Minimal]
        .into_iter()
        .find(|modules| modules.modules() == current.as_slice())
}

fn bar_modules_body(text: &str, modules: BarModules) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|line| line.contains("\"modules-right\":"))
    else {
        return text.to_string();
    };
    let Some(end) = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim_start().starts_with(']'))
        .map(|(index, _)| index)
    else {
        return text.to_string();
    };
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let item_indent = format!("{indent}    ");
    let comma = if lines[end].trim_end().ends_with(',') {
        ","
    } else {
        ""
    };
    let mut out: Vec<String> = lines[..start].iter().map(|line| line.to_string()).collect();
    out.push(format!("{indent}\"modules-right\": ["));
    for module in modules.modules() {
        out.push(format!("{item_indent}\"{module}\","));
    }
    out.push(format!("{indent}]{comma}"));
    out.extend(lines[end + 1..].iter().map(|line| line.to_string()));
    let mut body = out.join("\n");
    if text.ends_with('\n') {
        body.push('\n');
    }
    body
}

pub fn toggle_bar_modules() -> Result<String, String> {
    let text = bar_text()?;
    let next = bar_modules_from(&text)
        .map(BarModules::next)
        .unwrap_or(BarModules::Full);
    write_atomic(&waybar_path(), &bar_modules_body(&text, next))?;
    signal_waybar();
    Ok(format!("Bar modules: {}", next.label()))
}

fn bar_position_from(text: &str) -> BarPosition {
    for line in text.lines() {
        if line.contains("\"position\":") {
            if line.contains("\"bottom\"") {
                return BarPosition::Bottom;
            }
            if line.contains("\"top\"") {
                return BarPosition::Top;
            }
        }
    }
    BarPosition::Top
}

fn bar_density_from(text: &str) -> BarDensity {
    let mut height = None;
    let mut spacing = None;
    for line in text.lines() {
        if height.is_none() && line.contains("\"height\":") {
            height = number_after(line, "\"height\":");
        }
        if spacing.is_none() && line.contains("\"spacing\":") {
            spacing = number_after(line, "\"spacing\":");
        }
        if height.is_some() && spacing.is_some() {
            break;
        }
    }
    let height = height.unwrap_or(30);
    let spacing = spacing.unwrap_or(4);
    if height <= 26 || spacing <= 2 {
        BarDensity::Compact
    } else {
        BarDensity::Comfortable
    }
}

fn number_after(line: &str, key: &str) -> Option<u16> {
    let rest = &line[line.find(key)? + key.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn string_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = &line[line.find(key)? + key.len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(&rest[start..end])
}

fn replace_string(line: &str, key: &str, value: &str) -> String {
    let Some(old) = string_after(line, key) else {
        return line.to_string();
    };
    line.replacen(&format!("\"{old}\""), &format!("\"{value}\""), 1)
}

fn replace_number(line: &str, key: &str, value: u16) -> String {
    let Some(old) = number_after(line, key) else {
        return line.to_string();
    };
    let old = old.to_string();
    let start = line.find(key).unwrap_or(0) + key.len();
    let rest = &line[start..];
    let offset = rest.find(&old).unwrap_or(0) + start;
    format!(
        "{}{}{}",
        &line[..offset],
        value,
        &line[offset + old.len()..]
    )
}

fn bar_body(text: &str, position: BarPosition, density: BarDensity) -> String {
    let mut out = Vec::new();
    let mut position_done = false;
    let mut margin_done = false;
    let mut height_done = false;
    let mut spacing_done = false;
    for line in text.lines() {
        let mut line = line.to_string();
        if !position_done && line.contains("\"position\":") {
            line = replace_string(&line, "\"position\":", position.label());
            position_done = true;
        }
        if !margin_done {
            if matches!(position, BarPosition::Bottom) && line.contains("\"margin-top\":") {
                line = line.replacen("\"margin-top\":", "\"margin-bottom\":", 1);
                margin_done = true;
            } else if matches!(position, BarPosition::Top) && line.contains("\"margin-bottom\":") {
                line = line.replacen("\"margin-bottom\":", "\"margin-top\":", 1);
                margin_done = true;
            }
        }
        if !height_done && line.contains("\"height\":") {
            line = replace_number(&line, "\"height\":", density.height());
            height_done = true;
        }
        if !spacing_done && line.contains("\"spacing\":") {
            line = replace_number(&line, "\"spacing\":", density.spacing());
            spacing_done = true;
        }
        out.push(line);
    }
    let mut body = out.join("\n");
    if text.ends_with('\n') {
        body.push('\n');
    }
    body
}

fn waybar_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "waybar"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn signal_waybar() {
    let was_running = waybar_running();
    let _ = Command::new("pkill")
        .args(["-SIGUSR2", "waybar"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // A reload that leaves no bar is worse than a settings change that did not
    // apply yet. Only restart one that was already up; a session that does not
    // use Waybar is left alone.
    std::thread::sleep(std::time::Duration::from_millis(250));
    if was_running && !waybar_running() {
        let config = waybar_path().to_string_lossy().to_string();
        let style = config_home()
            .join("waybar/style.css")
            .to_string_lossy()
            .to_string();
        crate::system::spawn_detached_argv("waybar", &["-c", &config, "-s", &style]);
    }
}

fn set_bar(position: BarPosition, density: BarDensity) -> Result<(), String> {
    let text = bar_text()?;
    write_atomic(&waybar_path(), &bar_body(&text, position, density))?;
    signal_waybar();
    Ok(())
}

pub fn toggle_bar_position() -> Result<String, String> {
    let position = bar_position().next();
    set_bar(position, bar_density())?;
    Ok(format!(
        "Bar: {} · {}",
        position.label(),
        bar_density().label()
    ))
}

pub fn toggle_bar_density() -> Result<String, String> {
    let density = bar_density().next();
    set_bar(bar_position(), density)?;
    Ok(format!(
        "Bar: {} · {}",
        bar_position().label(),
        density.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_round_trip_and_cycle() {
        let effects = Effects {
            corners: Corners::Soft,
            shadows: false,
            blur: Blur::Xray,
            animation: Animation::Calm,
            custom_wallpaper: Some("/home/b/Pictures/wall.jpg".to_string()),
        };
        assert_eq!(parse_effects(&effects_body(&effects)), effects);
        assert_eq!(Corners::Sharp.next(), Corners::Rounded);
        assert_eq!(Blur::Xray.next(), Blur::Off);
        assert_eq!(Animation::Normal.next(), Animation::Off);
    }

    #[test]
    fn a_custom_wallpaper_round_trips_and_is_quoted() {
        let effects = Effects {
            custom_wallpaper: Some("/home/b/My Wall.jpg".to_string()),
            ..Default::default()
        };
        assert_eq!(parse_effects(&effects_body(&effects)), effects);
        let body = custom_dropin_body(Path::new("/home/b/My Wall.jpg"));
        assert!(body.contains("output * bg \"/home/b/My Wall.jpg\" fill"));
    }

    #[test]
    fn the_dropin_is_the_live_command_list() {
        let effects = Effects::default();
        let body = dropin_body(&effects);
        assert!(body.contains("corner_radius 6"));
        assert!(body.contains("smart_corner_radius enable"));
        assert!(body.contains("shadows enable"));
        assert!(body.contains("blur enable"));
        assert!(body.contains("blur_xray disable"));
        assert!(body.contains("animation_duration_ms 150"));
    }

    #[test]
    fn bar_lines_are_replaced_without_touching_comments() {
        let text = "// top bar\n{\n    \"position\": \"top\",\n    \"height\": 30,\n    \"spacing\": 4,\n    \"margin-top\": 4,\n    \"tray\": { \"spacing\": 10 }\n}\n";
        let body = bar_body(text, BarPosition::Bottom, BarDensity::Compact);
        assert!(body.contains("\"position\": \"bottom\""));
        assert!(body.contains("\"height\": 29"));
        assert!(body.contains("\"spacing\": 2"));
        assert!(body.contains("\"margin-bottom\": 4"));
        assert!(body.contains("// top bar"));
        assert!(body.ends_with('\n'));
        assert_eq!(bar_position_from(&body), BarPosition::Bottom);
        assert_eq!(bar_density_from(&body), BarDensity::Compact);
    }

    #[test]
    fn a_bar_without_effects_defaults_to_top_comfortable() {
        assert_eq!(bar_position_from("{}"), BarPosition::Top);
        assert_eq!(bar_density_from("{}"), BarDensity::Comfortable);
    }

    fn strip_jsonc(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        let mut in_string = false;
        let mut escaped = false;
        while let Some(ch) = chars.next() {
            if in_string {
                out.push(ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            if ch == '"' {
                in_string = true;
                out.push(ch);
                continue;
            }
            if ch == '/' && chars.peek() == Some(&'/') {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
                continue;
            }
            out.push(ch);
        }
        let mut cleaned = String::new();
        let bytes = out.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b',' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                    i += 1;
                    continue;
                }
            }
            cleaned.push(bytes[i] as char);
            i += 1;
        }
        cleaned
    }

    #[test]
    fn every_bar_combination_remains_valid_jsonc() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../uis/sway/files/home/.config/waybar/config.jsonc");
        let text = std::fs::read_to_string(&path).unwrap();
        for position in [BarPosition::Top, BarPosition::Bottom] {
            for density in [BarDensity::Comfortable, BarDensity::Compact] {
                let body = bar_body(&text, position, density);
                let json = strip_jsonc(&body);
                serde_json::from_str::<serde_json::Value>(&json)
                    .unwrap_or_else(|error| panic!("{position:?}/{density:?}: {error}\n{body}"));
            }
        }
    }

    #[test]
    fn bar_module_presets_replace_only_the_right_module_list() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../uis/sway/files/home/.config/waybar/config.jsonc");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(bar_modules_from(&text), Some(BarModules::Full));
        for modules in [BarModules::Compact, BarModules::Minimal] {
            let body = bar_modules_body(&text, modules);
            assert_eq!(bar_modules_from(&body), Some(modules));
            serde_json::from_str::<serde_json::Value>(&strip_jsonc(&body)).unwrap();
        }
        let compact = bar_modules_body(&text, BarModules::Compact);
        assert_eq!(bar_modules_from(&compact), Some(BarModules::Compact));
        assert!(compact.contains("\"modules-left\""));
        assert!(compact.contains("\"tray\""));
    }
}
