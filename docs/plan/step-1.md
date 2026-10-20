# Step 1 — one deck plays (macOS first)

Goal: one track plays through the real engine architecture on a Mac, with the real-time guardrails in place from the first commit. Everything later (keylock, Rekordbox, library format, UI, Pi) plugs into this skeleton.

## Scope

1. **Workspace** with four crates under `crates/`: `engine-core`, `engine-media`, `engine-io`, `engined`.
2. **`engine-media`**: decode one file with Symphonia into a resident in-memory buffer, in chunks with per-chunk ready flags, filled by a loader thread so playback can start before decoding finishes. Resample once at load to the device sample rate (rubato).
3. **`engine-core`** (no I/O, no threads):
   - `Source`: the resident sample store with chunk ready flags.
   - `Transport`: play, pause, seek, rate (incl. negative), loop-free for now, plus the **slip shadow playhead** (shadow position advances at the real rate; returning from a slip-able action snaps the playhead to the shadow).
   - `Scaler`: interpolating resampler reading the source at fractional positions (cubic Hermite); works for any rate including 0 and reverse.
   - `Deck` tying them together; `Engine::process(&mut self, out: &mut [f32], frames)` as the callback body.
   - Commands into the callback through a bounded SPSC ring (rtrb); state out through atomics.
   - If the playhead hits an unloaded chunk, output silence for that block and flag it — never block.
4. **`engine-io`**: `AudioBackend` trait and a cpal implementation (CoreAudio on macOS, ALSA on Linux). All thread priority / device specifics stay behind the trait.
5. **`engined`**: CLI `engined <file>`; keys: space play/pause, ←/→ seek ±5 s, +/- rate ±1 %, 0 reset rate, r reverse, s toggle slip, q quit.

## Guardrails (first commit)

- `assert_no_alloc` around the callback in debug builds.
- Callback load meter (time spent ÷ buffer duration), printed once per second by `engined`.
- Offline test: render 10 s of a deck playing a generated test signal to a WAV file with no audio device; assert sample-accurate seek and rate behaviour.
- One criterion bench of the offline render (ns per frame), to compare Mac vs Pi later.
- Cross-build `aarch64-unknown-linux-gnu` in CI (GitHub Actions), even though nothing runs there yet.

## Done when

- A track plays on the Mac; seek and rate changes are glitch-free.
- The load meter shows a number.
- `cargo test`, `cargo clippy -D warnings` pass; the cross-build is green in CI.

## Out of scope

Keylock, beatgrids, Rekordbox, library format, UI, controller, Pi audio tuning.

## Status

As of 2026-10-21, the code for step 1 is complete. What remains are the checks that need a Mac, the first CI run and, later, a Pi.

**Done**

- Workspace (resolver 3, edition 2024, GPL-3.0-or-later) with `engine-core`, `engine-media`, `engine-io` and `engined`.
- `engine-core`:
  - `Source` has per-chunk ready flags and a single `SourceWriter`.
  - `Transport` handles play, pause, seek, rate (negative and 0 included) and reverse. It has the slip shadow playhead: vinyl pause and reverse are slip-able, the playhead snaps to the shadow when the last of them ends, and a seek ends a slip.
  - Cubic-Hermite `Scaler`, `Deck`, and `Engine::process`, which takes commands through an rtrb ring.
  - Replaced sources leave the callback on a retire ring and are freed outside it.
  - State goes out through atomics, and a `LoadMeter` tracks the callback load.
  - When the playhead reaches an unloaded chunk, the block is silent, the transport holds and a starved flag/count is set. Playback resumes with no skipped audio.
  - Playhead jumps get a 128-frame crossfade (de-click). This was not in the plan; it was added for "glitch-free seek".
- `engine-media`: Symphonia decode with gapless on, mapping to stereo, and a rubato FFT resample to the device rate. The resample is streamed, its delay is trimmed and the output length is exact. A loader thread fills the source chunk by chunk.
- `engine-io`: the `AudioBackend` trait and `CpalBackend`. The callback always gets f32; other device formats are converted through a preallocated buffer.
- `engined`: all keys from the scope plus `--device`, `--buffer` and `--list-devices`. It prints the load meter once per second. Debug builds use the `assert_no_alloc` global allocator around the callback.
- Guardrails:
  - Offline render test: 10 s of a generated index ramp written to WAV. It asserts seek, +1 % rate, negative rate, rate 0, slip-reverse and slip-pause snap-back to the exact source position, under `assert_no_alloc`.
  - A starvation test and a de-click test.
  - Unit tests for Transport (reverse and rate 0 included) and Scaler.
  - Criterion bench `engine-core/benches/render.rs`.
  - CI (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, tests and bench build on Ubuntu and macOS, plus the aarch64 cross-build.

**Verified here (Linux x86_64, Ubuntu 24.04)**

- `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` and `cargo bench --no-run` all pass.
- The aarch64 cross-build ran locally with the same commands as the CI job, and the `engine-core`/`engine-media` tests pass under qemu.
- `engined` runs on the ALSA `null` device, debug and release, at the default buffer and at `--buffer 128`. It loads a 44.1 kHz file, resamples it to 48 kHz, plays it to the end and prints the load meter. In debug the callback ran under `assert_no_alloc` with no abort. Keys were driven through a pseudo-terminal.
- Bench on this x86 box: about 7 ns/frame at rate 1.0 and 25 ns/frame at rate 1.08 (interpolating). That is roughly 3 µs per 128-frame block against the 1.3 ms budget.

**Only verifiable on the Mac**

- Audible playback through CoreAudio, and checking by ear that seek, rate, reverse and slip changes are glitch-free.
- Load-meter numbers on a real device.
- The `macos-14` CI job. Nothing in the code is macOS-specific, but it has not been compiled on macOS yet.

**Only verifiable on the Pi**

- Running the cross-built binary. CI links against glibc 2.39; see `docs/dev-setup.md`.
- ALSA `hw:` at 128 frames with no xruns, RT priority and core pinning (spike 1).
- Bench numbers against the 1.3 ms budget.

**Known gaps, deferred on purpose**

- The scaler is cubic Hermite at every rate. The windowed-sinc kernel for normal play above 1.0× (to avoid aliasing) comes later.
- A loader thread that finishes early is not cancelled when a new track is loaded.
- Startup does not yet wait for the first chunk; until it arrives the deck reports starved blocks.
- State is per-field atomics, not a seqlock'd snapshot.
