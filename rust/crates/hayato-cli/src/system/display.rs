//! External display arrangement studio: live spatial topology, resolution, scale (`hayato-display`).
//!
//! The monitor is placed relative to the internal panel (never moved).
//! A runtime arrangement is also written to `~/.config/sway/config.d/40-display.conf`
//! so it survives logout. Only the monitor's lines are written; the panel is
//! untouched because disabling it would boot a black screen.
//!
//! The blind `--panel-on` path runs from a key with no menu: it enables the
//! internal panel when both outputs are off.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use hayato_ui::widgets::{put, put_trunc};
use hayato_ui::{
    BodyCtx, ColorRole, Design, Glyph, Header, KeyHint, Outcome, Screen, TextStyle, draw_card,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{checked, out, print};

/// What the menu needs before it offers anything.
pub const NEEDED: &[&str] = &["swaymsg", "jq"];

/// Supported display scales.
const SCALES: &[&str] = &["1", "1.25", "1.5", "1.75", "2"];

/// Supported relative monitor positions.
const POSITIONS: &[&str] = &["right", "left", "above", "below", "mirror"];

/// The config file path.
fn config_path() -> PathBuf {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .and_then(|v| {
            let p = PathBuf::from(v);
            if p.is_absolute() { Some(p) } else { None }
        })
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(".config")
        });
    config_home.join("sway/config.d/40-display.conf")
}

/// Default internal panel name when no output is reported by sway.
const DEFAULT_PANEL: &str = "eDP-1";

fn internal_panel_name(outputs: &[Output]) -> String {
    find_monitor_and_panel(outputs)
        .1
        .map(|p| p.name)
        .unwrap_or_else(|| DEFAULT_PANEL.to_string())
}

/// The command `--panel-on` runs to recover from a black screen.
pub fn panel_on_command() -> String {
    format!(
        "swaymsg output {} enable",
        internal_panel_name(&read_outputs())
    )
}

/// An output's live state from `swaymsg -t get_outputs`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub mode_width: u32,
    pub mode_height: u32,
    pub mode_refresh: u32,
    pub active: bool,
}

impl Output {
    fn mode_str(&self) -> String {
        if self.mode_width > 0 && self.mode_height > 0 {
            format!(
                "{}x{}@{}Hz",
                self.mode_width,
                self.mode_height,
                hz(self.mode_refresh)
            )
        } else {
            format!("{}x{}", self.width, self.height)
        }
    }

    fn short_res(&self) -> String {
        if self.mode_width > 0 && self.mode_height > 0 {
            format!(
                "{}×{} · {}Hz",
                self.mode_width,
                self.mode_height,
                hz(self.mode_refresh)
            )
        } else if self.width > 0 && self.height > 0 {
            format!("{}×{}", self.width, self.height)
        } else {
            "off".to_string()
        }
    }
}

/// Parse `swaymsg -t get_outputs -r` JSON array into `Output` structs.
fn parse_outputs(json: &str) -> Vec<Output> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    let mut outputs = Vec::new();
    for v in items {
        let Some(name) = v["name"].as_str() else {
            continue;
        };
        let active = v["active"].as_bool().unwrap_or(false);
        let scale = v["scale"].as_f64().unwrap_or(1.0) as f32;
        let rect = &v["rect"];
        let x = rect["x"].as_i64().unwrap_or(0) as i32;
        let y = rect["y"].as_i64().unwrap_or(0) as i32;
        let width = rect["width"].as_u64().unwrap_or(0) as u32;
        let height = rect["height"].as_u64().unwrap_or(0) as u32;
        let cur = &v["current_mode"];
        let mode_width = cur["width"].as_u64().unwrap_or(0) as u32;
        let mode_height = cur["height"].as_u64().unwrap_or(0) as u32;
        let mode_refresh = cur["refresh"].as_u64().unwrap_or(0) as u32;
        outputs.push(Output {
            name: name.to_string(),
            x,
            y,
            width,
            height,
            scale,
            mode_width,
            mode_height,
            mode_refresh,
            active,
        });
    }
    outputs
}

fn is_internal_panel_name(name: &str) -> bool {
    name.starts_with("eDP-")
        || name.starts_with("DSI-")
        || name.starts_with("LVDS-")
        || name.starts_with("DPI-")
}

/// The active outputs, internal panel first, for the workspace picker.
pub fn active_outputs() -> Vec<String> {
    let mut names: Vec<String> = read_outputs()
        .into_iter()
        .filter(|output| output.active)
        .map(|output| output.name)
        .collect();
    names.sort_by_key(|name| {
        let internal = is_internal_panel_name(name);
        (!internal, name.clone())
    });
    names
}

/// The output that currently has focus, when sway reports one.
pub fn focused_output() -> Option<String> {
    let text = out(&["swaymsg", "-t", "get_outputs", "-r"])?;
    let outputs = serde_json::from_str::<Vec<serde_json::Value>>(&text).ok()?;
    outputs
        .iter()
        .find(|output| output["focused"].as_bool().unwrap_or(false))
        .and_then(|output| output["name"].as_str())
        .map(str::to_string)
}

/// Read all outputs from sway.
fn read_outputs() -> Vec<Output> {
    let output = out(&["swaymsg", "-t", "get_outputs", "-r"]).unwrap_or_default();
    parse_outputs(&output)
}

/// Find the external monitor (DP-* or HDMI-*) and the internal panel (eDP-*, DSI-*, LVDS-*, DPI-*).
fn find_monitor_and_panel(outputs: &[Output]) -> (Option<Output>, Option<Output>) {
    let mut mon = None;
    let mut panel: Option<Output> = None;
    for o in outputs {
        if o.name.starts_with("DP-") || o.name.starts_with("HDMI-") {
            mon = Some(o.clone());
        } else if o.name.starts_with("eDP-") || (is_internal_panel_name(&o.name) && panel.is_none())
        {
            panel = Some(o.clone());
        }
    }
    (mon, panel)
}

/// Convert mHz to Hz string.
fn hz(mhz: u32) -> String {
    let hz = mhz / 1000;
    if mhz.is_multiple_of(1000) {
        format!("{hz}")
    } else {
        format!("{}.{:03}", hz, mhz % 1000)
            .trim_end_matches('0')
            .to_string()
    }
}

