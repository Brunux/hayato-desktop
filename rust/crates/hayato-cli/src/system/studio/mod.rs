//! Hayato Studio (`hayato studio` / `hayato-studio` / `hayato-camera`):
//! All-in-one Screen Recording, Webcam Video, and Photo Capture studio.
//!
//! - Ephemeral PipeWire virtual mixer (`hayato_rec_mix`) enables live audio
//!   switching (`Off` / `Mic` / `System` / `Both Mixed`) without restarting
//!   `wf-recorder` or `ffmpeg`.
//! - Direct `$SWAYSOCK` IPC provides 60 FPS cubic-eased webcam PiP gliding,
//!   scratchpad Hide/Show, `sticky enable`, and fullscreen awareness.
//! - Minimal Waybar `custom/rec` icon (`hayato studio --bar`) replaces the
//!   bulky `custom/busy` recording tab.

pub mod digits;
pub mod hud;
pub mod mixer;
pub mod screen;
pub mod sway_ipc;
pub mod v4l2;

use std::fs::File;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hayato_ui::recording::{
    self, AudioMode, Options, PipCorner, PipSize, QualityPreset, StudioSession,
};
use hayato_ui::{Design, Glyph, Urgency};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::hud::LiveHudScreen;
use self::screen::{StudioMode, StudioScreen};

pub fn signal_bar() {
    let _ = crate::system::out(&["pkill", "-RTMIN+8", "waybar"]);
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn videos_dir() -> PathBuf {
    dirs::video_dir().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join("Videos")
    })
}

pub fn camera_photos_dir() -> PathBuf {
    dirs::picture_dir()
        .map(|p| p.join("Camera"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join("Pictures").join("Camera")
        })
}

fn screen_output_file() -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    videos_dir().join(format!("hayato-screen-{stamp}.mp4"))
}

fn webcam_output_file() -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    videos_dir().join(format!("hayato-cam-{stamp}.mp4"))
}

fn photo_output_file() -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    camera_photos_dir().join(format!("hayato-photo-{stamp}.png"))
}

fn mpv_sock_path() -> PathBuf {
    recording::studio_state_dir().join("mpv.sock")
}

/// Send a JSON IPC command to the active `mpv` webcam viewfinder if running.
pub fn mpv_ipc_command(json_line: &str) -> bool {
    let sock = mpv_sock_path();
    let Ok(mut stream) = UnixStream::connect(&sock) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let msg = format!("{json_line}\n");
    stream.write_all(msg.as_bytes()).is_ok()
}

