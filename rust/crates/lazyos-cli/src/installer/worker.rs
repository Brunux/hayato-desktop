//! Pure-Rust installer execution engine and detached `/run/lazyos-install` IPC state.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::boot;
use super::config::{InstallConfig, InstallMode, target_short_name};
use super::deviceinfo::{BootType, DeviceInfo};
use super::disk;
use super::identity;
use super::preflight::check_safety;

/// The six sequential phases of installation (invariants tested in `all_phases_are_declared`).
pub const PHASES: &[&str] = &[
    "Partitioning storage (GPT layout)",
    "Formatting target filesystems",
    "Copying root filesystem (rsync)",
    "Configuring system identity & user",
    "Generating bootloader & kernel config",
    "Verifying target & finalizing",
];

/// Boot-architecture-aware label shown for phase `idx` in TUI and logs.
pub fn phase_label(idx: usize, boot_type: BootType) -> &'static str {
    match (idx, boot_type) {
        (0, BootType::Depthcharge) => "Partitioning storage (ChromeOS GPT)",
        (0, BootType::Uefi) => "Partitioning storage (UEFI GPT)",
        (0, BootType::Extlinux) => "Partitioning storage (GPT /boot + root)",
        (1, BootType::Depthcharge) => "Formatting filesystems (ext2, ext4)",
        (1, BootType::Uefi) => "Formatting filesystems (FAT32 ESP, ext4)",
        (1, BootType::Extlinux) => "Formatting filesystems (ext2, ext4)",
        (2, _) => PHASES[2],
        (3, _) => PHASES[3],
        (4, BootType::Depthcharge) => "Generating signed depthcharge kernel FIT",
        (4, BootType::Uefi) => "Installing systemd-boot (EFI System Partition)",
        (4, BootType::Extlinux) => "Writing extlinux.conf bootloader config",
        (5, BootType::Depthcharge) => "Verifying vboot signature & target ext4",
        (5, _) => "Verifying target filesystems & marker",
        _ => PHASES[idx.min(PHASES.len() - 1)],
    }
}

/// Progress state written atomically by worker to `state.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerState {
    pub phase_index: usize,
    pub phase_name: String,
    pub percent: u8,
    pub status: String, // "running", "success", "failed"
    pub error: Option<String>,
    pub log_lines: Vec<String>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub pid: u32,
    pub dry_run: bool,
    #[serde(default)]
    pub first_write: bool,
    #[serde(default = "default_cancellable")]
    pub cancellable: bool,
}

fn default_cancellable() -> bool {
    true
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            phase_index: 0,
            phase_name: PHASES[0].to_string(),
            percent: 0,
            status: "running".to_string(),
            error: None,
            log_lines: Vec::new(),
            started_at: current_epoch_secs(),
            finished_at: None,
            pid: std::process::id(),
            dry_run: false,
            first_write: false,
            cancellable: true,
        }
    }
}

/// Determine IPC directory: explicit -> `LAZYOS_INSTALL_STATE_DIR` -> `/run/lazyos-install` (if root)
/// -> `$XDG_RUNTIME_DIR/lazyos-install` -> `/tmp/lazyos-install`.
pub fn resolve_state_dir(explicit: Option<&str>) -> PathBuf {
    if let Some(dir) = explicit.filter(|s| !s.is_empty()) {
        let p = PathBuf::from(dir);
        let _ = ensure_dir_0755(&p);
        return p;
    }
    if let Ok(env_dir) = std::env::var("LAZYOS_INSTALL_STATE_DIR") {
        if !env_dir.is_empty() {
            let p = PathBuf::from(env_dir);
            let _ = ensure_dir_0755(&p);
            return p;
        }
    }
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        let p = PathBuf::from("/run/lazyos-install");
        if ensure_dir_0755(&p).is_ok() {
            return p;
        }
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(runtime).join("lazyos-install");
        if ensure_dir_0755(&p).is_ok() {
            return p;
        }
    }
    let p = PathBuf::from("/tmp/lazyos-install");
    let _ = ensure_dir_0755(&p);
    p
}

