# Codebase structure (proposal, 2026-10-22)

A housekeeping pass across every crate, done after the UI refactor (`ui-architecture.md`) and before the next features land: the Rekordbox overlay, history and play counts, memory cue CALL, hot cue bank B, a second deck sharing one library per stick, and the RP2040 reader. It also answers when the UI and the player become two processes.

**Status:** proposal; nothing here is built yet.

## What hurts today

- **A few files hold several jobs each.** `library/src/library.rs` (1,785 lines: mount, transactions, damage, edits, playlists, identity, reads, blobs and all the SQL), `engined/src/browse.rs` (1,534), `engined/src/lib.rs` (1,351: options, the `Player`, cue sync, batch bookkeeping, the live waveform, the analysis pipeline), `engined/src/persist.rs` (1,098: worker protocol, a 305-line event loop, record operations, volume location, the device id), `engine-media/src/lib.rs` (960, one file, with tags after the test module), `ui/src/standin.rs` (932, a 393-line `build`).
- **Everything is single-deck underneath.** `DECK = 0` in `engined`, the library worker's one `Current` track and its implicit current-track requests, `Surface` with one sample rate. Each of these has to change for the second deck, and each is easier to change once it is in its own module.
- **Cycles and reach-ins.**
  - Inside `engined`, `persist` uses `browse::walk` and `browse` uses `persist::{LIBRARY_DIR, Listing, Playlists, stored_path}`.
  - `engined` builds file hashes from `library::hash` internals.
  - `engined` builds the analyzer chain itself; that is policy `engine-analysis` should own.
  - The UI hard-codes `"library.db"`.
  - The renderer depends on `engined::browse::Thumb`.
- **The same thing in several shapes.**
  - Hot cues come in four shapes, beats in three and loops in two.
  - A constant 4/4 grid is built twice, and `60/bpm·sr` is written in four places.
  - The gapless trim is computed twice (`rekordbox::timeline_offset`, `Timeline::trim_seconds`).
  - The crossfade is written three times (`deck.rs` twice, `keylock.rs` once).
- **Public surface wider than its use.**
  - `engine-core` makes every module `pub mod` and also re-exports its items, so each item has two paths. Outside the crate only `Engine`, `EngineHandle`, `DeckCommand`, `DeckState`, `Source`, `DeckGrid` and `TrackCues` are used.
  - `library` exports `db` and `position`, which nothing outside the crate uses.
  - `engined` exports worker types that only its tests use.

## Bugs found on the way (fix first)

1. **Rate is unbounded.**
   - `Transport::set_rate` accepts any finite rate.
   - Above about 8× on a short loop, a block wraps more than `MAX_SEGMENTS` (16) times, and `Segments::push` silently drops the rest.
   - The last piece then reads past the loop end.
   - Fix: clamp the rate to what the deck supports, and test that a block with more wraps than `MAX_SEGMENTS` still stays inside the loop.
2. **Panic in the callback.** `Deck::render` has `assert!(frames <= MAX_BLOCK_FRAMES)`. Make it a `debug_assert!`; `Engine::process` already splits the buffer into blocks.
3. **`SetCues` by value.** `DeckCommand::SetCues` carries `TrackCues` by value (about 170 bytes), so every slot of the ring is that size. A stale one is dropped in the callback. That is harmless while `TrackCues` is `Copy`, but memory cues for CALL would make the drop a free on the audio thread. Move it to `Arc` plus the retire ring now, as `SetGrid` already does, before memory cues land.
4. **Retire pushes ignore failure.** `let _ = self.retire.push(..)` in `engine.rs`. Only the earlier `slots()` check keeps an `Arc` from being freed in the callback. Make the invariant explicit with a `debug_assert!` and a comment.

## UI and player in one process: split now?

**Draw the boundary now; run two processes later.**

The design already fixes where this ends: one `engined` per host, with the UI as a client. Fast state goes through a seqlock'd shared-memory block, and commands and slow state go over a Unix socket (`docs/research/engine.md`, "Process and threads"; `docs/spikes/ui-shell.md`, step 2). The question is only when.

