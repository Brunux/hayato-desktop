//! Full-screen interactive wizard for Hayato installation (spec 018 & spec 023).

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hayato_ui::app::{BodyCtx, Outcome, Screen};
use hayato_ui::design::{ColorRole, Design, TextStyle};
use hayato_ui::widgets::{
    BRAILLE_SPINNER, Input, KeyHint, ListAction, ListState, Row, draw_card, draw_gauge, put,
    put_trunc,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::config::{
    ApplyRequest, InstallConfig, InstallMode, target_short_name, validate_confirmation,
    validate_hostname, validate_password, validate_username,
};
use super::deviceinfo::BootType;
use super::preflight::Preflight;
use super::worker::{
    PHASES, WorkerPhase, WorkerState, active_running_state, load_state, phase_label,
    resolve_state_dir, run_worker, state_file_path_for,
};

fn fill_rect(buf: &mut ratatui::buffer::Buffer, area: Rect, style: Style) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(style);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStep {
    Welcome = 0,
    Localization = 1,
    UserAccount = 2,
    DiskReview = 3,
    Progress = 4,
    Complete = 5,
}

impl WizardStep {
    pub fn title(self) -> &'static str {
        match self {
            WizardStep::Welcome => "Pre-Flight & System Overview",
            WizardStep::Localization => "Localization, Timezone & Keyboard",
            WizardStep::UserAccount => "User Account & Host Identity",
            WizardStep::DiskReview => "Target Storage & Partition Review",
            WizardStep::Progress => "Installation in Progress",
            WizardStep::Complete => "Installation Complete",
        }
    }
}

#[derive(Clone, Default)]
struct InstallerHits {
    welcome_next: Option<Rect>,
    tz_input: Option<Rect>,
    tz_list: Option<Rect>,
    locale_rect: Option<Rect>,
    keymap_rect: Option<Rect>,
    keymap_test: Option<Rect>,
    loc_back: Option<Rect>,
    loc_next: Option<Rect>,
    user_inputs: [Option<Rect>; 5],
    user_ssh: Option<Rect>,
    user_back: Option<Rect>,
    user_next: Option<Rect>,
    disk_target: Option<Rect>,
    disk_confirm: Option<Rect>,
    disk_cancel: Option<Rect>,
    disk_erase: Option<Rect>,
    progress_log: Option<Rect>,
    complete_reboot: Option<Rect>,
    complete_exit: Option<Rect>,
}

/// Whether a worker is attached, and whether this process owns its cancel
/// token. An attached detached run is active but not cancellable from here.
enum WorkerRun {
    Idle,
    Active { cancel: Option<Arc<AtomicBool>> },
}

/// Whether `swayidle` was paused by this screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Swayidle {
    Running,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocFocus {
    TimezoneInput,
    TimezoneList,
    Locale,
    Keymap,
    KeymapTest,
    Back,
    Next,
}

