//! Local audio playback engine (`symphonia` in-process decoder + `ffmpeg` Opus
//! fallback + `cpal` ALSA/PipeWire output + lock-free `PcmTapWriter` tap).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::RingBuffer;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::PlayState;
use crate::dsp::{PcmTapWriter, SpectrumAnalyzer, create_tap};

/// Commands sent from the UI/MPRIS thread to the audio engine.
#[derive(Clone, Debug)]
pub enum EngineCommand {
    PlayFile { path: PathBuf, duration_secs: u64 },
    Pause,
    Resume,
    Stop,
    SeekSecs(u64),
    SetVolume(u8),
    SetMuted(bool),
    Shutdown,
}

/// Shared lock-free telemetry polled by the UI thread at 30 FPS (0 allocations).
pub struct EngineTelemetry {
    state: AtomicU32, // 0=Stopped, 1=Playing, 2=Paused
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    finished_gen: AtomicU64,
    volume_pct: AtomicU32,
    muted: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl EngineTelemetry {
    fn new(volume_pct: u8) -> Self {
        Self {
            state: AtomicU32::new(0),
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            finished_gen: AtomicU64::new(0),
            volume_pct: AtomicU32::new(u32::from(volume_pct.min(100))),
            muted: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    pub fn play_state(&self) -> PlayState {
        match self.state.load(Ordering::Relaxed) {
            1 => PlayState::Playing,
            2 => PlayState::Paused,
            _ => PlayState::Stopped,
        }
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms.load(Ordering::Relaxed)
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms.load(Ordering::Relaxed)
    }

    pub fn finished_gen(&self) -> u64 {
        self.finished_gen.load(Ordering::Relaxed)
    }

    pub fn volume_pct(&self) -> u8 {
        self.volume_pct.load(Ordering::Relaxed) as u8
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn take_error(&self) -> Option<String> {
        self.last_error.lock().ok()?.take()
    }

    fn set_state(&self, s: PlayState) {
        let v = match s {
            PlayState::Stopped => 0,
            PlayState::Playing => 1,
            PlayState::Paused => 2,
        };
        self.state.store(v, Ordering::Relaxed);
    }

    fn set_error(&self, msg: String) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(msg);
        }
    }
}

/// Handle owned by the UI thread to drive local audio playback.
pub struct AudioEngine {
    tx: Sender<EngineCommand>,
    pub telemetry: Arc<EngineTelemetry>,
}

impl AudioEngine {
    pub fn spawn(initial_volume: u8) -> (Self, SpectrumAnalyzer) {
        let (tx, rx) = mpsc::channel();
        let telemetry = Arc::new(EngineTelemetry::new(initial_volume));
        let (tap_tx, analyzer) = create_tap();
        let tel_clone = Arc::clone(&telemetry);

        thread::Builder::new()
            .name("cassette-audio".into())
            .spawn(move || engine_loop(rx, tel_clone, tap_tx))
            .expect("spawn audio engine thread");

        (Self { tx, telemetry }, analyzer)
    }

    pub fn send(&self, cmd: EngineCommand) {
        let _ = self.tx.send(cmd);
    }

    pub fn play_file(&self, path: PathBuf, duration_secs: u64) {
        self.send(EngineCommand::PlayFile {
            path,
            duration_secs,
        });
    }

    pub fn pause(&self) {
        self.send(EngineCommand::Pause);
    }

    pub fn resume(&self) {
        self.send(EngineCommand::Resume);
    }

    pub fn stop(&self) {
        self.send(EngineCommand::Stop);
    }

    pub fn seek_secs(&self, secs: u64) {
        self.send(EngineCommand::SeekSecs(secs));
    }

    pub fn set_volume(&self, pct: u8) {
        let clamped = pct.min(100);
        self.telemetry
            .volume_pct
            .store(u32::from(clamped), Ordering::Relaxed);
        self.send(EngineCommand::SetVolume(clamped));
    }

    pub fn set_muted(&self, muted: bool) {
        self.telemetry.muted.store(muted, Ordering::Relaxed);
        self.send(EngineCommand::SetMuted(muted));
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.send(EngineCommand::Shutdown);
    }
}

struct ActiveOutput {
    _stream: cpal::Stream,
    prod: rtrb::Producer<f32>,
    sample_rate: u32,
    channels: usize,
}

fn open_output(
    sample_rate: u32,
    channels: u16,
    telemetry: Arc<EngineTelemetry>,
    tap: Arc<Mutex<PcmTapWriter>>,
) -> Result<ActiveOutput, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No audio output device available".to_string())?;

    let desired_ch = channels.clamp(1, 2);
    let config = cpal::StreamConfig {
        channels: desired_ch,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    // ~1.0s audio buffer
    let ring_len = (sample_rate as usize * desired_ch as usize).max(16384);
    let (prod, mut cons) = RingBuffer::<f32>::new(ring_len);

    let err_tel = Arc::clone(&telemetry);
    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let paused = telemetry.play_state() != PlayState::Playing;
                let muted = telemetry.muted();
                let gain = if muted {
                    0.0
                } else {
                    let v = telemetry.volume_pct() as f32 / 100.0;
                    // Perceptual volume curve
                    v * v
                };

                if paused {
                    for s in data.iter_mut() {
                        *s = 0.0;
                    }
                    return;
                }

                for s in data.iter_mut() {
                    *s = cons.pop().unwrap_or(0.0) * gain;
                }

                if let Ok(mut guard) = tap.try_lock() {
                    guard.push_interleaved(data, desired_ch as usize);
                }
            },
            move |err| {
                err_tel.set_error(format!("Audio stream error: {err}"));
            },
            None,
        )
        .map_err(|e| format!("open audio stream ({sample_rate}Hz): {e}"))?;

