//! Voice Dictation (`dictate`, `lazyos menu dictate`).
//!
//! By the nature of voice-to-text, the transcribed output goes directly into the
//! user's currently focused window via `wtype` (and onto the clipboard via `wl-copy`
//! as a backup). Therefore, dictation must never steal Wayland keyboard focus with
//! a floating terminal window while recording or transcribing.
//!
//! Instead, LazyOS coordinates three non-focus-stealing session surfaces:
//! 1. **Push-to-Talk & Toggle Shortcut (`Alt+Ctrl+X`)**:
//!    - Holding `Alt+Ctrl+X` (`dictate --press` / `dictate --release` after >= 350 ms)
//!      records while held and transcribes immediately upon key release.
//!    - A quick tap (< 350 ms) or starting from the Root Menu enters hands-free
//!      recording mode until stopped.
//! 2. **Interactive Layer-Shell Notification (`lazyos-notify`)**:
//!    - Displays a persistent (`-t 0`), non-focus-stealing card (`sync_tag`:
//!      `lazyos-dictate`) with clickable **`[ Stop & Transcribe ]`** and
//!      **`[ Cancel ]`** action buttons, then morphs in-place into
//!      `Dictation · Transcribing…` and finally `Dictation · Typed`.
//! 3. **Waybar `custom/busy` Spinner & Root Menu (`Alt+Space`)**:
//!    - Registers `dictate` in `$XDG_RUNTIME_DIR/lazyos-busy` (`RTMIN+8`) so the
//!      top bar shows `⠋ Dictation · Recording` (click to stop & transcribe), and
//!      the Root Menu dynamically switches its row to `Stop dictation`.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::have;

const HOLD_THRESHOLD_MS: u128 = 350;
const SYNC_HINT: &str = "string:x-canonical-private-synchronous:lazyos-dictate";
const MIC_ICON: &str = "audio-input-microphone";
const ERR_ICON: &str = "dialog-error";

fn runtime_dir() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) => PathBuf::from(dir).join("dictate"),
        Err(_) => PathBuf::from("/tmp/dictate"),
    }
}

fn busy_dir() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) => PathBuf::from(dir).join("lazyos-busy"),
        Err(_) => PathBuf::from("/tmp/lazyos-busy"),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn model_path() -> PathBuf {
    if let Ok(m) = std::env::var("DICTATE_MODEL") {
        return PathBuf::from(m);
    }
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".local/share")
        });
    data_dir.join("whisper/ggml-tiny.en.bin")
}

/// Check whether `parecord` is actively recording a dictation session.
pub fn is_recording() -> bool {
    let dir = runtime_dir();
    let state_file = dir.join("pid");
    let Ok(pid_str) = fs::read_to_string(&state_file) else {
        return false;
    };
    let Ok(pid) = pid_str.trim().parse::<i32>() else {
        let _ = fs::remove_file(&state_file);
        return false;
    };
    let alive = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !alive {
        let _ = fs::remove_file(&state_file);
    }
    alive
}