impl LocFocus {
    fn next(self) -> Self {
        match self {
            Self::TimezoneInput => Self::TimezoneList,
            Self::TimezoneList => Self::Locale,
            Self::Locale => Self::Keymap,
            Self::Keymap => Self::KeymapTest,
            Self::KeymapTest => Self::Back,
            Self::Back => Self::Next,
            Self::Next => Self::TimezoneInput,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::TimezoneInput => Self::Next,
            Self::TimezoneList => Self::TimezoneInput,
            Self::Locale => Self::TimezoneList,
            Self::Keymap => Self::Locale,
            Self::KeymapTest => Self::Keymap,
            Self::Back => Self::KeymapTest,
            Self::Next => Self::Back,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserFocus {
    FullName,
    Username,
    Password,
    Confirm,
    Hostname,
    Ssh,
    Back,
    Next,
}

impl UserFocus {
    fn next(self) -> Self {
        match self {
            Self::FullName => Self::Username,
            Self::Username => Self::Password,
            Self::Password => Self::Confirm,
            Self::Confirm => Self::Hostname,
            Self::Hostname => Self::Ssh,
            Self::Ssh => Self::Back,
            Self::Back => Self::Next,
            Self::Next => Self::FullName,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::FullName => Self::Next,
            Self::Username => Self::FullName,
            Self::Password => Self::Username,
            Self::Confirm => Self::Password,
            Self::Hostname => Self::Confirm,
            Self::Ssh => Self::Hostname,
            Self::Back => Self::Ssh,
            Self::Next => Self::Back,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskFocus {
    Cancel,
    Confirm,
    Erase,
    Target,
}

impl DiskFocus {
    fn next(self, multi: bool) -> Self {
        match self {
            Self::Cancel => Self::Confirm,
            Self::Confirm => Self::Erase,
            Self::Erase if multi => Self::Target,
            Self::Erase => Self::Cancel,
            Self::Target => Self::Cancel,
        }
    }

    fn prev(self, multi: bool) -> Self {
        match self {
            Self::Cancel if multi => Self::Target,
            Self::Cancel => Self::Erase,
            Self::Confirm => Self::Cancel,
            Self::Erase => Self::Confirm,
            Self::Target => Self::Erase,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompleteFocus {
    Reboot,
    Exit,
}

impl CompleteFocus {
    fn toggle(self) -> Self {
        match self {
            Self::Reboot => Self::Exit,
            Self::Exit => Self::Reboot,
        }
    }
}

pub struct InstallerScreen {
    pub step: WizardStep,
    pub preflight: Preflight,
    pub config: InstallConfig,
    // Step 1: Localization
    pub tz_input: Input,
    pub tz_list: ListState,
    pub selected_tz: String,
    pub locales: Vec<&'static str>,
    pub locale_idx: usize,
    pub keymaps: Vec<&'static str>,
    pub keymap_idx: usize,
    pub keymap_test: Input,
    pub loc_focus: LocFocus,
    // Step 2: User Account
    pub fullname_input: Input,
    pub username_input: Input,
    pub password_input: Input,
    pub confirm_input: Input,
    pub hostname_input: Input,
    pub regen_ssh: bool,
    pub user_focus: UserFocus,
    pub user_error: Option<&'static str>,
    // Step 3: Disk Review & Gated Confirmation
    pub disk_idx: usize,
    pub disk_confirm_input: Input,
    pub disk_focus: DiskFocus,
    // Step 4: Progress
    pub worker_state: WorkerState,
    worker_run: WorkerRun,
    pub spinner_idx: usize,
    pub show_log_full: bool,
    swayidle: Swayidle,
    // Step 5: Complete
    pub complete_focus: CompleteFocus,
    // Cursor blink & mouse hit regions
    cursor_blink_on: bool,
    last_blink: Instant,
    hits: RefCell<InstallerHits>,
    // Keybar cache
    keybar_hints: Vec<KeyHint>,
}

const COMMON_TIMEZONES: &[&str] = &[
    "UTC",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Toronto",
    "America/Mexico_City",
    "America/Bogota",
    "America/Lima",
    "America/Santiago",
    "America/Sao_Paulo",
    "America/Buenos_Aires",
    "Europe/London",
    "Europe/Madrid",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Rome",
    "Europe/Amsterdam",
    "Europe/Warsaw",
    "Europe/Kyiv",
    "Europe/Athens",
    "Asia/Tokyo",
    "Asia/Shanghai",
    "Asia/Hong_Kong",
    "Asia/Singapore",
    "Asia/Kolkata",
    "Asia/Dubai",
    "Australia/Sydney",
    "Australia/Melbourne",
    "Pacific/Auckland",
];

const LOCALES: &[&str] = &[
    "en_US.UTF-8",
    "es_ES.UTF-8",
    "fr_FR.UTF-8",
    "de_DE.UTF-8",
    "pt_BR.UTF-8",
    "it_IT.UTF-8",
    "ja_JP.UTF-8",
    "zh_CN.UTF-8",
];

const KEYMAPS: &[&str] = &["us", "es", "latam", "gb", "de", "fr", "it", "pt"];

impl Drop for InstallerScreen {
    fn drop(&mut self) {
        self.resume_swayidle();
    }
}

impl InstallerScreen {
    pub fn new(preflight: Preflight, dry_run: bool) -> Self {
        let mut screen = Self::from_preflight_isolated(preflight, dry_run);
        if !dry_run {
            if let Some((_dir, state)) = active_running_state() {
                screen.worker_state = state;
                screen.worker_run = WorkerRun::Active { cancel: None };
                screen.step = WizardStep::Progress;
                screen.pause_swayidle();
                screen.update_keybar();
            }
        }
        screen
    }

    /// Construct wizard cleanly without checking host `/run/hayato-install` (ideal for tests).
    pub fn from_preflight_isolated(preflight: Preflight, dry_run: bool) -> Self {
        let live_tz = detect_live_timezone().unwrap_or_else(|| "UTC".to_string());
        let live_locale = detect_live_locale().unwrap_or_else(|| "en_US.UTF-8".to_string());
        let live_keymap = detect_live_keymap().unwrap_or_else(|| "us".to_string());
        let (live_user, live_full) = detect_live_user();
        let live_host = detect_live_hostname().unwrap_or_else(|| "hayato".to_string());

        let locale_idx = LOCALES.iter().position(|&l| l == live_locale).unwrap_or(0);
        let keymap_idx = KEYMAPS.iter().position(|&k| k == live_keymap).unwrap_or(0);

        let disk_idx = preflight
            .candidates
            .iter()
            .position(|c| c.path == preflight.target_disk)
            .unwrap_or(0);

        let state_dir = if dry_run {
            Some(resolve_state_dir(None).to_string_lossy().into_owned())
        } else {
            Some("/run/hayato-install".to_string())
        };

        let config = InstallConfig {
            mode: InstallMode::Fresh,
            fullname: live_full.clone(),
            username: live_user.clone(),
            hostname: live_host.clone(),
            timezone: live_tz.clone(),
            locale: LOCALES[locale_idx].to_string(),
            keymap: KEYMAPS[keymap_idx].to_string(),
            target_disk: preflight.target_disk.clone(),
            dry_run,
            state_dir,
            ..Default::default()
        };

        let mut tz_rows: Vec<Row> = COMMON_TIMEZONES
            .iter()
            .map(|&tz| Row::new(tz).detail("IANA timezone"))
            .collect();
        if !COMMON_TIMEZONES.contains(&live_tz.as_str()) {
            tz_rows.insert(0, Row::new(&live_tz).detail("Detected timezone"));
        }
        let mut tz_list = ListState::new();
        tz_list.set_rows(tz_rows);
        if let Some(pos) = tz_list.rows().iter().position(|r| r.label == live_tz) {
            tz_list.set_cursor(pos);
        }

        let boot_type = preflight.device_info.boot_type;
        let initial_state = WorkerState {
            phase_name: phase_label(0, boot_type).to_string(),
            dry_run,
            ..Default::default()
        };

        let mut screen = Self {
            step: WizardStep::Welcome,
            preflight,
            config,
            tz_input: Input::new(),
            tz_list,
            selected_tz: live_tz,
            locales: LOCALES.to_vec(),
            locale_idx,
            keymaps: KEYMAPS.to_vec(),
            keymap_idx,
            keymap_test: Input::new(),
            loc_focus: LocFocus::TimezoneInput,
            fullname_input: Input::with_text(&live_full),
            username_input: Input::with_text(&live_user),
            password_input: Input::new().with_masked(true),
            confirm_input: Input::new().with_masked(true),
            hostname_input: Input::with_text(&live_host),
            regen_ssh: true,
            user_focus: UserFocus::FullName,
            user_error: None,
            disk_idx,
            disk_confirm_input: Input::new(),
            disk_focus: DiskFocus::Cancel,
            worker_state: initial_state,
            worker_run: WorkerRun::Idle,
            spinner_idx: 0,
            show_log_full: false,
            swayidle: Swayidle::Running,
            complete_focus: CompleteFocus::Reboot,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            hits: RefCell::new(InstallerHits::default()),
            keybar_hints: Vec::new(),
        };
        screen.update_keybar();
        screen
    }

    fn reset_blink(&mut self) {
        self.cursor_blink_on = true;
        self.last_blink = Instant::now();
    }

    fn focused_input_active(&self) -> bool {
        matches!(
            (self.step, self.loc_focus, self.user_focus, self.disk_focus),
            (
                WizardStep::Localization,
                LocFocus::TimezoneInput | LocFocus::KeymapTest,
                _,
                _
            ) | (
                WizardStep::UserAccount,
                _,
                UserFocus::FullName
                    | UserFocus::Username
                    | UserFocus::Password
                    | UserFocus::Confirm
                    | UserFocus::Hostname,
                _
            ) | (WizardStep::DiskReview, _, _, DiskFocus::Confirm)
        )
    }

    fn try_advance_user_account(&mut self) -> Outcome {
        self.config.fullname = self.fullname_input.text().to_string();
        self.config.username = self.username_input.text().to_string();
        self.config.password = self.password_input.text().to_string();
        self.config.password_confirm = self.confirm_input.text().to_string();
        self.config.hostname = self.hostname_input.text().to_string();
        self.config.regenerate_ssh_keys = self.regen_ssh;

        if let Err(err) = self.config.validate_user_fields() {
            self.user_error = Some(err);
            // Move cursor focus directly to the field that failed validation.
            if validate_username(&self.config.username).is_err() {
                self.user_focus = UserFocus::Username;
            } else if validate_password(&self.config.password, &self.config.password_confirm)
                .is_err()
            {
                self.user_focus = if self.config.password.is_empty() {
                    UserFocus::Password
                } else {
                    UserFocus::Confirm
                };
            } else if validate_hostname(&self.config.hostname).is_err() {
                self.user_focus = UserFocus::Hostname;
            }
            self.reset_blink();
            return Outcome::Consumed;
        }
        self.user_error = None;
        self.step = WizardStep::DiskReview;
        self.disk_focus = DiskFocus::Cancel;
        self.reset_blink();
        self.update_keybar();
        Outcome::Consumed
    }

    fn pause_swayidle(&mut self) {
        if self.swayidle == Swayidle::Paused {
            return;
        }
        if Command::new("pkill")
            .args(["-STOP", "-x", "swayidle"])
            .status()
            .is_ok_and(|s| s.success())
        {
            self.swayidle = Swayidle::Paused;
        }
    }

    fn resume_swayidle(&mut self) {
        if self.swayidle == Swayidle::Paused {
            let _ = Command::new("pkill")
                .args(["-CONT", "-x", "swayidle"])
                .status();
            self.swayidle = Swayidle::Running;
        }
    }

    fn target_short(&self) -> &str {
        target_short_name(&self.config.target_disk)
    }

    fn target_size(&self) -> &str {
        self.preflight
            .candidates
            .iter()
            .find(|c| c.path == self.config.target_disk)
            .map(|c| c.size_human.as_str())
            .unwrap_or(&self.preflight.target_size)
    }

    fn cycle_target(&mut self, delta: isize) {
        if self.preflight.candidates.len() <= 1 {
            return;
        }
        let len = self.preflight.candidates.len() as isize;
        let next = ((self.disk_idx as isize + delta).rem_euclid(len)) as usize;
        if next != self.disk_idx {
            self.disk_idx = next;
            self.config.target_disk = self.preflight.candidates[next].path.clone();
            self.disk_confirm_input = Input::new();
        }
    }

    fn update_keybar(&mut self) {
        self.keybar_hints = match self.step {
            WizardStep::Welcome => {
                if self.preflight.is_safe {
                    vec![
                        KeyHint::new("Enter", "continue"),
                        KeyHint::new("Esc", "exit"),
                    ]
                } else {
                    vec![KeyHint::new("Esc / Enter", "exit installer")]
                }
            }
            WizardStep::Localization => vec![
                KeyHint::new("Tab/Shift+Tab", "navigate"),
                KeyHint::new("Enter", "select/next"),
                KeyHint::new("Esc", "back"),
            ],
            WizardStep::UserAccount => vec![
                KeyHint::new("Tab/Shift+Tab", "next field"),
                KeyHint::new("Space", "toggle ssh"),
                KeyHint::new("Enter", "continue"),
                KeyHint::new("Esc", "back"),
            ],
            WizardStep::DiskReview => {
                let mut hints = vec![
                    KeyHint::new("Tab/Shift+Tab", "navigate"),
                    KeyHint::new("Esc", "back"),
                    KeyHint::new("Enter", "confirm/cancel"),
                ];
                if self.preflight.candidates.len() > 1 {
                    hints.push(KeyHint::new("Left/Right", "switch target"));
                }
                hints
            }
            WizardStep::Progress => {
                if self.worker_state.status == WorkerPhase::Failed {
                    vec![KeyHint::new("Esc / Enter", "exit")]
                } else if self.worker_state.first_write || !self.worker_state.cancellable {
                    vec![
                        KeyHint::new("l", "expand log"),
                        KeyHint::new("Esc", "detach (continues in background)"),
                    ]
                } else {
                    vec![
                        KeyHint::new("l", "expand log"),
                        KeyHint::new("Esc", "cancel before write"),
                    ]
                }
            }
            WizardStep::Complete => vec![
                KeyHint::new("Tab", "switch action"),
                KeyHint::new("Enter", "choose action"),
            ],
        };
    }

    pub fn start_worker(&mut self) {
        self.pause_swayidle();
        let cancel = Arc::new(AtomicBool::new(false));
        self.worker_run = WorkerRun::Active {
            cancel: Some(cancel.clone()),
        };

        let mut conf = self.config.clone();

        if conf.dry_run {
            let dir = resolve_state_dir(conf.state_dir.as_deref());
            let _ = fs::remove_file(state_file_path_for(&dir));
            std::thread::spawn(move || {
                let _ = run_worker(conf, Some(cancel));
            });
            return;
        }

        conf.state_dir = Some("/run/hayato-install".to_string());
        let payload = ApplyRequest {
            secret_password: (!conf.password.is_empty()).then(|| conf.password.clone()),
            config: conf,
        };
        let Ok(json_bytes) = serde_json::to_vec(&payload) else {
            self.mark_spawn_failure("Failed to serialize installer configuration");
            return;
        };

        let exe = std::env::current_exe()
            .ok()
            .filter(|p| p.is_file())
            .unwrap_or_else(|| std::path::PathBuf::from("/usr/bin/hayato"));
        let exe_str = exe.to_string_lossy().into_owned();

        let has_inhibit =
            path_exists("/usr/bin/systemd-inhibit") || path_exists("/bin/systemd-inhibit");
        let has_setsid = path_exists("/usr/bin/setsid") || path_exists("/bin/setsid");

        let mut cmd = Command::new("sudo");
        cmd.arg("-n");
        if has_setsid {
            cmd.arg("setsid");
        }
        if has_inhibit {
            cmd.args([
                "systemd-inhibit",
                "--what=sleep:idle:handle-lid-switch",
                "--mode=block",
                "--why=Hayato installation in progress",
            ]);
        }
        cmd.args([
            &exe_str,
            "installer",
            "apply",
            "--stdin",
            "--state-dir",
            "/run/hayato-install",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(&json_bytes) {
                        self.mark_spawn_failure(&format!(
                            "Failed passing config to root worker: {e}"
                        ));
                        return;
                    }
                }
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                self.mark_spawn_failure(&format!(
                    "Failed invoking sudo -n setsid installer apply: {e}"
                ));
            }
        }
    }

    fn mark_spawn_failure(&mut self, msg: &str) {
        self.worker_state.status = WorkerPhase::Failed;
        self.worker_state.cancellable = false;
        self.worker_state.error = Some(msg.to_string());
        self.worker_state.log_lines.push(format!("ERROR: {msg}"));
        self.worker_run = WorkerRun::Idle;
        self.update_keybar();
    }
}

fn path_exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

impl Screen for InstallerScreen {
    fn keybar(&self) -> Option<&[KeyHint]> {
        Some(&self.keybar_hints)
    }

    fn phase(&self) -> Option<&'static str> {
        Some(match self.step {
            WizardStep::Welcome => "welcome",
            WizardStep::Localization => "localization",
            WizardStep::UserAccount => "user-account",
            WizardStep::DiskReview => "disk-review",
            WizardStep::Progress => match self.worker_state.status {
                WorkerPhase::Running => "progress-running",
                WorkerPhase::Success => "progress-success",
                WorkerPhase::Failed => "progress-failed",
            },
            WizardStep::Complete => "complete",
        })
    }

    fn needs_tick(&self) -> Option<Duration> {
        if self.step == WizardStep::Progress {
            Some(Duration::from_millis(100))
        } else if self.focused_input_active() {
            Some(Duration::from_millis(500))
        } else {
            None
        }
    }

    fn tick(&mut self) -> Outcome {
        if self.step == WizardStep::Progress {
            self.spinner_idx = (self.spinner_idx + 1) % BRAILLE_SPINNER.len();
            let prev_first = self.worker_state.first_write;
            let prev_status = self.worker_state.status;
            if let Some(state) = load_state() {
                self.worker_state = state;
                if self.worker_state.status == WorkerPhase::Success {
                    self.worker_run = WorkerRun::Idle;
                    self.resume_swayidle();
                    self.step = WizardStep::Complete;
                    self.update_keybar();
                } else if self.worker_state.first_write != prev_first
                    || self.worker_state.status != prev_status
                {
                    if self.worker_state.status == WorkerPhase::Failed {
                        self.worker_run = WorkerRun::Idle;
                        self.resume_swayidle();
                    }
                    self.update_keybar();
                }
            }
        } else if self.focused_input_active()
            && self.last_blink.elapsed() >= Duration::from_millis(500)
        {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = Instant::now();
        }
        Outcome::Continue
    }

    fn hit(&self, _area: Rect, x: u16, y: u16, design: &Design) -> Option<usize> {
        let hits = self.hits.borrow();
        let contains = |rect: Option<Rect>| -> bool {
            rect.is_some_and(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
        };
        match self.step {
            WizardStep::Welcome => contains(hits.welcome_next).then_some(1),
            WizardStep::Localization => {
                if contains(hits.tz_input) {
                    return Some(10);
                }
                if let Some(list_rect) = hits.tz_list {
                    if let Some(row) = self.tz_list.hit_row(list_rect, x, y, design) {
                        return Some(1000 + row);
                    }
                    if contains(Some(list_rect)) {
                        return Some(11);
                    }
                }
                if contains(hits.locale_rect) {
                    return Some(12);
                }
                if contains(hits.keymap_rect) {
                    return Some(13);
                }
                if contains(hits.keymap_test) {
                    return Some(14);
                }
                if contains(hits.loc_back) {
                    return Some(15);
                }
                if contains(hits.loc_next) {
                    return Some(16);
                }
                None
            }
            WizardStep::UserAccount => {
                for (i, r) in hits.user_inputs.iter().enumerate() {
                    if contains(*r) {
                        return Some(20 + i);
                    }
                }
                if contains(hits.user_ssh) {
                    return Some(25);
                }
                if contains(hits.user_back) {
                    return Some(26);
                }
                if contains(hits.user_next) {
                    return Some(27);
                }
                None
            }
            WizardStep::DiskReview => {
                if contains(hits.disk_target) {
                    return Some(30);
                }
                if contains(hits.disk_confirm) {
                    return Some(31);
                }
                if contains(hits.disk_cancel) {
                    return Some(32);
                }
                if contains(hits.disk_erase) {
                    return Some(33);
                }
                None
            }
            WizardStep::Progress => contains(hits.progress_log).then_some(40),
            WizardStep::Complete => {
                if contains(hits.complete_reboot) {
                    Some(50)
                } else if contains(hits.complete_exit) {
                    Some(51)
                } else {
                    None
                }
            }
        }
    }

    fn click(&mut self, hit_id: Option<usize>) -> Outcome {
        let Some(id) = hit_id else {
            return Outcome::Continue;
        };
        self.reset_blink();
        if id >= 1000 && self.step == WizardStep::Localization {
            let row = id - 1000;
            self.tz_list.set_cursor(row);
            self.loc_focus = LocFocus::TimezoneList;
            if let Some(selected) = self.tz_list.selected() {
                self.selected_tz = selected.label.clone();
                self.config.timezone = selected.label.clone();
            }
            return Outcome::Consumed;
        }
        match id {
            1 if self.step == WizardStep::Welcome => {
                if self.preflight.is_safe {
                    self.step = WizardStep::Localization;
                    self.update_keybar();
                    Outcome::Consumed
                } else {
                    Outcome::Done(hayato_ui::app::exit::ESCAPE)
                }
            }
            10 => {
                self.loc_focus = LocFocus::TimezoneInput;
                Outcome::Consumed
            }
            11 => {
                self.loc_focus = LocFocus::TimezoneList;
                Outcome::Consumed
            }
            12 => {
                self.loc_focus = LocFocus::Locale;
                self.locale_idx = (self.locale_idx + 1) % self.locales.len().max(1);
                self.config.locale = self.locales[self.locale_idx].to_string();
                Outcome::Consumed
            }
            13 => {
                self.loc_focus = LocFocus::Keymap;
                self.keymap_idx = (self.keymap_idx + 1) % self.keymaps.len().max(1);
                self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                Outcome::Consumed
            }
            14 => {
                self.loc_focus = LocFocus::KeymapTest;
                Outcome::Consumed
            }
            15 => {
                self.step = WizardStep::Welcome;
                self.update_keybar();
                Outcome::Consumed
            }
            16 => {
                self.config.timezone = self.selected_tz.clone();
                self.config.locale = self.locales[self.locale_idx].to_string();
                self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                self.step = WizardStep::UserAccount;
                self.update_keybar();
                Outcome::Consumed
            }
            20..=24 => {
                self.user_focus = match id {
                    20 => UserFocus::FullName,
                    21 => UserFocus::Username,
                    22 => UserFocus::Password,
                    23 => UserFocus::Confirm,
                    _ => UserFocus::Hostname,
                };
                Outcome::Consumed
            }
            25 => {
                self.user_focus = UserFocus::Ssh;
                self.regen_ssh = !self.regen_ssh;
                self.config.regenerate_ssh_keys = self.regen_ssh;
                Outcome::Consumed
            }
            26 => {
                self.step = WizardStep::Localization;
                self.update_keybar();
                Outcome::Consumed
            }
            27 => self.try_advance_user_account(),
            30 => {
                self.disk_focus = DiskFocus::Target;
                self.cycle_target(1);
                Outcome::Consumed
            }
            31 => {
                self.disk_focus = DiskFocus::Confirm;
                Outcome::Consumed
            }
            32 => {
                self.step = WizardStep::UserAccount;
                self.update_keybar();
                Outcome::Consumed
            }
            33 => {
                self.disk_focus = DiskFocus::Erase;
                let token = self.target_short().to_string();
                if validate_confirmation(self.disk_confirm_input.text(), &token) {
                    self.step = WizardStep::Progress;
                    self.update_keybar();
                    self.start_worker();
                } else {
                    self.disk_focus = DiskFocus::Confirm;
                }
                Outcome::Consumed
            }
            40 if self.step == WizardStep::Progress => {
                self.show_log_full = !self.show_log_full;
                Outcome::Consumed
            }
            50 if self.step == WizardStep::Complete => {
                self.complete_focus = CompleteFocus::Reboot;
                if !self.config.dry_run {
                    let _ = Command::new("systemctl").arg("reboot").spawn();
                }
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            51 if self.step == WizardStep::Complete => {
                self.complete_focus = CompleteFocus::Exit;
                Outcome::Done(hayato_ui::app::exit::OK)
            }
            _ => Outcome::Continue,
        }
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return Outcome::Continue;
        }
        self.reset_blink();

        match self.step {
            WizardStep::Welcome => match key.code {
                KeyCode::Esc => Outcome::Done(hayato_ui::app::exit::ESCAPE),
                KeyCode::Enter => {
                    if self.preflight.is_safe {
                        self.step = WizardStep::Localization;
                        self.update_keybar();
                        Outcome::Consumed
                    } else {
                        Outcome::Done(hayato_ui::app::exit::ESCAPE)
                    }
                }
                _ => Outcome::Continue,
            },

            WizardStep::Localization => self.handle_localization_key(key, design),
            WizardStep::UserAccount => self.handle_user_key(key),
            WizardStep::DiskReview => self.handle_disk_review_key(key),

            WizardStep::Progress => match key.code {
                KeyCode::Char('l') => {
                    self.show_log_full = !self.show_log_full;
                    Outcome::Consumed
                }
                KeyCode::Enter if self.worker_state.status == WorkerPhase::Failed => {
                    Outcome::Done(hayato_ui::app::exit::FAILED)
                }
                KeyCode::Esc => {
                    if !self.worker_state.first_write
                        && self.worker_state.cancellable
                        && let WorkerRun::Active {
                            cancel: Some(cancel),
                        } = &self.worker_run
                    {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    self.resume_swayidle();
                    Outcome::Done(hayato_ui::app::exit::ESCAPE)
                }
                _ => Outcome::Consumed,
            },

            WizardStep::Complete => match key.code {
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    self.complete_focus = self.complete_focus.toggle();
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    if self.complete_focus == CompleteFocus::Reboot && !self.config.dry_run {
                        let _ = Command::new("systemctl").arg("reboot").spawn();
                    }
                    Outcome::Done(hayato_ui::app::exit::OK)
                }
                KeyCode::Esc => Outcome::Done(hayato_ui::app::exit::OK),
                _ => Outcome::Continue,
            },
        }
    }

    fn draw(&mut self, ctx: &mut BodyCtx) {
        let area = ctx.area;
        let design = ctx.design;
        let buf = &mut *ctx.buf;

        if area.width < 50 || area.height < 15 {
            put(
                buf,
                area.x,
                area.y,
                "Terminal too small for installer wizard (min 80x24)",
                area.width,
                design.style(TextStyle::Dim),
            );
            return;
        }

        let badge = format!(" Step {} of 6 ", self.step as usize + 1);
        let badge_w = badge.chars().count() as u16;
        let x = put(
            buf,
            area.x + 2,
            area.y,
            &badge,
            area.width.saturating_sub(4),
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD),
        );
        let x = put(
            buf,
            x,
            area.y,
            &format!("  {}", self.step.title()),
            area.right().saturating_sub(x + 2),
            design.style(TextStyle::Title).add_modifier(Modifier::BOLD),
        );
        // Right-align concise step breadcrumb rail when viewport width allows.
        let rail = "1 Welcome \u{203a} 2 Locale \u{203a} 3 Account \u{203a} 4 Disk \u{203a} 5 Install \u{203a} 6 Done";
        let rail_w = rail.chars().count() as u16;
        let _ = badge_w;
        if x + 2 + rail_w <= area.right().saturating_sub(2) {
            let steps_short = [
                "1 Welcome",
                "2 Locale",
                "3 Account",
                "4 Disk",
                "5 Install",
                "6 Done",
            ];
            let mut rx = area.right().saturating_sub(2 + rail_w);
            for (idx, lbl) in steps_short.iter().enumerate() {
                if idx > 0 {
                    rx = put(
                        buf,
                        rx,
                        area.y,
                        " \u{203a} ",
                        3,
                        design.style(TextStyle::Dim),
                    );
                }
                let st = if idx == self.step as usize {
                    Style::default()
                        .fg(design.color(ColorRole::Accent))
                        .bg(design.color(ColorRole::Bg))
                        .add_modifier(Modifier::BOLD)
                } else if idx < self.step as usize {
                    design.style(TextStyle::Body)
                } else {
                    design.style(TextStyle::Dim)
                };
                rx = put(buf, rx, area.y, lbl, lbl.chars().count() as u16, st);
            }
        }

        let content_area = Rect {
            x: area.x + 1,
            y: area.y + 2,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(3),
        };

        let mut hits = InstallerHits::default();
        match self.step {
            WizardStep::Welcome => self.draw_welcome(content_area, buf, design, &mut hits),
            WizardStep::Localization => {
                self.draw_localization(content_area, buf, design, &mut hits);
            }
            WizardStep::UserAccount => self.draw_user_account(content_area, buf, design, &mut hits),
            WizardStep::DiskReview => self.draw_disk_review(content_area, buf, design, &mut hits),
            WizardStep::Progress => self.draw_progress(content_area, buf, design, &mut hits),
            WizardStep::Complete => self.draw_complete(content_area, buf, design, &mut hits),
        }
        *self.hits.borrow_mut() = hits;
    }
}

impl InstallerScreen {
    fn handle_localization_key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::Welcome;
                self.update_keybar();
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                self.loc_focus = if shift {
                    self.loc_focus.prev()
                } else {
                    self.loc_focus.next()
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.loc_focus = self.loc_focus.prev();
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.loc_focus {
            LocFocus::TimezoneInput => {
                if key.code == KeyCode::Down {
                    self.loc_focus = LocFocus::TimezoneList;
                    return Outcome::Consumed;
                }
                if key.code == KeyCode::Enter {
                    if let Some(row) = self.tz_list.selected() {
                        self.selected_tz = row.label.clone();
                        self.config.timezone = row.label.clone();
                    }
                    self.loc_focus = LocFocus::Locale;
                    return Outcome::Consumed;
                }
                self.tz_input.handle(key);
                self.tz_list.set_filter(self.tz_input.text());
                Outcome::Consumed
            }
            LocFocus::TimezoneList => {
                if key.code == KeyCode::Up && self.tz_list.cursor() == 0 {
                    self.loc_focus = LocFocus::TimezoneInput;
                    return Outcome::Consumed;
                }
                match self.tz_list.handle(key, 15, design) {
                    ListAction::Activated(_) => {
                        if let Some(row) = self.tz_list.selected() {
                            self.selected_tz = row.label.clone();
                            self.config.timezone = row.label.clone();
                        }
                        self.loc_focus = LocFocus::Locale;
                        Outcome::Consumed
                    }
                    ListAction::Ignored if key.code == KeyCode::Esc => {
                        self.loc_focus = LocFocus::TimezoneInput;
                        Outcome::Consumed
                    }
                    _ => Outcome::Consumed,
                }
            }
            LocFocus::Locale => {
                match key.code {
                    KeyCode::Left => {
                        self.locale_idx = self.locale_idx.saturating_sub(1);
                        self.config.locale = self.locales[self.locale_idx].to_string();
                    }
                    KeyCode::Right => {
                        self.locale_idx =
                            (self.locale_idx + 1).min(self.locales.len().saturating_sub(1));
                        self.config.locale = self.locales[self.locale_idx].to_string();
                    }
                    KeyCode::Up => self.loc_focus = LocFocus::TimezoneList,
                    KeyCode::Down | KeyCode::Enter => self.loc_focus = LocFocus::Keymap,
                    _ => {}
                }
                Outcome::Consumed
            }
            LocFocus::Keymap => {
                match key.code {
                    KeyCode::Left => {
                        self.keymap_idx = self.keymap_idx.saturating_sub(1);
                        self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                    }
                    KeyCode::Right => {
                        self.keymap_idx =
                            (self.keymap_idx + 1).min(self.keymaps.len().saturating_sub(1));
                        self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                    }
                    KeyCode::Up => self.loc_focus = LocFocus::Locale,
                    KeyCode::Down | KeyCode::Enter => self.loc_focus = LocFocus::KeymapTest,
                    _ => {}
                }
                Outcome::Consumed
            }
            LocFocus::KeymapTest => match key.code {
                KeyCode::Up => {
                    self.loc_focus = LocFocus::Keymap;
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Enter => {
                    self.loc_focus = LocFocus::Next;
                    Outcome::Consumed
                }
                _ => {
                    self.keymap_test.handle(key);
                    Outcome::Consumed
                }
            },
            LocFocus::Back => {
                match key.code {
                    KeyCode::Right => self.loc_focus = LocFocus::Next,
                    KeyCode::Up => self.loc_focus = LocFocus::KeymapTest,
                    KeyCode::Enter => {
                        self.step = WizardStep::Welcome;
                        self.update_keybar();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            LocFocus::Next => {
                match key.code {
                    KeyCode::Left => self.loc_focus = LocFocus::Back,
                    KeyCode::Up => self.loc_focus = LocFocus::KeymapTest,
                    KeyCode::Enter => {
                        self.config.timezone = self.selected_tz.clone();
                        self.config.locale = self.locales[self.locale_idx].to_string();
                        self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                        self.step = WizardStep::UserAccount;
                        self.update_keybar();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
        }
    }

    fn handle_user_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::Localization;
                self.update_keybar();
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                self.user_focus = if shift {
                    self.user_focus.prev()
                } else {
                    self.user_focus.next()
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.user_focus = self.user_focus.prev();
                return Outcome::Consumed;
            }
            _ => {}
        }

        if matches!(
            self.user_focus,
            UserFocus::FullName
                | UserFocus::Username
                | UserFocus::Password
                | UserFocus::Confirm
                | UserFocus::Hostname
        ) {
            match key.code {
                KeyCode::Down | KeyCode::Enter => {
                    self.user_focus = match self.user_focus {
                        UserFocus::Hostname => UserFocus::Ssh,
                        focus => focus.next(),
                    };
                    return Outcome::Consumed;
                }
                KeyCode::Up => {
                    self.user_focus = match self.user_focus {
                        UserFocus::FullName => UserFocus::FullName,
                        focus => focus.prev(),
                    };
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }

        match self.user_focus {
            UserFocus::FullName => {
                self.fullname_input.handle(key);
                self.config.fullname = self.fullname_input.text().to_string();
                Outcome::Consumed
            }
            UserFocus::Username => {
                self.username_input.handle(key);
                self.config.username = self.username_input.text().to_string();
                self.user_error = validate_username(&self.config.username).err();
                Outcome::Consumed
            }
            UserFocus::Password => {
                self.password_input.handle(key);
                self.config.password = self.password_input.text().to_string();
                self.user_error =
                    validate_password(&self.config.password, &self.config.password_confirm).err();
                Outcome::Consumed
            }
            UserFocus::Confirm => {
                self.confirm_input.handle(key);
                self.config.password_confirm = self.confirm_input.text().to_string();
                self.user_error =
                    validate_password(&self.config.password, &self.config.password_confirm).err();
                Outcome::Consumed
            }
            UserFocus::Hostname => {
                self.hostname_input.handle(key);
                self.config.hostname = self.hostname_input.text().to_string();
                self.user_error = validate_hostname(&self.config.hostname).err();
                Outcome::Consumed
            }
            UserFocus::Ssh => {
                match key.code {
                    KeyCode::Up => self.user_focus = UserFocus::Hostname,
                    KeyCode::Down => self.user_focus = UserFocus::Next,
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        self.regen_ssh = !self.regen_ssh;
                        self.config.regenerate_ssh_keys = self.regen_ssh;
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            UserFocus::Back => {
                match key.code {
                    KeyCode::Right => self.user_focus = UserFocus::Next,
                    KeyCode::Up => self.user_focus = UserFocus::Ssh,
                    KeyCode::Enter => {
                        self.step = WizardStep::Localization;
                        self.update_keybar();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            UserFocus::Next => {
                match key.code {
                    KeyCode::Left => self.user_focus = UserFocus::Back,
                    KeyCode::Up => self.user_focus = UserFocus::Ssh,
                    KeyCode::Enter => return self.try_advance_user_account(),
                    _ => {}
                }
                Outcome::Consumed
            }
        }
    }

    fn handle_disk_review_key(&mut self, key: KeyEvent) -> Outcome {
        let multi = self.preflight.candidates.len() > 1;
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::UserAccount;
                self.update_keybar();
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                self.disk_focus = if shift {
                    self.disk_focus.prev(multi)
                } else {
                    self.disk_focus.next(multi)
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.disk_focus = self.disk_focus.prev(multi);
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.disk_focus {
            DiskFocus::Cancel => {
                match key.code {
                    KeyCode::Right => self.disk_focus = DiskFocus::Erase,
                    KeyCode::Up => self.disk_focus = DiskFocus::Confirm,
                    KeyCode::Enter => {
                        self.step = WizardStep::UserAccount;
                        self.update_keybar();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            DiskFocus::Confirm => {
                match key.code {
                    KeyCode::Up if multi => {
                        self.disk_focus = DiskFocus::Target;
                        return Outcome::Consumed;
                    }
                    KeyCode::Down => {
                        self.disk_focus = DiskFocus::Cancel;
                        return Outcome::Consumed;
                    }
                    KeyCode::Enter => {
                        let token = self.target_short().to_string();
                        if validate_confirmation(self.disk_confirm_input.text(), &token) {
                            self.disk_focus = DiskFocus::Erase;
                        }
                        return Outcome::Consumed;
                    }
                    _ => {}
                }
                self.disk_confirm_input.handle(key);
                Outcome::Consumed
            }
            DiskFocus::Erase => {
                match key.code {
                    KeyCode::Left => self.disk_focus = DiskFocus::Cancel,
                    KeyCode::Up => self.disk_focus = DiskFocus::Confirm,
                    KeyCode::Enter => {
                        let token = self.target_short().to_string();
                        if validate_confirmation(self.disk_confirm_input.text(), &token) {
                            self.step = WizardStep::Progress;
                            self.update_keybar();
                            self.start_worker();
                        } else {
                            self.disk_focus = DiskFocus::Confirm;
                        }
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            DiskFocus::Target => {
                match key.code {
                    KeyCode::Left => self.cycle_target(-1),
                    KeyCode::Right | KeyCode::Enter => self.cycle_target(1),
                    KeyCode::Down => self.disk_focus = DiskFocus::Confirm,
                    _ => {}
                }
                Outcome::Consumed
            }
        }
    }

    fn draw_welcome(
        &self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + 1,
                y: area.y,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(1),
            },
            design,
            Some("Hayato Full-System Installer"),
            true,
        );

        let mut y = card.y + 1;
        put(
            buf,
            card.x + 2,
            y,
            &format!(
                "Welcome to the Hayato full-system installer for {} ({}).",
                self.preflight.device_info.name,
                self.preflight.device_info.boot_type.label()
            ),
            card.width.saturating_sub(4),
            design.style(TextStyle::Body).add_modifier(Modifier::BOLD),
        );
        y += 2;

        put(
            buf,
            card.x + 2,
            y,
            "System Hardware & Boot Media:",
            card.width.saturating_sub(4),
            design
                .style(TextStyle::Dim)
                .add_modifier(Modifier::UNDERLINED),
        );
        y += 1;
        for line in [
            format!("• Platform / SoC:  {}", self.preflight.soc_model),
            format!(
                "• Boot Backend:    {}",
                self.preflight.device_info.boot_type.label()
            ),
            format!("• Total Memory:    {}", self.preflight.total_ram),
            format!("• Power Status:    {}", self.preflight.power_source),
            format!("• Running Root:    {}", self.preflight.root_source),
            format!(
                "• Target Disk:     {} ({})",
                self.config.target_disk,
                self.target_size()
            ),
        ] {
            put(
                buf,
                card.x + 4,
                y,
                &line,
                card.width.saturating_sub(8),
                design.style(TextStyle::Body),
            );
            y += 1;
        }
        y += 1;

        if !self.preflight.is_safe {
            let danger_area = Rect {
                x: card.x + 2,
                y,
                width: card.width.saturating_sub(4),
                height: 5,
            };
            fill_rect(
                buf,
                danger_area,
                Style::default().bg(design.color(ColorRole::SurfaceAlt)),
            );
            put(
                buf,
                danger_area.x + 2,
                danger_area.y + 1,
                &format!(
                    "CRITICAL SAFETY REFUSAL: System already running from target ({})!",
                    self.config.target_disk
                ),
                danger_area.width.saturating_sub(4),
                Style::default()
                    .fg(design.color(ColorRole::Danger))
                    .add_modifier(Modifier::BOLD),
            );
            put(
                buf,
                danger_area.x + 2,
                danger_area.y + 2,
                "Overwriting the active running disk is blocked to prevent filesystem destruction.",
                danger_area.width.saturating_sub(4),
                design.style(TextStyle::Body),
            );
            put(
                buf,
                danger_area.x + 2,
                danger_area.y + 3,
                "Boot Hayato from removable USB or microSD media to install to internal storage.",
                danger_area.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
            let exit_y = (danger_area.bottom() + 1).min(card.bottom().saturating_sub(1));
            if exit_y >= danger_area.bottom() && exit_y < card.bottom() {
                let lbl = " [ Exit Installer ] ";
                let w = (lbl.chars().count() as u16).min(card.width.saturating_sub(4));
                let rect = Rect::new(card.x + 2, exit_y, w, 1);
                put(
                    buf,
                    rect.x,
                    rect.y,
                    lbl,
                    rect.width,
                    design.style(TextStyle::Key).add_modifier(Modifier::BOLD),
                );
                hits.welcome_next = Some(rect);
            }
        } else {
            let badge_h = if card.height >= 20 { 3 } else { 1 };
            let badge_area = Rect {
                x: card.x + 2,
                y,
                width: card.width.saturating_sub(4),
                height: badge_h,
            };
            fill_rect(
                buf,
                badge_area,
                Style::default().bg(design.color(ColorRole::Surface)),
            );
            let badge_msg = match self.preflight.device_info.boot_type {
                BootType::Depthcharge => {
                    "✓ Pre-flight checks passed: internal target ready (Depthcharge GPT)."
                }
                BootType::Uefi => "✓ Pre-flight checks passed: target storage ready (UEFI GPT).",
                BootType::Extlinux => {
                    "✓ Pre-flight checks passed: target storage ready (U-Boot extlinux GPT)."
                }
            };
            let badge_y = if badge_h >= 3 {
                badge_area.y + 1
            } else {
                badge_area.y
            };
            put(
                buf,
                badge_area.x + 2,
                badge_y,
                badge_msg,
                badge_area.width.saturating_sub(4),
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .bg(design.color(ColorRole::Surface))
                    .add_modifier(Modifier::BOLD),
            );
            y += badge_h + 1;

            let btn_y = card.bottom().saturating_sub(1);
            let list_bottom = btn_y.saturating_sub(1);
            if y + 1 < list_bottom {
                put(
                    buf,
                    card.x + 2,
                    y,
                    "Verified Installer Utilities:",
                    card.width.saturating_sub(4),
                    design
                        .style(TextStyle::Dim)
                        .add_modifier(Modifier::UNDERLINED),
                );
                y += 1;
                let mut col1_y = y;
                let mut col2_y = y;
                for (idx, tool) in self.preflight.tools.iter().enumerate() {
                    if col1_y >= list_bottom || col2_y >= list_bottom {
                        break;
                    }
                    let status_icon = if tool.available { "✓" } else { "✗" };
                    let text = format!("{status_icon} {:<14} ({})", tool.name, tool.description);
                    if idx % 2 == 0 {
                        put(
                            buf,
                            card.x + 4,
                            col1_y,
                            &text,
                            (card.width / 2).saturating_sub(4),
                            design.style(TextStyle::Body),
                        );
                        col1_y += 1;
                    } else {
                        put(
                            buf,
                            card.x + card.width / 2 + 2,
                            col2_y,
                            &text,
                            (card.width / 2).saturating_sub(4),
                            design.style(TextStyle::Body),
                        );
                        col2_y += 1;
                    }
                }
            }
            if btn_y > y && btn_y < card.bottom() {
                let next_lbl = " [ Continue: Localization > ] ";
                let w = (next_lbl.chars().count() as u16).min(card.width.saturating_sub(4));
                let rect = Rect::new(card.x + 2, btn_y, w, 1);
                put(
                    buf,
                    rect.x,
                    rect.y,
                    next_lbl,
                    rect.width,
                    design.style(TextStyle::Key).add_modifier(Modifier::BOLD),
                );
                hits.welcome_next = Some(rect);
            }
        }
    }

    fn draw_localization(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let left_w = area.width / 2;
        let right_w = area.width.saturating_sub(left_w);

        let tz_card = draw_card(
            buf,
            Rect {
                x: area.x,
                y: area.y,
                width: left_w.saturating_sub(1),
                height: area.height,
            },
            design,
            Some("Timezone Selection"),
            matches!(
                self.loc_focus,
                LocFocus::TimezoneInput | LocFocus::TimezoneList
            ),
        );

        let filter_label_style = if self.loc_focus == LocFocus::TimezoneInput {
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Dim)
        };
        put(
            buf,
            tz_card.x + 1,
            tz_card.y,
            "Filter Timezones:",
            tz_card.width.saturating_sub(2),
            filter_label_style,
        );
        let tz_input_rect = Rect {
            x: tz_card.x + 1,
            y: tz_card.y + 1,
            width: tz_card.width.saturating_sub(2),
            height: 1,
        };
        self.tz_input.render_field(
            tz_input_rect,
            buf,
            design,
            "Type to filter (e.g. America, Europe)...",
            self.loc_focus == LocFocus::TimezoneInput,
            self.cursor_blink_on,
            ColorRole::Surface,
            TextStyle::Body,
        );
        hits.tz_input = Some(tz_input_rect);

        let tz_list_rect = Rect {
            x: tz_card.x + 1,
            y: tz_card.y + 3,
            width: tz_card.width.saturating_sub(2),
            height: tz_card.height.saturating_sub(4),
        };
        self.tz_list.render(
            tz_list_rect,
            buf,
            design,
            self.loc_focus == LocFocus::TimezoneList,
        );
        hits.tz_list = Some(tz_list_rect);

        let right_card = draw_card(
            buf,
            Rect {
                x: area.x + left_w,
                y: area.y,
                width: right_w,
                height: area.height,
            },
            design,
            Some("Locale & Keyboard Setup"),
            matches!(
                self.loc_focus,
                LocFocus::Locale
                    | LocFocus::Keymap
                    | LocFocus::KeymapTest
                    | LocFocus::Back
                    | LocFocus::Next
            ),
        );

        let compact = right_card.height < 18;
        let mut y = right_card.y + if compact { 0 } else { 1 };
        put(
            buf,
            right_card.x + 2,
            y,
            &format!("Selected Timezone:  {}", self.selected_tz),
            right_card.width.saturating_sub(4),
            design.style(TextStyle::Body).add_modifier(Modifier::BOLD),
        );
        y += 2;

        let loc_style = if self.loc_focus == LocFocus::Locale {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let loc_rect = Rect::new(right_card.x + 2, y, right_card.width.saturating_sub(4), 1);
        put(
            buf,
            loc_rect.x,
            loc_rect.y,
            &format!("System Locale:  < {} >", self.locales[self.locale_idx]),
            loc_rect.width,
            loc_style,
        );
        hits.locale_rect = Some(loc_rect);
        if !compact {
            put(
                buf,
                right_card.x + 2,
                y + 1,
                "(Left/Right to switch locale)",
                right_card.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
            y += 3;
        } else {
            y += 2;
        }

        let kbd_style = if self.loc_focus == LocFocus::Keymap {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let kbd_rect = Rect::new(right_card.x + 2, y, right_card.width.saturating_sub(4), 1);
        put(
            buf,
            kbd_rect.x,
            kbd_rect.y,
            &format!("Keyboard Layout:  < {} >", self.keymaps[self.keymap_idx]),
            kbd_rect.width,
            kbd_style,
        );
        hits.keymap_rect = Some(kbd_rect);
        if !compact {
            put(
                buf,
                right_card.x + 2,
                y + 1,
                "(Left/Right to switch keymap)",
                right_card.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
            y += 3;
        } else {
            y += 2;
        }

        let test_label_style = if self.loc_focus == LocFocus::KeymapTest {
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Dim)
        };
        put(
            buf,
            right_card.x + 2,
            y,
            "Interactive Keyboard Test (verify layout):",
            right_card.width.saturating_sub(4),
            test_label_style,
        );
        y += 1;
        let test_rect = Rect {
            x: right_card.x + 2,
            y,
            width: right_card.width.saturating_sub(4),
            height: 1,
        };
        self.keymap_test.render_field(
            test_rect,
            buf,
            design,
            "Type test keys: @ # $ % & / ( ) = ? ~",
            self.loc_focus == LocFocus::KeymapTest,
            self.cursor_blink_on,
            ColorRole::Surface,
            TextStyle::Body,
        );
        hits.keymap_test = Some(test_rect);
        y += if compact { 2 } else { 3 };
        let btn_y = y.min(right_card.bottom().saturating_sub(1));

        let back_style = if self.loc_focus == LocFocus::Back {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let next_style = if self.loc_focus == LocFocus::Next {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };

        let back_lbl = "[ < Back ]";
        let back_w = back_lbl.chars().count() as u16;
        let back_x = right_card.x + 2;
        put(buf, back_x, btn_y, back_lbl, back_w, back_style);
        hits.loc_back = Some(Rect::new(back_x, btn_y, back_w, 1));

        let next_lbl = if right_card.width >= 40 {
            "[ Next: User Account > ]"
        } else {
            "[ Next: User > ]"
        };
        let next_w = next_lbl.chars().count() as u16;
        let next_x = (back_x + back_w + 2).min(right_card.right().saturating_sub(next_w + 1));
        put(buf, next_x, btn_y, next_lbl, next_w, next_style);
        hits.loc_next = Some(Rect::new(next_x, btn_y, next_w, 1));
    }

    fn draw_user_account(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let card_rect = if area.width < 100 {
            Rect {
                x: area.x + 1,
                y: area.y,
                width: area.width.saturating_sub(2),
                height: area.height,
            }
        } else {
            let w = ((area.width * 2) / 3).clamp(72, 96);
            Rect {
                x: area.x + area.width.saturating_sub(w) / 2,
                y: area.y,
                width: w,
                height: area.height,
            }
        };
        let card = draw_card(
            buf,
            card_rect,
            design,
            Some("Primary User Account & Machine Identity"),
            true,
        );

        let compact = card.height < 21;
        let mut y = card.y + if compact { 0 } else { 1 };
        let pw_typed =
            !self.password_input.text().is_empty() && !self.confirm_input.text().is_empty();
        let pw_match = self.password_input.text() == self.confirm_input.text();

        let fields = [
            (
                "Full Name (Display Name):",
                "Full Name:",
                "Your full name (e.g. Bruno)",
                UserFocus::FullName,
            ),
            (
                "POSIX Username (Login & Sudo User):",
                "POSIX Username:",
                "lowercase login name (e.g. brunux)",
                UserFocus::Username,
            ),
            (
                "Account Password:",
                "Account Password:",
                "enter password",
                UserFocus::Password,
            ),
            (
                "Confirm Password:",
                "Confirm Password:",
                "re-type password",
                UserFocus::Confirm,
            ),
            (
                "System Hostname:",
                "System Hostname:",
                "network hostname (default hayato)",
                UserFocus::Hostname,
            ),
        ];

        for (wide_label, short_label, ph, focus) in fields {
            let focused = self.user_focus == focus;
            let idx = match focus {
                UserFocus::FullName => 0,
                UserFocus::Username => 1,
                UserFocus::Password => 2,
                UserFocus::Confirm => 3,
                UserFocus::Hostname => 4,
                UserFocus::Ssh | UserFocus::Back | UserFocus::Next => continue,
            };
            let label_style = if focused {
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Dim)
            };
            let rect = if compact {
                let lx = card.x + 2;
                let lw = 19u16.min(card.width / 3);
                put_trunc(buf, lx, y, short_label, lw, label_style);
                let ix = lx + lw + 1;
                let iw = card.width.saturating_sub(lw + 5);
                let r = Rect::new(ix, y, iw, 1);
                if focus == UserFocus::Confirm && pw_typed && card.height >= 16 {
                    let (badge, role) = if pw_match {
                        ("✓ Passwords match", ColorRole::Ok)
                    } else {
                        ("✗ Passwords do not match", ColorRole::Danger)
                    };
                    put(
                        buf,
                        ix,
                        y + 1,
                        badge,
                        iw,
                        Style::default()
                            .fg(design.color(role))
                            .add_modifier(Modifier::BOLD),
                    );
                }
                y += if card.height >= 16 { 2 } else { 1 };
                r
            } else {
                let lx = card.x + 4;
                let lw = card.width.saturating_sub(8);
                put_trunc(buf, lx, y, wide_label, lw, label_style);
                if focus == UserFocus::Confirm && pw_typed {
                    let (badge, role) = if pw_match {
                        ("✓ Passwords match", ColorRole::Ok)
                    } else {
                        ("✗ Passwords do not match", ColorRole::Danger)
                    };
                    let bw = badge.chars().count() as u16;
                    if lx + wide_label.chars().count() as u16 + 2 + bw <= card.right() - 4 {
                        put(
                            buf,
                            card.right().saturating_sub(4 + bw),
                            y,
                            badge,
                            bw,
                            Style::default()
                                .fg(design.color(role))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
                y += 1;
                let r = Rect::new(lx, y, lw, 1);
                y += 2;
                r
            };

            let input = match idx {
                0 => &self.fullname_input,
                1 => &self.username_input,
                2 => &self.password_input,
                3 => &self.confirm_input,
                _ => &self.hostname_input,
            };
            input.render_field(
                rect,
                buf,
                design,
                ph,
                focused,
                self.cursor_blink_on,
                ColorRole::Surface,
                TextStyle::Body,
            );
            hits.user_inputs[idx] = Some(rect);
        }

        if compact && card.height < 16 {
            if pw_typed && y + 3 < card.bottom() {
                let (badge, role) = if pw_match {
                    ("✓ Passwords match", ColorRole::Ok)
                } else {
                    ("✗ Passwords do not match", ColorRole::Danger)
                };
                put(
                    buf,
                    card.x + 2,
                    y,
                    badge,
                    card.width.saturating_sub(4),
                    Style::default()
                        .fg(design.color(role))
                        .add_modifier(Modifier::BOLD),
                );
            }
            y += 1;
        }

        let lx = if compact { card.x + 2 } else { card.x + 4 };
        let lw = if compact {
            card.width.saturating_sub(4)
        } else {
            card.width.saturating_sub(8)
        };
        let check_mark = if self.regen_ssh { "[X]" } else { "[ ]" };
        let ssh_style = if self.user_focus == UserFocus::Ssh {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let ssh_text = format!("{check_mark} Generate fresh SSH host keys on first boot");
        put_trunc(buf, lx, y, &ssh_text, lw, ssh_style);
        hits.user_ssh = Some(Rect::new(lx, y, lw.min(ssh_text.chars().count() as u16), 1));
        y += if compact { 1 } else { 2 };

        if let Some(err) = self.user_error {
            if y < card.bottom().saturating_sub(1) {
                put_trunc(
                    buf,
                    lx,
                    y,
                    &format!("⚠ Validation Error: {err}"),
                    lw,
                    Style::default()
                        .fg(design.color(ColorRole::Danger))
                        .add_modifier(Modifier::BOLD),
                );
                y += if compact { 1 } else { 2 };
            }
        }

        let btn_y = if compact {
            card.bottom().saturating_sub(1)
        } else {
            y.min(card.bottom().saturating_sub(1))
        };
        if btn_y < card.bottom() {
            let back_style = if self.user_focus == UserFocus::Back {
                design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Body)
            };
            let next_style = if self.user_focus == UserFocus::Next {
                design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
            } else {
                design.style(TextStyle::Body)
            };

            let back_lbl = "[ < Back ]";
            let back_w = back_lbl.chars().count() as u16;
            put(buf, lx, btn_y, back_lbl, back_w, back_style);
            hits.user_back = Some(Rect::new(lx, btn_y, back_w, 1));

            let next_lbl = "[ Next: Review & Confirm > ]";
            let next_w = next_lbl.chars().count() as u16;
            let next_x = (lx + back_w + 4).min(card.right().saturating_sub(next_w + 2));
            put(buf, next_x, btn_y, next_lbl, next_w, next_style);
            hits.user_next = Some(Rect::new(next_x, btn_y, next_w, 1));
        }
    }

    fn draw_disk_review(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + if area.width < 90 { 0 } else { 2 },
                y: area.y,
                width: area
                    .width
                    .saturating_sub(if area.width < 90 { 0 } else { 4 }),
                height: area.height,
            },
            design,
            Some("Target Storage & Partition Review"),
            true,
        );

        let compact = card.height < 21 || card.width < 88;
        let mut y = card.y + if compact { 0 } else { 1 };
        let selector_suffix = if self.preflight.candidates.len() > 1 {
            "  [< Left / Right >]"
        } else {
            ""
        };
        let target_style = if self.disk_focus == DiskFocus::Target {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body).add_modifier(Modifier::BOLD)
        };
        let target_rect = Rect::new(card.x + 2, y, card.width.saturating_sub(4), 1);
        put(
            buf,
            target_rect.x,
            target_rect.y,
            &format!(
                "Target Storage Device: {} ({}){selector_suffix}",
                self.config.target_disk,
                self.target_size()
            ),
            target_rect.width,
            target_style,
        );
        if self.preflight.candidates.len() > 1 {
            hits.disk_target = Some(target_rect);
        }
        y += if compact { 1 } else { 2 };

        y = self.draw_partition_diagram(card.x, y, card.width, compact, buf, design);
        y += if compact { 0 } else { 1 };

        let danger_h = if compact { 2 } else { 4 };
        let danger_box = Rect {
            x: card.x + 2,
            y,
            width: card.width.saturating_sub(4),
            height: danger_h,
        };
        fill_rect(
            buf,
            danger_box,
            Style::default().bg(design.color(ColorRole::Surface)),
        );
        let warn_y = if compact {
            danger_box.y
        } else {
            danger_box.y + 1
        };
        let warn_headline = if compact {
            "⚠ WARNING: ALL DATA ON TARGET DRIVE WILL BE ERASED!"
        } else {
            "⚠ WARNING: ALL EXISTING DATA ON TARGET DRIVE WILL BE PERMANENTLY ERASED!"
        };
        let warn_sub = if compact {
            format!(
                "Destroys all existing partitions on {}.",
                self.config.target_disk
            )
        } else {
            format!(
                "This action destroys all existing partitions and data on {}.",
                self.config.target_disk
            )
        };
        put_trunc(
            buf,
            danger_box.x + 1,
            warn_y,
            warn_headline,
            danger_box.width.saturating_sub(2),
            Style::default()
                .fg(design.color(ColorRole::Danger))
                .bg(design.color(ColorRole::Surface))
                .add_modifier(Modifier::BOLD),
        );
        put_trunc(
            buf,
            danger_box.x + 1,
            warn_y + 1,
            &warn_sub,
            danger_box.width.saturating_sub(2),
            design
                .style(TextStyle::Body)
                .bg(design.color(ColorRole::Surface)),
        );
        y += danger_h + 1;

        let short = self.target_short().to_string();
        let confirmed = validate_confirmation(self.disk_confirm_input.text(), &short);
        let confirm_focused = self.disk_focus == DiskFocus::Confirm;
        let prompt_label = format!("Type \"{short}\" to confirm disk erasure:");
        let prompt_style = if confirm_focused {
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Dim)
        };
        put(
            buf,
            card.x + 2,
            y,
            &prompt_label,
            card.width.saturating_sub(4),
            prompt_style,
        );
        y += 1;
        let confirm_w = 32u16.min(card.width.saturating_sub(18));
        let confirm_box = Rect {
            x: card.x + 2,
            y,
            width: confirm_w,
            height: 1,
        };
        self.disk_confirm_input.render_field(
            confirm_box,
            buf,
            design,
            &format!("type {short}"),
            confirm_focused,
            self.cursor_blink_on,
            ColorRole::Surface,
            TextStyle::Body,
        );
        hits.disk_confirm = Some(confirm_box);

        if confirmed && confirm_box.right() + 14 <= card.right() {
            put(
                buf,
                confirm_box.right() + 2,
                y,
                "✓ Confirmed",
                11,
                Style::default()
                    .fg(design.color(ColorRole::Ok))
                    .add_modifier(Modifier::BOLD),
            );
        }
        y += 2;
        let btn_y = y.min(card.bottom().saturating_sub(1));

        let cancel_style = if self.disk_focus == DiskFocus::Cancel {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let erase_style = if !confirmed {
            design.style(TextStyle::Dim)
        } else if self.disk_focus == DiskFocus::Erase {
            Style::default()
                .bg(design.color(ColorRole::Danger))
                .fg(design.color(ColorRole::Bg))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(design.color(ColorRole::Danger))
                .add_modifier(Modifier::BOLD)
        };

        if btn_y < card.bottom() {
            let cancel_lbl = "[ Cancel / Back ]";
            let cancel_w = cancel_lbl.chars().count() as u16;
            put(buf, card.x + 2, btn_y, cancel_lbl, cancel_w, cancel_style);
            hits.disk_cancel = Some(Rect::new(card.x + 2, btn_y, cancel_w, 1));

            let erase_lbl = "[ Erase Disk & Install Hayato ]";
            let erase_w = erase_lbl.chars().count() as u16;
            let erase_x = card.x + 22;
            put(buf, erase_x, btn_y, erase_lbl, erase_w, erase_style);
            hits.disk_erase = Some(Rect::new(erase_x, btn_y, erase_w, 1));
        }
    }

    fn draw_partition_diagram(
        &self,
        card_x: u16,
        mut y: u16,
        card_width: u16,
        compact: bool,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
    ) -> u16 {
        let info = &self.preflight.device_info;
        let header = match info.boot_type {
            BootType::Depthcharge => "Target ChromeOS GPT Partition Layout:",
            BootType::Uefi => "Target UEFI GPT Partition Layout:",
            BootType::Extlinux => "Target U-Boot GPT Partition Layout:",
        };
        put(
            buf,
            card_x + 2,
            y,
            header,
            card_width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 1;

        let kpart_mib = info.cgpt_kpart_size / 2048;
        let bullets: Vec<String> = match info.boot_type {
            BootType::Depthcharge => vec![
                format!("• p1: pmOS_kernel  ({kpart_mib} MiB)   ChromeOS signed FIT kernel"),
                format!(
                    "• p2: pmOS_boot    ({} MiB)   Boot filesystem ({}, /boot)",
                    info.boot_size_mib, info.boot_filesystem
                ),
                "• p3: pmOS_root    (Remainder) Debian arm64 root filesystem (ext4, /)".into(),
            ],
            BootType::Uefi => vec![
                format!(
                    "• p1: hayato_efi   ({} MiB)   EFI System Partition (FAT32, /boot/efi)",
                    info.boot_size_mib
                ),
                "• p2: hayato_root  (Remainder) Debian arm64 root filesystem (ext4, /)".into(),
            ],
            BootType::Extlinux => vec![
                format!(
                    "• p1: hayato_boot  ({} MiB)   Boot partition ({}, extlinux + DTBs)",
                    info.boot_size_mib, info.boot_filesystem
                ),
                "• p2: hayato_root  (Remainder) Debian arm64 root filesystem (ext4, /)".into(),
            ],
        };

        if compact {
            for b in bullets {
                put_trunc(
                    buf,
                    card_x + 3,
                    y,
                    &b,
                    card_width.saturating_sub(6),
                    design.style(TextStyle::Body),
                );
                y += 1;
            }
            return y;
        }

        let kpart_cell = format!("{kpart_mib} MiB");
        let boot_cell = format!("{} MiB", info.boot_size_mib);
        let box_rows: [String; 4] = match info.boot_type {
            BootType::Depthcharge => [
                "┌─────────────────┬───────────────────┬──────────────────────────────────────┐"
                    .into(),
                "│ p1: pmOS_kernel │   p2: pmOS_boot   │             p3: pmOS_root            │"
                    .into(),
                format!(
                    "│ {kpart_cell:^15} │ {boot_cell:^17} │          remaining storage           │"
                ),
                "└─────────────────┴───────────────────┴──────────────────────────────────────┘"
                    .into(),
            ],
            BootType::Uefi => {
                let uefi_cell = format!("{boot_cell} (FAT32)");
                [
                    "┌───────────────────────────┬────────────────────────────────────────────────┐".into(),
                    "│      p1: hayato_efi       │                p2: hayato_root                 │".into(),
                    format!("│ {uefi_cell:^25} │               remaining (ext4)                 │"),
                    "└───────────────────────────┴────────────────────────────────────────────────┘".into(),
                ]
            }
            BootType::Extlinux => {
                let ext_cell = format!("{boot_cell} ({})", info.boot_filesystem);
                [
                    "┌───────────────────────────┬────────────────────────────────────────────────┐".into(),
                    "│     p1: hayato_boot       │                p2: hayato_root                 │".into(),
                    format!("│ {ext_cell:^25} │               remaining (ext4)                 │"),
                    "└───────────────────────────┴────────────────────────────────────────────────┘".into(),
                ]
            }
        };

        for (idx, row) in box_rows.iter().enumerate() {
            let st = if idx == 2 {
                design.style(TextStyle::Dim)
            } else {
                design.style(TextStyle::Body)
            };
            put_trunc(buf, card_x + 4, y, row, card_width.saturating_sub(8), st);
            y += 1;
        }
        for b in bullets {
            put_trunc(
                buf,
                card_x + 4,
                y,
                &b,
                card_width.saturating_sub(8),
                design.style(TextStyle::Body),
            );
            y += 1;
        }
        y
    }

    fn draw_progress(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + 1,
                y: area.y,
                width: area.width.saturating_sub(2),
                height: area.height,
            },
            design,
            Some("Installation Progress"),
            true,
        );

        let mut y = card.y + 1;
        let pct = self.worker_state.percent as f64 / 100.0;
        // The gauge draws the percentage itself (`put_percent`); the label is
        // the phase only, or the number appears twice beside the bar.
        let gauge_label = self.worker_state.phase_name.clone();
        draw_gauge(
            buf,
            Rect {
                x: card.x + 2,
                y,
                width: card.width.saturating_sub(4),
                height: 1,
            },
            design,
            pct,
            &gauge_label,
        );
        y += 2;

        if !self.show_log_full {
            put(
                buf,
                card.x + 2,
                y,
                "Installation Sequence:",
                card.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
            y += 1;

            let current_spinner = BRAILLE_SPINNER[self.spinner_idx % BRAILLE_SPINNER.len()];
            let boot_type = self.preflight.device_info.boot_type;

            for idx in 0..PHASES.len() {
                let label = phase_label(idx, boot_type);
                let (icon, style) = if idx < self.worker_state.phase_index {
                    ("✓", Style::default().fg(design.color(ColorRole::Accent)))
                } else if idx == self.worker_state.phase_index {
                    if self.worker_state.status == WorkerPhase::Failed {
                        ("✗", Style::default().fg(design.color(ColorRole::Danger)))
                    } else if self.worker_state.status == WorkerPhase::Success {
                        ("✓", Style::default().fg(design.color(ColorRole::Accent)))
                    } else {
                        (current_spinner, design.style(TextStyle::Key))
                    }
                } else {
                    ("○", design.style(TextStyle::Dim))
                };

                put(
                    buf,
                    card.x + 4,
                    y,
                    &format!("[{icon}] {}. {label}", idx + 1),
                    card.width.saturating_sub(8),
                    style,
                );
                y += 1;
            }
            y += 1;
        }

        if let Some(ref err) = self.worker_state.error {
            put_trunc(
                buf,
                card.x + 2,
                y,
                &format!("⚠ Installation failed: {err}"),
                card.width.saturating_sub(4),
                Style::default()
                    .fg(design.color(ColorRole::Danger))
                    .add_modifier(Modifier::BOLD),
            );
            y += 2;
        }

        put(
            buf,
            card.x + 2,
            y,
            "Live Installation Log:",
            card.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 1;

        let log_height = card.bottom().saturating_sub(y + 1);
        if log_height == 0 {
            return;
        }
        let log_area = Rect {
            x: card.x + 2,
            y,
            width: card.width.saturating_sub(4),
            height: log_height,
        };
        hits.progress_log = Some(log_area);
        fill_rect(
            buf,
            log_area,
            Style::default().bg(design.color(ColorRole::Surface)),
        );

        let tail = if self.worker_state.log_lines.len() > log_area.height as usize {
            &self.worker_state.log_lines
                [self.worker_state.log_lines.len() - log_area.height as usize..]
        } else {
            &self.worker_state.log_lines[..]
        };

        for (line_idx, line) in tail.iter().enumerate() {
            put_trunc(
                buf,
                log_area.x + 1,
                log_area.y + line_idx as u16,
                line,
                log_area.width.saturating_sub(2),
                design.style(TextStyle::Dim),
            );
        }
    }

    fn draw_complete(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
        hits: &mut InstallerHits,
    ) {
        let compact = area.height < 21 || area.width < 96;
        let card = draw_card(
            buf,
            Rect {
                x: area.x + if compact { 1 } else { area.width / 8 },
                y: area.y + if compact { 0 } else { 1 },
                width: if compact {
                    area.width.saturating_sub(2)
                } else {
                    (area.width * 3) / 4
                },
                height: area.height.saturating_sub(if compact { 0 } else { 2 }),
            },
            design,
            Some("Installation Finished"),
            true,
        );

        let mut y = card.y + if compact { 0 } else { 2 };
        put(
            buf,
            card.x + 4,
            y,
            "✓ Hayato Successfully Installed!",
            card.width.saturating_sub(8),
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
        );
        y += if compact { 1 } else { 2 };

        put_trunc(
            buf,
            card.x + 4,
            y,
            &format!(
                "Target storage ({}, {}) is now prepared and bootable.",
                self.config.target_disk,
                self.preflight.device_info.boot_type.label()
            ),
            card.width.saturating_sub(8),
            design.style(TextStyle::Body),
        );
        y += 1;
        put_trunc(
            buf,
            card.x + 4,
            y,
            &format!(
                "Primary user: {} · Hostname: {} · Timezone: {} · Locale: {}",
                self.config.username,
                self.config.hostname,
                self.config.timezone,
                self.config.locale
            ),
            card.width.saturating_sub(8),
            design.style(TextStyle::Dim),
        );
        y += if compact { 2 } else { 3 };

        put(
            buf,
            card.x + 4,
            y,
            "How to boot your new installation:",
            card.width.saturating_sub(8),
            design
                .style(TextStyle::Body)
                .add_modifier(Modifier::UNDERLINED),
        );
        y += 1;

        let steps = match self.preflight.device_info.boot_type {
            BootType::Depthcharge => [
                "1. Reboot your Chromebook and remove the microSD/USB installation media.",
                "2. On the developer mode boot menu, pick \"internal disk\" or Ctrl+D.",
                "3. With external media removed, internal eMMC boots automatically.",
            ],
            BootType::Uefi => [
                "1. Shut down or reboot your system.",
                "2. Remove the installation USB/microSD media before UEFI firmware boots.",
                "3. Select Hayato (systemd-boot) from the UEFI boot menu if prompted.",
            ],
            BootType::Extlinux => [
                "1. Power off or reboot your board.",
                "2. Remove the installation microSD/USB card so U-Boot loads internal storage.",
                "3. U-Boot scans /boot/extlinux/extlinux.conf and DTBs in /boot/dtbs/ automatically.",
            ],
        };

        for line in steps {
            put_trunc(
                buf,
                card.x + 6,
                y,
                line,
                card.width.saturating_sub(12),
                design.style(TextStyle::Body),
            );
            y += 1;
        }
        y += if compact { 1 } else { 3 };
        let btn_y = y.min(card.bottom().saturating_sub(1));

        let reboot_style = if self.complete_focus == CompleteFocus::Reboot {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let exit_style = if self.complete_focus == CompleteFocus::Exit {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };

        if btn_y < card.bottom() {
            let reboot_lbl = "[ Reboot System Now ]";
            let reboot_w = reboot_lbl.chars().count() as u16;
            let reboot_x = card.x + 4;
            put(buf, reboot_x, btn_y, reboot_lbl, reboot_w, reboot_style);
            hits.complete_reboot = Some(Rect::new(reboot_x, btn_y, reboot_w, 1));

            let exit_lbl = "[ Stay in Live Session ]";
            let exit_w = exit_lbl.chars().count() as u16;
            let exit_x = (reboot_x + reboot_w + 4).min(card.right().saturating_sub(exit_w + 2));
            put(buf, exit_x, btn_y, exit_lbl, exit_w, exit_style);
            hits.complete_exit = Some(Rect::new(exit_x, btn_y, exit_w, 1));
        }
    }
}

fn detect_live_timezone() -> Option<String> {
    let raw = fs::read_to_string("/etc/timezone").ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn detect_live_locale() -> Option<String> {
    let raw = fs::read_to_string("/etc/default/locale").ok()?;
    for line in raw.lines() {
        if let Some(val) = line.strip_prefix("LANG=") {
            return Some(val.trim_matches('"').trim().to_string());
        }
    }
    None
}

fn detect_live_keymap() -> Option<String> {
    let raw = fs::read_to_string("/etc/default/keyboard").ok()?;
    for line in raw.lines() {
        if let Some(val) = line.strip_prefix("XKBLAYOUT=") {
            return Some(val.trim_matches('"').trim().to_string());
        }
    }
    None
}

fn detect_live_hostname() -> Option<String> {
    let raw = fs::read_to_string("/etc/hostname").ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn detect_live_user() -> (String, String) {
    if let Ok(passwd) = fs::read_to_string("/etc/passwd") {
        for line in passwd.lines() {
            let cols: Vec<&str> = line.split(':').collect();
            if cols.len() >= 5 && cols[2] == "1000" {
                let user = cols[0].to_string();
                let gecos = cols[4].split(',').next().unwrap_or("").trim();
                let full = if gecos.is_empty() {
                    user.clone()
                } else {
                    gecos.to_string()
                };
                return (user, full);
            }
        }
    }
    ("brunux".to_string(), "Bruno".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::deviceinfo::DeviceInfo;
    use crate::installer::preflight::TargetCandidate;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn focus_enums_keep_the_tab_order() {
        assert_eq!(LocFocus::Next.next(), LocFocus::TimezoneInput);
        assert_eq!(LocFocus::TimezoneInput.prev(), LocFocus::Next);
        assert_eq!(UserFocus::Next.next(), UserFocus::FullName);
        assert_eq!(UserFocus::FullName.prev(), UserFocus::Next);
        assert_eq!(DiskFocus::Erase.next(false), DiskFocus::Cancel);
        assert_eq!(DiskFocus::Erase.next(true), DiskFocus::Target);
        assert_eq!(DiskFocus::Cancel.prev(true), DiskFocus::Target);
        assert_eq!(CompleteFocus::Reboot.toggle(), CompleteFocus::Exit);
    }

    fn mock_preflight(boot_type: BootType) -> Preflight {
        let (target, name, model) = match boot_type {
            BootType::Depthcharge => (
                "/dev/mmcblk0",
                "Google Asurada Chromebook",
                "Google Hayato (MediaTek MT8192)",
            ),
            BootType::Uefi => ("/dev/nvme0n1", "ARM64 Generic UEFI", "Lenovo ThinkPad X13s"),
            BootType::Extlinux => (
                "/dev/mmcblk0",
                "ARM64 Generic U-Boot / SBC",
                "Radxa ROCK 5B",
            ),
        };
        let device_info = DeviceInfo {
            boot_type,
            name: name.to_string(),
            ..Default::default()
        };
        Preflight {
            device_info,
            soc_model: model.to_string(),
            total_ram: "8.0 GiB".to_string(),
            power_source: "AC connected".to_string(),
            root_source: "/dev/sda2".to_string(),
            target_disk: target.to_string(),
            target_size: "64.0 GiB".to_string(),
            candidates: vec![TargetCandidate {
                path: target.to_string(),
                short_name: target.trim_start_matches("/dev/").to_string(),
                size_human: "64.0 GiB".to_string(),
                model: "Test Disk".to_string(),
                tran: None,
                removable: false,
            }],
            is_safe: true,
            refusal_reason: None,
            installed_notice: None,
            tools: Vec::new(),
        }
    }

    fn render_step(screen: &mut InstallerScreen, step: WizardStep, w: u16, h: u16) -> String {
        screen.step = step;
        screen.update_keybar();
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let design = Design::solarized();
        terminal
            .draw(|f| {
                let area = f.area();
                let mut ctx = BodyCtx {
                    area,
                    buf: f.buffer_mut(),
                    design: &design,
                };
                screen.draw(&mut ctx);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_all_three_boot_types_in_welcome_review_and_complete() {
        for (bt, expect_part, expect_confirm, expect_complete) in [
            (
                BootType::Depthcharge,
                "pmOS_kernel",
                "Type \"mmcblk0\" to confirm",
                "developer mode boot menu",
            ),
            (
                BootType::Uefi,
                "hayato_efi",
                "Type \"nvme0n1\" to confirm",
                "UEFI boot menu",
            ),
            (
                BootType::Extlinux,
                "hayato_boot",
                "Type \"mmcblk0\" to confirm",
                "/boot/extlinux/extlinux.conf",
            ),
        ] {
            let mut scr = InstallerScreen::from_preflight_isolated(mock_preflight(bt), true);
            for (w, h) in [(80, 24), (152, 45)] {
                let welcome = render_step(&mut scr, WizardStep::Welcome, w, h);
                assert!(
                    welcome.contains("Pre-flight checks passed"),
                    "boot={bt:?} size={w}x{h}"
                );
            }
            let review = render_step(&mut scr, WizardStep::DiskReview, 120, 40);
            assert!(review.contains(expect_part), "missing {expect_part}");
            assert!(review.contains(expect_confirm), "missing {expect_confirm}");
            let complete = render_step(&mut scr, WizardStep::Complete, 120, 40);
            assert!(
                complete.contains(expect_complete),
                "missing {expect_complete}"
            );
        }
    }

    #[test]
    fn step_header_keeps_legible_contrast_and_only_focused_input_blinks() {
        let design = Design::solarized();
        let mut scr =
            InstallerScreen::from_preflight_isolated(mock_preflight(BootType::Depthcharge), true);
        scr.step = WizardStep::UserAccount;
        scr.user_focus = UserFocus::FullName;

        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                let mut ctx = BodyCtx {
                    area,
                    buf: f.buffer_mut(),
                    design: &design,
                };
                scr.draw(&mut ctx);
            })
            .unwrap();
        let buf = terminal.backend().buffer();

        // Badge " Step 3 of 6 " must have distinct fg != bg (fixing old Accent-on-Accent bug).
        let badge_cell = buf.cell((3, 0)).unwrap();
        assert_ne!(
            badge_cell.fg, badge_cell.bg,
            "step badge foreground and background collided"
        );

        // Focused field 0 draws left Accent bar; unfocused field 1 does not.
        let r0 = scr.hits.borrow().user_inputs[0].expect("fullname hit rect");
        let r1 = scr.hits.borrow().user_inputs[1].expect("username hit rect");
        assert_eq!(buf.cell((r0.x, r0.y)).unwrap().symbol(), "\u{258d}");
        assert_ne!(buf.cell((r1.x, r1.y)).unwrap().symbol(), "\u{258d}");

        // Blink tick toggles cursor state cleanly while UserAccount input is focused.
        assert!(scr.needs_tick().is_some());
        assert!(scr.cursor_blink_on);
        scr.last_blink -= Duration::from_millis(600);
        scr.tick();
        assert!(!scr.cursor_blink_on);

        // Key press resets blink phase to solid-on and Enter advances to next field.
        scr.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &design);
        assert!(scr.cursor_blink_on);
        assert_eq!(scr.user_focus, UserFocus::Username);
    }

    #[test]
    fn compact_80x24_layout_keeps_all_step_buttons_and_confirm_visible() {
        use hayato_ui::App;
        use hayato_ui::widgets::Header;

        let design = Design::solarized();
        for step in [
            WizardStep::Localization,
            WizardStep::UserAccount,
            WizardStep::DiskReview,
        ] {
            let mut scr = InstallerScreen::from_preflight_isolated(
                mock_preflight(BootType::Depthcharge),
                true,
            );
            scr.step = step;
            scr.update_keybar();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut app = App::new(Header::new("Hayato"))
                .design(design.clone())
                .screen(&mut scr);
            terminal.draw(|f| app.render(f)).unwrap();
            let buf = terminal.backend().buffer();
            let mut text = String::new();
            for y in 0..24 {
                for x in 0..80 {
                    text.push_str(buf[(x, y)].symbol());
                }
                text.push('\n');
            }
            match step {
                WizardStep::Localization => {
                    assert!(text.contains("[ Next: User > ]"), "clipped step 2:\n{text}");
                    assert!(text.contains("[ < Back ]"), "clipped back step 2:\n{text}");
                }
                WizardStep::UserAccount => {
                    assert!(
                        text.contains("[ Next: Review & Confirm > ]"),
                        "clipped step 3:\n{text}"
                    );
                    assert!(text.contains("SSH host keys"), "clipped ssh:\n{text}");
                }
                WizardStep::DiskReview => {
                    assert!(
                        text.contains("Type \"mmcblk0\" to confirm"),
                        "clipped confirm step 4:\n{text}"
                    );
                    assert!(
                        text.contains("[ Erase Disk & Install Hayato ]"),
                        "clipped erase btn:\n{text}"
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn disk_confirm_enter_arms_erase_without_skipping_confirmation_gate() {
        let design = Design::solarized();
        let mut scr =
            InstallerScreen::from_preflight_isolated(mock_preflight(BootType::Depthcharge), true);
        scr.step = WizardStep::DiskReview;
        scr.disk_focus = DiskFocus::Confirm;
        for ch in "mmcblk0".chars() {
            scr.key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &design,
            );
        }
        // Pressing Enter inside confirmed input moves focus to Erase button without auto-starting.
        scr.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &design);
        assert_eq!(scr.step, WizardStep::DiskReview);
        assert_eq!(scr.disk_focus, DiskFocus::Erase);

        // Pre-selected timezone list cursor matches detected timezone row.
        if let Some(sel) = scr.tz_list.selected() {
            assert_eq!(sel.label, scr.selected_tz);
        }
    }

    /// The gauge draws the percentage itself; a label that carries it too puts
    /// the number on the screen twice ("42% 42% - Copying").
    #[test]
    fn the_progress_gauge_prints_the_percentage_once() {
        let mut scr =
            InstallerScreen::from_preflight_isolated(mock_preflight(BootType::Depthcharge), true);
        scr.worker_state.percent = 42;
        scr.worker_state.phase_name = "Copying the system".to_string();
        let frame = render_step(&mut scr, WizardStep::Progress, 100, 30);
        let row = frame
            .lines()
            .find(|line| line.contains("42%"))
            .expect("the gauge row carries the percentage");
        assert_eq!(
            row.matches("42%").count(),
            1,
            "the percentage is drawn twice: {row}"
        );
    }
}
