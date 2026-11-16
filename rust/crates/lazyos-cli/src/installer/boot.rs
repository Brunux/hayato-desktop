//! Bootloader installation & verification across `Depthcharge`, `Uefi`, and `Extlinux`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::deviceinfo::{BootType, DeviceInfo, load_cmdline_fragments};
use super::disk::{DiskLayout, blockdev_bytes, run_cmd};

/// Prepared depthcharge FIT image built before touching the target partition table.
#[derive(Clone, Debug)]
pub struct PrebuiltKpart {
    pub path: PathBuf,
    pub kernel_release: String,
}

/// Resolve currently running kernel release string from `/proc/sys/kernel/osrelease` or `uname -r`.
pub fn running_kernel_release() -> Result<String, String> {
    if let Ok(rel) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        let trimmed = rel.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let out = Command::new("uname")
        .arg("-r")
        .output()
        .map_err(|e| format!("run uname -r: {e}"))?;
    let trimmed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if trimmed.is_empty() {
        Err("could not determine running kernel release".to_string())
    } else {
        Ok(trimmed)
    }
}

/// Build canonical single-space kernel command line from base config + `root=UUID=...`.
pub fn build_cmdline(base_cmdline: &str, root_uuid: &str) -> String {
    let base = base_cmdline
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if base.is_empty() {
        format!("root=UUID={root_uuid} rootwait rw")
    } else {
        format!("{base} root=UUID={root_uuid} rootwait rw")
    }
}

/// Pre-build & `futility`-verify signed ChromeOS FIT image on `Depthcharge` before destructive GPT writes.
pub fn prebuild_depthcharge_kpart(
    info: &DeviceInfo,
    root_uuid: &str,
    work_dir: &Path,
) -> Result<PrebuiltKpart, String> {
    let rel = running_kernel_release()?;
    let kimg = PathBuf::from(format!("/boot/vmlinuz-{rel}"));
    let initrd = PathBuf::from(format!("/boot/initrd.img-{rel}"));
    if !kimg.is_file() {
        return Err(format!("missing live kernel image: {}", kimg.display()));
    }
    if !initrd.is_file() {
        return Err(format!("missing live initrd: {}", initrd.display()));
    }

    let dtbs_root = Path::new("/boot/dtbs");
    let matched_dtbs = find_matching_dtbs(dtbs_root, info.dtb.as_deref());
    if matched_dtbs.is_empty() {
        return Err(format!(
            "no DTBs matching '{}' under /boot/dtbs",
            info.dtb.as_deref().unwrap_or("*.dtb")
        ));
    }

    let priv_dir = work_dir.join("private");
    let dtb_stage = priv_dir.join("dtbs");
    let dct_tmp = priv_dir.join("dct");
    let _ = fs::remove_dir_all(&dtb_stage);
    let _ = fs::remove_dir_all(&dct_tmp);
    fs::create_dir_all(&dtb_stage).map_err(|e| format!("create dtb stage: {e}"))?;
    fs::create_dir_all(&dct_tmp).map_err(|e| format!("create dct tmp: {e}"))?;

    for dtb in &matched_dtbs {
        let name = dtb
            .file_name()
            .ok_or_else(|| format!("bad dtb path {}", dtb.display()))?;
        fs::copy(dtb, dtb_stage.join(name))
            .map_err(|e| format!("stage dtb {}: {e}", dtb.display()))?;
    }

    let kpart_out = priv_dir.join("kpart.img");
    let base_cmdline = load_cmdline_fragments(Path::new("/"));
    let full_cmdline = build_cmdline(&base_cmdline, root_uuid);
    let board = info.depthcharge_board.as_deref().unwrap_or("mt8192");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1)
        .to_string();

    run_cmd(
        "depthchargectl",
        &[
            "build",
            "--root",
            "none",
            "--board",
            board,
            "--kernel",
            &kimg.to_string_lossy(),
            "--kernel-cmdline",
            &full_cmdline,
            "--initramfs",
            &initrd.to_string_lossy(),
            "--fdtdir",
            &dtb_stage.to_string_lossy(),
            "--compress",
            &info.depthcharge_compression,
            "--timestamp",
            &ts,
            "--tmpdir",
            &dct_tmp.to_string_lossy(),
            "--output",
            &kpart_out.to_string_lossy(),
        ],
    )?;

    let size_bytes = fs::metadata(&kpart_out)
        .map(|m| m.len())
        .map_err(|e| format!("stat {}: {e}", kpart_out.display()))?;
    let max_bytes = info.cgpt_kpart_size * 512;
    if size_bytes > max_bytes {
        return Err(format!(
            "built kpart ({size_bytes} bytes) exceeds p1 size ({max_bytes} bytes)"
        ));
    }

    verify_futility(&kpart_out)?;
    Ok(PrebuiltKpart {
        path: kpart_out,
        kernel_release: rel,
    })
}

