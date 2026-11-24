//! Ephemeral PipeWire virtual mixer (`hayato_rec_mix`) for live audio switching
//! and mixing (`Off` / `Mic` / `System` / `Both`) without restarting `wf-recorder`
//! or `ffmpeg`.
//!
//! Creates a single ephemeral null-sink (`hayato_rec_mix`, with
//! `node.dont-fallback=true` so WirePlumber never makes it the default speaker)
//! and dynamically loads/unloads `module-loopback` modules for the microphone
//! and system-audio monitor. Tears down all module IDs cleanly on Stop.

use std::path::Path;
use std::process::{Command, Stdio};

use hayato_ui::recording::AudioMode;

use crate::system::out;

pub const MIX_SINK: &str = "hayato_rec_mix";
pub const MIX_MONITOR: &str = "hayato_rec_mix.monitor";

fn read_mod_id(state_dir: &Path, filename: &str) -> Option<u32> {
    std::fs::read_to_string(state_dir.join(filename))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn write_mod_id(state_dir: &Path, filename: &str, id: u32) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(state_dir.join(filename), format!("{id}\n"));
}

fn unload_mod_file(state_dir: &Path, filename: &str) {
    if let Some(id) = read_mod_id(state_dir, filename) {
        let _ = Command::new("pactl")
            .args(["unload-module", &id.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = std::fs::remove_file(state_dir.join(filename));
}

/// Remove any orphaned `hayato_rec_mix` modules left over from an earlier crash.
pub fn cleanup_orphans() {
    let Some(list) = out(&["pactl", "list", "short", "modules"]) else {
        return;
    };
    for line in list.lines() {
        if line.contains(MIX_SINK)
            && let Some(id) = line.split_whitespace().next()
        {
            let _ = Command::new("pactl")
                .args(["unload-module", id])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Resolve the real microphone source (never `hayato_rec_mix.monitor`).
pub fn resolve_mic_source() -> Option<String> {
    if let Some(src) = out(&["pactl", "get-default-source"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains(MIX_SINK) && !s.ends_with(".monitor"))
    {
        return Some(src);
    }
    out(&["pactl", "list", "short", "sources"]).and_then(|text| {
        text.lines().find_map(|line| {
            let name = line.split_whitespace().nth(1)?;
            (!name.ends_with(".monitor") && !name.contains(MIX_SINK)).then(|| name.to_string())
        })
    })
}

/// Resolve the real system-audio monitor source (never `hayato_rec_mix.monitor`,
/// preventing any feedback loop!).
pub fn resolve_system_monitor() -> Option<String> {
    if let Some(sink) = out(&["pactl", "get-default-sink"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != MIX_SINK)
    {
        return Some(format!("{sink}.monitor"));
    }
    out(&["pactl", "list", "short", "sinks"]).and_then(|text| {
        text.lines().find_map(|line| {
            let name = line.split_whitespace().nth(1)?;
            (name != MIX_SINK).then(|| format!("{name}.monitor"))
        })
    })
}

/// Create the ephemeral `hayato_rec_mix` null sink and apply the initial audio mode.
pub fn start_mixer(state_dir: &Path, initial: AudioMode) -> Result<(), String> {
    cleanup_orphans();
    let _ = std::fs::create_dir_all(state_dir);
    let output = Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            &format!("sink_name={MIX_SINK}"),
            "sink_properties=device.description=Hayato_Rec_Mix node.dont-fallback=true",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("pactl load-module failed: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("could not create PipeWire mixer sink: {err}"));
    }
    let id_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Ok(id) = id_str.parse::<u32>() {
        write_mod_id(state_dir, "pw_null.mod", id);
    }
    apply_audio_mode(state_dir, initial);
    Ok(())
}

fn load_loopback(state_dir: &Path, mod_file: &str, source: &str) {
    if read_mod_id(state_dir, mod_file).is_some() {
        return;
    }
    let out = Command::new("pactl")
        .args([
            "load-module",
            "module-loopback",
            &format!("source={source}"),
            &format!("sink={MIX_SINK}"),
            "latency_msec=20",
            "adjust_time=1",
            "sink_dont_move=true",
        ])
        .stdin(Stdio::null())
        .output();
    if let Ok(o) = out
        && o.status.success()
        && let Ok(id) = String::from_utf8_lossy(&o.stdout).trim().parse::<u32>()
    {
        write_mod_id(state_dir, mod_file, id);
    }
}

/// Live-switch the active PipeWire loopback routing into `hayato_rec_mix`.
pub fn apply_audio_mode(state_dir: &Path, mode: AudioMode) {
    let want_mic = matches!(mode, AudioMode::Mic | AudioMode::Both);
    let want_sys = matches!(mode, AudioMode::System | AudioMode::Both);

    if want_mic {
        if let Some(src) = resolve_mic_source() {
            load_loopback(state_dir, "pw_mic.mod", &src);
        }
    } else {
        unload_mod_file(state_dir, "pw_mic.mod");
    }

    if want_sys {
        if let Some(mon) = resolve_system_monitor() {
            load_loopback(state_dir, "pw_sys.mod", &mon);
        }
    } else {
        unload_mod_file(state_dir, "pw_sys.mod");
    }
}

/// Tear down all loopbacks and the ephemeral null sink in reverse order.
pub fn teardown_mixer(state_dir: &Path) {
    unload_mod_file(state_dir, "pw_mic.mod");
    unload_mod_file(state_dir, "pw_sys.mod");
    unload_mod_file(state_dir, "pw_null.mod");
    cleanup_orphans();
}
