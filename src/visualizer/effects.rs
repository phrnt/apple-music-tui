use super::{
    analysis::{AnalysisFrame, BANDS, WAVE_POINTS, smooth},
    canvas::{Canvas, brightness, palette},
    config::Preset,
};
use std::time::Duration;

/// Contribution boundary: no capture, playback, filesystem or terminal APIs.
pub trait Visualization: Send {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn validate(&self, preset: &Preset) -> Result<(), String> {
        if !preset.parameters.is_empty() {
            return Err(format!(
                "{} has no module-specific parameters.",
                self.name()
            ));
        }
        Ok(())
    }
    fn configure(&mut self, preset: &Preset);
    fn reset(&mut self);
    fn update(&mut self, audio: &AnalysisFrame, elapsed: Duration);
    fn render(&mut self, canvas: &mut Canvas);
}

/// Add a boxed module here; switching, capture and presets need no effect code.
pub fn registry() -> Vec<Box<dyn Visualization>> {
    vec![
        Box::new(Spectrum::default()),
        Box::new(Mandelbrot::default()),
        Box::new(Waveform::default()),
    ]
}

struct Spectrum {
    values: [f32; BANDS],
    peaks: [f32; BANDS],
    colors: Vec<[u8; 3]>,
    sensitivity: f32,
}
impl Default for Spectrum {
    fn default() -> Self {
        Self {
            values: [0.0; BANDS],
            peaks: [0.0; BANDS],
            colors: Preset::default().colors,
            sensitivity: 1.0,
        }
    }
}
impl Visualization for Spectrum {
    fn id(&self) -> &'static str {
        "spectrum"
    }
    fn name(&self) -> &'static str {
        "Spectrum"
    }
    fn configure(&mut self, p: &Preset) {
        self.colors = p.colors.clone();
        self.sensitivity = p.sensitivity;
        self.reset();
    }
    fn reset(&mut self) {
        self.values.fill(0.0);
        self.peaks.fill(0.0);
    }
    fn update(&mut self, audio: &AnalysisFrame, dt: Duration) {
        for i in 0..BANDS {
            self.values[i] = (audio.bands[i] * self.sensitivity).clamp(0.0, 1.0);
            self.peaks[i] = self.values[i].max(self.peaks[i] - dt.as_secs_f32() * 0.45);
        }
    }
    fn render(&mut self, canvas: &mut Canvas) {
        canvas.clear();
        if canvas.width == 0 || canvas.height == 0 {
            return;
        }
        let colors = palette(&self.colors);
        let bars = (canvas.width / 3).clamp(1, BANDS);
        for b in 0..bars {
            let lo = b * BANDS / bars;
            let hi = ((b + 1) * BANDS / bars).max(lo + 1);
            let value = self.values[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
            let peak = self.peaks[lo..hi].iter().copied().fold(0.0, f32::max);
            let height = (value * canvas.height as f32).round() as usize;
            let x0 = b * canvas.width / bars;
            let x1 = ((b + 1) * canvas.width / bars)
                .saturating_sub(1)
                .max(x0 + 1);
            for x in x0..x1 {
                for y in 0..height.min(canvas.height) {
                    canvas.set(
                        x,
                        canvas.height - 1 - y,
                        colors[(y * 255 / canvas.height).min(255)],
                    );
                }
                if peak > 0.01 {
                    canvas.set(
                        x,
                        canvas.height - 1 - (peak * (canvas.height - 1) as f32) as usize,
                        [210, 229, 255],
                    );
                }
            }
        }
    }
}