/// Write and verify bootloader configuration / kernel partition on the mounted target root.
pub fn install_boot_target(
    mnt: &Path,
    info: &DeviceInfo,
    layout: &DiskLayout,
    prebuilt_kpart: Option<&PrebuiltKpart>,
) -> Result<(), String> {
    let base_cmdline = load_cmdline_fragments(mnt);
    let cmdline = build_cmdline(&base_cmdline, &layout.root_uuid);

    match info.boot_type {
        BootType::Depthcharge => {
            let prebuilt = prebuilt_kpart.ok_or_else(|| {
                "internal error: depthcharge install missing prebuilt kpart".to_string()
            })?;
            let kpart_part = layout
                .kpart_part
                .as_deref()
                .ok_or_else(|| "internal error: depthcharge layout missing p1".to_string())?;
            let target_vmlinuz = mnt.join(format!("boot/vmlinuz-{}", prebuilt.kernel_release));
            let target_initrd = mnt.join(format!("boot/initrd.img-{}", prebuilt.kernel_release));
            if !target_vmlinuz.is_file() || !target_initrd.is_file() {
                return Err(format!(
                    "copied /boot lacks kernel or initrd for {}",
                    prebuilt.kernel_release
                ));
            }
            let part_bytes = blockdev_bytes(kpart_part)?;
            let img_bytes = fs::metadata(&prebuilt.path)
                .map(|m| m.len())
                .map_err(|e| format!("stat kpart: {e}"))?;
            if img_bytes > part_bytes {
                return Err(format!(
                    "kpart ({img_bytes} bytes) does not fit in {kpart_part} ({part_bytes} bytes)"
                ));
            }

            let data = fs::read(&prebuilt.path)
                .map_err(|e| format!("read {}: {e}", prebuilt.path.display()))?;
            let mut dev = OpenOptions::new()
                .write(true)
                .open(kpart_part)
                .map_err(|e| format!("open {kpart_part} to write kpart: {e}"))?;
            dev.write_all(&data)
                .and_then(|_| dev.sync_all())
                .map_err(|e| format!("write kpart to {kpart_part}: {e}"))?;

            verify_futility(Path::new(kpart_part))?;
            let _ = Command::new("cgpt")
                .args(["show", &layout.target_disk])
                .status();
            Ok(())
        }
        BootType::Uefi => install_uefi_target(mnt, info, &cmdline),
        BootType::Extlinux => install_extlinux_target(mnt, info, layout, &cmdline),
    }
}

fn install_uefi_target(mnt: &Path, info: &DeviceInfo, cmdline: &str) -> Result<(), String> {
    let rel = resolve_latest_kernel_release(&mnt.join("boot"))?;
    let vmlinuz = mnt.join(format!("boot/vmlinuz-{rel}"));
    let initrd = mnt.join(format!("boot/initrd.img-{rel}"));
    if !vmlinuz.is_file() || !initrd.is_file() {
        return Err(format!(
            "missing kernel/initrd in {}/boot for {rel}",
            mnt.display()
        ));
    }

    let esp = mnt.join("boot/efi");
    let esp_lazyos = esp.join("lazyos");
    let esp_entries = esp.join("loader/entries");
    let esp_boot = esp.join("EFI/BOOT");
    let esp_systemd = esp.join("EFI/systemd");
    for d in [&esp_lazyos, &esp_entries, &esp_boot, &esp_systemd] {
        fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }

    fs::copy(&vmlinuz, esp_lazyos.join(format!("vmlinuz-{rel}")))
        .map_err(|e| format!("copy vmlinuz to ESP: {e}"))?;
    fs::copy(&initrd, esp_lazyos.join(format!("initrd.img-{rel}")))
        .map_err(|e| format!("copy initrd to ESP: {e}"))?;

    let dtbs_dir = mnt.join("boot/dtbs");
    let mut dtb_rel: Option<String> = None;
    if let Some(ref pattern) = info.dtb {
        if let Some(first) = find_matching_dtbs(&dtbs_dir, Some(pattern))
            .into_iter()
            .next()
        {
            if let Ok(rel_path) = first.strip_prefix(&dtbs_dir) {
                let rel_str = rel_path.to_string_lossy().into_owned();
                let dst = esp_lazyos.join("dtbs").join(rel_path);
                if let Some(p) = dst.parent() {
                    let _ = fs::create_dir_all(p);
                }
                fs::copy(&first, &dst).map_err(|e| format!("copy DTB to ESP: {e}"))?;
                dtb_rel = Some(rel_str);
            }
        }
    } else if dtbs_dir.is_dir() {
        copy_dir_recursive(&dtbs_dir, &esp.join("dtb"))?;
    }

    fs::write(esp.join("loader/loader.conf"), render_systemd_boot_loader())
        .map_err(|e| format!("write loader.conf: {e}"))?;
    fs::write(
        esp_entries.join("lazyos.conf"),
        render_systemd_boot_entry(&rel, dtb_rel.as_deref(), cmdline),
    )
    .map_err(|e| format!("write lazyos.conf: {e}"))?;

    let mut installed_efi = false;
    for (src_rel, boot_name, sys_name) in [
        (
            "usr/lib/systemd/boot/efi/systemd-bootaa64.efi",
            "BOOTAA64.EFI",
            "systemd-bootaa64.efi",
        ),
        (
            "usr/lib/systemd/boot/efi/systemd-bootx64.efi",
            "BOOTX64.EFI",
            "systemd-bootx64.efi",
        ),
    ] {
        let src = mnt.join(src_rel);
        if src.is_file() {
            fs::copy(&src, esp_boot.join(boot_name))
                .map_err(|e| format!("install {boot_name}: {e}"))?;
            fs::copy(&src, esp_systemd.join(sys_name))
                .map_err(|e| format!("install {sys_name}: {e}"))?;
            installed_efi = true;
            break;
        }
    }
    if !installed_efi {
        return Err(
            "systemd-boot EFI binary missing under /usr/lib/systemd/boot/efi/ (install systemd-boot-efi)"
                .to_string(),
        );
    }

    // Best-effort NVRAM boot registration on live UEFI systems; removable-media BOOTAA64.EFI still works if read-only.
    if Path::new("/sys/firmware/efi/efivars").is_dir() {
        let _ = Command::new("bootctl")
            .args(["--esp-path", &esp.to_string_lossy(), "update"])
            .status();
    }
    Ok(())
}

