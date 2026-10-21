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

## Validation (162 hand-corrected tracks, 2026-10-21)

The `Validation` playlist (80–185 BPM, 160 MP3 + 2 WAV), corrected by hand in Rekordbox and disjoint from the tuning set:

| | gridfit+tempocnn | tempocnn | gridfit | essentia (DSP) | hpcpkey |
| --- | --- | --- | --- | --- | --- |
| Tempo within ±0.5 BPM | **94.4 %** | 92.0 % | 85.2 % | 72.8 % | — |
| Half/double, 3:2 or wrong | 5.6 % (all half/double) | 5.6 % | 14.3 % | 16.7 % | — |
| Beat F (±70 ms) | **0.939** | — | 0.885 | 0.702 | — |
| Downbeat F | **0.607** | — | 0.539 | — | — |
| Key correct / MIREX | — | — | — | 50 % / 0.610 | 50 % / 0.610 |
| s per 5-min track | 0.54 (+0.27 tempocnn) | 0.27 | 0.55 | 3.16 | 0.35 |

- The combined tempo + grid is better on unseen tracks than on the tuning set (beat F 0.939 vs 0.83), so the tuning did not overfit.
- All nine tempo misses are 80–92 vs 160–185 BPM, and the reference goes both ways (87 for some tracks, 174 for others): a display convention to settle with a BPM range setting, as Rekordbox does, not a detection error.
- **MP3 start offset:** on MP3s Rekordbox's grid is 23.4 ms (median) later than ours; on the two WAVs 0.7 ms. That is the size of an MP3 encoder's start delay (576 + 529 samples ≈ 25 ms at 44.1 kHz): Rekordbox and Symphonia (gapless on) disagree on where an MP3 starts. Grids and cues imported from Rekordbox need that correction for MP3s; more WAV/AIFF tracks would pin it down.
- Key is the weak feature: 50 %.

## Ported backends

Both ports are checked against the implementation they replace before they are scored:

- **`tempocnn`** (Rust, tract): same BPM as Essentia's TempoCNN on all 100 tuning tracks, 7.5x faster. The model is not in the repo; convert it once with tf2onnx (TensorFlow 2.15, Python 3.11): `python -m tf2onnx.convert --graphdef deeptemp-k16-3.pb --inputs input:0 --outputs output:0 --opset 13 --output deeptemp-k16-3.onnx` (max abs difference to TensorFlow 5e-7), and point `TEMPOCNN_MODEL` at it.
- **`hpcpkey`** (Rust): Essentia's `KeyExtractor` with its defaults and the EDM profile; same key as Essentia on all 162 validation tracks, 9x faster. **License:** a close translation of Essentia's code, so the file is AGPL-3.0-or-later; it sits behind the `hpcpkey` feature so the crate can be built without AGPL code.
- **`gridfit+tempocnn`**: gridfit refines the tempo of the analyzer named in its hint (TempoCNN) within ±1 %, so TempoCNN decides octave and 3:2 questions and gridfit the exact BPM, phase and bars.

## Plan: our own analysis package

Pick the best part for each feature, port it to Rust, and run everything in-process on the Pi:

1. ~~Tempo: TempoCNN in Rust~~ (done: `tempocnn`).
2. ~~Grid: gridfit around the TempoCNN tempo~~ (done: `gridfit+tempocnn`).
3. ~~Key: port Essentia's `KeyExtractor`~~ (done: `hpcpkey`). Key references are Rekordbox's own (not checked by ear), so results are *agreement with Rekordbox*. Tried on a 30-track tuning sample (MIREX, Essentia defaults 0.763): other profiles (bgate 0.717, Krumhansl 0.670, Temperley 0.397); analysed range from 60/120/250 Hz instead of 25 Hz (0.737/0.687/0.647); 5 kHz upper limit and no PCP gate (no change). key-cnn (Schreiber, `deepspec_k16`): 0.467 with current and with librosa 0.6.2 features alike; it calls an A minor chord C major and mostly answers the relative major, so it does not suit minor-heavy club music. Essentia's defaults stay. Still open: libKeyFinder.
4. Essentia (Python worker) and stratum-dsp stay as reference backends for the scorer only.
5. ~~A second, independent ground-truth set~~ (done: the Validation playlist).
6. ~~BPM range setting~~ (done): `Settings::bpm_range`, `--bpm-range 88-176`. Tempos are folded by octaves into the range; `tempocnn` folds patch tempos before its vote, gridfit searches in the range and folds a hint. Default: no range (the network's choice). The nine 87/174-type misses on the validation set cannot all be fixed by one range: the reference itself has 80 and 85 on some and 180 and 185 on others (more than an octave apart).
7. ~~MP3 start offset~~ (done, automatic): Rekordbox decodes MP3s without gapless trimming, so its times are later than ours by the file's own start trim (`engine_media::TrackInfo::start_trim` ÷ rate; 1105 frames for the usual LAME delay). Measured on 141 reliable MP3s: −23.4 ms at 48 kHz (predicted −23.0) and −25.4 ms at 44.1 kHz (predicted −25.1). `rekordbox::timeline_offset` gives it per file; the scorer applies it (`--no-offset-correction` to disable). After correction the median offset on a 20-track sample is +0.4 ms. Files with other encoder delays are untested but are handled by the same rule.
8. **Licensing** (decided 2026-10-21): AGPL-3.0 is fine; the player may ship the TempoCNN model and `hpcpkey`.

Each port is checked against the reference implementation it replaces (same outputs on the same audio) before it is scored.

