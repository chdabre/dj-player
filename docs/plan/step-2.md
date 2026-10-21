# Step 2 — analysis in the deck, cues, loops, quantize

Goal: a loaded track gets its beatgrid, tempo and key from our own analysis, and the deck uses the grid for quantized cues, hot cues, loops and beat jumps, as the XDJ reference spec describes them. Engine and `engined` CLI only; no UI, no stored library yet.

Behaviours and values come from the XDJ reference spec (Playback & performance). Analysis is `crates/engine-analysis` (see `docs/plan/analysis.md`).

## Scope

1. **Analysis on load (`engined`).** A worker thread decodes the file at its own rate and runs `tempocnn`, `gridfit+tempocnn` and `hpcpkey` (whatever is available). The grid, converted to engine frames, reaches the deck through the command ring; the replaced grid leaves through the retire ring, like sources. Until the grid arrives, grid-dependent actions use the fallbacks below. Later, stored results (Rekordbox, our library) take precedence through the existing `Policy`.
2. **Deck grid (`engine-core`).** Beat positions in engine frames with bar positions; beat at/before/after a position, nearest grid point at a resolution, BPM at a position. Published state: track BPM, current BPM (track BPM × rate), key, beat and bar phase.
3. **Quantize.** On/off, resolution 1/8, 1/4, 1/2, 1 beat. Applies to cue set, loop in/out, hot cue set and call. Without a grid, quantize does nothing.
4. **Cue.** Set when paused (quantized). CUE while playing jumps back to the cue and pauses. Holding CUE while paused at the cue previews from it; releasing returns to the cue and pauses; PLAY while holding continues playing. Real-time cue: IN while playing sets the cue (and the loop in point).
5. **Hot cues A–H.** Set on an empty pad at the current position (quantized); calling a set pad jumps there and keeps the play state; delete clears a pad. Hot-cue loops, banks and colours come with the library.
6. **Loops.** Manual IN / OUT (quantized), RELOOP/EXIT. Auto beat loop of 1/2, 1, 2, 4, 8, 16 beats from the current (quantized) position; IN long-press = 4 beats. LOOP 1/2X and LOOP 2X keep the in point. Without a grid, beat lengths use 130 BPM, as the reference does. A loop is slip-able: the shadow keeps running, and exiting snaps back.
7. **Beat jump.** ±1, 2, 4, 8, 16, 32 beats; with a loop active, it moves the loop instead.

## Real-time design

- `Transport::advance` returns up to a fixed number of segments per block, so a loop wrap lands on the exact frame inside a block. The deck renders them in order and crossfades ~64 frames across each wrap, rendering a little past the loop end for the outgoing side.
- With keylock on, a wrap re-primes the stretcher and crossfades (one stretcher prime per wrap). A seamless keylocked loop, with the stretcher's read-ahead wrapping too, is a later refinement.
- All grid lookups in the callback are binary searches over a preallocated slice; no allocation.

## Done when

- `engined <file>` prints the analysed BPM, key and first downbeat after load, and the deck uses that grid.
- Offline tests, sample-accurate where it matters: quantized cue/hot cue/loop points land on grid lines; loops wrap on the exact frame for 1/2 to 16 beats at non-integer frame periods; beat jump and loop move; LOOP 1/2X / 2X; cue press/hold/release states; slip snap-back after a loop; no allocation in the callback.
- `engined` keys for cue, hot cues A–D, loop in/out/exit, beat loop 4, halve/double, beat jump ±4, quantize toggle.
- fmt, clippy `-D warnings`, tests pass; CI green.

## Out of scope

Memory cues and storing anything (needs the library), hot-cue loops/banks/colours, auto cue on silence, cue/loop fine adjust (UI), slip auto-end rules (hot cue 4 beats, reverse 8 beats) and the 8 s slip-loop limit, beat sync, grid adjust.

## Status

As of 2026-10-21 the engine part is done: grid on load, quantize, cue, hot cues A–H, loops (IN/OUT, RELOOP/EXIT, beat loops, 1/2X and 2X, slip), beat jump and loop move, with `engined` keys. Offline tests (`engine-core/tests/cues_loops.rs`) check frame-exact wraps for 1/2–16 beats at a non-integer beat period, reverse wraps, slip snap-back, quantized cue and hot cue, beat jump and a click-free wrap, all under `assert_no_alloc`.

Known gaps: holding CUE to preview cannot be tried in a terminal (no key releases; the engine supports it); with keylock on, a loop wrap re-primes the stretcher with no crossfade (the 64-frame wrap fade is resampler-only), so keylocked loops can click at the wrap and cost one prime (~0.6 ms on the Mac) per wrap; the fix is the stretcher's read-ahead following the loop; listening on the Mac is still to do.
