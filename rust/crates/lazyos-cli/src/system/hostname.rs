//! The machine's hostname (`hostnamectl`), Settings → System.
//!
//! `hostnamectl set-hostname` writes `/etc/hostname`; `/etc/hosts` carries the
//! old name on the `127.0.1.1` line, so the same change rewrites that line. The
//! installer's validator is reused, which keeps a hostname typed here subject
//! to the same rules as one typed during installation.

use super::{out, sudo};
use crate::installer::config::validate_hostname;

pub fn current() -> String {
    out(&["hostnamectl", "hostname"])
        .or_else(|| out(&["hostname"]))
        .unwrap_or_else(|| "lazyos".to_string())
        .trim()
        .to_string()
}

pub fn set(name: &str) -> Result<(), String> {
    let name = name.trim();
    validate_hostname(name).map_err(str::to_string)?;
    sudo(&["hostnamectl", "set-hostname", name])?;
    // Keep the local host entry in step. A missing line is not an error: the
    // hostname itself is already written.
    let expression = format!("s/^127\\.0\\.1\\.1[[:space:]].*/127.0.1.1\\t{name}/");
    let _ = sudo(&["/usr/bin/sed", "-i", &expression, "/etc/hosts"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_rules_are_the_settings_rules() {
        assert!(validate_hostname("lazyos").is_ok());
        assert!(validate_hostname("my-chromebook").is_ok());
        assert!(validate_hostname("").is_err());
        assert!(validate_hostname("-bad").is_err());
        assert!(validate_hostname("two words").is_err());
    }
}
