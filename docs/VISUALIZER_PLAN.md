# Modular Rust visualizer implementation

Status: static visualizers implemented and verified, 2026-10-14. The optional animated zoom experiment was removed before publication. The live feature, native crate, analysis, three effects, configuration and contributor template are complete. Workspace tests, formatting, Clippy and release builds pass. See [the current usage/contribution guide](VISUALIZERS.md). The design below records the rationale and remaining verification gates; proposals are now implemented unless explicitly listed as future work.

## Evidence and capture choice

Both Core Audio process taps and ScreenCaptureKit have now delivered nonzero 48 kHz stereo audio on this Mac while the user-confirmed Apple Music subscription track played through MacBook Pro Speakers. The release probe measured approximately 0.62% versus 1.02% of one CPU core respectively, with no drops or invalid samples in eight-second runs. These numbers exclude system services and terminal rendering.

Implemented default: a private, unmuted **Music.app process tap**, using Rust `cidre` bindings. It has demonstrated Music.app-specific capture and a small local measured cost. **ScreenCaptureKit** is an explicitly selectable system-audio alternative. The production workers for both sources have received live audio with zero drops on the local speakers. The application never silently switches a Music-only source to system-wide capture. No BlackHole, Swift helper or browser playback is needed for the currently verified local-speaker case.

The short measurements do not establish a universal performance winner. Device/format transitions, permissions and AirPlay remain gates before making broader compatibility claims. The user's terminal is Ghostty; tmux use and other output preferences have not been established.

## Architecture

```text
Rust native capture crate → bounded PCM ring → Rust analysis worker
                                               ↓
                                   latest AnalysisFrame snapshot
                                               ↓
                                selected visualization module
                                               ↓
                              reusable pixel/cell canvas → terminal
```

- Keep platform-specific FFI and callback lifetime handling in a small native capture crate. The main application's `unsafe_code = "forbid"` remains intact. Gate the dependency to macOS so non-macOS builds still work.
- Start capture only when a visualization is visible. Stop it on disable/exit and expose explicit initializing, live, silent, unavailable and permission-denied states. A silent buffer is not proof of authorization or unsupported DRM.
- Validate the actual PCM format; support interleaved/planar stereo. Preallocate a bounded ring; callback work is sample copying only. Do not block, allocate or render there. On overload, drop visualization data and report counters.
- Process audio independently of Music.app's metadata poll. Start with a preplanned 4096-point Hann FFT at 30 Hz, logarithmic frequency bands, channel-aware spectral power, waveform, loudness and smoothed bass/mid/treble features. Keep frequency boundaries and sample timestamps explicit.
- Deliver the latest frame through a replaceable snapshot, not an unbounded history queue. An effect that falls behind skips frames. No need to retain or write captured audio.

## Integration points in this fork

`src/runtime.rs` now has a visualization ticker gated by visibility and playback. The existing Music.app metadata poll remains unchanged. A service owns capture and analysis; the controller consumes its latest status/frame snapshot. Mutable effect state stays outside domain/library state so animation does not clone collections or rebuild indexes.

`src/app/state.rs`, `src/app/action.rs`, `src/app/reducer.rs` and `src/input.rs` provide navigation and control integration. `v` opens/closes a dedicated view; Tab changes effects; `V` changes presets; Escape returns; `c` retries capture. Playback controls remain available. Contextual help is included and closing the view preserves navigation and playback.

`src/ui/artwork.rs` already handles Ghostty/Kitty image presentation and image lifecycle, but it is optimized for occasional static artwork, not video. Do not retransmit/reencode artwork at animation frequency. Give visualizer graphics a separate image ID/lifecycle and cap resolution/frame rate. First implement portable half-block output, then measure a Ghostty pixel-graphics option.

## Contributor interface and initial effects

The in-source `Visualization` trait provides identifier/name, parameter validation, configuration, reset, elapsed-time update and RGB canvas rendering. Canvas dimensions communicate resize. A central registry selects modules; the contribution template is compiled and tested against the public API. Contributors add effects without editing audio code.

Presets are TOML configuration: palette, sensitivity, smoothing, frequency mappings and effect parameters. Contributors can share presets without compiling, and contribute entirely new Rust effects through source modules. Runtime-loaded native libraries are out of initial scope because Rust has no stable plugin ABI. Sandboxed runtime plugins can be evaluated separately once the effect interface is proven.

Initial effects:

1. **Spectrum bars**: real frequency bands, configurable palette and peak decay; use this to verify analysis visually.
2. **Mandelbrot**: precompute escape iterations on resize/parameter changes, then recolor from smoothed spectrum features. Bass/mid/treble can control palette position, saturation and brightness; beat strength can add a bounded brightness pulse. This makes palette animation cheap. Debounce expensive geometry changes, cap iteration/resolution budgets, and treat animated zoom as an independently measured enhancement.
3. **Waveform/oscilloscope**, after stereo/timing fidelity is established.

## Performance and verification gates

- Compare release capture-only with capture+analysis. Measure own-process CPU, audio/display service costs and sustained power behavior; repeat under the same workload. Do not generalize the current eight-second percentages.
- Track callback processing time, lost frames, analysis duration, frame age and draw/transport time separately.
- Keep reusable buffers, plans and geometry caches. Target 30 FPS initially, coalesce settings/resize changes and drop old frames. Lower rendering resolution/iteration count when over budget.
- Record no per-frame logs in normal operation. Disable capture/animation work when not visible; measure idle behavior.
- Tests: known tones map to the expected bands, silence/finite values, stereo cancellation, smoothing independent of frame rate, bounded overflow and lifecycle cleanup. Use synthetic signals only in explicit tests/development previews; production failure never becomes a simulated music response.
- Check app pause/resume/restart, track changes, speaker/headphone/Bluetooth transitions, permission denial, small terminals, switching effects and clean exit. AirPlay is an explicit separate test.
- For Mandelbrot, verify geometry caching and audio-driven color changes without allocating a fresh canvas per frame. Measure Ghostty rendering separately from audio capture.

## Delivered and remaining verification

Delivered: isolated native crate, process-tap default, explicit SCK alternate, bounded stereo ring, shared FFT/features, gated runtime, spectrum, cached Mandelbrot recoloring, stereo waveform, TOML presets, separate selection persistence, contributor API/template and contextual help. Safe application code still forbids unsafe Rust. Callback/layout, tone/stereo, time-based smoothing, geometry cache, contributor options, bounded canvases and navigation have automated coverage.

The production capture smoke checks exercise disable/re-enable and explicit teardown without sending playback commands. Terminal output is checked in a true-color pseudo-terminal, including all three effects, preset switching, help and returning to the library. The generated-signal preview measured about 1.7 ms for a 160×100 Mandelbrot geometry build and 0.02 ms for cached recoloring on this Mac; this excludes terminal transport.

Remaining: Bluetooth/AirPlay, physical Ghostty display/GPU/power measurement, sleep/wake and older/denied/cold permission paths. Optional terminal image transport, animated fractal geometry and runtime plugin loading are future enhancements. Fixed work limits are implemented; automatic resolution/iteration adaptation is not.
