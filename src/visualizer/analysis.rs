use super::config::Signal;
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub const BANDS: usize = 64;
pub const FFT_SIZE: usize = 4096;
pub const WAVE_POINTS: usize = 256;

#[derive(Clone, Debug)]
pub struct AnalysisFrame {
    pub bands: [f32; BANDS],
    pub frequencies: [f32; BANDS],
    pub left: [f32; WAVE_POINTS],
    pub right: [f32; WAVE_POINTS],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat: f32,
    pub loudness: f32,
    pub captured_at: Instant,
    pub sequence: u64,
}
impl Default for AnalysisFrame {
    fn default() -> Self {
        Self {
            bands: [0.0; BANDS],
            frequencies: [0.0; BANDS],
            left: [0.0; WAVE_POINTS],
            right: [0.0; WAVE_POINTS],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            beat: 0.0,
            loudness: 0.0,
            captured_at: Instant::now(),
            sequence: 0,
        }
    }
}
impl AnalysisFrame {
    pub fn signal(&self, signal: Signal) -> f32 {
        match signal {
            Signal::Bass => self.bass,
            Signal::Mid => self.mid,
            Signal::Treble => self.treble,
            Signal::Beat => self.beat,
            Signal::Loudness => self.loudness,
        }
    }
}

pub fn smooth(previous: f32, target: f32, elapsed: Duration, milliseconds: f32) -> f32 {
    if milliseconds <= 0.0 {
        return target;
    }
    let alpha = 1.0 - (-elapsed.as_secs_f32() * 1000.0 / milliseconds).exp();
    previous + alpha * (target - previous)
}

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    work: Vec<Complex<f32>>,
    window: Vec<f32>,
    ring: Vec<[f32; 2]>,
    power: Vec<f32>,
    edges: [(usize, usize); BANDS],
    write: usize,
    samples: usize,
    scale: f32,
    rate: f32,
    frame: AnalysisFrame,
    bass_average: f32,
}
impl Analyzer {
    pub fn new(rate: f32) -> Self {
        let rate = if rate.is_finite() && rate >= 8000.0 {
            rate
        } else {
            48000.0
        };
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let window: Vec<_> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        let scale = (2.0 / window.iter().sum::<f32>()).powi(2) / 2.0;
        let mut frame = AnalysisFrame::default();
        let high = 16000.0_f32.min(rate / 2.0);
        let mut edges = [(0, 0); BANDS];
        for (i, edge) in edges.iter_mut().enumerate() {
            let lo = 30.0 * (high / 30.0).powf(i as f32 / BANDS as f32);
            let hi = 30.0 * (high / 30.0).powf((i + 1) as f32 / BANDS as f32);
            frame.frequencies[i] = (lo * hi).sqrt();
            let start = (lo * FFT_SIZE as f32 / rate).floor() as usize;
            let end = (hi * FFT_SIZE as f32 / rate).ceil() as usize;
            *edge = (
                start.clamp(1, FFT_SIZE / 2 - 1),
                end.max(start + 1).min(FFT_SIZE / 2),
            );
        }
        Self {
            fft,
            scratch,
            work: vec![Complex::default(); FFT_SIZE],
            window,
            ring: vec![[0.0; 2]; FFT_SIZE],
            power: vec![0.0; FFT_SIZE / 2],
            edges,
            write: 0,
            samples: 0,
            scale,
            rate,
            frame,
            bass_average: 0.0,
        }
    }
    pub fn push(&mut self, left: f32, right: f32) {
        if !left.is_finite() || !right.is_finite() {
            return;
        }
        self.ring[self.write] = [left, right];
        self.write = (self.write + 1) % FFT_SIZE;
        self.samples = self.samples.saturating_add(1);
    }
    pub fn ready(&self) -> bool {
        self.samples >= FFT_SIZE
    }
    pub fn analyze(&mut self, elapsed: Duration) -> AnalysisFrame {
        self.power.fill(0.0);
        for channel in 0..2 {
            for (i, value) in self.work.iter_mut().enumerate() {
                *value = Complex::new(
                    self.ring[(self.write + i) % FFT_SIZE][channel] * self.window[i],
                    0.0,
                );
            }
            self.fft
                .process_with_scratch(&mut self.work, &mut self.scratch);
            for (power, value) in self.power.iter_mut().zip(&self.work) {
                *power += value.norm_sqr() * self.scale;
            }
        }
        for (i, &(lo, hi)) in self.edges.iter().enumerate() {
            let peak = self.power[lo..hi].iter().copied().fold(0.0_f32, f32::max);
            let target = ((10.0 * (peak + 1e-12).log10() + 75.0) / 60.0).clamp(0.0, 1.0);
            let previous = self.frame.bands[i];
            self.frame.bands[i] = smooth(
                previous,
                target,
                elapsed,
                if target > previous { 35.0 } else { 190.0 },
            );
        }
        let region = |lo: f32, hi: f32| {
            let mut total = 0.0;
            let mut n = 0;
            for (value, hz) in self.frame.bands.iter().zip(self.frame.frequencies) {
                if hz >= lo && hz < hi {
                    total += value;
                    n += 1;
                }
            }
            if n == 0 { 0.0 } else { total / n as f32 }
        };
        self.frame.bass = region(30.0, 250.0);
        self.frame.mid = region(250.0, 4000.0);
        self.frame.treble = region(4000.0, 16001.0);
        let bass_end = (250.0 * FFT_SIZE as f32 / self.rate).ceil() as usize;
        let bass_power = self.power[1..bass_end.min(self.power.len())]
            .iter()
            .sum::<f32>();
        let onset = if bass_power > 1e-7 && self.bass_average > 1e-7 {
            ((bass_power / self.bass_average - 1.4) / 3.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.bass_average = smooth(self.bass_average, bass_power, elapsed, 700.0);
        self.frame.beat = onset.max(self.frame.beat * (-elapsed.as_secs_f32() / 0.18).exp());
        let rms = (self
            .ring
            .iter()
            .map(|v| (v[0].powi(2) + v[1].powi(2)) * 0.5)
            .sum::<f32>()
            / FFT_SIZE as f32)
            .sqrt();
        self.frame.loudness = (rms * 4.0).clamp(0.0, 1.0);
        for i in 0..WAVE_POINTS {
            let frame = self.ring[(self.write + i * FFT_SIZE / WAVE_POINTS) % FFT_SIZE];
            self.frame.left[i] = frame[0].clamp(-1.0, 1.0);
            self.frame.right[i] = frame[1].clamp(-1.0, 1.0);
        }
        self.frame.captured_at = Instant::now();
        self.frame.sequence += 1;
        self.frame.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tone_is_located_and_opposite_phase_stereo_does_not_cancel() {
        let mut analyzer = Analyzer::new(48000.0);
        for i in 0..FFT_SIZE {
            let sample = (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin() * 0.25;
            analyzer.push(sample, -sample);
        }
        let frame = analyzer.analyze(Duration::from_secs(1));
        let peak = frame
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((frame.frequencies[peak] - 1000.0).abs() < 150.0);
        assert!(frame.bands[peak] > 0.5 && frame.loudness > 0.1);
        assert!(
            frame
                .left
                .iter()
                .zip(frame.right)
                .any(|(l, r)| *l > 0.1 && r < -0.1)
        );
    }
    #[test]
    fn silence_is_finite_and_smoothing_is_time_based() {
        let mut analyzer = Analyzer::new(48000.0);
        let frame = analyzer.analyze(Duration::from_secs(1));
        assert!(frame.bands.iter().all(|v| *v == 0.0));
        assert_eq!(frame.beat, 0.0);
        let one = smooth(0.0, 1.0, Duration::from_millis(100), 180.0);
        let two = smooth(
            smooth(0.0, 1.0, Duration::from_millis(50), 180.0),
            1.0,
            Duration::from_millis(50),
            180.0,
        );
        assert!((one - two).abs() < 1e-6);
    }
}
