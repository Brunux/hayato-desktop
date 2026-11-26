//! Curated software capability bundles for Step 6 (Software) of the Setup Assistant.
//!
//! Instead of dropping a new user into raw `apt-cache search`, bundles group
//! practical desktop capabilities and check what is already installed via
//! `dpkg-query` + local model paths.

use std::collections::HashSet;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleAction {
    Apt(&'static [&'static str]),
    LocalAiModels,
}

#[derive(Clone, Copy, Debug)]
pub struct CapabilityBundle {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub est_mb: u32,
    pub action: BundleAction,
}

pub const BUNDLES: &[CapabilityBundle] = &[
    CapabilityBundle {
        id: "office",
        name: "Office & Documents",
        summary: "LibreOffice Writer/Calc + PDF & eBook reading",
        est_mb: 320,
        action: BundleAction::Apt(&["libreoffice-writer", "libreoffice-calc"]),
    },
    CapabilityBundle {
        id: "creative",
        name: "Media & Creative",
        summary: "GIMP image editor, Inkscape vector art, Audacity audio",
        est_mb: 240,
        action: BundleAction::Apt(&["gimp", "inkscape", "audacity"]),
    },
    CapabilityBundle {
        id: "dev",
        name: "Developer Toolkit",
        summary: "Git, Python 3, build-essential, Podman containers & tmux",
        est_mb: 280,
        action: BundleAction::Apt(&["git", "python3", "build-essential", "podman", "tmux"]),
    },
    CapabilityBundle {
        id: "games",
        name: "Games & Emulation",
        summary: "RetroArch + classic console cores",
        est_mb: 160,
        action: BundleAction::Apt(&["retroarch"]),
    },
    CapabilityBundle {
        id: "local-ai",
        name: "Offline AI & Voice Models",
        summary: "Qwen3.5-0.8B local assistant (~517 MB) + Whisper dictation (~74 MB)",
        est_mb: 600,
        action: BundleAction::LocalAiModels,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleStatus {
    pub id: &'static str,
    pub installed: usize,
    pub total: usize,
}

impl BundleStatus {
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.installed >= self.total
    }

    pub fn label(&self) -> String {
        if self.is_complete() {
            "Installed".to_string()
        } else if self.installed > 0 {
            format!("Partial ({}/{})", self.installed, self.total)
        } else {
            "Available".to_string()
        }
    }
}

pub fn installed_dpkg_set() -> HashSet<String> {
    let Ok(out) = Command::new("dpkg-query")
        .args(["-W", "-f=${Package}\t${Status}\n"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return HashSet::new();
    };
    if !out.status.success() {
        return HashSet::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pkg, status) = line.split_once('\t')?;
            status
                .contains("install ok installed")
                .then(|| pkg.to_string())
        })
        .collect()
}

pub fn probe_all(
    installed_pkgs: &HashSet<String>,
    local_llm: bool,
    whisper: bool,
) -> Vec<BundleStatus> {
    BUNDLES
        .iter()
        .map(|b| match b.action {
            BundleAction::Apt(pkgs) => {
                let count = pkgs.iter().filter(|p| installed_pkgs.contains(**p)).count();
                BundleStatus {
                    id: b.id,
                    installed: count,
                    total: pkgs.len(),
                }
            }
            BundleAction::LocalAiModels => {
                let count = usize::from(local_llm) + usize::from(whisper);
                BundleStatus {
                    id: b.id,
                    installed: count,
                    total: 2,
                }
            }
        })
        .collect()
}

/// Build the shell command lines to install a batch of selected bundle IDs.
pub fn build_install_script(selected_ids: &[&str]) -> Option<String> {
    let mut apt_pkgs: Vec<&str> = Vec::new();
    let mut fetch_models = false;
    for id in selected_ids {
        let Some(bundle) = BUNDLES.iter().find(|b| b.id == *id) else {
            continue;
        };
        match bundle.action {
            BundleAction::Apt(pkgs) => {
                for p in pkgs {
                    if !apt_pkgs.contains(p) {
                        apt_pkgs.push(p);
                    }
                }
            }
            BundleAction::LocalAiModels => fetch_models = true,
        }
    }
    let mut parts = Vec::new();
    if !apt_pkgs.is_empty() {
        parts.push(format!(
            "sudo -n apt-get update && sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
            apt_pkgs.join(" ")
        ));
    }
    if fetch_models {
        parts.push(
            "\"$HOME/.config/bin/hayato-model\" --fetch && \"$HOME/.config/bin/dictate-model\" --fetch"
                .to_string(),
        );
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" && "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_partial_and_complete_bundles() {
        let mut installed = HashSet::new();
        installed.insert("retroarch".to_string());
        installed.insert("git".to_string());
        let statuses = probe_all(&installed, true, false);
        let games = statuses.iter().find(|s| s.id == "games").unwrap();
        assert!(games.is_complete());
        let dev = statuses.iter().find(|s| s.id == "dev").unwrap();
        assert_eq!(dev.label(), "Partial (1/5)");
        let ai = statuses.iter().find(|s| s.id == "local-ai").unwrap();
        assert_eq!(ai.label(), "Partial (1/2)");
    }

    #[test]
    fn builds_batched_install_script() {
        let script = build_install_script(&["games", "local-ai"]).unwrap();
        assert!(script.contains("apt-get install -y retroarch"));
        assert!(script.contains("hayato-model"));
    }
}
