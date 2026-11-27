//! Package-transaction safety (`hayato menu pkg`, Setup bundles, VPN plugin install).
//!
//! Every install, removal and upgrade the UI starts is **simulated first** with
//! `apt-get -s` and judged here, before any `sudo` runs. The rule exists because
//! an install of `wireguard-tools` once pulled `linux-image-arm64` through
//! *Recommends*, and Debian's kernel postinst hook then flashed the Chromebook's
//! kernel partition with a kernel Hayato does not boot. So:
//!
//! - Installs always pass `--no-install-recommends` (the image is built that way).
//! - A transaction that would install or upgrade a kernel or boot package, or
//!   remove a package the session cannot live without, is **blocked**: the UI
//!   offers no way to continue and names the packages.
//! - A transaction that removes more than was asked for, or is unusually large,
//!   is a **warning**: the confirm opens with Cancel selected.
//!
//! The rules are pure functions over the text `apt-get -s` prints, so a fixture
//! from a real machine is a unit test. The guard only covers UI paths: a shell
//! running `apt` bypasses it, which the manual says in plain words.

use std::process::{Command, Stdio};

/// What a simulated transaction would do, by package name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub installs: Vec<String>,
    pub upgrades: Vec<String>,
    pub removes: Vec<String>,
}

/// What the guard decided about a [`Plan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Continue only after an explicit confirm (Cancel selected first).
    Warn(Vec<String>),
    /// Never continue from the UI.
    Block(Vec<String>),
}

/// Parse the `Inst` / `Remv` lines of `apt-get -s`. `Conf` lines and the rest
/// are ignored. An `Inst` whose second token is `[old-version]` is an upgrade.
pub fn parse_simulation(text: &str) -> Plan {
    let mut plan = Plan::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("Inst") => {
                let Some(name) = words.next() else { continue };
                if words.next().is_some_and(|w| w.starts_with('[')) {
                    plan.upgrades.push(name.to_string());
                } else {
                    plan.installs.push(name.to_string());
                }
            }
            Some("Remv") => {
                if let Some(name) = words.next() {
                    plan.removes.push(name.to_string());
                }
            }
            _ => {}
        }
    }
    plan
}

/// Kernel, module and boot-chain packages. Installing or upgrading one of these
/// from the UI would rewrite the boot partition (see the module docs).
pub fn is_kernel_or_boot(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "linux-image",
        "linux-modules",
        "linux-kbuild",
        "linux-binary",
        "linux-base",
        "u-boot",
        "grub-",
        "grub2",
        "vboot",
        "depthcharge",
    ];
    name == "cgpt" || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Packages whose removal would leave the machine or the session unusable.
pub fn is_removal_protected(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "systemd",
        "libc6",
        "apt",
        "dpkg",
        "initramfs-tools",
        "network-manager",
        "sudo",
        "greetd",
        "foot",
        "bash",
        "coreutils",
        "util-linux",
    ];
    is_kernel_or_boot(name)
        || name.starts_with("hayato")
        || name.starts_with("sway")
        || EXACT.contains(&name)
}

/// Judge a simulated transaction. `requested` are the package names the person
/// asked for, so removing exactly those is not "extra".
pub fn evaluate(plan: &Plan, requested: &[&str]) -> Verdict {
    let mut block = Vec::new();
    for name in plan.installs.iter().chain(&plan.upgrades) {
        if is_kernel_or_boot(name) {
            block.push(format!("would install or upgrade {name}"));
        }
    }
    for name in &plan.removes {
        if is_removal_protected(name) {
            block.push(format!("would remove {name}"));
        }
    }
    if !block.is_empty() {
        if !requested.is_empty() {
            block.push(format!("requested: {}", requested.join(" ")));
        }
        block.push("kernel and boot packages change only through the system updater".to_string());
        return Verdict::Block(block);
    }

    let mut warn = Vec::new();
    let extra: Vec<&str> = plan
        .removes
        .iter()
        .map(String::as_str)
        .filter(|name| !requested.contains(name))
        .collect();
    if !extra.is_empty() {
        let shown: Vec<&str> = extra.iter().copied().take(6).collect();
        let more = extra.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        warn.push(format!(
            "also removes {} package{}: {}{tail}",
            extra.len(),
            if extra.len() == 1 { "" } else { "s" },
            shown.join(", ")
        ));
    }
    if plan.installs.len() > 40 {
        warn.push(format!("installs {} new packages", plan.installs.len()));
    }
    if warn.is_empty() {
        Verdict::Allow
    } else {
        Verdict::Warn(warn)
    }
}

/// Run `apt-get -s` with these arguments and parse what it would do. No root
/// and no lock are needed; a failure is the first `E:` line or exit status.
pub fn simulate(args: &[&str]) -> Result<Plan, String> {
    let output = Command::new("apt-get")
        .args(["-s", "-o", "Debug::NoLocking=1"])
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run apt-get: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = stderr
            .lines()
            .find(|line| line.starts_with("E:"))
            .or_else(|| stderr.lines().find(|line| !line.trim().is_empty()))
            .unwrap_or("apt-get refused the simulation");
        return Err(line.trim().to_string());
    }
    Ok(parse_simulation(&String::from_utf8_lossy(&output.stdout)))
}

/// Simulate installing `names` (without Recommends) and judge the result.
pub fn check_install(names: &[&str]) -> Result<Verdict, String> {
    let mut args = vec!["install", "--no-install-recommends"];
    args.extend_from_slice(names);
    Ok(evaluate(&simulate(&args)?, names))
}

