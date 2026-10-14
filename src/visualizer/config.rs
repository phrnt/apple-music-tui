use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBackend {
    #[default]
    CoreAudio,
    ScreenCaptureKit,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Bass,
    Mid,
    Treble,
    Beat,
    Loudness,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Mapping {
    pub source: Signal,
    pub amount: f32,
    pub smoothing_ms: u16,
}
impl Default for Mapping {
    fn default() -> Self {
        Self {
            source: Signal::Bass,
            amount: 0.4,
            smoothing_ms: 180,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub effect: String,
    pub colors: Vec<[u8; 3]>,
    pub sensitivity: f32,
    pub hue: Mapping,
    pub brightness: Mapping,
    pub zoom: f64,
    pub center_x: f64,
    pub center_y: f64,
    pub iterations: u16,
    /// Module-owned options, deserialized and validated by the effect itself.
    pub parameters: toml::Table,
}
impl Default for Preset {
    fn default() -> Self {
        Self {
            name: "Neon".into(),
            effect: "spectrum".into(),
            colors: vec![
                [26, 220, 192],
                [59, 108, 255],
                [206, 55, 217],
                [255, 191, 104],
            ],
            sensitivity: 1.0,
            hue: Mapping::default(),
            brightness: Mapping {
                source: Signal::Treble,
                amount: 0.45,
                smoothing_ms: 90,
            },
            zoom: 1.0,
            center_x: -0.7,
            center_y: 0.0,
            iterations: 128,
            parameters: toml::Table::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub backend: CaptureBackend,
    pub framerate: u16,
    pub initial_preset: String,
    pub presets: Vec<Preset>,
}
impl Default for Settings {
    fn default() -> Self {
        let neon = Preset::default();
        let ember = Preset {
            name: "Ember Fractal".into(),
            effect: "mandelbrot".into(),
            colors: vec![
                [43, 13, 66],
                [192, 36, 108],
                [255, 126, 39],
                [255, 235, 156],
            ],
            ..Preset::default()
        };
        let ocean = Preset {
            name: "Ocean Scope".into(),
            effect: "waveform".into(),
            colors: vec![[11, 34, 69], [27, 171, 209], [135, 250, 211]],
            ..Preset::default()
        };
        Self {
            backend: CaptureBackend::CoreAudio,
            framerate: 30,
            initial_preset: neon.name.clone(),
            presets: vec![neon, ember, ocean],
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self, String> {
        let value = match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map_err(|e| format!("Invalid visualizer settings at {}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(10..=60).contains(&self.framerate) {
            return Err("Visualizer framerate must be 10–60.".into());
        }
        if self.presets.is_empty() || self.presets.len() > 64 {
            return Err("Provide 1–64 visualizer presets.".into());
        }
        let mut names = std::collections::HashSet::new();
        for p in &self.presets {
            if p.name.trim().is_empty() || p.name.len() > 80 || !names.insert(p.name.clone()) {
                return Err("Preset names must be nonempty, unique and at most 80 bytes.".into());
            }
            if !(2..=16).contains(&p.colors.len()) {
                return Err(format!("Preset '{}' needs 2–16 RGB colors.", p.name));
            }
            if !p.sensitivity.is_finite() || !(0.1..=5.0).contains(&p.sensitivity) {
                return Err("Sensitivity must be 0.1–5.0.".into());
            }
            for m in [&p.hue, &p.brightness] {
                if !m.amount.is_finite()
                    || !(0.0..=1.0).contains(&m.amount)
                    || m.smoothing_ms > 3000
                {
                    return Err("Mapping amount must be 0–1 and smoothing_ms 0–3000.".into());
                }
            }
            if !p.zoom.is_finite()
                || !(0.1..=100_000.0).contains(&p.zoom)
                || !p.center_x.is_finite()
                || !p.center_y.is_finite()
                || p.center_x.abs() > 4.0
                || p.center_y.abs() > 4.0
                || !(16..=256).contains(&p.iterations)
            {
                return Err(
                    "Invalid fractal center, zoom (0.1–100000) or iterations (16–256).".into(),
                );
            }
        }
        if !names.contains(&self.initial_preset) {
            return Err("initial_preset must name one of the configured presets.".into());
        }
        Ok(())
    }
}

pub fn settings_path() -> Result<PathBuf, String> {
    crate::config::default_config_path()
        .map(|p| p.with_file_name("visualizer.toml"))
        .map_err(|e| e.to_string())
}

#[derive(Default, Deserialize, Serialize)]
pub struct Selection {
    pub effect: String,
    pub preset: String,
}

pub fn load_selection(path: &Path) -> Selection {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}
pub fn save_selection(path: &Path, selection: &Selection) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string(selection).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, text).map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contributor_presets_round_trip_and_reject_unbounded_work() {
        let settings = Settings::default();
        let text = toml::to_string(&settings).unwrap();
        let parsed: Settings = toml::from_str(&text).unwrap();
        assert_eq!(parsed, settings);
        parsed.validate().unwrap();
        let mut invalid = parsed;
        invalid.presets[0].iterations = 65535;
        assert!(invalid.validate().is_err());
        invalid.presets[0].iterations = 128;
        invalid.presets[0].sensitivity = f32::NAN;
        assert!(invalid.validate().is_err());
    }
}
