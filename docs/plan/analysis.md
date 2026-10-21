# Track analysis: model and first backend

Status 2026-10-21: the model, backend interface, selection policy, scorer, Rekordbox XML importer (with overrides) and four backends (gridfit, stratum-dsp, Essentia DSP and TempoCNN through a Python worker) are in `crates/engine-analysis`. Next: our own package (see the plan below). Background: `docs/research/engine.md`, "On-device analysis".

## Model

- **Features:** tempo (the BPM a player shows), beatgrid (every beat with its bar position, so tempo changes work) and key. Loudness and waveforms come later as further features.
- **Timebase:** seconds (`f64`) from the first decoded frame, after gapless trimming. Rekordbox stores milliseconds, files come at 44.1 or 48 kHz and the deck runs at the device rate; each consumer converts to its own frames on load.
- **Provenance:** every value is an `Analysed<T>` with a confidence and its source: `Manual`, `Rekordbox`, or `Analyzer(id)`. The `AnalyzerId` (name, version, parameter fingerprint) is the cache key, so results from an older version or other settings are never reused.
- **Backends:** one `Analyzer` trait per backend that declares the features it provides, so a library that does tempo, grid and key in one pass runs once. Backends get the decoded track at its own rate and resample internally if they need to.
- **Selection:** a `Policy` picks one result per feature: manual, then Rekordbox, then analyzers in preference order. An analyzer result below the confidence threshold loses to the next source; if nothing passes, the best-ranked result is used.
- **Scoring:** tempo (±0.5 BPM, ±2 BPM, half/double, 3:2), beat F-measure (±70 ms) with the median signed offset, downbeat F-measure, and key with MIREX weights. `engine-analyze score truth.csv` runs every compiled-in analyzer and prints a Markdown report.

## stratum-dsp 1.0.0: findings so far

- **Silence trimming shifts the grid.** It trims leading silence and reports beat times relative to the trimmed audio, so a track with a silent intro gets a grid early by the length of that silence. The backend turns trimming off (same tempo and key; only the offset changes).
- **Tuned for 44.1 kHz.** At 48 and 96 kHz a 124 BPM test track came out at 123 BPM, with beat F 0.06–0.48. The backend resamples to 44.1 kHz first; tempo is then right at every rate.
- **Grid bias on synthetic tracks.** Where it locks (F = 1.0), every beat is about 32 ms early (124, 140 and 174 BPM; 44.1, 48 and 96 kHz). On other synthetic tracks it does not lock at all (F 0–0.66), and 174 BPM came out as 87. Not corrected yet: real tracks will show whether the bias is constant.
- **On three real tracks** its grid disagrees with its own tempo (median grid tempo 188, 212 and 227 BPM against 168, 150 and 138 reported), and key confidence is 0.00–0.03. Not scored yet, since their true tempo and key are unknown.
- **Speed:** about 55x real time on an M-series Mac, single-threaded (3–7 s per track).

## Ground truth

Tracks analysed in Rekordbox and exported with File → Export Collection in xml format; the XML carries BPM, key and the grid's tempo markers. `engine-analyze rekordbox EXPORT.xml --overrides FILE` scores every compiled-in analyzer against it (`--only NAME` for one). The overrides file corrects what a listener found wrong (`exclude`, `bpm`, `shift_beats`).

All scored tracks so far are MP3s, so a constant offset between Rekordbox's sample 0 and ours (encoder delay) cannot be told apart from analyzer bias yet; a set with WAV/AIFF tracks would.

## Results on the Rekordbox set (2026-10-21)

100 tracks from the owner's collection (MP3, 90–165 BPM, mostly 110–150) analysed by Rekordbox 7.2.19, after the owner checked the 10 worst by ear: Rekordbox was right on 7, half a beat late on 2 (one fixed in Rekordbox, one in `rekordbox-overrides.csv`) and wrong on 1 drifting track (fixed in Rekordbox). The ground truth lives outside the repo, in `~/Documents/dj-player-groundtruth/`.

| | stratum-dsp | essentia (DSP) | essentia-tempocnn | gridfit |
| --- | --- | --- | --- | --- |
| Tempo within ±0.5 BPM | 70 % | 87 % | 89 % | 89 % |
| Tempo within ±2 BPM | 95 % | 97 % | 100 % | ~90 % |
| Half/double, 3:2 or wrong | 5 % | 3 % | 0 % | ~10 % |
| Beat F (±70 ms) | 0.30 | 0.67 | — | 0.81 |
| Downbeat F | 0.10 | — | — | 0.56 |
| Key correct / MIREX | 42 % / 0.55 | 60 % / 0.70 | — | — |
| s per 5-min track (rough) | 6.3 | 3.4 | 2.2 | 0.6 |

The Essentia and stratum-dsp columns were scored before the two Rekordbox corrections, and their times overlapped with other runs; a clean rerun is due. gridfit's defaults (band envelope for phase and tempo, prior width 1.0) were chosen on this same set, from a handful of discrete options, so its numbers are optimistic until a second set confirms them.

What the tuning showed:

- **Phase:** spectral flux summed over linear FFT bins is dominated by the hat range (6–14 kHz), which is louder on the off-beat in most tracks. Every log-spaced band below ~4.5 kHz is louder on the beat in 53–66 of 70 tracks. Taking the phase from those bands, equally weighted, cut whole-grid half-beat errors from 17 tracks to 6.
- **Octave:** gridfit alone confuses 4:3 and 3:2 (90→120, 160→107) and, with a kick on every beat, double and half. TempoCNN made none of these errors but reports whole BPM only.
- **Key:** Essentia's `KeyExtractor` (EDM profile) is the best so far and still only 60 %.

## Plan: our own analysis package

Pick the best part for each feature, port it to Rust, and run everything in-process on the Pi:

1. **Tempo:** TempoCNN (`deeptemp-k16-3`, AGPL-3.0, 1.3 MB) converted to ONNX and run in Rust (`tract` or `ort`), with Essentia's mel front-end ported. It decides the tempo; gridfit refines it to ~0.001 BPM.
2. **Grid:** gridfit, searching only around the TempoCNN tempo. Needs the `Analyzer` interface to pass earlier results (a tempo hint) to later analyzers.
3. **Key:** port Essentia's `KeyExtractor` (HPCP chroma plus key profiles) to Rust; then compare profiles and libKeyFinder.
4. Essentia (Python worker) and stratum-dsp stay as reference backends for the scorer only.
5. A second, independent ground-truth set before trusting the tuned numbers.

Each port is checked against the reference implementation it replaces (same outputs on the same audio) before it is scored.

