//! Opt-in real-audio smoke test of the production worker and teardown.
//! Writes measurements only. No playback commands or captured audio files.
use apple_music_tui::visualizer::{
    config::CaptureBackend,
    service::{AudioService, CaptureStatus},
};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = if std::env::args().nth(1).as_deref() == Some("sck") {
        CaptureBackend::ScreenCaptureKit
    } else {
        CaptureBackend::CoreAudio
    };
    let mut audio = AudioService::new(backend, true);
    audio.set_enabled(true);
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut live_frames = 0;
    let mut last_sequence = 0;
    let mut maximum_analysis_us = 0;
    while Instant::now() < deadline {
        let frame = audio.snapshot();
        if let CaptureStatus::Unavailable(message) = frame.status {
            audio.finish().await;
            return Err(message.into());
        }
        if frame.status == CaptureStatus::Live && frame.audio.sequence != last_sequence {
            live_frames += 1;
            last_sequence = frame.audio.sequence;
            maximum_analysis_us = maximum_analysis_us.max(frame.analysis_us);
        }
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    let snapshot = audio.snapshot();
    audio.set_enabled(false);
    tokio::time::sleep(Duration::from_millis(250)).await;
    if audio.snapshot().status != CaptureStatus::Disabled {
        audio.finish().await;
        return Err("A stopped worker published stale capture data.".into());
    }
    audio.set_enabled(true);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut restarted = false;
    while Instant::now() < deadline {
        if audio.snapshot().status == CaptureStatus::Live {
            restarted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    audio.set_enabled(false);
    audio.finish().await;
    println!(
        "{}",
        serde_json::json!({"backend":format!("{backend:?}"),"live_frames":live_frames,"dropped_frames":snapshot.dropped_frames,"max_analysis_us":maximum_analysis_us,"last_bass":snapshot.audio.bass,"last_mid":snapshot.audio.mid,"last_treble":snapshot.audio.treble,"capture_restarted":restarted})
    );
    if live_frames == 0 {
        return Err("No nonzero audio received; check local playback and permission.".into());
    }
    if !restarted {
        return Err("Capture did not restart after disabling the view.".into());
    }
    Ok(())
}
