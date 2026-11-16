//! Pre-flight checks, hardware discovery, target selection, and safety verification.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::deviceinfo::{BootType, DeviceInfo};

/// Verification status for a critical command-line tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: &'static str,
    pub available: bool,
    pub description: &'static str,
}

/// Candidate internal/external whole block device discovered during preflight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetCandidate {
    pub path: String,
    pub short_name: String,
    pub size_human: String,
    pub model: String,
    pub tran: Option<String>,
    pub removable: bool,
}

/// Collected facts about the running system and candidate target storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preflight {
    pub device_info: DeviceInfo,
    pub soc_model: String,
    pub total_ram: String,
    pub power_source: String,
    pub root_source: String,
    pub target_disk: String,
    pub target_size: String,
    pub candidates: Vec<TargetCandidate>,
    pub is_safe: bool,
    pub refusal_reason: Option<String>,
    pub installed_notice: Option<String>,
    pub tools: Vec<ToolStatus>,
}

const COMMON_TOOLS: &[(&str, &str)] = &[
    ("parted", "GPT table generator"),
    ("mkfs.ext4", "Root partition filesystem (ext4)"),
    ("rsync", "File copy engine"),
    ("blkid", "Block device UUID identification"),
    ("blockdev", "Block device sizing"),
    ("lsblk", "Block device inspection"),
    ("partx", "Kernel partition re-reader"),
    ("udevadm", "Device node synchronization"),
    ("e2fsck", "Filesystem integrity check"),
];

pub fn required_tools_for(boot_type: BootType) -> Vec<(&'static str, &'static str)> {
    let mut list = COMMON_TOOLS.to_vec();
    match boot_type {
        BootType::Depthcharge => {
            list.push(("cgpt", "ChromeOS partition table manager"));
            list.push(("mkfs.ext2", "Boot partition filesystem (ext2)"));
            list.push(("depthchargectl", "ChromeOS kernel FIT builder"));
            list.push(("futility", "ChromeOS vboot signature verifier"));
        }
        BootType::Uefi => {
            list.push(("mkfs.vfat", "EFI System Partition filesystem (FAT32)"));
        }
        BootType::Extlinux => {
            list.push(("mkfs.ext2", "Boot partition filesystem (ext2)"));
        }
    }
    list
}

impl Preflight {
    /// Gather real hardware facts and evaluate safety invariants without hardcoding board names.
    pub fn from_machine() -> Self {
        let device_info = DeviceInfo::load_from_machine();
        let root_source = detect_root_source();
        let boot_source = detect_mount_source("/boot");
        let efi_source = detect_mount_source("/boot/efi");

        let mut excluded_disks = Vec::new();
        for src in [
            Some(&root_source),
            boot_source.as_ref(),
            efi_source.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(parent) = parent_disk_name(src) {
                if !excluded_disks.contains(&parent) {
                    excluded_disks.push(parent);
                }
            }
        }
        if let Some(kern_disk) = running_kern_guid_disk() {
            if !excluded_disks.contains(&kern_disk) {
                excluded_disks.push(kern_disk);
            }
        }

        let candidates = discover_candidates(&excluded_disks);
        let env_block = std::env::var("DEVICE_BLOCK")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let target = select_default_target(
            env_block.as_deref(),
            &device_info,
            &candidates,
            &excluded_disks,
        );

        let soc_model = detect_soc_model(&device_info);
        let total_ram = detect_memory();
        let power_source = detect_power();
        let target_size = candidates
            .iter()
            .find(|c| c.path == target)
            .map(|c| c.size_human.clone())
            .unwrap_or_else(|| detect_target_size(&target));

        let mut safety_err = check_safety(&root_source, &target).err();
        if safety_err.is_none() {
            if let Some(ref bsrc) = boot_source {
                if let Err(e) = check_safety(bsrc, &target) {
                    safety_err = Some(e);
                }
            }
        }
        if safety_err.is_none() {
            if let Some(ref esrc) = efi_source {
                if let Err(e) = check_safety(esrc, &target) {
                    safety_err = Some(e);
                }
            }
        }

        let (is_safe, refusal_reason) = match safety_err {
            None => (true, None),
            Some(reason) => (false, Some(reason)),
        };

        let installed_notice = detect_installed_marker(Path::new("/"));

        let tools = required_tools_for(device_info.boot_type)
            .into_iter()
            .map(|(name, desc)| ToolStatus {
                name,
                available: has_binary(name),
                description: desc,
            })
            .collect();

        Self {
            device_info,
            soc_model,
            total_ram,
            power_source,
            root_source,
            target_disk: target,
            target_size,
            candidates,
            is_safe,
            refusal_reason,
            installed_notice,
            tools,
        }
    }

