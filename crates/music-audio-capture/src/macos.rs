use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use cidre::{cat, cf, cm, core_audio as ca, dispatch, ns, os, sc};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Producer, Split},
};

use super::{Backend, Metrics, StereoFrame};

const CAPACITY: usize = 8192;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioDeviceDestroyIOProcID(device: ca::Device, proc: ca::DeviceIoProcId) -> os::Status;
}

#[derive(Default)]
struct Counters {
    callbacks: AtomicU64,
    dropped: AtomicU64,
    invalid: AtomicU64,
    rate: AtomicU64,
}

struct Sink {
    producer: ringbuf::HeapProd<StereoFrame>,
    counters: Arc<Counters>,
}

impl Sink {
    // Apple's buffers are borrowed for one callback. No locks, allocations,
    // encoding, FFT or terminal operations are performed in this method.
    fn push(&mut self, buffers: &[cat::AudioBuf], channels: usize, rate: f64) {
        self.counters.callbacks.fetch_add(1, Ordering::Relaxed);
        let valid = (1..=2).contains(&channels)
            && (buffers.len() == 1 && buffers[0].number_channels as usize == channels
                || buffers.len() == channels && buffers.iter().all(|b| b.number_channels == 1));
        if !valid
            || buffers.iter().any(|b| {
                b.data.is_null()
                    || !(b.data_bytes_size as usize)
                        .is_multiple_of(4 * b.number_channels.max(1) as usize)
                    || !(b.data as usize).is_multiple_of(std::mem::align_of::<f32>())
            })
        {
            self.counters.invalid.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let frames = buffers
            .iter()
            .map(|b| b.data_bytes_size as usize / 4 / b.number_channels as usize)
            .min()
            .unwrap_or(0);
        if buffers
            .iter()
            .any(|b| b.data_bytes_size as usize / 4 / b.number_channels as usize != frames)
        {
            self.counters.invalid.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.counters.rate.store(rate.to_bits(), Ordering::Relaxed);
        let mut dropped = 0;
        let mut invalid = 0;
        for i in 0..frames {
            // SAFETY: float32 format, alignment, channel layout and buffer lengths
            // were checked. Samples are consumed before Apple's callback returns.
            let left = unsafe {
                *(buffers[0].data as *const f32).add(if buffers.len() == 1 {
                    i * channels
                } else {
                    i
                })
            };
            let right = if channels == 1 {
                left
            } else if buffers.len() == 1 {
                unsafe { *(buffers[0].data as *const f32).add(i * 2 + 1) }
            } else {
                unsafe { *(buffers[1].data as *const f32).add(i) }
            };
            if !left.is_finite() || !right.is_finite() {
                invalid += 1;
            } else if self.producer.try_push(StereoFrame { left, right }).is_err() {
                dropped += 1;
            }
        }
        self.counters.dropped.fetch_add(dropped, Ordering::Relaxed);
        self.counters.invalid.fetch_add(invalid, Ordering::Relaxed);
    }
}

fn float32(format: &cat::AudioStreamBasicDesc) -> bool {
    format.format == cat::AudioFormat::LINEAR_PCM
        && format
            .format_flags
            .contains(cat::AudioFormatFlags::IS_FLOAT)
        && !format
            .format_flags
            .contains(cat::AudioFormatFlags::IS_BIG_ENDIAN)
        && format.bits_per_channel == 32
        && format.frames_per_packet == 1
        && format.bytes_per_packet == format.bytes_per_frame
        && format.bytes_per_frame == 4 * format.interleaved_channels_num()
        && (1..=2).contains(&format.channels_per_frame)
        && format.sample_rate.is_finite()
        && format.sample_rate > 0.0
}

fn music_pid() -> Result<i32, String> {
    ca::System::processes().map_err(|e| format!("Cannot inspect Music.app audio process: {e:?}"))?
        .into_iter().find_map(|p| {
            (p.bundle_id().ok()?.to_string() == "com.apple.Music").then(|| p.pid().ok()).flatten()
        }).ok_or_else(|| "Music.app has no local audio process. Start playback in Music.app, then press c to retry.".into())
}

fn output_uid() -> Result<String, String> {
    ca::System::default_output_device()
        .and_then(|d| d.uid())
        .map(|u| u.to_string())
        .map_err(|e| format!("Cannot inspect the current audio output: {e:?}"))
}

struct CoreContext {
    sink: Sink,
    rate: f64,
    channels: usize,
}

extern "C" fn callback(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut CoreContext>,
) -> os::Status {
    if let Some(ctx) = ctx {
        let n = input.number_buffers as usize;
        if !(1..=2).contains(&n) {
            ctx.sink.counters.invalid.fetch_add(1, Ordering::Relaxed);
        } else {
            // SAFETY: Core Audio supplies a variable-length buffer list with n
            // entries, valid until this callback returns. No references escape.
            let buffers = unsafe { std::slice::from_raw_parts(input.buffers.as_ptr(), n) };
            ctx.sink.push(buffers, ctx.channels, ctx.rate);
        }
    }
    os::Status(0)
}

struct CoreSession {
    started: Option<ca::hardware::StartedDevice<ca::AggregateDevice>>,
    tap: Option<ca::TapGuard>,
    context: Option<Box<CoreContext>>,
    proc: ca::DeviceIoProcId,
    pid: i32,
    output: String,
    format: cat::AudioStreamBasicDesc,
}

impl CoreSession {
    fn open(sink: Sink) -> Result<Self, String> {
        use ca::{aggregate_device_keys as agg, sub_device_keys as sub};
        let pid = music_pid()?;
        let process =
            ca::Process::with_pid(pid).map_err(|e| format!("Music.app process lookup: {e:?}"))?;
        let id = ns::Number::with_u32(process.0.0);
        let mut desc =
            ca::TapDesc::with_stereo_mixdown_of_processes(&ns::Array::from_slice(&[id.as_ref()]));
        desc.set_mute_behavior(ca::TapMuteBehavior::Unmuted);
        desc.set_private(true);
        desc.set_name(Some(ns::str!(c"Apple Music TUI visualizer")));
        let tap = desc.create_process_tap().map_err(|e| format!("Cannot create Music.app audio tap ({e:?}). Allow system audio recording for your terminal in macOS Privacy & Security, restart it if requested, then press c."))?;
        let format = tap
            .asbd()
            .map_err(|e| format!("Cannot read the audio format: {e:?}"))?;
        if !float32(&format) {
            return Err(
                "This audio output does not supply supported float32 mono/stereo audio.".into(),
            );
        }
        let output =
            ca::System::default_output_device().map_err(|e| format!("Audio output: {e:?}"))?;
        let output_uid = output
            .uid()
            .map_err(|e| format!("Audio output identifier: {e:?}"))?;
        let output_name = output_uid.to_string();
        let tap_uid = tap
            .uid()
            .map_err(|e| format!("Audio tap identifier: {e:?}"))?;
        let sub_device =
            cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output_uid.as_type_ref()]);
        let sub_tap = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[tap_uid.as_type_ref()]);
        let dict = cf::DictionaryOf::with_keys_values(
            &[
                agg::is_private(),
                agg::is_stacked(),
                agg::tap_auto_start(),
                agg::name(),
                agg::main_sub_device(),
                agg::uid(),
                agg::sub_device_list(),
                agg::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::Boolean::value_true(),
                cf::str!(c"Apple Music TUI visualizer"),
                &output_uid,
                &cf::Uuid::new().to_cf_string(),
                &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );
        let device = ca::AggregateDevice::with_desc(&dict)
            .map_err(|e| format!("Cannot initialize audio capture: {e:?}"))?;
        let mut context = Box::new(CoreContext {
            sink,
            rate: format.sample_rate,
            channels: format.channels_per_frame as usize,
        });
        let proc = device
            .create_io_proc_id(callback, Some(&mut *context))
            .map_err(|e| format!("Cannot register audio callback: {e:?}"))?;
        let started = ca::device_start(device, Some(proc))
            .map_err(|e| format!("Cannot start audio capture: {e:?}"))?;
        Ok(Self {
            started: Some(started),
            tap: Some(tap),
            context: Some(context),
            proc,
            pid,
            output: output_name,
            format,
        })
    }

