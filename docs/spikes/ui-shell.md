# UI shell (spike)

Status 2026-10-21: the main playback screen runs on the Mac at the panel's 1024×600 and drives one deck of the real engine in-process. Measured on the Mac only. The Pi run is still to do.

## Short answer

- **Raw wgpu + glyphon is enough for the shell.** We wrote two pieces ourselves: one pipeline that draws rounded rectangles, and a cache that keeps shaped text between frames. With them the whole main screen (about 150 rectangles and 50 text runs) takes **0.8 ms of CPU per frame** on an M1 Max at **120 fps**. Layout takes 0.03 ms of that. Most of the rest is glyphon's text preparation and the GPU submit.
- **Frame pacing:** FIFO present with `desired_maximum_frame_latency: 1`. In steady state no frame is late (none over 1.5× the median interval). Nearly all of each frame is spent blocked in `get_current_texture`, about 7.4 ms of an 8.3 ms frame. That is the vsync wait. So an input lands on screen within about one refresh plus the compositor.
- **Playhead interpolation works.** The engine publishes its position once per audio callback, which is not in step with vsync. The UI extrapolates from when it last saw the position change, and pulls the shown playhead towards that estimate with a 50 ms time constant. With a 1024-frame buffer at 120 Hz, the per-frame step error is **p99 0.76 ms of audio** (a frame step is 8.3 ms). Seeks, cues, loop wraps and pause snap straight to the engine.
- **GPUI / GPUI Kit: rejected (owner, 2026-10-21).** It would replace our renderer, not add to it. It has no public hook for our own shader passes, which the waveforms need, and no documented touchscreen input. It also decides its own frame pacing.

## What it is

`crates/ui`, binary `dj-ui`:

```
cargo run -p ui --release -- <file> [--play] [--at <s>] [--buffer <frames>] [--fullscreen]
cargo run -p ui --release -- <file> --at 60 --screenshot target/tmp/ui-main.png
```

- **Screen:** `screen.rs` lays out the main screen ("1 · Normal playback", wireframe canvas version 28) from the canvas's pixel values. It is a pure function from deck state to a scene plus touch targets, and it is unit-tested: everything stays on the panel, touch targets don't overlap, and every target is at least 36 px.
- **Renderer:** `gfx.rs`. Rectangles are instanced quads; the fragment shader uses a signed distance function for the corner radius, border and anti-aliasing. Colours are sRGB and get linearised for an sRGB surface. Text is glyphon, with Barlow Condensed embedded. Shaped buffers are reused until their text changes. Layout happens in design pixels; one uniform scales and centres it, so a Retina window and the panel draw the same scene. `--screenshot` renders the frame offscreen at exactly 1024×600, which is what the panel will show.
- **Engine:** the device, decode, analysis and stream wiring moved out of `engined`'s `main.rs` into `engined::Player`, shared by the CLI and the UI. For the spike the UI reads `DeckState` atomics directly. The planned split (UI as a client, a seqlock'd shared-memory block for fast state, a socket for commands) is unchanged; nothing here depends on running in-process.
- **Input:**
  - mouse and touch (winit `Touch`) on SLIP, CALL ◀ ▶ (beat jump ∓4, or loop ½X / 2X in a loop), ZOOM, and the overview for needle search, with drag;
  - keys as in `engined`, plus a real CUE hold: winit reports key releases, so holding CUE at the cue point previews, which the terminal could not do.
- **Waveforms:** real, from the `waveform` crate (see "Waveforms" below). The zoom view marks the analysed beat grid with ticks along its top and bottom edges, drawn over the waveform: white for beats, solid red for downbeats. The loop, cue, slip shadow and centred playhead are drawn on top. The overview adds loading progress and 16-bar ticks. `w` cycles 3-band → RGB → single colour; `--wave-mode` and `--wave-theme` (default, findings, spectrum, blended) pick them at start.

## Hardware stand-in (debug shell)

Until the control surface exists, the window draws the player's physical controls around the device screen, so the prototype can be played on a laptop. The layout is the canvas artboard "8 · Hardware stand-in" (1512×950, the reference player's controls in their real order). It is a debug tool only:

- **A separate layer, discarded for the device.** It lives in `standin.rs` behind the cargo feature `standin` (on by default). The device screen is rendered on its own into an offscreen texture the size of the shell's display slot, the shell is drawn to the window, and the texture is copied into the slot (`gfx::ShellFrame`). The device layer never knows about the shell.
  - `--no-standin` turns it off at run time: the device screen alone, byte-identical to the build without it.
  - `cargo build -p ui --no-default-features` leaves it out entirely, as the device build will.
- **The controls send the same deck commands** the RP2040 control surface will:
  - CUE (hold to preview), PLAY/PAUSE, REV, QUANTIZE, KEYLOCK, loop IN / OUT / RELOOP;
  - hot cue pads A–H (right click or shift deletes); track search back to the start;
  - SEARCH scrubs at 4x while held;
  - the jog's top plate scratches (one turn = 1.8 s, a record at 33⅓ rpm), its ring bends the pitch by its speed, and with JOG MODE off the top plate bends too;
  - the tempo fader (up is slower), with TEMPO cycling ±6 / ±10 / ±16 / WIDE.
- **Lights follow the deck:** the CUE and PLAY rings, the quantize and keylock lights, REV, the loop buttons, the set pads, the fader cap (it follows the rate, bends included), and a marker that turns with the track on the jog.
- **Browse controls:** BACK goes up a level (leaving the browser at its root); the browse knob loads or opens the selection when pushed, and turns when dragged up or down (one step per 18 px) or under the mouse wheel; its ▲ / ▼ step once. Pushing it on another screen opens the browser.
- **Not wired yet:** time mode, track search forward, TAG TRACK, F1 and F2. The keyboard keys still work; `+` / `−` now nudge the rate, so they agree with the fader.
- **Tests** (`standin.rs`): CUE hold and release, pads set and delete, the fader's rate (top = −10 % on ±10), a quarter turn of the plate scratching a quarter of 1.8 s, ring bend and its release, SEARCH while held, and the display slot staying clear of the controls.
- **Cost:** about 1.5 ms of CPU per frame with the shell at 100–114 fps on the M1 Max (1.2 ms without).

## Browse screen

`LIBRARY_DIR=<dir>` makes a folder the root of a stick (see `docs/dev-setup.md`); the browse screen ("Browse" on the canvas) lists it. Without a file the app starts there. Tab, the BROWSE tab or a push of the stand-in's knob opens it. The rail picks the source: USB (the folders on the stick) or LIBRARY, which opens on a menu: FOLDERS (the library's tracks in their folders, from the stored paths) and TRACKS (one flat list). A search from a library folder looks under that folder only.