/// Toggle horizontal flip (mirror) live on the active `mpv` viewfinder.
pub fn mpv_toggle_mirror() -> bool {
    mpv_ipc_command(r#"{"command":["vf","toggle","hflip"]}"#)
}

/// Start the floating `mpv` webcam overlay/viewfinder on the auto-detected UVC node.
pub fn start_webcam_overlay(
    corner: PipCorner,
    size: PipSize,
    sticky: bool,
    mirror: bool,
    reduced_motion: bool,
) -> Result<u32, String> {
    if !crate::system::have("mpv") {
        return Err("mpv is not installed".to_string());
    }
    let cam = v4l2::find_webcam()
        .ok_or_else(|| "no webcam device found under /sys/class/video4linux".to_string())?;
    let state = recording::studio_state_dir();
    let _ = std::fs::create_dir_all(&state);
    let sock = mpv_sock_path();
    let _ = std::fs::remove_file(&sock);
    let log = state.join("camera.log");
    let log_out = File::create(&log).map_err(|e| format!("camera log: {e}"))?;
    let log_err = log_out.try_clone().map_err(|e| format!("camera log: {e}"))?;
    let (w, h) = size.dims();
    let url = format!("av://v4l2:{}", cam.device);

    let mut args = vec![
        "--no-config".to_string(),
        "--msg-level=all=error".to_string(),
        "--vo=wlshm".to_string(),
        "--no-osc".to_string(),
        "--no-input-default-bindings".to_string(),
        "--no-border".to_string(),
        "--ontop".to_string(),
        "--wayland-app-id=hayato-webcam".to_string(),
        "--title=Hayato Webcam".to_string(),
        format!("--geometry={w}x{h}"),
        format!("--input-ipc-server={}", sock.display()),
    ];
    if mirror {
        args.push("--vf=hflip".to_string());
    }
    args.push(url);

    let mut command = Command::new("mpv");
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start mpv: {e}"))?;
    let pid = child.id();

    for _ in 0..16 {
        std::thread::sleep(Duration::from_millis(100));
        if sway_ipc::webcam_exists() {
            break;
        }
    }
    sway_ipc::glide_pip(corner, size, sticky, true);
    std::thread::sleep(Duration::from_millis(250));
    sway_ipc::glide_pip(corner, size, sticky, reduced_motion);

    if let Ok(Some(status)) = child.try_wait() {
        let detail = last_line(&log);
        return Err(if detail.is_empty() {
            format!("webcam overlay exited ({status})")
        } else {
            format!("webcam overlay exited ({status}): {detail}")
        });
    }
    Ok(pid)
}

pub fn stop_pid_if_comm(pid: u32, expected: &[&str]) {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if expected.iter().any(|e| *e == comm) {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// Capture a still photo from the webcam: uses `mpv.sock` zero-flicker
/// `screenshot-to-file` when the viewfinder is open, or headless `ffmpeg` otherwise.
pub fn snap_photo(mirror: bool) -> Result<PathBuf, String> {
    let dir = camera_photos_dir();
    let _ = std::fs::create_dir_all(&dir);
    let dest = photo_output_file();

    if mpv_sock_path().exists() {
        let cmd = serde_json::json!({
            "command": ["screenshot-to-file", dest.to_string_lossy(), "video"]
        })
        .to_string();
        if mpv_ipc_command(&cmd) {
            for _ in 0..20 {
                if dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                    return Ok(dest);
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }

    let cam = v4l2::find_webcam()
        .ok_or_else(|| "no webcam device found under /sys/class/video4linux".to_string())?;
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-f".to_string(),
        "v4l2".to_string(),
        "-i".to_string(),
        cam.device,
        "-frames:v".to_string(),
        "1".to_string(),
    ];
    if mirror {
        args.push("-vf".to_string());
        args.push("hflip".to_string());
    }
    args.push(dest.to_string_lossy().into_owned());

    let out = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg failed to start: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "ffmpeg could not capture photo".to_string()
        } else {
            err
        });
    }
    Ok(dest)
}

fn slurp_geometry() -> Option<String> {
    let output = Command::new("slurp")
        .arg("-d")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let geom = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!geom.is_empty()).then_some(geom)
}

fn last_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| {
            t.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default()
}

fn spawn_self(args: &[String]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::system::spawn_detached_argv(exe, &refs);
}

pub fn trigger_start(kind: &str, area: bool) {
    let mut args = vec![
        "menu".to_string(),
        "studio".to_string(),
        "--start".to_string(),
        kind.to_string(),
    ];
    if area {
        args.push("area".to_string());
    }
    spawn_self(&args);
}

pub fn trigger_stop() {
    spawn_self(&[
        "menu".to_string(),
        "studio".to_string(),
        "--stop".to_string(),
    ]);
}

