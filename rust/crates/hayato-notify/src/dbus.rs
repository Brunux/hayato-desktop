use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;

use crate::model::{ActionItem, AnimState, Notification, Rect, Urgency};

pub enum DbusCommand {
    New(Box<Notification>),
    Close(u32),
    Control(crate::model::Control),
}

pub struct NotificationServer {
    next_id: AtomicU32,
    tx: Sender<DbusCommand>,
    wake_fd: i32,
    default_timeout: Duration,
}

impl NotificationServer {
    pub fn new(tx: Sender<DbusCommand>, wake_fd: i32, default_timeout: Duration) -> Self {
        Self {
            next_id: AtomicU32::new(1),
            tx,
            wake_fd,
            default_timeout,
        }
    }

    fn notify_wake(&self) {
        if self.wake_fd >= 0 {
            let val: u64 = 1;
            unsafe {
                libc::write(
                    self.wake_fd,
                    &val as *const _ as *const libc::c_void,
                    std::mem::size_of::<u64>(),
                );
            }
        }
    }
}

/// The file a click should show: the explicit hint first, then grimshot's
/// absolute `image-path` (it sends the saved picture there and predates the
/// hint; `notify-send -i` puts the path in that hint, not in `app_icon`).
fn open_path_for(
    hint: Option<&str>,
    image_path: Option<&str>,
    app_name: &str,
    app_icon: &str,
) -> Option<PathBuf> {
    if let Some(path) = hint.filter(|path| path.starts_with('/')) {
        return Some(PathBuf::from(path));
    }
    if app_name != "grimshot" {
        return None;
    }
    image_path
        .filter(|path| path.starts_with('/'))
        .or_else(|| app_icon.starts_with('/').then_some(app_icon))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, Value<'_>>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        };

        // Parse urgency (0 = Low, 1 = Normal, 2 = Critical)
        let urgency = if let Some(val) = hints.get("urgency") {
            if let Ok(u) = u8::try_from(val) {
                Urgency::from_u8(u)
            } else {
                Urgency::Normal
            }
        } else {
            Urgency::Normal
        };

        // Parse category
        let category = hints.get("category").and_then(|v| match v {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        });

        // Parse progress value (0..=100)
        let progress = hints.get("value").and_then(|v| {
            if let Ok(n) = i32::try_from(v) {
                Some(n.clamp(0, 100) as u8)
            } else if let Ok(n) = u8::try_from(v) {
                Some(n.min(100))
            } else {
                None
            }
        });

        // Parse sync tag
        let sync_tag = hints
            .get("x-canonical-private-synchronous")
            .and_then(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            });

        // A card can name a file whose folder a click opens. grimshot predates
        // the hint and passes the saved picture as its icon path, so that is
        // read too - only for grimshot, where an absolute icon is the file.
        let open_hint = hints
            .get(hayato_ui::card::OPEN_PATH_HINT)
            .and_then(|value| match value {
                Value::Str(text) => Some(text.to_string()),
                _ => None,
            });
        let image_path = hints.get("image-path").and_then(|value| match value {
            Value::Str(text) => Some(text.to_string()),
            _ => None,
        });
        let open_path = open_path_for(
            open_hint.as_deref(),
            image_path.as_deref(),
            &app_name,
            &app_icon,
        );

        // Parse action items: pair of [id, label, id, label, ...]
        let mut action_items = Vec::new();
        let mut iter = actions.into_iter();
        while let (Some(key), Some(label)) = (iter.next(), iter.next()) {
            action_items.push(ActionItem { key, label });
        }
        // A file card gets one visible action even when its sender named none;
        // the daemon handles this key itself, so no D-Bus client is involved.
        if open_path.is_some() && !action_items.iter().any(|a| a.key == "hayato-open") {
            action_items.push(ActionItem {
                key: "hayato-open".to_string(),
                label: "Files".to_string(),
            });
        }

        let timeout_ms = match expire_timeout {
            t if t < 0 => match urgency {
                Urgency::Critical => 0, // Critical doesn't auto-expire
                _ => self.default_timeout.as_millis() as u64,
            },
            0 => 0,
            t => t as u64,
        };

        let notification = Notification {
            id,
            app_name,
            summary,
            body,
            app_icon,
            actions: action_items,
            urgency,
            category,
            progress,
            sync_tag,
            open_path,
            repeat_count: 1,
            created_at: std::time::Instant::now(),
            timeout: Duration::from_millis(timeout_ms),
            anim_state: AnimState::Opening {
                start: std::time::Instant::now(),
            },
            card_y: 0.0,
            card_height: 70.0,
            card_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            action_rects: Vec::new(),
            close_rect: None,
            is_hovered: false,
            hovered_close: false,
            hovered_action: None,
            hovered_at: None,
            dismissing: false,
            progress_color_override: None,
        };

        let _ = self.tx.send(DbusCommand::New(Box::new(notification)));
        self.notify_wake();
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.tx.send(DbusCommand::Close(id));
        self.notify_wake();
    }

    fn get_capabilities(&self) -> Vec<&'static str> {
        vec![
            "actions",
            "body",
            "body-hyperlinks",
            "body-markup",
            "icon-static",
            "persistence",
        ]
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("hayato-notify", "Hayato", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    pub async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_open_path_hint_wins_and_grimshot_falls_back_to_its_picture() {
        assert_eq!(
            open_path_for(
                Some("/home/b/out/a.png"),
                Some("/home/b/other.png"),
                "Hayato",
                "camera-web"
            ),
            Some(PathBuf::from("/home/b/out/a.png"))
        );
        assert_eq!(
            open_path_for(None, Some("/home/b/Pictures/a.png"), "grimshot", ""),
            Some(PathBuf::from("/home/b/Pictures/a.png"))
        );
        assert_eq!(
            open_path_for(None, None, "grimshot", "/home/b/icon.png"),
            Some(PathBuf::from("/home/b/icon.png"))
        );
        assert_eq!(
            open_path_for(None, Some("/home/b/icon.png"), "Hayato", ""),
            None
        );
        assert_eq!(open_path_for(Some(""), None, "Hayato", "camera-web"), None);
    }
}