    /// Machine-readable check summary for `lazyos installer --check`.
    pub fn check_summary(&self) -> (bool, String) {
        if !self.is_safe {
            let reason = self
                .refusal_reason
                .as_deref()
                .unwrap_or("Unsafe target disk configuration");
            (
                false,
                format!(
                    "unsafe: boot={} target={} root_source={} ({})",
                    self.device_info.boot_type.as_str(),
                    self.target_disk,
                    self.root_source,
                    reason
                ),
            )
        } else {
            let missing_count = self.tools.iter().filter(|t| !t.available).count();
            let tool_str = if missing_count == 0 {
                "tools=ok".to_string()
            } else {
                format!("tools_missing={missing_count}")
            };
            (
                true,
                format!(
                    "ok: boot={} target={} source={} soc=\"{}\" mem={} power=\"{}\" {}",
                    self.device_info.boot_type.as_str(),
                    self.target_disk,
                    self.root_source,
                    self.soc_model,
                    self.total_ram,
                    self.power_source,
                    tool_str
                ),
            )
        }
    }
}

/// Choose default target disk: `DEVICE_BLOCK` -> `deviceinfo_install_target` hint ->
/// fixed non-USB candidate -> any safe candidate -> `/dev/mmcblk0` test fallback.
pub fn select_default_target(
    env_block: Option<&str>,
    info: &DeviceInfo,
    candidates: &[TargetCandidate],
    excluded_disks: &[String],
) -> String {
    if let Some(explicit) = env_block {
        return explicit.trim().to_string();
    }
    if let Some(ref hint) = info.install_target {
        let hint_short = hint.trim_start_matches("/dev/");
        if candidates.iter().any(|c| c.path == *hint)
            || (candidates.is_empty() && !excluded_disks.iter().any(|d| d == hint_short))
        {
            return hint.clone();
        }
    }
    if let Some(fixed) = candidates
        .iter()
        .find(|c| !c.removable && c.tran.as_deref() != Some("usb"))
    {
        return fixed.path.clone();
    }
    if let Some(first) = candidates.first() {
        return first.path.clone();
    }
    info.install_target
        .clone()
        .unwrap_or_else(|| "/dev/mmcblk0".to_string())
}

/// Pseudo, SPI flash, and eMMC hardware-boot disks that must never be offered.
pub fn is_excluded_disk(name: &str) -> bool {
    name.starts_with("mtd")
        || name.contains("boot0")
        || name.contains("boot1")
        || name.contains("rpmb")
        || name.starts_with("zram")
        || name.starts_with("loop")
        || name.starts_with("dm-")
        || name.starts_with("md")
        || name.starts_with("ram")
        || name.starts_with("sr")
        || name.starts_with("nbd")
        || name.starts_with("rbd")
        || name.starts_with("fd")
}