/// Determine the arrangement name from positions.
fn arrangement(mon: &Output, panel: &Output) -> String {
    if !mon.active || !panel.active {
        return "".to_string();
    }
    if mon.y == panel.y && mon.x == panel.x {
        "mirror".to_string()
    } else if mon.y == panel.y && mon.x == panel.x + panel.width as i32 {
        "right".to_string()
    } else if mon.y == panel.y && mon.x + mon.width as i32 == panel.x {
        "left".to_string()
    } else if mon.x == panel.x && mon.y + mon.height as i32 == panel.y {
        "above".to_string()
    } else if mon.x == panel.x && mon.y == panel.y + panel.height as i32 {
        "below".to_string()
    } else {
        "custom".to_string()
    }
}

/// Save the current arrangement to the config file.
fn save_config(mon: &Output) -> Result<(), String> {
    if !mon.active {
        return Ok(());
    }
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;

    let refresh_hz = hz(mon.mode_refresh);
    let mode = if mon.mode_width > 0 {
        format!("{}x{}@{}Hz", mon.mode_width, mon.mode_height, refresh_hz)
    } else {
        format!("{}x{}", mon.width, mon.height)
    };
    let scale = format!("{}", mon.scale);

    let content = format!(
        "# Written by hayato menu display (Settings -> Display arrangement).\n\
         # The monitor is the output that can be absent, so a boot without it is\n\
         # untouched.  Delete this file to go back to sway default (on the right).\n\
         output {} resolution {}\n\
         output {} scale {}\n\
         output {} position {} {}\n",
        mon.name, mode, mon.name, scale, mon.name, mon.x, mon.y
    );

    let tmp = format!("{}.tmp.{}", path.display(), std::process::id());
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Parse available modes for the monitor from `swaymsg -t get_outputs -r` JSON.
fn parse_modes(json: &str, mon_name: &str) -> Vec<String> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return Vec::new();
    };
    let mut modes = Vec::new();
    for v in items {
        if v["name"].as_str() == Some(mon_name) {
            if let Some(modes_arr) = v["modes"].as_array() {
                for m in modes_arr {
                    let w = m["width"].as_u64().unwrap_or(0);
                    let h = m["height"].as_u64().unwrap_or(0);
                    let r = m["refresh"].as_u64().unwrap_or(0);
                    let hz_str = hz(r as u32);
                    modes.push(format!("{}x{}@{}Hz", w, h, hz_str));
                }
            }
            break;
        }
    }
    modes.sort();
    modes.dedup();
    modes
}

/// Get available modes for the monitor.
fn get_modes(mon_name: &str) -> Vec<String> {
    let output = out(&["swaymsg", "-t", "get_outputs", "-r"]).unwrap_or_default();
    parse_modes(&output, mon_name)
}

/// The `--status` output (also used by the Settings row).
pub fn status() -> String {
    status_for(&read_outputs())
}

fn status_for(outputs: &[Output]) -> String {
    let (mon, panel) = find_monitor_and_panel(outputs);

    match (mon, panel) {
        (None, Some(p)) if !p.active => "internal panel off".to_string(),
        (None, _) => "no external monitor".to_string(),
        (Some(m), _) if !m.active => "monitor disabled".to_string(),
        (Some(_m), Some(p)) if !p.active => "monitor only".to_string(),
        (Some(m), Some(p)) => {
            let arr = arrangement(&m, &p);
            match arr.as_str() {
                "right" => "right of the panel".to_string(),
                "left" => "left of the panel".to_string(),
                "above" => "above the panel".to_string(),
                "below" => "below the panel".to_string(),
                "mirror" => "mirrored".to_string(),
                _ => "custom position".to_string(),
            }
        }
        _ => "unknown".to_string(),
    }
}

/// The `--panel-on` blind key handler.
pub fn panel_on() -> Result<(), String> {
    let panel = internal_panel_name(&read_outputs());
    if let Err(e) = checked(&["swaymsg", "output", &panel, "enable"]) {
        return Err(format!("Could not enable panel: {e}"));
    }
    print(&format!("{panel} is back on"));
    Ok(())
}

/// Build the `(key, label, detail)` rows for `hayato menu display --rows`.
fn rows_for(outputs: &[Output]) -> Vec<(String, String, String)> {
    let (mon, panel) = find_monitor_and_panel(outputs);
    let panel_name = panel
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| DEFAULT_PANEL.to_string());
    let mut list = Vec::new();

    let Some(mon) = mon else {
        let panel_active = panel.as_ref().map(|p| p.active).unwrap_or(false);
        if !panel_active {
            list.push((
                "enable:panel".to_string(),
                "Bring the internal panel back".to_string(),
                format!("{panel_name} enable"),
            ));
        }
        list.push((
            "wd".to_string(),
            "Open wdisplays".to_string(),
            "arrange by dragging".to_string(),
        ));
        return list;
    };

    let panel = panel.unwrap_or_default();
    let cur_arr = arrangement(&mon, &panel);

    if mon.active && panel.active {
        for a in POSITIONS {
            let (name, hint) = match *a {
                "right" => ("Monitor on the right", "panel | monitor"),
                "left" => ("Monitor on the left", "monitor | panel"),
                "above" => ("Monitor above", "monitor over the panel"),
                "below" => ("Monitor below", "monitor under the panel"),
                "mirror" => ("Mirrored", "both show the same picture"),
                _ => continue,
            };
            let detail = if *a == cur_arr { "current" } else { hint };
            list.push((format!("pos:{a}"), name.to_string(), detail.to_string()));
        }
    }

    if panel.active {
        list.push((
            "mono:monitor".to_string(),
            "Only the monitor".to_string(),
            "internal panel off · not remembered".to_string(),
        ));
    } else {
        list.push((
            "enable:panel".to_string(),
            "Bring the internal panel back".to_string(),
            format!("{panel_name} enable"),
        ));
    }
    if mon.active {
        list.push((
            "mono:panel".to_string(),
            "Only the internal panel".to_string(),
            "monitor off · not remembered".to_string(),
        ));
    } else {
        list.push((
            "enable:monitor".to_string(),
            "Bring the monitor back".to_string(),
            format!("{} enable", mon.name),
        ));
    }

    list.push((
        "res".to_string(),
        "Resolution".to_string(),
        format!("{} @ {} Hz", mon.mode_str(), hz(mon.mode_refresh)),
    ));
    list.push((
        "scale".to_string(),
        "Scale".to_string(),
        format!("{}", mon.scale),
    ));
    list.push((
        "wd".to_string(),
        "Open wdisplays".to_string(),
        "arrange by dragging".to_string(),
    ));
    list
}

