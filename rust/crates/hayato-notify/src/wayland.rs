use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Instant;

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::config::Config;
use crate::model::{AnimState, Control, Notification};
use crate::render::Renderer;

pub enum UiEvent {
    ActionInvoked(u32, String),
    NotificationClosed(u32, u32), // id, reason (1 = expired, 2 = user dismissed)
}

/// The folder Files should show: the file's parent, or the directory itself.
fn files_dir(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

/// Open Files at the folder holding `path`. `hayato-files` has no terminal of
/// its own here, so it opens its `foot` window and returns; the daemon is not
/// blocked while the person browses.
fn open_in_files(path: &Path) {
    let Some(dir) = files_dir(path) else {
        return;
    };
    let opened = Command::new("hayato-files")
        .arg("--")
        .arg(&dir)
        .env_remove("HAYATO_UI")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = opened {
        eprintln!(
            "hayato-notify: cannot open Files at {}: {error}",
            dir.display()
        );
    }
}

pub struct WaylandState {
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub shm: Option<wl_shm::WlShm>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub seat: Option<wl_seat::WlSeat>,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub surface: Option<wl_surface::WlSurface>,
    pub layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,

    pub width: u32,
    pub height: u32,
    pub configured: bool,

    pub pointer_x: f64,
    pub pointer_y: f64,

    pub notifications: Vec<Notification>,
    pub renderer: Renderer,
    pub event_tx: Sender<UiEvent>,
}

impl WaylandState {
    pub fn new(event_tx: Sender<UiEvent>, config: &Config) -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            surface: None,
            layer_surface: None,
            width: 0,
            height: 0,
            configured: false,
            pointer_x: 0.0,
            pointer_y: 0.0,
            notifications: Vec::new(),
            renderer: Renderer::with_layout(config.card_width, config.max_visible),
            event_tx,
        }
    }

    /// Act on the newest card without taking keyboard focus. A file card opens
    /// its folder, a card with an action presses it, and a plain card closes.
    pub fn control(&mut self, command: Control) -> bool {
        match command {
            Control::DismissAll => {
                if self.notifications.is_empty() {
                    return false;
                }
                let ids: Vec<u32> = self.notifications.iter().map(|n| n.id).collect();
                self.notifications.clear();
                for id in ids {
                    let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
                }
                true
            }
            Control::DismissNewest => {
                let Some(index) = self.newest_index() else {
                    return false;
                };
                self.close_at(index);
                true
            }
            Control::ActNewest => {
                let Some(index) = self.newest_index() else {
                    return false;
                };
                let id = self.notifications[index].id;
                let open_path = self.notifications[index].open_path.clone();
                let action = self.notifications[index]
                    .actions
                    .iter()
                    .find(|action| action.key == "default")
                    .or_else(|| self.notifications[index].actions.first())
                    .cloned();
                match action {
                    Some(action) if action.key == "hayato-open" => {
                        if let Some(path) = open_path {
                            open_in_files(&path);
                        }
                    }
                    Some(action) => {
                        let _ = self.event_tx.send(UiEvent::ActionInvoked(id, action.key));
                    }
                    None => {
                        if let Some(path) = open_path {
                            open_in_files(&path);
                        }
                    }
                }
                self.close_at(index);
                true
            }
        }
    }

    fn newest_index(&self) -> Option<usize> {
        self.notifications
            .iter()
            .enumerate()
            .filter(|(_, notification)| !notification.is_closing())
            .max_by_key(|(_, notification)| notification.created_at)
            .map(|(index, _)| index)
    }

    fn close_at(&mut self, index: usize) {
        let id = self.notifications[index].id;
        self.notifications.remove(index);
        let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
    }

    pub fn setup_layer_surface(&mut self, qh: &QueueHandle<Self>, w: u32, h: u32) {
        if self.surface.is_some() {
            return;
        }

        let Some(compositor) = self.compositor.as_ref() else {
            return;
        };
        let Some(layer_shell) = self.layer_shell.as_ref() else {
            return;
        };

        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "hayato-notify".to_string(),
            qh,
            (),
        );

        layer_surface
            .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right);
        // Waybar exclusive zone is 34px. Margin top 12px places the notification at y = 46px
        // (aligning exactly with the Sway terminal window top border at y = 46px), and margin right 12px
        // places it at right = 12px, aligning with the Sway window right border.
        layer_surface.set_margin(12, 12, 0, 0);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        layer_surface.set_size(w, h);

        surface.commit();

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
        self.width = w;
        self.height = h;
        self.configured = false;
    }

    pub fn destroy_surface(&mut self) {
        if let Some(ls) = self.layer_surface.take() {
            ls.destroy();
        }
        if let Some(s) = self.surface.take() {
            s.destroy();
        }
        self.width = 0;
        self.height = 0;
        self.configured = false;
    }

    pub fn paint_surface(&mut self, qh: &QueueHandle<Self>) {
        if !self.configured || self.notifications.is_empty() {
            return;
        }

        let Some(ref surface) = self.surface else {
            return;
        };
        let Some(ref shm) = self.shm else {
            return;
        };
        let Some(ref compositor) = self.compositor else {
            return;
        };

        let target_w = self.width;
        let target_h = self.height;
        if target_w == 0 || target_h == 0 {
            return;
        }

        // Allocate buffer and paint
        let stride = (target_w * 4) as usize;
        let size = stride * target_h as usize;

        let file = match tempfile::tempfile() {
            Ok(f) => f,
            Err(_) => return,
        };
        let _ = file.set_len(size as u64);

        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            target_w as i32,
            target_h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        let mut data = vec![0u8; size];
        if let Some(mut pixmap) = tiny_skia::PixmapMut::from_bytes(&mut data, target_w, target_h) {
            self.renderer.render(&mut pixmap, &mut self.notifications);
        }

        // tiny-skia outputs RGBA in memory. Wayland wl_shm::Format::Argb8888 on little-endian
        // is BGRA in memory. Swap Red (byte 0) and Blue (byte 2) so displayed colors match exactly.
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let _ = file.write_all_at(&data, 0);

        // Update input region around cards only
        let region = compositor.create_region(qh, ());
        for notif in &self.notifications {
            region.add(
                notif.card_bounds.x.round() as i32,
                notif.card_bounds.y.round() as i32,
                notif.card_bounds.w.round() as i32,
                notif.card_bounds.h.round() as i32,
            );
        }
        surface.set_input_region(Some(&region));

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, target_w as i32, target_h as i32);
        surface.commit();
    }

    pub fn handle_pointer_button(
        &mut self,
        button: u32,
        state: wl_pointer::ButtonState,
        qh: &QueueHandle<Self>,
    ) {
        if state != wl_pointer::ButtonState::Pressed {
            return;
        }

        let px = self.pointer_x as f32;
        let py = self.pointer_y as f32;

        // Find which card was clicked
        let mut clicked_index = None;
        let mut clicked_action = None;
        let mut clicked_close = false;

        for (i, notif) in self.notifications.iter().enumerate() {
            if notif.is_closing() {
                continue;
            }
            if notif.card_bounds.contains(px, py) {
                clicked_index = Some(i);

                // Check close button
                if let Some(ref close) = notif.close_rect {
                    if close.contains(px, py) {
                        clicked_close = true;
                        break;
                    }
                }

                // Check action buttons
                for (key, rect) in &notif.action_rects {
                    if rect.contains(px, py) {
                        clicked_action = Some(key.clone());
                        break;
                    }
                }
                break;
            }
        }

        let Some(idx) = clicked_index else {
            return;
        };

        let reduced = std::env::var("HAYATO_REDUCED_MOTION").is_ok();

        match button {
            0x110 => {
                // Left click
                if clicked_close {
                    let id = self.notifications[idx].id;
                    if reduced {
                        self.notifications.remove(idx);
                        let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
                        if self.notifications.is_empty() {
                            self.destroy_surface();
                        } else {
                            let metrics = self.renderer.compute_layout(&mut self.notifications);
                            self.width = metrics.total_width.ceil() as u32;
                            self.height = metrics.total_height.ceil() as u32;
                            self.paint_surface(qh);
                        }
                    } else {
                        self.notifications[idx].anim_state = AnimState::Closing {
                            start: Instant::now(),
                            reason: 2,
                        };
                        self.notifications[idx].is_hovered = false;
                        self.notifications[idx].hovered_action = None;
                        self.paint_surface(qh);
                    }
                } else if let Some(action_key) = clicked_action {
                    let id = self.notifications[idx].id;
                    if action_key == "hayato-open" {
                        if let Some(path) = self.notifications[idx].open_path.clone() {
                            open_in_files(&path);
                        }
                    } else {
                        let _ = self.event_tx.send(UiEvent::ActionInvoked(id, action_key));
                    }
                    let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
                    if reduced {
                        self.notifications.remove(idx);
                        if self.notifications.is_empty() {
                            self.destroy_surface();
                        } else {
                            let metrics = self.renderer.compute_layout(&mut self.notifications);
                            self.width = metrics.total_width.ceil() as u32;
                            self.height = metrics.total_height.ceil() as u32;
                            self.paint_surface(qh);
                        }
                    } else {
                        self.notifications[idx].anim_state = AnimState::Closing {
                            start: Instant::now(),
                            reason: 0,
                        };
                        self.notifications[idx].is_hovered = false;
                        self.notifications[idx].hovered_action = None;
                        self.paint_surface(qh);
                    }
                } else {
                    // Click on card body: show the file in Files when the card
                    // names one, otherwise invoke "default" if present, or
                    // dismiss.
                    let id = self.notifications[idx].id;
                    if let Some(path) = self.notifications[idx].open_path.clone() {
                        open_in_files(&path);
                    } else if self.notifications[idx]
                        .actions
                        .iter()
                        .any(|a| a.key == "default")
                    {
                        let _ = self
                            .event_tx
                            .send(UiEvent::ActionInvoked(id, "default".to_string()));
                    }
                    if reduced {
                        self.notifications.remove(idx);
                        let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
                        if self.notifications.is_empty() {
                            self.destroy_surface();
                        } else {
                            let metrics = self.renderer.compute_layout(&mut self.notifications);
                            self.width = metrics.total_width.ceil() as u32;
                            self.height = metrics.total_height.ceil() as u32;
                            self.paint_surface(qh);
                        }
                    } else {
                        self.notifications[idx].anim_state = AnimState::Closing {
                            start: Instant::now(),
                            reason: 2,
                        };
                        self.notifications[idx].is_hovered = false;
                        self.notifications[idx].hovered_action = None;
                        self.paint_surface(qh);
                    }
                }
            }
            0x111 => {
                // Right click: dismiss card
                let id = self.notifications[idx].id;
                if reduced {
                    self.notifications.remove(idx);
                    let _ = self.event_tx.send(UiEvent::NotificationClosed(id, 2));
                    if self.notifications.is_empty() {
                        self.destroy_surface();
                    } else {
                        let metrics = self.renderer.compute_layout(&mut self.notifications);
                        self.width = metrics.total_width.ceil() as u32;
                        self.height = metrics.total_height.ceil() as u32;
                        self.paint_surface(qh);
                    }
                } else {
                    self.notifications[idx].anim_state = AnimState::Closing {
                        start: Instant::now(),
                        reason: 2,
                    };
                    self.notifications[idx].is_hovered = false;
                    self.notifications[idx].hovered_action = None;
                    self.paint_surface(qh);
                }
            }
            0x112 => {
                // Middle click: dismiss all from this app
                let app = self.notifications[idx].app_name.clone();
                if reduced {
                    let mut remaining = Vec::new();
                    for notif in self.notifications.drain(..) {
                        if notif.app_name == app {
                            let _ = self.event_tx.send(UiEvent::NotificationClosed(notif.id, 2));
                        } else {
                            remaining.push(notif);
                        }
                    }
                    self.notifications = remaining;
                    if self.notifications.is_empty() {
                        self.destroy_surface();
                    } else {
                        let metrics = self.renderer.compute_layout(&mut self.notifications);
                        self.width = metrics.total_width.ceil() as u32;
                        self.height = metrics.total_height.ceil() as u32;
                        self.paint_surface(qh);
                    }
                } else {
                    for notif in &mut self.notifications {
                        if notif.app_name == app && !notif.is_closing() {
                            notif.anim_state = AnimState::Closing {
                                start: Instant::now(),
                                reason: 2,
                            };
                            notif.is_hovered = false;
                            notif.hovered_action = None;
                        }
                    }
                    self.paint_surface(qh);
                }
            }
            _ => {}
        }
    }

    pub fn handle_pointer_motion(&mut self, x: f64, y: f64, qh: &QueueHandle<Self>) {
        self.pointer_x = x;
        self.pointer_y = y;

        let px = x as f32;
        let py = y as f32;
        let mut needs_repaint = false;

        for notif in &mut self.notifications {
            if notif.is_closing() {
                if notif.is_hovered || notif.hovered_action.is_some() {
                    notif.is_hovered = false;
                    notif.hovered_action = None;
                    needs_repaint = true;
                }
                continue;
            }

            let was_hovered = notif.is_hovered;
            let is_hovered = notif.card_bounds.contains(px, py);
            if was_hovered != is_hovered {
                notif.is_hovered = is_hovered;
                needs_repaint = true;
            }

            let old_action = notif.hovered_action.clone();
            let mut new_action = None;
            for (key, rect) in &notif.action_rects {
                if rect.contains(px, py) {
                    new_action = Some(key.clone());
                    break;
                }
            }
            if old_action != new_action {
                notif.hovered_action = new_action;
                needs_repaint = true;
            }
        }

        if needs_repaint {
            self.paint_surface(qh);
        }
    }
}

// --- Wayland Dispatch Implementations ---

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, 4.min(version), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, 4.min(version), qh, ()));
                }
                "wl_seat" => {
                    let seat: wl_seat::WlSeat = registry.bind(name, 7.min(version), qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for WaylandState {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_region::WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let has_pointer = match capabilities {
                wayland_client::WEnum::Value(caps) => caps.contains(wl_seat::Capability::Pointer),
                _ => false,
            };
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.handle_pointer_motion(surface_x, surface_y, qh);
            }
            wl_pointer::Event::Button {
                button,
                state: wayland_client::WEnum::Value(val),
                ..
            } => {
                state.handle_pointer_button(button, val, qh);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer_surface.ack_configure(serial);
                state.configured = true;
                state.paint_surface(qh);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.configured = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_opens_its_folder_and_a_folder_opens_itself() {
        let dir = std::env::temp_dir().join(format!("hayato-files-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(files_dir(&dir), Some(dir.clone()));
        assert_eq!(files_dir(&dir.join("a.png")), Some(dir.clone()));
        assert_eq!(files_dir(Path::new("a.png")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
