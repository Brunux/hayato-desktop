//! Resumable persisted state for the 8-step Personal Setup Assistant.
//!
//! Stored atomically at `$XDG_STATE_HOME/hayato/setup.json`. Tracks per-step
//! completion (`pending`, `done`, `skipped`), `machine_id`, and `install_epoch`
//! so a cloned disk or fresh install starts clean while a paused session
//! resumes right where the user left off.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hayato_ui::Glyph;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepId {
    Welcome,
    Security,
    Connectivity,
    Personalization,
    Hardware,
    Software,
    Privacy,
    Protection,
}

impl StepId {
    pub const ALL: [Self; 8] = [
        Self::Welcome,
        Self::Security,
        Self::Connectivity,
        Self::Personalization,
        Self::Hardware,
        Self::Software,
        Self::Privacy,
        Self::Protection,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Welcome => "welcome",
            Self::Security => "security",
            Self::Connectivity => "connectivity",
            Self::Personalization => "personalization",
            Self::Hardware => "hardware",
            Self::Software => "software",
            Self::Privacy => "privacy",
            Self::Protection => "protection",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "welcome" | "1" => Some(Self::Welcome),
            "security" | "2" => Some(Self::Security),
            "connectivity" | "network" | "3" => Some(Self::Connectivity),
            "personalization" | "look" | "4" => Some(Self::Personalization),
            "hardware" | "5" => Some(Self::Hardware),
            "software" | "apps" | "6" => Some(Self::Software),
            "privacy" | "7" => Some(Self::Privacy),
            "protection" | "backup" | "8" => Some(Self::Protection),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Security => "Security",
            Self::Connectivity => "Connectivity",
            Self::Personalization => "Personalization",
            Self::Hardware => "Hardware",
            Self::Software => "Software",
            Self::Privacy => "Privacy",
            Self::Protection => "Protection",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Welcome => "Identify this machine & see what needs attention",
            Self::Security => "Password, encryption status, firewall & remote access",
            Self::Connectivity => "Wi-Fi, Bluetooth, DNS & optional VPN",
            Self::Personalization => "Language, timezone, theme, keyboard & touchpad",
            Self::Hardware => "Test speakers, mic, camera, touch & external display",
            Self::Software => "Essential apps, capability bundles & local AI models",
            Self::Privacy => "Local AI posture, clipboard, mic mute & LAN sharing",
            Self::Protection => "Encrypted backups, system image & recovery guidance",
        }
    }

    pub fn glyph(self) -> Glyph {
        match self {
            Self::Welcome => Glyph::Sparkle,
            Self::Security => Glyph::Shield,
            Self::Connectivity => Glyph::Wifi,
            Self::Personalization => Glyph::Theme,
            Self::Hardware => Glyph::Display,
            Self::Software => Glyph::Box,
            Self::Privacy => Glyph::Eye,
            Self::Protection => Glyph::Disk,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Pending,
    Done,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepSource {
    Auto,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub status: StepStatus,
    pub source: StepSource,
    pub updated_at: u64,
}

impl Default for StepRecord {
    fn default() -> Self {
        Self {
            status: StepStatus::Pending,
            source: StepSource::Auto,
            updated_at: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupState {
    pub schema: u32,
    pub machine_id: String,
    pub install_epoch: u64,
    pub dismissed_first_run: bool,
    pub completed_first_run: bool,
    pub password_verified: bool,
    pub last_step: StepId,
    pub started_at: u64,
    pub updated_at: u64,
    pub steps: BTreeMap<StepId, StepRecord>,
}

impl Default for SetupState {
    fn default() -> Self {
        let mut steps = BTreeMap::new();
        for step in StepId::ALL {
            steps.insert(step, StepRecord::default());
        }
        Self {
            schema: 1,
            machine_id: String::new(),
            install_epoch: 0,
            dismissed_first_run: false,
            completed_first_run: false,
            password_verified: false,
            last_step: StepId::Welcome,
            started_at: now_epoch(),
            updated_at: 0,
            steps,
        }
    }
}

impl SetupState {
    pub fn for_machine(machine_id: String, install_epoch: u64) -> Self {
        Self {
            machine_id,
            install_epoch,
            ..Self::default()
        }
    }

    pub fn load() -> Self {
        let machine_id = read_machine_id();
        let install_epoch = read_install_epoch();
        let Some(path) = state_path() else {
            return Self::for_machine(machine_id, install_epoch);
        };
        Self::load_from(&path, &machine_id, install_epoch)
    }

    pub fn load_from(path: &Path, machine_id: &str, install_epoch: u64) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::for_machine(machine_id.to_string(), install_epoch);
        };
        let Ok(mut state) = serde_json::from_str::<SetupState>(&raw) else {
            return Self::for_machine(machine_id.to_string(), install_epoch);
        };
        // Reset cleanly if the disk was cloned or freshly re-installed.
        if (!machine_id.is_empty()
            && !state.machine_id.is_empty()
            && state.machine_id != machine_id)
            || (install_epoch != 0
                && state.install_epoch != 0
                && state.install_epoch != install_epoch)
        {
            return Self::for_machine(machine_id.to_string(), install_epoch);
        }
        if state.machine_id.is_empty() {
            state.machine_id = machine_id.to_string();
        }
        if state.install_epoch == 0 {
            state.install_epoch = install_epoch;
        }
        for step in StepId::ALL {
            state.steps.entry(step).or_default();
        }
        state
    }

    pub fn save(&mut self) -> Result<(), String> {
        let path = state_path().ok_or_else(|| "cannot resolve XDG state directory".to_string())?;
        self.save_to(&path)
    }

    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        self.updated_at = now_epoch();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.new");
        fs::write(&tmp, body).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("cannot replace {}: {e}", path.display())
        })
    }

    pub fn status(&self, step: StepId) -> StepStatus {
        self.steps
            .get(&step)
            .map(|r| r.status)
            .unwrap_or(StepStatus::Pending)
    }

    pub fn mark(&mut self, step: StepId, status: StepStatus, source: StepSource) {
        self.steps.insert(
            step,
            StepRecord {
                status,
                source,
                updated_at: now_epoch(),
            },
        );
        self.last_step = step;
        self.completed_first_run = StepId::ALL
            .iter()
            .all(|s| self.status(*s) != StepStatus::Pending);
    }

    pub fn progress_counts(&self) -> (usize, usize, usize) {
        let mut done = 0;
        let mut skipped = 0;
        for step in StepId::ALL {
            match self.status(step) {
                StepStatus::Done => done += 1,
                StepStatus::Skipped => skipped += 1,
                StepStatus::Pending => {}
            }
        }
        (done, skipped, StepId::ALL.len())
    }

    /// Which step to open when resuming: first pending step from `last_step`
    /// onward, or `Welcome` when everything is already complete.
    pub fn resume_step(&self) -> StepId {
        if self.status(self.last_step) == StepStatus::Pending {
            return self.last_step;
        }
        StepId::ALL
            .into_iter()
            .find(|&s| self.status(s) == StepStatus::Pending)
            .unwrap_or(StepId::Welcome)
    }

    pub fn summary_badge(&self) -> String {
        let (done, skipped, total) = self.progress_counts();
        if done + skipped == total {
            format!("{done}/{total} configured")
        } else {
            format!("{done}/{total} ready")
        }
    }
}

