//! Isolated macOS feasibility/measurement tool, not part of the TUI.
//! No files, audio encoding, microphone capture or playback changes.
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use cidre::sc::stream::Output;
use cidre::{cat, cf, cm, core_audio as ca, define_obj_type, dispatch, ns, objc, os, sc};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Producer, Split},
};
use rustfft::{FftPlanner, num_complex::Complex};

const CAPACITY: usize = 8192;
const FFT_SIZE: usize = 4096;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioDeviceDestroyIOProcID(device: ca::Device, proc: ca::DeviceIoProcId) -> os::Status;
}

#[derive(Default)]
struct Stats {
    callbacks: AtomicU64,
    frames: AtomicU64,
    dropped: AtomicU64,
    invalid: AtomicU64,
    callback_ns: AtomicU64,
    callback_max_ns: AtomicU64,
    rate: AtomicU64,
    channels: AtomicU64,
    raw_peak: AtomicU32,
}

struct Sink {
    producer: ringbuf::HeapProd<f32>,
    stats: Arc<Stats>,
}

impl Sink {
    /// The caller validates float32 PCM and supplies buffers borrowed from
    /// Apple's callback. Downmixing is bounded by the callback's frame count.
    fn push(&mut self, buffers: &[cat::AudioBuf], rate: f64, channels: usize) {
        let start = Instant::now();
        self.stats.callbacks.fetch_add(1, Ordering::Relaxed);
        self.stats.rate.store(rate.to_bits(), Ordering::Relaxed);
        self.stats
            .channels
            .store(channels as u64, Ordering::Relaxed);
        let valid = buffers.len() == 1 && buffers[0].number_channels as usize == channels
            || buffers.len() == channels && buffers.iter().all(|b| b.number_channels == 1);
        if !valid
            || channels == 0
            || channels > 2
            || buffers.iter().any(|b| {
                b.data.is_null()
                    || !(b.data_bytes_size as usize)
                        .is_multiple_of(4 * b.number_channels.max(1) as usize)
                    || !(b.data as usize).is_multiple_of(std::mem::align_of::<f32>())
            })
        {
            self.stats.invalid.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let frames = buffers
            .iter()
            .map(|b| b.data_bytes_size as usize / 4 / b.number_channels as usize)
            .min()
            .unwrap_or(0);
        self.stats
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        let mut dropped = 0;
        let mut raw_peak = 0.0_f32;
        for frame in 0..frames {
            let sample = if buffers.len() == 1 {
                // SAFETY: float32 format/alignment/length checked above. The
                // callback owns these buffers and they are not retained.
                let data = unsafe {
                    std::slice::from_raw_parts(
                        buffers[0].data as *const f32,
                        buffers[0].data_bytes_size as usize / 4,
                    )
                };
                data[frame * channels..(frame + 1) * channels]
                    .iter()
                    .map(|value| {
                        raw_peak = raw_peak.max(value.abs());
                        *value
                    })
                    .sum::<f32>()
                    / channels as f32
            } else {
                buffers
                    .iter()
                    .map(|b| {
                        // SAFETY: frame is within every validated borrowed buffer.
                        let value = unsafe { *(b.data as *const f32).add(frame) };
                        raw_peak = raw_peak.max(value.abs());
                        value
                    })
                    .sum::<f32>()
                    / channels as f32
            };
            if !sample.is_finite() {
                self.stats.invalid.fetch_add(1, Ordering::Relaxed);
            } else if self.producer.try_push(sample).is_err() {
                dropped += 1;
            }
        }
        self.stats.dropped.fetch_add(dropped, Ordering::Relaxed);
        self.stats
            .raw_peak
            .fetch_max(raw_peak.to_bits(), Ordering::Relaxed);
        let ns = start.elapsed().as_nanos() as u64;
        self.stats.callback_ns.fetch_add(ns, Ordering::Relaxed);
        self.stats.callback_max_ns.fetch_max(ns, Ordering::Relaxed);
    }
}

struct TapContext {
    sink: Sink,
    rate: f64,
    channels: usize,
}

extern "C" fn tap_callback(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    context: Option<&mut TapContext>,
) -> os::Status {
    if let Some(ctx) = context {
        let n = input.number_buffers as usize;
        if n == 0 || n > 2 {
            ctx.sink.stats.invalid.fetch_add(1, Ordering::Relaxed);
        } else {
            // SAFETY: Core Audio's variable-sized AudioBufferList contains
            // number_buffers entries, valid for this callback only.
            let buffers = unsafe { std::slice::from_raw_parts(input.buffers.as_ptr(), n) };
            ctx.sink.push(buffers, ctx.rate, ctx.channels);
        }
    }
    os::Status(0)
}

#[repr(C)]
struct ScreenOutputInner {
    sink: Sink,
}
mod screen_output {
    // Scoped to generated binding code, not our buffer-handling code.
    #![allow(clippy::useless_transmute)]
    use super::*;
    define_obj_type!(
        pub(super) ScreenOutput + sc::stream::OutputImpl,
        ScreenOutputInner,
        SCREEN_OUTPUT
    );
}
use screen_output::ScreenOutput;
impl sc::stream::Output for ScreenOutput {}

#[objc::add_methods]
impl sc::stream::OutputImpl for ScreenOutput {
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
            sink.stats.invalid.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if !float32_pcm(format) {
            sink.stats.invalid.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Retains the native block buffer only for this callback. This API
        // can copy internally for alignment; it is not claimed to be zero-copy.
        match sample.audio_buf_list::<2>() {
            Ok(list) => {
                let raw = list.list();
                let n = raw.number_buffers as usize;
                if n == 0 || n > 2 {
                    sink.stats.invalid.fetch_add(1, Ordering::Relaxed);
                } else {
                    sink.push(
                        &raw.buffers[..n],
                        format.sample_rate,
                        format.channels_per_frame as usize,
                    );
                }
            }
            Err(_) => {
                sink.stats.invalid.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn float32_pcm(format: &cat::AudioStreamBasicDesc) -> bool {
    format.format == cat::AudioFormat::LINEAR_PCM
        && format
            .format_flags
            .contains(cat::AudioFormatFlags::IS_FLOAT)
        && !format
            .format_flags
            .contains(cat::AudioFormatFlags::IS_BIG_ENDIAN)
        && format.bits_per_channel == 32
        && (1..=2).contains(&format.channels_per_frame)
        && format.sample_rate.is_finite()
        && format.sample_rate > 0.0
}

fn cpu_seconds() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the provided struct on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0.0;
    }
    let usage = unsafe { usage.assume_init() };
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1_000_000.0
}

fn measure(
    mut consumer: ringbuf::HeapCons<f32>,
    stats: &Stats,
    seconds: u64,
    analysis: bool,
) -> serde_json::Value {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut fft_buffer = vec![Complex::default(); FFT_SIZE];
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
        .collect();
    let mut ring = [0.0_f32; FFT_SIZE];
    let mut incoming = [0.0_f32; CAPACITY];
    let mut write = 0;
    let mut count = 0_u64;
    let mut square_sum = 0.0_f64;
    let mut peak = 0.0_f32;
    let mut fft_total_ns = 0_u128;
    let mut fft_max_ns = 0_u128;
    let mut fft_count = 0;
    let mut top_hz = 0.0_f32;
    let cpu_start = cpu_seconds();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        let tick = Instant::now();
        let n = consumer.pop_slice(&mut incoming);
        for value in &incoming[..n] {
            square_sum += (*value as f64).powi(2);
            peak = peak.max(value.abs());
            ring[write] = *value;
            write = (write + 1) % FFT_SIZE;
        }
        count += n as u64;
        if analysis && n > 0 && count >= FFT_SIZE as u64 {
            let started = Instant::now();
            for i in 0..FFT_SIZE {
                fft_buffer[i] = Complex::new(ring[(write + i) % FFT_SIZE] * window[i], 0.0);
            }
            fft.process_with_scratch(&mut fft_buffer, &mut scratch);
            let strongest = (1..FFT_SIZE / 2)
                .max_by(|&a, &b| {
                    fft_buffer[a]
                        .norm_sqr()
                        .total_cmp(&fft_buffer[b].norm_sqr())
                })
                .unwrap_or(0);
            let rate = f64::from_bits(stats.rate.load(Ordering::Relaxed));
            top_hz = if fft_buffer[strongest].norm_sqr() > 1e-12 {
                strongest as f32 * rate as f32 / FFT_SIZE as f32
            } else {
                0.0
            };
            let elapsed = started.elapsed().as_nanos();
            fft_total_ns += elapsed;
            fft_max_ns = fft_max_ns.max(elapsed);
            fft_count += 1;
        }
        std::thread::sleep(Duration::from_secs_f64(1.0 / 30.0).saturating_sub(tick.elapsed()));
    }
    let elapsed = start.elapsed().as_secs_f64();
    let callbacks = stats.callbacks.load(Ordering::Relaxed);
    serde_json::json!({
        "elapsed_seconds": elapsed,
        "process_cpu_percent_one_core": 100.0 * (cpu_seconds() - cpu_start) / elapsed,
        "sample_rate": f64::from_bits(stats.rate.load(Ordering::Relaxed)),
        "source_channels": stats.channels.load(Ordering::Relaxed),
        "callbacks": callbacks,
        "source_frames": stats.frames.load(Ordering::Relaxed),
        "analysed_mono_samples": count,
        "dropped_frames": stats.dropped.load(Ordering::Relaxed),
        "invalid_buffers_or_samples": stats.invalid.load(Ordering::Relaxed),
        "rms": if count > 0 { (square_sum / count as f64).sqrt() } else { 0.0 },
        "peak": peak,
        "raw_channel_peak": f32::from_bits(stats.raw_peak.load(Ordering::Relaxed)),
        "nonzero_audio": f32::from_bits(stats.raw_peak.load(Ordering::Relaxed)) > 0.00001,
        "callback_copy_mean_us": stats.callback_ns.load(Ordering::Relaxed) as f64 / callbacks.max(1) as f64 / 1000.0,
        "callback_copy_max_us": stats.callback_max_ns.load(Ordering::Relaxed) as f64 / 1000.0,
        "fft_frames": fft_count,
        "fft_and_peak_mean_us": fft_total_ns as f64 / fft_count.max(1) as f64 / 1000.0,
        "fft_and_peak_max_us": fft_max_ns as f64 / 1000.0,
        "last_strongest_frequency_hz": top_hz,
        "mono_ring_bytes": CAPACITY * std::mem::size_of::<f32>(),
        "note": "CPU excludes initialization and other processes (coreaudiod, WindowServer, terminal). SCK callback timing covers extraction-to-ring copy only."
    })
}

fn coreaudio(
    sink: Sink,
    consumer: ringbuf::HeapCons<f32>,
    stats: &Stats,
    seconds: u64,
    analysis: bool,
    pid: Option<i32>,
) -> Result<serde_json::Value, String> {
    use ca::{aggregate_device_keys as agg, sub_device_keys as sub};
    let output =
        ca::System::default_output_device().map_err(|e| format!("default output: {e:?}"))?;
    let output_uid = output.uid().map_err(|e| format!("output UID: {e:?}"))?;
    let output_name = output
        .name()
        .map_err(|e| format!("output name: {e:?}"))?
        .to_string();
    let mut desc = if let Some(pid) = pid {
        let proc = ca::Process::with_pid(pid).map_err(|e| format!("Music process: {e:?}"))?;
        let id = ns::Number::with_u32(proc.0.0);
        ca::TapDesc::with_stereo_mixdown_of_processes(&ns::Array::from_slice(&[id.as_ref()]))
    } else {
        ca::TapDesc::with_stereo_global_tap_excluding_processes(&ns::Array::new())
    };
    desc.set_mute_behavior(ca::TapMuteBehavior::Unmuted);
    desc.set_private(true);
    desc.set_name(Some(ns::str!(c"Trapl capture probe")));
    let tap = desc
        .create_process_tap()
        .map_err(|e| format!("create tap: {e:?}"))?;
    let format = tap.asbd().map_err(|e| format!("tap format: {e:?}"))?;
    if !float32_pcm(&format) {
        return Err(format!("unsupported tap format: {format:?}"));
    }
    let tap_uid = tap.uid().map_err(|e| format!("tap UID: {e:?}"))?;
    let sub_device = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output_uid.as_type_ref()]);
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
            cf::str!(c"Trapl capture probe"),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );
    let device = ca::AggregateDevice::with_desc(&dict).map_err(|e| format!("aggregate: {e:?}"))?;
    let mut ctx = Box::new(TapContext {
        sink,
        rate: format.sample_rate,
        channels: format.channels_per_frame as usize,
    });
    let proc = device
        .create_io_proc_id(tap_callback, Some(&mut *ctx))
        .map_err(|e| format!("callback: {e:?}"))?;
    let started = ca::device_start(device, Some(proc)).map_err(|e| format!("start: {e:?}"))?;
    let mut result = measure(consumer, stats, seconds, analysis);
    let device = started.stop().map_err(|e| format!("stop: {e:?}"))?;
    // SAFETY: the callback belongs to this aggregate device, which has been
    // stopped; the context remains alive until its callback is destroyed.
    unsafe { AudioDeviceDestroyIOProcID(*device.as_ref(), proc) }
        .result()
        .map_err(|e| format!("destroy callback: {e:?}"))?;
    result["output_device"] = output_name.into();
    result["scope"] = if pid.is_some() {
        "Music.app process"
    } else {
        "system mix"
    }
    .into();
    drop(device);
    drop(tap);
    Ok(result)
}

