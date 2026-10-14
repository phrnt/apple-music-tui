//! Contributor template: copy to src/visualizer/effects/pulse.rs and register.
use crate::visualizer::{
    analysis::{AnalysisFrame, smooth},
    canvas::{Canvas, palette},
    config::Preset,
    effects::Visualization,
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    radius: Option<f32>,
}

pub struct Pulse {
    energy: f32,
    radius: f32,
    colors: [[u8; 3]; 256],
}
impl Default for Pulse {
    fn default() -> Self {
        Self {
            energy: 0.0,
            radius: 0.4,
            colors: palette(&Preset::default().colors),
        }
    }
}
impl Visualization for Pulse {
    fn id(&self) -> &'static str {
        "pulse"
    }
    fn name(&self) -> &'static str {
        "Bass Pulse"
    }
    fn validate(&self, preset: &Preset) -> Result<(), String> {
        let options: Options = preset
            .parameters
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let radius = options.radius.unwrap_or(0.4);
        if !radius.is_finite() || !(0.1..=0.5).contains(&radius) {
            return Err("radius must be 0.1–0.5".into());
        }
        Ok(())
    }
    fn configure(&mut self, preset: &Preset) {
        let options: Options = preset.parameters.clone().try_into().unwrap_or_default();
        self.radius = options.radius.unwrap_or(0.4);
        self.colors = palette(&preset.colors);
        self.reset();
    }
    fn reset(&mut self) {
        self.energy = 0.0;
    }
    fn update(&mut self, audio: &AnalysisFrame, dt: Duration) {
        self.energy = smooth(self.energy, audio.bass, dt, 120.0);
    }
    fn render(&mut self, canvas: &mut Canvas) {
        canvas.clear();
        let radius =
            self.radius * canvas.width.min(canvas.height) as f32 * (0.5 + self.energy * 0.5);
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                let distance = ((x as f32 - canvas.width as f32 * 0.5).powi(2)
                    + (y as f32 - canvas.height as f32 * 0.5).powi(2))
                .sqrt();
                if (distance - radius).abs() < 1.5 {
                    canvas.set(x, y, self.colors[(self.energy * 255.0) as usize]);
                }
            }
        }
    }
}
