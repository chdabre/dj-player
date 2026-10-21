# Waveforms (spike)

Status 2026-10-21: desk research, now implemented in `crates/waveform` (draft PR chdabre/dj-player#1). One measurement: I decoded the ANLZ files of the rekordcrate sweep fixture to check the 3-band byte order (see "Rekordbox").

## Decisions (project owner, 2026-10-21)

- **Always compute our own waveform**, during decode, for every track. An imported Rekordbox tag is at most a placeholder until ours is ready. This supersedes `docs/research/engine.md` ("use ANLZ waveforms when present"); update the living doc to match.
- **Three display modes:** single colour, 3-band and RGB (colour from spectral content). All three are drawn from the same stored data: the three band peaks below, with single colour from their combined level and RGB from their ratios. No mode needs extra stored data.
- **Per-track auto gain** (2026-10-21): loud masters filled the whole height, so heights are normalised per track at display time. The stored data stays absolute. The gain puts the 95th percentile of the combined level at 0.8 of the half-height, clamped to 0.5–4 (silence: 1). It multiplies every height in the zoom and the overview alike, as one uniform. Auto, off or a fixed value is a setting. Implemented as `waveform::display::auto_gain` and `Style::track_gain`.
- **Colours are a renderer parameter** (2026-10-21), so themes can be applied. Every colour the views draw (waveform palettes, the RGB mode's per-band colours and weights, background, grid, labels, cues, loop, playhead, pad colours) lives in one `Theme` of plain colours and floats (shader uniforms). The renderer hard-codes none. Presets: `default`, `findings` (the old palette) and `spectrum` (RGB weighted low 0.45 / mid 1.0 / high 2.2, since equal weights come out red on real music). Gains and dimming stay display settings next to the theme.
- **Default 3-band colours: blue low, orange mid, white high** (2026-10-21). This **supersedes the findings.md palette** (indigo #4B4FC2 / teal #3FB8AE / cream #F4EAD2), which stays available as the `findings` theme; the living doc needs updating. Values chosen by eye on the previews: low #2F6FE8, mid #F0762A, high #F5F5F2. The orange started at #F0952E and was moved towards red-orange, because it merged with the amber cue and loop overlays (#E0A33A / #F2C06A). The cue colours were left alone.

## Short answer

- **Compute three bands of peak amplitude, at 150 entries per second, mono, one `u8` per band.** That is 450 B/s, about **135 KB for a 5-minute track** plus about 3.6 KB for the overview. The rate is the one Rekordbox's detail tags use, so its 3-band detail (PWV7) imports entry for entry.
- **Define the time base in seconds, not samples.** Then the data doesn't depend on the file or device sample rate.
- **Our mirrored 3-band look matches the hardware convention.** The CDJ-3000 3Band and Engine DJ "Tri-Band" also draw lows blue and highs white. Before building it, settle two things: the draw order (overlaid, not stacked), and per-band display gains. Details under "Recommendation".
- **Rekordbox gives us one direct import: PWV7/PWV6 (the `.2EX` file).** The older colour tags (PWV5/PWV4) don't store bands. The blue tags (PWV3/PWAV/PWV2) are mono. Use them only as placeholders.
- **Two primary sources disagree** on the byte order of PWV6/PWV7. The fixture measurement favours low, mid, high.

## Rekordbox / Pioneer ANLZ waveform tags

The ANLZ files are big-endian tagged files. `.DAT` holds the original tags, `.EXT` the colour ones, and `.2EX` was added for the CDJ-3000 3-band waveforms ([Deep Symmetry, ANLZ](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html); the old `/djl-analysis/anlz.html` URL now returns 404). Offsets in that document are hex unless it says "decimal": for example "len_header is 14" means 0x14 = 20 bytes, which matches the PWV6 header in the fixture.

| Tag | File | Size | Entry layout | Shown on |
| --- | --- | --- | --- | --- |
| PWAV | .DAT | 400 columns, fixed | 1 B: low 5 bits height (0–31), high 3 bits "whiteness" (whiter = less saturated blue) | Original nexus overview |
| PWV2 | .DAT | 100 columns, fixed | 1 B: low 4 bits height (0–15) | "seems to be" CDJ-900 |
| PWV3 | .EXT | 150 per second ("half-frame, 75 frames per second") | 1 B, same as PWAV | Blue scrolling detail |
| PWV4 | .EXT | 1200 columns, 6 B each | 6 channels; not fully decoded (see below) | nexus 2 colour overview, rekordbox |
| PWV5 | .EXT | 150 per second, 2 B each | big-endian u16: 3 bits red, 3 green, 3 blue, 5 height, 2 unused | nexus 2 colour detail, rekordbox |
| PWV6 | .2EX | 1200 columns, 3 B each | 3 × u8 band heights | CDJ-3000 3-band overview, rekordbox |
| PWV7 | .2EX | 150 per second, 3 B each | 3 × u8 band heights | CDJ-3000 3-band detail, rekordbox |
| PWVC | .2EX | 6 B | 3 × u16 "per-band gain calibration" (low, mid, high) | not documented |

The table comes from [Deep Symmetry](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html) and the [crate-digger Kaitai spec](https://github.com/Deep-Symmetry/crate-digger/blob/7c9d5358d8a3ce663f566380f2dc79ab1393201e/src/main/kaitai/rekordbox_anlz.ksy#L78-L84). PWVC is only in [rekordcrate](https://github.com/Holzhaus/rekordcrate/blob/14d54eded7b0d2f6fe67c7871533dca35a73e5fa/src/anlz.rs#L1133-L1147) and [rbox](https://github.com/dylanljones/rbox) (0.1.8). The fixture's PWVC holds 80 / 88 / 119, and what those values mean is unknown. Deep Symmetry's PWV7 text says `.EXT`, but the Kaitai spec and the fixture put it in `.2EX`.

**Which units show which style.** Rekordbox's FAQ ([v7 FAQ](https://rekordbox.com/en/support/faq/v7/#faq-q700013), "as of July 2026") lists the colour modes per model:

| Colour modes | Models |
| --- | --- |
| BLUE, RGB, 3Band | CDJ-3000(X), CDJ-1500X, OPUS-QUAD, XDJ-AZ, XDJ-RX3, among others |
| BLUE, RGB | CDJ-2000NXS2, XDJ-1000MK2, XDJ-XZ, among others |
| BLUE only | CDJ-2000NXS, XDJ-1000, XDJ-700, among others |

So our reference, the XDJ-1000MK2, has no 3-band mode, and 3-band is an extension for us. Pioneer describes 3Band only as "different colors to indicate the volume of each frequency band" ([CDJ-3000 announcement](https://www.pioneerdj.com/en/news/2020/cdj-3000-professional-dj-multi-player/)). The XDJ-1000MK2 manual says the enlarged waveform's frequency bands "are color-coded" (p. 4). It is not shown for tracks over 60 minutes (p. 40). The beat grid is drawn as red lines (p. 28), and the overview carries a 16-bar or 1-minute scale (p. 16) ([manual](https://rent.djcenter.ee/images/kasutusjuhendid/Pioneer-XDJ-1000MK2.pdf)).

**How colour is derived.** None of this is documented by Pioneer.
- **PWV5 (RGB):** each entry stores its colour directly. In the fixture (a 20 kHz → 20 Hz sweep), the 3-bit blue field dominates at the start (highs), then green, then red (lows). So the stored fields mean red = low, green = mid, blue = high. Height saturates at 31 throughout.
- **PWV4:** decoded only from a sine sweep ([Deep Symmetry colour preview analysis](https://djl-analysis.deepsymmetry.org/djl-analysis/track_metadata.html#color-preview-analysis)). Bytes 0–1 are "whiteness", byte 2 is energy below about 10 kHz, bytes 3, 4 and 5 are bottom, middle and top thirds. Beat Link builds the colour by dividing bytes 3–5 by the column height ([`WaveformPreview.java`](https://github.com/Deep-Symmetry/beat-link/blob/855d0bd664716886445fefaddd587aab7b222934/src/main/java/org/deepsymmetry/beatlink/data/WaveformPreview.java#L379-L401)).
- **3-band:** Deep Symmetry describes the look:
  - **Overview (PWV6):** bands stacked, lows dark blue, mids amber, highs white.
  - **Detail (PWV7):** bands "drawn on the same axis rather than being stacked". Where lows and mids overlap it shows brown, and white highs are drawn last, on top. The scaling "seems to be non-linear".

  Beat Link's approximation of that look is:
  - **Detail:** heights are 0.4 × low, 0.3 × mid and 0.06 × high ([`WaveformDetail.java`](https://github.com/Deep-Symmetry/beat-link/blob/855d0bd664716886445fefaddd587aab7b222934/src/main/java/org/deepsymmetry/beatlink/data/WaveformDetail.java#L377-L405)).
  - **Preview:** 0.49 / 0.32 / 0.25, stacked.
  - **Colours:** low (32, 83, 217), mid (242, 170, 60), high white ([`WaveformFinder.java`](https://github.com/Deep-Symmetry/beat-link/blob/855d0bd664716886445fefaddd587aab7b222934/src/main/java/org/deepsymmetry/beatlink/data/WaveformFinder.java#L65-L80)).

  So the raw high byte is large and gets weighted down heavily.

**Byte order conflict (PWV6/PWV7).** Deep Symmetry says "mid-range, high, and low, in that order", and Beat Link reads byte 2 as low. rekordcrate and rbox read low, mid, high, and [rbox has a test](https://docs.rs/crate/rbox/0.1.8/source/tests/anlz_format.rs) built on rekordcrate's [sweep fixture](https://github.com/Holzhaus/rekordcrate/tree/14d54eded7b0d2f6fe67c7871533dca35a73e5fa/data/anlz/sweep). I parsed that fixture's PWV7 myself. On the descending sweep, byte 2 dominates first (highs), byte 1 takes over at 32 % of the sweep, and byte 0 at 62 % (lows). **Byte 0 is low.** If the sweep is logarithmic (the README doesn't say), the handovers are at about 2.2 kHz and about 280 Hz. That is my inference, not a documented crossover. A sine at the fixture's (undocumented) level peaks at about 81 / 85 / 124 in the low / mid / high bytes.

**Analysis method.** The filters, peak vs RMS, and the curve are not documented anywhere I could find. Nothing of Rekordbox's analysis is public.

## Mixxx

Source: [mixxxdj/mixxx @ 300bd89](https://github.com/mixxxdj/mixxx/tree/300bd8967c4edaea38af9e32949be510f07b94f9) (2026-10-20). Paths are relative to `src/`.

**Analyzer** ([`analyzer/analyzerwaveform.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/analyzer/analyzerwaveform.cpp#L18-L20)):
- **Filters:** 4th-order Bessel low-pass, band-pass and high-pass, crossovers **600 Hz and 4 kHz** (L179–182). A Butterworth-8 variant is commented out.
- **Value:** per stride and per channel (L, R kept separately), the **maximum |x|** of the broadband signal and of each band (L237–262). The RMS code is commented out, although the protobuf still labels the signals `RMS` ([`waveform/waveform.cpp` L113](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/waveform.cpp#L113)).
- **Encoding:** `u8`, linear, `min(255, 255·peak)` ([`analyzerwaveform.h`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/analyzer/analyzerwaveform.h#L53-L60)).
- **Entry contents:** `low, mid, high, all`, plus up to 4 stem bytes ([`waveform/waveform.h`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/waveform.h#L22-L32)).
- **Crossovers are recorded in the blob** ([`waveform.cpp` L101](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/waveform.cpp#L101)). A 200 Hz / 2 kHz split sits commented out beside them.
- **Resolution:** main waveform **441 entries/s**. The overview is **1920 points per channel** for any track length ("two visual sample per pixel in full width overview in full hd"), each the average of the main waveform's stride maxima (L62–64, `averageStore`).
- **Storage:** zlib-compressed protobuf. The source comment says "a track that takes 1.2MB to store the big waveform" shrinks to about 600 KB ([`library/dao/analysisdao.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/library/dao/analysisdao.cpp#L14-L18)). For 5 minutes: 441 × 2 ch × 4 B × 300 s ≈ 1.06 MB raw.

**Renderer types** ([`waveform/widgets/waveformwidgettype.h`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/widgets/waveformwidgettype.h)). The current backend is "allshader" (`waveform/renderers/allshader/`).

| Type | Height | Colour |
| --- | --- | --- |
| Simple | broadband `all` | one colour |
| Filtered | each band from the axis, overlaid; low drawn first, high last | fixed colour per band ([`waveformrendererfiltered.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrendererfiltered.cpp)) |
| HSV | `all` | skin hue; saturation = 1 − high share, value = 1 − low share ([`waveformrendererhsv.cpp` L190](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrendererhsv.cpp#L190)) |
| RGB | `all` | band peaks weight the low/mid/high colours (fallback red/green/blue), then normalised to the brightest component ([`waveformrendererrgb.cpp` L186–213](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrendererrgb.cpp#L186-L213)) |
| Stacked | same as Filtered but with the RGB colours; in "High detail" mode bands really stack (low, mid on low, high on top), scaled by 1/√3 ([`res/shaders/stackedsignal.frag`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/res/shaders/stackedsignal.frag#L65)) | RGB colours |
| Stem | per stem | per stem ([`waveformrendererstem.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrendererstem.cpp)) |

**Rendering:**
- **Default path:** each frame the CPU builds one thin rectangle per pixel column, two triangles each, using the max over the entries that the column covers.
- **"High detail" path** (Filtered, RGB, Stacked; not GLES): uploads the waveform as an RGBA texture. A fragment shader then works per pixel, into a framebuffer 4× the widget size ([`waveformrenderertextured.cpp` L209](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrenderertextured.cpp#L209), [`widgets/allshader/waveformwidget.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/widgets/allshader/waveformwidget.cpp#L106-L139)).
- **Channels:** the top half shows the left channel, the bottom half the right.

**Readability details:**
- **Zoom** is 1–10 visual entries per pixel, default 3. That is about 147 px/s at 441 entries/s. The scale is multiplied by the rate ratio, so the waveform stretches with tempo ([`waveformwidgetrenderer.cpp` L16–18, L169](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/waveformwidgetrenderer.cpp#L16-L18)).
- **Play marker** at 0.5, so centred by default; its position is configurable.
- **Anti-jitter:** the first column is snapped to a multiple of the entries-per-pixel step, so columns don't shimmer while scrolling ([`waveformrendererrgb.cpp` L116](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/renderers/allshader/waveformrendererrgb.cpp#L116)).
- **EQ:** EQ knob gains scale the bands live. The stacked shader draws what the EQ cut in a dimmed "filtered" colour.
- **Visual gain** defaults to 2× broadband and 1× per band. Overview normalisation is off by default ([`waveformwidgetfactory.cpp` L53–54](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/waveform/waveformwidgetfactory.cpp#L53-L54)).
- **Overview:** has a played-part overlay colour ([`widget/woverview.cpp`](https://github.com/mixxxdj/mixxx/blob/300bd8967c4edaea38af9e32949be510f07b94f9/src/widget/woverview.cpp#L155)).
- **Other renderers:** end-of-track and slip renderers exist. The beat renderer draws every beat the same way, with user-set opacity; nothing under `waveform/` mentions downbeats.

## Serato, Traktor, Engine DJ, VirtualDJ

None of the four publishes analysis details (filters, rates, storage). The table lists what their manuals say. "Not documented" means I looked and found nothing.

| | Serato DJ Pro | Traktor Pro 4 | Engine DJ (inMusic) | VirtualDJ |
| --- | --- | --- | --- | --- |
| Colour meaning | Spectrum colour: red low, green mid, blue high, on both the main waveform and the overview ([Main Waveform](https://support.serato.com/hc/en-us/articles/224969307-Main-Waveform-Display), [Track Overview](https://support.serato.com/hc/en-us/articles/224969227-Track-Overview-Display)) | "lighter colors represent high frequency content while darker colors represent low frequency" (manual p. 63) | **Tri-Band**: blue bass, green mid, white treble; **RGB**: red, green, blue (guide p. 15, 40) | "Colors" type: 5 schemes (Monochrome, Neutral, Ultra-Blue, Infra-Red, Per Deck); "Beats/Shapes": "traditional blue and red" ([manual](https://virtualdj.com/manuals/virtualdj/interface/topsection/appcontrols/layout/waveform.html)) |
| Modes | Option "EQ Colored Waveforms": low EQ moves red, mid green, high blue ([Library + Display](https://support.serato.com/hc/en-us/articles/223364427-Library-Display)) | Ultraviolet, Infrared, X-Ray, Spectrum; one mode shared by waveform and stripe (p. 63, 175) | Waveform Style: Tri-Band or RGB | Beats / Colors / Shapes; Shapes greys out muted stems |
| Overview | Track overview, grey line every minute, thick every 5 minutes | "Stripe": cues, loops; flashes red at track end | Track overview, same colours | Not documented beyond the deck waveform |
| Zoom / scale | +/− keys; "waveforms scale with the pitch slider" | +/− and reset | "Waveform Zoom … from 3 seconds to 30 seconds" | Zoom slider; playhead fixed at centre, past on the left |
| Grid | Not documented on these pages | Grid Mode: Full, Dim, Ticks, Invisible; bar markers on/off (p. 175) | Beat grid lines on the waveform only | Beat grid as blocks below the waveform, large block = bar start ([Rhythm Wave](https://virtualdj.com/manuals/virtualdj/interface/topsection/rhythmwave.html)) |
| Data | Not documented | Not documented | See below | Not documented |

Manuals: [Traktor Pro 4](https://native-instruments.com/fileadmin/ni_media/downloads/manuals/traktor/Traktor-Pro-4-Manual-English-170724.pdf) and [Engine DJ 4.5](https://cdn.inmusicbrands.com/engine/Engine%20DJ%20-%20User%20Guide%20-%20v4.5.0.pdf). I read the Serato pages through the help centre's JSON API, because the HTML returns 403 to scripts.

**Engine DJ storage** (reverse-engineered, from [libdjinterop](https://github.com/xsco/libdjinterop/blob/597be49121abf175f5347662e425ae28a463d922/src/djinterop/engine/track_utils.hpp#L24-L68)):
- **Detail:** one entry per `(sample_rate/210)·2` samples, that is about **105 entries/s** (9.5 ms at 44.1 kHz).
- **Entry contents:** low, mid and high, each `u8` value plus `u8` "opacity", so 6 B per entry, zlib-compressed ([v1 format](https://github.com/xsco/libdjinterop/blob/597be49121abf175f5347662e425ae28a463d922/src/djinterop/engine/v1/performance_data_format.cpp#L213-L230)).
- **Overview:** fixed **1024 entries × 3 B**.
- **Newer schema (v2):** the library models only the overview column. The guide notes that RGB waveforms "will be built when loading a track". So newer versions may compute the scrolling waveform on the device. That is inferred, not documented.

## Across products

| | Rekordbox / CDJ | Mixxx | Engine DJ | Ours (proposed) |
| --- | --- | --- | --- | --- |
| Bands | 3 (PWV6/7) | 3 + broadband | 3 | 3 |
| Split | ≈280 Hz / ≈2.2 kHz (inferred from sweep) | 600 Hz / 4 kHz, Bessel-4 | not documented | 250 Hz / 2.5 kHz, LR4 |
| Value | not documented ("non-linear" display) | peak, linear `u8` | not documented | peak, √-encoded `u8` |
| Detail rate | 150 /s (6.7 ms) | 441 /s (2.3 ms), stereo | ≈105 /s (9.5 ms) | 150 /s, mono |
| Overview | 1200 columns | 1920 per channel | 1024 | ≈1200, derived |
| 5-minute track | PWV3 45 KB + PWV5 90 KB + PWV7 135 KB | ≈1.06 MB raw | ≈189 KB raw (v1) | ≈139 KB raw |

**What a zoomed view covers per pixel.** Our 20-beat view is about 1100 px wide (an assumption; check it against the wireframes).

| Tempo | 20 beats span | Time per pixel | 150/s entries per pixel |
| --- | --- | --- | --- |
| 128 BPM | 9.4 s | ≈8.5 ms | ≈1.3 |
| 174 BPM | 6.9 s | ≈6.3 ms | ≈0.9 |
| 85 BPM | 14.1 s | ≈12.8 ms | ≈1.9 |

So 150/s is enough at 20 beats. A 4-beat zoom would spread each entry over about 4 px.

**Readability tricks the others use.**
- **Dim the played part:** Beat Link's played overlay ([`WaveformPreviewComponent.java`](https://github.com/Deep-Symmetry/beat-link/blob/855d0bd664716886445fefaddd587aab7b222934/src/main/java/org/deepsymmetry/beatlink/data/WaveformPreviewComponent.java#L128-L142)), Mixxx's played overlay colour, and the XDJ's lit position bar.
- **Fixed centre playhead:** Mixxx default and VirtualDJ.
- **Waveform scale follows tempo:** Serato and Mixxx, which keeps two synced tracks' beats the same width.
- **Separate grid styling for beats and bars:** Traktor, VirtualDJ, and the XDJ's red grid lines.
- **Snap columns to the data grid** against shimmer: Mixxx.
- **EQ-reactive colour:** Serato and Mixxx.
- **Track-end warning:** XDJ flashes under 30 s and 15 s; Traktor's stripe turns red; Mixxx has an end-of-track renderer.

**Log or perceptual height scaling.** Only Rekordbox's display is said to be non-linear, and that is observed, not documented. Mixxx stores linear values and applies a fixed gain.

## Recommendation

### Data to compute and store

- **Bands:**
  - **Split:** low < 250 Hz, mid 250 Hz–2.5 kHz, high > 2.5 kHz, with 4th-order Linkwitz-Riley filters (two cascaded Butterworth biquads, done separately per channel).
  - **Why this split:** 250 Hz keeps the indigo body to kick and bass. It also sits near Rekordbox's inferred split, so imported and computed waveforms look alike. Mixxx's 600 Hz pulls low vocals and snare body into "low".
  - **Record it:** store the crossovers in the blob header, as Mixxx does. Then a later change is a version bump, not a silent change of meaning.
- **Value:**
  - **Per entry, per band:** peak |x|, taken as the max over L and R (mono). Our design mirrors one signal, so storing L/R as Mixxx does doubles the size for nothing.
  - **Smooth the low band:** apply a centred max over ±12 ms. A 6.7 ms window over a 30–50 Hz tone otherwise ripples by 13–40 % (my own calculation), which reads as a comb.
- **Encoding:**
  - **Curve:** `u8 = round(255·√min(peak, 1))`, so −40 dB still gets 25 steps instead of 2–3. The display curve and the per-band gains live in the shader as uniforms.
  - **Layout:** interleaved `[low, mid, high]`.
  - **No normalisation:** store absolute levels, so loud and quiet masters look different. Mixxx also leaves overview normalisation off by default.
- **Resolutions:**
  - **Detail:** 150 entries per second, starting at our timeline's t = 0 (after the gapless trim).
  - **Overview:** about 1200 columns, max-reduced from the detail at load. It costs well under a millisecond.
  - **Cache:** store the overview too, as a small second blob. Track lists can then show it without loading the detail.
  - **Not per beat:** don't store anything per beat. The grid is editable and can change tempo, and a time base needs no recompute when it does.
- **Size for 5 minutes:**
  - **Detail:** 300 × 150 × 3 = 135,000 B.
  - **Overview:** 3,600 B.
  - **Header:** about 64 B, holding magic, version, rate, entry count, crossovers, filter ID, curve ID and provenance (own / PWV7 / PWV5 / PWV3).
  - **Total:** **≈139 KB raw** in content-addressed blobs. I haven't measured compression.
- **GPU:**
  - **Upload:** once per load, as a storage buffer, or as a 2D-wrapped `R8G8B8A8` texture (Mixxx's `textureStride` approach) if a 1D texture of 45,000 entries exceeds the Pi's limit.
  - **Shader:** each fragment takes the max over the entries its column covers. Snap the column start to the entry grid, as Mixxx does.
  - **Hardware:** the Pi 5's VideoCore VII supports Vulkan 1.3 ([Raspberry Pi](https://www.raspberrypi.com/products/raspberry-pi-5/)).
  - **Tight zooms** (grid adjust) can be drawn from the PCM already in RAM instead of a higher stored rate.

### Mapping the Rekordbox tags

| Tag | Use | Fidelity |
| --- | --- | --- |
| PWV7 (.2EX) | Copy entry for entry into the detail blob. Re-encode each band through a per-band curve and gain fitted once, by analysing the same tracks both ways. Shift by `timeline_offset` for MP3s (about 25 ms ≈ 4 entries) | Timing lossless. Values lossy: crossovers, peak/energy and curve all differ |
| PWV6 (.2EX) | Overview, resampled to our width, until the detail exists | Same as PWV7 |
| PWVC (.2EX) | Read and keep. Meaning unknown | — |
| PWV5 (.EXT) | Placeholder only: band_k ≈ height × c_k / max(c), with red = low, green = mid, blue = high per the sweep | Heuristic: 3-bit colour, 5-bit height |
| PWV4 (.EXT) | Placeholder overview from bytes 3–5 | Heuristic, partly decoded |
| PWV3 (.EXT) | Mono placeholder: all bands set to the height | Mono, 5-bit |
| PWAV, PWV2 (.DAT) | Ignore, or list thumbnails only | 400 / 100 columns, mono |

I'd always compute our own waveform during decode, which is nearly free in the single pass engine.md already plans, and show an imported tag only until ours is ready. That way every track has the same scale and look. **This differs from engine.md**, which says to use ANLZ waveforms "when present and compute your own only for unanalysed tracks". Decided: always compute our own (see Decisions at the top).

The 25 ms shift assumes the ANLZ waveforms are on the same untrimmed MP3 timeline as Rekordbox's beatgrid (`crates/engine-analysis/src/rekordbox.rs`). That is inferred; verify it.

### Does the mirrored 3-band design hold up?

Yes. Blue lows and white highs are what the CDJ-3000 3Band and Engine's Tri-Band use. Engine's blue / green / white is close to our indigo / teal / cream. A centred, mirrored, fixed-playhead view with the played part dimmed matches Mixxx, VirtualDJ and the CDJs. A 20-beat window scales with tempo, as Serato's and Mixxx's do, so synced decks line up visually. What I'd change:

1. **Overlay the bands in the detail view; stack them in the overview.** In the detail view, draw each band mirrored from the centre line in painter's order, low → mid → high. That is the CDJ-3000 detail and Mixxx Filtered/Stacked. In the overview, stack them, as the CDJ-3000 overview does. Stacking in the detail view makes the total height a sum, so it overstates loudness and makes the high crest jitter. "Cream crest" then becomes a cream inner core on hi-hats and transients. Rename it in findings.md if we agree.
2. **Give each band a display gain, as uniforms.** Beat Link needs 0.4 / 0.3 / 0.06 to make PWV7 look right. Tune ours on real tracks once and store them as a palette setting.
3. **Grid lines go behind the waveform.** Beat lines dim, bar lines brighter and full height, bar numbers in the top margin. Traktor's Full/Dim/Ticks modes show that people want to tone the grid down. Cue, loop and hot-cue overlays stay in front.
4. **Add zoom steps around 20 beats** (for example 8 / 16 / 20 / 32 / 64), still counted in beats. The XDJ zooms with the rotary selector, Engine offers 3–30 s and Mixxx 1–10×.
5. **Optional, all-in-one mode only:** EQ-reactive bands, where a cut band is drawn dim, as in Mixxx's stacked shader and Serato. It costs nothing in the shader.
6. **Keep the indigo-vs-accent fix that findings.md already flags.** Blue lows are the convention, so move them toward violet or lower their luminance rather than abandoning blue.
7. **Leave room for N signals in the blob format** (stems later; Mixxx already stores 4).

### Open questions

- **Rekordbox's crossovers and value curve.** A calibrated test export would settle them: log sweeps and fixed-level sines at known dBFS, analysed by Rekordbox 7. The rekordcrate sweep doesn't document its shape or level.
- **The byte-order conflict** between Deep Symmetry and rekordcrate/rbox. Our measurement favours low, mid, high. Report it upstream.
- **What PWVC means**, and whether the CDJ-3000 applies it.
- **Do ANLZ waveforms use the untrimmed MP3 timeline?** Check against our own analysis on the 141 hand-checked MP3s.
- **The width and height of the zoom and overview views** in the wireframes. My per-pixel numbers assume about 1100 px.
- **The XDJ-1000MK2 zoom steps.** The manual doesn't list them; check the reference spec.
- **On the Pi 5 (v3dv):** the maximum texture size, and the cost of a storage buffer read in a fragment shader. Cover both in the UI spike.
- **Visual A/B on ten real tracks:** 250 Hz/2.5 kHz against 600 Hz/4 kHz, √ against linear encoding, and low-band smoothing on or off.

## Sources (opened 2026-10-21)

- Deep Symmetry, Rekordbox export analysis, ANLZ: <https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html>
- Deep Symmetry, DJ Link track metadata, colour preview analysis: <https://djl-analysis.deepsymmetry.org/djl-analysis/track_metadata.html#color-preview-analysis>
- crate-digger `rekordbox_anlz.ksy` @ 7c9d535: <https://github.com/Deep-Symmetry/crate-digger/blob/7c9d5358d8a3ce663f566380f2dc79ab1393201e/src/main/kaitai/rekordbox_anlz.ksy>
- beat-link @ 855d0bd, `WaveformPreview.java`, `WaveformDetail.java`, `WaveformFinder.java`, `WaveformPreviewComponent.java`: <https://github.com/Deep-Symmetry/beat-link/tree/855d0bd664716886445fefaddd587aab7b222934/src/main/java/org/deepsymmetry/beatlink/data>
- rekordcrate @ 14d54ed, `src/anlz.rs` and `data/anlz/sweep`: <https://github.com/Holzhaus/rekordcrate/tree/14d54eded7b0d2f6fe67c7871533dca35a73e5fa>
- rbox 0.1.8, `tests/anlz_format.rs`, `src/anlz/sections.rs`: <https://docs.rs/crate/rbox/0.1.8/source/>
- rekordbox v7 FAQ, waveform colours per model: <https://rekordbox.com/en/support/faq/v7/#faq-q700013>
- Pioneer DJ, CDJ-3000 announcement: <https://www.pioneerdj.com/en/news/2020/cdj-3000-professional-dj-multi-player/>
- XDJ-1000MK2 Operating Instructions: <https://rent.djcenter.ee/images/kasutusjuhendid/Pioneer-XDJ-1000MK2.pdf>
- Mixxx @ 300bd89, `src/analyzer/`, `src/waveform/`, `src/widget/woverview.cpp`, `src/library/dao/analysisdao.cpp`, `res/shaders/`: <https://github.com/mixxxdj/mixxx/tree/300bd8967c4edaea38af9e32949be510f07b94f9>
- Mixxx manual source (no waveform-type descriptions found): <https://github.com/mixxxdj/manual>
- Serato support, Main Waveform Display: <https://support.serato.com/hc/en-us/articles/224969307-Main-Waveform-Display>
- Serato support, Track Overview Display: <https://support.serato.com/hc/en-us/articles/224969227-Track-Overview-Display>
- Serato support, Library + Display: <https://support.serato.com/hc/en-us/articles/223364427-Library-Display>
- Serato blog, EQ Colored Waveforms: <https://serato.com/latest/blog/18238/learn-eq-colored-waveforms>
- Traktor Pro 4 manual: <https://native-instruments.com/fileadmin/ni_media/downloads/manuals/traktor/Traktor-Pro-4-Manual-English-170724.pdf>
- Engine DJ User Guide v4.5.0: <https://cdn.inmusicbrands.com/engine/Engine%20DJ%20-%20User%20Guide%20-%20v4.5.0.pdf>
- libdjinterop @ 597be49, `track_utils.hpp`, `v1/performance_data_format.cpp`, `performance_data.hpp`: <https://github.com/xsco/libdjinterop/tree/597be49121abf175f5347662e425ae28a463d922>
- VirtualDJ manual, Waveform Display: <https://virtualdj.com/manuals/virtualdj/interface/topsection/appcontrols/layout/waveform.html>
- VirtualDJ manual, Rhythm Wave: <https://virtualdj.com/manuals/virtualdj/interface/topsection/rhythmwave.html>
- Raspberry Pi 5 product page (GPU): <https://www.raspberrypi.com/products/raspberry-pi-5/>