Why not now:
- **The pay-off is on the device.** Crash isolation (a UI panic must not stop the music), separate scheduling and a UI frame stall that cannot touch the engine all matter on the Pi. On the Mac, one process is easier to debug and profile, and there is no Pi bring-up yet.
- **The protocol is about to grow.** The Rekordbox tree, history, settings, memory cues and a second deck will each add messages. A wire format frozen today would be reworked several times.
- **It is more than commands.** A socket has to carry the live waveform and stored waveforms, the browser's listings and artwork, scan and analysis progress and the save state. The live waveform also needs shared memory.

Why the boundary is worth drawing now:
- **The UI reaches into the player.** It imports `engined::Player`, `engine_io::CpalBackend`, `engined::browse::Browser`, `engined::persist::{Listing, Playlists}` and `library::Edit`, and calls about 20 `Player` methods, most of them library, scan and analysis calls. It reads them in `frame.rs`, `app/mod.rs` and `platform.rs`, not only in `app/engine.rs` as `ui-architecture.md` intended.
- **The browser runs in the UI's process.** It reads the stick, the tags and the artwork there; the UI's `Track` even reads tags and the thumbnail on the UI thread at load. In two processes the browser is a request/reply service of the engine.
- **The control surface is on the wrong side.**
  - `ui/src/app/surface.rs` turns jog, fader and SEARCH input into scratch, bend and rate.
  - The design has the RP2040 talking to `engined` directly, "so jog latency is bounded by the engine, not by UI frame timing".
  - That interpretation belongs on the engine's side. The UI's stand-in and dev keyboard then send the same `ControlEvent`s the board will send.

So:
- Define the port as plain data in its own crate (below).
- Make the UI depend on that crate alone, plus `waveform` for drawing.
- Implement the port in-process with today's `Player`.

The process split then becomes swapping the transport behind the port. Do it at Pi bring-up, or when the RP2040 reader lands in `engined`, whichever comes first.

## Target layout

### New crates

| Crate | Holds | Depends on |
| --- | --- | --- |
| `surface` | `Control` (`#[repr(u8)]`, with a deck side), raw `ControlEvent` (knob detents, jog counts, fader as u16), frame encode/decode. `no_std`, no dependencies. | nothing |
| `rekordbox` | `export.pdb` pages and rows, ANLZ tags (cues, beat grid, waveform placeholder), and the XML reader moved out of `engine-analysis/src/rekordbox.rs`. Read-only, with no engine or SQLite dependency. | nothing heavier than `quick-xml` |
| `player-api` | The port: `Command` (deck commands and control events, library edits, browse requests, scan and analysis), `DeckSnapshot`, slow-state events (`Listing`, `Playlists`, `Persistence`, `BatchProgress`, browse results, artwork as plain RGBA), and the `Port` trait. Plain data, serialisable-ready. | `surface`, `library` (ids and edit types only) |

The scorer's ground-truth helpers (`Overrides`, `truth()`) stay in `engine-analysis` and call the `rekordbox` crate. `timeline_offset` takes `start_trim` and the sample rate as numbers, so `rekordbox` stays a leaf.

A shared `track-model` crate (one beat, grid, cue and key vocabulary for analysis, waveform, library and the overlay) was considered. It is left until the Rekordbox overlay shows which shapes it needs. For now `waveform`'s `GridBeat` and the duplicated constant grid are folded into the existing types.

### Per crate

**engine-core** (mechanical unless marked):
- `transport/`: `mod.rs` (the core and `advance`), `segments.rs`, `slip.rs`, `scratch.rs`, `loops.rs`. Also fix the stale module doc on what is slip-able.
- `deck/`: `command.rs` (`DeckCommand`, `BlockOutcome`), `mod.rs` (state, setters, `apply`), `render.rs` (`render` split into outgoing, readiness, parts and wrap fade), `publish.rs`.
- `fade.rs`: one crossfade for the three copies.
- `keylock/`: `mod.rs`, `fifo.rs`, `grain_path.rs`.
- `engine.rs` stays the callback; `command.rs` (rings and retire) and `handle.rs` (`EngineHandle`) move out.
- `render.rs` (test and bench helpers) moves behind a `testing` feature.
- **Visibility:** private modules; `pub` only the items used outside the crate.
- **Helpers:** one `beat_period(bpm, rate)` helper.

