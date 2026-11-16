//! Full-screen interactive wizard for LazyOS installation (spec 018 & spec 023).

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lazyos_ui::app::{BodyCtx, Outcome, Screen};
use lazyos_ui::design::{ColorRole, Design, TextStyle};
use lazyos_ui::widgets::{
    Input, KeyHint, ListAction, ListState, Row, draw_card, draw_gauge, put, put_trunc,
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
    PHASES, WorkerState, active_running_state, load_state, phase_label, resolve_state_dir,
    run_worker, state_file_path_for,
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
    pub loc_focus: usize,
    // Step 2: User Account
    pub fullname_input: Input,
    pub username_input: Input,
    pub password_input: Input,
    pub confirm_input: Input,
    pub hostname_input: Input,
    pub regen_ssh: bool,
    pub user_focus: usize,
    pub user_error: Option<&'static str>,
    // Step 3: Disk Review & Gated Confirmation
    pub disk_idx: usize,
    pub disk_confirm_input: Input,
    pub disk_focus: usize, // 0: Cancel/Back, 1: Confirm Input, 2: Erase & Install, 3: Target selector
    // Step 4: Progress
    pub worker_state: WorkerState,
    pub worker_running: bool,
    pub cancel_worker: Option<Arc<AtomicBool>>,
    pub spinner_idx: usize,
    pub show_log_full: bool,
    swayidle_paused: bool,
    // Step 5: Complete
    pub complete_focus: usize,
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
                screen.worker_running = true;
                screen.step = WizardStep::Progress;
                screen.pause_swayidle();
                screen.update_keybar();
            }
        }
        screen
    }

    /// Construct wizard cleanly without checking host `/run/lazyos-install` (ideal for tests).
    pub fn from_preflight_isolated(preflight: Preflight, dry_run: bool) -> Self {
        let live_tz = detect_live_timezone().unwrap_or_else(|| "UTC".to_string());
        let live_locale = detect_live_locale().unwrap_or_else(|| "en_US.UTF-8".to_string());
        let live_keymap = detect_live_keymap().unwrap_or_else(|| "us".to_string());
        let (live_user, live_full) = detect_live_user();
        let live_host = detect_live_hostname().unwrap_or_else(|| "lazyos".to_string());

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
            Some("/run/lazyos-install".to_string())
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

        let tz_rows: Vec<Row> = COMMON_TIMEZONES
            .iter()
            .map(|&tz| Row::new(tz).detail("IANA timezone"))
            .collect();
        let mut tz_list = ListState::new();
        tz_list.set_rows(tz_rows);

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
            loc_focus: 0,
            fullname_input: Input::with_text(&live_full),
            username_input: Input::with_text(&live_user),
            password_input: Input::new().with_masked(true),
            confirm_input: Input::new().with_masked(true),
            hostname_input: Input::with_text(&live_host),
            regen_ssh: true,
            user_focus: 0,
            user_error: None,
            disk_idx,
            disk_confirm_input: Input::new(),
            disk_focus: 0,
            worker_state: initial_state,
            worker_running: false,
            cancel_worker: None,
            spinner_idx: 0,
            show_log_full: false,
            swayidle_paused: false,
            complete_focus: 0,
            keybar_hints: Vec::new(),
        };
        screen.update_keybar();
        screen
    }

    fn pause_swayidle(&mut self) {
        if self.swayidle_paused {
            return;
        }
        if Command::new("pkill")
            .args(["-STOP", "-x", "swayidle"])
            .status()
            .is_ok_and(|s| s.success())
        {
            self.swayidle_paused = true;
        }
    }

    fn resume_swayidle(&mut self) {
        if self.swayidle_paused {
            let _ = Command::new("pkill")
                .args(["-CONT", "-x", "swayidle"])
                .status();
            self.swayidle_paused = false;
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
                if self.worker_state.status == "failed" {
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
        self.cancel_worker = Some(cancel.clone());
        self.worker_running = true;

        let mut conf = self.config.clone();

        if conf.dry_run {
            let dir = resolve_state_dir(conf.state_dir.as_deref());
            let _ = fs::remove_file(state_file_path_for(&dir));
            std::thread::spawn(move || {
                let _ = run_worker(conf, Some(cancel));
            });
            return;
        }

        conf.state_dir = Some("/run/lazyos-install".to_string());
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
            .unwrap_or_else(|| std::path::PathBuf::from("/usr/bin/lazyos"));
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
                "--why=LazyOS installation in progress",
            ]);
        }
        cmd.args([
            &exe_str,
            "installer",
            "apply",
            "--stdin",
            "--state-dir",
            "/run/lazyos-install",
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
        self.worker_state.status = "failed".to_string();
        self.worker_state.cancellable = false;
        self.worker_state.error = Some(msg.to_string());
        self.worker_state.log_lines.push(format!("ERROR: {msg}"));
        self.worker_running = false;
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

    fn needs_tick(&self) -> Option<Duration> {
        if self.step == WizardStep::Progress {
            Some(Duration::from_millis(100))
        } else {
            None
        }
    }

    fn tick(&mut self) -> Outcome {
        if self.step == WizardStep::Progress {
            self.spinner_idx = (self.spinner_idx + 1) % 4;
            let prev_first = self.worker_state.first_write;
            let prev_status = self.worker_state.status.clone();
            if let Some(state) = load_state() {
                self.worker_state = state;
                if self.worker_state.status == "success" {
                    self.worker_running = false;
                    self.resume_swayidle();
                    self.step = WizardStep::Complete;
                    self.update_keybar();
                } else if self.worker_state.first_write != prev_first
                    || self.worker_state.status != prev_status
                {
                    if self.worker_state.status == "failed" {
                        self.worker_running = false;
                        self.resume_swayidle();
                    }
                    self.update_keybar();
                }
            }
        }
        Outcome::Continue
    }

    fn key(&mut self, key: KeyEvent, design: &Design) -> Outcome {
        if key.kind != ratatui::crossterm::event::KeyEventKind::Press {
            return Outcome::Continue;
        }

        match self.step {
            WizardStep::Welcome => match key.code {
                KeyCode::Esc => Outcome::Done(lazyos_ui::app::exit::ESCAPE),
                KeyCode::Enter => {
                    if self.preflight.is_safe {
                        self.step = WizardStep::Localization;
                        self.update_keybar();
                        Outcome::Consumed
                    } else {
                        Outcome::Done(lazyos_ui::app::exit::ESCAPE)
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
                KeyCode::Enter if self.worker_state.status == "failed" => {
                    Outcome::Done(lazyos_ui::app::exit::FAILED)
                }
                KeyCode::Esc => {
                    if !self.worker_state.first_write && self.worker_state.cancellable {
                        if let Some(ref cancel) = self.cancel_worker {
                            cancel.store(true, Ordering::SeqCst);
                        }
                    }
                    self.resume_swayidle();
                    Outcome::Done(lazyos_ui::app::exit::ESCAPE)
                }
                _ => Outcome::Consumed,
            },

            WizardStep::Complete => match key.code {
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    self.complete_focus = 1 - self.complete_focus;
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    if self.complete_focus == 0 && !self.config.dry_run {
                        let _ = Command::new("systemctl").arg("reboot").spawn();
                    }
                    Outcome::Done(lazyos_ui::app::exit::OK)
                }
                KeyCode::Esc => Outcome::Done(lazyos_ui::app::exit::OK),
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

        let step_text = format!(
            " Step {} of 6: {} ",
            self.step as usize + 1,
            self.step.title()
        );
        put(
            buf,
            area.x + 2,
            area.y,
            &step_text,
            area.width.saturating_sub(4),
            design
                .style(TextStyle::Key)
                .add_modifier(Modifier::BOLD)
                .fg(design.color(ColorRole::Accent)),
        );

        let content_area = Rect {
            x: area.x + 1,
            y: area.y + 2,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(3),
        };

        match self.step {
            WizardStep::Welcome => self.draw_welcome(content_area, buf, design),
            WizardStep::Localization => self.draw_localization(content_area, buf, design),
            WizardStep::UserAccount => self.draw_user_account(content_area, buf, design),
            WizardStep::DiskReview => self.draw_disk_review(content_area, buf, design),
            WizardStep::Progress => self.draw_progress(content_area, buf, design),
            WizardStep::Complete => self.draw_complete(content_area, buf, design),
        }
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
                    if self.loc_focus == 0 {
                        6
                    } else {
                        self.loc_focus - 1
                    }
                } else {
                    (self.loc_focus + 1) % 7
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.loc_focus = if self.loc_focus == 0 {
                    6
                } else {
                    self.loc_focus - 1
                };
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.loc_focus {
            0 => {
                if key.code == KeyCode::Down {
                    self.loc_focus = 1;
                    return Outcome::Consumed;
                }
                self.tz_input.handle(key);
                self.tz_list.set_filter(self.tz_input.text());
                Outcome::Consumed
            }
            1 => match self.tz_list.handle(key, 15, design) {
                ListAction::Activated(_) => {
                    if let Some(row) = self.tz_list.selected() {
                        self.selected_tz = row.label.clone();
                        self.config.timezone = row.label.clone();
                    }
                    Outcome::Consumed
                }
                ListAction::Ignored if key.code == KeyCode::Esc => {
                    self.loc_focus = 0;
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            },
            2 => {
                match key.code {
                    KeyCode::Left | KeyCode::Up => {
                        self.locale_idx = self.locale_idx.saturating_sub(1);
                        self.config.locale = self.locales[self.locale_idx].to_string();
                    }
                    KeyCode::Right | KeyCode::Down => {
                        self.locale_idx =
                            (self.locale_idx + 1).min(self.locales.len().saturating_sub(1));
                        self.config.locale = self.locales[self.locale_idx].to_string();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            3 => {
                match key.code {
                    KeyCode::Left | KeyCode::Up => {
                        self.keymap_idx = self.keymap_idx.saturating_sub(1);
                        self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                    }
                    KeyCode::Right | KeyCode::Down => {
                        self.keymap_idx =
                            (self.keymap_idx + 1).min(self.keymaps.len().saturating_sub(1));
                        self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                    }
                    _ => {}
                }
                Outcome::Consumed
            }
            4 => {
                self.keymap_test.handle(key);
                Outcome::Consumed
            }
            5 => {
                if key.code == KeyCode::Enter {
                    self.step = WizardStep::Welcome;
                    self.update_keybar();
                }
                Outcome::Consumed
            }
            6 => {
                if key.code == KeyCode::Enter {
                    self.config.timezone = self.selected_tz.clone();
                    self.config.locale = self.locales[self.locale_idx].to_string();
                    self.config.keymap = self.keymaps[self.keymap_idx].to_string();
                    self.step = WizardStep::UserAccount;
                    self.update_keybar();
                }
                Outcome::Consumed
            }
            _ => Outcome::Continue,
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
                    if self.user_focus == 0 {
                        7
                    } else {
                        self.user_focus - 1
                    }
                } else {
                    (self.user_focus + 1) % 8
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.user_focus = if self.user_focus == 0 {
                    7
                } else {
                    self.user_focus - 1
                };
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.user_focus {
            0 => {
                self.fullname_input.handle(key);
                self.config.fullname = self.fullname_input.text().to_string();
                Outcome::Consumed
            }
            1 => {
                self.username_input.handle(key);
                self.config.username = self.username_input.text().to_string();
                self.user_error = validate_username(&self.config.username).err();
                Outcome::Consumed
            }
            2 => {
                self.password_input.handle(key);
                self.config.password = self.password_input.text().to_string();
                self.user_error =
                    validate_password(&self.config.password, &self.config.password_confirm).err();
                Outcome::Consumed
            }
            3 => {
                self.confirm_input.handle(key);
                self.config.password_confirm = self.confirm_input.text().to_string();
                self.user_error =
                    validate_password(&self.config.password, &self.config.password_confirm).err();
                Outcome::Consumed
            }
            4 => {
                self.hostname_input.handle(key);
                self.config.hostname = self.hostname_input.text().to_string();
                self.user_error = validate_hostname(&self.config.hostname).err();
                Outcome::Consumed
            }
            5 => {
                if key.code == KeyCode::Char(' ') || key.code == KeyCode::Enter {
                    self.regen_ssh = !self.regen_ssh;
                    self.config.regenerate_ssh_keys = self.regen_ssh;
                }
                Outcome::Consumed
            }
            6 => {
                if key.code == KeyCode::Enter {
                    self.step = WizardStep::Localization;
                    self.update_keybar();
                }
                Outcome::Consumed
            }
            7 => {
                if key.code == KeyCode::Enter {
                    self.config.fullname = self.fullname_input.text().to_string();
                    self.config.username = self.username_input.text().to_string();
                    self.config.password = self.password_input.text().to_string();
                    self.config.password_confirm = self.confirm_input.text().to_string();
                    self.config.hostname = self.hostname_input.text().to_string();
                    self.config.regenerate_ssh_keys = self.regen_ssh;

                    if let Err(err) = self.config.validate_user_fields() {
                        self.user_error = Some(err);
                        return Outcome::Consumed;
                    }
                    self.user_error = None;
                    self.step = WizardStep::DiskReview;
                    self.disk_focus = 0; // Always start on Cancel!
                    self.update_keybar();
                }
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn handle_disk_review_key(&mut self, key: KeyEvent) -> Outcome {
        let max_focus = if self.preflight.candidates.len() > 1 {
            4
        } else {
            3
        };
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::UserAccount;
                self.update_keybar();
                return Outcome::Consumed;
            }
            KeyCode::Tab => {
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                self.disk_focus = if shift {
                    if self.disk_focus == 0 {
                        max_focus - 1
                    } else {
                        self.disk_focus - 1
                    }
                } else {
                    (self.disk_focus + 1) % max_focus
                };
                return Outcome::Consumed;
            }
            KeyCode::BackTab => {
                self.disk_focus = if self.disk_focus == 0 {
                    max_focus - 1
                } else {
                    self.disk_focus - 1
                };
                return Outcome::Consumed;
            }
            _ => {}
        }

        match self.disk_focus {
            0 => {
                if key.code == KeyCode::Enter {
                    self.step = WizardStep::UserAccount;
                    self.update_keybar();
                }
                Outcome::Consumed
            }
            1 => {
                self.disk_confirm_input.handle(key);
                Outcome::Consumed
            }
            2 => {
                if key.code == KeyCode::Enter {
                    let token = self.target_short().to_string();
                    if validate_confirmation(self.disk_confirm_input.text(), &token) {
                        self.step = WizardStep::Progress;
                        self.update_keybar();
                        self.start_worker();
                    }
                }
                Outcome::Consumed
            }
            3 => {
                match key.code {
                    KeyCode::Left => self.cycle_target(-1),
                    KeyCode::Right | KeyCode::Enter => self.cycle_target(1),
                    _ => {}
                }
                Outcome::Consumed
            }
            _ => Outcome::Continue,
        }
    }

    fn draw_welcome(&self, area: Rect, buf: &mut ratatui::buffer::Buffer, design: &Design) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + 1,
                y: area.y,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(1),
            },
            design,
            Some("LazyOS Full-System Installer"),
            true,
        );

        let mut y = card.y + 1;
        put(
            buf,
            card.x + 2,
            y,
            &format!(
                "Welcome to the LazyOS full-system installer for {} ({}).",
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
                "Boot LazyOS from removable USB or microSD media to install to internal storage.",
                danger_area.width.saturating_sub(4),
                design.style(TextStyle::Dim),
            );
        } else {
            let badge_area = Rect {
                x: card.x + 2,
                y,
                width: card.width.saturating_sub(4),
                height: 3,
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
            put(
                buf,
                badge_area.x + 2,
                badge_area.y + 1,
                badge_msg,
                badge_area.width.saturating_sub(4),
                Style::default()
                    .fg(design.color(ColorRole::Accent))
                    .add_modifier(Modifier::BOLD),
            );
            y += 4;

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
                if col1_y >= card.bottom() || col2_y >= card.bottom() {
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
    }

    fn draw_localization(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
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
            self.loc_focus == 0 || self.loc_focus == 1,
        );

        put(
            buf,
            tz_card.x + 1,
            tz_card.y,
            "Filter Timezones:",
            tz_card.width.saturating_sub(2),
            design.style(TextStyle::Dim),
        );
        self.tz_input.render(
            Rect {
                x: tz_card.x + 1,
                y: tz_card.y + 1,
                width: tz_card.width.saturating_sub(2),
                height: 1,
            },
            buf,
            design,
            "Type to filter (e.g. America, Europe)...",
        );

        self.tz_list.render(
            Rect {
                x: tz_card.x + 1,
                y: tz_card.y + 3,
                width: tz_card.width.saturating_sub(2),
                height: tz_card.height.saturating_sub(4),
            },
            buf,
            design,
            self.loc_focus == 1,
        );

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
            self.loc_focus >= 2,
        );

        let mut y = right_card.y + 1;
        put(
            buf,
            right_card.x + 2,
            y,
            &format!("Selected Timezone:  {}", self.selected_tz),
            right_card.width.saturating_sub(4),
            design.style(TextStyle::Body).add_modifier(Modifier::BOLD),
        );
        y += 2;

        let loc_style = if self.loc_focus == 2 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        put(
            buf,
            right_card.x + 2,
            y,
            &format!("System Locale:  < {} >", self.locales[self.locale_idx]),
            right_card.width.saturating_sub(4),
            loc_style,
        );
        put(
            buf,
            right_card.x + 2,
            y + 1,
            "(Use Left/Right arrow keys to switch locale)",
            right_card.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 3;

        let kbd_style = if self.loc_focus == 3 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        put(
            buf,
            right_card.x + 2,
            y,
            &format!("Keyboard Layout:  < {} >", self.keymaps[self.keymap_idx]),
            right_card.width.saturating_sub(4),
            kbd_style,
        );
        put(
            buf,
            right_card.x + 2,
            y + 1,
            "(Use Left/Right arrow keys to switch keymap)",
            right_card.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 3;

        put(
            buf,
            right_card.x + 2,
            y,
            "Interactive Keyboard Test (Type here to verify symbols):",
            right_card.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 1;
        self.keymap_test.render(
            Rect {
                x: right_card.x + 2,
                y,
                width: right_card.width.saturating_sub(4),
                height: 1,
            },
            buf,
            design,
            "Type test keys: @ # $ % & / ( ) = ? ¡ ¿ ~",
        );
        y += 3;

        let back_style = if self.loc_focus == 5 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let next_style = if self.loc_focus == 6 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };

        put(buf, right_card.x + 4, y, "[ < Back ]", 10, back_style);
        put(
            buf,
            right_card.x + 18,
            y,
            "[ Next: User Account > ]",
            24,
            next_style,
        );
    }

    fn draw_user_account(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        design: &Design,
    ) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + area.width / 6,
                y: area.y,
                width: (area.width * 2) / 3,
                height: area.height,
            },
            design,
            Some("Primary User Account & Machine Identity"),
            true,
        );

        let mut y = card.y + 1;
        let fields = [
            (
                "Full Name (Display Name):",
                "Your full name (e.g. Bruno)",
                0,
            ),
            (
                "POSIX Username (Login & Sudo User):",
                "lowercase login name (e.g. brunux)",
                1,
            ),
            ("Account Password:", "enter password", 2),
            ("Confirm Password:", "re-type password", 3),
            ("System Hostname:", "network hostname (default lazyos)", 4),
        ];

        for (label, ph, idx) in fields {
            put(
                buf,
                card.x + 4,
                y,
                label,
                card.width.saturating_sub(8),
                design.style(TextStyle::Dim),
            );
            y += 1;
            let rect = Rect {
                x: card.x + 4,
                y,
                width: card.width.saturating_sub(8),
                height: 1,
            };
            match idx {
                0 => self.fullname_input.render(rect, buf, design, ph),
                1 => self.username_input.render(rect, buf, design, ph),
                2 => self.password_input.render(rect, buf, design, ph),
                3 => self.confirm_input.render(rect, buf, design, ph),
                _ => self.hostname_input.render(rect, buf, design, ph),
            }
            y += 2;
        }

        let check_mark = if self.regen_ssh { "[X]" } else { "[ ]" };
        let ssh_style = if self.user_focus == 5 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        put(
            buf,
            card.x + 4,
            y,
            &format!("{check_mark} Generate fresh SSH host keys on first boot"),
            card.width.saturating_sub(8),
            ssh_style,
        );
        y += 2;

        if let Some(err) = self.user_error {
            put(
                buf,
                card.x + 4,
                y,
                &format!("⚠ Validation Error: {err}"),
                card.width.saturating_sub(8),
                Style::default()
                    .fg(design.color(ColorRole::Danger))
                    .add_modifier(Modifier::BOLD),
            );
            y += 2;
        }

        let back_style = if self.user_focus == 6 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let next_style = if self.user_focus == 7 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };

        put(buf, card.x + 4, y, "[ < Back ]", 10, back_style);
        put(
            buf,
            card.x + 20,
            y,
            "[ Next: Review & Confirm > ]",
            28,
            next_style,
        );
    }

    fn draw_disk_review(&mut self, area: Rect, buf: &mut ratatui::buffer::Buffer, design: &Design) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + 2,
                y: area.y,
                width: area.width.saturating_sub(4),
                height: area.height,
            },
            design,
            Some("Target Storage & Partition Review"),
            true,
        );

        let mut y = card.y + 1;
        let selector_suffix = if self.preflight.candidates.len() > 1 {
            "  [< Left / Right >]"
        } else {
            ""
        };
        let target_style = if self.disk_focus == 3 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body).add_modifier(Modifier::BOLD)
        };
        put(
            buf,
            card.x + 2,
            y,
            &format!(
                "Target Storage Device: {} ({}){selector_suffix}",
                self.config.target_disk,
                self.target_size()
            ),
            card.width.saturating_sub(4),
            target_style,
        );
        y += 2;

        y = self.draw_partition_diagram(card.x, y, card.width, buf, design);
        y += 1;

        let danger_box = Rect {
            x: card.x + 2,
            y,
            width: card.width.saturating_sub(4),
            height: 4,
        };
        fill_rect(
            buf,
            danger_box,
            Style::default().bg(design.color(ColorRole::SurfaceAlt)),
        );
        put(
            buf,
            danger_box.x + 2,
            danger_box.y + 1,
            "⚠ WARNING: ALL EXISTING DATA ON TARGET DRIVE WILL BE PERMANENTLY ERASED!",
            danger_box.width.saturating_sub(4),
            Style::default()
                .fg(design.color(ColorRole::Danger))
                .add_modifier(Modifier::BOLD),
        );
        put(
            buf,
            danger_box.x + 2,
            danger_box.y + 2,
            &format!(
                "This action destroys all existing partitions and data on {}.",
                self.config.target_disk
            ),
            danger_box.width.saturating_sub(4),
            design.style(TextStyle::Body),
        );
        y += 5;

        let short = self.target_short().to_string();
        let confirmed = validate_confirmation(self.disk_confirm_input.text(), &short);
        put(
            buf,
            card.x + 2,
            y,
            &format!("Type \"{short}\" to confirm disk erasure:"),
            card.width.saturating_sub(4),
            design.style(TextStyle::Dim),
        );
        y += 1;
        let confirm_box = Rect {
            x: card.x + 2,
            y,
            width: 32,
            height: 1,
        };
        self.disk_confirm_input
            .render(confirm_box, buf, design, &format!("type {short}"));
        y += 2;

        let cancel_style = if self.disk_focus == 0 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let erase_style = if !confirmed {
            design.style(TextStyle::Dim)
        } else if self.disk_focus == 2 {
            Style::default()
                .bg(design.color(ColorRole::Danger))
                .fg(design.color(ColorRole::Surface))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(design.color(ColorRole::Danger))
                .add_modifier(Modifier::BOLD)
        };

        if y < card.bottom() {
            put(buf, card.x + 2, y, "[ Cancel / Back ]", 17, cancel_style);
            put(
                buf,
                card.x + 22,
                y,
                "[ Erase Disk & Install LazyOS ]",
                32,
                erase_style,
            );
        }
    }

    fn draw_partition_diagram(
        &self,
        card_x: u16,
        mut y: u16,
        card_width: u16,
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

        let (box_rows, bullets): ([&str; 4], Vec<String>) = match info.boot_type {
            BootType::Depthcharge => (
                [
                    "┌─────────────────┬───────────────────┬──────────────────────────────────────┐",
                    "│ p1: pmOS_kernel │   p2: pmOS_boot   │             p3: pmOS_root            │",
                    "│     128 MiB     │      512 MiB      │          remaining storage           │",
                    "└─────────────────┴───────────────────┴──────────────────────────────────────┘",
                ],
                vec![
                    "• p1 (128 MiB): ChromeOS signed FIT kernel partition (Depthcharge bootloader)"
                        .into(),
                    format!(
                        "• p2 ({} MiB): Boot filesystem ({}, mounted to /boot)",
                        info.boot_size_mib, info.boot_filesystem
                    ),
                    "• p3 (Remainder): Debian arm64 root filesystem (ext4, mounted to /)".into(),
                ],
            ),
            BootType::Uefi => (
                [
                    "┌───────────────────────────┬────────────────────────────────────────────────┐",
                    "│      p1: lazyos_efi       │                p2: lazyos_root                 │",
                    "│      256 MiB (FAT32)      │               remaining (ext4)                 │",
                    "└───────────────────────────┴────────────────────────────────────────────────┘",
                ],
                vec![
                    format!(
                        "• p1 ({} MiB): EFI System Partition (FAT32, systemd-boot at /boot/efi)",
                        info.boot_size_mib
                    ),
                    "• p2 (Remainder): Debian arm64 root filesystem (ext4, mounted to /)".into(),
                ],
            ),
            BootType::Extlinux => (
                [
                    "┌───────────────────────────┬────────────────────────────────────────────────┐",
                    "│     p1: lazyos_boot       │                p2: lazyos_root                 │",
                    "│      512 MiB (ext2)       │               remaining (ext4)                 │",
                    "└───────────────────────────┴────────────────────────────────────────────────┘",
                ],
                vec![
                    format!(
                        "• p1 ({} MiB): Boot partition ({}, extlinux + DTBs at /boot)",
                        info.boot_size_mib, info.boot_filesystem
                    ),
                    "• p2 (Remainder): Debian arm64 root filesystem (ext4, mounted to /)".into(),
                ],
            ),
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

    fn draw_progress(&mut self, area: Rect, buf: &mut ratatui::buffer::Buffer, design: &Design) {
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
        let gauge_label = format!(
            "{}% - {}",
            self.worker_state.percent, self.worker_state.phase_name
        );
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

            let spinners = ["◴", "◷", "◶", "◵"];
            let current_spinner = spinners[self.spinner_idx];
            let boot_type = self.preflight.device_info.boot_type;

            for idx in 0..PHASES.len() {
                let label = phase_label(idx, boot_type);
                let (icon, style) = if idx < self.worker_state.phase_index {
                    ("✓", Style::default().fg(design.color(ColorRole::Accent)))
                } else if idx == self.worker_state.phase_index {
                    if self.worker_state.status == "failed" {
                        ("✗", Style::default().fg(design.color(ColorRole::Danger)))
                    } else if self.worker_state.status == "success" {
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

    fn draw_complete(&mut self, area: Rect, buf: &mut ratatui::buffer::Buffer, design: &Design) {
        let card = draw_card(
            buf,
            Rect {
                x: area.x + area.width / 8,
                y: area.y + 1,
                width: (area.width * 3) / 4,
                height: area.height.saturating_sub(2),
            },
            design,
            Some("Installation Finished"),
            true,
        );

        let mut y = card.y + 2;
        put(
            buf,
            card.x + 4,
            y,
            "✓ LazyOS Successfully Installed!",
            card.width.saturating_sub(8),
            Style::default()
                .fg(design.color(ColorRole::Accent))
                .add_modifier(Modifier::BOLD),
        );
        y += 2;

        put(
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
        put(
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
        y += 3;

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
                "1. Reboot your Chromebook.",
                "2. At the developer mode boot menu, choose \"internal disk\".",
                "3. With the microSD/USB removed, the internal eMMC boots automatically.",
            ],
            BootType::Uefi => [
                "1. Shut down or reboot your system.",
                "2. Remove the installation USB/microSD media before UEFI firmware boots.",
                "3. Select LazyOS (systemd-boot) from the UEFI boot menu if prompted.",
            ],
            BootType::Extlinux => [
                "1. Power off or reboot your board.",
                "2. Remove the installation microSD/USB card so U-Boot loads internal storage.",
                "3. U-Boot scans /boot/extlinux/extlinux.conf and DTBs in /boot/dtbs/ automatically.",
            ],
        };

        for line in steps {
            put(
                buf,
                card.x + 6,
                y,
                line,
                card.width.saturating_sub(12),
                design.style(TextStyle::Body),
            );
            y += 1;
        }
        y += 3;

        let reboot_style = if self.complete_focus == 0 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };
        let exit_style = if self.complete_focus == 1 {
            design.style(TextStyle::Key).add_modifier(Modifier::BOLD)
        } else {
            design.style(TextStyle::Body)
        };

        if y < card.bottom() {
            put(
                buf,
                card.x + 6,
                y,
                "[ Reboot System Now ]",
                22,
                reboot_style,
            );
            put(
                buf,
                card.x + 32,
                y,
                "[ Stay in Live Session ]",
                24,
                exit_style,
            );
        }
    }
}

fn detect_live_timezone() -> Option<String> {
    let raw = fs::read_to_string("/etc/timezone").ok()?;
    let trimmed = raw.trim();
    COMMON_TIMEZONES
        .contains(&trimmed)
        .then(|| trimmed.to_string())
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
                "lazyos_efi",
                "Type \"nvme0n1\" to confirm",
                "UEFI boot menu",
            ),
            (
                BootType::Extlinux,
                "lazyos_boot",
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
}
