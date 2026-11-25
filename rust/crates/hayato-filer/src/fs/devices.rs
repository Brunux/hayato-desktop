//! Removable block device discovery, mounting, safe eject, and busy-holder detection.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::fs::fs_usage_bytes;

/// Fast, zero-subprocess fingerprint of connected block devices and mount table.
/// Reads only in-memory procfs/sysfs so we can detect hotplug/mount changes
/// without spawning `lsblk` until something actually changes.
pub fn block_mount_signature() -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/block") {
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort_unstable();
        for n in names {
            n.hash(&mut hasher);
        }
    }
    if let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") {
        mounts.hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub dev_path: String,
    pub parent_dev: String,
    pub label: String,
    pub mountpoint: Option<PathBuf>,
    pub fs_type: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl DeviceEntry {
    pub fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }

    pub fn usage_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }
}

#[derive(Deserialize)]
struct LsblkOut {
    blockdevices: Vec<LsblkDev>,
}

#[derive(Deserialize)]
struct LsblkDev {
    name: String,
    #[serde(default)]
    pkname: Option<String>,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    rm: bool,
    #[serde(default)]
    hotplug: bool,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    mountpoint: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    children: Vec<LsblkDev>,
}

pub fn list_removable_devices() -> Vec<DeviceEntry> {
    let Ok(out) = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,PKNAME,TYPE,RM,HOTPLUG,TRAN,MOUNTPOINT,LABEL,FSTYPE,SIZE",
        ])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_lsblk_json(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_lsblk_json(raw: &str) -> Vec<DeviceEntry> {
    let Ok(parsed) = serde_json::from_str::<LsblkOut>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dev in parsed.blockdevices {
        let is_removable =
            dev.rm || dev.hotplug || matches!(dev.tran.as_deref(), Some("usb" | "mmc" | "sd"));
        if !is_removable {
            continue;
        }
        let parent_path = format!("/dev/{}", dev.name);
        if dev.children.is_empty() {
            if let Some(entry) = to_device_entry(&dev, &parent_path) {
                out.push(entry);
            }
        } else {
            for child in &dev.children {
                if let Some(entry) = to_device_entry(child, &parent_path) {
                    out.push(entry);
                }
            }
        }
    }
    out
}

fn to_device_entry(dev: &LsblkDev, parent_path: &str) -> Option<DeviceEntry> {
    let fstype = dev.fstype.as_deref().unwrap_or("").trim();
    if fstype.is_empty() || fstype == "swap" || dev.kind == "loop" {
        return None;
    }
    let mountpoint = dev
        .mountpoint
        .as_deref()
        .filter(|m| !m.is_empty())
        .map(PathBuf::from);
    // Never show system root/boot partitions as removable user drives
    if let Some(mp) = &mountpoint {
        let s = mp.to_string_lossy();
        if s == "/" || s.starts_with("/boot") || s == "[SWAP]" {
            return None;
        }
    }
    let dev_path = format!("/dev/{}", dev.name);
    let parent_dev = dev
        .pkname
        .as_deref()
        .map(|p| format!("/dev/{p}"))
        .unwrap_or_else(|| parent_path.to_string());
    let label = dev
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| dev.name.clone());
    let size_bytes = dev.size.unwrap_or(0);
    let (used_bytes, total_bytes) = mountpoint
        .as_deref()
        .and_then(fs_usage_bytes)
        .unwrap_or((0, size_bytes));
    Some(DeviceEntry {
        dev_path,
        parent_dev,
        label,
        mountpoint,
        fs_type: fstype.to_string(),
        size_bytes,
        used_bytes,
        total_bytes,
    })
}

pub fn mount_device(dev_path: &str) -> Result<PathBuf, String> {
    let out = Command::new("udisksctl")
        .args(["mount", "-b", dev_path, "--no-user-interaction"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("udisksctl failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("Failed to mount {dev_path}")
        } else {
            err
        });
    }
    signal_waybar_drives();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // "Mounted /dev/sda1 at /media/user/USB."
    if let Some(at) = stdout.find(" at ") {
        let mp = stdout[at + 4..].trim().trim_end_matches('.');
        return Ok(PathBuf::from(mp));
    }
    // Fallback: re-query lsblk
    list_removable_devices()
        .into_iter()
        .find(|d| d.dev_path == dev_path)
        .and_then(|d| d.mountpoint)
        .ok_or_else(|| format!("Mounted {dev_path}, but mountpoint not found"))
}

pub fn eject_device(dev: &DeviceEntry) -> Result<String, String> {
    if let Some(mp) = &dev.mountpoint {
        let out = Command::new("udisksctl")
            .args(["unmount", "-b", &dev.dev_path, "--no-user-interaction"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("udisksctl failed: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if let Some(holder) = busy_holder(mp) {
                return Err(format!("{} is busy ({holder})", dev.label));
            }
            return Err(if err.is_empty() {
                format!("Failed to unmount {}", dev.label)
            } else {
                err
            });
        }
    }
    // Best-effort power-off so USB drive can be safely unplugged
    let _ = Command::new("udisksctl")
        .args(["power-off", "-b", &dev.parent_dev, "--no-user-interaction"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    signal_waybar_drives();
    Ok(format!("Safely ejected {}", dev.label))
}

fn busy_holder(mp: &Path) -> Option<String> {
    let out = Command::new("fuser")
        .args(["-m", &mp.to_string_lossy()])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let pids = String::from_utf8_lossy(&out.stdout);
    let pid = pids.split_whitespace().next()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(format!("{} (pid {pid})", comm.trim()))
}

fn signal_waybar_drives() {
    let _ = Command::new("pkill")
        .args(["-RTMIN+8", "-x", "waybar"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usb_mounted_and_unmounted_and_ignores_root() {
        let json = r#"{
          "blockdevices": [
            {
              "name": "mmcblk0", "type": "disk", "rm": false, "hotplug": false, "tran": "mmc",
              "children": [
                {"name": "mmcblk0p3", "pkname": "mmcblk0", "type": "part", "mountpoint": "/", "fstype": "ext4", "size": 60000000000}
              ]
            },
            {
              "name": "sda", "type": "disk", "rm": true, "hotplug": true, "tran": "usb",
              "children": [
                {"name": "sda1", "pkname": "sda", "type": "part", "mountpoint": "/media/brunux/SANDISK", "label": "SANDISK", "fstype": "vfat", "size": 32000000000},
                {"name": "sda2", "pkname": "sda", "type": "part", "mountpoint": null, "label": "BACKUP", "fstype": "ext4", "size": 16000000000}
              ]
            }
          ]
        }"#;
        let devs = parse_lsblk_json(json);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].label, "SANDISK");
        assert!(devs[0].is_mounted());
        assert_eq!(devs[1].label, "BACKUP");
        assert!(!devs[1].is_mounted());
    }
}
