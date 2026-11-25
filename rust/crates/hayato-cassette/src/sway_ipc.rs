//! Direct `$SWAYSOCK` binary `i3-ipc` client for Hayato Cassette:
//! zero-fork scratchpad toggle (`hayato-cassette --toggle`), mini-mode resize,
//! and window focus/visibility queries.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

pub const APP_ID: &str = "hayato-cassette";

const MAGIC: &[u8; 6] = b"i3-ipc";
const MSG_RUN_COMMAND: u32 = 0;
const MSG_GET_TREE: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Absent,
    Hidden,
    Visible,
}

fn send_ipc(msg_type: u32, payload: &str) -> Option<String> {
    let sock = std::env::var("SWAYSOCK").ok()?;
    let mut stream = UnixStream::connect(sock).ok()?;
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

fn find_in_tree(node: &Value, in_scratchpad: bool) -> Option<bool> {
    let name = node["name"].as_str().unwrap_or("");
    let scratch = in_scratchpad || name == "__i3_scratch";
    if node["app_id"].as_str() == Some(APP_ID) {
        return Some(scratch);
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(arr) = node[key].as_array() {
            for child in arr {
                if let Some(found) = find_in_tree(child, scratch) {
                    return Some(found);
                }
            }
        }
    }
    None
}

pub fn window_state() -> WindowState {
    let Some(raw) = send_ipc(MSG_GET_TREE, "") else {
        return WindowState::Absent;
    };
    let Ok(tree) = serde_json::from_str::<Value>(&raw) else {
        return WindowState::Absent;
    };
    match find_in_tree(&tree, false) {
        None => WindowState::Absent,
        Some(true) => WindowState::Hidden,
        Some(false) => WindowState::Visible,
    }
}

pub fn hide_to_scratchpad() -> bool {
    run_command(&format!("[app_id=\"{APP_ID}\"] move scratchpad"))
}

pub fn show_from_scratchpad(mini: bool) -> bool {
    let (w, h) = if mini { (560, 300) } else { (1120, 680) };
    run_command(&format!(
        "[app_id=\"{APP_ID}\"] move scratchpad; [app_id=\"{APP_ID}\"] scratchpad show, resize set {w} {h}, move position center"
    ))
}

pub fn resize_window(mini: bool) -> bool {
    let (w, h) = if mini { (560, 300) } else { (1120, 680) };
    run_command(&format!(
        "[app_id=\"{APP_ID}\"] floating enable, resize set {w} {h}, move position center"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scratchpad_vs_visible_tree() {
        let hidden = serde_json::json!({
            "name": "root",
            "nodes": [{
                "name": "__i3_scratch",
                "floating_nodes": [{ "app_id": "hayato-cassette" }]
            }]
        });
        assert_eq!(find_in_tree(&hidden, false), Some(true));

        let visible = serde_json::json!({
            "name": "root",
            "nodes": [{
                "name": "1",
                "floating_nodes": [{ "app_id": "hayato-cassette" }]
            }]
        });
        assert_eq!(find_in_tree(&visible, false), Some(false));
    }
}
