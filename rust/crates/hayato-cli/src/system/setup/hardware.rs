//! Self-contained hardware verification helpers for Step 5 (Hardware).
//!
//! - Stereo speaker chime synthesized in pure Rust and piped to `paplay`
//!   (left channel 440 Hz, then right channel 523 Hz — zero external assets).
//! - 3-second microphone capture via `parecord` with peak level detection and
//!   instant playback via `paplay`.
//! - Live camera preview via `mpv av://v4l2:<dev>`.

use std::f32::consts::TAU;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Play a short, pleasant stereo chime (Left 440 Hz -> Right 523 Hz) through PipeWire.
pub fn play_speaker_chime() -> Result<String, String> {
    let pcm = synthesize_stereo_chime();
    let mut child = Command::new("paplay")
        .args([
            "--raw",
            "--rate=16000",
            "--channels=2",
            "--format=s16le",
            "--client-name=Hayato Setup",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("paplay could not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&pcm);
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok("Playing stereo test chime (Left -> Right)".to_string())
}

/// Synthesize 0.8s of 16 kHz stereo 16-bit LE PCM: 0.4s left 440 Hz, 0.4s right 523.25 Hz.
pub fn synthesize_stereo_chime() -> Vec<u8> {
    let rate = 16_000usize;
    let half = rate * 4 / 10; // 0.4s per channel
    let mut out = Vec::with_capacity(half * 2 * 4);
    for (ch_idx, freq) in [(0usize, 440.0f32), (1usize, 523.25f32)] {
        for i in 0..half {
            let t = i as f32 / rate as f32;
            let env = if i < 600 {
                i as f32 / 600.0
            } else if i + 1600 > half {
                (half - i) as f32 / 1600.0
            } else {
                1.0
            };
            let sample = ((TAU * freq * t).sin() * env * 9000.0) as i16;
            let (l, r) = if ch_idx == 0 {
                (sample, 0i16)
            } else {
                (0i16, sample)
            };
            out.extend_from_slice(&l.to_le_bytes());
            out.extend_from_slice(&r.to_le_bytes());
        }
    }
    out
}

/// Record 3 seconds from `@DEFAULT_SOURCE@`, returning `(temp_pcm_path, peak_0_to_1)`.
pub fn record_mic_sample() -> Result<(PathBuf, f32), String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = PathBuf::from(runtime).join("hayato-setup-mic-test.pcm");
    let mut child = Command::new("parecord")
        .args([
            "--raw",
            "--rate=16000",
            "--channels=1",
            "--format=s16le",
            "--client-name=Hayato Setup Mic Test",
        ])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("parecord could not start: {e}"))?;

    std::thread::sleep(std::time::Duration::from_secs(3));
    let _ = child.kill();
    let _ = child.wait();

    let bytes = std::fs::read(&path).map_err(|e| format!("could not read recording: {e}"))?;
    let peak = compute_s16le_peak(&bytes);
    Ok((path, peak))
}

pub fn compute_s16le_peak(bytes: &[u8]) -> f32 {
    let mut max_abs = 0i32;
    for &chunk in bytes.as_chunks::<2>().0 {
        let s = i16::from_le_bytes(chunk) as i32;
        max_abs = max_abs.max(s.abs());
    }
    (max_abs as f32 / i16::MAX as f32).clamp(0.0, 1.0)
}

pub fn playback_mic_sample(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err("No mic recording yet — run Mic Test first".to_string());
    }
    let _ = Command::new("paplay")
        .args([
            "--raw",
            "--rate=16000",
            "--channels=1",
            "--format=s16le",
            "--client-name=Hayato Setup Playback",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("paplay could not start: {e}"))?;
    Ok("Playing back your 3-second mic test".to_string())
}

pub fn preview_camera(device: &str) -> Result<String, String> {
    let url = format!("av://v4l2:{device}");
    let _ = Command::new("mpv")
        .args([
            "--no-config",
            "--profile=low-latency",
            "--untimed",
            "--title=Hayato Camera Test",
            "--geometry=640x480",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("mpv could not open {device}: {e}"))?;
    Ok(format!("Opened camera preview for {device}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_chime_has_expected_length_and_signal() {
        let pcm = synthesize_stereo_chime();
        assert_eq!(pcm.len(), 16_000 * 4 / 10 * 2 * 4);
        assert!(compute_s16le_peak(&pcm) > 0.2);
    }

    #[test]
    fn silence_has_zero_peak() {
        let zeros = vec![0u8; 256];
        assert_eq!(compute_s16le_peak(&zeros), 0.0);
    }
}