fn ensure_dir_0755(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, Permissions::from_mode(0o755))
}

pub fn shared_state_file_path() -> PathBuf {
    PathBuf::from("/run/lazyos-install/state.json")
}

pub fn state_file_path_for(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

pub fn state_file_path() -> PathBuf {
    let run = shared_state_file_path();
    if run.exists() {
        return run;
    }
    state_file_path_for(&resolve_state_dir(None))
}

pub fn log_file_path_for(dir: &Path) -> PathBuf {
    let now = current_epoch_secs();
    dir.join(format!("install-{now}.log"))
}

pub fn load_state_from(path: &Path) -> Option<WorkerState> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn load_state() -> Option<WorkerState> {
    let shared = shared_state_file_path();
    if shared.is_file() {
        if let Some(s) = load_state_from(&shared) {
            return Some(s);
        }
    }
    let dir = resolve_state_dir(None);
    load_state_from(&state_file_path_for(&dir))
}

/// Check if an existing detached installation is currently active in `/run/lazyos-install`.
pub fn active_running_state() -> Option<(PathBuf, WorkerState)> {
    let shared = shared_state_file_path();
    let state = load_state_from(&shared)?;
    if state.status != "running" {
        return None;
    }
    let proc_dir = PathBuf::from(format!("/proc/{}", state.pid));
    if proc_dir.exists() {
        Some((shared.parent()?.to_path_buf(), state))
    } else {
        None
    }
}

pub fn save_state_to(path: &Path, state: &WorkerState) -> io::Result<()> {
    let json = serde_json::to_string_pretty(state)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, json)?;
    let _ = fs::set_permissions(&tmp, Permissions::from_mode(0o644));
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn save_state(state: &WorkerState) -> io::Result<()> {
    let dir = resolve_state_dir(None);
    save_state_to(&state_file_path_for(&dir), state)
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Run installer worker (dry-run simulation or real pure-Rust installation).
pub fn run_worker(config: InstallConfig, cancel_token: Option<Arc<AtomicBool>>) -> io::Result<()> {
    let pw = if config.password.is_empty() {
        None
    } else {
        Some(config.password.clone())
    };
    run_worker_with_secret(config, pw.as_deref(), cancel_token)
}

/// Execute installation worker with optional transient password secret (never persisted).
pub fn run_worker_with_secret(
    config: InstallConfig,
    secret_password: Option<&str>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> io::Result<()> {
    let state_dir = resolve_state_dir(config.state_dir.as_deref());
    let state_path = state_file_path_for(&state_dir);
    let log_path = log_file_path_for(&state_dir);
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let _ = fs::set_permissions(&log_path, Permissions::from_mode(0o644));

    let device_info = DeviceInfo::load_from_machine();
    let mut state = WorkerState {
        phase_index: 0,
        phase_name: phase_label(0, device_info.boot_type).to_string(),
        percent: 0,
        status: "running".to_string(),
        error: None,
        log_lines: Vec::new(),
        started_at: current_epoch_secs(),
        finished_at: None,
        pid: std::process::id(),
        dry_run: config.dry_run,
        first_write: false,
        cancellable: true,
    };

    log_line(
        &state_path,
        &mut state,
        &mut log_file,
        &format!(
            "Starting LazyOS ({}) installation to {} ({})",
            match config.mode {
                InstallMode::Fresh => "fresh",
                InstallMode::Clone => "clone",
            },
            config.target_disk,
            device_info.boot_type.label()
        ),
    );

    if config.dry_run {
        return run_dry_run_simulation(
            &config,
            &device_info,
            &state_path,
            state,
            log_file,
            cancel_token,
        );
    }

    run_real_install(
        &config,
        &device_info,
        secret_password,
        &state_dir,
        &state_path,
        state,
        log_file,
        cancel_token,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_real_install(
    config: &InstallConfig,
    info: &DeviceInfo,
    secret_password: Option<&str>,
    state_dir: &Path,
    state_path: &Path,
    mut state: WorkerState,
    mut log_file: fs::File,
    cancel_token: Option<Arc<AtomicBool>>,
) -> io::Result<()> {
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        return fail_state(
            state_path,
            &mut state,
            &mut log_file,
            "Installer apply requires root privileges (run via sudo)",
        );
    }

    // Revalidate live root safety invariant immediately before touching anything.
    if let Ok(out) = std::process::Command::new("findmnt")
        .args(["-n", "-o", "SOURCE", "/"])
        .output()
    {
        let root_src = String::from_utf8_lossy(&out.stdout);
        if let Err(reason) = check_safety(&root_src, &config.target_disk) {
            return fail_state(state_path, &mut state, &mut log_file, &reason);
        }
    }

    let root_uuid = match disk::new_uuid_v4() {
        Ok(u) => u,
        Err(e) => return fail_state(state_path, &mut state, &mut log_file, &e),
    };
    let boot_uuid = match disk::new_uuid_v4() {
        Ok(u) => u,
        Err(e) => return fail_state(state_path, &mut state, &mut log_file, &e),
    };
    let root_uuid_str = root_uuid.clone();

    // Pre-build depthcharge kpart before first target write so verification failures abort safely.
    let prebuilt_kpart = if info.boot_type == BootType::Depthcharge {
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            &format!("Pre-building signed depthcharge FIT for root=UUID={root_uuid_str}"),
        );
        match boot::prebuild_depthcharge_kpart(info, &root_uuid_str, state_dir) {
            Ok(k) => {
                log_line(
                    state_path,
                    &mut state,
                    &mut log_file,
                    &format!("Verified FIT body with futility ({})", k.kernel_release),
                );
                Some(k)
            }
            Err(e) => return fail_state(state_path, &mut state, &mut log_file, &e),
        }
    } else {
        None
    };

    if is_cancelled(&cancel_token) {
        return fail_state(
            state_path,
            &mut state,
            &mut log_file,
            "Installation cancelled prior to first write",
        );
    }

    let udisks_active = disk::suspend_udisks2();
    let work_mnt = state_dir.join("target");

    let res = (|| -> Result<(), String> {
        disk::release_target(&config.target_disk)?;

        // Point of no return: mark first_write before altering partition tables.
        state.phase_index = 0;
        state.phase_name = phase_label(0, info.boot_type).to_string();
        state.percent = 8;
        state.first_write = true;
        state.cancellable = false;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            &format!(
                "Writing {} partition layout on {}",
                info.boot_type.as_str(),
                config.target_disk
            ),
        );

        let layout = disk::partition_and_format(&config.target_disk, info, &root_uuid, &boot_uuid)?;

        state.phase_index = 1;
        state.phase_name = phase_label(1, info.boot_type).to_string();
        state.percent = 22;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            &format!(
                "Formatted root ({}) and boot ({})",
                layout.root_part, layout.boot_part
            ),
        );

        let mounted = disk::MountedTarget::mount(&layout, &work_mnt)?;

        state.phase_index = 2;
        state.phase_name = phase_label(2, info.boot_type).to_string();
        state.percent = 25;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            &format!("Synchronizing live system to {}", mounted.root.display()),
        );

        identity::copy_rootfs_with_progress(
            &mounted.root,
            state_dir,
            config.regenerate_ssh_keys,
            |rsync_pct, detail| {
                let mapped = 25 + ((u16::from(rsync_pct) * 47) / 100) as u8;
                state.percent = mapped.min(72);
                log_line(
                    state_path,
                    &mut state,
                    &mut log_file,
                    &format!("rsync: {detail}"),
                );
            },
        )?;

        state.phase_index = 3;
        state.phase_name = phase_label(3, info.boot_type).to_string();
        state.percent = 76;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            "Applying /etc/fstab, machine-id, locale, and user identity",
        );
        disk::write_target_fstab(&mounted.root, &layout)?;
        identity::apply_target_identity(&mounted.root, config, secret_password)?;

        state.phase_index = 4;
        state.phase_name = phase_label(4, info.boot_type).to_string();
        state.percent = 86;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            &format!(
                "Installing {} bootloader artifacts",
                info.boot_type.as_str()
            ),
        );
        boot::install_boot_target(&mounted.root, info, &layout, prebuilt_kpart.as_ref())?;

        state.phase_index = 5;
        state.phase_name = phase_label(5, info.boot_type).to_string();
        state.percent = 94;
        log_line(
            state_path,
            &mut state,
            &mut log_file,
            "Writing /etc/lazyos/installation marker & checking filesystems",
        );
        identity::write_installation_marker(&mounted.root, &layout, config.mode)?;
        mounted.unmount()?;
        disk::verify_target_fs(&layout)?;
        Ok(())
    })();

    disk::restore_udisks2(udisks_active);

    match res {
        Ok(()) => {
            state.phase_index = 5;
            state.phase_name = phase_label(5, info.boot_type).to_string();
            state.percent = 100;
            state.status = "success".to_string();
            state.finished_at = Some(current_epoch_secs());
            log_line(
                state_path,
                &mut state,
                &mut log_file,
                &format!(
                    "LazyOS installation complete on {} ({})",
                    config.target_disk,
                    info.boot_type.label()
                ),
            );
            Ok(())
        }
        Err(err) => fail_state(state_path, &mut state, &mut log_file, &err),
    }
}