**engine-media** (mechanical):
- Split `lib.rs` into `error.rs`, `cancel.rs`, `decode.rs` (with one shared probe helper), `id3.rs`, `resample.rs`, `load.rs`, `tags.rs` and `artwork.rs`.
- `tags()` stops probing the file twice.
- Add a tagged fixture test before moving `tags.rs`, because nothing tests it today.

**engine-analysis:**
- `model/` (`feature`, `tempo`, `grid`, `key`, `provenance`); `TrackAudio` into `audio.rs`. Split `gridfit/` and `hpcpkey/` at their existing seams.
- **API:** the analyzer chain and the merge of results move here from `engined` (`default_chain(settings)`, `merge`). `stored.rs`, the JSON codec for analysis results, moves here from `engined`.
- `select::Policy` is used only in its own tests: wire it in as the merge, or delete it.

**waveform** (mechanical):
- `model.rs` (types out of `compute.rs`), `filter.rs`, `theme.rs`, `gain.rs`, and `render/` (`canvas`, `markers`, `zoom`, `overview`).
- `beat_period` uses `FALLBACK_BPM`.

**library:**
- **Split the big file.** `library.rs` becomes `library/{mod, types, txn, recover}.rs`, plus `ops.rs` (`Op` and its SQL, the replication unit for the cloud later), `identity.rs` (find, create, merge, resolve; "remove from library" lands here), `edits.rs`, `playlists.rs`, `read.rs`, `blobs.rs` (blob GC lands here) and `time.rs`. All mechanical.
- **Break up the long functions.** `Op::exec` (244 lines), `apply_playlist` (143) and `track` (122) split with the move. The duplicated upsert column lists, position lookups, playlist row mapping and transaction plumbing get one helper each.
- **New APIs for `engined`.** `FileHash::of_file`, `AnalysisRef::new` and `has_library(root)`. Path policy (`stored_path`) moves here.
- **Narrower surface.**
  - `db` and `position` become private.
  - Modules are not exported twice; `hash::BLOCK` is re-exported instead.
  - `FileHash::parse` and `AudioHash::hash_hex` are deleted.
  - `Edit` stays one type; its two `unreachable!` become an internal split.

**engined:**
- **`lib.rs`** becomes declarations and re-exports.
- **`player/`:** `mod.rs` (`Player`, start, load, poll), `options.rs`, `track.rs`, `cues.rs` and later `history.rs`.
  - **API:** a private `Deck { loaded, lib, cue_serial }` indexed by deck, in place of `DECK`.
  - **API:** cue restore, auto cue and write-back become a `CueSync` over `EngineHandle`, tested offline. Today only the `#[ignore]`d device test covers them.
- **`analysis/`:** `analyze_with` and the background job; most of it shrinks once the chain lives in `engine-analysis`.
- **`batch.rs`** takes `BatchProgress` and `poll_batch`.
- **`waveform_live.rs`** gets `LiveWaveform` and `WaveTap`. `batch.rs` hand-rolls the waveform build at the file rate, while the tap builds at the engine rate; check that the two agree.
- **`store/`** (renamed from `persist`):
  - `mod.rs` holds the protocol and `LibraryWorker`.
  - `worker.rs` holds the event loop as a `WorkerState`, with one method per request and `mark_dirty()` in place of the 8 copied deadline lines.
  - `records.rs` and `locate.rs`.
  - Later `rekordbox.rs`, the overlay at mount.
  - **API:** requests name a `TrackId` or `(deck, serial)` instead of an implicit current track.
- **`stick.rs`** holds the walk, audio extensions and `PIONEER/` detection, which breaks the `browse` ↔ `persist` cycle. **`config.rs`** holds the device id and later the player settings.
- **`browse/`:** `mod.rs` (navigation and search), `level.rs`, `view.rs` (a `LibraryView` over listing and playlists; the Rekordbox tree becomes a second view), `order.rs` and `tags.rs` (the tag and artwork thread). The tests move with their modules.
- **`surface.rs`** moves here from the UI, with its tests.
- **Implements `player_api::Port`.**
- **Dead code** to delete: `Player::open`, `LibraryWorker::spawn`, `persist::describe`, `Loaded::file_hash` and `Analysed::from_library`.
- **Test-only items** go behind a `testing` feature.

