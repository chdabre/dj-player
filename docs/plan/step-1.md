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