async fn screencapturekit(
    sink: Sink,
    consumer: ringbuf::HeapCons<f32>,
    stats: &Stats,
    seconds: u64,
    analysis: bool,
) -> Result<serde_json::Value, String> {
    let content = tokio::time::timeout(Duration::from_secs(10), sc::ShareableContent::current())
        .await
        .map_err(|_| "ScreenCaptureKit enumeration timed out".to_string())?
        .map_err(|e| format!("ScreenCaptureKit content / permission: {e:?}"))?;
    let display = content
        .displays()
        .get(0)
        .map_err(|e| format!("No display available: {e:?}"))?;
    let filter = sc::ContentFilter::with_display_excluding_windows(&display, &ns::Array::new());
    let mut cfg = sc::StreamCfg::new();
    cfg.set_captures_audio(true);
    cfg.set_excludes_current_process_audio(true);
    cfg.set_sample_rate(48000);
    cfg.set_channel_count(2);
    cfg.set_width(2);
    cfg.set_height(2);
    cfg.set_minimum_frame_interval(cm::Time::new(1, 1));
    let stream = sc::Stream::new(&filter, &cfg);
    let queue = dispatch::Queue::serial_with_ar_pool();
    let delegate = ScreenOutput::with(ScreenOutputInner { sink });
    stream
        .add_stream_output(delegate.as_ref(), sc::OutputType::Audio, Some(&queue))
        .map_err(|e| format!("audio output: {e:?}"))?;
    tokio::time::timeout(Duration::from_secs(10), stream.start())
        .await
        .map_err(|_| "ScreenCaptureKit start timed out".to_string())?
        .map_err(|e| format!("ScreenCaptureKit start: {e:?}"))?;
    let mut result = measure(consumer, stats, seconds, analysis);
    tokio::time::timeout(Duration::from_secs(10), stream.stop())
        .await
        .map_err(|_| "ScreenCaptureKit stop timed out".to_string())?
        .map_err(|e| format!("ScreenCaptureKit stop: {e:?}"))?;
    stream
        .remove_stream_output(delegate.as_ref(), sc::OutputType::Audio)
        .map_err(|e| format!("remove audio output: {e:?}"))?;
    result["scope"] = "system mix; audio output only".into();
    Ok(result)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = args.first().map(String::as_str).unwrap_or("");
    if !matches!(backend, "coreaudio" | "sck") || args.len() > 4 {
        eprintln!(
            "Usage: capture-probe <coreaudio|sck> [seconds:1..30] [capture|fft] [Music PID; coreaudio only]"
        );
        std::process::exit(1);
    }
    let seconds = match args.get(1).map(|s| s.parse::<u64>()).transpose() {
        Ok(Some(s @ 1..=30)) => s,
        Ok(None) => 10,
        _ => {
            eprintln!("seconds must be 1..30");
            std::process::exit(1);
        }
    };
    let analysis = match args.get(2).map(String::as_str).unwrap_or("fft") {
        "fft" => true,
        "capture" => false,
        _ => {
            eprintln!("mode must be capture or fft");
            std::process::exit(1);
        }
    };
    let pid = match args.get(3).map(|s| s.parse::<i32>()).transpose() {
        Ok(p) if backend == "coreaudio" || p.is_none() => p,
        _ => {
            eprintln!("PID is valid only for coreaudio");
            std::process::exit(1);
        }
    };
    let stats = Arc::new(Stats::default());
    let (producer, consumer) = HeapRb::<f32>::new(CAPACITY).split();
    let sink = Sink {
        producer,
        stats: stats.clone(),
    };
    eprintln!("Starting {backend} ({seconds}s, analysis={analysis}); audio stays in memory.");
    let result = if backend == "coreaudio" {
        coreaudio(sink, consumer, &stats, seconds, analysis, pid)
    } else {
        screencapturekit(sink, consumer, &stats, seconds, analysis).await
    };
    match result {
        Ok(mut value) => {
            value["backend"] = backend.into();
            value["analysis"] = if analysis {
                "4096-point Hann FFT; 30 Hz target"
            } else {
                "capture and level metering only"
            }
            .into();
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            if value["nonzero_audio"] != true {
                std::process::exit(2);
            }
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"backend": backend, "error": error})
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_and_interleaved_stereo_produce_the_same_mono_samples() {
        let stats = Arc::new(Stats::default());
        let (producer, mut consumer) = HeapRb::<f32>::new(16).split();
        let mut sink = Sink { producer, stats };
        let mut left = [0.2_f32, 0.4];
        let mut right = [0.6_f32, 0.8];
        let planar = [
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
        ];
        sink.push(&planar, 48000.0, 2);
        let mut a = [0.0; 2];
        assert_eq!(consumer.pop_slice(&mut a), 2);
        let mut stereo = [0.2_f32, 0.6, 0.4, 0.8];
        sink.push(
            &[cat::AudioBuf {
                number_channels: 2,
                data_bytes_size: 16,
                data: stereo.as_mut_ptr().cast(),
            }],
            48000.0,
            2,
        );
        let mut b = [0.0; 2];
        assert_eq!(consumer.pop_slice(&mut b), 2);
        assert_eq!(a, b);
    }

    #[test]
    fn full_ring_drops_samples_without_blocking_and_rejects_bad_layout() {
        let stats = Arc::new(Stats::default());
        let (producer, _consumer) = HeapRb::<f32>::new(1).split();
        let mut sink = Sink {
            producer,
            stats: stats.clone(),
        };
        let mut samples = [0.25_f32, 0.5];
        let buffer = cat::AudioBuf {
            number_channels: 1,
            data_bytes_size: 8,
            data: samples.as_mut_ptr().cast(),
        };
        sink.push(&[buffer], 48000.0, 1);
        assert_eq!(stats.dropped.load(Ordering::Relaxed), 1);
        sink.push(&[buffer], 48000.0, 2);
        assert_eq!(stats.invalid.load(Ordering::Relaxed), 1);
    }
}