**ui:**
- **Depends on** `player-api`, `surface` (through the port) and `waveform`. It no longer depends on `engined`, `engine-core`, `engine-io`, `engine-analysis` (already unused) or `library`.
- **`app/engine.rs` is the only user of the port.** `frame.rs`, `app/mod.rs` and `platform.rs` stop reading the player. `App::frame` (173 lines) splits into a library sync, startup requests, view building and capture readiness.
- **`screens/mod.rs`** is reduced to the types; `router.rs` (`Screens`, `Cx`, `Show`), `view.rs` and `tests/{layout,input,fixtures}.rs` move out.
- **`screens/deck/`:** `state`, `zoom`, `overview`, `status` and `cues` (memory CALL lands in `cues`). `DeckState` is renamed `DeckDisplay` so it no longer clashes with the engine's type.
- **`playlists.rs`** moves under `screens/browse/editing/`. Its menu stops walking the filesystem: that becomes a browse request.
- **`standin/`:** `mod.rs` (input), `layout.rs`, `draw.rs`. Its own `hit`, `label` and `rule` go in favour of `kit`, and the tuples become `Rect`s.
- **Chrome is composed, not copied.**
  - `chrome::frame(ui, Chrome { .. })` replaces the copies of the same frame calls in the main, info, browse and perform screens.
  - Its geometry becomes `pub const`s. That removes the repeated `113.0` and `703.0` (in 10 files) and the inline section `Rect`s.
- **The colour-literal test** checks every file in `src/`, not a hand-written list.
- **`render/`:** `Surface` is renamed `PlaneSet` (it clashed with `app::surface::Surface`) and moves with `quad_pipeline` to `plane.rs`; coordinate helpers go to `coords.rs`.
- **Dead targets and controls** are wired or deleted: `DeckTarget::{Delete, Memory, Grid, …}` and `Control::{TimeMode, TrackNext, F1, F2}`.

## Order

Each step is its own commit or commits and passes `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` on its own.

1. **The four fixes above,** with tests.
2. **Mechanical splits, one crate per commit:** `engine-core`, `engine-media`, `waveform`, `engine-analysis`, `library`, `engined`, `ui`. These are moves only, plus narrowing visibility and deleting dead code. Steps 1 and 2 can run in parallel per crate, because no two crates share files.
3. **Duplication helpers:** fade, `beat_period`, the library's SQL helpers, the worker's `mark_dirty`, the cue send. The new library APIs, and `engined` stops reaching into `library::hash`.
4. **`stick.rs` and `store/`** (the cycle broken); the analysis chain into `engine-analysis`.
5. **`rekordbox` crate** (the XML reader moves; pdb and ANLZ come with the overlay work).
6. **`surface` crate; `surface.rs` moves to `engined`.**
7. **`player-api` and the `Port`.** The UI moves onto it, and `Browser` goes behind it as requests and replies. This is the largest step, and the UI's `Cargo.toml` shows when it is done.
8. **Deck-indexed internals:** `Deck` in `engined`, worker requests by track, and `CueSync` tested offline. No second deck yet; just no `DECK = 0`.

After step 8:
- no source file over about 600 lines outside tests;
- no function over about 100 lines;
- no crate cycle;
- the UI's dependencies are the port and drawing.

## Not changing

- The real-time rules, the callback's ownership of all decks, the chunked RAM decode and the slip shadow playhead.
- One SQLite file plus blobs; Rekordbox read-only.
- raw wgpu + glyphon on winit, immediate mode, hand layout.
- The two-process design itself; only its timing is decided here.

## Open questions

- **`player-api` or `engine-api`?** The port carries library and browse traffic as well as the engine's, so "player" fits better.
- **`Edit` as one type or `TrackEdit` / `PlaylistEdit`?** It stays one type for now; split it when history and tag edits arrive, if the match keeps growing.
