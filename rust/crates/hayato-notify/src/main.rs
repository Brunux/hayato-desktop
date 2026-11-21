#![allow(unknown_lints)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::chunks_exact_to_as_chunks)]

mod config;
mod control;
mod dbus;
mod font;
mod history;
mod model;
mod render;
mod wayland;

use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use wayland_client::Connection;
use zbus::blocking::connection::Builder;

use crate::dbus::{DbusCommand, NotificationServer};
use crate::history::{append_record, record_from_notification};
use crate::model::{ANIM_DURATION, AnimState, Notification, Urgency};
use crate::render::CardTheme;
use crate::wayland::{UiEvent, WaylandState};

static THEME_DIRTY: AtomicBool = AtomicBool::new(false);
static GLOBAL_WAKE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn sigusr1_handler(_: libc::c_int) {
    THEME_DIRTY.store(true, Ordering::SeqCst);
    let fd = GLOBAL_WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let val: u64 = 1;
        unsafe {
            libc::write(
                fd,
                &val as *const _ as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A question, not a daemon: `check-desktop.sh` asks whether a card of this
    // urgency would be drawn right now, so the "no cards in a recording" rule
    // has an executable witness that needs no Wayland session.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--control") {
        let word = args.get(1).map(String::as_str).unwrap_or("");
        std::process::exit(control::send(word));
    }
    if args.first().map(String::as_str) == Some("--config") {
        let config = config::load();
        println!(
            "default_timeout={} max_visible={} card_width={:.0} history_limit={} group_by_sender={} collapse_repeats={}",
            config.default_timeout.as_millis(),
            config.max_visible,
            config.card_width,
            config.history_limit,
            if config.group_by_sender { "yes" } else { "no" },
            if config.collapse_repeats { "yes" } else { "no" }
        );
        return Ok(());
    }

    if args.first().map(String::as_str) == Some("--would-draw") {
        let critical = args
            .iter()
            .skip(1)
            .any(|arg| arg == "--critical" || arg == "--urgency=critical")
            || args
                .windows(2)
                .any(|pair| pair[0] == "--urgency" && pair[1] == "critical");
        println!(
            "{}",
            if hayato_ui::recording::would_draw(critical) {
                "draw"
            } else {
                "suppress"
            }
        );
        return Ok(());
    }

    let config = config::load();
    let (dbus_tx, dbus_rx) = channel::<DbusCommand>();
    let (ui_tx, ui_rx) = channel::<UiEvent>();

    // 0. Setup eventfd for instantaneous wakeups on D-Bus notifications and signals
    let wake_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if wake_fd >= 0 {
        GLOBAL_WAKE_FD.store(wake_fd, Ordering::SeqCst);
        unsafe {
            libc::signal(libc::SIGUSR1, sigusr1_handler as *const () as usize);
        }
    }

    // 1. Start D-Bus thread and the one-word control socket
    let control_tx = dbus_tx.clone();
    let server = NotificationServer::new(dbus_tx, wake_fd, config.default_timeout);
    let _control = match control::listen(control_tx, wake_fd) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("hayato-notify: cannot listen for control: {error}");
            None
        }
    };

    let _dbus_thread = thread::spawn(
        move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            // Fallback to /run/user/$UID/bus if DBUS_SESSION_BUS_ADDRESS is unset
            if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err() {
                let uid = unsafe { libc::getuid() };
                let bus_path = format!("/run/user/{uid}/bus");
                if std::path::Path::new(&bus_path).exists() {
                    unsafe {
                        std::env::set_var(
                            "DBUS_SESSION_BUS_ADDRESS",
                            format!("unix:path={bus_path}"),
                        );
                    }
                }
            }

            let connection = Builder::session()?
                .name("org.freedesktop.Notifications")?
                .serve_at("/org/freedesktop/Notifications", server)?
                .build()?;

            let iface_ref = connection
                .object_server()
                .interface::<_, NotificationServer>("/org/freedesktop/Notifications")?;

            while let Ok(event) = ui_rx.recv() {
                match event {
                    UiEvent::ActionInvoked(id, key) => {
                        let _ = zbus::block_on(NotificationServer::action_invoked(
                            iface_ref.signal_emitter(),
                            id,
                            &key,
                        ));
                    }
                    UiEvent::NotificationClosed(id, reason) => {
                        let _ = zbus::block_on(NotificationServer::notification_closed(
                            iface_ref.signal_emitter(),
                            id,
                            reason,
                        ));
                    }
                }
            }

            Ok(())
        },
    );

    // 2. Connect to Wayland
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hayato-notify: could not connect to Wayland display: {e}");
            return Ok(());
        }
    };

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = WaylandState::new(ui_tx.clone(), &config);

    let display = conn.display();
    let _ = display.get_registry(&qh, ());

    // Initial roundtrip to bind globals
    event_queue.roundtrip(&mut state)?;
    event_queue.roundtrip(&mut state)?;

    let mut last_check = Instant::now();

    // 3. Main event loop
    loop {
        // Dispatch Wayland events
        event_queue.dispatch_pending(&mut state)?;
        let _ = conn.flush();

        let mut dirty = false;

        // Check theme reload signal
        if THEME_DIRTY.swap(false, Ordering::SeqCst) {
            state.renderer.theme = CardTheme::from_active();
            if !state.notifications.is_empty() {
                dirty = true;
            }
        }

        // Process incoming D-Bus commands
        while let Ok(cmd) = dbus_rx.try_recv() {
            match cmd {
                DbusCommand::New(notif) => {
                    let notif = *notif;
                    append_record(&record_from_notification(&notif), config.history_limit);
                    state.renderer.theme = CardTheme::from_active();

                    // A recorder is writing the screen to a file, so a card
                    // drawn now is a card in that file. History above keeps
                    // the record; the card itself is dropped. Critical cards
                    // still draw: a dying battery outranks a clean capture.
                    if !hayato_ui::recording::would_draw(notif.urgency == Urgency::Critical) {
                        close_non_critical(&mut state.notifications, &mut dirty);
                        // The card was never shown, so it is closed as far as
                        // the client is concerned (reason 1: expired).
                        let _ = ui_tx.send(UiEvent::NotificationClosed(notif.id, 1));
                        continue;
                    }

                    // An identical repeat becomes one card with a count, not a
                    // stack of identical cards.
                    if config.collapse_repeats {
                        if let Some(old_id) = collapse_repeat(&mut state.notifications, &notif) {
                            if old_id != notif.id {
                                let _ = ui_tx.send(UiEvent::NotificationClosed(old_id, 2));
                            }
                            dirty = true;
                            continue;
                        }
                    }

                    // Check for in-place replacement (replaces_id or matching sync_tag)
                    let mut replaced = false;
                    for existing in &mut state.notifications {
                        let matches_tag = notif.sync_tag.is_some()
                            && existing.sync_tag.as_ref() == notif.sync_tag.as_ref();
                        let matches_id = notif.id == existing.id;

                        if matches_id || matches_tag {
                            if existing.id != notif.id {
                                let _ = ui_tx.send(UiEvent::NotificationClosed(existing.id, 2));
                                existing.id = notif.id;
                            }
                            existing.summary = notif.summary.clone();
                            existing.body = notif.body.clone();
                            existing.app_icon = notif.app_icon.clone();
                            existing.progress = notif.progress;
                            existing.open_path = notif.open_path.clone();
                            existing.actions = notif.actions.clone();
                            existing.urgency = notif.urgency;
                            existing.created_at = Instant::now();
                            existing.timeout = notif.timeout;
                            existing.anim_state = AnimState::Active;
                            existing.hovered_action = None;
                            replaced = true;
                            dirty = true;
                            break;
                        }
                    }

                    if !replaced {
                        let insert_at = if config.group_by_sender {
                            state
                                .notifications
                                .iter()
                                .rposition(|n| n.app_name == notif.app_name)
                                .map(|index| index + 1)
                                .unwrap_or(state.notifications.len())
                        } else {
                            state.notifications.len()
                        };
                        state.notifications.insert(insert_at, notif);
                        dirty = true;
                    }
                }
                DbusCommand::Control(command) => {
                    if state.control(command) {
                        dirty = true;
                    }
                }
                DbusCommand::Close(id) => {
                    let reduced = state.renderer.reduced_motion;
                    if reduced {
                        let before = state.notifications.len();
                        state.notifications.retain(|n| n.id != id);
                        if state.notifications.len() != before {
                            dirty = true;
                            let _ = ui_tx.send(UiEvent::NotificationClosed(id, 2));
                        }
                    } else {
                        for n in &mut state.notifications {
                            if n.id == id && !n.is_closing() {
                                n.anim_state = AnimState::Closing {
                                    start: Instant::now(),
                                    reason: 2,
                                };
                                dirty = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Expiry & subtle countdown bar check (every 100 ms)
        if last_check.elapsed() >= Duration::from_millis(100) {
            last_check = Instant::now();
            let reduced = state.renderer.reduced_motion;
            for n in &mut state.notifications {
                if n.is_expired() && !n.is_closing() {
                    if reduced {
                        n.anim_state = AnimState::Closing {
                            start: Instant::now() - ANIM_DURATION,
                            reason: 1,
                        };
                    } else {
                        n.anim_state = AnimState::Closing {
                            start: Instant::now(),
                            reason: 1,
                        };
                    }
                    dirty = true;
                } else if !n.is_closing() && !n.is_hovered && n.remaining_fraction().is_some() {
                    // Advance subtle bottom countdown hairline smoothly.
                    dirty = true;
                }
            }
        }

        // Animation state transitions
        let mut closed = Vec::new();
        let mut is_animating = false;

        for n in &mut state.notifications {
            match n.anim_state {
                AnimState::Opening { start } => {
                    if start.elapsed() >= ANIM_DURATION {
                        n.anim_state = AnimState::Active;
                        dirty = true;
                    } else {
                        is_animating = true;
                        dirty = true;
                    }
                }
                AnimState::Active => {}
                AnimState::Closing { start, reason } => {
                    if start.elapsed() >= ANIM_DURATION {
                        closed.push((n.id, reason));
                    } else {
                        is_animating = true;
                        dirty = true;
                    }
                }
            }
        }

        if !closed.is_empty() {
            state
                .notifications
                .retain(|n| !closed.iter().any(|(id, _)| *id == n.id));
            for (id, reason) in closed {
                if reason != 0 {
                    let _ = ui_tx.send(UiEvent::NotificationClosed(id, reason));
                }
            }
            dirty = true;
        }

        if dirty {
            if state.notifications.is_empty() {
                state.destroy_surface();
                let _ = conn.flush();
            } else {
                let metrics = state.renderer.compute_layout(&mut state.notifications);
                let target_w = metrics.total_width.ceil() as u32;
                let target_h = metrics.total_height.ceil() as u32;

                if state.surface.is_none() {
                    state.setup_layer_surface(&qh, target_w, target_h);
                    event_queue.roundtrip(&mut state)?;
                    let _ = conn.flush();
                } else if target_w != state.width || target_h != state.height {
                    state.width = target_w;
                    state.height = target_h;
                    if let Some(ref ls) = state.layer_surface {
                        ls.set_size(target_w, target_h);
                    }
                    if let Some(ref s) = state.surface {
                        s.commit();
                    }
                    event_queue.roundtrip(&mut state)?;
                    let _ = conn.flush();
                } else {
                    state.paint_surface(&qh);
                    let _ = conn.flush();
                }
            }
        }

        // Prepare poll for socket & wake_fd
        if let Some(guard) = conn.prepare_read() {
            let wayland_fd = guard.connection_fd().as_raw_fd();
            let mut pfds = [
                libc::pollfd {
                    fd: wayland_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let timeout = if state.notifications.is_empty() {
                -1 // Idle poll: wait indefinitely for wake_fd or Wayland event
            } else if is_animating {
                16 // 60 fps animation loop (16 ms)
            } else {
                50 // Active animation/timer poll
            };

            let _ = conn.flush();
            let ret = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout) };
            if ret > 0 {
                if pfds[0].revents & libc::POLLIN != 0 {
                    let _ = guard.read();
                }
                if pfds[1].revents & libc::POLLIN != 0 {
                    let mut buf = 0u64;
                    let _ = unsafe {
                        libc::read(
                            wake_fd,
                            &mut buf as *mut _ as *mut libc::c_void,
                            std::mem::size_of::<u64>(),
                        )
                    };
                }
            }
        }

        event_queue.dispatch_pending(&mut state)?;
        let _ = conn.flush();
    }
}

/// Fold an identical repeat into the card already on screen. Returns the id
/// that was on screen before the new one replaced it, so the caller can close
/// the old client's card.
fn collapse_repeat(notifications: &mut [Notification], notif: &Notification) -> Option<u32> {
    let existing = notifications.iter_mut().find(|n| {
        n.app_name == notif.app_name
            && n.summary == notif.summary
            && n.body == notif.body
            && n.urgency == notif.urgency
            && n.open_path == notif.open_path
            && n.actions == notif.actions
    })?;
    let old_id = existing.id;
    existing.id = notif.id;
    existing.repeat_count = existing.repeat_count.saturating_add(1);
    existing.created_at = Instant::now();
    existing.timeout = notif.timeout;
    existing.anim_state = AnimState::Active;
    existing.hovered_action = None;
    existing.open_path = notif.open_path.clone();
    existing.actions = notif.actions.clone();
    Some(old_id)
}

/// Close the cards that a recorder would capture. Critical cards stay: a dying
/// battery matters more than a clean file.
fn close_non_critical(notifications: &mut [Notification], dirty: &mut bool) {
    let reduced = std::env::var("HAYATO_REDUCED_MOTION").is_ok();
    for notification in notifications {
        if notification.urgency != Urgency::Critical && !notification.is_closing() {
            notification.anim_state = AnimState::Closing {
                start: if reduced {
                    Instant::now() - ANIM_DURATION
                } else {
                    Instant::now()
                },
                reason: 2,
            };
            *dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    fn notification(id: u32, app: &str, body: &str) -> Notification {
        Notification {
            id,
            app_name: app.to_string(),
            summary: "Saved".to_string(),
            body: body.to_string(),
            app_icon: String::new(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            category: None,
            progress: None,
            sync_tag: None,
            open_path: None,
            repeat_count: 1,
            created_at: Instant::now(),
            timeout: Duration::from_secs(5),
            anim_state: AnimState::Active,
            card_y: 0.0,
            card_height: 70.0,
            card_bounds: Rect::new(0.0, 0.0, 300.0, 70.0),
            action_rects: Vec::new(),
            close_rect: None,
            is_hovered: false,
            hovered_close: false,
            hovered_action: None,
            hovered_at: None,
            dismissing: false,
            progress_color_override: None,
        }
    }

    #[test]
    fn identical_repeats_collapse_and_replace_the_id() {
        let mut cards = vec![notification(1, "Hayato", "Saved a.png")];
        let repeat = notification(2, "Hayato", "Saved a.png");
        assert_eq!(collapse_repeat(&mut cards, &repeat), Some(1));
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, 2);
        assert_eq!(cards[0].repeat_count, 2);
    }

    #[test]
    fn a_different_body_is_a_new_card() {
        let mut cards = vec![notification(1, "Hayato", "Saved a.png")];
        let other = notification(2, "Hayato", "Saved b.png");
        assert_eq!(collapse_repeat(&mut cards, &other), None);
        assert_eq!(cards[0].repeat_count, 1);
    }

    #[test]
    fn the_same_text_for_another_file_is_a_new_card() {
        let mut first = notification(1, "Hayato", "Saved a.png");
        first.open_path = Some(std::path::PathBuf::from("/home/b/Pictures/a.png"));
        let mut second = notification(2, "Hayato", "Saved a.png");
        second.open_path = Some(std::path::PathBuf::from("/home/b/Videos/a.png"));
        let mut cards = vec![first];
        assert_eq!(collapse_repeat(&mut cards, &second), None);
        assert_eq!(cards[0].repeat_count, 1);
    }
}