pub fn state_path() -> Option<PathBuf> {
    Paths::from_process()
        .ok()
        .map(|p| p.state.join("setup.json"))
}

pub fn read_machine_id() -> String {
    fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn read_install_epoch() -> u64 {
    let Ok(raw) = fs::read_to_string("/etc/hayato/installation") else {
        return 0;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return 0;
    };
    val.get("installed_at_epoch")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_resumes_from_first_pending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.json");

        let mut st = SetupState::for_machine("mid-1".into(), 100);
        st.mark(StepId::Welcome, StepStatus::Done, StepSource::User);
        st.mark(StepId::Security, StepStatus::Skipped, StepSource::User);
        st.save_to(&path).unwrap();

        let loaded = SetupState::load_from(&path, "mid-1", 100);
        assert_eq!(loaded.status(StepId::Welcome), StepStatus::Done);
        assert_eq!(loaded.status(StepId::Security), StepStatus::Skipped);
        assert_eq!(loaded.status(StepId::Connectivity), StepStatus::Pending);
        assert_eq!(loaded.resume_step(), StepId::Connectivity);
        assert_eq!(loaded.progress_counts(), (1, 1, 8));
    }

    #[test]
    fn machine_id_change_resets_stale_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.json");

        let mut st = SetupState::for_machine("old-machine".into(), 100);
        st.dismissed_first_run = true;
        st.mark(StepId::Welcome, StepStatus::Done, StepSource::User);
        st.save_to(&path).unwrap();

        let fresh = SetupState::load_from(&path, "new-machine", 200);
        assert!(!fresh.dismissed_first_run);
        assert_eq!(fresh.status(StepId::Welcome), StepStatus::Pending);
        assert_eq!(fresh.machine_id, "new-machine");
    }

    #[test]
    fn step_slugs_round_trip() {
        for step in StepId::ALL {
            assert_eq!(StepId::from_slug(step.slug()), Some(step));
        }
    }
}
