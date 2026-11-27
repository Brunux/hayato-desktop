//! Session-scoped phone camera ingest, snapshot capture, and optional
//! `v4l2loopback` / `mpv` viewfinder sink for Hayato Connect.
//!
//! Zero background daemons: all pipes and child processes are owned by
//! `CamSink` and terminated immediately when stopped or when `ConnectScreen`
//! drops.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_AUDIO_CHUNK: usize = 65_536;
const PHONE_MIC_SINK: &str = "hayato_phone_mic";

#[derive(Clone, Debug, Default)]
pub struct CamTelemetry {
    pub active: bool,
    pub viewfinder_running: bool,
    pub mic_enabled: bool,
    pub mic_active: bool,
    pub v4l2_device: Option<String>,
    pub resolution: String,
    pub facing: String,
    pub fps: u16,
    pub kbps: u32,
    pub frames: u64,
}

struct ActiveFeed {
    child: Child,
    stdin: ChildStdin,
    label: String,
}

struct AudioFeed {
    child: Child,
    stdin: ChildStdin,
    mod_id: Option<u32>,
}

struct InnerCam {
    latest_jpeg: Option<Vec<u8>>,
    feed: Option<ActiveFeed>,
    audio: Option<AudioFeed>,
    telemetry: CamTelemetry,
    window_start: Instant,
    window_frames: u32,
    window_bytes: usize,
}

pub struct CamSink {
    inner: Arc<Mutex<InnerCam>>,
}

impl Default for CamSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CamSink {
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl CamSink {
    pub fn new() -> Self {
        let v4l2 = find_loopback_device(Path::new("/sys/class/video4linux"));
        Self {
            inner: Arc::new(Mutex::new(InnerCam {
                latest_jpeg: None,
                feed: None,
                audio: None,
                telemetry: CamTelemetry {
                    v4l2_device: v4l2,
                    mic_enabled: true,
                    facing: "environment".to_string(),
                    ..CamTelemetry::default()
                },
                window_start: Instant::now(),
                window_frames: 0,
                window_bytes: 0,
            })),
        }
    }

    pub fn set_mic_enabled(&self, enabled: bool) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.telemetry.mic_enabled = enabled;
        if !enabled {
            stop_audio_locked(&mut g);
        }
    }

    pub fn mic_enabled(&self) -> bool {
        self.inner
            .lock()
            .map(|g| g.telemetry.mic_enabled)
            .unwrap_or(false)
    }

    /// Feed raw 24 kHz mono signed 16-bit LE PCM samples into an ephemeral
    /// PipeWire virtual sink (`hayato_phone_mic`), auto-created on first packet
    /// and torn down cleanly on toggle or `Drop`.
    pub fn push_pcm_audio(&self, pcm: &[u8]) {
        if pcm.is_empty() || pcm.len() > MAX_AUDIO_CHUNK || !pcm.len().is_multiple_of(2) {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if !g.telemetry.mic_enabled {
            return;
        }
        if g.audio.is_none() && start_audio_locked(&mut g).is_err() {
            return;
        }
        if let Some(aud) = &mut g.audio
            && (aud.stdin.write_all(pcm).is_err() || aud.stdin.flush().is_err())
        {
            stop_audio_locked(&mut g);
        }
    }

    pub fn set_meta(&self, active: bool, facing: String, width: u32, height: u32) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.telemetry.active = active;
        if !facing.is_empty() {
            g.telemetry.facing = facing;
        }
        if width > 0 && height > 0 {
            g.telemetry.resolution = format!("{width}x{height}");
        }
        if !active {
            g.telemetry.fps = 0;
            g.telemetry.kbps = 0;
            stop_feed_locked(&mut g);
        }
    }

    /// Accept one JPEG frame from an authenticated phone WebSocket connection.
    pub fn push_jpeg_frame(&self, frame: &[u8]) {
        if !is_valid_jpeg(frame) {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.telemetry.active = true;
        g.telemetry.frames += 1;
        g.window_frames += 1;
        g.window_bytes += frame.len();

        let elapsed = g.window_start.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            g.telemetry.fps = (g.window_frames as f64 / elapsed).round() as u16;
            g.telemetry.kbps = ((g.window_bytes as f64 * 8.0 / 1000.0) / elapsed).round() as u32;
            g.window_start = Instant::now();
            g.window_frames = 0;
            g.window_bytes = 0;
        }
        g.latest_jpeg = Some(frame.to_vec());

        if let Some(feed) = &mut g.feed {
            let dead = feed.stdin.write_all(frame).is_err() || feed.stdin.flush().is_err();
            if dead {
                stop_feed_locked(&mut g);
            }
        }
    }

    pub fn telemetry(&self) -> CamTelemetry {
        self.inner
            .lock()
            .map(|g| g.telemetry.clone())
            .unwrap_or_default()
    }