/// The rows `hayato menu display --rows` prints for `check-system.sh`.
pub fn rows() -> Vec<(String, String, String)> {
    rows_for(&read_outputs())
}

/// Apply arrangement and save.
fn apply_and_save(arr: &str, mon: &Output, panel: &Output) -> Result<(), String> {
    let (x, y) = match arr {
        "right" => (panel.x + panel.width as i32, panel.y),
        "left" => (panel.x - mon.width as i32, panel.y),
        "above" => (panel.x, panel.y - mon.height as i32),
        "below" => (panel.x, panel.y + panel.height as i32),
        "mirror" => (panel.x, panel.y),
        _ => return Err("unknown arrangement".to_string()),
    };
    if let Err(e) = checked(&[
        "swaymsg",
        "--",
        "output",
        &mon.name,
        "position",
        &x.to_string(),
        &y.to_string(),
    ]) {
        return Err(format!("Could not position monitor: {e}"));
    }
    let mut updated = mon.clone();
    updated.x = x;
    updated.y = y;
    save_config(&updated)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Spatial Display Arrangement Studio
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayPhase {
    DualMonitor,
    SinglePanel,
    PanelOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlRow {
    Position,
    Resolution,
    Scale,
    PanelPower,
    MonitorPower,
    ApplySave,
    EnablePanel,
    OpenWdisplays,
}

#[derive(Clone, Debug, PartialEq)]
struct DisplayDraft {
    pos_idx: usize,
    mode_idx: usize,
    scale_idx: usize,
    panel_active: bool,
    mon_active: bool,
}

/// Interactive Spatial Display Arrangement Studio.
pub struct DisplayScreen {
    outputs: Vec<Output>,
    modes: Vec<String>,
    phase: DisplayPhase,
    live: DisplayDraft,
    draft: DisplayDraft,
    controls: Vec<ControlRow>,
    cursor: usize,
    banner: Option<(String, bool)>,
    header: Header,
    keybar: Vec<KeyHint>,
    row_hits: RefCell<Vec<Rect>>,
    rows_area: Cell<Rect>,
}

impl Default for DisplayScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayScreen {
    /// Build the Display Studio from live `swaymsg` outputs.
    pub fn new() -> Self {
        let outputs = read_outputs();
        let modes = find_monitor_and_panel(&outputs)
            .0
            .map(|m| get_modes(&m.name))
            .unwrap_or_default();
        Self::from_outputs(outputs, modes)
    }

    /// Deterministic constructor for unit tests and golden snapshots.
    pub fn from_outputs(outputs: Vec<Output>, mut modes: Vec<String>) -> Self {
        let (mon, panel) = find_monitor_and_panel(&outputs);
        let phase = match (&mon, &panel) {
            (Some(_), _) => DisplayPhase::DualMonitor,
            (None, Some(p)) if !p.active => DisplayPhase::PanelOff,
            (None, _) => DisplayPhase::SinglePanel,
        };

        if let Some(m) = &mon {
            let cur = m.mode_str();
            if !modes.contains(&cur) && !cur.is_empty() {
                modes.push(cur);
            }
        }

        let live = Self::draft_from_outputs(&outputs, &modes);
        let draft = live.clone();
        let controls = Self::controls_for(phase);

        let mut screen = Self {
            outputs,
            modes,
            phase,
            live,
            draft,
            controls,
            cursor: 0,
            banner: None,
            header: Header::new("Hayato").subtitle("Display Studio"),
            keybar: vec![
                KeyHint::new("←→", "adjust"),
                KeyHint::new("↑↓", "select"),
                KeyHint::new("Ctrl+S", "apply & save"),
                KeyHint::new("Ctrl+R", "refresh"),
                KeyHint::new("Esc", "close"),
            ],
            row_hits: RefCell::new(Vec::new()),
            rows_area: Cell::new(Rect::default()),
        };
        screen.update_header();
        screen
    }

    fn draft_from_outputs(outputs: &[Output], modes: &[String]) -> DisplayDraft {
        let (mon, panel) = find_monitor_and_panel(outputs);
        let panel_active = panel.as_ref().map(|p| p.active).unwrap_or(true);
        let mon_active = mon.as_ref().map(|m| m.active).unwrap_or(false);

        let pos_idx = match (&mon, &panel) {
            (Some(m), Some(p)) => {
                let arr = arrangement(m, p);
                POSITIONS.iter().position(|&k| k == arr).unwrap_or(0)
            }
            _ => 0,
        };

        let mode_idx = mon
            .as_ref()
            .and_then(|m| {
                let cur = m.mode_str();
                modes.iter().position(|x| x == &cur)
            })
            .unwrap_or(0);

        let scale_idx = mon
            .as_ref()
            .and_then(|m| {
                let cur = format!("{}", m.scale);
                SCALES.iter().position(|&s| s == cur)
            })
            .unwrap_or(0);

        DisplayDraft {
            pos_idx,
            mode_idx,
            scale_idx,
            panel_active,
            mon_active,
        }
    }

    fn controls_for(phase: DisplayPhase) -> Vec<ControlRow> {
        match phase {
            DisplayPhase::DualMonitor => vec![
                ControlRow::Position,
                ControlRow::Resolution,
                ControlRow::Scale,
                ControlRow::PanelPower,
                ControlRow::MonitorPower,
                ControlRow::ApplySave,
                ControlRow::OpenWdisplays,
            ],
            DisplayPhase::PanelOff => vec![ControlRow::EnablePanel, ControlRow::OpenWdisplays],
            DisplayPhase::SinglePanel => vec![ControlRow::OpenWdisplays],
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.phase == DisplayPhase::DualMonitor && self.draft != self.live
    }

    fn update_header(&mut self) {
        let summary = match self.phase {
            DisplayPhase::DualMonitor => {
                let (mon, panel) = find_monitor_and_panel(&self.outputs);
                let p_name = panel
                    .as_ref()
                    .map(|p| p.name.as_str())
                    .unwrap_or(DEFAULT_PANEL);
                let m_name = mon.as_ref().map(|m| m.name.as_str()).unwrap_or("DP-1");
                if self.is_dirty() {
                    format!("{p_name} + {m_name} · unsaved draft")
                } else {
                    format!("{p_name} + {m_name} · {}", status_for(&self.outputs))
                }
            }
            DisplayPhase::PanelOff => "Internal panel disabled · press Enter to recover".into(),
            DisplayPhase::SinglePanel => "Internal panel only · plug in a USB-C monitor".into(),
        };
        self.header = Header::new("Hayato")
            .subtitle("Display Studio")
            .status(summary);
        self.keybar = match self.phase {
            DisplayPhase::DualMonitor => {
                let mut hints = vec![
                    KeyHint::new("←→", "adjust"),
                    KeyHint::new("↑↓", "select"),
                    KeyHint::new("Ctrl+S", "apply & save"),
                ];
                if self.is_dirty() {
                    hints.push(KeyHint::new("Ctrl+Z", "discard"));
                }
                hints.push(KeyHint::new("Ctrl+R", "refresh"));
                hints.push(KeyHint::new("Esc", "close"));
                hints
            }
            DisplayPhase::PanelOff => vec![
                KeyHint::new("Enter", "enable"),
                KeyHint::new("↑↓", "select"),
                KeyHint::new("Ctrl+R", "refresh"),
                KeyHint::new("Esc", "close"),
            ],
            DisplayPhase::SinglePanel => vec![
                KeyHint::new("Enter", "open wdisplays"),
                KeyHint::new("Ctrl+R", "refresh"),
                KeyHint::new("Esc", "close"),
            ],
        };
    }

    fn reload_live(&mut self) {
        let outputs = read_outputs();
        let modes = find_monitor_and_panel(&outputs)
            .0
            .map(|m| get_modes(&m.name))
            .unwrap_or_default();
        let prev_cursor = self.cursor;
        let fresh = Self::from_outputs(outputs, modes);
        self.outputs = fresh.outputs;
        self.modes = fresh.modes;
        self.phase = fresh.phase;
        self.live = fresh.live;
        self.draft = fresh.draft;
        self.controls = fresh.controls;
        self.cursor = prev_cursor.min(self.controls.len().saturating_sub(1));
        self.update_header();
    }

    fn split_panes(area: Rect) -> (Rect, Option<Rect>) {
        if area.width >= super::DUAL_PANE_MIN_WIDTH {
            let gap = 1u16;
            let left_w = ((u32::from(area.width) * 52) / 100)
                .max(48)
                .min(u32::from(area.width.saturating_sub(44 + gap)))
                as u16;
            let right_w = area.width.saturating_sub(left_w + gap);
            (
                Rect::new(area.x, area.y, left_w, area.height),
                Some(Rect::new(
                    area.x + left_w + gap,
                    area.y,
                    right_w,
                    area.height,
                )),
            )
        } else {
            (area, None)
        }
    }

    fn adjust_row(&mut self, row: ControlRow, delta: i32) {
        match row {
            ControlRow::Position => {
                let n = POSITIONS.len() as i32;
                self.draft.pos_idx = ((self.draft.pos_idx as i32 + delta).rem_euclid(n)) as usize;
                self.banner = None;
            }
            ControlRow::Resolution => {
                if !self.modes.is_empty() {
                    let n = self.modes.len() as i32;
                    self.draft.mode_idx =
                        ((self.draft.mode_idx as i32 + delta).rem_euclid(n)) as usize;
                    self.banner = None;
                }
            }
            ControlRow::Scale => {
                let n = SCALES.len() as i32;
                self.draft.scale_idx =
                    ((self.draft.scale_idx as i32 + delta).rem_euclid(n)) as usize;
                self.banner = None;
            }
            ControlRow::PanelPower => {
                if self.draft.panel_active && !self.draft.mon_active {
                    self.banner = Some((
                        "Cannot disable both displays — enable monitor first".into(),
                        true,
                    ));
                } else {
                    self.draft.panel_active = !self.draft.panel_active;
                    self.banner = None;
                }
            }
            ControlRow::MonitorPower => {
                if self.draft.mon_active && !self.draft.panel_active {
                    self.banner = Some((
                        "Cannot disable both displays — enable internal panel first".into(),
                        true,
                    ));
                } else {
                    self.draft.mon_active = !self.draft.mon_active;
                    self.banner = None;
                }
            }
            _ => {}
        }
        self.update_header();
    }

    fn apply_draft(&mut self) {
        let (mon, panel) = find_monitor_and_panel(&self.outputs);
        let Some(mon) = mon else {
            self.banner = Some(("No external monitor connected".into(), true));
            return;
        };
        let panel_name = panel
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| DEFAULT_PANEL.to_string());

        // 1. Apply power states (enable first so we never blank both outputs).
        if self.draft.panel_active
            && let Err(e) = checked(&["swaymsg", "output", &panel_name, "enable"])
        {
            self.banner = Some((format!("Could not enable {panel_name}: {e}"), true));
            return;
        }
        if self.draft.mon_active
            && let Err(e) = checked(&["swaymsg", "output", &mon.name, "enable"])
        {
            self.banner = Some((format!("Could not enable {}: {e}", mon.name), true));
            return;
        }
        if !self.draft.panel_active
            && let Err(e) = checked(&["swaymsg", "output", &panel_name, "disable"])
        {
            self.banner = Some((format!("Could not disable {panel_name}: {e}"), true));
            return;
        }
        if !self.draft.mon_active
            && let Err(e) = checked(&["swaymsg", "output", &mon.name, "disable"])
        {
            self.banner = Some((format!("Could not disable {}: {e}", mon.name), true));
            return;
        }

        // 2. Apply resolution & scale if monitor is active.
        if self.draft.mon_active {
            if let Some(mode) = self.modes.get(self.draft.mode_idx)
                && let Err(e) = checked(&["swaymsg", "output", &mon.name, "resolution", mode])
            {
                self.banner = Some((format!("Could not set resolution: {e}"), true));
                return;
            }
            let scale = SCALES[self.draft.scale_idx];
            if let Err(e) = checked(&["swaymsg", "output", &mon.name, "scale", scale]) {
                self.banner = Some((format!("Could not set scale: {e}"), true));
                return;
            }

            // Re-read after mode/scale so coordinates use the updated logical size.
            let fresh = read_outputs();
            if let (Some(m), Some(p)) = find_monitor_and_panel(&fresh) {
                if self.draft.panel_active {
                    let arr = POSITIONS[self.draft.pos_idx];
                    if let Err(e) = apply_and_save(arr, &m, &p) {
                        self.banner = Some((format!("Could not apply arrangement: {e}"), true));
                        return;
                    }
                } else if let Err(e) = save_config(&m) {
                    self.banner = Some((format!("Applied, but could not save config: {e}"), true));
                    return;
                }
            }
        }

        self.reload_live();
        self.banner = Some(("Display layout applied & saved".into(), false));
    }

    fn activate_row(&mut self, row: ControlRow) -> Outcome {
        match row {
            ControlRow::Position
            | ControlRow::Resolution
            | ControlRow::Scale
            | ControlRow::PanelPower
            | ControlRow::MonitorPower => {
                self.adjust_row(row, 1);
                Outcome::Consumed
            }
            ControlRow::ApplySave => {
                self.apply_draft();
                Outcome::Consumed
            }
            ControlRow::EnablePanel => {
                let panel_name = internal_panel_name(&self.outputs);
                match checked(&["swaymsg", "output", &panel_name, "enable"]) {
                    Ok(_) => {
                        self.reload_live();
                        self.banner = Some((format!("Enabled {panel_name}"), false));
                    }
                    Err(e) => {
                        self.banner = Some((format!("Could not enable {panel_name}: {e}"), true));
                    }
                }
                Outcome::Consumed
            }
            ControlRow::OpenWdisplays => {
                if !have("wdisplays") {
                    self.banner = Some((
                        "wdisplays is not installed (apt install wdisplays)".into(),
                        true,
                    ));
                    return Outcome::Consumed;
                }
                if out(&["pgrep", "-x", "wdisplays"]).is_some() {
                    let _ = out(&["swaymsg", "[app_id=\"wdisplays\"]", "focus"]);
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
                let _ = out(&["swaymsg", "workspace", "number", "11"]);
                crate::system::spawn_detached("exec wdisplays");
                Outcome::Done(hayato_ui::app::exit::OK)
            }
        }
    }

    fn position_label(idx: usize) -> &'static str {
        match POSITIONS.get(idx).copied().unwrap_or("right") {
            "right" => "Right of panel",
            "left" => "Left of panel",
            "above" => "Above panel",
            "below" => "Below panel",
            "mirror" => "Mirrored",
            _ => "Right of panel",
        }
    }

    fn scale_label(idx: usize) -> &'static str {
        match SCALES.get(idx).copied().unwrap_or("1") {
            "1" => "1.0× · crisp",
            "1.25" => "1.25× · fractional",
            "1.5" => "1.5× · fractional",
            "1.75" => "1.75× · fractional",
            "2" => "2.0× · crisp HiDPI",
            _ => "1.0×",
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_box(
        buf: &mut Buffer,
        r: Rect,
        title: &str,
        line1: &str,
        line2: &str,
        border_style: Style,
        text_style: Style,
        dim_style: Style,
        dashed: bool,
    ) {
        if r.width < 6 || r.height < 3 {
            return;
        }
        let h_char = if dashed { "┄" } else { "─" };
        let v_char = if dashed { "┆" } else { "│" };
        put(buf, r.x, r.y, "╭", 1, border_style);
        put(buf, r.right() - 1, r.y, "╮", 1, border_style);
        put(buf, r.x, r.bottom() - 1, "╰", 1, border_style);
        put(buf, r.right() - 1, r.bottom() - 1, "╯", 1, border_style);
        for x in (r.x + 1)..(r.right() - 1) {
            put(buf, x, r.y, h_char, 1, border_style);
            put(buf, x, r.bottom() - 1, h_char, 1, border_style);
        }
        for y in (r.y + 1)..(r.bottom() - 1) {
            put(buf, r.x, y, v_char, 1, border_style);
            put(buf, r.right() - 1, y, v_char, 1, border_style);
        }
        let inner_w = r.width.saturating_sub(4);
        let ix = r.x + 2;
        if r.height >= 3 {
            put_trunc(buf, ix, r.y + 1, title, inner_w, text_style);
        }
        if r.height >= 4 && !line1.is_empty() {
            put_trunc(buf, ix, r.y + 2, line1, inner_w, dim_style);
        }
        if r.height >= 5 && !line2.is_empty() {
            put_trunc(buf, ix, r.y + 3, line2, inner_w, dim_style);
        }
    }

    fn draw_topology_canvas(&self, buf: &mut Buffer, area: Rect, design: &Design) {
        draw_card(buf, area, design, Some("Spatial Topology"), false);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 12 || inner.height < 6 {
            return;
        }

        // Top status pill row: ● SAVED / ● UNSAVED + arrangement label
        let (pill, pill_style) = if self.is_dirty() {
            (
                "● UNSAVED",
                Style::default()
                    .fg(design.color(ColorRole::Warn))
                    .add_modifier(Modifier::BOLD),
            )
        } else if self.phase == DisplayPhase::PanelOff {
            (
                "● RECOVERY",
                Style::default()
                    .fg(design.color(ColorRole::Danger))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                "● LIVE",
                Style::default()
                    .fg(design.color(ColorRole::Ok))
                    .add_modifier(Modifier::BOLD),
            )
        };
        put_trunc(buf, inner.x, inner.y, pill, inner.width, pill_style);
        let summary = match self.phase {
            DisplayPhase::DualMonitor => {
                if !self.draft.panel_active {
                    "External monitor only".to_string()
                } else if !self.draft.mon_active {
                    "Internal panel only".to_string()
                } else {
                    Self::position_label(self.draft.pos_idx).to_string()
                }
            }
            DisplayPhase::PanelOff => "Internal panel disabled".to_string(),
            DisplayPhase::SinglePanel => "Single display".to_string(),
        };
        let sum_x = inner.x + pill.chars().count() as u16 + 2;
        if sum_x < inner.right() {
            put_trunc(
                buf,
                sum_x,
                inner.y,
                &summary,
                inner.right().saturating_sub(sum_x),
                design.style(TextStyle::Dim),
            );
        }

        // Canvas stage for proportional ASCII monitors
        let stage_y = inner.y + 2;
        let stage_h = inner.height.saturating_sub(5);
        let stage = Rect::new(inner.x, stage_y, inner.width, stage_h);

        let (mon, panel) = find_monitor_and_panel(&self.outputs);
        let panel_name = panel
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or(DEFAULT_PANEL);
        let panel_res = panel
            .as_ref()
            .map(Output::short_res)
            .unwrap_or_else(|| "internal".into());
        let panel_scale = panel
            .as_ref()
            .map(|p| format!("{}× · panel", p.scale))
            .unwrap_or_else(|| "1× · panel".into());

        let active_border = Style::default().fg(design.color(ColorRole::Accent));
        let panel_border = Style::default().fg(design.color(ColorRole::Text));
        let dim_border = design.style(TextStyle::Dim);
        let title_bold = design.style(TextStyle::Body).add_modifier(Modifier::BOLD);
        let dim_style = design.style(TextStyle::Dim);

        if let Some(m) = &mon {
            let mon_mode = self
                .modes
                .get(self.draft.mode_idx)
                .cloned()
                .unwrap_or_else(|| m.short_res());
            let mon_scale = format!("{}× · external", SCALES[self.draft.scale_idx]);
            let pos = POSITIONS[self.draft.pos_idx];

            let p_border = if self.draft.panel_active {
                panel_border
            } else {
                dim_border
            };
            let m_border = if self.draft.mon_active {
                active_border
            } else {
                dim_border
            };
            let p_title = if self.draft.panel_active {
                format!("{}  ON", panel_name)
            } else {
                format!("{}  OFF", panel_name)
            };
            let m_title = if self.draft.mon_active {
                format!("{}  ON", m.name)
            } else {
                format!("{}  OFF", m.name)
            };

            let box_w_side = ((stage.width.saturating_sub(2)) / 2).clamp(16, 26);
            let box_h_side = 6u16.min(stage.height);
            let box_w_stack = stage.width.min(30);
            let box_h_stack = ((stage.height.saturating_sub(1)) / 2).clamp(4, 6);

            match pos {
                "right" | "left" => {
                    let total_w = box_w_side * 2 + 2;
                    let start_x = stage.x + stage.width.saturating_sub(total_w) / 2;
                    let start_y = stage.y + stage.height.saturating_sub(box_h_side) / 2;
                    let left_rect = Rect::new(start_x, start_y, box_w_side, box_h_side);
                    let right_rect =
                        Rect::new(start_x + box_w_side + 2, start_y, box_w_side, box_h_side);
                    let (p_rect, m_rect) = if pos == "right" {
                        (left_rect, right_rect)
                    } else {
                        (right_rect, left_rect)
                    };
                    Self::draw_box(
                        buf,
                        p_rect,
                        &p_title,
                        &panel_res,
                        &panel_scale,
                        p_border,
                        title_bold,
                        dim_style,
                        !self.draft.panel_active,
                    );
                    Self::draw_box(
                        buf,
                        m_rect,
                        &m_title,
                        &mon_mode,
                        &mon_scale,
                        m_border,
                        title_bold,
                        dim_style,
                        !self.draft.mon_active,
                    );
                }
                "above" | "below" => {
                    let total_h = box_h_stack * 2 + 1;
                    let start_x = stage.x + stage.width.saturating_sub(box_w_stack) / 2;
                    let start_y = stage.y + stage.height.saturating_sub(total_h) / 2;
                    let top_rect = Rect::new(start_x, start_y, box_w_stack, box_h_stack);
                    let bot_rect =
                        Rect::new(start_x, start_y + box_h_stack + 1, box_w_stack, box_h_stack);
                    let (p_rect, m_rect) = if pos == "above" {
                        (bot_rect, top_rect)
                    } else {
                        (top_rect, bot_rect)
                    };
                    Self::draw_box(
                        buf,
                        p_rect,
                        &p_title,
                        &panel_res,
                        &panel_scale,
                        p_border,
                        title_bold,
                        dim_style,
                        !self.draft.panel_active,
                    );
                    Self::draw_box(
                        buf,
                        m_rect,
                        &m_title,
                        &mon_mode,
                        &mon_scale,
                        m_border,
                        title_bold,
                        dim_style,
                        !self.draft.mon_active,
                    );
                }
                _ => {
                    // Mirrored: overlapping offset card look
                    let w = stage.width.min(30);
                    let h = 6u16.min(stage.height);
                    let sx = stage.x + stage.width.saturating_sub(w) / 2;
                    let sy = stage.y + stage.height.saturating_sub(h) / 2;
                    let title = format!("{panel_name} ⇄ {} (Mirrored)", m.name);
                    Self::draw_box(
                        buf,
                        Rect::new(sx, sy, w, h),
                        &title,
                        &mon_mode,
                        "Both displays mirror the same output",
                        active_border,
                        title_bold,
                        dim_style,
                        false,
                    );
                }
            }
        } else {
            // Single panel + dashed ghost external monitor slot
            let box_w = ((stage.width.saturating_sub(2)) / 2).clamp(16, 24);
            let box_h = 6u16.min(stage.height);
            let total_w = box_w * 2 + 2;
            let sx = stage.x + stage.width.saturating_sub(total_w) / 2;
            let sy = stage.y + stage.height.saturating_sub(box_h) / 2;
            let p_on = panel.as_ref().map(|p| p.active).unwrap_or(true);
            Self::draw_box(
                buf,
                Rect::new(sx, sy, box_w, box_h),
                &format!("{panel_name}  {}", if p_on { "ON" } else { "OFF" }),
                &panel_res,
                &panel_scale,
                if p_on { active_border } else { dim_border },
                title_bold,
                dim_style,
                !p_on,
            );
            Self::draw_box(
                buf,
                Rect::new(sx + box_w + 2, sy, box_w, box_h),
                "USB-C / DP",
                "No monitor",
                "Plug in + Ctrl+R",
                dim_border,
                dim_style,
                dim_style,
                true,
            );
        }

        // Bottom config path + status banner
        let foot_y = inner.bottom().saturating_sub(2);
        if let Some((msg, is_err)) = &self.banner {
            let st = Style::default().fg(design.color(if *is_err {
                ColorRole::Danger
            } else {
                ColorRole::Ok
            }));
            put_trunc(buf, inner.x, foot_y, msg, inner.width, st);
        } else {
            put_trunc(
                buf,
                inner.x,
                foot_y,
                "Config: ~/.config/sway/config.d/40-display.conf",
                inner.width,
                dim_style,
            );
        }
        put_trunc(
            buf,
            inner.x,
            inner.bottom().saturating_sub(1),
            "←/→ adjust · Ctrl+S apply & save · Ctrl+R refresh",
            inner.width,
            dim_style,
        );
    }

    fn draw_inspector(&self, buf: &mut Buffer, area: Rect, design: &Design, compact: bool) {
        let title = if compact {
            "Display Studio"
        } else {
            "Configuration"
        };
        draw_card(buf, area, design, Some(title), true);
        let inner = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        if inner.width < 10 || inner.height < 4 {
            return;
        }

        let mut y = inner.y;
        let dim = design.style(TextStyle::Dim);
        let accent = Style::default()
            .fg(design.color(ColorRole::Accent))
            .add_modifier(Modifier::BOLD);

        // On compact terminals (< 100 cols), draw a 2-line live topology strip at top.
        if compact {
            let (mon, panel) = find_monitor_and_panel(&self.outputs);
            let p_name = panel
                .as_ref()
                .map(|p| p.name.as_str())
                .unwrap_or(DEFAULT_PANEL);
            let strip = if let Some(m) = &mon {
                let diagram = match POSITIONS[self.draft.pos_idx] {
                    "right" => format!("[{p_name}] ▸ [{}]", m.name),
                    "left" => format!("[{}] ◂ [{p_name}]", m.name),
                    "above" => format!("[{}] ▴ [{p_name}]", m.name),
                    "below" => format!("[{p_name}] ▾ [{}]", m.name),
                    _ => format!("[{p_name} ⇄ {}]", m.name),
                };
                let badge = if self.is_dirty() {
                    "● UNSAVED"
                } else {
                    "● LIVE"
                };
                format!("{badge} · {diagram}")
            } else {
                format!("[{p_name}] · no external monitor")
            };
            let st = if self.is_dirty() {
                Style::default()
                    .fg(design.color(ColorRole::Warn))
                    .add_modifier(Modifier::BOLD)
            } else {
                accent
            };
            put_trunc(buf, inner.x, y, &strip, inner.width, st);
            y += 1;
            if let Some((msg, is_err)) = &self.banner {
                let bst = Style::default().fg(design.color(if *is_err {
                    ColorRole::Danger
                } else {
                    ColorRole::Ok
                }));
                put_trunc(buf, inner.x, y, msg, inner.width, bst);
            } else {
                put_trunc(
                    buf,
                    inner.x,
                    y,
                    "←/→ adjusts inline · Ctrl+S applies & saves",
                    inner.width,
                    dim,
                );
            }
            y += 2;
        }

        let mut hits = self.row_hits.borrow_mut();
        hits.clear();
        self.rows_area.set(Rect::new(
            inner.x,
            y,
            inner.width,
            inner.bottom().saturating_sub(y),
        ));

        let (mon, panel) = find_monitor_and_panel(&self.outputs);
        let p_name = panel
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or(DEFAULT_PANEL);
        let m_name = mon.as_ref().map(|m| m.name.as_str()).unwrap_or("DP-1");

        for (idx, ctrl) in self.controls.iter().enumerate() {
            if y >= inner.bottom() {
                break;
            }
            let selected = idx == self.cursor;
            let (glyph, label, value, is_cta) = match ctrl {
                ControlRow::Position => (
                    Glyph::Display,
                    "Position".to_string(),
                    format!("◀ {} ▶", Self::position_label(self.draft.pos_idx)),
                    false,
                ),
                ControlRow::Resolution => {
                    let cur = self
                        .modes
                        .get(self.draft.mode_idx)
                        .map(String::as_str)
                        .unwrap_or("auto");
                    (
                        Glyph::Display,
                        "Resolution".to_string(),
                        format!("◀ {cur} ▶"),
                        false,
                    )
                }
                ControlRow::Scale => (
                    Glyph::Sliders,
                    "Scale".to_string(),
                    format!("◀ {} ▶", Self::scale_label(self.draft.scale_idx)),
                    false,
                ),
                ControlRow::PanelPower => (
                    Glyph::Desktop,
                    format!("Internal ({p_name})"),
                    if self.draft.panel_active {
                        "[ ON ]".to_string()
                    } else {
                        "[ OFF ]".to_string()
                    },
                    false,
                ),
                ControlRow::MonitorPower => (
                    Glyph::Display,
                    format!("Monitor ({m_name})"),
                    if self.draft.mon_active {
                        "[ ON ]".to_string()
                    } else {
                        "[ OFF ]".to_string()
                    },
                    false,
                ),
                ControlRow::ApplySave => (
                    Glyph::Check,
                    "Apply & Save".to_string(),
                    if self.is_dirty() {
                        "Enter · save".to_string()
                    } else {
                        "up to date".to_string()
                    },
                    true,
                ),
                ControlRow::EnablePanel => (
                    Glyph::Desktop,
                    format!("Bring {p_name} back on"),
                    "Enter · enable".to_string(),
                    true,
                ),
                ControlRow::OpenWdisplays => (
                    Glyph::Mouse,
                    "Open wdisplays".to_string(),
                    "freehand drag GUI".to_string(),
                    false,
                ),
            };

            let step = if inner.height >= 16 { 2 } else { 1 };
            let hit_h = design.row_stride().min(step);
            let row_rect = Rect::new(inner.x, y, inner.width, hit_h);
            hits.push(row_rect);

            let marker = if selected { "▍" } else { " " };
            let marker_style = if selected {
                accent
            } else {
                design.style(TextStyle::Dim)
            };
            put(buf, inner.x, y, marker, 1, marker_style);

            let label_style = if selected {
                Style::default()
                    .fg(design.color(if is_cta && self.is_dirty() {
                        ColorRole::Ok
                    } else {
                        ColorRole::Accent
                    }))
                    .add_modifier(Modifier::BOLD)
            } else if is_cta && self.is_dirty() {
                Style::default()
                    .fg(design.color(ColorRole::Ok))
                    .add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Body)
            };
            let left = format!("{}  {}", glyph.cell(), label);
            let val_w = value.chars().count() as u16;
            let left_max = inner.width.saturating_sub(val_w + 4);
            put_trunc(buf, inner.x + 2, y, &left, left_max, label_style);

            let val_style = if selected {
                Style::default()
                    .fg(design.color(ColorRole::Text))
                    .add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            let vx = inner.right().saturating_sub(val_w);
            if vx > inner.x + 4 {
                put_trunc(buf, vx, y, &value, val_w, val_style);
            }

            y += step;
        }
    }
}

impl Screen for DisplayScreen {
    fn draw(&mut self, ctx: &mut BodyCtx) {
        let (left, right) = Self::split_panes(ctx.area);
        if let Some(right_area) = right {
            self.draw_topology_canvas(ctx.buf, left, ctx.design);
            self.draw_inspector(ctx.buf, right_area, ctx.design, false);
        } else {
            self.draw_inspector(ctx.buf, left, ctx.design, true);
        }
    }

    fn key(&mut self, key: KeyEvent, _design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if self.phase == DisplayPhase::DualMonitor {
                        self.apply_draft();
                        return Outcome::Consumed;
                    }
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.reload_live();
                    self.banner = Some(("Refreshed outputs from Sway".into(), false));
                    return Outcome::Consumed;
                }
                KeyCode::Char('z') | KeyCode::Char('Z') => {
                    self.draft = self.live.clone();
                    self.banner = Some(("Discarded unsaved changes".into(), false));
                    self.update_header();
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => Outcome::Done(hayato_ui::app::exit::ESCAPE),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < self.controls.len() {
                    self.cursor += 1;
                }
                Outcome::Consumed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(&ctrl) = self.controls.get(self.cursor) {
                    self.adjust_row(ctrl, -1);
                }
                Outcome::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(&ctrl) = self.controls.get(self.cursor) {
                    self.adjust_row(ctrl, 1);
                }
                Outcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(&ctrl) = self.controls.get(self.cursor) {
                    self.activate_row(ctrl)
                } else {
                    Outcome::Continue
                }
            }
            _ => Outcome::Continue,
        }
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, _design: &Design) -> Option<usize> {
        let hits = self.row_hits.borrow();
        for (idx, r) in hits.iter().enumerate() {
            if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                return Some(idx);
            }
        }
        None
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        if let Some(idx) = row
            && idx < self.controls.len()
        {
            self.cursor = idx;
            let ctrl = self.controls[idx];
            return self.activate_row(ctrl);
        }
        Outcome::Continue
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.phase {
            DisplayPhase::DualMonitor => "display.dual",
            DisplayPhase::SinglePanel => "display.single",
            DisplayPhase::PanelOff => "display.recovery",
        })
    }
}

