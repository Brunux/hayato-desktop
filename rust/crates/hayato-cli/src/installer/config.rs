//! Configuration data structures, validation rules, and safe serialization for the Hayato installer.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Installation mode: interactive customization (`Fresh`) or headless live clone (`Clone`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallMode {
    #[default]
    Fresh,
    Clone,
}

/// Target configuration for a complete Hayato installation onto storage.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallConfig {
    /// Whether to apply new identity/locale/password (`Fresh`) or clone live user (`Clone`).
    #[serde(default)]
    pub mode: InstallMode,
    /// Full display name of the primary user (e.g., "Bruno").
    pub fullname: String,
    /// POSIX username for system login and home directory (e.g., "brunux").
    pub username: String,
    /// Plaintext password kept out of serialization/logs; transferred transiently over stdin pipe.
    #[serde(default, skip_serializing)]
    pub password: String,
    /// Confirmation password kept out of serialization/logs.
    #[serde(default, skip_serializing)]
    pub password_confirm: String,
    /// System hostname (default "hayato").
    pub hostname: String,
    /// Selected IANA timezone (default "UTC" or system-detected).
    pub timezone: String,
    /// Selected system locale (default "en_US.UTF-8").
    pub locale: String,
    /// Selected console and X11/Wayland keyboard layout (default "us").
    pub keymap: String,
    /// Whether to erase inherited machine identity and generate fresh SSH host keys.
    pub regenerate_ssh_keys: bool,
    /// Target block device path (e.g., "/dev/mmcblk0" or "/dev/nvme0n1").
    pub target_disk: String,
    /// Whether to run in non-destructive dry-run simulation mode.
    pub dry_run: bool,
    /// Explicit shared IPC/log directory (used by elevated worker `/run/hayato-install`).
    #[serde(default)]
    pub state_dir: Option<String>,
}

/// Private transient stdin payload sent from the unprivileged TUI to root `hayato installer apply --stdin`.
#[derive(Serialize, Deserialize)]
pub struct ApplyRequest {
    pub config: InstallConfig,
    #[serde(default)]
    pub secret_password: Option<String>,
}

impl fmt::Debug for InstallConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallConfig")
            .field("mode", &self.mode)
            .field("fullname", &self.fullname)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("hostname", &self.hostname)
            .field("timezone", &self.timezone)
            .field("locale", &self.locale)
            .field("keymap", &self.keymap)
            .field("regenerate_ssh_keys", &self.regenerate_ssh_keys)
            .field("target_disk", &self.target_disk)
            .field("dry_run", &self.dry_run)
            .field("state_dir", &self.state_dir)
            .finish()
    }
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            mode: InstallMode::Fresh,
            fullname: "Hayato User".to_string(),
            username: "brunux".to_string(),
            password: String::new(),
            password_confirm: String::new(),
            hostname: "hayato".to_string(),
            timezone: "UTC".to_string(),
            locale: "en_US.UTF-8".to_string(),
            keymap: "us".to_string(),
            regenerate_ssh_keys: true,
            target_disk: "/dev/mmcblk0".to_string(),
            dry_run: false,
            state_dir: None,
        }
    }
}

impl InstallConfig {
    /// Validate all user credential and hostname fields.
    pub fn validate_user_fields(&self) -> Result<(), &'static str> {
        validate_username(&self.username)?;
        validate_password(&self.password, &self.password_confirm)?;
        validate_hostname(&self.hostname)?;
        Ok(())
    }
}

/// Derive short confirmation token from block device path (e.g., `/dev/nvme0n1` -> `nvme0n1`).
pub fn target_short_name(target_disk: &str) -> &str {
    target_disk
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(target_disk)
}

/// Validate username against strict POSIX rules: `^[a-z_][a-z0-9_-]{0,31}$`.
/// Rejects empty usernames, root, and illegal characters.
pub fn validate_username(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Username cannot be empty");
    }
    if name.len() > 32 {
        return Err("Username must be at most 32 characters");
    }
    if name == "root" {
        return Err("Username 'root' is reserved");
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && first != '_' {
        return Err("Username must start with a lowercase letter or underscore");
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' && c != '-' {
            return Err(
                "Username can only contain lowercase letters, numbers, hyphens, and underscores",
            );
        }
    }
    Ok(())
}

/// Validate that password is non-empty and matches the confirmation.
pub fn validate_password(p1: &str, p2: &str) -> Result<(), &'static str> {
    if p1.is_empty() {
        return Err("Password cannot be empty");
    }
    if p1 != p2 {
        return Err("Passwords do not match");
    }
    Ok(())
}

