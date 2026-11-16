//! Block-device partitioning, formatting, mounting, and `/etc/fstab` generation.
//!
//! Executes standard Linux utilities (`parted`, `cgpt`, `mkfs.*`, `partx`, `mount`, `e2fsck`)
//! via explicit argument vectors driven strictly by `DeviceInfo`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use super::deviceinfo::{BootType, DeviceInfo};

/// Generate a RFC 4122 version-4 UUID from `/dev/urandom` without external crate dependencies.
pub fn new_uuid_v4() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("read /dev/urandom for UUID: {e}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

/// Resolved target layout after partitioning and formatting.
#[derive(Clone, Debug)]
pub struct DiskLayout {
    pub target_disk: String,
    pub boot_type: BootType,
    pub boot_fs: String,
    pub kpart_part: Option<String>,
    pub boot_part: String,
    pub root_part: String,
    pub boot_uuid: String,
    pub root_uuid: String,
}

/// Active mountpoint guard ensuring target mounts are unmounted when dropped.
pub struct MountedTarget {
    pub root: PathBuf,
    boot_rel: &'static str,
    mounted_boot: bool,
    mounted_root: bool,
}

impl MountedTarget {
    pub fn mount(layout: &DiskLayout, root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| format!("create {}: {e}", root.display()))?;
        run_cmd(
            "mount",
            &[&layout.root_part, root.to_string_lossy().as_ref()],
        )?;

        let mut guard = Self {
            root: root.to_path_buf(),
            boot_rel: match layout.boot_type {
                BootType::Uefi => "boot/efi",
                BootType::Depthcharge | BootType::Extlinux => "boot",
            },
            mounted_boot: false,
            mounted_root: true,
        };

        let boot_dir = root.join(guard.boot_rel);
        fs::create_dir_all(&boot_dir).map_err(|e| format!("create {}: {e}", boot_dir.display()))?;
        run_cmd(
            "mount",
            &[&layout.boot_part, boot_dir.to_string_lossy().as_ref()],
        )?;
        guard.mounted_boot = true;
        Ok(guard)
    }

    pub fn unmount(mut self) -> Result<(), String> {
        let mut first_err = None;
        if self.mounted_boot {
            let boot_dir = self.root.join(self.boot_rel);
            if let Err(e) = run_cmd("umount", &[boot_dir.to_string_lossy().as_ref()]) {
                first_err = Some(e);
            } else {
                self.mounted_boot = false;
            }
        }
        if self.mounted_root {
            if let Err(e) = run_cmd("umount", &[self.root.to_string_lossy().as_ref()]) {
                first_err = first_err.or(Some(e));
            } else {
                self.mounted_root = false;
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Drop for MountedTarget {
    fn drop(&mut self) {
        if self.mounted_boot {
            let boot_dir = self.root.join(self.boot_rel);
            let _ = Command::new("umount").arg("-l").arg(&boot_dir).status();
            self.mounted_boot = false;
        }
        if self.mounted_root {
            let _ = Command::new("umount").arg("-l").arg(&self.root).status();
            self.mounted_root = false;
        }
    }
}

/// Mask and stop `udisks2` so newly partitioned devices are not auto-mounted mid-mkfs.
pub fn suspend_udisks2() -> bool {
    let was_active = Command::new("systemctl")
        .args(["is-active", "--quiet", "udisks2"])
        .status()
        .is_ok_and(|s| s.success());
    let _ = Command::new("systemctl")
        .args(["mask", "--runtime", "udisks2"])
        .status();
    let _ = Command::new("systemctl").args(["stop", "udisks2"]).status();
    was_active
}

/// Restore `udisks2` if suspended prior to installation failure/cancellation.
pub fn restore_udisks2(was_active: bool) {
    let _ = Command::new("systemctl")
        .args(["unmask", "--runtime", "udisks2"])
        .status();
    if was_active {
        let _ = Command::new("systemctl")
            .args(["start", "udisks2"])
            .status();
    }
}

/// Release any existing mounts/swap on `target_disk` partitions before repartitioning.
pub fn release_target(target_disk: &str) -> Result<(), String> {
    let parts = list_block_nodes(target_disk);
    for dev in parts {
        let _ = Command::new("swapoff").arg(&dev).status();
        let mounts = Command::new("findmnt")
            .args(["-rn", "-S", &dev, "-o", "TARGET"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if !mounts.is_empty() {
            if mounts == "/" || mounts == "/boot" || mounts == "/boot/efi" {
                return Err(format!(
                    "{dev} is mounted at {mounts}; refusing to overwrite active OS partition"
                ));
            }
            let status = Command::new("umount")
                .arg(&dev)
                .status()
                .map_err(|e| format!("failed running umount on {dev}: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "{dev} is busy at {mounts}; unmount it manually and retry"
                ));
            }
        }
    }
    Ok(())
}

/// Compute ChromeOS GPT sectors `(boot_start, boot_size, root_start, root_size)`.
pub fn depthcharge_sectors(
    total_sectors: u64,
    kpart_start: u64,
    kpart_size: u64,
    boot_mib: u64,
) -> Result<(u64, u64, u64, u64), String> {
    if boot_mib < 512 {
        return Err(format!(
            "boot partition size {boot_mib} MiB is below 512 MiB depthcharge minimum"
        ));
    }
    let boot_start = kpart_start + kpart_size;
    let boot_size = boot_mib * 2048;
    let root_start = boot_start + boot_size;
    if total_sectors <= root_start + 33 {
        return Err("target disk too small for ChromeOS GPT layout".to_string());
    }
    let root_size = total_sectors - root_start - 33;
    if root_size <= 1_048_576 {
        return Err(format!(
            "root partition size {root_size} sectors is too small (< 512 MiB)"
        ));
    }
    Ok((boot_start, boot_size, root_start, root_size))
}

/// Partition and format `target_disk` using planned `root_uuid` and `boot_uuid` according to `info`.
pub fn partition_and_format(
    target_disk: &str,
    info: &DeviceInfo,
    root_uuid: &str,
    boot_uuid: &str,
) -> Result<DiskLayout, String> {
    match info.boot_type {
        BootType::Depthcharge => {
            partition_and_format_depthcharge(target_disk, info, root_uuid, boot_uuid)
        }
        BootType::Uefi => partition_and_format_uefi(target_disk, info, root_uuid),
        BootType::Extlinux => {
            partition_and_format_extlinux(target_disk, info, root_uuid, boot_uuid)
        }
    }
}

fn partition_and_format_depthcharge(
    disk: &str,
    info: &DeviceInfo,
    root_uuid: &str,
    boot_uuid: &str,
) -> Result<DiskLayout, String> {
    let total_sectors = blockdev_sectors(disk)?;
    let (boot_start, boot_size, root_start, root_size) = depthcharge_sectors(
        total_sectors,
        info.cgpt_kpart_start,
        info.cgpt_kpart_size,
        info.boot_size_mib,
    )?;

    zero_gpt_areas(disk, total_sectors)?;
    run_cmd("parted", &["-s", disk, "mktable", "gpt"])?;
    run_cmd("cgpt", &["create", disk])?;
    run_cmd(
        "cgpt",
        &[
            "add",
            "-i",
            "1",
            "-b",
            &info.cgpt_kpart_start.to_string(),
            "-s",
            &info.cgpt_kpart_size.to_string(),
            "-t",
            "kernel",
            "-l",
            "pmOS_kernel",
            "-S",
            "1",
            "-T",
            "5",
            "-P",
            "10",
            disk,
        ],
    )?;
    run_cmd(
        "cgpt",
        &[
            "add",
            "-i",
            "2",
            "-b",
            &boot_start.to_string(),
            "-s",
            &boot_size.to_string(),
            "-t",
            "efi",
            "-l",
            "pmOS_boot",
            disk,
        ],
    )?;
    run_cmd(
        "cgpt",
        &[
            "add",
            "-i",
            "3",
            "-b",
            &root_start.to_string(),
            "-s",
            &root_size.to_string(),
            "-t",
            "data",
            "-l",
            "pmOS_root",
            disk,
        ],
    )?;

    reread_partitions(disk);
    let lsblk_out = lsblk_partn(disk)?;
    let kpart = parse_lsblk_partn(&lsblk_out, 1)
        .ok_or_else(|| format!("could not resolve p1 on {disk}"))?;
    let boot_part = parse_lsblk_partn(&lsblk_out, 2)
        .ok_or_else(|| format!("could not resolve p2 on {disk}"))?;
    let root_part = parse_lsblk_partn(&lsblk_out, 3)
        .ok_or_else(|| format!("could not resolve p3 on {disk}"))?;

    assert_part_not_whole(&[&kpart, &boot_part, &root_part], disk)?;
    assert_partlabel(&kpart, "pmOS_kernel")?;
    assert_partlabel(&boot_part, "pmOS_boot")?;
    assert_partlabel(&root_part, "pmOS_root")?;

    let boot_fs = &info.boot_filesystem;
    run_cmd(
        &format!("mkfs.{boot_fs}"),
        &["-F", "-q", "-L", "pmOS_boot", "-U", boot_uuid, &boot_part],
    )?;
    run_cmd(
        "mkfs.ext4",
        &["-F", "-q", "-L", "pmOS_root", "-U", root_uuid, &root_part],
    )?;

    Ok(DiskLayout {
        target_disk: disk.to_string(),
        boot_type: BootType::Depthcharge,
        boot_fs: boot_fs.clone(),
        kpart_part: Some(kpart),
        boot_uuid: blkid_uuid(&boot_part).unwrap_or_else(|| boot_uuid.to_string()),
        boot_part,
        root_part,
        root_uuid: root_uuid.to_string(),
    })
}

fn partition_and_format_uefi(
    disk: &str,
    info: &DeviceInfo,
    root_uuid: &str,
) -> Result<DiskLayout, String> {
    if info.boot_size_mib < 128 {
        return Err(format!(
            "UEFI ESP size {} MiB too small (minimum 128 MiB)",
            info.boot_size_mib
        ));
    }
    let esp_end = format!("{}MiB", 1 + info.boot_size_mib);
    run_cmd(
        "parted",
        &[
            "-s",
            disk,
            "mktable",
            "gpt",
            "mkpart",
            "lazyos_efi",
            "fat32",
            "1MiB",
            &esp_end,
            "set",
            "1",
            "esp",
            "on",
            "mkpart",
            "lazyos_root",
            "ext4",
            &esp_end,
            "100%",
        ],
    )?;

    reread_partitions(disk);
    let lsblk_out = lsblk_partn(disk)?;
    let boot_part = parse_lsblk_partn(&lsblk_out, 1)
        .ok_or_else(|| format!("could not resolve EFI p1 on {disk}"))?;
    let root_part = parse_lsblk_partn(&lsblk_out, 2)
        .ok_or_else(|| format!("could not resolve root p2 on {disk}"))?;
    assert_part_not_whole(&[&boot_part, &root_part], disk)?;

    run_cmd("mkfs.vfat", &["-F", "32", "-n", "LAZYOS_EFI", &boot_part])?;
    run_cmd(
        "mkfs.ext4",
        &["-F", "-q", "-L", "lazyos_root", "-U", root_uuid, &root_part],
    )?;
    let esp_uuid = blkid_uuid(&boot_part)
        .ok_or_else(|| format!("could not read FAT32 UUID on {boot_part}"))?;

    Ok(DiskLayout {
        target_disk: disk.to_string(),
        boot_type: BootType::Uefi,
        boot_fs: "vfat".to_string(),
        kpart_part: None,
        boot_part,
        root_part,
        boot_uuid: esp_uuid,
        root_uuid: root_uuid.to_string(),
    })
}

fn partition_and_format_extlinux(
    disk: &str,
    info: &DeviceInfo,
    root_uuid: &str,
    boot_uuid: &str,
) -> Result<DiskLayout, String> {
    if info.boot_size_mib < 256 {
        return Err(format!(
            "extlinux /boot size {} MiB too small (minimum 256 MiB)",
            info.boot_size_mib
        ));
    }
    let boot_start = format!("{}MiB", info.extlinux_start_mib);
    let boot_end = format!("{}MiB", info.extlinux_start_mib + info.boot_size_mib);
    let boot_fs = &info.boot_filesystem;

    run_cmd(
        "parted",
        &[
            "-s",
            disk,
            "mktable",
            "gpt",
            "mkpart",
            "lazyos_boot",
            boot_fs,
            &boot_start,
            &boot_end,
            "set",
            "1",
            "legacy_boot",
            "on",
            "mkpart",
            "lazyos_root",
            "ext4",
            &boot_end,
            "100%",
        ],
    )?;

    reread_partitions(disk);
    let lsblk_out = lsblk_partn(disk)?;
    let boot_part = parse_lsblk_partn(&lsblk_out, 1)
        .ok_or_else(|| format!("could not resolve boot p1 on {disk}"))?;
    let root_part = parse_lsblk_partn(&lsblk_out, 2)
        .ok_or_else(|| format!("could not resolve root p2 on {disk}"))?;
    assert_part_not_whole(&[&boot_part, &root_part], disk)?;

    run_cmd(
        &format!("mkfs.{boot_fs}"),
        &["-F", "-q", "-L", "lazyos_boot", "-U", boot_uuid, &boot_part],
    )?;
    run_cmd(
        "mkfs.ext4",
        &["-F", "-q", "-L", "lazyos_root", "-U", root_uuid, &root_part],
    )?;

    Ok(DiskLayout {
        target_disk: disk.to_string(),
        boot_type: BootType::Extlinux,
        boot_fs: boot_fs.clone(),
        kpart_part: None,
        boot_part,
        root_part,
        boot_uuid: boot_uuid.to_string(),
        root_uuid: root_uuid.to_string(),
    })
}

/// Build `/etc/fstab` preserving non-OS entries (e.g. auxiliary swap) from `existing_live_fstab`.
pub fn render_fstab(
    existing_live_fstab: &str,
    boot_type: BootType,
    boot_fs: &str,
    boot_uuid: &str,
    root_uuid: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("UUID={root_uuid}\t/\text4\tdefaults\t0 1\n"));
    match boot_type {
        BootType::Uefi => out.push_str(&format!(
            "UUID={boot_uuid}\t/boot/efi\tvfat\tumask=0077\t0 2\n"
        )),
        BootType::Depthcharge | BootType::Extlinux => out.push_str(&format!(
            "UUID={boot_uuid}\t/boot\t{boot_fs}\tnodev,nosuid,noexec\t0 2\n"
        )),
    }

    for line in existing_live_fstab.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() >= 2 && matches!(cols[1], "/" | "/boot" | "/boot/efi") {
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
}

pub fn write_target_fstab(mnt: &Path, layout: &DiskLayout) -> Result<(), String> {
    let live_fstab = fs::read_to_string("/etc/fstab").unwrap_or_default();
    let content = render_fstab(
        &live_fstab,
        layout.boot_type,
        &layout.boot_fs,
        &layout.boot_uuid,
        &layout.root_uuid,
    );
    let dst = mnt.join("etc/fstab");
    fs::write(&dst, content).map_err(|e| format!("write {}: {e}", dst.display()))
}

/// Run read-only `e2fsck -f -n` on unmounted root partition and verify UUID matches plan.
pub fn verify_target_fs(layout: &DiskLayout) -> Result<(), String> {
    let actual_root = blkid_uuid(&layout.root_part)
        .ok_or_else(|| format!("could not read UUID back from {}", layout.root_part))?;
    if actual_root != layout.root_uuid {
        return Err(format!(
            "root UUID mismatch on {}: expected {}, got {}",
            layout.root_part, layout.root_uuid, actual_root
        ));
    }
    run_cmd("e2fsck", &["-f", "-n", &layout.root_part])?;
    Ok(())
}

/// Resolve partition path by `PARTN` column (unaffected by kernel minor reordering).
pub fn parse_lsblk_partn(raw: &str, wanted_partn: u8) -> Option<String> {
    let want = wanted_partn.to_string();
    for line in raw.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(path), Some(partn)) = (fields.next(), fields.next()) {
            if partn == want {
                return Some(path.to_string());
            }
        }
    }
    None
}

fn zero_gpt_areas(disk: &str, total_sectors: u64) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .open(disk)
        .map_err(|e| format!("open {disk} to clear GPT: {e}"))?;
    let primary_zero = vec![0u8; 34 * 512];
    f.seek(SeekFrom::Start(0))
        .and_then(|_| f.write_all(&primary_zero))
        .map_err(|e| format!("zero primary GPT on {disk}: {e}"))?;
    if total_sectors > 33 {
        let backup_zero = vec![0u8; 33 * 512];
        let offset = (total_sectors - 33) * 512;
        f.seek(SeekFrom::Start(offset))
            .and_then(|_| f.write_all(&backup_zero))
            .map_err(|e| format!("zero backup GPT on {disk}: {e}"))?;
    }
    f.sync_all()
        .map_err(|e| format!("sync GPT wipe on {disk}: {e}"))
}

pub fn blockdev_sectors(disk: &str) -> Result<u64, String> {
    let out = Command::new("blockdev")
        .args(["--getsz", disk])
        .output()
        .map_err(|e| format!("blockdev --getsz {disk}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "blockdev --getsz {disk} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("parse sectors for {disk}: {e}"))
}

pub fn blockdev_bytes(dev: &str) -> Result<u64, String> {
    let out = Command::new("blockdev")
        .args(["--getsize64", dev])
        .output()
        .map_err(|e| format!("blockdev --getsize64 {dev}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "blockdev --getsize64 {dev} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("parse bytes for {dev}: {e}"))
}

fn reread_partitions(disk: &str) {
    let _ = Command::new("partprobe").arg(disk).status();
    let _ = Command::new("partx").args(["-d", disk]).status();
    let ok = Command::new("partx")
        .args(["-a", disk])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        let _ = Command::new("blockdev").args(["--rereadpt", disk]).status();
    }
    if Command::new("udevadm").arg("settle").status().is_err() {
        thread::sleep(Duration::from_secs(2));
    }
}

fn lsblk_partn(disk: &str) -> Result<String, String> {
    let out = Command::new("lsblk")
        .args(["-nro", "PATH,PARTN", disk])
        .output()
        .map_err(|e| format!("lsblk {disk}: {e}"))?;
    if !out.status.success() {
        return Err(format!("lsblk failed on {disk}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn assert_part_not_whole(parts: &[&str], disk: &str) -> Result<(), String> {
    for &p in parts {
        if p == disk {
            return Err(format!(
                "refusing to operate on whole disk {disk}; partition resolution failed"
            ));
        }
    }
    Ok(())
}

fn assert_partlabel(part: &str, expected: &str) -> Result<(), String> {
    let out = Command::new("lsblk")
        .args(["-nro", "PARTLABEL", part])
        .output()
        .map_err(|e| format!("read PARTLABEL on {part}: {e}"))?;
    let actual = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if actual != expected {
        return Err(format!(
            "{part} has PARTLABEL '{actual}', expected '{expected}'"
        ));
    }
    Ok(())
}

fn blkid_uuid(dev: &str) -> Option<String> {
    let out = Command::new("blkid")
        .args(["-s", "UUID", "-o", "value", dev])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn list_block_nodes(disk: &str) -> Vec<String> {
    let Ok(out) = Command::new("lsblk").args(["-nro", "PATH", disk]).output() else {
        return vec![disk.to_string()];
    };
    if !out.status.success() {
        return vec![disk.to_string()];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn run_cmd(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(prog)
        .args(args)
        .output()
        .map_err(|e| format!("spawn {prog}: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!(
            "{prog} {} failed ({}): {detail}",
            args.join(" "),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_uuid_v4_strings() {
        let a = new_uuid_v4().unwrap();
        let b = new_uuid_v4().unwrap();
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().nth(14), Some('4'));
        assert_ne!(a, b);
    }

    #[test]
    fn resolves_reordered_partitions_by_partn() {
        let lsblk = "/dev/loop0\n/dev/loop0p3 3\n/dev/loop0p1 1\n/dev/loop0p2 2\n";
        assert_eq!(parse_lsblk_partn(lsblk, 1).as_deref(), Some("/dev/loop0p1"));
        assert_eq!(parse_lsblk_partn(lsblk, 2).as_deref(), Some("/dev/loop0p2"));
        assert_eq!(parse_lsblk_partn(lsblk, 3).as_deref(), Some("/dev/loop0p3"));
        assert_eq!(parse_lsblk_partn(lsblk, 4), None);
    }

    #[test]
    fn calculates_hayato_chromeos_gpt_sectors() {
        let (bs, bsz, rs, rsz) = depthcharge_sectors(122_142_720, 8192, 262_144, 512).unwrap();
        assert_eq!(bs, 270_336);
        assert_eq!(bsz, 1_048_576);
        assert_eq!(rs, 1_318_912);
        assert_eq!(rsz, 122_142_720 - 1_318_912 - 33);
    }

    #[test]
    fn fstab_replaces_os_mounts_and_preserves_swap() {
        let live = "UUID=old-root / ext4 defaults 0 1\nUUID=old-boot /boot ext2 nodev 0 2\nLABEL=lazyos_swap none swap sw,nofail,pri=10 0 0\n";
        let uefi = render_fstab(live, BootType::Uefi, "vfat", "AAAA-BBBB", "1111-2222");
        assert!(uefi.contains("UUID=1111-2222\t/\text4\tdefaults\t0 1"));
        assert!(uefi.contains("UUID=AAAA-BBBB\t/boot/efi\tvfat\tumask=0077\t0 2"));
        assert!(uefi.contains("LABEL=lazyos_swap none swap sw,nofail,pri=10 0 0"));
        assert!(!uefi.contains("old-root"));
        assert!(!uefi.contains("old-boot"));

        let ext = render_fstab(live, BootType::Extlinux, "ext2", "3333", "4444");
        assert!(ext.contains("UUID=3333\t/boot\text2\tnodev,nosuid,noexec\t0 2"));
    }
}
