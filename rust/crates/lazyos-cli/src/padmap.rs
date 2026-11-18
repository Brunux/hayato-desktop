//! The pad's button table: which physical button does what, and where it is
//! saved.
//!
//! `lazyos gamepad` sends keys for the framework's windows, and the Controller
//! screen (`lazyos menu controller`) edits the same table. Keeping it in one
//! module is what makes the screen's preview true: the row says Enter because
//! the mapper will send Enter, not because a second table agrees.
//!
//! The defaults are the console layout: the south face button confirms (Cross
//! on a PlayStation pad, A on an Xbox pad), east cancels (Circle/B), west opens
//! the Games options (Square/X), north the manual (Triangle/Y), L1 the game's
//! folder, and the D-pad and the left stick are the arrows. A button may also
//! be [`Action::Nothing`], which is how a button is freed.
//!
//! The file is `~/.config/lazyos/pad.json` (or under `XDG_CONFIG_HOME`). It is
//! optional: a missing or broken file keeps the defaults, and the mapper
//! reloads it within two seconds, so a change needs no restart. Only buttons
//! that are mapped are read; an unknown button in the file is ignored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a button does. The navigation actions are the keys every framework
/// window reads; the last three are Alt combinations, because a bare letter
/// would type into another screen's filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Nothing,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Options,
    Manual,
    Folder,
}

impl Action {
    /// The cycle order in the Controller screen, with `Nothing` first so a
    /// button can be freed before the next action.
    pub const ALL: [Self; 10] = [
        Self::Nothing,
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Enter,
        Self::Escape,
        Self::Options,
        Self::Manual,
        Self::Folder,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Nothing => "—",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Options => "Options",
            Self::Manual => "Manual",
            Self::Folder => "Folder",
        }
    }

    /// The Alt letter, for the three actions that are not a plain key.
    pub fn alt(self) -> Option<&'static str> {
        match self {
            Self::Options => Some("o"),
            Self::Manual => Some("m"),
            Self::Folder => Some("r"),
            _ => None,
        }
    }
}

/// A button this mapper knows. The names are the file's keys and are stable;
/// the labels are what the Controller screen shows, naming the PlayStation and
/// Xbox markings together because evdev's `BTN_SOUTH` is Cross and A alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Button {
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    Start,
    Select,
    L3,
    R3,
}

impl Button {
    pub const ALL: [Self; 16] = [
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
        Self::Start,
        Self::Select,
        Self::L3,
        Self::R3,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::DpadUp => "dpad-up",
            Self::DpadDown => "dpad-down",
            Self::DpadLeft => "dpad-left",
            Self::DpadRight => "dpad-right",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::North => "north",
            Self::L1 => "l1",
            Self::R1 => "r1",
            Self::L2 => "l2",
            Self::R2 => "r2",
            Self::Start => "start",
            Self::Select => "select",
            Self::L3 => "l3",
            Self::R3 => "r3",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DpadUp => "D-pad up",
            Self::DpadDown => "D-pad down",
            Self::DpadLeft => "D-pad left",
            Self::DpadRight => "D-pad right",
            Self::South => "Cross / A",
            Self::East => "Circle / B",
            Self::West => "Square / X",
            Self::North => "Triangle / Y",
            Self::L1 => "L1 / LB",
            Self::R1 => "R1 / RB",
            Self::L2 => "L2 / LT",
            Self::R2 => "R2 / RT",
            Self::Start => "Start",
            Self::Select => "Select",
            Self::L3 => "L3",
            Self::R3 => "R3",
        }
    }

    pub fn code(self) -> evdev::KeyCode {
        match self {
            Self::DpadUp => evdev::KeyCode::BTN_DPAD_UP,
            Self::DpadDown => evdev::KeyCode::BTN_DPAD_DOWN,
            Self::DpadLeft => evdev::KeyCode::BTN_DPAD_LEFT,
            Self::DpadRight => evdev::KeyCode::BTN_DPAD_RIGHT,
            Self::South => evdev::KeyCode::BTN_SOUTH,
            Self::East => evdev::KeyCode::BTN_EAST,
            Self::West => evdev::KeyCode::BTN_WEST,
            Self::North => evdev::KeyCode::BTN_NORTH,
            Self::L1 => evdev::KeyCode::BTN_TL,
            Self::R1 => evdev::KeyCode::BTN_TR,
            Self::L2 => evdev::KeyCode::BTN_TL2,
            Self::R2 => evdev::KeyCode::BTN_TR2,
            Self::Start => evdev::KeyCode::BTN_START,
            Self::Select => evdev::KeyCode::BTN_SELECT,
            Self::L3 => evdev::KeyCode::BTN_THUMBL,
            Self::R3 => evdev::KeyCode::BTN_THUMBR,
        }
    }

    pub fn from_code(code: evdev::KeyCode) -> Option<Self> {
        Self::ALL.into_iter().find(|button| button.code() == code)
    }
}

