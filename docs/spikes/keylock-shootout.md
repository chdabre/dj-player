# Keylock shootout (spike 2)

Status 2026-10-21: Mac numbers in; blind listening and the Pi 5 run outstanding. Plan: `docs/research/engine.md`, "Crates, budget and spikes", item 2.

## Provisional read (Mac only)

CPU doesn't decide it on the Mac: every candidate's mean is under 20 % of the 1.3 ms block budget. What separates them is listed below. The listening test and the Pi then decide.

- **Bungee** is the only one that keeps the pitch in reverse (and so could keep keylock through scratching). It also reacts fastest to a tempo change (27 ms vs Signalsmith's 60 ms), at a CPU cost similar to Signalsmith's.
- **Signalsmith default** does its spectral work in one block per 30 ms interval (p99 ≈ 265 µs). The **cheaper** preset spreads it (split computation): p99 32–75 µs, but 90 ms rate response.
- **Rubber Band R3** is the heaviest: p99 0.84–1.13 ms on the Mac, already most of the budget. Expect it to break the budget on the Pi. **R2** and **R3 short-window** are cheap and react in about 30 ms.
- **Rubber Band allocates when it (re)starts:** 4–8 malloc/free events at a seek. That breaks the no-allocation rule for a seek inside the callback unless it is moved out of the callback. No candidate allocates in steady-state playback.
- **Seek cost:** Signalsmith (seek + one output latency run off) and Bungee (flush + pre-roll) take about 0.55–0.65 ms per start on the Mac, about half the budget. On the Pi that is a candidate for doing the prime over several blocks.
- **R2 is quieter:** its output is 6–20 % below the source level, where the others are within 0–7 %. That would be audible in a mix and can bias the blind test.

## Method

`spikes/keylock-shootout`, one deck's worth of work per candidate, 48 kHz, 128-frame blocks, on an ordinary (non-real-time) thread.

- **CPU:** a real track (Indecorum, "Fire In My Body"), 1 s warm-up then 20 s per rate. The `max` column is dominated by OS preemption on a busy Mac; read p99 as the worst-case figure until the Pi run with RT priority.
- **Start:** time and allocations of one aligned start (a seek), single measurement, so noisy.
- **Allocations:** every malloc/free in the process while armed, through macOS `malloc_logger` (sees C++ allocations too). Not available on Linux.
- **Alignment:** a burst train; the energy centroid of each heard burst against where the playhead says it is.
- **Rate response:** 1.0 → 1.25 step; a change applied `D` late makes later bursts `0.25 D / 1.25` late.
- **Read-ahead:** how far beyond the playhead each one has to read; that is the latency compensation the deck has to apply.
- **Feeding:** Signalsmith as `engine_core::Keylock` feeds it (no lookahead slew). Bungee through its grain API, which takes positions straight from the resident track. Rubber Band 4.0 in real-time mode through the raw C API (the `rubberband` crate's wrapper allocates on every call), with its start pad/start delay recipe and a FIFO to fixed blocks.

## Run it

```sh
cargo run -p keylock-shootout --release -- bench --track <file> [--seconds 20]
cargo run -p keylock-shootout --release -- render --out target/keylock-shootout/listening <ten files>
```

`render` writes `NN-ref.wav` (the untouched excerpt) and `NN-<rate>-<A..F>.wav` (20 s from 60 s in, at 0.92x and 1.08x, candidates shuffled per track and rate), plus `listening-sheet.csv` to score and `key.csv`. Score the sheet before opening the key. Needs CMake for Bungee.

## Outstanding

- Blind A/B on the ten tracks rendered on 2026-10-21 (`target/keylock-shootout/listening`, not in the repo).
- The same `bench` on a Pi 5, built natively there, with RT priority.

## Raw results, macOS aarch64

macos aarch64, 48000 Hz, 128-frame blocks, 20 s per row, CPU track: Indecorum - Fire In My Body.

### CPU per block

Budget: 1333 µs per block (half a callback). Allocations count every malloc/free in `render` after warm-up.

| candidate | rate | mean µs | p99 µs | max µs | mean % of budget | start µs | allocs start / run | read-ahead ms | underruns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| signalsmith-default | 0.92 | 28.1 | 265.6 | 2796.0 | 2.1 % | 638 | 0 / 0 | 115.2 | 0 |
| signalsmith-default | 1 | 23.1 | 228.5 | 2801.0 | 1.7 % | 558 | 0 / 0 | 120.0 | 0 |
| signalsmith-default | 1.08 | 27.0 | 265.4 | 2793.8 | 2.0 % | 1860 | 0 / 0 | 124.8 | 0 |
| signalsmith-default | 0.1 | 24.9 | 267.5 | 4101.7 | 1.9 % | 587 | 0 / 0 | 66.0 | 0 |
| signalsmith-default | 2 | 27.0 | 266.3 | 2073.8 | 2.0 % | 591 | 0 / 0 | 180.0 | 0 |
| signalsmith-default | -1 | — | — | — | — | — | — | — | resampler fallback |
| signalsmith-cheaper | 0.92 | 20.0 | 75.2 | 1958.4 | 1.5 % | 653 | 0 / 0 | 132.8 | 0 |
| signalsmith-cheaper | 1 | 16.1 | 56.2 | 4015.4 | 1.2 % | 575 | 0 / 0 | 140.0 | 0 |
| signalsmith-cheaper | 1.08 | 16.7 | 32.3 | 35.2 | 1.3 % | 636 | 0 / 0 | 147.2 | 0 |
| signalsmith-cheaper | 0.1 | 18.1 | 56.0 | 4702.9 | 1.4 % | 3204 | 0 / 0 | 59.0 | 0 |
| signalsmith-cheaper | 2 | 19.9 | 59.9 | 4702.0 | 1.5 % | 620 | 0 / 0 | 230.0 | 0 |
| signalsmith-cheaper | -1 | — | — | — | — | — | — | — | resampler fallback |
| bungee | 0.92 | 42.6 | 361.2 | 6440.0 | 3.2 % | 679 | 0 / 0 | 62.3 | 0 |
| bungee | 1 | 37.9 | 352.1 | 3484.5 | 2.8 % | 546 | 0 / 0 | 64.0 | 0 |
| bungee | 1.08 | 40.6 | 358.5 | 5239.2 | 3.0 % | 1476 | 0 / 0 | 65.7 | 0 |
| bungee | 0.1 | 34.1 | 135.5 | 1403.2 | 2.6 % | 532 | 0 / 0 | 44.8 | 0 |
| bungee | 2 | 37.6 | 143.1 | 2149.1 | 2.8 % | 554 | 0 / 0 | 85.3 | 0 |
| bungee | -1 | 36.9 | 143.1 | 1389.4 | 2.8 % | 555 | 0 / 0 | 64.0 | 0 |
| rubberband-r2 | 0.92 | 38.9 | 165.2 | 6374.0 | 2.9 % | 34 | 0 / 0 | 39.9 | 0 |
| rubberband-r2 | 1 | 14.0 | 113.0 | 3427.0 | 1.0 % | 9 | 0 / 0 | 34.7 | 0 |
| rubberband-r2 | 1.08 | 26.3 | 151.8 | 584.5 | 2.0 % | 7 | 0 / 0 | 39.1 | 0 |
| rubberband-r2 | 0.1 | 23.9 | 125.5 | 17213.3 | 1.8 % | 40 | 8 / 0 | 13.5 | 0 |
| rubberband-r2 | 2 | 47.2 | 151.7 | 2148.5 | 3.5 % | 8 | 0 / 0 | 63.8 | 0 |
| rubberband-r2 | -1 | — | — | — | — | — | — | — | resampler fallback |
| rubberband-r3 | 0.92 | 183.1 | 949.8 | 13710.5 | 13.7 % | 187 | 4 / 0 | 80.8 | 0 |
| rubberband-r3 | 1 | 168.4 | 842.7 | 5955.9 | 12.6 % | 29 | 4 / 0 | 82.7 | 0 |
| rubberband-r3 | 1.08 | 192.3 | 1131.9 | 12807.4 | 14.4 % | 24 | 4 / 0 | 84.8 | 0 |
| rubberband-r3 | 0.1 | 97.3 | 837.9 | 12998.2 | 7.3 % | 57 | 4 / 0 | 46.4 | 0 |
| rubberband-r3 | 2 | 254.1 | 974.2 | 6658.1 | 19.1 % | 28 | 4 / 0 | 125.5 | 0 |
| rubberband-r3 | -1 | — | — | — | — | — | — | — | resampler fallback |
| rubberband-r3-short | 0.92 | 63.2 | 249.9 | 2577.5 | 4.7 % | 52 | 4 / 0 | 47.1 | 0 |
| rubberband-r3-short | 1 | 61.1 | 239.1 | 3948.8 | 4.6 % | 22 | 4 / 0 | 50.7 | 0 |
| rubberband-r3-short | 1.08 | 65.1 | 245.1 | 1814.1 | 4.9 % | 20 | 4 / 0 | 51.6 | 0 |
| rubberband-r3-short | 0.1 | 47.8 | 231.3 | 3108.9 | 3.6 % | 22 | 4 / 0 | 28.0 | 0 |
| rubberband-r3-short | 2 | 109.4 | 674.9 | 10860.0 | 8.2 % | 23 | 4 / 0 | 67.4 | 0 |
| rubberband-r3-short | -1 | — | — | — | — | — | — | — | resampler fallback |

### Timing

Burst train (Hann-windowed 1 kHz bursts every 500 ms). Alignment = where the burst is heard minus where the playhead says it is (mean, worst). Rate response = delay before a 1.0 → 1.25 change is heard.

| candidate | align 0.92 ms | align 1.08 ms | align reverse ms | rate response ms |
| --- | --- | --- | --- | --- |
| signalsmith-default | -0.1 (-0.1) | +0.1 (+0.1) | — | 60.2 |
| signalsmith-cheaper | -0.2 (-0.2) | +0.1 (+0.2) | — | 90.3 |
| bungee | +0.0 (+0.0) | +0.0 (+0.0) | -0.0 (+0.0) | 26.7 |
| rubberband-r2 | -0.8 (-1.2) | +0.6 (+0.8) | — | 33.4 |
| rubberband-r3 | -0.2 (-0.3) | +0.2 (+0.3) | — | 45.0 |
| rubberband-r3-short | +0.5 (+1.1) | +0.4 (+0.5) | — | 30.4 |

