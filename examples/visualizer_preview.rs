//! Explicit developer preview using a generated signal, never a live fallback.
use apple_music_tui::visualizer::{
    analysis::{Analyzer, FFT_SIZE},
    canvas::Canvas,
    config::Settings,
    effects::registry,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/visualizer-preview"));
    std::fs::create_dir_all(&directory)?;
    let mut analyzer = Analyzer::new(48000.0);
    for i in 0..FFT_SIZE {
        let t = i as f32 / 48000.0;
        let left = (std::f32::consts::TAU * 80.0 * t).sin() * 0.22
            + (std::f32::consts::TAU * 700.0 * t).sin() * 0.1
            + (std::f32::consts::TAU * 8000.0 * t).sin() * 0.04;
        analyzer.push(left, -left * 0.8);
    }
    let audio = analyzer.analyze(Duration::from_secs(1));
    for (mut effect, preset) in registry().into_iter().zip(Settings::default().presets) {
        effect.configure(&preset);
        effect.update(&audio, Duration::from_secs(1));
        let mut canvas = Canvas::default();
        canvas.resize(160, 50);
        let geometry = Instant::now();
        effect.render(&mut canvas);
        let first_ms = geometry.elapsed().as_secs_f64() * 1000.0;
        let path = directory.join(format!("{}-generated-preview.png", effect.id()));
        let bytes: Vec<u8> = canvas.pixels().iter().flatten().copied().collect();
        let image = image::RgbImage::from_raw(canvas.width as u32, canvas.height as u32, bytes)
            .ok_or("image size")?;
        image::imageops::resize(
            &image,
            image.width() * 4,
            image.height() * 4,
            image::imageops::FilterType::Nearest,
        )
        .save(&path)?;
        let start = Instant::now();
        for _ in 0..300 {
            effect.update(&audio, Duration::from_secs_f64(1.0 / 30.0));
            effect.render(&mut canvas);
        }
        println!(
            "{}: first={first_ms:.3}ms, cached mean={:.3}ms; {}",
            effect.name(),
            start.elapsed().as_secs_f64() * 1000.0 / 300.0,
            path.display()
        );
    }
    Ok(())
}
