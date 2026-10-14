# Local validation — 2026-10-13

Environment: arm64 Mac, macOS 27.2, Rust 1.94.1, release builds. The user confirmed Apple Music subscription playback through MacBook Pro Speakers and enabled Ghostty capture permission. These are short feasibility checks, not controlled CPU/power benchmarks or device compatibility certification.

## Quality gates

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace` and `cargo build --release --bins --examples` pass. 214 tests pass; 10 pre-existing live backend tests remain ignored.

New coverage includes stereo layout/phase and overflow, padded/invalid PCM format rejection, tone location and silence, time-based smoothing, fractal geometry caching and palette response, bounded canvases, RGB half-block scaling, preserved navigation/playback controls and the compiled contributor template/options.

## Production worker checks

`visualizer_live_check` observes eight seconds of real audio, disables capture, checks that disabled status stays clear, re-enables capture and checks for live audio, then explicitly shuts down. It sends no playback commands and stores no audio.

| Source | Observed live snapshots | Dropped frames | Maximum measured analysis | Restarted |
|---|---:|---:|---:|---|
| Core Audio Music.app tap | 211 | 0 | 81 µs | Yes |
| ScreenCaptureKit system audio | 207 | 0 | 137 µs | Yes |

Sampling snapshots every 33 ms can skip worker sequences. Analysis timings are worker FFT/features only, not capture, drawing or latency. Tracks/passages differed between checks; system-audio capture can include other applications.

## Rendering and own-process cost

The release TUI ran through a 110×36 pseudo-terminal with true-color output enabled, a fast draining consumer and 30 FPS defaults. All three effect headers reported live capture; preset switching, contextual help and returning to the library worked. Captured RGB terminal output was rendered to PNG and visually inspected. `NO_COLOR` was removed from the test subprocess environment so the existing terminal backend emitted colors.

Own-process CPU was the difference in `ps` cumulative CPU time over each interval, expressed as a percentage of **one core**. It includes the TUI's capture worker, analysis, rendering and terminal encoding; it excludes the consumer, physical Ghostty/GPU, AppleScript subprocesses and OS audio services. Memory is resident process memory.

| View | Sample duration | Own-process CPU | Terminal output | RSS |
|---|---:|---:|---:|---:|
| Library, capture hidden | 6 s | 1.33% | 172 B/s | 81.8 MiB |
| Spectrum | 6 s | 3.16% | 56.6 kB/s | 86.3 MiB |
| Stereo waveform | 6 s | 3.67% | 280 kB/s | 86.3 MiB |
| Mandelbrot, final quantized palette | 8 s | 3.62% | 537 kB/s | 86.0 MiB |

An earlier continuously recolored Mandelbrot run measured 7.48% and 2.06 MB/s. Small palette-position/brightness steps now avoid insignificant repainting; a regression test verifies stable pixels under small audio fluctuations. Different music passages and separate runs mean these figures do not establish a controlled percentage improvement. Larger windows, terminal backpressure and 60 FPS can cost more.

The explicit generated-signal preview at 160×100 pixels measured 1.795 ms for initial Mandelbrot geometry and 0.016 ms mean cached recoloring across 300 frames. Spectrum/waveform averaged 0.005/0.003 ms. These pixel timings exclude terminal drawing/transport.

## Remaining checks

Bluetooth/AirPlay, sleep/wake, device/process transitions under live load, older macOS releases, cold/denied permission flows and sustained physical Ghostty CPU/GPU/power measurements remain unverified. Native image transport, automatic work-budget adaptation and runtime plugin loading are not implemented. Fixed memory/resolution/iteration limits and explicit failure/retry behavior are implemented.