/// Parse `lsblk -dnro NAME,TYPE,SIZE,MODEL,TRAN,RM` output into candidate whole disks.
pub fn parse_lsblk_candidates(listing: &str, excluded_disks: &[String]) -> Vec<TargetCandidate> {
    let mut out = Vec::new();
    for line in listing.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else { continue };
        let Some(kind) = fields.next() else { continue };
        if kind != "disk" || is_excluded_disk(name) || excluded_disks.iter().any(|d| d == name) {
            continue;
        }
        let rest: Vec<&str> = fields.collect();
        if rest.is_empty() {
            continue;
        }
        let rm = rest.last().copied().unwrap_or("0") == "1";
        let before_rm = &rest[..rest.len().saturating_sub(1)];
        let (size, model, tran) = match before_rm {
            [] => (String::new(), "unknown".to_string(), None),
            [sz] => ((*sz).replace("\\x20", " "), "unknown".to_string(), None),
            [sz, body @ ..] => {
                let maybe_tran = body.last().copied();
                let is_tran = matches!(
                    maybe_tran,
                    Some(
                        "usb" | "mmc" | "nvme" | "sata" | "ata" | "scsi" | "virtio" | "sas" | "spi"
                    )
                );
                let tran = is_tran.then(|| maybe_tran.unwrap().to_string());
                let model_slice = &body[..body.len() - usize::from(is_tran)];
                let model = if model_slice.is_empty() {
                    "unknown".to_string()
                } else {
                    model_slice.join(" ").replace("\\x20", " ")
                };
                ((*sz).replace("\\x20", " "), model, tran)
            }
        };

        out.push(TargetCandidate {
            path: format!("/dev/{name}"),
            short_name: name.to_string(),
            size_human: size,
            model,
            tran,
            removable: rm,
        });
    }
    out
}