    fn changed(&self) -> Result<bool, String> {
        let Some(tap) = self.tap.as_ref() else {
            return Ok(false);
        };
        let format = tap
            .asbd()
            .map_err(|e| format!("Audio output disconnected: {e:?}"))?;
        Ok(output_uid()? != self.output || music_pid()? != self.pid || format != self.format)
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(started) = self.started.take() else {
            return Ok(());
        };
        match started.stop() {
            Ok(device) => {
                // SAFETY: this callback was registered on this device, which is
                // now stopped. The boxed callback context is still alive.
                let result =
                    unsafe { AudioDeviceDestroyIOProcID(*device.as_ref(), self.proc) }.result();
                drop(device);
                if let Err(error) = result {
                    // Do not risk freeing an OS callback's context if teardown
                    // cannot be established. This exceptional allocation stays
                    // alive until process exit; capture is reported unavailable.
                    std::mem::forget(self.context.take());
                    return Err(format!("Audio callback teardown failed: {error:?}"));
                }
                self.context.take();
                self.tap.take();
                Ok(())
            }
            Err(error) => {
                std::mem::forget(self.context.take());
                std::mem::forget(self.tap.take());
                Err(format!("Audio capture could not stop cleanly: {error:?}"))
            }
        }
    }
}

impl Drop for CoreSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

