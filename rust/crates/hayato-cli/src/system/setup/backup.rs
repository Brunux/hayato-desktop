//! Security, privacy, and backup/recovery action helpers for the Setup Assistant.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::system::setup::probes::free_gb;

/// Validate an external mountpoint for `/usr/libexec/hayato/backup-system.sh`.
pub fn validate_system_backup_target(mountpoint: &Path) -> Result<PathBuf, String> {
    if mountpoint == Path::new("/") || mountpoint.starts_with("/boot") {
        return Err("Refusing to write a system backup onto the OS disk".to_string());
    }
    if !mountpoint.is_dir() {
        return Err(format!("{} is not mounted", mountpoint.display()));
    }
    let avail = free_gb(mountpoint);
    if avail < 2.0 {
        return Err(format!(
            "Target has only {avail:.1} GiB free (need at least 2.0 GiB)"
        ));
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    Ok(mountpoint.join(format!("hayato-backup-{stamp}")))
}

/// Tailored bare-metal recovery guidance matching `deviceinfo_boot_type`.
pub fn recovery_guidance(boot_type: &str) -> &'static str {
    match boot_type.to_ascii_lowercase().as_str() {
        "depthcharge" => {
            "Chromebook Depthcharge: keep a rescue microSD flashed with hayato.img.xz; press Ctrl+U at boot to recover or run restore-system.sh."
        }
        "uefi" => {
            "ARM64 UEFI: keep a rescue USB drive flashed with hayato.img.xz to boot systemd-boot recovery or restore from backup."
        }
        "extlinux" => {
            "SBC Extlinux: keep a spare microSD card with hayato.img.xz to boot U-Boot recovery or restore your rootfs."
        }
        _ => {
            "Keep a rescue USB/SD flashed with hayato.img.xz to boot live and run /usr/libexec/hayato/restore-system.sh."
        }
    }
}

pub fn toggle_ufw(currently_active: bool) -> Result<String, String> {
    let action = if currently_active {
        "disable"
    } else {
        "enable"
    };
    let out = Command::new("sudo")
        .args(["-n", "ufw", "--force", action])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ufw failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    if currently_active {
        Ok("Firewall (UFW) disabled".to_string())
    } else {
        Ok("Firewall (UFW) enabled (deny incoming by default)".to_string())
    }
}

pub fn set_clipboard_enabled(enable: bool) -> Result<String, String> {
    let path = config_dir().join("hayato/clipboard.conf");
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let val = if enable { "yes" } else { "no" };
    fs::write(&path, format!("enabled={val}\n")).map_err(|e| e.to_string())?;
    if !enable {
        let _ = Command::new("cliphist")
            .arg("wipe")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("pkill")
            .args(["-f", "wl-paste.*cliphist"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok("Clipboard history disabled & wiped".to_string())
    } else {
        Ok("Clipboard history enabled".to_string())
    }
}

pub fn set_automount_enabled(enable: bool) -> Result<String, String> {
    let path = config_dir().join("hayato/automount.conf");
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let val = if enable { "yes" } else { "no" };
    fs::write(&path, format!("enabled={val}\n")).map_err(|e| e.to_string())?;
    Ok(if enable {
        "USB auto-mount enabled".to_string()
    } else {
        "USB auto-mount disabled (manual mount in Files)".to_string()
    })
}

pub fn toggle_mic_mute() -> Result<String, String> {
    let ok = Command::new("pactl")
        .args(["set-source-mute", "@DEFAULT_SOURCE@", "toggle"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok("Toggled default microphone mute".to_string())
    } else {
        Err("pactl could not toggle microphone mute".to_string())
    }
}

pub fn set_default_ai_posture(posture: &str) -> Result<String, String> {
    let path = config_dir().join("hayato/agents/default");
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    fs::write(&path, format!("{posture}\n")).map_err(|e| e.to_string())?;
    Ok(format!("Default AI posture set to: {posture}"))
}

pub fn run_profile_backup(dest_dir: &Path) -> Result<String, String> {
    let _ = fs::create_dir_all(dest_dir);
    let script = home_dir().join(".config/bin/hayato-profile");
    let prog = if script.is_file() {
        script.to_string_lossy().into_owned()
    } else {
        "hayato-profile".to_string()
    };
    let out = Command::new(&prog)
        .args(["export", "--out"])
        .arg(dest_dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("hayato-profile failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "Profile backup failed".to_string()
        } else {
            err
        });
    }
    Ok(format!(
        "Encrypted profile backup saved to {}",
        dest_dir.display()
    ))
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_system_backup_to_root() {
        assert!(validate_system_backup_target(Path::new("/")).is_err());
        assert!(validate_system_backup_target(Path::new("/boot")).is_err());
    }

    #[test]
    fn recovery_guidance_tailors_to_boot_type() {
        assert!(recovery_guidance("depthcharge").contains("Ctrl+U"));
        assert!(recovery_guidance("uefi").contains("systemd-boot"));
        assert!(recovery_guidance("extlinux").contains("U-Boot"));
    }
}