fn discover_candidates(excluded_disks: &[String]) -> Vec<TargetCandidate> {
    let Ok(out) = Command::new("lsblk")
        .args(["-dnro", "NAME,TYPE,SIZE,MODEL,TRAN,RM"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parsed = parse_lsblk_candidates(&text, excluded_disks);
    for c in &mut parsed {
        let dev_rm_path = format!("/sys/block/{}/device/removable", c.short_name);
        if let Ok(state) = fs::read_to_string(&dev_rm_path) {
            if state.trim() == "removable" {
                c.removable = true;
            }
        }
    }
    parsed
}

/// Enforce the safety invariant: refusing to touch target if active OS runs from it.
pub fn check_safety(root_source: &str, target_disk: &str) -> Result<(), String> {
    let clean_root = root_source.trim();
    let clean_target = target_disk.trim().trim_end_matches('/');

    if clean_root.is_empty() || clean_target.is_empty() {
        return Ok(());
    }

    if let Some(suffix) = clean_root.strip_prefix(clean_target) {
        let same_disk = suffix.is_empty()
            || suffix.starts_with('p')
            || suffix.starts_with("part")
            || suffix.chars().next().is_some_and(|c| c.is_ascii_digit());
        if same_disk {
            return Err(format!(
                "System is already booted from target disk ({clean_target}). Boot from external USB or microSD media to install."
            ));
        }
    }

    Ok(())
}

pub fn parent_disk_name(source: &str) -> Option<String> {
    let src = source.trim();
    if !src.starts_with("/dev/") {
        return None;
    }
    if let Ok(out) = Command::new("lsblk")
        .args(["-snro", "NAME,TYPE", src])
        .output()
    {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines() {
                let mut fields = line.split_whitespace();
                if let (Some(name), Some(kind)) = (fields.next(), fields.next()) {
                    if kind == "disk" {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    // Fallback heuristic if lsblk is unavailable
    let short = src.strip_prefix("/dev/")?;
    if let Some((base, part)) = short.rsplit_once('p') {
        if !base.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
            return Some(base.to_string());
        }
    }
    let trimmed = short.trim_end_matches(|c: char| c.is_ascii_digit());
    if !trimmed.is_empty() {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn running_kern_guid_disk() -> Option<String> {
    let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
    let guid = cmdline
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix("kern_guid="))?;
    let out = Command::new("findfs")
        .arg(format!("PARTUUID={guid}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let part = String::from_utf8_lossy(&out.stdout).trim().to_string();
    parent_disk_name(&part)
}

fn has_binary(name: &str) -> bool {
    let path =
        std::env::var("PATH").unwrap_or_else(|_| "/usr/sbin:/usr/bin:/sbin:/bin".to_string());
    for dir in path.split(':') {
        if Path::new(dir).join(name).is_file() {
            return true;
        }
    }
    false
}

fn detect_mount_source(mountpoint: &str) -> Option<String> {
    if let Ok(output) = Command::new("findmnt")
        .args(["-n", "-o", "SOURCE", mountpoint])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn detect_root_source() -> String {
    if let Some(src) = detect_mount_source("/") {
        return src;
    }
    if let Ok(contents) = fs::read_to_string("/proc/mounts") {
        for line in contents.lines() {
            let mut fields = line.split_whitespace();
            if let (Some(dev), Some(target)) = (fields.next(), fields.next()) {
                if target == "/" {
                    return dev.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

fn detect_installed_marker(root: &Path) -> Option<String> {
    let marker_path = root.join("etc/lazyos/installation");
    let raw = fs::read_to_string(&marker_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let target = val.get("target")?.as_str()?;
    Some(format!("Existing installation record for {target}"))
}

fn detect_soc_model(info: &DeviceInfo) -> String {
    if let Ok(model) = fs::read_to_string("/proc/device-tree/model") {
        let trimmed = model.trim_matches('\0').trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(comp) = fs::read("/proc/device-tree/compatible") {
        let strings: Vec<&str> = comp
            .split(|&b| b == 0)
            .filter_map(|s| std::str::from_utf8(s).ok())
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(first) = strings.first() {
            if first.contains("asurada") || first.contains("hayato") {
                return "Google Hayato (MediaTek MT8192)".to_string();
            }
            return (*first).to_string();
        }
    }

    // DMI SMBIOS fallback for ARM64/x86_64 ACPI UEFI laptops and VMs.
    let vendor = fs::read_to_string("/sys/class/dmi/id/sys_vendor")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let product = fs::read_to_string("/sys/class/dmi/id/product_name")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match (vendor, product) {
        (Some(v), Some(p)) => {
            if p.to_ascii_lowercase().starts_with(&v.to_ascii_lowercase()) {
                return p;
            }
            return format!("{v} {p}");
        }
        (Some(v), None) => return v,
        (None, Some(p)) => return p,
        (None, None) => {}
    }

    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if let Some(model) = line.strip_prefix("Model\t: ") {
                return model.trim().to_string();
            }
            if let Some(hardware) = line.strip_prefix("Hardware\t: ") {
                return hardware.trim().to_string();
            }
        }
    }

    info.name.clone()
}

fn detect_memory() -> String {
    if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                let kb_str: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(kb) = kb_str.parse::<u64>() {
                    let gib = kb as f64 / (1024.0 * 1024.0);
                    return format!("{gib:.1} GiB");
                }
            }
        }
    }
    "4.0 GiB".to_string()
}

fn detect_power() -> String {
    let power_path = Path::new("/sys/class/power_supply");
    if power_path.is_dir() {
        let mut ac_online = false;
        let mut bat_capacity = None;

        if let Ok(entries) = fs::read_dir(power_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name.starts_with("AC") || name.starts_with("ADP") || name.contains("mains") {
                    if let Ok(online) = fs::read_to_string(path.join("online")) {
                        if online.trim() == "1" {
                            ac_online = true;
                        }
                    }
                } else if name.starts_with("BAT") {
                    if let Ok(cap) = fs::read_to_string(path.join("capacity")) {
                        if let Ok(percent) = cap.trim().parse::<u8>() {
                            bat_capacity = Some(percent);
                        }
                    }
                }
            }
        }

        if ac_online {
            if let Some(cap) = bat_capacity {
                return format!("AC connected · Battery {cap}%");
            }
            return "AC connected".to_string();
        } else if let Some(cap) = bat_capacity {
            return format!("Battery {cap}% (unplugged)");
        }
    }
    "External Power".to_string()
}

fn detect_target_size(target: &str) -> String {
    if let Ok(output) = Command::new("blockdev")
        .args(["--getsize64", target])
        .output()
    {
        if output.status.success() {
            let bytes_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(bytes) = bytes_str.parse::<u64>() {
                let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                return format!("{gib:.1} GiB");
            }
        }
    }
    "Unknown capacity".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_guard_blocks_target_disk_and_partitions() {
        assert!(check_safety("/dev/mmcblk0", "/dev/mmcblk0").is_err());
        assert!(check_safety("/dev/mmcblk0p1", "/dev/mmcblk0").is_err());
        assert!(check_safety("/dev/mmcblk0p3", "/dev/mmcblk0").is_err());
        assert!(check_safety("/dev/nvme0n1p2", "/dev/nvme0n1").is_err());
        assert!(check_safety("/dev/vda2", "/dev/vda").is_err());
        assert!(check_safety("/dev/sda1", "/dev/sda").is_err());
    }

    #[test]
    fn safety_guard_allows_different_disks() {
        assert!(check_safety("/dev/mmcblk1p3", "/dev/mmcblk0").is_ok());
        assert!(check_safety("/dev/sda2", "/dev/mmcblk0").is_ok());
        assert!(check_safety("/dev/sda2", "/dev/nvme0n1").is_ok());
        assert!(check_safety("", "/dev/mmcblk0").is_ok());
    }

    #[test]
    fn excludes_spi_hardware_boot_and_running_disks() {
        let lsblk = "\
mtdblock0 disk 8M  spi 0
mmcblk0boot0 disk 4M  mmc 0
mmcblk0rpmb disk 4M  mmc 0
mmcblk0 disk 58.2G eMMC\\x20Card mmc 0
mmcblk1 disk 29.7G SD\\x20Card mmc 0
nvme0n1 disk 476.9G Samsung\\x20PM9B1 nvme 0
sda disk 14.9G USB\\x20Flash usb 1
zram0 disk 1.9G   0
";
        let excluded = vec!["mmcblk1".to_string()];
        let cands = parse_lsblk_candidates(lsblk, &excluded);
        let paths: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["/dev/mmcblk0", "/dev/nvme0n1", "/dev/sda"]);
        assert_eq!(cands[1].model, "Samsung PM9B1");
        assert!(!cands[1].removable);
        assert!(cands[2].removable);
    }

    #[test]
    fn default_target_prefers_hint_then_fixed_disk() {
        let mut info = DeviceInfo::default();
        let hayato_cands = vec![TargetCandidate {
            path: "/dev/mmcblk0".into(),
            short_name: "mmcblk0".into(),
            size_human: "58.2G".into(),
            model: "eMMC".into(),
            tran: Some("mmc".into()),
            removable: false,
        }];
        assert_eq!(
            select_default_target(None, &info, &hayato_cands, &["mmcblk1".into()]),
            "/dev/mmcblk0"
        );

        info.boot_type = BootType::Uefi;
        info.install_target = None;
        let laptop_cands = vec![
            TargetCandidate {
                path: "/dev/sda".into(),
                short_name: "sda".into(),
                size_human: "32G".into(),
                model: "USB Stick".into(),
                tran: Some("usb".into()),
                removable: true,
            },
            TargetCandidate {
                path: "/dev/nvme0n1".into(),
                short_name: "nvme0n1".into(),
                size_human: "512G".into(),
                model: "WD Black".into(),
                tran: Some("nvme".into()),
                removable: false,
            },
        ];
        assert_eq!(
            select_default_target(None, &info, &laptop_cands, &["sdb".into()]),
            "/dev/nvme0n1"
        );
    }

    #[test]
    fn check_summary_reports_safety_and_boot_type() {
        let mut preflight = Preflight {
            device_info: DeviceInfo::default(),
            soc_model: "MT8192".into(),
            total_ram: "4.0 GiB".into(),
            power_source: "AC".into(),
            root_source: "/dev/mmcblk0p3".into(),
            target_disk: "/dev/mmcblk0".into(),
            target_size: "58 GiB".into(),
            candidates: Vec::new(),
            is_safe: false,
            refusal_reason: Some("already running from /dev/mmcblk0".into()),
            installed_notice: None,
            tools: Vec::new(),
        };

        let (ok, summary) = preflight.check_summary();
        assert!(!ok);
        assert!(summary.contains("unsafe:"));
        assert!(summary.contains("boot=depthcharge"));

        preflight.device_info.boot_type = BootType::Uefi;
        preflight.target_disk = "/dev/nvme0n1".into();
        preflight.root_source = "/dev/sda2".into();
        preflight.is_safe = true;
        preflight.refusal_reason = None;

        let (ok, summary) = preflight.check_summary();
        assert!(ok);
        assert!(summary.contains("ok: boot=uefi"));
        assert!(summary.contains("/dev/nvme0n1"));
    }
}
