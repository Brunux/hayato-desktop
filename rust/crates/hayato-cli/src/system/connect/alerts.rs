//! Scoped D-Bus notification watcher + desktop notification relay for
//! Hayato Connect (`[4] Alerts`).
//!
//! Browsers cannot silently read mobile OS notification shades, so Alerts
//! mirrors **Desktop -> Phone** foreground notifications (honoring secret/locked
//! and active recording suppression) and **Phone -> Desktop** explicit alerts
//! pushed from connected browsers or companion forwarder hooks.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct AlertWatcher {
    stop: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Drop for AlertWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut g) = self.child.lock()
            && let Some(mut c) = g.take()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl AlertWatcher {
    /// Start listening for desktop `org.freedesktop.Notifications.Notify` calls
    /// strictly while `ConnectScreen` is open.
    pub fn start<F>(enabled: bool, on_alert: F) -> Self
    where
        F: Fn(String, String, String) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let enabled_flag = Arc::new(AtomicBool::new(enabled));
        let child_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

        let stop_bg = Arc::clone(&stop);
        let en_bg = Arc::clone(&enabled_flag);
        let slot_bg = Arc::clone(&child_slot);

        let _ = std::thread::Builder::new()
            .name("connect-alerts".into())
            .spawn(move || {
                let Ok(mut child) = Command::new("dbus-monitor")
                    .args([
                        "--session",
                        "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    return;
                };
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                };
                if let Ok(mut g) = slot_bg.lock() {
                    *g = Some(child);
                }

                let reader = BufReader::new(stdout);
                let mut current_strings: Vec<String> = Vec::new();
                let mut in_notify = false;
                for line in reader.lines() {
                    if stop_bg.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(line) = line else { break };
                    let t = line.trim();
                    if t.starts_with("method call ") && t.contains("member=Notify") {
                        in_notify = true;
                        current_strings.clear();
                        continue;
                    }
                    if in_notify {
                        if let Some(s) = parse_dbus_string_arg(t) {
                            current_strings.push(s);
                            // D-Bus Notify positional strings:
                            // 0=app_name, 1=app_icon, 2=summary, 3=body
                            if current_strings.len() == 4 {
                                in_notify = false;
                                if en_bg.load(Ordering::Relaxed)
                                    && !hayato_ui::recording::is_recording()
                                    && should_forward_app(&current_strings[0])
                                {
                                    on_alert(
                                        current_strings[0].clone(),
                                        current_strings[2].clone(),
                                        current_strings[3].clone(),
                                    );
                                }
                            }
                        } else if t.starts_with("method ") || t.starts_with("signal ") {
                            in_notify = false;
                        }
                    }
                }
            });

        Self {
            stop,
            enabled: enabled_flag,
            child: child_slot,
        }
    }

    pub fn set_enabled(&self, val: bool) {
        self.enabled.store(val, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

pub fn parse_dbus_string_arg(trimmed_line: &str) -> Option<String> {
    let rest = trimmed_line.strip_prefix("string \"")?;
    let val = rest.strip_suffix('"')?;
    Some(val.replace("\\\"", "\"").replace("\\\\", "\\"))
}

pub fn should_forward_app(app: &str) -> bool {
    let low = app.trim().to_ascii_lowercase();
    !low.is_empty()
        && !low.contains("hayato connect")
        && !low.contains("bitwarden")
        && !low.contains("keepass")
}

/// Forward an authenticated phone alert onto the Hayato desktop via Hayato's
/// native notification card helper (`crate::system::say`).
pub fn send_desktop_alert(from: &str, title: &str, body: &str) -> bool {
    let clean_title = if title.trim().is_empty() {
        format!("Alert from {from}")
    } else {
        format!("{from}: {}", title.trim())
    };
    crate::system::say(
        &clean_title,
        body.trim(),
        hayato_ui::Urgency::Normal,
        hayato_ui::Glyph::Bell,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_parser_and_loop_filter() {
        assert_eq!(
            parse_dbus_string_arg("string \"Signal\""),
            Some("Signal".to_string())
        );
        assert_eq!(parse_dbus_string_arg("uint32 0"), None);
        assert!(should_forward_app("Signal"));
        assert!(!should_forward_app("Hayato Connect"));
        assert!(!should_forward_app("KeePassXC"));
    }
}
