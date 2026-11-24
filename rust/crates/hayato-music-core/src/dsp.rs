//! Real-time DSP pipeline: lock-free PCM tap, 1024-pt Hann-windowed `realfft`
//! 32-band logarithmic spectrum analyzer, peak-hold caps, oscilloscope decimator,
//! and spectral-flux bass beat detector.
//!
//! Zero heap allocations on the audio callback thread and zero heap allocations
//! per UI frame once constructed.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use rtrb::{Consumer, Producer, RingBuffer};

pub const FFT_SIZE: usize = 1024;
pub const BANDS: usize = 32;
pub const WAVE_POINTS: usize = 64;
const RING_CAP: usize = 8192;

/// Lock-free producer held by the audio output callback.
pub struct PcmTapWriter {
    prod: Producer<f32>,
}

impl PcmTapWriter {
    /// Push mono-summed samples without ever blocking or allocating.
    #[inline]
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        let ch = channels.max(1);
        for frame in samples.chunks(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s;
            }
            let mono = sum / ch as f32;
            if self.prod.push(mono).is_err() {
                break;
            }
        }
    }
}

/// Create a matched lock-free `(PcmTapWriter, SpectrumAnalyzer)` pair.
pub fn create_tap() -> (PcmTapWriter, SpectrumAnalyzer) {
    let (prod, cons) = RingBuffer::new(RING_CAP);
    (PcmTapWriter { prod }, SpectrumAnalyzer::new(cons))
}

/// Pre-allocated FFT spectrum + oscilloscope + beat detector owned by the UI thread.
pub struct SpectrumAnalyzer {
    cons: Consumer<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: [f32; FFT_SIZE],
    ring: [f32; FFT_SIZE],
    ring_pos: usize,
    input: [f32; FFT_SIZE],
    output: Vec<Complex32>,
    scratch: Vec<Complex32>,
    prev_bass: [f32; 16],
    flux_history: [f32; 24],
    flux_idx: usize,
    /// Smoothed normalized band magnitudes (`0.0..=1.0`).
    pub bands: [f32; BANDS],
    /// Falling peak-hold caps (`0.0..=1.0`).
    pub peaks: [f32; BANDS],
    /// Decimated waveform points (`-1.0..=1.0`) for oscilloscope mode.
    pub wave: [f32; WAVE_POINTS],
    /// Beat-pulse intensity (`0.0..=1.0`), decaying smoothly between kicks.
    pub beat_intensity: f32,
}

