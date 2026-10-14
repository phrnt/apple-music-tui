use super::{analysis::AnalysisFrame, config::CaptureBackend};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CaptureStatus {
    #[default]
    Disabled,
    Initializing,
    Live,
    Silent,
    Unavailable(String),
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: CaptureStatus,
    pub audio: AnalysisFrame,
    pub dropped_frames: u64,
    pub analysis_us: u64,
}
struct Control {
    enabled: AtomicBool,
    quit: AtomicBool,
    generation: AtomicU64,
    latest: Mutex<Snapshot>,
}

/// One lazy worker per controller. Changing effects never creates a new tap.
/// Pending native permission calls stay on this worker, never in the TUI loop.
pub struct AudioService {
    control: Arc<Control>,
    worker: Option<JoinHandle<()>>,
    backend: CaptureBackend,
    allowed: bool,
}
impl AudioService {
    pub fn new(backend: CaptureBackend, allowed: bool) -> Self {
        Self {
            control: Arc::new(Control {
                enabled: AtomicBool::new(false),
                quit: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                latest: Mutex::new(Snapshot::default()),
            }),
            worker: None,
            backend,
            allowed,
        }
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        let was = self.control.enabled.swap(enabled, Ordering::AcqRel);
        if was != enabled {
            self.control.generation.fetch_add(1, Ordering::AcqRel);
            if enabled && self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                self.worker.take();
            }
            publish(
                &self.control,
                Snapshot {
                    status: if enabled {
                        CaptureStatus::Initializing
                    } else {
                        CaptureStatus::Disabled
                    },
                    ..Snapshot::default()
                },
            );
        }
        if enabled && self.worker.is_none() {
            let control = Arc::clone(&self.control);
            let backend = self.backend;
            let allowed = self.allowed;
            match thread::Builder::new()
                .name("music-visualizer-audio".into())
                .spawn(move || {
                    // A native binding failure must not take down music controls.
                    if crate::terminal::catch_background_panic(|| run(&control, backend, allowed))
                        .is_err()
                    {
                        publish(
                            &control,
                            Snapshot {
                                status: CaptureStatus::Unavailable(
                                    "Audio capture stopped unexpectedly. Press c to retry.".into(),
                                ),
                                ..Snapshot::default()
                            },
                        );
                    }
                }) {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => publish(
                    &self.control,
                    Snapshot {
                        status: CaptureStatus::Unavailable(format!(
                            "Cannot start audio capture: {e}"
                        )),
                        ..Snapshot::default()
                    },
                ),
            }
        }
        if was != enabled
            && let Some(worker) = &self.worker
        {
            worker.thread().unpark();
        }
    }
    pub fn retry(&mut self) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.worker.take();
        }
        self.control.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.control
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn shutdown(&mut self) {
        self.control.quit.store(true, Ordering::Release);
        self.control.enabled.store(false, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    pub async fn finish(&mut self) {
        self.shutdown();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while self.worker.as_ref().is_some_and(|w| !w.is_finished())
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
        // An uncancellable OS consent call may still be pending. Its worker
        // checks cancellation immediately on returning and owns all resources.
    }
}
impl Drop for AudioService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn publish(control: &Control, value: Snapshot) {
    *control.latest.lock().unwrap_or_else(|e| e.into_inner()) = value;
}

#[cfg(target_os = "macos")]
fn publish_for(control: &Control, generation: u64, value: Snapshot) {
    let mut latest = control.latest.lock().unwrap_or_else(|e| e.into_inner());
    if control.enabled.load(Ordering::Acquire)
        && control.generation.load(Ordering::Acquire) == generation
        && !control.quit.load(Ordering::Acquire)
    {
        *latest = value;
    }
}

#[cfg(not(target_os = "macos"))]
fn run(control: &Control, _backend: CaptureBackend, _allowed: bool) {
    publish(
        control,
        Snapshot {
            status: CaptureStatus::Unavailable(
                "Live Music.app audio capture requires macOS.".into(),
            ),
            ..Snapshot::default()
        },
    );
}

#[cfg(target_os = "macos")]
fn run(control: &Control, backend: CaptureBackend, allowed: bool) {
    use super::analysis::Analyzer;
    use music_audio_capture::{Backend, Capture, StereoFrame};
    use std::time::Instant;
    if !allowed {
        publish(control,Snapshot { status:CaptureStatus::Unavailable("This backend has no real audio. Use the Music.app backend for live visualizations.".into()),..Snapshot::default() });
        return;
    }
    let backend = match backend {
        CaptureBackend::CoreAudio => Backend::CoreAudio,
        CaptureBackend::ScreenCaptureKit => Backend::ScreenCaptureKit,
    };
    let mut capture: Option<Capture> = None;
    let mut analyzer: Option<Analyzer> = None;
    let mut incoming = [StereoFrame::default(); 8192];
    let mut generation = u64::MAX;
    let mut failed = false;
    let mut last_check = Instant::now();
    let mut last_audio = Instant::now();
    let mut last_analysis = Instant::now();
    let mut rate = 0.0_f32;
    while !control.quit.load(Ordering::Acquire) {
        let tick = Instant::now();
        let next = control.generation.load(Ordering::Acquire);
        if generation != next || !control.enabled.load(Ordering::Acquire) {
            generation = next;
            failed = false;
            if let Some(previous) = capture.take()
                && let Err(e) = previous.close()
            {
                failed = true;
                publish_for(
                    control,
                    generation,
                    Snapshot {
                        status: CaptureStatus::Unavailable(e),
                        ..Snapshot::default()
                    },
                );
            }
            analyzer = None;
            if !control.enabled.load(Ordering::Acquire) {
                thread::park();
                continue;
            }
        }
        if failed {
            thread::park();
            continue;
        }
        if capture.is_none() {
            publish_for(
                control,
                generation,
                Snapshot {
                    status: CaptureStatus::Initializing,
                    ..Snapshot::default()
                },
            );
            match Capture::open(backend) {
                Ok(c) => {
                    capture = Some(c);
                    last_check = Instant::now();
                    last_audio = Instant::now();
                    last_analysis = Instant::now();
                }
                Err(e) => {
                    publish_for(
                        control,
                        generation,
                        Snapshot {
                            status: CaptureStatus::Unavailable(e),
                            ..Snapshot::default()
                        },
                    );
                    failed = true;
                    continue;
                }
            }
            if !control.enabled.load(Ordering::Acquire) || control.quit.load(Ordering::Acquire) {
                continue;
            }
        }
        let Some(source) = capture.as_mut() else {
            continue;
        };
        if last_check.elapsed() >= Duration::from_secs(2) {
            last_check = Instant::now();
            match source.changed() {
                Ok(true) => {
                    if let Some(previous) = capture.take()
                        && let Err(e) = previous.close()
                    {
                        publish_for(
                            control,
                            generation,
                            Snapshot {
                                status: CaptureStatus::Unavailable(e),
                                ..Snapshot::default()
                            },
                        );
                        failed = true;
                    }
                    analyzer = None;
                    continue;
                }
                Err(e) => {
                    if let Some(previous) = capture.take() {
                        let _ = previous.close();
                    }
                    publish_for(
                        control,
                        generation,
                        Snapshot {
                            status: CaptureStatus::Unavailable(e),
                            ..Snapshot::default()
                        },
                    );
                    analyzer = None;
                    failed = true;
                    continue;
                }
                Ok(false) => {}
            }
        }
        let count = source.read(&mut incoming);
        let measured_rate = source.sample_rate() as f32;
        if count > 0 && measured_rate.is_finite() && measured_rate >= 8000.0 {
            if analyzer.is_none() || measured_rate != rate {
                rate = measured_rate;
                analyzer = Some(Analyzer::new(rate));
            }
            let Some(dsp) = analyzer.as_mut() else {
                continue;
            };
            for value in &incoming[..count] {
                dsp.push(value.left, value.right);
            }
            last_audio = Instant::now();
            if dsp.ready() {
                let started = Instant::now();
                let elapsed = last_analysis.elapsed();
                last_analysis = started;
                let audio = dsp.analyze(elapsed);
                let analysis_us = started.elapsed().as_micros() as u64;
                let metrics = source.metrics();
                let status = if metrics.invalid_buffers > 0 {
                    CaptureStatus::Unavailable("The audio output supplied an unsupported or invalid format. Press c to retry.".into())
                } else if audio.loudness < 0.00001 {
                    CaptureStatus::Silent
                } else {
                    CaptureStatus::Live
                };
                publish_for(
                    control,
                    generation,
                    Snapshot {
                        status,
                        audio,
                        dropped_frames: metrics.dropped_frames,
                        analysis_us,
                    },
                );
                if metrics.invalid_buffers > 0 {
                    if let Some(previous) = capture.take() {
                        let _ = previous.close();
                    }
                    analyzer = None;
                    failed = true;
                    continue;
                }
            }
        } else if last_audio.elapsed() > Duration::from_secs(2) {
            publish_for(control, generation,Snapshot {status:CaptureStatus::Unavailable("No audio buffers arrived. Check capture permission and local output, then press c.".into()),..Snapshot::default()});
            if let Some(previous) = capture.take() {
                let _ = previous.close();
            }
            analyzer = None;
            failed = true;
            continue;
        }
        thread::park_timeout(Duration::from_secs_f64(1.0 / 30.0).saturating_sub(tick.elapsed()));
    }
    if let Some(previous) = capture {
        let _ = previous.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_is_lazy_until_visible_and_mock_capture_never_becomes_live() {
        let mut service = AudioService::new(CaptureBackend::CoreAudio, false);
        assert!(service.worker.is_none());
        assert_eq!(service.snapshot().status, CaptureStatus::Disabled);
        service.set_enabled(true);
        for _ in 0..100 {
            if matches!(service.snapshot().status, CaptureStatus::Unavailable(_)) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            service.snapshot().status,
            CaptureStatus::Unavailable(_)
        ));
        service.shutdown();
    }
}
