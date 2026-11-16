//! Data-driven parser for `/etc/lazyos/deviceinfo` and `/etc/lazyos/kernel-cmdline*.conf`.
//!
//! No shell is ever sourced at install time: lines are parsed strictly as
//! `key="value"` assignments matching the lazyos-devices / postmarketOS format.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Supported LazyOS boot architectures, selected by `deviceinfo_boot_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BootType {
    #[default]
    Depthcharge,
    Uefi,
    Extlinux,
}

impl BootType {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "depthcharge" => Ok(Self::Depthcharge),
            "uefi" => Ok(Self::Uefi),
            "extlinux" => Ok(Self::Extlinux),
            other => Err(format!("unsupported deviceinfo_boot_type '{other}'")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Depthcharge => "depthcharge",
            Self::Uefi => "uefi",
            Self::Extlinux => "extlinux",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Depthcharge => "ChromeOS Depthcharge",
            Self::Uefi => "UEFI (systemd-boot)",
            Self::Extlinux => "U-Boot / extlinux",
        }
    }
}

/// Typed hardware profile loaded from `/etc/lazyos/deviceinfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub codename: String,
    pub name: String,
    pub arch: String,
    pub boot_type: BootType,
    pub dtb: Option<String>,
    pub boot_size_mib: u64,
    pub boot_filesystem: String,
    pub extlinux_start_mib: u64,
    pub install_target: Option<String>,
    pub image_size_mib: u64,
    pub cgpt_kpart_start: u64,
    pub cgpt_kpart_size: u64,
    pub depthcharge_board: Option<String>,
    pub depthcharge_compression: String,
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self {
            codename: "google-asurada".to_string(),
            name: "Google Asurada Chromebook".to_string(),
            arch: "aarch64".to_string(),
            boot_type: BootType::Depthcharge,
            dtb: Some("mediatek/mt8192-asurada*".to_string()),
            boot_size_mib: 512,
            boot_filesystem: "ext2".to_string(),
            extlinux_start_mib: 16,
            install_target: Some("/dev/mmcblk0".to_string()),
            image_size_mib: 6144,
            cgpt_kpart_start: 8192,
            cgpt_kpart_size: 262144,
            depthcharge_board: Some("mt8192".to_string()),
            depthcharge_compression: "lzma".to_string(),
        }
    }
}

impl DeviceInfo {
    /// Strict `key="value"` parser; rejects unquoted syntax or shell substitutions.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for (idx, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, rest)) = trimmed.split_once('=') else {
                return Err(format!("line {}: expected key=\"value\"", idx + 1));
            };
            let key = key.trim();
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || key.is_empty() {
                return Err(format!("line {}: invalid key '{key}'", idx + 1));
            }
            let val = rest.trim();
            if !(val.starts_with('"') && val.ends_with('"') && val.len() >= 2) {
                return Err(format!(
                    "line {}: value for '{key}' must be double-quoted",
                    idx + 1
                ));
            }
            let inner = &val[1..val.len() - 1];
            if inner.contains('$') || inner.contains('`') {
                return Err(format!(
                    "line {}: shell expansion forbidden in '{key}'",
                    idx + 1
                ));
            }
            map.insert(key.to_string(), inner.to_string());
        }

        let boot_type = match map.get("deviceinfo_boot_type") {
            Some(v) => BootType::parse(v)?,
            None => BootType::Depthcharge,
        };

        let default_boot_size = match boot_type {
            BootType::Uefi => 256,
            BootType::Depthcharge | BootType::Extlinux => 512,
        };
        let default_boot_fs = match boot_type {
            BootType::Uefi => "vfat",
            BootType::Depthcharge | BootType::Extlinux => "ext2",
        };

        let parse_u64 = |k: &str, default: u64| -> Result<u64, String> {
            match map.get(k) {
                Some(s) if !s.trim().is_empty() => s
                    .trim()
                    .parse::<u64>()
                    .map_err(|e| format!("invalid integer for {k}: {e}")),
                _ => Ok(default),
            }
        };

        let non_empty = |k: &str| -> Option<String> {
            map.get(k)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        Ok(Self {
            codename: non_empty("deviceinfo_codename").unwrap_or_else(|| "generic".to_string()),
            name: non_empty("deviceinfo_name").unwrap_or_else(|| match boot_type {
                BootType::Depthcharge => "Chromebook (Depthcharge)".to_string(),
                BootType::Uefi => "ARM64 UEFI System".to_string(),
                BootType::Extlinux => "ARM64 U-Boot / SBC".to_string(),
            }),
            arch: non_empty("deviceinfo_arch").unwrap_or_else(|| "aarch64".to_string()),
            boot_type,
            dtb: non_empty("deviceinfo_dtb"),
            boot_size_mib: parse_u64("deviceinfo_boot_size_mib", default_boot_size)?,
            boot_filesystem: non_empty("deviceinfo_boot_filesystem")
                .unwrap_or_else(|| default_boot_fs.to_string()),
            extlinux_start_mib: parse_u64("deviceinfo_extlinux_start_mib", 16)?,
            install_target: non_empty("deviceinfo_install_target"),
            image_size_mib: parse_u64("deviceinfo_image_size_mib", 6144)?,
            cgpt_kpart_start: parse_u64("deviceinfo_cgpt_kpart_start", 8192)?,
            cgpt_kpart_size: parse_u64("deviceinfo_cgpt_kpart_size", 262144)?,
            depthcharge_board: non_empty("deviceinfo_depthcharge_board"),
            depthcharge_compression: non_empty("deviceinfo_depthcharge_compression")
                .unwrap_or_else(|| "lzma".to_string()),
        })
    }

    /// Load active device profile from `$LAZYOS_DEVICEINFO`, `/etc/lazyos/deviceinfo`,
    /// or `/usr/share/lazyos/deviceinfo`, defaulting safely on host test systems.
    pub fn load_from_machine() -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(explicit) = std::env::var("LAZYOS_DEVICEINFO") {
            if !explicit.is_empty() {
                candidates.push(PathBuf::from(explicit));
            }
        }
        candidates.push(PathBuf::from("/etc/lazyos/deviceinfo"));
        candidates.push(PathBuf::from("/usr/share/lazyos/deviceinfo"));

        for path in candidates {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(parsed) = Self::parse(&raw) {
                    return parsed;
                }
            }
        }
        Self::default()
    }
}

