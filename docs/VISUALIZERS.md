# Live visualizations

The visualizer reacts to real audio playing through Music.app. Music.app continues to handle playback and DRM. Capture is private and unmuted; audio stays in bounded memory buffers and is never recorded to a file.

## Run and controls

```sh
cargo run --release -- --backend macos
```

Press `v` to open the visualizer. Spectrum bars, Mandelbrot and stereo waveform are included. Escape, `v` or `q` returns to the previous library view without stopping playback.

| Key | Action |
|---|---|
| `Tab` | Next effect |
| `V` | Next preset, including its configured effect |
| `c` | Retry capture after a permission/output problem |
| `Space`, `n`, `p` | Play/pause, next, previous |
| `[`, `]`, `-`, `+` | Seek and volume |
| `?` | Contextual help |

Capture starts when the view is visible and Music.app is playing. Closing the view, opening help, pausing or exiting stops capture. Paused visuals remain still. Music.app metadata polling retains its existing cadence. The mock backend reports that it has no real audio; it does not animate a pretend spectrum.

Allow the launching terminal access in **System Settings → Privacy & Security → Screen & System Audio Recording**. If macOS asks, restart the terminal. Tap creation alone does not prove permission: silent buffers can also mean a quiet passage, capture restrictions or output routed away from the Mac.

Use a true-color terminal with color output enabled. `NO_COLOR` disables RGB escape sequences throughout the existing TUI; unset it when using these color-based effects. Core Audio taps require macOS 14.2 or newer.

## Presets

Settings are loaded from `visualizer.toml` beside the regular `config.toml` (normally `~/.config/apple-music-tui/visualizer.toml`, or the corresponding XDG directory). Missing settings use the built-in presets. Invalid settings produce an explicit message in the visualization view and disable capture without disabling music controls.

Copy [the example](../examples/visualizer.toml) there and restart the application to load edits. Effect and preset selection are remembered separately in `visualizer-state.toml`; choosing a preset applies its effect. Selection persistence does not rewrite your preset file.

The default backend is `core_audio`, capturing Music.app's process. Set `backend = "screen_capture_kit"` to explicitly select system-wide ScreenCaptureKit audio. The application never silently switches from Music.app-only capture to capturing other applications. The alternate source is labelled “System audio”.

Presets define RGB palette colors, sensitivity and optional mappings. Mapping sources are `bass`, `mid`, `treble`, `beat` and `loudness`. `amount` is 0–1; smoothing is time-based in milliseconds. Spectrum and waveform use palette and sensitivity. Mandelbrot also uses the hue/brightness mappings and a small beat pulse.

Mandelbrot's center, zoom and iteration count define geometry. Static presets cache geometry until size/settings change; frequency features recolor it each frame. Small palette/brightness steps avoid repainting most terminal cells for tiny audio fluctuations. Iterations and canvas resolution have fixed upper bounds. Animated zoom is not included.

## Contributing a new effect

New Rust effects implement [Visualization](../src/visualizer/effects.rs) and are registered by `effects::registry()`. They receive an `AnalysisFrame` with 64 logarithmic bands (30 Hz–16 kHz, capped by Nyquist), each band's frequency, stereo waveform samples, bass/mid/treble energy, loudness, beat strength, sequence and timestamp. FFT channel powers are combined without cancelling opposite-phase stereo.

Effects own their animation state and draw RGB pixels to a reusable `Canvas`. They do not capture audio, call Music.app, write terminal escape sequences or perform filesystem/network operations. The half-block renderer handles terminal output. The canvas is currently capped at 240×128 pixels, then scaled to the view. This works in Ghostty and other true-color terminals; native terminal image transport is not implemented for these effects yet.

1. Copy [the module template](visualizers/pulse.rs) into `src/visualizer/effects/pulse.rs`.
2. Add `mod pulse;` to `src/visualizer/effects.rs`, then add `Box::new(pulse::Pulse::default())` to `registry()`.
3. Add a preset with `effect = "pulse"`. Module-specific options go in `[presets.parameters]`; the module's `validate` method deserializes and bounds them before capture starts.
4. Reuse buffers; cache expensive geometry; use elapsed time instead of assuming a fixed FPS. Treat unavailable audio as unavailable rather than generating a fallback music signal.
5. Add meaningful signal/render tests and run the quality gates below.

Shared TOML presets require no rebuild. New algorithms are source contributions and require rebuilding the Rust application. This is not a runtime binary-plugin loader or a Winamp `.milk` preset interpreter.

## Development and checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release

# Explicit generated-signal developer previews and cached-render timings:
cargo run --release --example visualizer_preview

# Opt-in production worker check while Music.app is playing:
cargo run --release --example visualizer_live_check
cargo run --release --example visualizer_live_check -- sck
```

Previews are explicitly generated test signals, never a production fallback. The live check issues no playback commands and writes measurements only.

Live speaker capture has been verified on this arm64 Mac with user-confirmed subscription playback. Bluetooth/AirPlay, sleep/wake, older macOS releases and denied/cold permission behavior are not fully verified. Output/process/format changes are detected periodically and rebuild capture; failures expose `c` retry rather than substituting an unrelated audio source.

## Performance design

Audio callbacks validate and copy stereo frames into a fixed 64 KiB ring, with no locks, allocations, FFT or drawing in our callback code. Analysis runs on a dedicated worker; the TUI consumes its latest snapshot. An overloaded consumer drops visualization data instead of blocking audio. Native capture initialization and cleanup also run on that worker.

Rendering starts at 30 FPS, configurable from 10–60. When paused, hidden or showing help, animation ticks stop. Canvas memory, FFT plans/scratch and geometry are reused. No per-frame tracing is enabled normally. ScreenCaptureKit's native accessor may copy data internally; this is not claimed to be zero-copy.

Capture-plus-analysis feasibility percentages and cached pixel-render timings exclude terminal/GPU work and system services. Do not use them as promises for total application CPU or power use.

The original static-geometry effects measured approximately 3–4% of one CPU core at 110×36 cells and 30 FPS through a drained pseudo-terminal. This includes application drawing/encoding, but excludes Ghostty/GPU, AppleScript subprocesses and system audio services. See [the validation record](visualizers/VALIDATION.md) for methods and limits.