/// One pad the kernel knows about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadInfo {
    pub name: String,
    pub bus: evdev::BusType,
    pub vendor: u16,
    pub product: u16,
}

impl PadInfo {
    /// A PlayStation pad on USB is the one case where the mapper can say what
    /// the next step is: BlueZ's sixaxis plugin is writing this machine's
    /// address into it, and the pad only needs to be unplugged and switched on.
    pub fn is_playstation_on_usb(&self) -> bool {
        self.vendor == 0x054c && self.bus == evdev::BusType::BUS_USB
    }

    pub fn summary(&self) -> String {
        format!("{} · {}", self.name, self.bus)
    }
}

/// The pads the kernel currently exposes. A gamepad is a device with a south
/// face button; a keyboard has none, so the input devices of the machine are
/// skipped without a name list to maintain.
pub fn pads() -> Vec<PadInfo> {
    let mut found = Vec::new();
    for (_, device) in evdev::enumerate() {
        let is_pad = device
            .supported_keys()
            .map(|keys| keys.contains(evdev::KeyCode::BTN_SOUTH))
            .unwrap_or(false);
        if !is_pad {
            continue;
        }
        let id = device.input_id();
        found.push(PadInfo {
            name: device.name().unwrap_or("Controller").to_string(),
            bus: id.bus_type(),
            vendor: id.vendor(),
            product: id.product(),
        });
    }
    found
}

/// The one line Settings shows on its Controller row.
pub fn status() -> String {
    let pads = pads();
    match pads.as_slice() {
        [] => "No controller connected".to_string(),
        [pad] => pad.summary(),
        many => format!("{} controllers connected", many.len()),
    }
}

/// The button → action table. Every known button has an action; an unbound one
/// is [`Action::Nothing`].
#[derive(Clone, Debug)]
pub struct Bindings {
    map: BTreeMap<Button, Action>,
}

impl Default for Bindings {
    fn default() -> Self {
        let mut map: BTreeMap<Button, Action> = Button::ALL
            .into_iter()
            .map(|b| (b, Action::Nothing))
            .collect();
        let defaults = [
            (Button::DpadUp, Action::Up),
            (Button::DpadDown, Action::Down),
            (Button::DpadLeft, Action::Left),
            (Button::DpadRight, Action::Right),
            (Button::South, Action::Enter),
            (Button::East, Action::Escape),
            (Button::West, Action::Options),
            (Button::North, Action::Manual),
            (Button::L1, Action::Folder),
            (Button::Start, Action::Enter),
        ];
        for (button, action) in defaults {
            map.insert(button, action);
        }
        Self { map }
    }
}

impl Bindings {
    pub fn get(&self, button: Button) -> Action {
        self.map.get(&button).copied().unwrap_or(Action::Nothing)
    }

    pub fn set(&mut self, button: Button, action: Action) {
        self.map.insert(button, action);
    }