fn set_bar_busy(label: &str, stop_cmd: Option<&str>) {
    let bdir = busy_dir();
    let _ = fs::create_dir_all(&bdir);
    let _ = fs::write(bdir.join("dictate.label"), format!("{label}\n"));
    let _ = fs::write(bdir.join("dictate.icon"), format!("{MIC_ICON}\n"));
    let stop_path = bdir.join("dictate.stop");
    if let Some(cmd) = stop_cmd {
        let _ = fs::write(&stop_path, format!("{cmd}\n"));
    } else {
        let _ = fs::remove_file(&stop_path);
    }
    let _ = Command::new("pkill")
        .args(["-RTMIN+8", "waybar"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn clear_bar_busy() {
    let bdir = busy_dir();
    for ext in ["id", "label", "icon", "stop"] {
        let _ = fs::remove_file(bdir.join(format!("dictate.{ext}")));
    }
    let _ = Command::new("pkill")
        .args(["-RTMIN+8", "waybar"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn send_sync_card(summary: &str, body: &str, urgency: &str, timeout_ms: i32, icon: &str) {
    let _ = Command::new("notify-send")
        .args([
            "-a",
            "LazyOS",
            "-i",
            icon,
            "-u",
            urgency,
            "-t",
            &timeout_ms.to_string(),
            "-h",
            SYNC_HINT,
            summary,
            body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn spawn_notify_watcher(hands_free: bool) {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/usr/bin/lazyos".to_string());
    let mode_arg = if hands_free { "hands-free" } else { "hold" };
    let cmd = format!("exec '{exe}' menu dictate --watch-notify {mode_arg}");
    crate::system::spawn_detached(&cmd);
}

fn watch_notify(hands_free: bool) -> i32 {
    let body = if hands_free {
        "Listening hands-free \u{00b7} click Stop or press Alt+Ctrl+X to transcribe"
    } else {
        "Hold Alt+Ctrl+X to speak \u{00b7} release or click Stop to transcribe"
    };
    let out = Command::new("notify-send")
        .args([
            "-a",
            "LazyOS",
            "-i",
            MIC_ICON,
            "-u",
            "normal",
            "-t",
            "0",
            "-h",
            SYNC_HINT,
            "--action=stop=Stop & Transcribe",
            "--action=cancel=Cancel",
            "Dictation \u{00b7} Recording",
            body,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();

    if let Ok(res) = out {
        let action = String::from_utf8_lossy(&res.stdout).trim().to_string();
        match action.as_str() {
            "stop" => return stop_and_transcribe(),
            "cancel" => return cancel_recording(),
            _ => {}
        }
    }
    0
}

/// Start recording voice audio with `parecord`.
pub fn start_recording(hands_free: bool) -> i32 {
    let dir = runtime_dir();
    let _ = fs::create_dir_all(&dir);
    let state_file = dir.join("pid");
    let started_file = dir.join("started_ms");
    let wav_file = dir.join("rec.wav");

    let model = model_path();
    if !model.exists() {
        send_sync_card(
            "Dictation",
            "Fetching speech model (78 MB, once)\u{2026}",
            "normal",
            4000,
            MIC_ICON,
        );
        crate::system::spawn_detached("exec foot -T 'Dictation \u{00b7} fetching model' -e dictate-model");
        return 0;
    }

    if !have("parecord") {
        send_sync_card(
            "Dictation",
            "parecord is missing (package pulseaudio-utils)",
            "critical",
            6000,
            ERR_ICON,
        );
        return 1;
    }

    let _ = Command::new("playerctl")
        .args(["-a", "pause"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_file(&wav_file);

    use std::os::unix::process::CommandExt;
    let mut command = Command::new("parecord");
    command
        .args([
            "--rate=16000",
            "--channels=1",
            "--format=s16le",
            "--file-format=wav",
            wav_file.to_str().unwrap_or(""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    match command.spawn() {
        Ok(c) => {
            let _ = fs::write(&state_file, c.id().to_string());
            let _ = fs::write(&started_file, now_ms().to_string());
            set_bar_busy(
                "Dictation \u{00b7} Recording",
                Some("/usr/bin/lazyos menu dictate --stop"),
            );
            spawn_notify_watcher(hands_free);
            0
        }
        Err(e) => {
            send_sync_card(
                "Dictation",
                &format!("Could not start recorder: {e}"),
                "critical",
                6000,
                ERR_ICON,
            );
            1
        }
    }
}

/// Cancel an active recording without transcribing.
pub fn cancel_recording() -> i32 {
    let dir = runtime_dir();
    let state_file = dir.join("pid");
    let started_file = dir.join("started_ms");
    let wav_file = dir.join("rec.wav");
    let norm_file = dir.join("norm.wav");

    if let Ok(pid_str) = fs::read_to_string(&state_file) {
        let _ = fs::remove_file(&state_file);
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            let _ = Command::new("kill")
                .args(["-INT", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _ = fs::remove_file(&started_file);
    let _ = fs::remove_file(&wav_file);
    let _ = fs::remove_file(&norm_file);
    clear_bar_busy();
    send_sync_card(
        "Dictation",
        "Recording cancelled",
        "low",
        2500,
        MIC_ICON,
    );
    0
}

/// Stop recording, transcribe on the 4 Cortex-A76 performance cores, copy to
/// clipboard, and type into the focused window.
pub fn stop_and_transcribe() -> i32 {
    let dir = runtime_dir();
    let state_file = dir.join("pid");
    let started_file = dir.join("started_ms");
    let wav_file = dir.join("rec.wav");
    let norm_file = dir.join("norm.wav");

    let Ok(pid_str) = fs::read_to_string(&state_file) else {
        return 0;
    };
    // Remove `pid` immediately so concurrent stop triggers (e.g., key release + click)
    // do not double-transcribe the same audio.
    let _ = fs::remove_file(&state_file);
    let _ = fs::remove_file(&started_file);

    if let Ok(pid) = pid_str.trim().parse::<i32>() {
        let _ = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(200));
    }

    if !wav_file.exists() || fs::metadata(&wav_file).map(|m| m.len()).unwrap_or(0) <= 44 {
        let _ = fs::remove_file(&wav_file);
        clear_bar_busy();
        send_sync_card(
            "Dictation",
            "Nothing was recorded",
            "critical",
            4000,
            ERR_ICON,
        );
        return 1;
    }

    set_bar_busy("Dictation \u{00b7} Transcribing\u{2026}", None);
    send_sync_card(
        "Dictation \u{00b7} Transcribing\u{2026}",
        "Processing speech with Whisper\u{2026}",
        "normal",
        0,
        MIC_ICON,
    );

    // Optional normalize with ffmpeg
    let mut audio_target = wav_file.clone();
    if have("ffmpeg") {
        let norm_res = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-i",
                wav_file.to_str().unwrap_or(""),
                "-af",
                "dynaudnorm=f=150:g=15",
                "-ar",
                "16000",
                "-ac",
                "1",
                norm_file.to_str().unwrap_or(""),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if norm_res.map(|s| s.success()).unwrap_or(false) && norm_file.exists() {
            audio_target = norm_file.clone();
        }
    }

    let model = model_path();
    let whisper_args = [
        "-t",
        "4",
        "-m",
        model.to_str().unwrap_or(""),
        "-f",
        audio_target.to_str().unwrap_or(""),
        "-l",
        "en",
        "-nt",
        "-np",
    ];
    let out = if have("taskset") {
        Command::new("taskset")
            .args(["-c", "4-7", "whisper-cli"])
            .args(whisper_args)
            .stdin(Stdio::null())
            .output()
    } else {
        Command::new("whisper-cli")
            .args(whisper_args)
            .stdin(Stdio::null())
            .output()
    };

    let _ = fs::remove_file(&wav_file);
    let _ = fs::remove_file(&norm_file);
    clear_bar_busy();

    let text = match out {
        Ok(res) if res.status.success() => {
            let raw = String::from_utf8_lossy(&res.stdout);
            raw.lines()
                .map(|l| l.trim())
                .filter(|l| {
                    !l.is_empty()
                        && !l.starts_with('[')
                        && !l.eq_ignore_ascii_case("(blank audio)")
                })
                .collect::<Vec<&str>>()
                .join(" ")
        }
        _ => String::new(),
    };

    if text.trim().is_empty() {
        send_sync_card(
            "Dictation",
            "No speech was recognised",
            "normal",
            3500,
            MIC_ICON,
        );
        return 1;
    }

    // Copy to clipboard
    let _ = Command::new("wl-copy")
        .arg(&text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let preview: String = text.chars().take(72).collect();
    let preview = if text.chars().count() > 72 {
        format!("{preview}\u{2026}")
    } else {
        preview
    };

    // Type into focused window
    if have("wtype") {
        let typed = Command::new("wtype")
            .arg(&text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if typed {
            send_sync_card(
                "Dictation \u{00b7} Typed",
                &format!("\u{201c}{preview}\u{201d}"),
                "normal",
                4000,
                MIC_ICON,
            );
        } else {
            send_sync_card(
                "Dictation \u{00b7} Copied to clipboard",
                &format!("Could not type into window \u{00b7} \u{201c}{preview}\u{201d}"),
                "normal",
                4500,
                MIC_ICON,
            );
        }
    } else {
        send_sync_card(
            "Dictation \u{00b7} Copied to clipboard",
            &format!("\u{201c}{preview}\u{201d}"),
            "normal",
            4500,
            MIC_ICON,
        );
    }
    0
}

/// Handle key-press of `Alt+Ctrl+X` (`dictate --press`).
/// Starts recording if idle; stops & transcribes if already recording in hands-free mode.
pub fn handle_press() -> i32 {
    if is_recording() {
        stop_and_transcribe()
    } else {
        start_recording(false)
    }
}

/// Handle key-release of `Alt+Ctrl+X` (`dictate --release`).
/// If held for >= 350 ms (push-to-talk), stops and transcribes immediately.
/// If tapped briefly (< 350 ms), keeps recording in hands-free mode and updates the card.
pub fn handle_release() -> i32 {
    if !is_recording() {
        return 0;
    }
    let started_file = runtime_dir().join("started_ms");
    let started = fs::read_to_string(&started_file)
        .ok()
        .and_then(|s| s.trim().parse::<u128>().ok())
        .unwrap_or(0);
    let elapsed = now_ms().saturating_sub(started);
    if started == 0 || elapsed >= HOLD_THRESHOLD_MS {
        stop_and_transcribe()
    } else {
        // Quick tap: stay in hands-free recording mode and update notification prompt.
        spawn_notify_watcher(true);
        0
    }
}

/// Toggle dictation recording / transcription (used by Root Menu and `dictate` without args).
pub fn toggle() -> i32 {
    if is_recording() {
        // When triggered from the Root Menu, wait briefly so Sway restores keyboard
        // focus to the user's window before `wtype` types the transcript.
        std::thread::sleep(Duration::from_millis(120));
        stop_and_transcribe()
    } else {
        start_recording(true)
    }
}

/// Dispatch CLI flags for `dictate` / `lazyos menu dictate`.
pub fn run(args: &[String]) -> i32 {
    let first = args.first().map(|s| s.as_str()).unwrap_or("");
    match first {
        "--press" | "press" | "--start" | "start" => handle_press(),
        "--release" | "release" => handle_release(),
        "--stop" | "stop" => stop_and_transcribe(),
        "--cancel" | "cancel" => cancel_recording(),
        "--watch-notify" => {
            let hands_free = args.get(1).map(|s| s == "hands-free").unwrap_or(false);
            watch_notify(hands_free)
        }
        "--status" | "status" => {
            let rec = is_recording();
            println!("{{\"recording\":{rec}}}");
            0
        }
        _ => toggle(),
    }
}

/// Entrypoint for `lazyos menu dictate`.
pub fn menu() -> Result<i32, crate::Failure> {
    Ok(toggle())
}

/// Entrypoint for `lazyos menu dictate` with arguments.
pub fn menu_with_args(args: &[String]) -> Result<i32, crate::Failure> {
    Ok(run(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_path_defaults_to_whisper_tiny() {
        let p = model_path();
        assert!(p.ends_with("whisper/ggml-tiny.en.bin"));
    }

    #[test]
    fn hold_threshold_is_reasonable_push_to_talk_window() {
        assert!((250..=500).contains(&HOLD_THRESHOLD_MS));
    }
}