    /// Start streaming MJPEG frames into `/dev/videoN` (if `v4l2loopback` is
    /// present) or an ephemeral floating `mpv` viewfinder window.
    pub fn toggle_viewfinder(&self) -> Result<String, String> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| "camera lock poisoned".to_string())?;
        if g.feed.is_some() {
            let label = g.feed.as_ref().map(|f| f.label.clone()).unwrap_or_default();
            stop_feed_locked(&mut g);
            return Ok(format!("Stopped {label}"));
        }

        if let Some(dev) = g.telemetry.v4l2_device.clone() {
            let child = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "image2pipe",
                    "-vcodec",
                    "mjpeg",
                    "-i",
                    "-",
                    "-vf",
                    "format=yuv420p",
                    "-f",
                    "v4l2",
                    &dev,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("spawn ffmpeg v4l2 sink: {e}"))?;
            attach_feed(&mut g, child, format!("Virtual Cam ({dev})"))
        } else {
            let child = Command::new("mpv")
                .args([
                    "--wayland-app-id=hayato-webcam",
                    "--title=Hayato Phone Cam",
                    "--no-cache",
                    "--profile=low-latency",
                    "--untimed",
                    "--demuxer-lavf-format=mpjpeg",
                    "-",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("spawn mpv viewfinder: {e}"))?;
            attach_feed(&mut g, child, "Phone Cam Viewfinder (mpv)".to_string())
        }
    }

    pub fn save_snapshot(&self) -> Result<PathBuf, String> {
        let bytes = {
            let g = self
                .inner
                .lock()
                .map_err(|_| "camera lock poisoned".to_string())?;
            g.latest_jpeg
                .clone()
                .ok_or_else(|| "No phone camera frame received yet".to_string())?
        };
        let dir = dirs::picture_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Camera");
        fs::create_dir_all(&dir).map_err(|e| format!("create Camera dir: {e}"))?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("hayato-phone-{stamp}.jpg"));
        fs::write(&path, &bytes).map_err(|e| format!("write snapshot: {e}"))?;
        Ok(path)
    }

    pub fn stop_all(&self) {
        if let Ok(mut g) = self.inner.lock() {
            stop_feed_locked(&mut g);
            stop_audio_locked(&mut g);
            g.telemetry.active = false;
            g.telemetry.fps = 0;
            g.telemetry.kbps = 0;
        }
    }
}

fn start_audio_locked(g: &mut InnerCam) -> Result<(), String> {
    cleanup_phone_mic_orphans();
    let out = Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            &format!("sink_name={PHONE_MIC_SINK}"),
            "sink_properties=device.description=Hayato_Phone_Mic node.dont-fallback=true",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("pactl load-module: {e}"))?;
    let mod_id = if out.status.success() {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u32>()
            .ok()
    } else {
        None
    };

    let mut child = Command::new("pw-cat")
        .args([
            "--playback",
            "--format=s16",
            "--rate=24000",
            "--channels=1",
            &format!("--target={PHONE_MIC_SINK}"),
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if let Some(id) = mod_id {
                unload_pactl_module(id);
            }
            format!("spawn pw-cat mic sink: {e}")
        })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pw-cat stdin missing".to_string())?;
    g.telemetry.mic_active = true;
    g.audio = Some(AudioFeed {
        child,
        stdin,
        mod_id,
    });
    Ok(())
}

fn stop_audio_locked(g: &mut InnerCam) {
    if let Some(mut aud) = g.audio.take() {
        drop(aud.stdin);
        let _ = aud.child.kill();
        let _ = aud.child.wait();
        if let Some(id) = aud.mod_id {
            unload_pactl_module(id);
        }
        cleanup_phone_mic_orphans();
    }
    g.telemetry.mic_active = false;
}

fn unload_pactl_module(id: u32) {
    let _ = Command::new("pactl")
        .args(["unload-module", &id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn cleanup_phone_mic_orphans() {
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "modules"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.contains(PHONE_MIC_SINK)
            && let Some(id) = line.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            unload_pactl_module(id);
        }
    }
}

fn attach_feed(g: &mut InnerCam, mut child: Child, label: String) -> Result<String, String> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "capture child stdin missing".to_string())?;
    if let Some(frame) = &g.latest_jpeg {
        let _ = stdin.write_all(frame);
        let _ = stdin.flush();
    }
    g.telemetry.viewfinder_running = true;
    g.feed = Some(ActiveFeed {
        child,
        stdin,
        label: label.clone(),
    });
    Ok(format!("Started {label}"))
}

fn stop_feed_locked(g: &mut InnerCam) {
    if let Some(mut feed) = g.feed.take() {
        drop(feed.stdin);
        let _ = feed.child.kill();
        let _ = feed.child.wait();
    }
    g.telemetry.viewfinder_running = false;
}

pub fn is_valid_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.len() <= MAX_FRAME_BYTES && bytes[0] == 0xFF && bytes[1] == 0xD8
}

pub fn find_loopback_device(sys_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(sys_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("video") {
            continue;
        }
        let drv = fs::canonicalize(entry.path().join("device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if drv == "v4l2loopback" {
            return Some(format!("/dev/{name}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_validation_and_frame_counter() {
        assert!(!is_valid_jpeg(b"not-jpeg"));
        assert!(!is_valid_jpeg(&[0xFF]));
        assert!(is_valid_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]));

        let sink = CamSink::new();
        sink.push_jpeg_frame(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]);
        let tel = sink.telemetry();
        assert!(tel.active);
        assert_eq!(tel.frames, 1);

        let tmp = tempfile::tempdir().unwrap();
        assert!(find_loopback_device(tmp.path()).is_none());
    }
}
