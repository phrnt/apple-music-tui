# Rust audio capture feasibility probe

Isolated macOS research tool. It is not wired into the TUI and does not change Music.app playback, routing, volume or mute state. No audio files are written. JSON results contain numerical measurements only.

Both backends use `cidre 0.29.0` Rust bindings in the same process. There is no Swift helper or audio pipe between application processes. Cidre builds small Objective-C framework glue internally; it is not a Swift-free implementation of the operating system itself. Our capture, buffering and analysis code is Rust.

## Build and check

```sh
cargo build --release --locked --manifest-path tools/capture-probe/Cargo.toml
cargo test --locked --manifest-path tools/capture-probe/Cargo.toml
cargo clippy --locked --manifest-path tools/capture-probe/Cargo.toml --all-targets -- -D warnings
```

The probe is a separate Cargo package so the TUI's dependency graph and `unsafe_code = "forbid"` policy are unchanged. It requires macOS and native developer tools.

## Run

Play a subscription track in Music.app through this Mac's speakers or headphones. Grant capture access to the launching application in System Settings → Privacy & Security → Screen & System Audio Recording; restart that application if macOS requests it.

```sh
# Music.app-only Core Audio tap, with FFT analysis:
tools/capture-probe/target/release/capture-probe coreaudio 8 fft "$(pgrep -x Music)"

# Core Audio system mix, with FFT analysis:
tools/capture-probe/target/release/capture-probe coreaudio 8 fft

# ScreenCaptureKit system mix, audio output only:
tools/capture-probe/target/release/capture-probe sck 8 fft

# Capture and level metering, without FFT:
tools/capture-probe/target/release/capture-probe coreaudio 8 capture "$(pgrep -x Music)"
tools/capture-probe/target/release/capture-probe sck 8 capture
```

The duration must be 1–30 seconds. Exit status 0 means nonzero channel data was observed, 2 means silence, and 1 means initialization/format/permission/shutdown failed. Nonzero system-mix data alone does not prove attribution to Music.app if other applications are making sound.

The Core Audio tap is explicitly private and unmuted, bound to the current default output device. ScreenCaptureKit registers an audio output only, requests stereo 48 kHz and minimizes associated display dimensions. It currently captures the system mix, not an application-filtered stream.

## Memory and callback behavior

Float32 PCM is validated before reading. Interleaved and planar stereo layouts are supported. The probe downmixes to mono into a fixed 8192-sample ring (32 KiB) and drops samples if full rather than blocking. Native sample buffers are not retained after their callback. FFT buffers, the Hann window and plan scratch memory are preallocated. Analysis runs on the consumer at a nominal 30 Hz.

Raw-channel peak is measured separately so opposite-phase stereo cannot be mistaken for absent audio. The FFT is still mono: this probe is not a full stereo spectrum implementation. Production effects should preserve stereo or combine spectral channel powers to avoid cancellation.

## Live results — 2026-10-13

Environment: arm64 Mac, macOS 27.2 (26B5091g), Rust 1.94.1. User confirmed an Apple Music subscription track was playing. Music.app reported playing, volume 100, unmuted; default output was MacBook Pro Speakers. `xr0` was confirmed to be the computer's Music.app output name, not a remote speaker.

After the user enabled capture permission for Ghostty:

| Backend | Scope | Rate | Process CPU, % of one core | Mean FFT + peak scan | Drops / invalid |
|---|---|---|---|---|---|
| Core Audio | Music.app process | 48 kHz stereo | 0.62% | 80.5 µs | 0 / 0 |
| ScreenCaptureKit | System mix, audio output only | 48 kHz stereo | 1.02% | 88.2 µs | 0 / 0 |

Each measurement ran about eight seconds in an optimized release build. A preceding successful Music.app run measured 0.53% CPU and 61.2 µs per FFT/peak scan. These are short feasibility measurements, not a statistically controlled benchmark or a guarantee for other Macs, formats or routes. Different portions of the track were measured sequentially.

CPU is this probe process's user + system CPU divided by wall time. It excludes initialization, coreaudiod, WindowServer, Music.app and Ghostty. No terminal graphics were drawn. Callback timing covers sample-to-ring processing, not the whole OS capture pipeline; ScreenCaptureKit block extraction occurs before that timer.

Initial tap runs delivered zero-filled buffers, and ScreenCaptureKit returned TCC error -3801. Both later delivered nonzero audio. Do not infer that a created tap or regular callback stream proves capture permission or real audio availability.

Raw numerical records are in `results/`. No track audio, title or account credentials are included.

## Remaining checks

- Permission attribution and cold-start behavior when launched through Ghostty versus Codex.
- Pause/resume, track changes, app restart and output-device transitions.
- Wired/Bluetooth outputs and Music.app AirPlay routing.
- Separate capture-only versus capture+analysis costs; service CPU and power overhead.
- Stereo signal integrity and format changes.
- Rendering cost: static Mandelbrot palette cycling versus animated fractal geometry, half-blocks versus Ghostty image graphics.