    stream
        .play()
        .map_err(|e| format!("start audio stream: {e}"))?;

    Ok(ActiveOutput {
        _stream: stream,
        prod,
        sample_rate,
        channels: desired_ch as usize,
    })
}

enum DecoderBackend {
    Symphonia {
        format: Box<dyn symphonia::core::formats::FormatReader>,
        decoder: Box<dyn symphonia::core::codecs::Decoder>,
        track_id: u32,
        sample_rate: u32,
        channels: usize,
        sample_buf: Option<SampleBuffer<f32>>,
    },
    Ffmpeg {
        child: Child,
        path: PathBuf,
        sample_rate: u32,
        channels: usize,
        played_frames: u64,
    },
}

impl DecoderBackend {
    fn open(path: &Path, start_secs: u64) -> Result<Self, String> {
        // Use ffmpeg streaming fallback for Opus (symphonia 0.5 lacks native Opus)
        let is_opus = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("opus"));
        if is_opus {
            return Self::open_ffmpeg(path, start_secs);
        }
        match Self::open_symphonia(path, start_secs) {
            Ok(s) => Ok(s),
            Err(_) => Self::open_ffmpeg(path, start_secs),
        }
    }

    fn open_symphonia(path: &Path, start_secs: u64) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("probe failed: {e}"))?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| "no audio track found".to_string())?;
        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count())
            .unwrap_or(2)
            .clamp(1, 2);

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("unsupported codec: {e}"))?;

        if start_secs > 0 {
            let _ = format.seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::new(start_secs, 0.0),
                    track_id: Some(track_id),
                },
            );
        }

        Ok(Self::Symphonia {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            sample_buf: None,
        })
    }

    fn open_ffmpeg(path: &Path, start_secs: u64) -> Result<Self, String> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error"]);
        if start_secs > 0 {
            cmd.args(["-ss", &start_secs.to_string()]);
        }
        cmd.arg("-i")
            .arg(path)
            .args(["-f", "f32le", "-ac", "2", "-ar", "48000", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn ffmpeg decoder: {e}"))?;
        Ok(Self::Ffmpeg {
            child,
            path: path.to_path_buf(),
            sample_rate: 48000,
            channels: 2,
            played_frames: start_secs * 48000,
        })
    }

    fn sample_rate(&self) -> u32 {
        match self {
            Self::Symphonia { sample_rate, .. } | Self::Ffmpeg { sample_rate, .. } => *sample_rate,
        }
    }

    fn channels(&self) -> usize {
        match self {
            Self::Symphonia { channels, .. } | Self::Ffmpeg { channels, .. } => *channels,
        }
    }

    fn seek(&mut self, secs: u64) -> Result<(), String> {
        match self {
            Self::Symphonia {
                format,
                decoder,
                track_id,
                ..
            } => {
                format
                    .seek(
                        SeekMode::Coarse,
                        SeekTo::Time {
                            time: Time::new(secs, 0.0),
                            track_id: Some(*track_id),
                        },
                    )
                    .map_err(|e| format!("seek failed: {e}"))?;
                decoder.reset();
                Ok(())
            }
            Self::Ffmpeg { child, path, .. } => {
                let _ = child.kill();
                let _ = child.wait();
                let p = path.clone();
                *self = Self::open_ffmpeg(&p, secs)?;
                Ok(())
            }
        }
    }

    /// Decode next chunk of interleaved `f32` samples into `out`.
    /// Returns `Ok(Some(position_ms))`, `Ok(None)` on clean EOF.
    fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<Option<u64>, String> {
        out.clear();
        match self {
            Self::Symphonia {
                format,
                decoder,
                track_id,
                sample_rate,
                sample_buf,
                ..
            } => loop {
                let packet = match format.next_packet() {
                    Ok(p) => p,
                    Err(SymphoniaError::IoError(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        return Ok(None);
                    }
                    Err(SymphoniaError::ResetRequired) => {
                        decoder.reset();
                        continue;
                    }
                    Err(e) => return Err(format!("read packet: {e}")),
                };
                if packet.track_id() != *track_id {
                    continue;
                }
                let ts_ms = if *sample_rate > 0 {
                    packet.ts().saturating_mul(1000) / u64::from(*sample_rate)
                } else {
                    0
                };
                match decoder.decode(&packet) {
                    Ok(decoded) => {
                        let spec = *decoded.spec();
                        let cap = decoded.capacity() as u64;
                        let sbuf = sample_buf.get_or_insert_with(|| SampleBuffer::<f32>::new(cap, spec));
                        if sbuf.capacity() < decoded.frames() * spec.channels.count() {
                            *sbuf = SampleBuffer::<f32>::new(cap, spec);
                        }
                        sbuf.copy_interleaved_ref(decoded);
                        out.extend_from_slice(sbuf.samples());
                        return Ok(Some(ts_ms));
                    }
                    Err(SymphoniaError::DecodeError(_)) => continue,
                    Err(e) => return Err(format!("decode error: {e}")),
                }
            },
            Self::Ffmpeg {
                child,
                sample_rate,
                channels,
                played_frames,
                ..
            } => {
                let Some(stdout) = child.stdout.as_mut() else {
                    return Ok(None);
                };
                let mut raw = [0u8; 4096];
                let n = stdout
                    .read(&mut raw)
                    .map_err(|e| format!("read ffmpeg pcm: {e}"))?;
                if n == 0 {
                    let _ = child.wait();
                    return Ok(None);
                }
                let usable = n - (n % 4);
                for chunk in raw[..usable].as_chunks::<4>().0 {
                    out.push(f32::from_le_bytes(*chunk));
                }
                *played_frames += (out.len() / (*channels).max(1)) as u64;
                let pos_ms = (*played_frames).saturating_mul(1000) / u64::from(*sample_rate);
                Ok(Some(pos_ms))
            }
        }
    }
}

impl Drop for DecoderBackend {
    fn drop(&mut self) {
        if let Self::Ffmpeg { child, .. } = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn engine_loop(
    rx: Receiver<EngineCommand>,
    telemetry: Arc<EngineTelemetry>,
    tap_writer: PcmTapWriter,
) {
    let tap = Arc::new(Mutex::new(tap_writer));
    let mut decoder: Option<DecoderBackend> = None;
    let mut output: Option<ActiveOutput> = None;
    let mut pending_samples: Vec<f32> = Vec::with_capacity(8192);
    let mut pending_offset: usize = 0;

    loop {
        // Handle commands (blocking when idle/paused, non-blocking when playing)
        let cmd = if telemetry.play_state() == PlayState::Playing && decoder.is_some() {
            rx.try_recv().ok()
        } else {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        if let Some(cmd) = cmd {
            match cmd {
                EngineCommand::Shutdown => break,
                EngineCommand::Stop => {
                    decoder = None;
                    output = None;
                    pending_samples.clear();
                    pending_offset = 0;
                    telemetry.set_state(PlayState::Stopped);
                    telemetry.position_ms.store(0, Ordering::Relaxed);
                }
                EngineCommand::Pause => {
                    if telemetry.play_state() == PlayState::Playing {
                        telemetry.set_state(PlayState::Paused);
                    }
                }
                EngineCommand::Resume => {
                    if decoder.is_some() && telemetry.play_state() == PlayState::Paused {
                        telemetry.set_state(PlayState::Playing);
                    }
                }
                EngineCommand::SetVolume(v) => {
                    telemetry.volume_pct.store(u32::from(v.min(100)), Ordering::Relaxed);
                }
                EngineCommand::SetMuted(m) => {
                    telemetry.muted.store(m, Ordering::Relaxed);
                }
                EngineCommand::SeekSecs(secs) => {
                    if let Some(dec) = decoder.as_mut() {
                        if let Err(e) = dec.seek(secs) {
                            telemetry.set_error(e);
                        } else {
                            pending_samples.clear();
                            pending_offset = 0;
                            telemetry
                                .position_ms
                                .store(secs.saturating_mul(1000), Ordering::Relaxed);
                        }
                    }
                }
                EngineCommand::PlayFile {
                    path,
                    duration_secs,
                } => {
                    pending_samples.clear();
                    pending_offset = 0;
                    match DecoderBackend::open(&path, 0) {
                        Ok(dec) => {
                            let sr = dec.sample_rate();
                            let ch = dec.channels();
                            let need_reopen = output.as_ref().is_none_or(|o| {
                                o.sample_rate != sr || o.channels != ch
                            });
                            if need_reopen {
                                output = None;
                                match open_output(
                                    sr,
                                    ch as u16,
                                    Arc::clone(&telemetry),
                                    Arc::clone(&tap),
                                ) {
                                    Ok(out) => output = Some(out),
                                    Err(e) => {
                                        telemetry.set_error(e);
                                        telemetry.set_state(PlayState::Stopped);
                                        continue;
                                    }
                                }
                            }
                            decoder = Some(dec);
                            telemetry.position_ms.store(0, Ordering::Relaxed);
                            telemetry
                                .duration_ms
                                .store(duration_secs.saturating_mul(1000), Ordering::Relaxed);
                            telemetry.set_state(PlayState::Playing);
                        }
                        Err(e) => {
                            telemetry.set_error(e);
                            decoder = None;
                            telemetry.set_state(PlayState::Stopped);
                        }
                    }
                }
            }
        }

        if telemetry.play_state() != PlayState::Playing {
            continue;
        }

        let (Some(dec), Some(out)) = (decoder.as_mut(), output.as_mut()) else {
            continue;
        };

        // Drain any pending samples into the audio ring buffer
        while pending_offset < pending_samples.len() {
            let slots = out.prod.slots();
            if slots == 0 {
                thread::sleep(Duration::from_millis(5));
                break;
            }
            let take = (pending_samples.len() - pending_offset).min(slots);
            for &s in &pending_samples[pending_offset..pending_offset + take] {
                let _ = out.prod.push(s);
            }
            pending_offset += take;
        }

        if pending_offset < pending_samples.len() {
            continue;
        }

        match dec.next_chunk(&mut pending_samples) {
            Ok(Some(pos_ms)) => {
                pending_offset = 0;
                telemetry.position_ms.store(pos_ms, Ordering::Relaxed);
            }
            Ok(None) => {
                // Track finished cleanly
                decoder = None;
                pending_samples.clear();
                pending_offset = 0;
                telemetry.set_state(PlayState::Stopped);
                telemetry.finished_gen.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                telemetry.set_error(e);
                decoder = None;
                pending_samples.clear();
                pending_offset = 0;
                telemetry.set_state(PlayState::Stopped);
            }
        }
    }
}