/// Validate system hostname syntax: alphanumeric and hyphens, 1-63 chars, no leading/trailing hyphen.
pub fn validate_hostname(host: &str) -> Result<(), &'static str> {
    if host.is_empty() {
        return Err("Hostname cannot be empty");
    }
    if host.len() > 63 {
        return Err("Hostname must be at most 63 characters");
    }
    if host.starts_with('-') || host.ends_with('-') {
        return Err("Hostname cannot start or end with a hyphen");
    }
    for c in host.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err("Hostname can only contain letters, numbers, and hyphens");
        }
    }
    Ok(())
}

/// Validate typed confirmation string against target disk name or explicit DESTROY keyword.
pub fn validate_confirmation(input: &str, target_name: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed == target_name || trimmed.eq_ignore_ascii_case("DESTROY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_validation_accepts_posix_identifiers() {
        assert!(validate_username("brunux").is_ok());
        assert!(validate_username("_service").is_ok());
        assert!(validate_username("user-123").is_ok());
        assert!(validate_username("u_456").is_ok());
    }

    #[test]
    fn username_validation_rejects_invalid_names() {
        assert_eq!(validate_username(""), Err("Username cannot be empty"));
        assert_eq!(
            validate_username("root"),
            Err("Username 'root' is reserved")
        );
        assert_eq!(
            validate_username("123user"),
            Err("Username must start with a lowercase letter or underscore")
        );
        assert_eq!(
            validate_username("Brunux"),
            Err("Username must start with a lowercase letter or underscore")
        );
        assert_eq!(
            validate_username("user name"),
            Err("Username can only contain lowercase letters, numbers, hyphens, and underscores")
        );
        assert_eq!(
            validate_username("user@home"),
            Err("Username can only contain lowercase letters, numbers, hyphens, and underscores")
        );
        let long = "a".repeat(33);
        assert_eq!(
            validate_username(&long),
            Err("Username must be at most 32 characters")
        );
    }

    #[test]
    fn password_matching_logic() {
        assert!(validate_password("secret123", "secret123").is_ok());
        assert_eq!(validate_password("", ""), Err("Password cannot be empty"));
        assert_eq!(
            validate_password("abc", "xyz"),
            Err("Passwords do not match")
        );
    }

    #[test]
    fn hostname_validation_rules() {
        assert!(validate_hostname("hayato").is_ok());
        assert!(validate_hostname("chromebook-mt8192").is_ok());
        assert!(validate_hostname("host1").is_ok());
        assert_eq!(validate_hostname(""), Err("Hostname cannot be empty"));
        assert_eq!(
            validate_hostname("-invalid"),
            Err("Hostname cannot start or end with a hyphen")
        );
        assert_eq!(
            validate_hostname("invalid-"),
            Err("Hostname cannot start or end with a hyphen")
        );
        assert_eq!(
            validate_hostname("host.local"),
            Err("Hostname can only contain letters, numbers, and hyphens")
        );
        let long = "h".repeat(64);
        assert_eq!(
            validate_hostname(&long),
            Err("Hostname must be at most 63 characters")
        );
    }

    #[test]
    fn confirmation_gate_matching() {
        assert!(validate_confirmation("mmcblk0", "mmcblk0"));
        assert!(validate_confirmation("  nvme0n1  ", "nvme0n1"));
        assert!(validate_confirmation("DESTROY", "mmcblk0"));
        assert!(validate_confirmation("destroy", "nvme0n1"));
        assert!(!validate_confirmation("", "mmcblk0"));
        assert!(!validate_confirmation("sda", "mmcblk0"));
        assert!(!validate_confirmation("yes", "mmcblk0"));
    }

    #[test]
    fn secrets_never_leak_into_debug_or_serialized_config() {
        let cfg = InstallConfig {
            password: "ultra-secret-password".to_string(),
            password_confirm: "ultra-secret-password".to_string(),
            ..Default::default()
        };
        let debug = format!("{cfg:?}");
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!debug.contains("ultra-secret-password"));
        assert!(!json.contains("ultra-secret-password"));
    }

    #[test]
    fn target_short_name_handles_all_block_paths() {
        assert_eq!(target_short_name("/dev/mmcblk0"), "mmcblk0");
        assert_eq!(target_short_name("/dev/nvme0n1"), "nvme0n1");
        assert_eq!(target_short_name("/dev/vda/"), "vda");
    }
}
