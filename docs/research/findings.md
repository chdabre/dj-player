# DJ Player — findings & decisions (as of 2026-10-20)

Copied from the claude.ai project doc `claude/dj-player-findings.md` (project "DJ Player"). Decisions made later in the engine research (see `engine.md`) take precedence where they differ — notably: Rekordbox data is read-only, our own library is written to the stick, and the device has no internal NVMe by default.

Open-source standalone DJ player inspired by the XDJ-700 / XDJ-1000MK2: Raspberry Pi based, custom PCB + enclosure, off-the-shelf (AliExpress-sourceable) parts, Rekordbox-library compatible, own engine.

## Related artifacts
- Reference spec (doc): https://claude.ai/code/artifact/29b73515-7daf-401f-aa8d-1b8cad02e09f — every control, screen, feature, value range and setting of the XDJ-1000MK2, plus our parity/extension table.
- Reference UI wireframes (canvas): https://claude.ai/artifact/3qgXr2k9inEpQf5V5AMuo8 — 6 base screens, waveform component + spec, 6 mid-set variants.
- Source manual: XDJ-1000MK2 Operating Instructions (PDF) — https://rent.djcenter.ee/images/kasutusjuhendid/Pioneer-XDJ-1000MK2.pdf (screen figures pp. 16–19, 29, 33, 35).

## Architecture (decided)
- Three separable parts:
  1. **Control-surface MCU** (RP2040/RP2350) on the custom PCB: Cherry MX key matrix, hot cue pads, arcade CUE/PLAY, pitch fader ADC, platter encoder + touch, LEDs. Talks USB (class-compliant MIDI/HID + own hi-res jog protocol) → the same board doubles as a PC controller.
  2. **Headless engine daemon** — **own engine, written in Rust** (decided; not Mixxx). Decks, decoding (symphonia), keylock (Rubber Band / Signalsmith Stretch), sync (Ableton Link between units), stems mixing, library.
  3. **UI client** — **raw wgpu** (decided, for maximum control; Flutter and Slint rejected). Engine ↔ UI: shared-memory ring for playhead/meters, message channel for the rest; UI interpolates playhead.
- Configurations from one engine: standalone player · all-in-one (2–4 decks + mixer) · PC + controller.
- Display path on Pi: start with cage (kiosk Wayland) + winit; direct KMS only if latency measurements demand it. FIFO present, frame queue depth 1.
- UI building blocks to write ourselves: glyphon (text), taffy (layout), evdev multitouch + gestures, virtualized lists, on-screen keyboard. Waveforms rendered in shader from per-track peak buffers.
- Compute: prototype on Pi 5, target CM5 on custom carrier (DSI display, USB3, GbE, I2S DAC). Display: Raspberry Pi Touch Display 2 (7", 720×1280, used landscape). Plan two stereo outputs (master + headphone cue) for all-in-one mode.
- Library: read Rekordbox Device Library (export.pdb + ANLZ) and OneLibrary (exportLibrary.db, SQLCipher) via the Rust `rbox` crate. *Superseded:* we never write Rekordbox files; history/tags/cues go to our own library format on the stick (engine.md, "Own library on the stick"). On-device analysis (BPM/key/grid/3-band waveform) so quantize/sync work without Rekordbox. Stems: Demucs-class separation on Pi CPU is roughly 3× slower than real time → background queue / pre-generated on a PC.
- Link: own open protocol + Ableton Link first; Pro DJ Link presence is a stretch goal.

## Platter (decided direction)
- Reference XDJ jog: touch is on the **top plate only** (embedded switch on the 1000MK2); side/edge has no sensing — rotation without touch = pitch bend. VINYL vs CDJ mode.
- Ours: **capacitive** touch (easier than mechanical with printed parts). Design: 3D-printed ring with **glass sheet on top** and a **thin PCB electrode ring inside**; **stationary centre GC9A01 round display**. Contactless pickup via stationary electrode under the rotating ring (no slip ring). Leave room in the hub to add a floating/mechanical press later.
- Bearing: **two stacked thin-section bearings** (6808-2RS 40×52×7 or 6806-2RS 30×42×7), 15–25 mm apart, wave-spring preload, around a hollow stationary post (display cables through it). Premium option: single crossed-roller (CRBH 3510). Avoid lazy-susan bearings.
- Encoder (no centre shaft available): belt-driven side encoder (GT2, e.g. 3:1 × 1000 PPR) is simplest; optical track etched on the PCB ring read by reflective sensors is the elegant option; off-axis magnetic ring (iC-MU) is the premium option.
- Feel: add steel mass in the hub; adjustable felt friction (jog-adjust equivalent).

## Reference UI wireframes — design tokens
- Layout follows the reference's documented placement at its 5:3 aspect, scaled to 1280×768; generic gray boxes only — none of the reference's styling, fonts, icons, waveform look or trademarked names (library key is labelled "LIBRARY").
- Panels: left rail · tab row · information section · status band · bottom strip, each a framed region on black (#000000, border #34373D), divider before the right-hand key column.
- Font: **Barlow Condensed** (tabular figures). Scale: status readouts 72 px (ms 48, % 28), BPM 52, buttons/tabs 26, list rows 26, labels 17–18, flags 18, pads 32, bar countdowns 34.
- Colour: accent **electric blue #2B7FFF** (text #7CB4FF, active fill #0B2350); amber #E0A33A / #F2C06A for status flags and cues; hot cue pads rose #E8667A, amber, sky #5AB8EC.
- Group labels (CUE/LOOP, CALL, BEAT LOOP) centred with hairline rules either side.
- Phase meter: two rows of 4 connected segments (bar vs master, beat in bar), each with its own countdown; MASTER PLAYER badge beside the top row.

## Waveform (our own design)
- Mirrored 3-band: lows = wide indigo body (#4B4FC2), mids = teal core (#3FB8AE), highs = cream crest (#F4EAD2); played part dimmed.
- Zoom (20 beats, fixed centre playhead): beat lines, bar lines + bar numbers, cue point (amber triangle + dashed line), memory cues (grey triangles), hot cue tags/lines in pad colour, hot loop as shaded amber region with in/out lines and beat count; states for slip (dashed blue background playhead + tag) and grid adjust (grid in accent).
- Overview: whole track, 16-bar scale ticks, memory/cue triangles, hot cue tags along the bottom, loop region, playhead.
- Open: low band indigo sits close to the electric-blue accent — consider shifting toward violet.

## Variants captured (mid-set states, muscle-memory priority)
Slip active · sync on (follower: slider BPM greyed, synced BPM shown) · grid adjust (grid menu: RESET / SNAP GRID / SHIFT GRID) · beat jump (pads ◀4 ◀1 1▶ 4▶, BANK 1·4 ⇄ 16·32 — our own arrangement, not documented) · hot cue call mode · hot cue delete mode.
Deliberately skipped as not muscle-memory relevant: tag list, link info, search keyboard, track filter editor, shortcut, menu overlays, hot cue bank list, medium select, utility edge cases.

## Hardware control decisions (from the brief)
- UI keys: Cherry MX-style switches; dedicated hot cue buttons (8 physical RGB pads, banks for 16); CUE / PLAY as arcade buttons; 100 mm slide pot for tempo.