fn install_extlinux_target(
    mnt: &Path,
    info: &DeviceInfo,
    layout: &DiskLayout,
    cmdline: &str,
) -> Result<(), String> {
    let boot_dir = mnt.join("boot");
    let rel = resolve_latest_kernel_release(&boot_dir)?;
    let vmlinuz = boot_dir.join(format!("vmlinuz-{rel}"));
    let initrd = boot_dir.join(format!("initrd.img-{rel}"));
    if !vmlinuz.is_file() || !initrd.is_file() {
        return Err(format!(
            "missing kernel/initrd in {} for {rel}",
            boot_dir.display()
        ));
    }

    let dtbs_dir = boot_dir.join("dtbs");
    let matched = find_matching_dtbs(&dtbs_dir, info.dtb.as_deref());
    let single_dtb_rel = if info.dtb.is_some() && matched.len() == 1 {
        matched[0]
            .strip_prefix(&dtbs_dir)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    } else {
        None
    };

    let extlinux_dir = boot_dir.join("extlinux");
    fs::create_dir_all(&extlinux_dir).map_err(|e| format!("create extlinux dir: {e}"))?;
    let conf = render_extlinux_conf(&rel, single_dtb_rel.as_deref(), dtbs_dir.is_dir(), cmdline);
    fs::write(extlinux_dir.join("extlinux.conf"), conf)
        .map_err(|e| format!("write extlinux.conf: {e}"))?;

    // Optional board hook for future SoCs with raw SPL sectors on disk.
    let hook = Path::new("/usr/libexec/lazyos/boot/extlinux-post-disk.sh");
    if hook.is_file() {
        run_cmd(
            &hook.to_string_lossy(),
            &[&layout.target_disk, &mnt.to_string_lossy()],
        )?;
    }
    Ok(())
}

pub fn render_systemd_boot_loader() -> &'static str {
    "default lazyos.conf\ntimeout 1\nconsole-mode max\n"
}

pub fn render_systemd_boot_entry(rel: &str, dtb_rel: Option<&str>, cmdline: &str) -> String {
    let mut s = format!(
        "title   LazyOS ({rel})\nlinux   /lazyos/vmlinuz-{rel}\ninitrd  /lazyos/initrd.img-{rel}\n"
    );
    if let Some(dtb) = dtb_rel {
        s.push_str(&format!("devicetree /lazyos/dtbs/{dtb}\n"));
    }
    s.push_str(&format!("options {cmdline}\n"));
    s
}

