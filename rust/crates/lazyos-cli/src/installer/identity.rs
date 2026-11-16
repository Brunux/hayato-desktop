//! Live filesystem synchronization (`rsync`) and target identity/account configuration.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::{InstallConfig, InstallMode};
use super::deviceinfo::BootType;
use super::disk::{DiskLayout, run_cmd};
use crate::version;

/// Mountpoints excluded from rsync and recreated empty on the target filesystem.
const EMPTY_MOUNT_DIRS: &[(&str, u32)] = &[
    ("dev", 0o755),
    ("proc", 0o555),
    ("sys", 0o555),
    ("run", 0o755),
    ("mnt", 0o755),
    ("media", 0o755),
];

/// RAII chroot bind mount guard for `/dev`, `/proc`, `/sys` inside target mount.
struct ChrootBinds {
    mnt: PathBuf,
    bound: Vec<&'static str>,
}

impl ChrootBinds {
    fn attach(mnt: &Path) -> Result<Self, String> {
        let mut guard = Self {
            mnt: mnt.to_path_buf(),
            bound: Vec::new(),
        };
        let specs = [
            ("dev", "none", true),
            ("proc", "proc", false),
            ("sys", "sysfs", false),
        ];
        for (name, fstype, bind) in specs {
            let target = mnt.join(name);
            let mut cmd = Command::new("mount");
            if bind {
                cmd.args(["--bind", &format!("/{name}"), &target.to_string_lossy()]);
            } else {
                cmd.args(["-t", fstype, fstype, &target.to_string_lossy()]);
            }
            let st = cmd
                .status()
                .map_err(|e| format!("mount {name} in target: {e}"))?;
            if !st.success() {
                return Err(format!("failed mounting /{name} inside {}", mnt.display()));
            }
            guard.bound.push(name);
        }
        Ok(guard)
    }
}

impl Drop for ChrootBinds {
    fn drop(&mut self) {
        for name in self.bound.iter().rev() {
            let p = self.mnt.join(name);
            let _ = Command::new("umount").arg("-lf").arg(&p).status();
        }
    }
}

/// Build `rsync` arguments for live `/` synchronization, excluding runtime mounts,
/// `/boot/efi` (FAT32 ESP mounted before sync and populated by `install_boot_target`),
/// and source-machine identity/cache state.
pub fn build_copy_rootfs_args(
    mnt: &Path,
    state_dir: &Path,
    regenerate_ssh_keys: bool,
) -> Vec<String> {
    let mnt_exclude = format!("--exclude={}", mnt.display());
    let state_exclude = format!("--exclude={}", state_dir.display());
    let mut args = vec![
        "-aHAX".to_string(),
        "--delete".to_string(),
        "--info=progress2".to_string(),
        "--exclude=/boot/efi".to_string(),
        "--exclude=/proc".to_string(),
        "--exclude=/sys".to_string(),
        "--exclude=/dev".to_string(),
        "--exclude=/run".to_string(),
        "--exclude=/mnt".to_string(),
        "--exclude=/media".to_string(),
        mnt_exclude,
        state_exclude,
        "--exclude=/tmp/*".to_string(),
        "--exclude=/var/tmp/*".to_string(),
        "--exclude=/var/cache/*".to_string(),
        "--exclude=/var/log/journal/*".to_string(),
        "--exclude=/etc/machine-id".to_string(),
        "--exclude=/var/lib/dbus/machine-id".to_string(),
        "--exclude=/etc/lazyos/installation".to_string(),
    ];
    if regenerate_ssh_keys {
        args.push("--exclude=/etc/ssh/ssh_host_*".to_string());
    }
    args.push("/".to_string());
    args.push(format!("{}/", mnt.display()));
    args
}