impl SpectrumAnalyzer {
    pub fn new(cons: Consumer<f32>) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let window = std::array::from_fn(|i| {
            0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        });
        Self {
            cons,
            fft,
            window,
            ring: [0.0; FFT_SIZE],
            ring_pos: 0,
            input: [0.0; FFT_SIZE],
            output,
            scratch,
            prev_bass: [0.0; 16],
            flux_history: [0.0; 24],
            flux_idx: 0,
            bands: [0.0; BANDS],
            peaks: [0.0; BANDS],
            wave: [0.0; WAVE_POINTS],
            beat_intensity: 0.0,
        }
    }

    /// Drain incoming PCM samples and update `bands`, `peaks`, `wave`, and `beat_intensity`.
    /// When `paused` is true, smoothly decays bars to zero without running FFT.
    pub fn update(&mut self, paused: bool) {
        if paused {
            while self.cons.pop().is_ok() {}
            for i in 0..BANDS {
                self.bands[i] *= 0.80;
                self.peaks[i] = (self.peaks[i] - 0.025).max(self.bands[i]);
            }
            for w in &mut self.wave {
                *w *= 0.80;
            }
            self.beat_intensity *= 0.78;
            return;
        }

        let mut received = 0usize;
        while let Ok(sample) = self.cons.pop() {
            self.ring[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % FFT_SIZE;
            received += 1;
        }

        if received == 0 {
            for i in 0..BANDS {
                self.bands[i] *= 0.86;
                self.peaks[i] = (self.peaks[i] - 0.02).max(self.bands[i]);
            }
            self.beat_intensity *= 0.82;
            return;
        }

        // Copy chronological samples into windowed FFT input + decimated waveform
        let stride = FFT_SIZE / WAVE_POINTS;
        for i in 0..FFT_SIZE {
            let s = self.ring[(self.ring_pos + i) % FFT_SIZE];
            self.input[i] = s * self.window[i];
            if i % stride == 0 {
                let wi = i / stride;
                if wi < WAVE_POINTS {
                    self.wave[wi] = s.clamp(-1.0, 1.0);
                }
            }
        }

        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }

        let bins = self.output.len().saturating_sub(1).max(1);
        // Logarithmic mapping from bin 1..bins across 32 bands
        let min_bin = 1.0f32;
        let max_bin = (bins as f32).min(420.0);
        let ratio = (max_bin / min_bin).powf(1.0 / BANDS as f32);

        for b in 0..BANDS {
            let start = (min_bin * ratio.powi(b as i32)).floor() as usize;
            let end = ((min_bin * ratio.powi((b + 1) as i32)).ceil() as usize)
                .max(start + 1)
                .min(bins);
            let mut sum = 0.0f32;
            for k in start..end {
                let c = self.output[k];
                sum += (c.re * c.re + c.im * c.im).sqrt();
            }
            let avg = sum / (end - start) as f32;
            // dB-like perceptual compression tuned for music
            let norm = ((avg / 14.0).ln_1p() / 2.2).clamp(0.0, 1.0);
            // Fast attack, smooth release
            if norm > self.bands[b] {
                self.bands[b] = self.bands[b] * 0.35 + norm * 0.65;
            } else {
                self.bands[b] = self.bands[b] * 0.78 + norm * 0.22;
            }
            if self.bands[b] >= self.peaks[b] {
                self.peaks[b] = self.bands[b];
            } else {
                self.peaks[b] = (self.peaks[b] - 0.018).max(self.bands[b]);
            }
        }

        // Spectral flux across sub-bass/bass bins (bins 1..16 ~ 43 Hz .. 680 Hz at 44.1kHz)
        let mut flux = 0.0f32;
        for k in 1..16.min(self.output.len()) {
            let c = self.output[k];
            let mag = (c.re * c.re + c.im * c.im).sqrt();
            let diff = (mag - self.prev_bass[k]).max(0.0);
            flux += diff;
            self.prev_bass[k] = mag;
        }
        let mean: f32 = self.flux_history.iter().sum::<f32>() / self.flux_history.len() as f32;
        self.flux_history[self.flux_idx] = flux;
        self.flux_idx = (self.flux_idx + 1) % self.flux_history.len();

        if mean > 0.5 && flux > mean * 1.45 {
            self.beat_intensity = 1.0;
        } else {
            self.beat_intensity *= 0.80;
        }
    }

    /// Generate a subtle synthetic idle wave when remotely controlling another
    /// Spotify Connect speaker (no local PCM available).
    pub fn update_synthetic(&mut self, phase: f32, playing: bool) {
        if !playing {
            self.update(true);
            return;
        }
        for i in 0..BANDS {
            let x = i as f32 * 0.35 + phase;
            let v = ((x.sin() * 0.5 + 0.5) * 0.45 + ((x * 0.43).cos() * 0.5 + 0.5) * 0.25)
                * (1.0 - (i as f32 / BANDS as f32) * 0.45);
            self.bands[i] = v.clamp(0.05, 0.85);
            self.peaks[i] = self.peaks[i].max(self.bands[i]);
        }
        for i in 0..WAVE_POINTS {
            self.wave[i] = ((i as f32 * 0.25 + phase * 1.5).sin() * 0.55).clamp(-1.0, 1.0);
        }
        self.beat_intensity = ((phase * 2.0).sin() * 0.5 + 0.5) * 0.5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_wave_activates_expected_frequency_band() {
        let (mut tx, mut rx) = create_tap();
        // 440 Hz sine wave at 44100 Hz
        let samples: Vec<f32> = (0..FFT_SIZE * 2)
            .map(|i| (2.0 * PI * 440.0 * i as f32 / 44100.0).sin() * 0.9)
            .collect();
        tx.push_interleaved(&samples, 1);
        rx.update(false);
        let peak_band = rx
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            (6..=16).contains(&peak_band),
            "440 Hz should land in lower-mid bands, got {peak_band}"
        );
        assert!(rx.bands[peak_band] > 0.2);
    }

    #[test]
    fn paused_update_decays_bars() {
        let (mut tx, mut rx) = create_tap();
        let samples = vec![0.8f32; FFT_SIZE * 2];
        tx.push_interleaved(&samples, 1);
        rx.update(false);
        let before: f32 = rx.bands.iter().sum();
        for _ in 0..20 {
            rx.update(true);
        }
        let after: f32 = rx.bands.iter().sum();
        assert!(after < before * 0.1 || before == 0.0);
    }
}