- **Scan question:** a stick with no library asks once, over the list: SCAN (then LIBRARY fills in as the scan goes, with SCANNING n / total in the header) or FOLDERS ONLY. Enter / Esc answer it.
- **Header chips:** SORT cycles name, title, artist, BPM, key, time (folders stay first; unknown values sort last); SEARCH opens the search; SCAN (in LIBRARY) scans again.
- **Search:** everything under the shown folder, or the whole library, matched by file name and tags, every word. An on-screen keyboard (laid out as a computer's: five staggered rows of 11 units, 37 px tall: digits and ⌫; QWERTY; ASDF, centred; SHIFT, ZXCV, `-`, `.` and `'`; CLEAR, `&`, a wide space bar and DONE bottom right, as on a phone. SHIFT capitalises the next letter, a second tap locks it as CAPS, a third turns it off; letters show the case they type; a new name starts with SHIFT on) replaces the status band and the bottom strip, so all seven rows stay visible; a real keyboard types too, `/` opens it, Esc / BACK ends it.
- **Rows:** BPM (ours once analysed), key and length at the right of the artist column, in the row's font and weight. Artwork in the jacket column: the front cover from the tags, scaled to 64×64 on the tag thread, drawn by an image pipeline in `gfx.rs` (one texture per picture, dropped after 600 frames unused).
- **ANALYZE ALL:** in LIBRARY, while tracks lack our analysis, the header offers ANALYZE ALL; running, it shows "ANALYZING n / total" and a lit STOP. Results fill in BPM and key as they come (the listing is republished within a second of each). On the Mac the UI stayed at 120 fps through a 12-track run.
- **Playlists** (`docs/plan/playlists.md`): PLAYLISTS in LIBRARY, the TAG LIST tab, TAG TRACK (stand-in button, `x`, long press or `shift+x` removes), tag dots on tagged tracks, and the MENU tab: a modal list of actions over the browse list (five rows, scrolling), the playlist picker, names typed on the search keyboard (the header reads "NEW PLAYLIST: …"), a confirmation drawn as a destructive button before DELETE and REMOVE ALL, and MOVE with the knob. `ui/src/playlists.rs` holds the choices (tested against a real browser); `main.rs` carries them out.
- **One column without tracks:** a level with no tracks (the library menu, a folder of folders) is one column, the names across the full width and no divider; the title/artist divider appears where the level holds tracks (and always beside the info panel).
- **Cursor:** stays on its entry when the list re-sorts as tags arrive; a list nobody has touched stays at its top.
- **INFO in the browser** (the tab, or `i`), as on the players: the list narrows to its jacket and title columns and a panel beside it shows the selected track: the artwork (288×288, read on the tag thread for the selected track only; the list's thumbnail stands in until then) with title, artist, album and genre; BPM, key, time and rating (stars); the comment. The knob and ↑/↓ keep moving the cursor and the panel follows; push / Enter loads; BACK or INFO again closes it. BPM and key are ours once analysed; the rating is the library's, else the tag's.
- **INFO screen** (INFO from any other screen): the loaded track, full width. Laid out denser than the canvas's "Info" artboard (which keeps the fields in a narrow list and leaves a third of the panel empty): a header with source, player and "ON DECK"; the artwork as tall as the panel (273 px, from a 288 px read); beside it title, artist, album · genre, large BPM / KEY / TIME / RATING readouts, and the comment across the rest. Its tags and artwork are read once when the track loads, so it works without a stick too. BACK, Esc, `i` or INFO return to the main screen. The canvas artboard should follow.

- **Model in `engined::browse`:** the folder levels with a cursor and the first shown row each, folders first then tracks (sorted by name, case aside), and a tag reader thread that reads the shown folder's tracks first and keeps the results. No scan, no library writes: a track gets its record when it is loaded. Tests: listing and hiding, navigation with the cursor kept in view, scrolling, tags arriving.
- **Screen in `browse.rs`:** the rail, the tabs (BROWSE lit), seven 40 px rows (jacket, title, artist with BPM and length at the right), the status band and the overview, as on the main screen. The loaded track's title is in the accent colour; a thin bar shows where the rows are in a long folder. Long titles end in an ellipsis: text can now have a maximum width, ellipsized by cosmic-text.
- **Touch:** tapping a folder opens it; tapping a track selects it, tapping it again loads it (one tap can't replace a playing track by accident). A drag on the list scrolls it with the finger, and the wheel scrolls it. The header goes up a level.
- **Keys:** ↑ / ↓ move, Enter loads or opens, Esc / Backspace go up (Esc no longer quits; `q` does), Tab toggles the screen. Loading shows the main screen.
- **Run-time loading:** `Player::start` opens the output with an empty deck and `Player::load` replaces the track: new source, waveform, analysis and library record (`LoadedTrack`), one library worker for the whole session. The hot cues of a track come back when it is loaded again after another (`engined/tests/player.rs`, ignored by default since it needs an audio device).
- **Selected row tint:** the wireframe's 22 % accent looks almost solid with linear-light blending; 9 % matches the canvas.
- **Text is drawn above every rectangle and picture**, so anything over the list (the scan question) hides the rows instead of covering them. A modal layer in the scene would lift that.
- **Open:** library categories (artist, album, BPM, key), and the on-screen keyboard's feel on the real panel.

## Scrubbing the waveform

Touching the zoom waveform (on either screen) takes the track; dragging moves it with the waveform under the finger (drag left to play forward); letting go releases it.

- **Engine: a scratch mode in the transport** (`ScratchBegin` / `ScratchBy(frames)` / `ScratchEnd`), meant for the jog wheel too.
  - The playhead is pulled to the hand's target by a critically damped spring (τ = 10 ms), solved exactly per block. Jittery touch updates play as smooth record movement, the track eases to a stop when the finger stops, and it never swings back past the target.
  - Catching a playing track starts from its speed and brings it to rest without running backwards.
  - Scratching plays through the resampler (the pitch follows the hand, like vinyl), even with keylock on, and a paused deck is audible while it is held. Taking and releasing the track is declicked.
  - Letting go: a playing track plays on at its rate; with slip on it returns to the shadow. A paused track stays exactly where the finger left it.
  - Tests (offline, under `assert_no_alloc`): following and settling, speed changes bounded by the spring, slip snap-back, play resuming, no click on take and release.
- **UI:** the drag sends relative moves (frames = −Δx × frames per design pixel of the zoom). While scrubbing, the waveform shows the finger's position directly, so the picture has no lag; the audio trails it by about 2τ.
- **Still to try on real hardware:** the feel at different zoom levels, and whether the spring time suits a jog wheel too.

## Performance screen

`p` or the PERFORM button switches between the main screen and the performance screen ("2 · Performance" and its three variants on the canvas, at 1024×600). The pads' logic is `perform::press`, a pure function from a touch to a deck command, tested without a window.

- **HOT CUE:** four pads per bank. An empty pad stores the playhead (quantized); a set pad jumps there. Pads are tinted in their cue's colour, from the theme until the library carries colours. BANK pages through the hot cues four at a time (A–D, E–H).
- **BEAT JUMP:** the pads jump ∓4 / ∓1 beats; BANK switches them to ∓32 / ∓16.
- **DELETE:** arms deletion. Set pads show "✕ A, TAP TO DELETE" and delete on tap, until DELETE is tapped again or the mode changes. The canvas's CALL mode (stored hot cues to call up) waits for the library.
- **BEAT LOOP:** 1/2 to 16 beats from the playhead; the active length is highlighted, and tapping it exits the loop.
- **Waveform:** the zoom is drawn smaller (703×72), and both waveforms now show hot cues: a line and a lettered tag in the zoom, a small tag in the overview. The engine publishes each hot cue's position for this (`DeckState::hot_cue_positions`, NaN when empty).
- **Shared with the main screen:** the rail's player number and SLIP, the phase meters (`screen::phase`), the cue column, the status band and the bottom strip are the same code, parametrised by position. The main screen renders byte-identically after that refactor.
- **Screenshots:** `--perform`, `--pad-mode hotcue|beatjump|delete` and `--hot-cues 57,61,64` (seconds) set up the screen for `--screenshot`.

## Design tokens

Every colour, font size, corner radius and line width the screen uses lives in one `Theme` (`crates/ui/src/theme.rs`), named by role: `button_edge`, `cue`, `readout`, `section_radius`, and so on. `screen.rs` asks the theme and has no colour literals; a test fails if a colour literal turns up in `screen.rs`, `main.rs` or `gfx.rs`. The frame's clear colour comes from the theme too.

- **Default:** `reference`, the wireframe's tokens. Moving to tokens changed no pixel: the 1024×600 screenshot before and after is byte-identical.
- **Switching:** `--theme <name>` picks a theme. Only `reference` exists so far; a new theme is one more constructor in `Theme::all()`.
- **Waveforms:** `WaveformTokens` holds the stand-in's colours (grid, cue, loop, playhead, played part). The waveform renderer's own theme (3-band palette blue / orange / white, RGB weights, auto gain as a display setting) joins it when that crate lands, so one theme drives the screen and the waveform shader.
- **Not tokens:** layout positions and sizes stay in `screen.rs`. They come from the canvas, per screen.

## Waveforms

- **Computed during decode.** `engine_media::load_file_tapped` hands every chunk the loader writes into the source to a tap. The tap is `engined`'s `WaveTap`, a `waveform::Builder` on the loader thread. So the waveform is built from exactly the frames the deck plays: at the engine rate, resampled and gapless-trimmed (timeline origin "trimmed"; the trim and the file rate are recorded for imports). Every quarter second of audio it publishes the finished entries to a `LiveWaveform`. The UI appends what's new each frame, so both views fill in while the track loads. Dropping the tap at the end of the decode finishes the waveform. Tests:
  - the tap sees exactly what the source gets, on the resampling and the same-rate path;
  - the live waveform grows during decode and ends byte-for-byte equal to `compute()` on the whole track.
- **Auto gain:** recomputed as the track fills in (every 10 s of new audio, and at the end), so it settles on the whole track.
- **Columns on the CPU, pixels on the GPU.** `waves.rs` builds the columns with the waveform crate's own `display::column`, so modes, gains, themes, layouts and alignment match its reference renderer. Each zoom column covers a fixed slice of the timeline. The columns are placed at their exact fractional position for the interpolated playhead. The waveform shader reads each column's segments (from, to, opacity, colour) from a small float texture. It blends a column's bands in sRGB values, as the reference does, and box-filters across the columns under each pixel. So the zoom scrolls smoothly, with no whole-pixel judder and no shimmer. A test checks that a column keeps its value and moves by exactly the playhead's fraction. The overview caches its per-pixel maxima until the waveform grows.
- **Defaults:** the zoom is mirrored, with the bands overlaid, and is not dimmed on either side of the playhead (`zoom_dims_played` turns dimming on). The overview is bottom-aligned with the bands stacked (lows on the baseline, as on the CDJ-3000), and its played part dimmed.
- **Panel:** a vertical gradient behind the waveform, from `background` at the edges to `background_glow` at the baseline (the centre line, or the bottom for a bottom-aligned view). It's theme tokens, drawn by the rectangle pipeline (`Gradient::Mirrored` / `Gradient::Down`). The waveform is drawn at the theme's `opacity` (70 %) over the panel and grid, so the gradient and beat lines show faintly through it.
- **Paint order:** panel, then the waveform, then the beat ticks, loop, cues, slip shadow and playhead (`Scene` interleaves rectangle batches and waveform draws).
- **Cost:** with both waveforms the main screen takes about 1.2 ms of CPU per frame at 120 fps on the M1 Max (0.8 ms without). Building the columns is about 0.1 ms of that.

## Measurements (Mac)

MacBook Pro M1 Max, built-in 120 Hz panel, Metal, release build, playing an MP3 at 44.1 kHz. Each line is a 2 s window, from the `--play --run-for` runs. The timing overlay shows the same line (`d` toggles it).

| Buffer | fps | Frame interval p50 / p99 / max | Late frames | CPU per frame (mean / max) | Playhead step error p99 |
| --- | --- | --- | --- | --- | --- |
| default | 120.0 | 8.3 / 11.0 / 11.4 ms | 0 | 0.76 / 2.9 ms | – |
| 1024 frames | 119.8 | 8.4 / 10.8 / 11.4 ms | 0 | 0.77 / 3.6 ms | 0.76 ms |

- **Startup:** the first frames take about 70 ms (shader and glyph-atlas setup, system font scan). This shows as a few late frames in the first window, and it should be measured on the Pi.
- **Covered window:** macOS stops presenting to a covered window. One run hit a 6 s gap from that. On the Pi the kiosk window is never covered.
- **Audio:** the callback load stayed at 0.1–0.2 % with the UI running, so the UI thread does not disturb the audio thread on the Mac.

## Findings

- **Glyphon's per-frame cost is the text, not the drawing.** Re-shaping every string each frame would dominate, so shaped buffers are cached by content, and only the changing readouts (remaining time, bars, BPM) re-shape. On the Pi this cache is the first thing to profile.
- **System fonts:** `FontSystem::new()` scans the system fonts. The ◀ ▶ glyphs come from a system fallback font, because Barlow Condensed doesn't have them. For the Pi: ship a small symbol font (or draw the arrows as geometry), and build the `FontSystem` from our own fonts only. That saves the scan and makes rendering the same on every machine.
- **Touch:** winit delivers `WindowEvent::Touch` on Wayland. Under cage that may be enough, and we may not need our own evdev reader. Multi-finger gestures (hold a pad and turn the encoder, a two-finger zoom on the waveform) still need checking on the Pi.
- **Wireframe naming:** the canvas still labels the keylock chip "MT". The app shows "KEYLOCK", per the naming decision; the canvas should follow.

## Next

1. **Pi 5 run:**
   - cage + winit on Pi OS, Vulkan (Mesa v3dv) versus GL;
   - frame pacing at 60 Hz on the DSI panel;
   - CPU per frame;
   - touch latency (photodiode or high-speed camera on a touch at the panel edge).
2. **Fast state:** replace the direct `DeckState` reads with the seqlock'd shared-memory block, and the command calls with the socket, so the UI runs as its own process as designed.
3. **Waveforms:** draw the waveform data in its own pipeline, in the zoom and overview areas.
4. **Remaining screens:**
   - Info and Utility from the canvas; the browser's library views (playlists, tags, history) once the library has them;
   - taffy only if the hand layout gets painful (so far it hasn't).