/// `hayato menu display` (`hayato-display`): Spatial Display Arrangement Studio.
pub fn menu(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    if let Some(said) = missing(NEEDED) {
        return Err(crate::Failure::new("Display", format!("{said} (sway, jq)")));
    }
    let mut screen = DisplayScreen::new();
    crate::run_screen(terminal, design, "Display", &mut screen)
}

pub fn missing(names: &[&str]) -> Option<String> {
    let gone: Vec<&str> = names.iter().copied().filter(|n| !have(n)).collect();
    if gone.is_empty() {
        None
    } else {
        Some(format!("Missing: {}", gone.join(" ")))
    }
}

fn have(program: &str) -> bool {
    crate::files::open::on_path(&std::env::var("PATH").unwrap_or_default(), program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_formats_correctly() {
        assert_eq!(hz(60000), "60");
        assert_eq!(hz(59940), "59.94");
        assert_eq!(hz(120000), "120");
    }

    #[test]
    fn arrangement_detects_positions() {
        let panel = Output {
            x: 0,
            y: 0,
            width: 1366,
            height: 912,
            active: true,
            ..Default::default()
        };
        let mon = Output {
            x: 1366,
            y: 0,
            width: 1920,
            height: 1080,
            active: true,
            ..Default::default()
        };
        assert_eq!(arrangement(&mon, &panel), "right");

        let mon = Output {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
            active: true,
            ..Default::default()
        };
        assert_eq!(arrangement(&mon, &panel), "left");

        let mon = Output {
            x: 0,
            y: -1080,
            width: 1920,
            height: 1080,
            active: true,
            ..Default::default()
        };
        assert_eq!(arrangement(&mon, &panel), "above");

        let mon = Output {
            x: 0,
            y: 912,
            width: 1920,
            height: 1080,
            active: true,
            ..Default::default()
        };
        assert_eq!(arrangement(&mon, &panel), "below");

        let mon = Output {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            active: true,
            ..Default::default()
        };
        assert_eq!(arrangement(&mon, &panel), "mirror");
    }

    #[test]
    fn panel_off_with_no_monitor_offers_recovery_row() {
        let json = r#"[{"name":"eDP-1","active":false,"scale":1.0,"rect":{"x":0,"y":0,"width":0,"height":0},"current_mode":null,"modes":[]}]"#;
        let outputs = parse_outputs(json);
        assert_eq!(outputs.len(), 1);
        assert!(!outputs[0].active);
        assert_eq!(status_for(&outputs), "internal panel off");
        let rows = rows_for(&outputs);
        assert!(
            rows.iter()
                .any(|(k, l, _)| k == "enable:panel" && l == "Bring the internal panel back")
        );

        let dsi_json = r#"[{"name":"DSI-1","active":false,"scale":1.0,"rect":{"x":0,"y":0,"width":0,"height":0},"current_mode":null,"modes":[]}]"#;
        let dsi_outputs = parse_outputs(dsi_json);
        assert_eq!(status_for(&dsi_outputs), "internal panel off");
        let dsi_rows = rows_for(&dsi_outputs);
        assert!(
            dsi_rows
                .iter()
                .any(|(k, _, d)| k == "enable:panel" && d == "DSI-1 enable")
        );
    }

    #[test]
    fn parse_swaymsg_outputs_and_modes() {
        let json = r#"[
          {"name":"eDP-1","active":true,"scale":1.0,"rect":{"x":0,"y":0,"width":1366,"height":768},"current_mode":{"width":1366,"height":768,"refresh":60000},"modes":[]},
          {"name":"DP-1","active":true,"scale":1.25,"rect":{"x":1366,"y":0,"width":1920,"height":1080},"current_mode":{"width":1920,"height":1080,"refresh":59940},"modes":[{"width":1920,"height":1080,"refresh":60000}]}
        ]"#;
        let outputs = parse_outputs(json);
        assert_eq!(outputs.len(), 2);
        assert_eq!(status_for(&outputs), "right of the panel");
        let modes = parse_modes(json, "DP-1");
        assert_eq!(modes, vec!["1920x1080@60Hz"]);
    }

    #[test]
    fn display_studio_inline_draft_and_safety_guard() {
        let design = Design::solarized();
        let outputs = vec![
            Output {
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
            Output {
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
        ];
        let mut screen = DisplayScreen::from_outputs(
            outputs,
            vec!["1920x1080@60Hz".into(), "2560x1440@60Hz".into()],
        );
        assert_eq!(screen.phase(), Some("display.dual"));
        assert!(!screen.is_dirty());

        // Right arrow cycles Position from Right -> Left and marks draft dirty.
        let _ = screen.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &design);
        assert!(screen.is_dirty());
        assert_eq!(POSITIONS[screen.draft.pos_idx], "left");

        // Ctrl+Z discards unsaved changes.
        let _ = screen.key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
            &design,
        );
        assert!(!screen.is_dirty());

        // Safety guard: cannot disable both panel and monitor.
        screen.adjust_row(ControlRow::PanelPower, 1);
        assert!(!screen.draft.panel_active);
        screen.adjust_row(ControlRow::MonitorPower, 1);
        assert!(screen.draft.mon_active);
    }
}