/// Simulate removing `names` and judge the result.
pub fn check_remove(names: &[&str]) -> Result<Verdict, String> {
    let mut args = vec!["remove"];
    args.extend_from_slice(names);
    Ok(evaluate(&simulate(&args)?, names))
}

/// Simulate `full-upgrade` on the current lists and judge the result.
pub fn check_full_upgrade() -> Result<Verdict, String> {
    Ok(evaluate(
        &simulate(&["full-upgrade", "--no-install-recommends"])?,
        &[],
    ))
}

/// One line for a status bar or banner: why a transaction did not start.
pub fn summary(lines: &[String]) -> String {
    lines.first().cloned().unwrap_or_default()
}

/// The argv every UI install uses: non-interactive, never Recommends.
pub fn install_argv(name: &str) -> Vec<String> {
    [
        "sudo",
        "-n",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "--no-install-recommends",
        "-o",
        "Dpkg::Options::=--force-confdef",
        "-o",
        "Dpkg::Options::=--force-confold",
        name,
    ]
    .iter()
    .map(|word| (*word).to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `apt-get -s install wireguard-tools` printed on the Chromebook the
    /// day the kernel partition was rewritten (Recommends enabled).
    const WIREGUARD_WITH_RECOMMENDS: &str = "\
Reading package lists...
Inst linux-base-7.3.5+deb14-arm64 (7.3.5-1 Debian:forky [all])
Inst linux-image-7.3.5+deb14-arm64 (7.3.5-1 Debian:forky [arm64])
Inst linux-image-arm64 (7.3.5-1 Debian:forky [arm64])
Inst wireguard-tools (1.0.20260223-2 Debian:forky [arm64])
Conf wireguard-tools (1.0.20260223-2 Debian:forky [arm64])
";

    #[test]
    fn the_recommends_chain_that_flashed_the_kernel_is_blocked() {
        let plan = parse_simulation(WIREGUARD_WITH_RECOMMENDS);
        assert_eq!(plan.installs.len(), 4);
        let Verdict::Block(lines) = evaluate(&plan, &["wireguard-tools"]) else {
            panic!("a kernel install must be blocked");
        };
        assert!(lines.iter().any(|l| l.contains("linux-image-arm64")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("requested: wireguard-tools"))
        );
    }

    #[test]
    fn the_same_install_without_recommends_is_allowed() {
        let plan = parse_simulation(
            "Inst wireguard-tools (1.0.20260223-2 Debian:forky [arm64])\n\
             Conf wireguard-tools (1.0.20260223-2 Debian:forky [arm64])\n",
        );
        assert_eq!(evaluate(&plan, &["wireguard-tools"]), Verdict::Allow);
    }

    #[test]
    fn a_kernel_upgrade_is_blocked_and_told_apart_from_an_install() {
        let plan = parse_simulation(
            "Inst linux-image-arm64 [7.2.7-1] (7.3.5-1 Debian:forky [arm64])\n\
             Inst openssl [3.5.1-1] (3.5.2-1 Debian-Security:forky [arm64])\n",
        );
        assert_eq!(plan.upgrades.len(), 2);
        assert!(plan.installs.is_empty());
        assert!(matches!(evaluate(&plan, &[]), Verdict::Block(_)));
    }

    #[test]
    fn hayato_and_session_packages_cannot_be_removed_but_can_be_upgraded() {
        let remove = parse_simulation("Remv hayato-desktop [0.6.1]\nRemv sway [1.11]\n");
        assert!(matches!(evaluate(&remove, &["sway"]), Verdict::Block(_)));
        let upgrade =
            parse_simulation("Inst hayato-desktop [0.6.1] (0.7.0 Hayato:forky [arm64])\n");
        assert_eq!(evaluate(&upgrade, &[]), Verdict::Allow);
    }

    #[test]
    fn removing_more_than_was_asked_is_a_warning_that_names_the_rest() {
        let plan =
            parse_simulation("Remv kdenlive [1.0]\nRemv kdenlive-data [1.0]\nRemv mlt [2.0]\n");
        let Verdict::Warn(lines) = evaluate(&plan, &["kdenlive"]) else {
            panic!("dependents are removed too");
        };
        assert!(lines[0].contains("also removes 2 packages"), "{lines:?}");
        assert!(lines[0].contains("kdenlive-data"));
        assert_eq!(
            evaluate(&parse_simulation("Remv kdenlive [1.0]\n"), &["kdenlive"]),
            Verdict::Allow
        );
    }

    #[test]
    fn a_huge_install_warns() {
        let text: String = (0..45)
            .map(|i| format!("Inst lib{i} (1.0 Debian:forky [arm64])\n"))
            .collect();
        let plan = parse_simulation(&text);
        assert!(matches!(evaluate(&plan, &["app"]), Verdict::Warn(_)));
    }

    #[test]
    fn protected_patterns_cover_the_boot_chain() {
        for name in [
            "linux-image-arm64",
            "linux-modules-7.3.5+deb14-arm64",
            "u-boot-tools",
            "grub-efi-arm64",
            "vboot-utils",
            "depthcharge-tools",
            "cgpt",
        ] {
            assert!(is_kernel_or_boot(name), "{name}");
        }
        assert!(!is_kernel_or_boot("linux-perf"));
        assert!(!is_kernel_or_boot("wireguard-tools"));
        assert!(is_removal_protected("hayato-greeter"));
        assert!(!is_removal_protected("kdenlive"));
    }

    #[test]
    fn the_install_argv_never_installs_recommends() {
        let argv = install_argv("kdenlive");
        assert!(argv.contains(&"--no-install-recommends".to_string()));
        assert_eq!(argv.last().map(String::as_str), Some("kdenlive"));
        assert_eq!(&argv[..2], ["sudo", "-n"]);
    }
}
