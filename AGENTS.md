# AGENTS.md

Open-source standalone DJ player (XDJ-700 / XDJ-1000MK2-inspired): Raspberry Pi CM5, custom PCB and enclosure, own Rust engine, raw wgpu UI, RP2040 control surface. Development starts on macOS (Apple Silicon) and moves to the Pi later.

## Where the ground truth lives

Read these before designing anything. If code and docs disagree, the docs win until a human changes them; if the docs disagree with each other, the newer decision wins and you should flag it.

| What | Where | Use it for |
| --- | --- | --- |
| Engine research and decisions | [`docs/research/engine.md`](docs/research/engine.md) — exported from the living doc <https://claude.ai/code/artifact/271c7e09-9192-49d3-bcdb-abe21dd9447d> | Architecture, real-time rules, deck pipeline, keylock, sync, library format, sources, licensing, crate layout, spikes |
| Project findings | [`docs/research/findings.md`](docs/research/findings.md) — copy of the claude.ai project doc | Hardware, platter, UI design tokens, waveform design |
| Feature parity baseline | XDJ reference spec <https://claude.ai/code/artifact/29b73515-7daf-401f-aa8d-1b8cad02e09f> | Exact behaviours and value ranges to match (cues, loops, slip, tempo ranges, settings) |
| UI wireframes | <https://claude.ai/artifact/3qgXr2k9inEpQf5V5AMuo8> | Screen layouts and waveform spec |
| Reference manual | XDJ-1000MK2 Operating Instructions (PDF): <https://rent.djcenter.ee/images/kasutusjuhendid/Pioneer-XDJ-1000MK2.pdf> | Primary source behind the reference spec |
| Local dev setup | [`docs/dev-setup.md`](docs/dev-setup.md) | Toolchain, test media, how to run |

The claude.ai links need the project owner's account. The exported copies in `docs/research/` are what you can always read; when the living doc changes, re-export it rather than editing the copy by hand.

## Decisions you must not quietly change

- Own engine in Rust; not Mixxx.
- One real-time audio callback owns all decks. It never allocates, frees, locks, logs or does I/O. Debug builds wrap it in `assert_no_alloc`.
- Tracks are decoded into RAM in chunks with ready flags before and during playback; the callback never waits on disk or network.
- `Transport` carries a slip shadow playhead from the start.
- Rekordbox libraries are **read-only**. All writes go to our own library format (snapshot + per-device append-only journals + content-addressed blobs), on the USB stick or a cloud folder.
- No internal storage by default: RAM is the working cache, the stick (or cloud) is the persistent store.
- Never hard-code 48 kHz; resample at load to the device rate.
- License: GPL-3.0-or-later.

## Current step

Step 1 — "one deck plays" vertical slice on macOS. Scope and done-criteria are in [`docs/plan/step-1.md`](docs/plan/step-1.md).

## Working conventions

- Workspace crates live in `crates/`. `engine-core` has no I/O and no threads, so it can be tested offline.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass before a commit.
- Keep commits small and descriptive. Don't commit audio files.
