//! Direct `$SWAYSOCK` binary IPC client (`i3-ipc`) for smooth 60 FPS animated
//! webcam PiP corner gliding, scratchpad hide/show, sticky pinning, and
//! fullscreen window detection.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use hayato_ui::recording::{PipCorner, PipSize};

const MAGIC: &[u8; 6] = b"i3-ipc";
const MSG_RUN_COMMAND: u32 = 0;
const MSG_GET_WORKSPACES: u32 = 1;
const MSG_GET_OUTPUTS: u32 = 3;
const MSG_GET_TREE: u32 = 4;

const MARGIN_SIDE: i64 = 20;
const MARGIN_TOP: i64 = 54; // Clears 34px Waybar + top margin + breathing room
const MARGIN_BOTTOM: i64 = 20;

static GLIDE_GEN: AtomicU64 = AtomicU64::new(0);

fn send_ipc(msg_type: u32, payload: &str) -> Option<String> {
    let sock_path = std::env::var("SWAYSOCK").ok()?;
    let mut stream = UnixStream::connect(sock_path).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

    let bytes = payload.as_bytes();
    let mut frame = Vec::with_capacity(14 + bytes.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&(bytes.len() as u32).to_ne_bytes());
    frame.extend_from_slice(&msg_type.to_ne_bytes());
    frame.extend_from_slice(bytes);
    stream.write_all(&frame).ok()?;

    let mut hdr = [0u8; 14];
    stream.read_exact(&mut hdr).ok()?;
    if &hdr[..6] != MAGIC {
        return None;
    }
    let len = u32::from_ne_bytes(hdr[6..10].try_into().ok()?) as usize;
    if len > 8 * 1024 * 1024 {
        return None;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

pub fn run_command(cmd: &str) -> bool {
    send_ipc(MSG_RUN_COMMAND, cmd).is_some()
}

pub fn active_output_rect() -> (i64, i64, i64, i64) {
    if let Some(raw) = send_ipc(MSG_GET_OUTPUTS, "")
        && let Ok(outputs) = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        && let Some(out) = outputs
            .iter()
            .find(|o| o["focused"].as_bool().unwrap_or(false))
            .or_else(|| outputs.first())
    {
        let r = &out["rect"];
        return (
            r["x"].as_i64().unwrap_or(0),
            r["y"].as_i64().unwrap_or(0),
            r["width"].as_i64().unwrap_or(1366),
            r["height"].as_i64().unwrap_or(768),
        );
    }
    (0, 0, 1366, 768)
}

pub fn workspace_origin() -> (i64, i64) {
    if let Some(raw) = send_ipc(MSG_GET_WORKSPACES, "")
        && let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        && let Some(ws) = list
            .iter()
            .find(|w| w["focused"].as_bool().unwrap_or(false))
    {
        return (
            ws["rect"]["x"].as_i64().unwrap_or(0),
            ws["rect"]["y"].as_i64().unwrap_or(0),
        );
    }
    (0, 0)
}

/// Compute target `(x, y, w, h)` in workspace coordinates for a PiP corner + size.
pub fn target_pip_rect(
    bounds: (i64, i64, i64, i64),
    ws_origin: (i64, i64),
    corner: PipCorner,
    size: PipSize,
) -> (i64, i64, i64, i64) {
    let (bx, by, bw, bh) = bounds;
    let (wx, wy) = ws_origin;
    let (w, h) = size.dims();
    let left_x = (bx + MARGIN_SIDE - wx).max(8);
    let right_x = (bx + bw - w - MARGIN_SIDE - wx).max(8);
    let center_x = (bx + (bw - w) / 2 - wx).max(8);
    let top_y = (by + MARGIN_TOP - wy).max(8);
    let bottom_y = (by + bh - h - MARGIN_BOTTOM - wy).max(8);
    let center_y = (by + (bh - h) / 2 - wy).max(8);

    let (x, y) = match corner {
        PipCorner::TopLeft => (left_x, top_y),
        PipCorner::TopRight => (right_x, top_y),
        PipCorner::BottomLeft => (left_x, bottom_y),
        PipCorner::BottomRight => (right_x, bottom_y),
        PipCorner::Center => (center_x, center_y),
    };
    (x, y, w, h)
}

fn find_node_rect(node: &serde_json::Value, app_id: &str) -> Option<(i64, i64, i64, i64)> {
    if node["app_id"].as_str() == Some(app_id) {
        let r = &node["rect"];
        return Some((
            r["x"].as_i64().unwrap_or(0),
            r["y"].as_i64().unwrap_or(0),
            r["width"].as_i64().unwrap_or(320),
            r["height"].as_i64().unwrap_or(240),
        ));
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(arr) = node[key].as_array() {
            for child in arr {
                if let Some(found) = find_node_rect(child, app_id) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn has_fullscreen_node(node: &serde_json::Value) -> bool {
    if node["fullscreen_mode"].as_i64().unwrap_or(0) != 0 {
        return true;
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(arr) = node[key].as_array() {
            for child in arr {
                if has_fullscreen_node(child) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn webcam_rect() -> Option<(i64, i64, i64, i64)> {
    let raw = send_ipc(MSG_GET_TREE, "")?;
    let tree: serde_json::Value = serde_json::from_str(&raw).ok()?;
    find_node_rect(&tree, "hayato-webcam")
}

pub fn webcam_exists() -> bool {
    webcam_rect().is_some()
}

pub fn has_active_fullscreen() -> bool {
    let Some(raw) = send_ipc(MSG_GET_TREE, "") else {
        return false;
    };
    let Ok(tree) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    has_fullscreen_node(&tree)
}

pub fn unfullscreen_active() {
    let _ = run_command("[fullscreen_mode=1] fullscreen disable");
}

pub fn set_pip_sticky(sticky: bool) {
    let arg = if sticky { "enable" } else { "disable" };
    let _ = run_command(&format!("[app_id=\"hayato-webcam\"] sticky {arg}"));
}

pub fn set_pip_visible(visible: bool, corner: PipCorner, size: PipSize, sticky: bool) {
    if !visible {
        let _ = run_command("[app_id=\"hayato-webcam\"] move scratchpad");
    } else {
        let bounds = active_output_rect();
        let ws = workspace_origin();
        let (x, y, w, h) = target_pip_rect(bounds, ws, corner, size);
        let st = if sticky { "enable" } else { "disable" };
        let _ = run_command(&format!(
            "[app_id=\"hayato-webcam\"] scratchpad show, floating enable, sticky {st}, border none, resize set {w} {h}, move position {x} {y}"
        ));
    }
}

/// Cubic ease-out interpolation factor in `[0.0, 1.0]`.
pub fn ease_out_cubic(t: f64) -> f64 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Smoothly glide the `[app_id="hayato-webcam"]` floating window to `(corner, size)`.
/// Snaps immediately when `reduced_motion` is true; otherwise glides over 14 frames (210ms)
/// on a background thread guarded by `GLIDE_GEN`.
pub fn glide_pip(corner: PipCorner, size: PipSize, sticky: bool, reduced_motion: bool) {
    let bounds = active_output_rect();
    let ws = workspace_origin();
    let (x1, y1, w1, h1) = target_pip_rect(bounds, ws, corner, size);
    let st = if sticky { "enable" } else { "disable" };

    if reduced_motion {
        let _ = run_command(&format!(
            "[app_id=\"hayato-webcam\"] floating enable, sticky {st}, border none, resize set {w1} {h1}, move position {x1} {y1}"
        ));
        return;
    }

    let start = webcam_rect()
        .map(|(rx, ry, rw, rh)| (rx - ws.0, ry - ws.1, rw, rh))
        .unwrap_or((x1, y1, w1, h1));
    let (x0, y0, w0, h0) = start;
    if (x0 - x1).abs() <= 2 && (y0 - y1).abs() <= 2 && (w0 - w1).abs() <= 2 && (h0 - h1).abs() <= 2
    {
        let _ = run_command(&format!(
            "[app_id=\"hayato-webcam\"] floating enable, sticky {st}, border none, resize set {w1} {h1}, move position {x1} {y1}"
        ));
        return;
    }

    let my_gen = GLIDE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let st_owned = Arc::new(st.to_string());
    std::thread::spawn(move || {
        const FRAMES: u32 = 14;
        for step in 1..=FRAMES {
            if GLIDE_GEN.load(Ordering::SeqCst) != my_gen {
                return;
            }
            let e = ease_out_cubic(f64::from(step) / f64::from(FRAMES));
            let ix = (x0 as f64 + (x1 - x0) as f64 * e).round() as i64;
            let iy = (y0 as f64 + (y1 - y0) as f64 * e).round() as i64;
            let iw = (w0 as f64 + (w1 - w0) as f64 * e).round() as i64;
            let ih = (h0 as f64 + (h1 - h0) as f64 * e).round() as i64;
            let cmd = if step == FRAMES {
                format!(
                    "[app_id=\"hayato-webcam\"] floating enable, sticky {}, border none, resize set {w1} {h1}, move position {x1} {y1}",
                    st_owned
                )
            } else if w0 != w1 || h0 != h1 {
                format!(
                    "[app_id=\"hayato-webcam\"] resize set {iw} {ih}, move position {ix} {iy}"
                )
            } else {
                format!("[app_id=\"hayato-webcam\"] move position {ix} {iy}")
            };
            let _ = run_command(&cmd);
            if step < FRAMES {
                std::thread::sleep(Duration::from_millis(15));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_bounds_and_monotonic() {
        assert!((ease_out_cubic(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-6);
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn target_pip_rect_clears_waybar_top_margin() {
        let (x, y, w, h) = target_pip_rect((0, 0, 1366, 912), (0, 0), PipCorner::TopLeft, PipSize::Medium);
        assert_eq!((x, y, w, h), (20, 54, 320, 240));
        let (rx, ry, _, _) = target_pip_rect((0, 0, 1366, 912), (0, 0), PipCorner::BottomRight, PipSize::Medium);
        assert_eq!((rx, ry), (1366 - 320 - 20, 912 - 240 - 20));
    }
}