pub fn render_extlinux_conf(
    rel: &str,
    dtb_rel: Option<&str>,
    has_dtbs_dir: bool,
    cmdline: &str,
) -> String {
    let mut s = format!(
        "TIMEOUT 10\nDEFAULT lazyos\n\nLABEL lazyos\n    MENU LABEL LazyOS ({rel})\n    LINUX /vmlinuz-{rel}\n    INITRD /initrd.img-{rel}\n"
    );
    if let Some(dtb) = dtb_rel {
        s.push_str(&format!("    FDT /dtbs/{dtb}\n"));
    } else if has_dtbs_dir {
        s.push_str("    FDTDIR /dtbs/\n");
    }
    s.push_str(&format!("    APPEND {cmdline}\n"));
    s
}

fn verify_futility(path: &Path) -> Result<(), String> {
    let out = Command::new("futility")
        .args(["show", &path.to_string_lossy()])
        .output()
        .map_err(|e| format!("run futility show: {e}"))?;
    if !out.status.success() {
        return Err(format!("futility rejected {}", path.display()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !stdout.contains("Body verification succeeded") {
        return Err(format!(
            "futility could not verify FIT body in {}",
            path.display()
        ));
    }
    Ok(())
}

fn resolve_latest_kernel_release(boot_dir: &Path) -> Result<String, String> {
    let mut candidates: Vec<String> = Vec::new();
    let entries =
        fs::read_dir(boot_dir).map_err(|e| format!("read {}: {e}", boot_dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(rel) = name.strip_prefix("vmlinuz-") {
            candidates.push(rel.to_string());
        }
    }
    candidates.sort();
    candidates
        .pop()
        .ok_or_else(|| format!("no vmlinuz-* found under {}", boot_dir.display()))
}

fn find_matching_dtbs(dtbs_root: &Path, pattern: Option<&str>) -> Vec<PathBuf> {
    if !dtbs_root.is_dir() {
        return Vec::new();
    }
    let mut all = Vec::new();
    collect_dtbs_recursive(dtbs_root, &mut all);
    if let Some(pat) = pattern {
        let pat_dtb = if pat.ends_with(".dtb") {
            pat.to_string()
        } else {
            format!("{pat}.dtb")
        };
        all.retain(|p| {
            p.strip_prefix(dtbs_root)
                .ok()
                .is_some_and(|rel| simple_glob_match(&pat_dtb, &rel.to_string_lossy()))
        });
    }
    all.sort_by(|a, b| {
        let a_hay = a.to_string_lossy().contains("hayato");
        let b_hay = b.to_string_lossy().contains("hayato");
        b_hay.cmp(&a_hay).then_with(|| a.cmp(b))
    });
    all
}

fn collect_dtbs_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_dtbs_recursive(&p, out);
        } else if p.extension().is_some_and(|e| e == "dtb") {
            out.push(p);
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let p = entry.path();
        let target = dst.join(entry.file_name());
        if p.is_dir() {
            copy_dir_recursive(&p, &target)?;
        } else if p.is_file() {
            fs::copy(&p, &target).map_err(|e| format!("copy {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// Minimal glob matching supporting `*` and `?` for `deviceinfo_dtb` paths.
pub fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut match_idx) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_deviceinfo_dtb_patterns() {
        assert!(simple_glob_match(
            "mediatek/mt8192-asurada*.dtb",
            "mediatek/mt8192-asurada-hayato-r5.dtb"
        ));
        assert!(!simple_glob_match(
            "mediatek/mt8192-asurada*.dtb",
            "rockchip/rk3588.dtb"
        ));
    }

    #[test]
    fn cmdline_builder_never_glues_words() {
        assert_eq!(
            build_cmdline("quiet\nloglevel=2\n", "1234-5678"),
            "quiet loglevel=2 root=UUID=1234-5678 rootwait rw"
        );
    }

    #[test]
    fn renders_uefi_and_extlinux_configs_accurately() {
        let uefi = render_systemd_boot_entry(
            "7.2.15+deb14-arm64",
            None,
            "quiet loglevel=2 root=UUID=abcd rootwait rw",
        );
        assert!(uefi.contains("linux   /lazyos/vmlinuz-7.2.15+deb14-arm64"));
        assert!(uefi.contains("initrd  /lazyos/initrd.img-7.2.15+deb14-arm64"));
        assert!(!uefi.contains("devicetree"));
        assert!(uefi.contains("options quiet loglevel=2 root=UUID=abcd rootwait rw"));

        let ext_generic = render_extlinux_conf("7.2.15+deb14-arm64", None, true, "root=UUID=ef");
        assert!(ext_generic.contains("    FDTDIR /dtbs/"));
        let ext_board = render_extlinux_conf(
            "7.2.15+deb14-arm64",
            Some("rockchip/rk3588-rock-5b.dtb"),
            true,
            "root=UUID=ef",
        );
        assert!(ext_board.contains("    FDT /dtbs/rockchip/rk3588-rock-5b.dtb"));
    }
}