fn run_dry_run_simulation(
    config: &InstallConfig,
    info: &DeviceInfo,
    state_path: &Path,
    mut state: WorkerState,
    mut log_file: fs::File,
    cancel_token: Option<Arc<AtomicBool>>,
) -> io::Result<()> {
    let short = target_short_name(&config.target_disk);
    let step_lines: Vec<(usize, u8, String)> = match info.boot_type {
        BootType::Depthcharge => vec![
            (
                0,
                10,
                format!(
                    "Zeroing GPT areas and creating ChromeOS GPT table on {}",
                    config.target_disk
                ),
            ),
            (
                0,
                16,
                format!("Adding pmOS_kernel (128 MiB), pmOS_boot (512 MiB), and pmOS_root on {short}"),
            ),
            (
                1,
                25,
                format!("Formatting p2 (ext2, pmOS_boot) and p3 (ext4, pmOS_root) on {short}"),
            ),
            (2, 45, "rsync -aHAX: copying root filesystem to target mount".into()),
            (2, 68, "rsync -aHAX: synchronizing desktop configuration and binaries".into()),
            (
                3,
                78,
                format!(
                    "Applying user '{}', hostname '{}', timezone '{}', locale '{}'",
                    config.username, config.hostname, config.timezone, config.locale
                ),
            ),
            (4, 88, "Building & signing depthcharge FIT (depthchargectl build)".into()),
            (
                5,
                96,
                format!("Writing signed FIT to /dev/{short}p1 & verifying with futility + e2fsck"),
            ),
        ],
        BootType::Uefi => vec![
            (
                0,
                12,
                format!("Creating UEFI GPT layout on {} (lazyos_efi + lazyos_root)", config.target_disk),
            ),
            (
                1,
                25,
                format!("Formatting p1 (FAT32 LAZYOS_EFI) and p2 (ext4 lazyos_root) on {short}"),
            ),
            (2, 48, "rsync -aHAX: copying root filesystem to target mount".into()),
            (2, 68, "rsync -aHAX: synchronizing kernel, DTBs, and firmware".into()),
            (
                3,
                78,
                format!(
                    "Applying user '{}', hostname '{}', timezone '{}', locale '{}'",
                    config.username, config.hostname, config.timezone, config.locale
                ),
            ),
            (
                4,
                88,
                "Installing systemd-boot (BOOTAA64.EFI), loader/entries/lazyos.conf & /boot/efi/dtb/".into(),
            ),
            (
                5,
                96,
                "Writing /etc/lazyos/installation marker & running e2fsck verification".into(),
            ),
        ],
        BootType::Extlinux => vec![
            (
                0,
                12,
                format!(
                    "Creating GPT /boot + root layout on {} (offset {} MiB)",
                    config.target_disk, info.extlinux_start_mib
                ),
            ),
            (
                1,
                25,
                format!("Formatting p1 (ext2 lazyos_boot) and p2 (ext4 lazyos_root) on {short}"),
            ),
            (2, 48, "rsync -aHAX: copying root filesystem to target mount".into()),
            (2, 68, "rsync -aHAX: synchronizing kernel, DTBs, and desktop files".into()),
            (
                3,
                78,
                format!(
                    "Applying user '{}', hostname '{}', timezone '{}', locale '{}'",
                    config.username, config.hostname, config.timezone, config.locale
                ),
            ),
            (4, 88, "Writing /boot/extlinux/extlinux.conf (FDTDIR /dtbs/)".into()),
            (
                5,
                96,
                "Writing /etc/lazyos/installation marker & running e2fsck verification".into(),
            ),
        ],
    };

    for (phase_idx, pct, desc) in step_lines {
        if is_cancelled(&cancel_token) && !state.first_write {
            return fail_state(
                state_path,
                &mut state,
                &mut log_file,
                "Dry-run installation cancelled before first write",
            );
        }
        if phase_idx >= 1 && !state.first_write {
            state.first_write = true;
            state.cancellable = false;
        }
        state.phase_index = phase_idx;
        state.phase_name = phase_label(phase_idx, info.boot_type).to_string();
        state.percent = pct;
        log_line(state_path, &mut state, &mut log_file, &desc);
        thread::sleep(Duration::from_millis(250));
    }

    state.phase_index = 5;
    state.phase_name = phase_label(5, info.boot_type).to_string();
    state.percent = 100;
    state.status = "success".to_string();
    state.finished_at = Some(current_epoch_secs());
    log_line(
        state_path,
        &mut state,
        &mut log_file,
        "Dry-run installation verified successfully!",
    );
    Ok(())
}

