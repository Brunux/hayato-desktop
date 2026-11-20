//! Language and locale (`localectl`), Settings → System.
//!
//! The session's locale is system-wide (`LANG` in `/etc/locale.conf`), so a
//! change applies at the next login. The picker lists `localectl list-locales`
//! and writes the chosen value with `localectl set-locale`; the locale is also
//! generated when `locale-gen` is present, because a name that is set but not
//! generated gives every program a warning instead of a translated UI.

use super::{out, sudo};

pub fn current() -> String {
    parse_status(&out(&["localectl", "status"]).unwrap_or_default())
}

fn parse_status(text: &str) -> String {
    text.lines()
        .find_map(|line| {
            let line = line.trim();
            let value = line.strip_prefix("System Locale:")?.trim();
            value
                .split_whitespace()
                .find_map(|item| item.strip_prefix("LANG="))
                .map(|locale| locale.to_string())
        })
        .unwrap_or_else(|| "C.UTF-8".to_string())
}

pub fn list() -> Vec<String> {
    let mut locales: Vec<String> = out(&["localectl", "list-locales"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if locales.is_empty() {
        locales = out(&["locale", "-a"])
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
    }
    locales.sort();
    locales.dedup();
    locales
}

pub fn set(locale: &str) -> Result<(), String> {
    if locale.is_empty() || locale.contains(char::is_whitespace) {
        return Err("that is not a locale name".to_string());
    }
    sudo(&["localectl", "set-locale", &format!("LANG={locale}")])?;
    // Debian keeps locale generation separate. A missing helper is not fatal:
    // the locale is still selected, and a later login can generate it.
    if std::path::Path::new("/usr/sbin/locale-gen").is_file() {
        let _ = sudo(&["/usr/sbin/locale-gen", locale]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_the_lang_value() {
        let text = "   System Locale: LANG=en_US.UTF-8\n       VC Keymap: (unset)\n";
        assert_eq!(parse_status(text), "en_US.UTF-8");
    }

    #[test]
    fn a_status_without_lang_falls_back_to_c_utf8() {
        assert_eq!(parse_status("System Locale: (unset)"), "C.UTF-8");
        assert_eq!(parse_status(""), "C.UTF-8");
    }
}
