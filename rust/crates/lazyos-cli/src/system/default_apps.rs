//! Default applications (`xdg-mime` / `gio`), Settings → System.
//!
//! The list of candidates is what `gio mime <type>` reports as registered for
//! that type; the current choice is `xdg-mime query default`. Setting a browser
//! also updates `xdg-settings`, because the browser is looked up through the
//! scheme handler and through the desktop setting, and the two must agree.

use super::{checked, out};

#[derive(Clone, Copy, Debug)]
pub struct Category {
    pub key: &'static str,
    pub label: &'static str,
    pub mime: &'static str,
    pub browser: bool,
}

pub const CATEGORIES: &[Category] = &[
    Category {
        key: "browser",
        label: "Web browser",
        mime: "x-scheme-handler/http",
        browser: true,
    },
    Category {
        key: "files",
        label: "File manager",
        mime: "inode/directory",
        browser: false,
    },
    Category {
        key: "text",
        label: "Text editor",
        mime: "text/plain",
        browser: false,
    },
    Category {
        key: "image",
        label: "Image viewer",
        mime: "image/png",
        browser: false,
    },
    Category {
        key: "video",
        label: "Video player",
        mime: "video/mp4",
        browser: false,
    },
    Category {
        key: "audio",
        label: "Music player",
        mime: "audio/mpeg",
        browser: false,
    },
    Category {
        key: "pdf",
        label: "PDF viewer",
        mime: "application/pdf",
        browser: false,
    },
];

pub fn category(key: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|category| category.key == key)
}

fn parse_gio_mime(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "Registered applications:" || trimmed == "Recommended applications:" {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        if trimmed.is_empty() {
            in_list = false;
            continue;
        }
        if trimmed.ends_with(".desktop") && !ids.iter().any(|id| id == trimmed) {
            ids.push(trimmed.to_string());
        }
    }
    ids
}

fn desktop_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![
        std::path::PathBuf::from("/usr/share/applications"),
        std::path::PathBuf::from("/usr/local/share/applications"),
    ];
    if let Ok(data) = std::env::var("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(data).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/share/applications"));
    }
    dirs
}

pub fn desktop_name(id: &str) -> String {
    for dir in desktop_dirs() {
        let path = dir.join(id);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("Name=") {
                let name = name.trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    id.strip_suffix(".desktop").unwrap_or(id).to_string()
}

pub fn current(category: &Category) -> Option<String> {
    if category.browser
        && let Some(browser) = out(&["xdg-settings", "get", "default-web-browser"])
    {
        let browser = browser.trim();
        if !browser.is_empty() {
            return Some(browser.to_string());
        }
    }
    out(&["xdg-mime", "query", "default", category.mime])
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

pub fn candidates(category: &Category) -> Vec<String> {
    let mut ids = out(&["gio", "mime", category.mime])
        .map(|text| parse_gio_mime(&text))
        .unwrap_or_default();
    if let Some(current) = current(category)
        && !ids.iter().any(|id| id == &current)
    {
        ids.insert(0, current);
    }
    ids
}

pub fn set(category: &Category, id: &str) -> Result<(), String> {
    if category.browser {
        checked(&["xdg-settings", "set", "default-web-browser", id])?;
        checked(&["xdg-mime", "default", id, "x-scheme-handler/http"])?;
        checked(&["xdg-mime", "default", id, "x-scheme-handler/https"])?;
    } else {
        checked(&["xdg-mime", "default", id, category.mime])?;
    }
    Ok(())
}

pub fn summary() -> String {
    let browser = category("browser").expect("browser category");
    current(browser)
        .map(|id| desktop_name(&id))
        .unwrap_or_else(|| "not set".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gio_output_is_read_without_duplicates() {
        let text = "\
Default application for “text/plain”: org.lazyos.view.desktop
Registered applications:
\torg.lazyos.view.desktop
\tnvim.desktop
\tvim.desktop
\torg.lazyos.view.desktop
Recommended applications:
\tnvim.desktop
";
        assert_eq!(
            parse_gio_mime(text),
            ["org.lazyos.view.desktop", "nvim.desktop", "vim.desktop"]
        );
    }

    #[test]
    fn categories_have_unique_keys() {
        let mut keys: Vec<&str> = CATEGORIES.iter().map(|category| category.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), CATEGORIES.len());
    }
}
