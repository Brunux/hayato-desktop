//! Personal Setup Assistant (`hayato menu setup`, aliased by `hayato menu welcome`).
//!
//! An intelligent, resumable, 8-step first-run experience that discovers what
//! the machine and user need, delegates inline to Hayato's native TUI tools
//! inside a single `foot` window, and persists per-step progress atomically in
//! `$XDG_STATE_HOME/hayato/setup.json`.

pub mod backup;
pub mod bundles;
pub mod hardware;
pub mod probes;
pub mod state;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use hayato_ui::app::{BodyCtx, Outcome, Screen};
use hayato_ui::design::{ColorRole, Design, TextStyle};
use hayato_ui::widgets::{KeyHint, ListState, Row, draw_card_with, put, put_trunc};
use hayato_ui::{Glyph, Header};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::Failure;
use crate::system::setup::bundles::BUNDLES;
use crate::system::setup::probes::{SetupSnapshot, StepHealth, step_health};
use crate::system::setup::state::{SetupState, StepId, StepSource, StepStatus};

/// An inline action that needs the outer `Terminal` (to delegate to another
/// Hayato `Screen` in the same window or run an interactive command).
#[derive(Clone, Debug)]
pub enum DelegateAction {
    OpenLearn,
    OpenThemePicker,
    OpenNet,
    OpenBluetooth,
    OpenDns,
    OpenSsh,
    OpenVnc,
    OpenIdle,
    OpenTz,
    OpenLayout,
    OpenLocale,
    OpenTouchpad,
    OpenDisplay,
    OpenAudio,
    OpenPkg,
    OpenDefaultApps,
    OpenNotifySettings,
    OpenSwap,
    OpenClean,
    ChangePassword,
    OpenVpn,
    InstallBundles(Vec<&'static str>),
    SystemBackup(PathBuf),
}

#[derive(Clone, Debug)]
enum RowAction {
    Delegate(DelegateAction),
    NextStep,
    ToggleUfw,
    PlaySpeakerChime,
    RecordMic,
    PlaybackMic,
    PreviewCamera(String),
    TouchConfirm,
    ToggleBundle(&'static str),
    InstallSelectedBundles,
    SetAiPosture(&'static str),
    ToggleClipboard,
    ToggleMicMute,
    ToggleAutomount,
    ProfileBackup(PathBuf),
    FinishSetup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupPhase {
    Probing,
    Interactive,
}

pub struct SetupScreen {
    pub state: SetupState,
    pub snap: SetupSnapshot,
    pub step: StepId,
    pub list: ListState,
    actions: Vec<RowAction>,
    selected_bundles: BTreeSet<&'static str>,
    pub pending_delegate: Option<DelegateAction>,
    pub status_msg: Option<String>,
    mic_sample_path: Option<PathBuf>,
    mic_peak: Option<f32>,
    touch_verified: bool,
    probe_rx: Option<mpsc::Receiver<SetupSnapshot>>,
    phase: SetupPhase,
    header: Header,
    keybar: Vec<KeyHint>,
    rows_height: Cell<u16>,
    /// True when running in deterministic test mode (skips disk writes / probes).
    silent_test: bool,
}

impl SetupScreen {
    /// Interactive screen backed by live background probes and `setup.json`.
    pub fn new(initial_step: Option<StepId>) -> Self {
        let state = SetupState::load();
        let step = initial_step.unwrap_or_else(|| state.resume_step());
        let (tx, rx) = mpsc::channel();
        let state_clone = state.clone();
        let _ = std::thread::Builder::new()
            .name("hayato-setup-probe".into())
            .spawn(move || {
                let _ = tx.send(SetupSnapshot::collect(&state_clone));
            });

        let mut screen = Self {
            state,
            snap: SetupSnapshot::default(),
            step,
            list: ListState::new(),
            actions: Vec::new(),
            selected_bundles: BTreeSet::new(),
            pending_delegate: None,
            status_msg: None,
            mic_sample_path: None,
            mic_peak: None,
            touch_verified: false,
            probe_rx: Some(rx),
            phase: SetupPhase::Probing,
            header: Header::new("Hayato").subtitle("Personal Setup Assistant"),
            keybar: vec![
                KeyHint::new("Enter", "run"),
                KeyHint::new("Tab", "next step"),
                KeyHint::new("d", "done"),
                KeyHint::new("s", "skip"),
                KeyHint::new("f", "finish"),
                KeyHint::new("Esc", "pause"),
            ],
            rows_height: Cell::new(12),
            silent_test: false,
        };
        screen.rebuild_rows();
        screen
    }

    /// Deterministic fixture screen for `tests/screens.rs` snapshots.
    pub fn for_snapshot() -> Self {
        let state = SetupState::for_machine("snapshot-machine".into(), 0);
        let mut screen = Self {
            state,
            snap: SetupSnapshot::sample(),
            step: StepId::Welcome,
            list: ListState::new(),
            actions: Vec::new(),
            selected_bundles: BTreeSet::new(),
            pending_delegate: None,
            status_msg: None,
            mic_sample_path: None,
            mic_peak: None,
            touch_verified: false,
            probe_rx: None,
            phase: SetupPhase::Interactive,
            header: Header::new("Hayato").subtitle("Personal Setup Assistant"),
            keybar: vec![
                KeyHint::new("Enter", "run"),
                KeyHint::new("Tab", "next step"),
                KeyHint::new("d", "done"),
                KeyHint::new("s", "skip"),
                KeyHint::new("f", "finish"),
                KeyHint::new("Esc", "pause"),
            ],
            rows_height: Cell::new(12),
            silent_test: true,
        };
        screen.rebuild_rows();
        screen
    }

    pub fn refresh_probes_sync(&mut self) {
        if self.silent_test {
            return;
        }
        self.snap = SetupSnapshot::collect(&self.state);
        self.phase = SetupPhase::Interactive;
        self.rebuild_rows();
    }

    fn poll_probes(&mut self) {
        if let Some(rx) = &self.probe_rx
            && let Ok(snap) = rx.try_recv()
        {
            self.snap = snap;
            self.probe_rx = None;
            self.phase = SetupPhase::Interactive;
            self.rebuild_rows();
        }
    }

    fn save_state(&mut self) {
        if !self.silent_test {
            let _ = self.state.save();
        }
    }

    fn go_step(&mut self, next: StepId) {
        self.step = next;
        self.state.last_step = next;
        self.status_msg = None;
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut actions = Vec::new();

        let mut push = |label: String, detail: String, glyph: Glyph, act: RowAction| {
            rows.push(Row::new(label).detail(detail).glyph(glyph));
            actions.push(act);
        };

        match self.step {
            StepId::Welcome => {
                push(
                    "Continue to Security (Step 2)".to_string(),
                    "recommended first step".to_string(),
                    Glyph::Next,
                    RowAction::NextStep,
                );
                push(
                    "Open Interactive Learn Manual".to_string(),
                    "shortcuts, gestures & Hayato tools".to_string(),
                    Glyph::Book,
                    RowAction::Delegate(DelegateAction::OpenLearn),
                );
                push(
                    "Finish & Enter Desktop".to_string(),
                    "dismiss first-run (reopen anytime in Settings)".to_string(),
                    Glyph::Check,
                    RowAction::FinishSetup,
                );
            }
            StepId::Security => {
                let pw_detail = if self.snap.default_password_risk {
                    "WARNING: default password may still be active".to_string()
                } else {
                    format!("account: {}", self.snap.username)
                };
                push(
                    "Change Account Password".to_string(),
                    pw_detail,
                    Glyph::Key,
                    RowAction::Delegate(DelegateAction::ChangePassword),
                );
                push(
                    if self.snap.ufw_active {
                        "Disable Firewall (UFW)".to_string()
                    } else {
                        "Enable Firewall (UFW)".to_string()
                    },
                    if self.snap.ufw_active {
                        "active · blocks unsolicited incoming traffic".to_string()
                    } else {
                        "inactive · recommended (note: LAN Share uses local ports)".to_string()
                    },
                    Glyph::Shield,
                    RowAction::ToggleUfw,
                );
                push(
                    "SSH Remote Login".to_string(),
                    format!(
                        "{} · {} authorized key(s)",
                        if self.snap.ssh_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        self.snap.ssh_keys_count
                    ),
                    Glyph::Terminal,
                    RowAction::Delegate(DelegateAction::OpenSsh),
                );
                push(
                    "Remote Desktop (VNC)".to_string(),
                    if self.snap.vnc_running {
                        "running on localhost:5900".to_string()
                    } else {
                        "off (on-demand only)".to_string()
                    },
                    Glyph::Display,
                    RowAction::Delegate(DelegateAction::OpenVnc),
                );
                push(
                    "Screen Lock & Idle Timeouts".to_string(),
                    "configure swayidle & swaylock".to_string(),
                    Glyph::Lock,
                    RowAction::Delegate(DelegateAction::OpenIdle),
                );
            }
            StepId::Connectivity => {
                push(
                    "Configure Wi-Fi Networks".to_string(),
                    self.snap.connection_summary.clone(),
                    Glyph::Wifi,
                    RowAction::Delegate(DelegateAction::OpenNet),
                );
                if self.snap.bt_present {
                    push(
                        "Pair Bluetooth Devices".to_string(),
                        if self.snap.bt_powered {
                            "radio on".to_string()
                        } else {
                            "radio off".to_string()
                        },
                        Glyph::Bluetooth,
                        RowAction::Delegate(DelegateAction::OpenBluetooth),
                    );
                }
                push(
                    "Configure Private DNS".to_string(),
                    "Cloudflare, Quad9, AdGuard or custom".to_string(),
                    Glyph::Globe,
                    RowAction::Delegate(DelegateAction::OpenDns),
                );
                push(
                    "Configure VPN (WireGuard / OpenVPN)".to_string(),
                    format!("{} profile(s) configured", self.snap.vpn_count),
                    Glyph::Shield,
                    RowAction::Delegate(DelegateAction::OpenVpn),
                );
            }
            StepId::Personalization => {
                push(
                    "Choose Theme & Wallpaper".to_string(),
                    format!("active: {}", self.snap.theme_name),
                    Glyph::Theme,
                    RowAction::Delegate(DelegateAction::OpenThemePicker),
                );
                push(
                    "Timezone & Network Clock".to_string(),
                    format!(
                        "{} · NTP {}",
                        self.snap.timezone,
                        if self.snap.ntp_synced {
                            "synced"
                        } else {
                            "pending"
                        }
                    ),
                    Glyph::Clock,
                    RowAction::Delegate(DelegateAction::OpenTz),
                );
                push(
                    "Language & Region (Locale)".to_string(),
                    format!("{} (applies next login)", self.snap.locale),
                    Glyph::Globe,
                    RowAction::Delegate(DelegateAction::OpenLocale),
                );
                push(
                    "Keyboard Layout".to_string(),
                    format!("active: {}", self.snap.keymap),
                    Glyph::Keyboard,
                    RowAction::Delegate(DelegateAction::OpenLayout),
                );
                if self.snap.touchpad_present {
                    push(
                        "Touchpad & Natural Scrolling".to_string(),
                        "tap-to-click, scroll direction, speed".to_string(),
                        Glyph::Sliders,
                        RowAction::Delegate(DelegateAction::OpenTouchpad),
                    );
                }
            }
            StepId::Hardware => {
                push(
                    "Test Speakers (Stereo Chime)".to_string(),
                    self.snap.audio_sink.clone(),
                    Glyph::Volume,
                    RowAction::PlaySpeakerChime,
                );
                let mic_detail = match self.mic_peak {
                    Some(pk) => format!(
                        "last peak {:.0}% · {}",
                        pk * 100.0,
                        if pk > 0.03 {
                            "signal detected"
                        } else {
                            "very quiet — check mute"
                        }
                    ),
                    None => format!(
                        "record 3s sample · mic {}",
                        if self.snap.mic_muted {
                            "MUTED"
                        } else {
                            "active"
                        }
                    ),
                };
                push(
                    "Test Microphone (3s Record)".to_string(),
                    mic_detail,
                    Glyph::Mic,
                    RowAction::RecordMic,
                );
                if self.mic_sample_path.is_some() {
                    push(
                        "Play Back Mic Recording".to_string(),
                        "hear what your microphone captured".to_string(),
                        Glyph::Play,
                        RowAction::PlaybackMic,
                    );
                }
                if let Some(dev) = &self.snap.webcam_device {
                    push(
                        "Test Camera Live Preview".to_string(),
                        format!("detected {dev} (q closes preview)"),
                        Glyph::Camera,
                        RowAction::PreviewCamera(dev.clone()),
                    );
                }
                if self.snap.touch_present {
                    push(
                        "Verify Touchscreen Tap".to_string(),
                        if self.touch_verified {
                            "touch verified".to_string()
                        } else {
                            "tap or click this row to confirm touch input".to_string()
                        },
                        Glyph::Sparkle,
                        RowAction::TouchConfirm,
                    );
                }
                push(
                    "External Display & Scale".to_string(),
                    format!(
                        "{} ({} active)",
                        self.snap.display_summary, self.snap.display_count
                    ),
                    Glyph::Display,
                    RowAction::Delegate(DelegateAction::OpenDisplay),
                );
                push(
                    "Audio Output & Input Mixer".to_string(),
                    "switch speakers, headphones or BT sink".to_string(),
                    Glyph::Sliders,
                    RowAction::Delegate(DelegateAction::OpenAudio),
                );
            }
            StepId::Software => {
                for bundle in BUNDLES {
                    let selected = self.selected_bundles.contains(bundle.id);
                    let status = self
                        .snap
                        .bundles
                        .iter()
                        .find(|b| b.id == bundle.id)
                        .map(|b| b.label())
                        .unwrap_or_else(|| "Available".to_string());
                    let mark = if selected { "[x]" } else { "[ ]" };
                    push(
                        format!("{mark} {} (~{} MB)", bundle.name, bundle.est_mb),
                        format!("{status} · {}", bundle.summary),
                        Glyph::Box,
                        RowAction::ToggleBundle(bundle.id),
                    );
                }
                push(
                    format!(
                        "Install Selected Bundles ({} chosen)",
                        self.selected_bundles.len()
                    ),
                    if self.snap.online {
                        format!("{:.1} GiB free on /", self.snap.root_free_gb)
                    } else {
                        "connect to Wi-Fi in Step 3 first".to_string()
                    },
                    Glyph::Download,
                    RowAction::InstallSelectedBundles,
                );
                push(
                    "Browse All Debian Packages".to_string(),
                    "search & install via APT".to_string(),
                    Glyph::Search,
                    RowAction::Delegate(DelegateAction::OpenPkg),
                );
                push(
                    "Default Applications".to_string(),
                    format!("browser: {}", self.snap.default_browser),
                    Glyph::Desktop,
                    RowAction::Delegate(DelegateAction::OpenDefaultApps),
                );
            }
            StepId::Privacy => {
                push(
                    "AI Posture: Local-Only Offline".to_string(),
                    if self.snap.local_llm_present {
                        "Qwen3.5-0.8B ready · zero cloud traffic".to_string()
                    } else {
                        "prefer offline model (fetch in Software step)".to_string()
                    },
                    Glyph::Sparkle,
                    RowAction::SetAiPosture("local"),
                );
                push(
                    "AI Posture: Cloud Coding Agents".to_string(),
                    format!("current default: {}", self.snap.default_agent),
                    Glyph::Globe,
                    RowAction::SetAiPosture("opencode"),
                );
                push(
                    if self.snap.clipboard_enabled {
                        "Disable & Wipe Clipboard History".to_string()
                    } else {
                        "Enable Clipboard History".to_string()
                    },
                    if self.snap.clipboard_enabled {
                        "currently saving history (Alt+Ctrl+V)".to_string()
                    } else {
                        "currently disabled (no history stored)".to_string()
                    },
                    Glyph::Clipboard,
                    RowAction::ToggleClipboard,
                );
                push(
                    "Toggle Default Microphone Mute".to_string(),
                    if self.snap.mic_muted {
                        "currently muted (Alt+Shift+M)".to_string()
                    } else {
                        "currently unmuted (Alt+Shift+M)".to_string()
                    },
                    Glyph::Mic,
                    RowAction::ToggleMicMute,
                );
                push(
                    if self.snap.automount_enabled {
                        "Disable USB Auto-Mount".to_string()
                    } else {
                        "Enable USB Auto-Mount".to_string()
                    },
                    if self.snap.automount_enabled {
                        "USB drives mount automatically".to_string()
                    } else {
                        "manual mount only in Files".to_string()
                    },
                    Glyph::Disk,
                    RowAction::ToggleAutomount,
                );
                push(
                    "Notifications & Do Not Disturb".to_string(),
                    "configure timeout & DND".to_string(),
                    Glyph::Bell,
                    RowAction::Delegate(DelegateAction::OpenNotifySettings),
                );
            }
            StepId::Protection => {
                let profile_dest = self
                    .snap
                    .backup_drives
                    .first()
                    .map(|(_, p, _)| p.clone())
                    .unwrap_or_else(|| {
                        let home = std::env::var("HOME")
                            .map(PathBuf::from)
                            .unwrap_or_else(|_| PathBuf::from("/tmp"));
                        home.join("Backups")
                    });
                push(
                    "Create Encrypted Profile Backup".to_string(),
                    format!("saves .tar.zst.gpg to {}", profile_dest.display()),
                    Glyph::Shield,
                    RowAction::ProfileBackup(profile_dest),
                );
                if let Some((label, mp, avail)) = self.snap.backup_drives.first() {
                    push(
                        format!("Full System Backup to {label}"),
                        format!("{avail:.1} GiB free at {}", mp.display()),
                        Glyph::Disk,
                        RowAction::Delegate(DelegateAction::SystemBackup(mp.clone())),
                    );
                }
                push(
                    "Configure Auxiliary Swap".to_string(),
                    format!(
                        "{:.1} GiB RAM + zram · aux swap {}",
                        self.snap.memory_gib,
                        if self.snap.aux_swap_configured {
                            "configured"
                        } else {
                            "not set"
                        }
                    ),
                    Glyph::Chip,
                    RowAction::Delegate(DelegateAction::OpenSwap),
                );
                push(
                    "Free Disk Space & Caches".to_string(),
                    format!("{:.1} GiB free on /", self.snap.root_free_gb),
                    Glyph::Trash,
                    RowAction::Delegate(DelegateAction::OpenClean),
                );
                push(
                    "Finish Setup Assistant".to_string(),
                    "mark first-run complete & enter desktop".to_string(),
                    Glyph::Check,
                    RowAction::FinishSetup,
                );
            }
        }

        self.actions = actions;
        self.list.set_rows(rows);
    }

    fn execute_selected(&mut self) -> Outcome {
        let Some(idx) = self.list.selected_index() else {
            return Outcome::Continue;
        };
        let Some(act) = self.actions.get(idx).cloned() else {
            return Outcome::Continue;
        };

        match act {
            RowAction::Delegate(d) => {
                self.pending_delegate = Some(d);
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
            RowAction::NextStep => {
                self.state
                    .mark(self.step, StepStatus::Done, StepSource::User);
                self.save_state();
                if let Some(next) = self.step.next() {
                    self.go_step(next);
                }
            }
            RowAction::ToggleUfw => match backup::toggle_ufw(self.snap.ufw_active) {
                Ok(msg) => {
                    self.snap.ufw_active = !self.snap.ufw_active;
                    self.state
                        .mark(StepId::Security, StepStatus::Done, StepSource::User);
                    self.save_state();
                    self.status_msg = Some(msg);
                    self.rebuild_rows();
                }
                Err(e) => self.status_msg = Some(format!("Firewall error: {e}")),
            },
            RowAction::PlaySpeakerChime => match hardware::play_speaker_chime() {
                Ok(msg) => {
                    self.status_msg = Some(msg);
                }
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::RecordMic => match hardware::record_mic_sample() {
                Ok((path, peak)) => {
                    self.mic_sample_path = Some(path);
                    self.mic_peak = Some(peak);
                    self.status_msg = Some(format!(
                        "Recorded 3s mic test · peak {:.0}% (select Play Back to listen)",
                        peak * 100.0
                    ));
                    self.rebuild_rows();
                }
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::PlaybackMic => {
                if let Some(path) = &self.mic_sample_path {
                    match hardware::playback_mic_sample(path) {
                        Ok(msg) => self.status_msg = Some(msg),
                        Err(e) => self.status_msg = Some(e),
                    }
                }
            }
            RowAction::PreviewCamera(dev) => match hardware::preview_camera(&dev) {
                Ok(msg) => self.status_msg = Some(msg),
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::TouchConfirm => {
                self.touch_verified = true;
                self.state
                    .mark(StepId::Hardware, StepStatus::Done, StepSource::User);
                self.save_state();
                self.status_msg = Some("Touchscreen input confirmed!".to_string());
                self.rebuild_rows();
            }
            RowAction::ToggleBundle(id) => {
                if !self.selected_bundles.remove(id) {
                    self.selected_bundles.insert(id);
                }
                self.rebuild_rows();
            }
            RowAction::InstallSelectedBundles => {
                if self.selected_bundles.is_empty() {
                    self.status_msg =
                        Some("Select at least one bundle first (Enter toggles)".to_string());
                } else if !self.snap.online {
                    self.status_msg =
                        Some("Offline — connect to Wi-Fi in Step 3 before installing".to_string());
                } else {
                    let ids: Vec<&'static str> = self.selected_bundles.iter().copied().collect();
                    self.pending_delegate = Some(DelegateAction::InstallBundles(ids));
                    return Outcome::Done(hayato_ui::app::exit::OK);
                }
            }
            RowAction::SetAiPosture(posture) => match backup::set_default_ai_posture(posture) {
                Ok(msg) => {
                    self.snap.default_agent = posture.to_string();
                    self.state
                        .mark(StepId::Privacy, StepStatus::Done, StepSource::User);
                    self.save_state();
                    self.status_msg = Some(msg);
                    self.rebuild_rows();
                }
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::ToggleClipboard => {
                match backup::set_clipboard_enabled(!self.snap.clipboard_enabled) {
                    Ok(msg) => {
                        self.snap.clipboard_enabled = !self.snap.clipboard_enabled;
                        self.state
                            .mark(StepId::Privacy, StepStatus::Done, StepSource::User);
                        self.save_state();
                        self.status_msg = Some(msg);
                        self.rebuild_rows();
                    }
                    Err(e) => self.status_msg = Some(e),
                }
            }
            RowAction::ToggleMicMute => match backup::toggle_mic_mute() {
                Ok(msg) => {
                    self.snap.mic_muted = !self.snap.mic_muted;
                    self.status_msg = Some(msg);
                    self.rebuild_rows();
                }
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::ToggleAutomount => {
                match backup::set_automount_enabled(!self.snap.automount_enabled) {
                    Ok(msg) => {
                        self.snap.automount_enabled = !self.snap.automount_enabled;
                        self.status_msg = Some(msg);
                        self.rebuild_rows();
                    }
                    Err(e) => self.status_msg = Some(e),
                }
            }
            RowAction::ProfileBackup(dest) => match backup::run_profile_backup(&dest) {
                Ok(msg) => {
                    self.snap.existing_backup_found = true;
                    self.state
                        .mark(StepId::Protection, StepStatus::Done, StepSource::User);
                    self.save_state();
                    self.status_msg = Some(msg);
                    self.rebuild_rows();
                }
                Err(e) => self.status_msg = Some(e),
            },
            RowAction::FinishSetup => {
                self.state.dismissed_first_run = true;
                self.state
                    .mark(self.step, StepStatus::Done, StepSource::User);
                self.save_state();
                return Outcome::Done(hayato_ui::app::exit::OK);
            }
        }
        Outcome::Consumed
    }

    fn summary_lines(&self) -> Vec<String> {
        let s = &self.snap;
        if !s.loaded {
            return vec!["Discovering hardware and current system settings…".to_string()];
        }
        match self.step {
            StepId::Welcome => vec![
                format!(
                    "{} ({}) · Linux {} · {:.1} GiB RAM",
                    s.board, s.hostname, s.kernel, s.memory_gib
                ),
                format!(
                    "Provisioning: {} · Boot: {} · Hayato {}",
                    s.provisioning, s.boot_type, s.version
                ),
                "Every step is optional & resumable. Press Tab for next, or Enter to run an action."
                    .to_string(),
            ],
            StepId::Security => vec![
                format!(
                    "Root disk: {} ({}) · Firewall: {} · SSH: {}",
                    s.root_fstype,
                    if s.root_encrypted {
                        "LUKS encrypted"
                    } else {
                        "unencrypted ext4"
                    },
                    if s.ufw_active { "active" } else { "inactive" },
                    if s.ssh_enabled { "on" } else { "off" }
                ),
                if s.default_password_risk {
                    "Alert: pre-built image default password detected — change it below."
                        .to_string()
                } else {
                    "Account password configured · sudo is passwordless for admin tasks."
                        .to_string()
                },
            ],
            StepId::Connectivity => vec![
                format!(
                    "Network: {} · Bluetooth: {} · VPNs: {}",
                    s.connection_summary,
                    if !s.bt_present {
                        "no adapter"
                    } else if s.bt_powered {
                        "on"
                    } else {
                        "off"
                    },
                    s.vpn_count
                ),
                "Tip: connect Wi-Fi here if you want to install Software bundles in Step 6."
                    .to_string(),
            ],
            StepId::Personalization => vec![
                format!(
                    "Theme: {} · Locale: {} · TZ: {} · Keys: {}",
                    s.theme_name, s.locale, s.timezone, s.keymap
                ),
                if s.installed_fresh {
                    "Fresh installer already configured locale, timezone & keyboard — tweak anytime."
                        .to_string()
                } else {
                    "Pre-built image defaults active — confirm your timezone, language & theme."
                        .to_string()
                },
            ],
            StepId::Hardware => vec![
                format!(
                    "Audio: {} · Camera: {} · Touch: {}",
                    s.audio_sink,
                    s.webcam_device.as_deref().unwrap_or("none"),
                    if s.touch_present { "yes" } else { "no" }
                ),
                format!(
                    "Displays: {} ({} active) — run quick self-tests below.",
                    s.display_summary, s.display_count
                ),
            ],
            StepId::Software => vec![
                format!(
                    "Disk free: {:.1} GiB · Network: {} · Browser: {}",
                    s.root_free_gb,
                    if s.online { "online" } else { "offline" },
                    s.default_browser
                ),
                "Press Enter on bundles to toggle [x], then select Install Selected Bundles."
                    .to_string(),
            ],
            StepId::Privacy => {
                vec![
                format!(
                    "AI: {} (local LLM: {}, Whisper: {}) · Clipboard: {}",
                    s.default_agent,
                    if s.local_llm_present { "ready" } else { "not fetched" },
                    if s.whisper_present { "ready" } else { "not fetched" },
                    if s.clipboard_enabled { "on" } else { "off" }
                ),
                "Local AI & Share run on-device with zero cloud daemon; Translate uses web APIs."
                    .to_string(),
            ]
            }
            StepId::Protection => vec![
                format!(
                    "Backup drives: {} mounted · Existing backup: {} · Aux swap: {}",
                    s.backup_drives.len(),
                    if s.existing_backup_found {
                        "found"
                    } else {
                        "none"
                    },
                    if s.aux_swap_configured { "on" } else { "off" }
                ),
                backup::recovery_guidance(&s.boot_type).to_string(),
            ],
        }
    }
}

impl Screen for SetupScreen {
    fn desired_size(&self) -> Option<(u16, u16)> {
        Some((940, 620))
    }

    fn header(&self) -> Option<&Header> {
        Some(&self.header)
    }

    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.phase {
            SetupPhase::Probing => "probing",
            SetupPhase::Interactive => "interactive",
        })
    }

    fn tick(&mut self) -> Outcome {
        let was_probing = self.probe_rx.is_some();
        self.poll_probes();
        if was_probing && self.probe_rx.is_none() {
            Outcome::Consumed
        } else {
            Outcome::Continue
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        self.poll_probes();
        let area = ctx.area;
        if area.width < 52 || area.height < 14 {
            return;
        }

        let left_w = ((area.width as u32 * 30) / 100).clamp(24, 30) as u16;
        let right_w = area.width.saturating_sub(left_w);
        let left_rect = Rect::new(area.x, area.y, left_w, area.height);
        let right_rect = Rect::new(area.x + left_w, area.y, right_w, area.height);

        // ── Left Rail: 8-Step Progress Stepper ────────────────────────────
        let progress = self.state.summary_badge();
        let rail_title = format!("Steps · {progress}");
        let rail = draw_card_with(
            ctx.buf,
            left_rect,
            ctx.design,
            Some(&rail_title),
            ColorRole::SurfaceAlt,
            ctx.design.style(TextStyle::Title),
        );

        for (i, (ry, step)) in (rail.y..rail.bottom()).zip(StepId::ALL).enumerate() {
            let active = step == self.step;
            let health = step_health(step, &self.snap, &self.state);
            let status = self.state.status(step);
            let (mark, mark_role) = match (status, health) {
                (StepStatus::Done, StepHealth::Attention) => ("!", ColorRole::Warn),
                (StepStatus::Done, _) => ("\u{25cf}", ColorRole::Ok),
                (StepStatus::Skipped, _) => ("\u{2013}", ColorRole::Dim),
                (StepStatus::Pending, StepHealth::Attention) => ("!", ColorRole::Warn),
                (StepStatus::Pending, StepHealth::Ready) => ("\u{25cf}", ColorRole::Ok),
                (StepStatus::Pending, StepHealth::Pending) => ("\u{25cb}", ColorRole::Dim),
            };
            let prefix = if active { "\u{258e} " } else { "  " };
            let text_style = if active {
                Style::default()
                    .fg(ctx.design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                ctx.design.style(TextStyle::Body)
            };
            put(ctx.buf, rail.x, ry, prefix, 2, text_style);
            put(
                ctx.buf,
                rail.x + 2,
                ry,
                mark,
                1,
                Style::default().fg(ctx.design.color(mark_role)),
            );
            put_trunc(
                ctx.buf,
                rail.x + 4,
                ry,
                &format!("{} {}", i + 1, step.title()),
                rail.width.saturating_sub(4),
                text_style,
            );
        }

        // ── Right Pane: Step Summary + Action List ────────────────────────
        let pane_title = format!("Step {}/8: {}", self.step.index() + 1, self.step.title());
        let card = draw_card_with(
            ctx.buf,
            right_rect,
            ctx.design,
            Some(&pane_title),
            ColorRole::Accent,
            ctx.design.style(TextStyle::Title),
        );

        let px = card.x + 1;
        let pw = card.width.saturating_sub(2);
        let mut py = card.y;

        put_trunc(
            ctx.buf,
            px,
            py,
            self.step.subtitle(),
            pw,
            Style::default()
                .fg(ctx.design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
        );
        py += 1;

        for line in self.summary_lines() {
            if py + 2 >= card.bottom() {
                break;
            }
            put_trunc(ctx.buf, px, py, &line, pw, ctx.design.style(TextStyle::Dim));
            py += 1;
        }
        py += 1;

        let reserve_toast = if self.status_msg.is_some() { 2 } else { 0 };
        let list_h = card.bottom().saturating_sub(py + reserve_toast);
        if list_h > 0 {
            let rows_area = Rect::new(px, py, pw, list_h);
            self.rows_height.set(rows_area.height);
            self.list.render(rows_area, ctx.buf, ctx.design, true);
        }

        if let Some(msg) = &self.status_msg {
            let ty = card.bottom().saturating_sub(1);
            put_trunc(
                ctx.buf,
                px,
                ty,
                &format!("\u{2139} {msg}"),
                pw,
                Style::default().fg(ctx.design.color(ColorRole::Ok)),
            );
        }
    }

    fn hit(&self, area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let left_w = ((area.width as u32 * 30) / 100).clamp(24, 30) as u16;
        // Clicking a step in the left rail returns encoded index `1000 + step_idx`
        if x >= area.x && x < area.x + left_w {
            let step_idx = y.checked_sub(area.y + 1)? as usize;
            if step_idx < StepId::ALL.len() {
                return Some(1000 + step_idx);
            }
            return None;
        }
        let right_x = area.x + left_w + 2;
        let right_w = area.width.saturating_sub(left_w + 4);
        let summary_h = self.summary_lines().len() as u16 + 2;
        let list_y = area.y + 1 + summary_h;
        let rows_area = Rect::new(
            right_x,
            list_y,
            right_w,
            area.bottom().saturating_sub(list_y + 1),
        );
        self.list.hit_row(rows_area, x, y, design)
    }

    fn click(&mut self, row: Option<usize>) -> Outcome {
        let Some(r) = row else {
            return Outcome::Continue;
        };
        if r >= 1000 {
            if let Some(&step) = StepId::ALL.get(r - 1000) {
                self.go_step(step);
            }
            return Outcome::Consumed;
        }
        self.list.set_cursor(r);
        self.execute_selected()
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                // Pause for later without setting dismissed_first_run.
                Outcome::Done(hayato_ui::app::exit::ESCAPE)
            }
            KeyCode::Tab | KeyCode::Right => {
                if let Some(next) = self.step.next() {
                    self.go_step(next);
                }
                Outcome::Consumed
            }
            KeyCode::BackTab | KeyCode::Left => {
                if let Some(prev) = self.step.prev() {
                    self.go_step(prev);
                }
                Outcome::Consumed
            }
            KeyCode::Char(c @ '1'..='8') => {
                let idx = (c as usize) - ('1' as usize);
                if let Some(&step) = StepId::ALL.get(idx) {
                    self.go_step(step);
                }
                Outcome::Consumed
            }
            KeyCode::Char('d') => {
                self.state
                    .mark(self.step, StepStatus::Done, StepSource::User);
                self.save_state();
                if let Some(next) = self.step.next() {
                    self.go_step(next);
                } else {
                    self.status_msg = Some("All steps reviewed! Press f to finish.".to_string());
                }
                Outcome::Consumed
            }
            KeyCode::Char('s') => {
                self.state
                    .mark(self.step, StepStatus::Skipped, StepSource::User);
                self.save_state();
                if let Some(next) = self.step.next() {
                    self.go_step(next);
                }
                Outcome::Consumed
            }
            KeyCode::Char('f') => {
                self.state.dismissed_first_run = true;
                self.save_state();
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.execute_selected(),
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k') => {
                let _ = self.list.handle(key, self.rows_height.get(), design);
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }
}

/// Headless status string for Settings & `hayato menu setup --status`.
pub fn status_summary() -> String {
    let st = SetupState::load();
    st.summary_badge()
}

/// Run the interactive Personal Setup Assistant with single-window inline
/// delegation to existing Hayato screens.
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &Design,
) -> Result<i32, Failure> {
    let initial_step = args
        .iter()
        .position(|a| a == "--step")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| StepId::from_slug(s));

    let mut current_design = design.clone();
    let mut screen = SetupScreen::new(initial_step);

    loop {
        let rc = {
            let mut app =
                hayato_ui::App::new(Header::new("Hayato").subtitle("Personal Setup Assistant"))
                    .design(current_design.clone())
                    .screen(&mut screen);
            app.run(terminal)
                .map_err(|e| Failure::new("setup assistant error", e.to_string()))?
        };

        let Some(delegate) = screen.pending_delegate.take() else {
            return Ok(rc);
        };

        run_delegate(delegate, terminal, &mut current_design, &mut screen)?;
        screen.refresh_probes_sync();
    }
}

fn run_delegate(
    action: DelegateAction,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    design: &mut Design,
    screen: &mut SetupScreen,
) -> Result<(), Failure> {
    match action {
        DelegateAction::OpenLearn => {
            let _ = crate::system::learn::menu(terminal, design);
        }
        DelegateAction::OpenThemePicker => {
            let _ = crate::system::theme_picker::menu(terminal, design);
            // Reload live design so the Setup Assistant immediately adopts the new theme!
            *design = Design::for_session(None);
            screen
                .state
                .mark(StepId::Personalization, StepStatus::Done, StepSource::User);
            screen.save_state();
        }
        DelegateAction::OpenNet => {
            let _ = crate::system::net::menu(terminal, design);
            screen
                .state
                .mark(StepId::Connectivity, StepStatus::Done, StepSource::User);
            screen.save_state();
        }
        DelegateAction::OpenBluetooth => {
            let _ = crate::system::bluetooth::menu(terminal, design);
        }
        DelegateAction::OpenDns => {
            let _ = crate::system::dns::menu(terminal, design);
        }
        DelegateAction::OpenSsh => {
            let _ = crate::system::ssh::menu(terminal, design);
        }
        DelegateAction::OpenVnc => {
            let _ = crate::system::vnc::menu(terminal, design);
        }
        DelegateAction::OpenIdle => {
            if let Ok(Some(msg)) = crate::system::settings::run_key("idle", terminal, design) {
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenTz => {
            let _ = crate::system::tz::menu(terminal, design);
            screen
                .state
                .mark(StepId::Personalization, StepStatus::Done, StepSource::User);
            screen.save_state();
        }
        DelegateAction::OpenLayout => {
            let _ = crate::system::layout::menu(terminal, design);
        }
        DelegateAction::OpenLocale => {
            if let Ok(Some(msg)) = crate::system::settings::run_key("locale", terminal, design) {
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenTouchpad => {
            if let Ok(Some(msg)) = crate::system::settings::run_key("touchpad", terminal, design) {
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenDisplay => {
            let _ = crate::system::display::menu(terminal, design);
        }
        DelegateAction::OpenAudio => {
            let _ = crate::system::audio::menu(terminal, design);
        }
        DelegateAction::OpenPkg => {
            let _ = crate::system::pkg::menu(terminal, design);
        }
        DelegateAction::OpenDefaultApps => {
            if let Ok(Some(msg)) =
                crate::system::settings::run_key("default-apps", terminal, design)
            {
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenNotifySettings => {
            if let Ok(Some(msg)) = crate::system::settings::run_key("notify", terminal, design) {
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenSwap => {
            let _ = crate::system::swap::menu(terminal, design);
        }
        DelegateAction::OpenClean => {
            let _ = crate::system::clean::menu(terminal, design);
        }
        DelegateAction::ChangePassword => {
            if let Ok(Some(msg)) = crate::system::settings::run_key("password", terminal, design) {
                if msg == "Password changed" {
                    screen.state.password_verified = true;
                    screen
                        .state
                        .mark(StepId::Security, StepStatus::Done, StepSource::User);
                    screen.save_state();
                }
                screen.status_msg = Some(msg);
            }
        }
        DelegateAction::OpenVpn => {
            let _ = crate::system::vpn::menu(terminal, design);
        }
        DelegateAction::InstallBundles(ids) => {
            let apt_pkgs = bundles::apt_packages(&ids);
            if !apt_pkgs.is_empty()
                && let Ok(crate::system::pkg_guard::Verdict::Block(lines)) =
                    crate::system::pkg_guard::check_install(&apt_pkgs)
            {
                screen.status_msg = Some(format!(
                    "Blocked for safety: {}",
                    crate::system::pkg_guard::summary(&lines)
                ));
                return Ok(());
            }
            if let Some(script) = bundles::build_install_script(&ids) {
                run_interactive_cmd(terminal, "sh", &["-c", &script]);
                screen
                    .state
                    .mark(StepId::Software, StepStatus::Done, StepSource::User);
                screen.save_state();
                screen.status_msg = Some("Finished installing selected bundles".to_string());
            }
        }
        DelegateAction::SystemBackup(mp) => match backup::validate_system_backup_target(&mp) {
            Ok(target) => {
                let cmd = format!(
                    "sudo -n /usr/libexec/hayato/backup-system.sh \"{}\"; echo; read -r -p 'Press Enter to return...' _",
                    target.display()
                );
                run_interactive_cmd(terminal, "sh", &["-c", &cmd]);
                screen
                    .state
                    .mark(StepId::Protection, StepStatus::Done, StepSource::User);
                screen.save_state();
                screen.status_msg = Some(format!("System backup finished: {}", target.display()));
            }
            Err(e) => screen.status_msg = Some(e),
        },
    }
    Ok(())
}

/// Temporarily hand the terminal to a full-screen CLI tool (`nmtui`, `apt-get`,
/// `backup-system.sh`), then restore Ratatui cleanly.
fn run_interactive_cmd(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    program: &str,
    args: &[&str],
) {
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
    let _ = ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::LeaveAlternateScreen
    );
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let _ = ratatui::crossterm::execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::EnterAlternateScreen
    );
    let _ = ratatui::crossterm::terminal::enable_raw_mode();
    let _ = terminal.clear();
}