mod screen {
    // Scoped to the binding's generated class-registration implementation.
    #![allow(clippy::useless_transmute)]
    use super::*;
    use cidre::{define_obj_type, objc, sc::stream::Output};
    #[repr(C)]
    struct Inner {
        sink: Sink,
    }
    define_obj_type!(
        OutputHandler + sc::stream::OutputImpl,
        Inner,
        MUSIC_TUI_AUDIO_OUTPUT
    );
    impl Output for OutputHandler {}
    #[objc::add_methods]
    impl sc::stream::OutputImpl for OutputHandler {
        extern "C" fn impl_stream_did_output_sample_buf(
            &mut self,
            _cmd: Option<&objc::Sel>,
            _stream: &sc::Stream,
            sample: &mut cm::SampleBuf,
            kind: sc::OutputType,
        ) {
            if kind != sc::OutputType::Audio {
                return;
            }
            let sink = &mut self.inner_mut().sink;
            let Some(format) = sample.format_desc().and_then(|d| d.stream_basic_desc()) else {
                sink.counters.invalid.fetch_add(1, Ordering::Relaxed);
                return;
            };
            if !float32(format) {
                sink.counters.invalid.fetch_add(1, Ordering::Relaxed);
                return;
            }
            // Apple's accessor may copy for alignment. Audio storage is borrowed
            // only for this callback; do not assume a zero-copy capture path.
            if let Ok(list) = sample.audio_buf_list::<2>() {
                let raw = list.list();
                let n = raw.number_buffers as usize;
                if (1..=2).contains(&n) {
                    sink.push(
                        &raw.buffers[..n],
                        format.channels_per_frame as usize,
                        format.sample_rate,
                    );
                } else {
                    sink.counters.invalid.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                sink.counters.invalid.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(super) struct Session {
        runtime: tokio::runtime::Runtime,
        stream: cidre::arc::R<sc::Stream>,
        handler: cidre::arc::R<OutputHandler>,
        _queue: cidre::arc::R<dispatch::Queue>,
        output: String,
        active: bool,
    }
    impl Session {
        pub(super) fn open(sink: Sink) -> Result<Self, String> {
            // Resolve fallible metadata before starting a stream, so an output
            // lookup failure cannot bypass the session's explicit teardown.
            let output = output_uid()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|e| e.to_string())?;
            runtime.block_on(async {
                let content = tokio::time::timeout(Duration::from_secs(8), sc::ShareableContent::current()).await
                    .map_err(|_| "Audio capture permission is still pending. Check macOS Privacy & Security, then press c.".to_string())?
                    .map_err(|e| format!("ScreenCaptureKit access denied or unavailable ({e:?}). Allow capture for your terminal in macOS Privacy & Security, then press c."))?;
                let display = content.displays().get(0).map_err(|_| "No display is available for system audio capture.".to_string())?;
                let filter = sc::ContentFilter::with_display_excluding_windows(&display, &ns::Array::new());
                let mut cfg = sc::StreamCfg::new();
                cfg.set_captures_audio(true); cfg.set_excludes_current_process_audio(true);
                cfg.set_sample_rate(48000); cfg.set_channel_count(2); cfg.set_width(2); cfg.set_height(2);
                cfg.set_minimum_frame_interval(cm::Time::new(1, 1));
                let stream = sc::Stream::new(&filter, &cfg);
                let queue = dispatch::Queue::serial_with_ar_pool();
                let handler = OutputHandler::with(Inner { sink });
                stream.add_stream_output(handler.as_ref(), sc::OutputType::Audio, Some(&queue)).map_err(|e| format!("Audio capture output: {e:?}"))?;
                tokio::time::timeout(Duration::from_secs(8), stream.start()).await.map_err(|_| "Audio capture startup timed out.".to_string())?
                    .map_err(|e| format!("Audio capture startup failed: {e:?}"))?;
                Ok((stream, handler, queue))
            }).map(|(stream, handler, queue)| Self { runtime, stream, handler, _queue: queue, output, active: true })
        }
        pub(super) fn changed(&self) -> Result<bool, String> {
            Ok(output_uid()? != self.output)
        }
        pub(super) fn close(&mut self) -> Result<(), String> {
            if !self.active {
                return Ok(());
            }
            self.active = false;
            self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(8), self.stream.stop())
                    .await
                    .map_err(|_| "System audio capture stop timed out.".to_string())?
                    .map_err(|e| format!("System audio capture stop failed: {e:?}"))
            })?;
            self.stream
                .remove_stream_output(self.handler.as_ref(), sc::OutputType::Audio)
                .map_err(|e| format!("Audio output cleanup failed: {e:?}"))
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.close();
        }
    }
}