    /// Step the button's action through [`Action::ALL`], wrapping.
    pub fn cycle(&mut self, button: Button, step: isize) {
        let all = Action::ALL;
        let current = all
            .iter()
            .position(|action| *action == self.get(button))
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(all.len() as isize) as usize;
        self.set(button, all[next]);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The action for an evdev key, if that button is bound.
    pub fn action_for(&self, code: evdev::KeyCode) -> Option<Action> {
        let button = Button::from_code(code)?;
        match self.get(button) {
            Action::Nothing => None,
            action => Some(action),
        }
    }

    /// `~/.config/lazyos/pad.json`, or under `XDG_CONFIG_HOME`.
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })?;
        Some(base.join("lazyos").join("pad.json"))
    }

    pub fn load() -> Self {
        Self::load_from(Self::path().as_deref())
    }

    /// A missing file, or one that does not parse, keeps the defaults: a pad
    /// that stops working because a half-written file has a typo is worse than
    /// a pad that ignores the file.
    pub fn load_from(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(stored) = serde_json::from_str::<Stored>(&text) else {
            return Self::default();
        };
        let mut bindings = Self::default();
        for (button, action) in stored.bindings {
            bindings.set(button, action);
        }
        bindings
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(Self::path().as_deref())
    }

    pub fn save_to(&self, path: Option<&Path>) -> Result<(), String> {
        let Some(path) = path else {
            return Err("no config directory (XDG_CONFIG_HOME or HOME is unset)".to_string());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let stored = Stored {
            bindings: self.map.clone(),
        };
        let text = serde_json::to_string_pretty(&stored)
            .map_err(|error| format!("cannot encode the bindings: {error}"))?;
        std::fs::write(path, format!("{text}\n"))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }
}

/// The file's shape. A top-level object leaves room for a version later without
/// changing the meaning of `bindings`.
#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    bindings: BTreeMap<Button, Action>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lazyos-pad-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn the_defaults_are_the_console_layout() {
        let bindings = Bindings::default();
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_SOUTH),
            Some(Action::Enter)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_EAST),
            Some(Action::Escape)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_WEST),
            Some(Action::Options)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_NORTH),
            Some(Action::Manual)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_TL),
            Some(Action::Folder)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_DPAD_UP),
            Some(Action::Up)
        );
        assert_eq!(
            bindings.action_for(evdev::KeyCode::BTN_START),
            Some(Action::Enter)
        );
        // R1 and the rest are free until someone binds them.
        assert_eq!(bindings.action_for(evdev::KeyCode::BTN_TR), None);
        assert_eq!(bindings.action_for(evdev::KeyCode::BTN_THUMBR), None);
        // A keyboard key is not a pad button, bound or not.
        assert_eq!(bindings.action_for(evdev::KeyCode::KEY_A), None);
    }

    #[test]
    fn a_saved_table_round_trips() {
        let path = temp_path("round-trip");
        let mut bindings = Bindings::default();
        bindings.set(Button::R1, Action::Options);
        bindings.set(Button::South, Action::Nothing);
        bindings.save_to(Some(&path)).expect("save");
        let loaded = Bindings::load_from(Some(&path));
        assert_eq!(loaded.get(Button::R1), Action::Options);
        assert_eq!(loaded.get(Button::South), Action::Nothing);
        assert_eq!(loaded.get(Button::North), Action::Manual);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_or_broken_file_keeps_the_defaults() {
        let missing = temp_path("missing");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(
            Bindings::load_from(Some(&missing)).get(Button::South),
            Action::Enter
        );

        let broken = temp_path("broken");
        std::fs::write(&broken, "{ not json").expect("write");
        assert_eq!(
            Bindings::load_from(Some(&broken)).get(Button::South),
            Action::Enter
        );
        let _ = std::fs::remove_file(broken);
    }

    #[test]
    fn cycling_wraps_and_reset_restores_the_defaults() {
        let mut bindings = Bindings::default();
        bindings.set(Button::R1, Action::Nothing);
        bindings.cycle(Button::R1, -1);
        assert_eq!(bindings.get(Button::R1), Action::Folder);
        bindings.cycle(Button::R1, 1);
        assert_eq!(bindings.get(Button::R1), Action::Nothing);

        bindings.set(Button::South, Action::Options);
        bindings.reset();
        assert_eq!(bindings.get(Button::South), Action::Enter);
        assert_eq!(bindings.get(Button::R1), Action::Nothing);
    }

    #[test]
    fn the_alt_actions_are_the_three_letters() {
        assert_eq!(Action::Options.alt(), Some("o"));
        assert_eq!(Action::Manual.alt(), Some("m"));
        assert_eq!(Action::Folder.alt(), Some("r"));
        assert_eq!(Action::Enter.alt(), None);
    }
}