/// Synchronize live `/` to `mnt` with real-time `--info=progress2` reporting.
pub fn copy_rootfs_with_progress<F>(
    mnt: &Path,
    state_dir: &Path,
    regenerate_ssh_keys: bool,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u8, &str),
{
    let args = build_copy_rootfs_args(mnt, state_dir, regenerate_ssh_keys);

    let mut child = Command::new("rsync")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn rsync: {e}"))?;

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(256);
        let mut last_pct: u8 = 0;
        loop {
            buf.clear();
            let n = read_until_cr_or_lf(&mut reader, &mut buf);
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            if let Some((pct, detail)) = parse_rsync_progress2(&line) {
                if pct != last_pct {
                    last_pct = pct;
                    on_progress(pct, &detail);
                }
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait rsync: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("rsync failed ({}): {stderr}", output.status));
    }

    // Recreate essential empty mountpoint dirs dropped by --exclude rules.
    for &(dir, mode) in EMPTY_MOUNT_DIRS {
        let full = mnt.join(dir);
        fs::create_dir_all(&full).map_err(|e| format!("create {}: {e}", full.display()))?;
        fs::set_permissions(&full, Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {}: {e}", full.display()))?;
    }

    if !mnt.join("dev").is_dir() {
        return Err("target /dev missing after copy".to_string());
    }
    if !mnt.join("sbin/init").exists() && !mnt.join("usr/sbin/init").exists() {
        return Err("target /sbin/init missing after copy".to_string());
    }
    Ok(())
}

/// Apply machine identity cleanup and optional Fresh user/timezone/locale/keymap/hostname setup.
pub fn apply_target_identity(
    mnt: &Path,
    config: &InstallConfig,
    secret_password: Option<&str>,
) -> Result<(), String> {
    // Always write empty read-only /etc/machine-id and remove stale dbus machine-id so first boot creates fresh IDs.
    let mid_path = mnt.join("etc/machine-id");
    fs::write(&mid_path, "").map_err(|e| format!("reset {}: {e}", mid_path.display()))?;
    let _ = fs::set_permissions(&mid_path, Permissions::from_mode(0o444));
    let _ = fs::remove_file(mnt.join("var/lib/dbus/machine-id"));

    if config.regenerate_ssh_keys {
        if let Ok(entries) = fs::read_dir(mnt.join("etc/ssh")) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("ssh_host_"))
                {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    if config.mode == InstallMode::Clone {
        return Ok(());
    }

    let _binds = ChrootBinds::attach(mnt)?;

    apply_timezone(mnt, &config.timezone)?;
    apply_locale(mnt, &config.locale)?;
    apply_hostname(mnt, &config.hostname)?;
    apply_primary_user(mnt, config, secret_password)?;
    apply_keyboard_layout(mnt, &config.username, &config.keymap)?;

    Ok(())
}

fn apply_timezone(mnt: &Path, timezone: &str) -> Result<(), String> {
    let tz_rel = format!("usr/share/zoneinfo/{timezone}");
    let tz_chosen = if mnt.join(&tz_rel).exists() {
        timezone
    } else {
        "UTC"
    };
    let tz_file = mnt.join("etc/timezone");
    fs::write(&tz_file, format!("{tz_chosen}\n")).map_err(|e| format!("write timezone: {e}"))?;

    let localtime = mnt.join("etc/localtime");
    let _ = fs::remove_file(&localtime);
    symlink(format!("/usr/share/zoneinfo/{tz_chosen}"), &localtime)
        .map_err(|e| format!("link /etc/localtime: {e}"))?;
    Ok(())
}

fn apply_locale(mnt: &Path, locale: &str) -> Result<(), String> {
    let loc_gen_path = mnt.join("etc/locale.gen");
    let existing = fs::read_to_string(&loc_gen_path).unwrap_or_default();
    let updated = render_locale_gen(&existing, locale);
    fs::write(&loc_gen_path, updated).map_err(|e| format!("write /etc/locale.gen: {e}"))?;

    let _ = run_cmd("chroot", &[&mnt.to_string_lossy(), "locale-gen"]);

    let lang_line = format!("LANG={locale}\n");
    fs::write(mnt.join("etc/default/locale"), &lang_line)
        .map_err(|e| format!("write /etc/default/locale: {e}"))?;
    fs::write(mnt.join("etc/locale.conf"), &lang_line)
        .map_err(|e| format!("write /etc/locale.conf: {e}"))?;
    Ok(())
}

fn apply_hostname(mnt: &Path, hostname: &str) -> Result<(), String> {
    fs::write(mnt.join("etc/hostname"), format!("{hostname}\n"))
        .map_err(|e| format!("write /etc/hostname: {e}"))?;
    fs::write(mnt.join("etc/hosts"), render_hosts(hostname))
        .map_err(|e| format!("write /etc/hosts: {e}"))?;
    Ok(())
}

fn apply_primary_user(
    mnt: &Path,
    config: &InstallConfig,
    secret_password: Option<&str>,
) -> Result<(), String> {
    let mnt_str = mnt.to_string_lossy().into_owned();
    let live_uid1000 = find_uid_1000_user(&mnt.join("etc/passwd"));
    let target_user = config.username.trim();
    let fullname = config.fullname.trim();

    if let Some(old_user) = live_uid1000 {
        if old_user != target_user {
            let new_home = format!("/home/{target_user}");
            run_cmd(
                "chroot",
                &[
                    &mnt_str,
                    "usermod",
                    "-l",
                    target_user,
                    "-d",
                    &new_home,
                    "-m",
                    "-c",
                    fullname,
                    &old_user,
                ],
            )?;
            let _ = Command::new("chroot")
                .args([&mnt_str, "groupmod", "-n", target_user, &old_user])
                .status();

            let old_home = format!("/home/{old_user}");
            rewrite_home_prefix(&mnt.join("etc/sway"), &old_home, &new_home);
            rewrite_home_prefix(&mnt.join("usr/share/applications"), &old_home, &new_home);
            rewrite_home_prefix(
                &mnt.join(format!("home/{target_user}/.config")),
                &old_home,
                &new_home,
            );
            let _ = fs::remove_file(mnt.join(format!("etc/sudoers.d/99-{old_user}")));
            let _ = fs::remove_file(mnt.join("var/lib/lazyos/greeter/state.toml"));
        } else if !fullname.is_empty() {
            let _ = Command::new("chroot")
                .args([&mnt_str, "usermod", "-c", fullname, target_user])
                .status();
        }
    } else {
        run_cmd(
            "chroot",
            &[
                &mnt_str,
                "useradd",
                "-m",
                "-s",
                "/bin/bash",
                "-G",
                "sudo,video,audio,input,render,netdev,plugdev",
                "-c",
                fullname,
                target_user,
            ],
        )?;
    }

    if let Some(pw) = secret_password.filter(|s| !s.is_empty()) {
        set_chroot_password(&mnt_str, target_user, pw)?;
    }

    let sudoers_dir = mnt.join("etc/sudoers.d");
    let _ = fs::create_dir_all(&sudoers_dir);
    let dropin = sudoers_dir.join(format!("99-{target_user}"));
    fs::write(&dropin, "%sudo ALL=(ALL) NOPASSWD:ALL\n")
        .map_err(|e| format!("write sudoers drop-in: {e}"))?;
    fs::set_permissions(&dropin, Permissions::from_mode(0o440))
        .map_err(|e| format!("chmod sudoers drop-in: {e}"))?;
    Ok(())
}

fn set_chroot_password(mnt_str: &str, username: &str, password: &str) -> Result<(), String> {
    let mut child = Command::new("chroot")
        .args([mnt_str, "chpasswd"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn chroot chpasswd: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("{username}:{password}\n").as_bytes())
            .map_err(|e| format!("write password to chpasswd stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait chpasswd: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("chpasswd failed: {stderr}"));
    }
    Ok(())
}

fn apply_keyboard_layout(mnt: &Path, username: &str, keymap: &str) -> Result<(), String> {
    let etc_default = mnt.join("etc/default");
    fs::create_dir_all(&etc_default).map_err(|e| format!("create etc/default: {e}"))?;
    fs::write(etc_default.join("keyboard"), render_keyboard(keymap))
        .map_err(|e| format!("write /etc/default/keyboard: {e}"))?;

    let sway_cfg_dir = mnt.join(format!("home/{username}/.config/sway/config.d"));
    if sway_cfg_dir.parent().is_some_and(|p| p.exists()) {
        fs::create_dir_all(&sway_cfg_dir).map_err(|e| format!("create sway config.d: {e}"))?;
        let drop_in = sway_cfg_dir.join("90-keyboard-layout.conf");
        fs::write(&drop_in, render_sway_keyboard(keymap))
            .map_err(|e| format!("write 90-keyboard-layout.conf: {e}"))?;
        let mnt_root = mnt.to_string_lossy().into_owned();
        let owner = format!("{username}:{username}");
        let sway_home = format!("/home/{username}/.config/sway");
        let _ = Command::new("chroot")
            .args([&mnt_root, "chown", "-R", &owner, &sway_home])
            .status();
    }
    Ok(())
}

/// Write `/etc/lazyos/installation` last to mark the installed target cleanly.
pub fn write_installation_marker(
    mnt: &Path,
    layout: &DiskLayout,
    mode: InstallMode,
) -> Result<(), String> {
    let dir = mnt.join("etc/lazyos");
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let marker = render_installation_marker(
        version::version(),
        mode,
        &layout.target_disk,
        layout.boot_type,
        &layout.root_uuid,
        &layout.boot_uuid,
    );
    let path = dir.join("installation");
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("open installation marker: {e}"))?;
    f.write_all(marker.as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("sync installation marker: {e}"))
}

pub fn parse_rsync_progress2(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let pct_tok = tokens.iter().find(|t| t.ends_with('%'))?;
    let pct_num = pct_tok.trim_end_matches('%').parse::<u8>().ok()?;
    let bytes_tok = tokens.first().copied().unwrap_or("");
    let rate_tok = tokens
        .iter()
        .find(|t| t.ends_with("/s"))
        .copied()
        .unwrap_or("");
    let detail = if !rate_tok.is_empty() {
        format!("{bytes_tok} ({pct_num}%, {rate_tok})")
    } else {
        format!("{bytes_tok} ({pct_num}%)")
    };
    Some((pct_num.min(100), detail))
}

pub fn render_hosts(hostname: &str) -> String {
    format!(
        "127.0.0.1\tlocalhost\n127.0.1.1\t{hostname}\n::1\t\tlocalhost ip6-localhost ip6-loopback\n"
    )
}

pub fn render_locale_gen(existing: &str, locale: &str) -> String {
    let commented = format!("# {locale} ");
    let active = format!("{locale} ");
    let mut found = false;
    let mut out = String::with_capacity(existing.len() + 32);

    for line in existing.lines() {
        if line.starts_with(&active) {
            found = true;
            out.push_str(line);
        } else if line.starts_with(&commented) {
            found = true;
            out.push_str(line.trim_start_matches("# "));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out.push_str(&format!("{locale} UTF-8\n"));
    }
    out
}

pub fn render_keyboard(layout: &str) -> String {
    format!(
        "XKBMODEL=\"pc105\"\nXKBLAYOUT=\"{layout}\"\nXKBVARIANT=\"\"\nXKBOPTIONS=\"\"\nBACKSPACE=\"guess\"\n"
    )
}

pub fn render_sway_keyboard(layout: &str) -> String {
    format!("# Written by lazyos installer\ninput \"type:keyboard\" xkb_layout \"{layout}\"\n")
}

pub fn render_installation_marker(
    ver: &str,
    mode: InstallMode,
    target_disk: &str,
    boot_type: BootType,
    root_uuid: &str,
    boot_uuid: &str,
) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": 1,
        "version": ver,
        "mode": mode,
        "target": target_disk,
        "boot_type": boot_type.as_str(),
        "root_uuid": root_uuid,
        "boot_uuid": boot_uuid,
        "installed_at_epoch": ts,
    }))
    .unwrap_or_else(|_| "{}".to_string())
        + "\n"
}