enum Session {
    Core(CoreSession),
    Screen(screen::Session),
}

/// Construct and use on the same dedicated worker thread. Native objects and
/// callback contexts are never handed to the TUI or its async executor.
pub struct Capture {
    session: Session,
    consumer: ringbuf::HeapCons<StereoFrame>,
    counters: Arc<Counters>,
}

impl Capture {
    pub fn open(backend: Backend) -> Result<Self, String> {
        let counters = Arc::new(Counters::default());
        let (producer, consumer) = HeapRb::new(CAPACITY).split();
        let sink = Sink {
            producer,
            counters: counters.clone(),
        };
        let session = match backend {
            Backend::CoreAudio => Session::Core(CoreSession::open(sink)?),
            Backend::ScreenCaptureKit => Session::Screen(screen::Session::open(sink)?),
        };
        Ok(Self {
            session,
            consumer,
            counters,
        })
    }
    pub fn read(&mut self, destination: &mut [StereoFrame]) -> usize {
        self.consumer.pop_slice(destination)
    }
    pub fn sample_rate(&self) -> f64 {
        f64::from_bits(self.counters.rate.load(Ordering::Relaxed))
    }
    pub fn changed(&self) -> Result<bool, String> {
        match &self.session {
            Session::Core(s) => s.changed(),
            Session::Screen(s) => s.changed(),
        }
    }
    pub fn metrics(&self) -> Metrics {
        Metrics {
            callbacks: self.counters.callbacks.load(Ordering::Relaxed),
            dropped_frames: self.counters.dropped.load(Ordering::Relaxed),
            invalid_buffers: self.counters.invalid.load(Ordering::Relaxed),
        }
    }
    pub fn close(mut self) -> Result<(), String> {
        match &mut self.session {
            Session::Core(s) => s.close(),
            Session::Screen(s) => s.close(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn float_pcm_format_rejects_padding_and_compressed_packets() {
        let mut format = cat::AudioStreamBasicDesc::common_f32(48000.0, 2, true);
        assert!(float32(&format));
        assert!(float32(&cat::AudioStreamBasicDesc::common_f32(
            48000.0, 2, false
        )));
        format.bytes_per_frame = 16;
        format.bytes_per_packet = 16;
        assert!(!float32(&format));
        format = cat::AudioStreamBasicDesc::common_f32(48000.0, 2, true);
        format.frames_per_packet = 2;
        assert!(!float32(&format));
    }
    #[test]
    fn stereo_phase_is_preserved_and_invalid_layouts_are_rejected() {
        let counters = Arc::new(Counters::default());
        let (producer, mut consumer) = HeapRb::new(8).split();
        let mut sink = Sink {
            producer,
            counters: counters.clone(),
        };
        let mut data = [0.25_f32, -0.25, 0.5, -0.5];
        let buffer = cat::AudioBuf {
            number_channels: 2,
            data_bytes_size: 16,
            data: data.as_mut_ptr().cast(),
        };
        sink.push(&[buffer], 2, 48000.0);
        let mut out = [StereoFrame::default(); 2];
        assert_eq!(consumer.pop_slice(&mut out), 2);
        assert_eq!(
            out[0],
            StereoFrame {
                left: 0.25,
                right: -0.25
            }
        );
        sink.push(&[buffer], 1, 48000.0);
        assert_eq!(counters.invalid.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn planar_audio_and_overflow_are_bounded() {
        let counters = Arc::new(Counters::default());
        let (producer, mut consumer) = HeapRb::new(1).split();
        let mut sink = Sink {
            producer,
            counters: counters.clone(),
        };
        let mut left = [0.25_f32, 0.5];
        let mut right = [-0.5_f32, -1.0];
        sink.push(
            &[
                cat::AudioBuf {
                    number_channels: 1,
                    data_bytes_size: 8,
                    data: left.as_mut_ptr().cast(),
                },
                cat::AudioBuf {
                    number_channels: 1,
                    data_bytes_size: 8,
                    data: right.as_mut_ptr().cast(),
                },
            ],
            2,
            48000.0,
        );
        assert_eq!(
            consumer.try_pop(),
            Some(StereoFrame {
                left: 0.25,
                right: -0.5
            })
        );
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 1);
    }
}
