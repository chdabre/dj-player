# AGENTS.md

Open-source standalone DJ player (XDJ-700 / XDJ-1000MK2-inspired): Raspberry Pi CM5, custom PCB and enclosure, own Rust engine, raw wgpu UI, RP2040 control surface. Development starts on macOS (Apple Silicon) and moves to the Pi later.

## Where the ground truth lives

Read these before designing anything. If code and docs disagree, the docs win until a human changes them; if the docs disagree with each other, the newer decision wins and you should flag it.

| What | Where | Use it for |
| --- | --- | --- |
| Engine research and decisions | [`docs/research/engine.md`](docs/research/engine.md) — exported from the living doc <https://claude.ai/code/artifact/271c7e09-9192-49d3-bcdb-abe21dd9447d> | Architecture, real-time rules, deck pipeline, keylock, sync, library format, sources, licensing, crate layout, spikes |
| Project findings | [`docs/research/findings.md`](docs/research/findings.md) — copy of the claude.ai project doc | Hardware, platter, UI design tokens, waveform design |
| Feature parity baseline | XDJ reference spec <https://claude.ai/code/artifact/29b73515-7daf-401f-aa8d-1b8cad02e09f> | Exact behaviours and value ranges to match (cues, loops, slip, tempo ranges, settings) |
| UI wireframes | <https://claude.ai/artifact/3qgXr2k9inEpQf5V5AMuo8> | Screen layouts at the display's 1024×600 (canvas version 28 and later) and the waveform spec |
| Spikes | [`docs/spikes/`](docs/spikes/) | Results and decisions of the keylock shootout, the waveform research and the UI shell |
| Reference manual | XDJ-1000MK2 Operating Instructions (PDF): <https://rent.djcenter.ee/images/kasutusjuhendid/Pioneer-XDJ-1000MK2.pdf> | Primary source behind the reference spec |
| Local dev setup | [`docs/dev-setup.md`](docs/dev-setup.md) | Toolchain, test media, how to run |

The claude.ai links need the project owner's account. The exported copies in `docs/research/` are what you can always read; when the living doc changes, re-export it rather than editing the copy by hand.

Known gap (2026-10-21): `docs/research/findings.md` predates these decisions, and the project doc it copies still needs them (newer decisions win):
- the display is 1024×600, not the Pi Touch Display 2 (720×1280) or 1280×768;
- waveforms are always our own;
- the 3-band default palette is blue / orange / white, replacing indigo / teal / cream.

## Decisions you must not quietly change

- Own engine in Rust; not Mixxx.
- One real-time audio callback owns all decks. It never allocates, frees, locks, logs or does I/O. Debug builds wrap it in `assert_no_alloc`.
- Tracks are decoded into RAM in chunks with ready flags before and during playback; the callback never waits on disk or network.
- `Transport` carries a slip shadow playhead from the start.
- Rekordbox libraries are **read-only**. All writes go to our own library: one SQLite file (`DJPLAYER/library.db`) plus content-addressed blobs, on the USB stick or a cloud folder (decided 2026-10-21, replacing the journals).
- No internal storage by default: RAM is the working cache, the stick (or cloud) is the persistent store.
- Never hard-code 48 kHz; resample at load to the device rate.
- License: GPL-3.0-or-later. AGPL-3.0 components are accepted (decided 2026-10-21): the TempoCNN model and the `hpcpkey` port in `engine-analysis` (see `docs/plan/analysis.md`).
- Waveforms are always computed by us, during decode, for every track. Rekordbox waveform tags are only a placeholder until ours exists (`docs/spikes/waveforms.md`).
- The UI is raw wgpu + glyphon on winit. GPUI / GPUI Kit was considered and rejected on 2026-10-21 (`docs/spikes/ui-shell.md`). Colours and sizes come from the theme (`crates/ui/src/theme.rs`), never literals in screen code.

## Current step

Our own library (the overlay library on the stick or a cloud folder: one SQLite file plus content-addressed blobs), so that cues, analysis and waveforms persist. Built on SQLite after a spike: [`docs/plan/library.md`](docs/plan/library.md). Earlier steps:

- **Step 1:** code-complete ([`docs/plan/step-1.md`](docs/plan/step-1.md)).
- **Step 2:** the engine part is done; known gaps are in its Status section ([`docs/plan/step-2.md`](docs/plan/step-2.md)). Analysis work is in [`docs/plan/analysis.md`](docs/plan/analysis.md).
- **UI shell spike:** the main screen at 1024×600 with real waveforms ([`docs/spikes/ui-shell.md`](docs/spikes/ui-shell.md)).
- **Keylock:** Bungee Basic ported to pure Rust is the only keylock (PR #2); Signalsmith is gone.

## Working conventions

- Workspace crates live in `crates/`. `engine-core` has no I/O and no threads, so it can be tested offline.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass before a commit.
- Keep commits small and descriptive. Don't commit audio files.
