//! Native ownership and FFI stay here; the TUI uses only this safe interface.
//! Audio is never saved, encoded, replayed or used to change Music.app volume.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    CoreAudio,
    ScreenCaptureKit,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub callbacks: u64,
    pub dropped_frames: u64,
    pub invalid_buffers: u64,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Capture;