/// Detached start entrypoint (`hayato menu studio --start screen|webcam [area]`).
pub fn start_headless(kind: &str, area: bool) -> i32 {
    std::thread::sleep(Duration::from_millis(200));
    let geometry = if kind == "screen" && area {
        match slurp_geometry() {
            Some(g) => Some(g),
            None => return 0,
        }
    } else {
        None
    };

    let options = recording::load_options();
    let state = recording::studio_state_dir();
    let _ = std::fs::create_dir_all(&state);

    // Start ephemeral PipeWire virtual mixer so audio can switch/mix live.
    let mixer_ok = mixer::start_mixer(&state, options.audio).is_ok();

    let mut cam_pid: Option<u32> = None;
    let file = if kind == "webcam" {
        webcam_output_file()
    } else {
        screen_output_file()
    };
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if kind == "screen" && options.webcam {
        match start_webcam_overlay(
            options.corner,
            options.pip_size,
            options.sticky,
            options.mirror,
            true,
        ) {
            Ok(pid) => cam_pid = Some(pid),
            Err(err) => {
                mixer::teardown_mixer(&state);
                crate::system::say("Hayato Studio", err, Urgency::Critical, Glyph::Warning);
                return 1;
            }
        }
    }

    let log = state.join("recorder.log");
    let Ok(log_out) = File::create(&log) else {
        if let Some(cp) = cam_pid {
            stop_pid_if_comm(cp, &["mpv"]);
        }
        mixer::teardown_mixer(&state);
        return 1;
    };
    let Ok(log_err) = log_out.try_clone() else {
        return 1;
    };

    let (program, argv, comm_name) = if kind == "webcam" {
        let Some(cam) = v4l2::find_webcam() else {
            mixer::teardown_mixer(&state);
            crate::system::say(
                "Hayato Studio",
                "No webcam device found under /sys/class/video4linux".to_string(),
                Urgency::Critical,
                Glyph::Warning,
            );
            return 1;
        };
        let mut a = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            "v4l2".to_string(),
            "-i".to_string(),
            cam.device,
        ];
        if mixer_ok {
            a.extend([
                "-f".to_string(),
                "pulse".to_string(),
                "-i".to_string(),
                mixer::MIX_MONITOR.to_string(),
            ]);
        }
        if options.mirror {
            a.extend(["-vf".to_string(), "hflip".to_string()]);
        }
        a.extend([
            "-c:v".to_string(),
            "libx264".to_string(),
            "-preset".to_string(),
            options.quality.x264_preset().to_string(),
            "-crf".to_string(),
            options.quality.crf().to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
        ]);
        if mixer_ok {
            a.extend(["-c:a".to_string(), "aac".to_string(), "-b:a".to_string(), "128k".to_string()]);
        }
        a.push(file.to_string_lossy().into_owned());
        ("ffmpeg", a, "ffmpeg")
    } else {
        let mut a = vec![
            "-c".to_string(),
            "libx264".to_string(),
            "-p".to_string(),
            format!("preset={}", options.quality.x264_preset()),
            "-p".to_string(),
            format!("crf={}", options.quality.crf()),
            "-r".to_string(),
            options.quality.fps().to_string(),
        ];
        if mixer_ok {
            a.push(format!("--audio={}", mixer::MIX_MONITOR));
        }
        if let Some(ref g) = geometry {
            a.push("-g".to_string());
            a.push(g.clone());
        }
        a.push("-f".to_string());
        a.push(file.to_string_lossy().into_owned());
        ("wf-recorder", a, "wf-recorder")
    };

    let mut command = Command::new(program);
    command
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            if let Some(cp) = cam_pid {
                stop_pid_if_comm(cp, &["mpv"]);
            }
            mixer::teardown_mixer(&state);
            crate::system::say(
                "Hayato Studio",
                format!("Could not start {program}: {e}"),
                Urgency::Critical,
                Glyph::Warning,
            );
            return 1;
        }
    };

    let pid = child.id();
    std::thread::sleep(Duration::from_millis(350));

    let session = StudioSession {
        kind: kind.to_string(),
        pid,
        comm: comm_name.to_string(),
        file: file.clone(),
        started_epoch: now_epoch(),
        quality: options.quality,
        audio: options.audio,
        geometry,
        cam_pid,
        cam_visible: cam_pid.is_some(),
        cam_sticky: options.sticky,
        cam_corner: options.corner,
        cam_size: options.pip_size,
        cam_mirror: options.mirror,
    };

    if recording::write_studio_session(&session).is_ok()
        && recording::read_studio_session().is_some()
    {
        signal_bar();
        0
    } else {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
        if let Some(cp) = cam_pid {
            stop_pid_if_comm(cp, &["mpv"]);
        }
        mixer::teardown_mixer(&state);
        recording::clear_studio_session();
        let why = last_line(&log);
        crate::system::say(
            "Hayato Studio",
            if why.is_empty() {
                "Recording did not start".to_string()
            } else {
                format!("Recording did not start · {why}")
            },
            Urgency::Critical,
            Glyph::Warning,
        );
        signal_bar();
        1
    }
}

