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
- **Waveforms:** real, from the `waveform` crate (see "Waveforms" below). The zoom view draws them on the analysed beat grid (bar lines brighter), under the loop, cue, slip shadow and centred playhead. The overview adds loading progress and 16-bar ticks. `w` cycles 3-band → RGB → single colour; `--wave-mode` and `--wave-theme` (default, findings, spectrum, blended) pick them at start.

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
- **Paint order:** panel and grid, then the waveform, then the loop, cues, slip shadow and playhead (`Scene` interleaves rectangle batches and waveform draws).
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
   - Performance, Browse (virtualized list), Info and Utility from the canvas;
   - taffy only if the hand layout gets painful (so far it hasn't).
