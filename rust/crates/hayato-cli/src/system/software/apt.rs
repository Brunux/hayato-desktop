//! APT & dpkg read-only queries, parsers, simulation, and cleanup measurement
//! for Hayato Software (`hayato menu software`).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::meta_cache;
use crate::system::clean::{self, Reclaimable};
use crate::system::out;
use crate::system::pkg_guard::{self, Plan, Verdict};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPkg {
    pub name: String,
    pub version: String,
    pub installed_kb: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageMeta {
    pub package: String,
    pub candidate: String,
    pub section: String,
    pub download_bytes: u64,
    pub installed_kb: u64,
    pub homepage: String,
    pub origin: String,
    pub summary: String,
    pub description: Vec<String>,
}

impl PackageMeta {
    /// Human-readable repository & trust badge (`Debian main`, `Debian non-free`, `Hayato APT`, etc.).
    pub fn source_badge(&self) -> (&'static str, &'static str) {
        let sec = self.section.as_str();
        if self.package.starts_with("hayato") || self.origin.contains("hayato") {
            ("Hayato APT", "signed repository")
        } else if sec.starts_with("non-free-firmware/") || sec == "non-free-firmware" {
            ("Debian non-free-firmware", "proprietary firmware")
        } else if sec.starts_with("non-free/") || sec == "non-free" {
            ("Debian non-free", "non-free license")
        } else if sec.starts_with("contrib/") || sec == "contrib" {
            ("Debian contrib", "depends on non-free components")
        } else {
            ("Debian main", "DFSG-free signed repository")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateEntry {
    pub package: String,
    pub suite: String,
    pub old_version: String,
    pub new_version: String,
    pub is_security: bool,
    pub held_for_safety: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebianMatch {
    pub package: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessFacts {
    pub installed: bool,
    pub services: Vec<String>,
    pub conffiles_count: usize,
    pub has_setuid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionPreview {
    pub package: String,
    pub remove: bool,
    pub plan: Plan,
    pub verdict: Verdict,
    pub net_disk_kb: i64,
    pub blocked_or_err: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupSnapshot {
    pub reclaimable: Reclaimable,
    pub autoremove_pkgs: Vec<String>,
    pub autoremove_kb: u64,
    pub residual_configs: Vec<String>,
    pub old_kernel_count: usize,
}

impl CleanupSnapshot {
    pub fn total_reclaimable_bytes(&self) -> u64 {
        self.reclaimable
            .safe_reclaimable_bytes()
            .saturating_add(self.autoremove_kb.saturating_mul(1024))
    }
}

pub fn format_kb(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
    } else if kb >= 1024 {
        format!("{:.1} MB", kb as f64 / 1024.0)
    } else {
        format!("{kb} KB")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "0 B".to_string()
    } else if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Single-call `dpkg-query` returning every installed package with version and size.
pub fn dpkg_installed() -> HashMap<String, InstalledPkg> {
    let raw = out(&[
        "dpkg-query",
        "-W",
        "-f=${db:Status-Abbrev}\t${Package}\t${Version}\t${Installed-Size}\n",
    ])
    .unwrap_or_default();
    parse_dpkg_installed(&raw)
}

pub fn parse_dpkg_installed(text: &str) -> HashMap<String, InstalledPkg> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(4, '\t');
        let Some(status) = parts.next() else { continue };
        let Some(pkg) = parts.next() else { continue };
        let Some(ver) = parts.next() else { continue };
        let kb = parts
            .next()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if status.trim_start().starts_with("ii") {
            let name = pkg.trim().to_string();
            map.insert(
                name.clone(),
                InstalledPkg {
                    name,
                    version: ver.trim().to_string(),
                    installed_kb: kb,
                },
            );
        }
    }
    map
}

pub fn manual_packages() -> HashSet<String> {
    out(&["apt-mark", "showmanual"])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn residual_config_packages() -> Vec<String> {
    let raw =
        out(&["dpkg-query", "-W", "-f=${db:Status-Abbrev}\t${Package}\n"]).unwrap_or_default();
    raw.lines()
        .filter_map(|line| {
            let (st, pkg) = line.split_once('\t')?;
            (st.trim() == "rc").then(|| pkg.trim().to_string())
        })
        .collect()
}

/// Parse multi-stanza `apt-cache show --no-all-versions` output keyed by package name.
pub fn parse_apt_show_multi(text: &str) -> HashMap<String, PackageMeta> {
    let mut out_map = HashMap::new();
    for stanza in text.split("\n\n") {
        let trimmed = stanza.trim();
        if trimmed.is_empty() {
            continue;
        }
        let meta = parse_apt_show_single(trimmed);
        if !meta.package.is_empty() {
            out_map.entry(meta.package.clone()).or_insert(meta);
        }
    }
    out_map
}

pub fn parse_apt_show_single(text: &str) -> PackageMeta {
    let mut meta = PackageMeta::default();
    let mut in_desc = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Package:") {
            meta.package = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Version:") {
            meta.candidate = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Section:") {
            meta.section = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Size:") {
            meta.download_bytes = rest.trim().parse().unwrap_or(0);
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Installed-Size:") {
            meta.installed_kb = rest.trim().parse().unwrap_or(0);
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("Homepage:") {
            meta.homepage = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line.strip_prefix("APT-Sources:") {
            meta.origin = rest.trim().to_string();
            in_desc = false;
        } else if let Some(rest) = line
            .strip_prefix("Description-en:")
            .or_else(|| line.strip_prefix("Description:"))
        {
            let s = rest.trim().to_string();
            meta.summary = s.clone();
            meta.description.push(s);
            in_desc = true;
        } else if in_desc && line.starts_with(' ') {
            let t = line.trim();
            if t == "." {
                meta.description.push(String::new());
            } else {
                meta.description.push(t.to_string());
            }
        } else {
            in_desc = false;
        }
    }
    meta
}

/// Fetch `PackageMeta` for a batch of packages, reading/updating the XDG cache.
pub fn fetch_metadata_batch(pkgs: &[&str]) -> HashMap<String, PackageMeta> {
    let stamp = meta_cache::apt_lists_stamp();
    let mut result = HashMap::new();
    let mut missing = Vec::new();
    for &p in pkgs {
        if let Some(cached) = meta_cache::read(p, stamp) {
            result.insert(p.to_string(), cached);
        } else {
            missing.push(p);
        }
    }
    if missing.is_empty() {
        return result;
    }
    let output = Command::new("apt-cache")
        .arg("show")
        .arg("--no-all-versions")
        .arg("--")
        .args(&missing)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    if let Ok(out_data) = output
        && out_data.status.success()
    {
        let parsed = parse_apt_show_multi(&String::from_utf8_lossy(&out_data.stdout));
        for (pkg, meta) in parsed {
            meta_cache::write(&pkg, stamp, &meta);
            result.insert(pkg, meta);
        }
    }
    result
}

/// Run `apt list --upgradable` and parse pending upgrades.
pub fn fetch_upgradable() -> Vec<UpdateEntry> {
    let raw = Command::new("apt")
        .args(["list", "--upgradable"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    parse_upgradable(&raw)
}

pub fn parse_upgradable(text: &str) -> Vec<UpdateEntry> {
    let mut list = Vec::new();
    for line in text.lines() {
        if !line.contains("[upgradable from:") {
            continue;
        }
        let Some((left, from_part)) = line.split_once("[upgradable from:") else {
            continue;
        };
        let old_version = from_part.trim().trim_end_matches(']').trim().to_string();
        let mut tokens = left.split_whitespace();
        let Some(pkg_suite) = tokens.next() else {
            continue;
        };
        let Some(new_version) = tokens.next() else {
            continue;
        };
        let (package, suite) = match pkg_suite.split_once('/') {
            Some((p, s)) => (p.to_string(), s.to_string()),
            None => (pkg_suite.to_string(), String::new()),
        };
        let is_security = suite.contains("-security") || suite.contains("security");
        let held_for_safety = pkg_guard::is_kernel_or_boot(&package);
        list.push(UpdateEntry {
            package,
            suite,
            old_version,
            new_version: new_version.to_string(),
            is_security,
            held_for_safety,
        });
    }
    list.sort_by(|a, b| {
        b.is_security
            .cmp(&a.is_security)
            .then_with(|| a.held_for_safety.cmp(&b.held_for_safety))
            .then_with(|| a.package.cmp(&b.package))
    });
    list
}

/// Run a debounced `apt-cache search --names-only` fallback.
pub fn search_debian_names(query: &str, limit: usize) -> Vec<DebianMatch> {
    let output = Command::new("apt-cache")
        .args(["search", "--names-only", "--", query])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out_data) = output else {
        return Vec::new();
    };
    if !out_data.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out_data.stdout)
        .lines()
        .take(limit)
        .filter_map(|line| {
            let (pkg, desc) = line.split_once(" - ")?;
            let pkg = pkg.trim();
            if pkg_guard::is_kernel_or_boot(pkg) {
                return None;
            }
            Some(DebianMatch {
                package: pkg.to_string(),
                summary: desc.trim().chars().take(72).collect(),
            })
        })
        .collect()
}

/// Simulate install/remove transaction via `pkg_guard` and compute net disk delta.
pub fn simulate_transaction(pkg: &str, remove: bool) -> TransactionPreview {
    let args: Vec<&str> = if remove {
        vec!["remove", pkg]
    } else {
        vec!["install", "--no-install-recommends", pkg]
    };
    match pkg_guard::simulate(&args) {
        Ok(plan) => {
            let verdict = pkg_guard::evaluate(&plan, &[pkg]);
            let delta_kb = if remove {
                let refs: Vec<&str> = plan.removes.iter().map(String::as_str).collect();
                let meta = fetch_metadata_batch(&refs);
                -(meta.values().map(|m| m.installed_kb as i64).sum::<i64>())
            } else {
                let refs: Vec<&str> = plan.installs.iter().map(String::as_str).collect();
                let meta = fetch_metadata_batch(&refs);
                meta.values().map(|m| m.installed_kb as i64).sum::<i64>()
            };
            TransactionPreview {
                package: pkg.to_string(),
                remove,
                plan,
                verdict,
                net_disk_kb: delta_kb,
                blocked_or_err: None,
            }
        }
        Err(err) => TransactionPreview {
            package: pkg.to_string(),
            remove,
            plan: Plan::default(),
            verdict: Verdict::Allow,
            net_disk_kb: 0,
            blocked_or_err: Some(err),
        },
    }
}

/// Probe truthful permission & system-impact facts for an installed package.
pub fn probe_access_facts(pkg: &str, installed: bool) -> AccessFacts {
    if !installed {
        return AccessFacts {
            installed: false,
            services: Vec::new(),
            conffiles_count: 0,
            has_setuid: false,
        };
    }
    let files = out(&["dpkg", "-L", pkg]).unwrap_or_default();
    let mut services = Vec::new();
    let mut has_setuid = false;
    for path in files.lines() {
        let p = path.trim();
        if (p.starts_with("/lib/systemd/system/") || p.starts_with("/usr/lib/systemd/system/"))
            && p.ends_with(".service")
            && let Some(fname) = Path::new(p).file_name().and_then(|n| n.to_str())
        {
            services.push(fname.to_string());
        }
        if !has_setuid
            && (p.starts_with("/usr/bin/")
                || p.starts_with("/bin/")
                || p.starts_with("/usr/libexec/"))
            && let Ok(meta) = fs::metadata(p)
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.is_file() && (meta.permissions().mode() & 0o6000) != 0 {
                has_setuid = true;
            }
        }
    }
    let conffiles_raw = out(&["dpkg-query", "-W", "-f=${Conffiles}\n", pkg]).unwrap_or_default();
    let conffiles_count = conffiles_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    AccessFacts {
        installed: true,
        services,
        conffiles_count,
        has_setuid,
    }
}

/// Measure reclaimable software caches + unused apt autoremove dependencies.
pub fn measure_cleanup(installed: &HashMap<String, InstalledPkg>) -> CleanupSnapshot {
    let reclaimable = clean::calculate_reclaimable();
    let mut autoremove_pkgs = Vec::new();
    if let Ok(plan) = pkg_guard::simulate(&["autoremove"]) {
        autoremove_pkgs = plan
            .removes
            .into_iter()
            .filter(|p| !pkg_guard::is_removal_protected(p))
            .collect();
    }
    let autoremove_kb: u64 = autoremove_pkgs
        .iter()
        .filter_map(|p| installed.get(p))
        .map(|i| i.installed_kb)
        .sum();
    let residual_configs = residual_config_packages();
    let old_kernel_count = installed
        .keys()
        .filter(|k| k.starts_with("linux-image-"))
        .count()
        .saturating_sub(1);
    CleanupSnapshot {
        reclaimable,
        autoremove_pkgs,
        autoremove_kb,
        residual_configs,
        old_kernel_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_stanza_apt_show() {
        let sample = "\
Package: shotcut
Version: 26.11.15+ds-1
Section: video
Size: 4512340
Installed-Size: 21196
Homepage: https://shotcut.org/
APT-Sources: http://deb.debian.org/debian forky/main arm64 Packages
Description-en: video editor
 Shotcut is a free, open source video editor.
 .
 Supports many formats.

Package: kdenlive
Version: 26.08.3-1
Section: video
Size: 4500000
Installed-Size: 20700
Description: non-linear video editor
";
        let map = parse_apt_show_multi(sample);
        assert_eq!(map.len(), 2);
        let s = &map["shotcut"];
        assert_eq!(s.candidate, "26.11.15+ds-1");
        assert_eq!(s.download_bytes, 4_512_340);
        assert_eq!(s.installed_kb, 21196);
        assert_eq!(s.source_badge().0, "Debian main");
    }

    #[test]
    fn parses_apt_upgradable_with_security_and_kernel_hold() {
        let sample = "\
Listing... Done
openssl/forky-security arm64 3.5.2-1 [upgradable from: 3.5.1-1]
linux-image-arm64/forky arm64 7.3.5-1 [upgradable from: 7.2.7-1]
firefox-esr/forky arm64 153.6.0esr-1 [upgradable from: 153.5.1esr-1]
";
        let ups = parse_upgradable(sample);
        assert_eq!(ups.len(), 3);
        assert_eq!(ups[0].package, "openssl");
        assert!(ups[0].is_security);
        assert!(!ups[0].held_for_safety);
        let kern = ups
            .iter()
            .find(|u| u.package == "linux-image-arm64")
            .unwrap();
        assert!(kern.held_for_safety);
    }

    #[test]
    fn parses_dpkg_installed_table() {
        let sample = "\
ii \tbtop\t1.4.7-1\t1842
rc \told-pkg\t1.0\t100
ii \tkdenlive\t26.08.3-1\t20700
";
        let map = parse_dpkg_installed(sample);
        assert_eq!(map.len(), 2);
        assert_eq!(map["btop"].installed_kb, 1842);
        assert!(!map.contains_key("old-pkg"));
    }
}