fn is_cancelled(token: &Option<Arc<AtomicBool>>) -> bool {
    token.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
}

fn log_line(state_path: &Path, state: &mut WorkerState, log_file: &mut fs::File, msg: &str) {
    let line = format!("[{}] {}", state.phase_name, msg);
    let _ = writeln!(log_file, "{line}");
    let _ = log_file.flush();
    state.log_lines.push(line);
    if state.log_lines.len() > 1000 {
        state.log_lines.remove(0);
    }
    let _ = save_state_to(state_path, state);
}

fn fail_state(
    state_path: &Path,
    state: &mut WorkerState,
    log_file: &mut fs::File,
    err: &str,
) -> io::Result<()> {
    state.status = "failed".to_string();
    state.cancellable = false;
    state.error = Some(err.to_string());
    state.finished_at = Some(current_epoch_secs());
    log_line(state_path, state, log_file, &format!("ERROR: {err}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_json_serialization_round_trip() {
        let state = WorkerState {
            phase_index: 2,
            phase_name: PHASES[2].to_string(),
            percent: 50,
            status: "running".to_string(),
            error: None,
            log_lines: vec!["line 1".into(), "line 2".into()],
            started_at: 1000,
            finished_at: None,
            pid: 1234,
            dry_run: true,
            first_write: false,
            cancellable: true,
        };

        let json = serde_json::to_string(&state).unwrap();
        let parsed: WorkerState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, parsed);
    }

    #[test]
    fn all_phases_are_declared() {
        assert_eq!(PHASES.len(), 6);
        assert!(PHASES[0].contains("Partitioning"));
        assert!(PHASES[1].contains("Formatting"));
        assert!(PHASES[2].contains("Copying"));
        assert!(PHASES[3].contains("Configuring"));
        assert!(PHASES[4].contains("Generating"));
        assert!(PHASES[5].contains("Verifying"));
    }

    #[test]
    fn boot_type_phase_labels_differentiate_targets() {
        assert!(phase_label(0, BootType::Depthcharge).contains("ChromeOS GPT"));
        assert!(phase_label(4, BootType::Depthcharge).contains("depthcharge"));
        assert!(phase_label(0, BootType::Uefi).contains("UEFI GPT"));
        assert!(phase_label(4, BootType::Uefi).contains("systemd-boot"));
        assert!(phase_label(0, BootType::Extlinux).contains("GPT /boot"));
        assert!(phase_label(4, BootType::Extlinux).contains("extlinux"));
    }
}