pub struct Mandelbrot {
    preset: Preset,
    colors: [[u8; 3]; 256],
    geometry: Vec<u16>,
    dimensions: (usize, usize),
    hue: f32,
    light: f32,
    beat: f32,
    pub geometry_builds: usize,
}
impl Default for Mandelbrot {
    fn default() -> Self {
        let p = Preset::default();
        Self {
            colors: palette(&p.colors),
            preset: p,
            geometry: Vec::new(),
            dimensions: (0, 0),
            hue: 0.0,
            light: 0.0,
            beat: 0.0,
            geometry_builds: 0,
        }
    }
}
impl Mandelbrot {
    fn build_geometry(&mut self, width: usize, height: usize) {
        self.geometry.resize(width * height, 0);
        self.dimensions = (width, height);
        self.geometry_builds += 1;
        let span = 3.2 / self.preset.zoom;
        for y in 0..height {
            for x in 0..width {
                let cr = self.preset.center_x + (x as f64 / width.max(1) as f64 - 0.5) * span;
                let ci = self.preset.center_y
                    + (y as f64 / height.max(1) as f64 - 0.5) * span * height as f64
                        / width.max(1) as f64;
                let (mut zr, mut zi) = (0.0_f64, 0.0_f64);
                let mut n = 0;
                while zr * zr + zi * zi <= 4.0 && n < self.preset.iterations {
                    (zr, zi) = (zr * zr - zi * zi + cr, 2.0 * zr * zi + ci);
                    n += 1;
                }
                self.geometry[y * width + x] = if n == self.preset.iterations {
                    0
                } else {
                    // Smooth escape coloring is computed once, not every frame.
                    let value = n as f64 + 1.0
                        - (zr * zr + zi * zi).sqrt().ln().ln() / std::f64::consts::LN_2;
                    (value.max(0.0) * 9.0) as u16 + 1
                };
            }
        }
    }
}
impl Visualization for Mandelbrot {
    fn id(&self) -> &'static str {
        "mandelbrot"
    }
    fn name(&self) -> &'static str {
        "Mandelbrot"
    }
    fn configure(&mut self, p: &Preset) {
        if p.zoom != self.preset.zoom
            || p.center_x != self.preset.center_x
            || p.center_y != self.preset.center_y
            || p.iterations != self.preset.iterations
        {
            self.dimensions = (0, 0);
        }
        self.preset = p.clone();
        self.colors = palette(&p.colors);
        self.reset();
    }
    fn reset(&mut self) {
        self.hue = 0.0;
        self.light = 0.0;
        self.beat = 0.0;
    }
    fn update(&mut self, a: &AnalysisFrame, dt: Duration) {
        self.hue = smooth(
            self.hue,
            (a.signal(self.preset.hue.source) * self.preset.sensitivity).clamp(0.0, 1.0),
            dt,
            self.preset.hue.smoothing_ms as f32,
        );
        self.light = smooth(
            self.light,
            (a.signal(self.preset.brightness.source) * self.preset.sensitivity).clamp(0.0, 1.0),
            dt,
            self.preset.brightness.smoothing_ms as f32,
        );
        self.beat = a.beat;
    }
    fn render(&mut self, c: &mut Canvas) {
        if self.dimensions != (c.width, c.height) {
            self.build_geometry(c.width, c.height);
        }
        // Small spectral fluctuations otherwise repaint most terminal cells.
        // Retain smooth audio state but use bounded palette/light steps for
        // transport, so Ratatui can skip visually insignificant color changes.
        let shift = ((self.hue * self.preset.hue.amount * 255.0 / 2.0).round() as usize) * 2;
        let light = ((0.55 + self.light * self.preset.brightness.amount + self.beat * 0.15) * 64.0)
            .round()
            / 64.0;
        let colors = self.colors.map(|color| brightness(color, light));
        for (i, &n) in self.geometry.iter().enumerate() {
            let color = if n == 0 {
                [2, 3, 10]
            } else {
                colors[(n as usize + shift) % 256]
            };
            c.set(i % c.width, i / c.width, color);
        }
    }
}

struct Waveform {
    left: [f32; WAVE_POINTS],
    right: [f32; WAVE_POINTS],
    colors: Vec<[u8; 3]>,
    sensitivity: f32,
}
impl Default for Waveform {
    fn default() -> Self {
        Self {
            left: [0.0; WAVE_POINTS],
            right: [0.0; WAVE_POINTS],
            colors: Preset::default().colors,
            sensitivity: 1.0,
        }
    }
}
impl Visualization for Waveform {
    fn id(&self) -> &'static str {
        "waveform"
    }
    fn name(&self) -> &'static str {
        "Stereo Waveform"
    }
    fn configure(&mut self, p: &Preset) {
        self.colors = p.colors.clone();
        self.sensitivity = p.sensitivity;
        self.reset();
    }
    fn reset(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
    }
    fn update(&mut self, a: &AnalysisFrame, _dt: Duration) {
        self.left = a.left;
        self.right = a.right;
    }
    fn render(&mut self, c: &mut Canvas) {
        c.clear();
        if c.width == 0 || c.height < 2 {
            return;
        }
        let colors = palette(&self.colors);
        let half = c.height / 2;
        for (channel, wave) in [&self.left, &self.right].into_iter().enumerate() {
            let mut previous = half / 2 + channel * half;
            for x in 0..c.width {
                let i = x * (WAVE_POINTS - 1) / c.width.max(1);
                let y = (((1.0 - (wave[i] * self.sensitivity).clamp(-1.0, 1.0))
                    * 0.5
                    * (half - 1) as f32) as usize)
                    + channel * half;
                for py in previous.min(y)..=previous.max(y) {
                    c.set(x, py, colors[(x * 255 / c.width).min(255)]);
                }
                previous = y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mandelbrot_recolors_from_audio_without_rebuilding_geometry() {
        let mut effect = Mandelbrot::default();
        let mut canvas = Canvas::default();
        canvas.resize(80, 24);
        effect.render(&mut canvas);
        let before = canvas.pixels().to_vec();
        assert_eq!(effect.geometry_builds, 1);
        let audio = AnalysisFrame {
            bass: 1.0,
            treble: 1.0,
            ..AnalysisFrame::default()
        };
        effect.update(&audio, Duration::from_secs(1));
        effect.render(&mut canvas);
        assert_ne!(before, canvas.pixels());
        assert_eq!(effect.geometry_builds, 1);
        canvas.resize(100, 24);
        effect.render(&mut canvas);
        assert_eq!(effect.geometry_builds, 2);
    }

    #[test]
    fn fractal_palette_avoids_repainting_for_small_audio_fluctuations() {
        let mut effect = Mandelbrot::default();
        let mut preset = Preset::default();
        preset.hue.smoothing_ms = 0;
        preset.brightness.smoothing_ms = 0;
        effect.configure(&preset);
        let mut canvas = Canvas::default();
        canvas.resize(80, 24);
        effect.update(
            &AnalysisFrame {
                bass: 0.5,
                treble: 0.5,
                ..AnalysisFrame::default()
            },
            Duration::from_millis(33),
        );
        effect.render(&mut canvas);
        let before = canvas.pixels().to_vec();
        effect.update(
            &AnalysisFrame {
                bass: 0.503,
                treble: 0.503,
                ..AnalysisFrame::default()
            },
            Duration::from_millis(33),
        );
        effect.render(&mut canvas);
        assert_eq!(before, canvas.pixels());
    }
}