/// Read kernel command-line base fragments from `/etc/lazyos/kernel-cmdline.conf`
/// and optional `/etc/lazyos/kernel-cmdline-extra` inside `root`.
pub fn load_cmdline_fragments(root: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for rel in [
        "etc/lazyos/kernel-cmdline.conf",
        "usr/share/lazyos/kernel-cmdline.conf",
        "etc/lazyos/kernel-cmdline-extra",
    ] {
        let path = root.join(rel);
        // Prefer /etc/lazyos/kernel-cmdline.conf over /usr/share/lazyos when both exist.
        if rel == "usr/share/lazyos/kernel-cmdline.conf" && !parts.is_empty() {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            for line in raw.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                for word in line.split_whitespace() {
                    parts.push(word.to_string());
                }
            }
        }
    }
    if parts.is_empty() {
        "quiet loglevel=2".to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hayato_deviceinfo() {
        let raw = r#"
deviceinfo_name="Google Asurada Chromebook"
deviceinfo_codename="google-asurada"
deviceinfo_arch="aarch64"
deviceinfo_dtb="mediatek/mt8192-asurada*"
deviceinfo_cgpt_kpart_start="8192"
deviceinfo_cgpt_kpart_size="262144"
deviceinfo_depthcharge_board="mt8192"
deviceinfo_depthcharge_compression="lzma"
deviceinfo_boot_type="depthcharge"
deviceinfo_boot_size_mib="512"
deviceinfo_boot_filesystem="ext2"
deviceinfo_image_size_mib="6144"
deviceinfo_install_target="/dev/mmcblk0"
"#;
        let info = DeviceInfo::parse(raw).unwrap();
        assert_eq!(info.boot_type, BootType::Depthcharge);
        assert_eq!(info.codename, "google-asurada");
        assert_eq!(info.boot_size_mib, 512);
        assert_eq!(info.boot_filesystem, "ext2");
        assert_eq!(info.install_target.as_deref(), Some("/dev/mmcblk0"));
        assert_eq!(info.depthcharge_board.as_deref(), Some("mt8192"));
    }

    #[test]
    fn parses_uefi_and_extlinux_profiles() {
        let uefi = DeviceInfo::parse(
            r#"
deviceinfo_codename="arm64-uefi"
deviceinfo_name="ARM64 Generic UEFI"
deviceinfo_arch="aarch64"
deviceinfo_boot_type="uefi"
deviceinfo_dtb=""
"#,
        )
        .unwrap();
        assert_eq!(uefi.boot_type, BootType::Uefi);
        assert_eq!(uefi.boot_size_mib, 256);
        assert_eq!(uefi.boot_filesystem, "vfat");
        assert_eq!(uefi.dtb, None);
        assert_eq!(uefi.install_target, None);

        let ext = DeviceInfo::parse(
            r#"
deviceinfo_codename="rock-5b"
deviceinfo_name="Radxa ROCK 5B"
deviceinfo_arch="aarch64"
deviceinfo_boot_type="extlinux"
deviceinfo_boot_size_mib="512"
deviceinfo_extlinux_start_mib="32"
deviceinfo_dtb="rockchip/rk3588-rock-5b.dtb"
"#,
        )
        .unwrap();
        assert_eq!(ext.boot_type, BootType::Extlinux);
        assert_eq!(ext.extlinux_start_mib, 32);
        assert_eq!(ext.boot_filesystem, "ext2");
        assert_eq!(ext.dtb.as_deref(), Some("rockchip/rk3588-rock-5b.dtb"));
    }

    #[test]
    fn rejects_shell_syntax_and_unknown_boot_type() {
        assert!(DeviceInfo::parse("deviceinfo_name=$(uname)").is_err());
        assert!(DeviceInfo::parse("deviceinfo_name=\"$(id)\"").is_err());
        assert!(DeviceInfo::parse("deviceinfo_boot_type=\"grub\"").is_err());
    }
}