pub fn find_uid_1000_user(passwd_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(passwd_path).ok()?;
    for line in raw.lines() {
        let cols: Vec<&str> = line.split(':').collect();
        if cols.len() >= 3 && cols[2] == "1000" {
            return Some(cols[0].to_string());
        }
    }
    None
}

fn rewrite_home_prefix(dir: &Path, old_home: &str, new_home: &str) {
    if !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rewrite_home_prefix(&p, old_home, new_home);
        } else if p.is_file() {
            if let Ok(txt) = fs::read_to_string(&p) {
                if txt.contains(old_home) {
                    let _ = fs::write(&p, txt.replace(old_home, new_home));
                }
            }
        }
    }
}

fn read_until_cr_or_lf<R: BufRead>(r: &mut R, out: &mut Vec<u8>) -> usize {
    let mut total = 0;
    while let Ok(avail) = r.fill_buf() {
        if avail.is_empty() {
            break;
        }
        let pos = avail.iter().position(|&b| b == b'\n' || b == b'\r');
        match pos {
            Some(idx) => {
                out.extend_from_slice(&avail[..idx]);
                let consume = idx + 1;
                r.consume(consume);
                total += consume;
                break;
            }
            None => {
                let len = avail.len();
                out.extend_from_slice(avail);
                r.consume(len);
                total += len;
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rsync_progress2_percent_and_rate() {
        let line = "  1,420,820,480  48%   62.14MB/s    0:00:22 (xfr#1820, to-chk=41/98210)";
        let (pct, detail) = parse_rsync_progress2(line).unwrap();
        assert_eq!(pct, 48);
        assert!(detail.contains("48%"));
        assert!(detail.contains("62.14MB/s"));
        assert!(parse_rsync_progress2("sending incremental file list").is_none());
    }

    #[test]
    fn uncomments_locale_gen_and_appends_when_missing() {
        let existing = "# en_US.UTF-8 UTF-8\n# es_ES.UTF-8 UTF-8\n";
        let out = render_locale_gen(existing, "es_ES.UTF-8");
        assert!(out.contains("\nes_ES.UTF-8 UTF-8\n"));
        let out2 = render_locale_gen(existing, "ja_JP.UTF-8");
        assert!(out2.ends_with("ja_JP.UTF-8 UTF-8\n"));
    }

    #[test]
    fn generates_marker_with_schema_and_boot_type() {
        let raw = render_installation_marker(
            "0.5.0",
            InstallMode::Fresh,
            "/dev/nvme0n1",
            BootType::Uefi,
            "1111-2222",
            "AAAA-BBBB",
        );
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["boot_type"], "uefi");
        assert_eq!(v["target"], "/dev/nvme0n1");
        assert_eq!(v["root_uuid"], "1111-2222");
    }

    #[test]
    fn copy_rootfs_args_exclude_esp_and_identity_state() {
        let args = build_copy_rootfs_args(
            Path::new("/run/lazyos-install/mnt"),
            Path::new("/run/lazyos-install"),
            true,
        );
        assert!(args.iter().any(|a| a == "--exclude=/boot/efi"));
        assert!(args.iter().any(|a| a == "--exclude=/etc/machine-id"));
        assert!(
            args.iter()
                .any(|a| a == "--exclude=/etc/lazyos/installation")
        );
        assert!(args.iter().any(|a| a == "--exclude=/etc/ssh/ssh_host_*"));
        assert!(
            args.iter()
                .any(|a| a == "--exclude=/run/lazyos-install/mnt")
        );

        let no_ssh = build_copy_rootfs_args(
            Path::new("/mnt/target"),
            Path::new("/run/lazyos-install"),
            false,
        );
        assert!(!no_ssh.iter().any(|a| a == "--exclude=/etc/ssh/ssh_host_*"));
        assert!(no_ssh.iter().any(|a| a == "--exclude=/boot/efi"));
    }
}