/// Stop the active Studio recording cleanly, finalize MP4, tear down PipeWire mixer,
/// close webcam PiP, update Waybar, and send one saved toast.
pub fn stop_live() -> i32 {
    let state = recording::studio_state_dir();
    let Some(session) = recording::read_studio_session() else {
        mixer::teardown_mixer(&state);
        recording::clear_studio_session();
        signal_bar();
        return 0;
    };

    unsafe {
        libc::kill(session.pid as libc::pid_t, libc::SIGINT);
    }
    let mut gone = false;
    for _ in 0..50 {
        if recording::read_studio_session().is_none() {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !gone {
        unsafe {
            libc::kill(session.pid as libc::pid_t, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    if let Some(cp) = session.cam_pid {
        stop_pid_if_comm(cp, &["mpv"]);
    }
    mixer::teardown_mixer(&state);
    recording::clear_studio_session();
    signal_bar();

    let base = session
        .file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording.mp4".to_string());
    let ok = session
        .file
        .metadata()
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if ok {
        crate::system::say_open(
            "Hayato Studio",
            format!("Recording saved · {base}"),
            Urgency::Normal,
            Glyph::Video,
            &session.file,
        );
    } else {
        crate::system::say(
            "Hayato Studio",
            format!("Recording failed · {base} is empty"),
            Urgency::Critical,
            Glyph::Warning,
        );
    }
    0
}

pub fn set_audio_live(mode: AudioMode) {
    let state = recording::studio_state_dir();
    mixer::apply_audio_mode(&state, mode);
    if let Some(mut s) = recording::read_studio_session() {
        s.audio = mode;
        let _ = recording::write_studio_session(&s);
    }
    let mut opts = recording::load_options();
    opts.audio = mode;
    let _ = recording::save_options(opts);
    signal_bar();
}

pub fn move_pip_live(corner: PipCorner, size: PipSize, reduced_motion: bool) {
    if let Some(mut s) = recording::read_studio_session() {
        s.cam_corner = corner;
        s.cam_size = size;
        if s.cam_visible {
            sway_ipc::glide_pip(corner, size, s.cam_sticky, reduced_motion);
        }
        let _ = recording::write_studio_session(&s);
    }
    let mut opts = recording::load_options();
    opts.corner = corner;
    opts.pip_size = size;
    let _ = recording::save_options(opts);
    signal_bar();
}

pub fn toggle_pip_visible(reduced_motion: bool) {
    if let Some(mut s) = recording::read_studio_session() {
        if s.cam_pid.is_none() {
            if let Ok(pid) = start_webcam_overlay(
                s.cam_corner,
                s.cam_size,
                s.cam_sticky,
                s.cam_mirror,
                reduced_motion,
            ) {
                s.cam_pid = Some(pid);
                s.cam_visible = true;
            }
        } else {
            s.cam_visible = !s.cam_visible;
            sway_ipc::set_pip_visible(s.cam_visible, s.cam_corner, s.cam_size, s.cam_sticky);
        }
        let _ = recording::write_studio_session(&s);
        signal_bar();
    }
}

pub fn toggle_pip_sticky() {
    if let Some(mut s) = recording::read_studio_session() {
        s.cam_sticky = !s.cam_sticky;
        sway_ipc::set_pip_sticky(s.cam_sticky);
        let _ = recording::write_studio_session(&s);
        let mut opts = recording::load_options();
        opts.sticky = s.cam_sticky;
        let _ = recording::save_options(opts);
        signal_bar();
    }
}

pub fn toggle_pip_mirror() {
    if let Some(mut s) = recording::read_studio_session() {
        s.cam_mirror = !s.cam_mirror;
        let _ = mpv_toggle_mirror();
        let _ = recording::write_studio_session(&s);
        let mut opts = recording::load_options();
        opts.mirror = s.cam_mirror;
        let _ = recording::save_options(opts);
        signal_bar();
    }
}

/// Waybar `custom/rec` JSON payload (`hayato studio --bar`).
/// Glyph-only on the bar (0 px when idle); telemetry lives in the Cyber-HUD tooltip.
pub fn bar_json() -> String {
    let Some(s) = recording::read_studio_session() else {
        return "{\"class\":\"empty\"}".to_string();
    };
    let elapsed = format_elapsed(now_epoch().saturating_sub(s.started_epoch));
    let size = s
        .file
        .metadata()
        .map(|m| format_bytes(m.len()))
        .unwrap_or_else(|_| "0 B".to_string());
    let mode_label = if s.kind == "webcam" {
        format!("Webcam ({})", s.quality.as_str())
    } else {
        format!("Screen ({})", s.quality.as_str())
    };
    let cam_label = if s.cam_pid.is_some() {
        let vis = if s.cam_visible {
            s.cam_corner.short()
        } else {
            "Hidden"
        };
        let st = if s.cam_sticky { " · Sticky" } else { "" };
        format!("{vis}{st}")
    } else {
        "Off".to_string()
    };

    let reduced = std::env::var("HAYATO_REDUCED_MOTION").as_deref() == Ok("1")
        || dirs::config_dir()
            .map(|d| d.join("hayato/reduced-motion").exists())
            .unwrap_or(false);
    let class = if reduced {
        "recording-static"
    } else {
        "recording"
    };

    let tooltip = format!(
        "<span alpha='50%'>╭─</span> <b>REC // LIVE · {elapsed}</b>\\n\
         <span alpha='45%'>│</span> <span alpha='60%'>MODE</span>  {mode_label}\\n\
         <span alpha='45%'>│</span> <span alpha='60%'>SIZE</span>  {size}\\n\
         <span alpha='45%'>│</span> <span alpha='60%'>AUD </span>  {}\\n\
         <span alpha='45%'>│</span> <span alpha='60%'>CAM </span>  {cam_label}\\n\
         <span alpha='50%'>╰─</span> <span alpha='65%'>[LMB] live HUD · [RMB] stop</span>",
        s.audio.short_label()
    );
    format!(
        "{{\"text\":\"<span font_desc='JetBrains Mono 13'>\u{f111}</span>\",\"class\":\"{class}\",\"tooltip\":\"{tooltip}\"}}"
    )
}

/// Non-interactive query / CLI control handler (invoked in `lib.rs::query()`).
pub fn try_query(args: &[String]) -> Option<i32> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--bar"] | ["bar"] => {
            println!("{}", bar_json());
            Some(0)
        }
        ["--status"] | ["status"] => {
            if recording::is_recording() {
                println!("recording");
            } else {
                println!("idle");
            }
            Some(0)
        }
        ["--stop"] | ["stop"] => Some(stop_live()),
        ["--start", kind] => Some(start_headless(kind, false)),
        ["--start", kind, "area"] => Some(start_headless(kind, true)),
        ["--audio", mode] => {
            if let Some(m) = AudioMode::parse(mode) {
                set_audio_live(m);
                Some(0)
            } else {
                Some(2)
            }
        }
        ["--pip", corner] => {
            if let Some(c) = PipCorner::parse(corner) {
                let sz = recording::read_studio_session()
                    .map(|s| s.cam_size)
                    .unwrap_or(PipSize::Medium);
                move_pip_live(c, sz, false);
                Some(0)
            } else {
                Some(2)
            }
        }
        _ => None,
    }
}

/// Interactive entrypoint (`hayato menu studio [--hud | --mode screen|webcam|photo]`).
pub fn menu(
    args: &[String],
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    design: &Design,
) -> Result<i32, crate::Failure> {
    let want_hud = args.iter().any(|a| a == "--hud");
    if want_hud || recording::read_studio_session().is_some() {
        if let Some(session) = recording::read_studio_session() {
            let mut hud = LiveHudScreen::new(session);
            return crate::run_screen(terminal, design, "Hayato Studio · Live", &mut hud);
        }
    }

    let mut initial_mode = StudioMode::Screen;
    for pair in args.windows(2) {
        if pair[0] == "--mode" {
            initial_mode = match pair[1].as_str() {
                "webcam" | "video" => StudioMode::Webcam,
                "photo" | "camera" => StudioMode::Photo,
                _ => StudioMode::Screen,
            };
        }
    }

    let mut screen = StudioScreen::new(initial_mode);
    crate::run_screen(terminal, design, "Hayato Studio", &mut screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_json_is_empty_when_idle() {
        // When no studio session is active in the test environment, class is empty or recording
        let j = bar_json();
        assert!(j.starts_with('{') && j.ends_with('}'));
    }

    #[test]
    fn format_helpers_work() {
        assert_eq!(format_elapsed(0), "00:00:00");
        assert_eq!(format_elapsed(3665), "01:01:05");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
    }
}
