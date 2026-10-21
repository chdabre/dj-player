# Keylock shootout (spike 2)

Status 2026-10-21: Mac numbers and blind listening in (no audible difference at ±8 %; Bungee chosen); Bungee ported to Rust and verified against the C++; the Pi 5 run outstanding. Plan: `docs/research/engine.md`, "Crates, budget and spikes", item 2.

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
cargo run -p keylock-shootout --release -- compare
```

`render` writes `NN-ref.wav` (the untouched excerpt) and `NN-<rate>-<A..E>.wav` (20 s from 60 s in, at 0.92x and 1.08x, candidates shuffled per track and rate), plus `listening-sheet.csv` to score and `key.csv`. Score the sheet before opening the key. `compare` runs the Rust port of Bungee against the C++ library (see below). Needs CMake ≥ 3.30 for the C++ Bungee; `bench` and `render` include the port as `bungee-rust`.

Signalsmith was removed from the code base on 2026-10-21, once Bungee became the keylock (owner decision); its rows below are from runs before that. To rerun them, check out a commit before the removal.

## Listening result (2026-10-21)

Blind A/B on the ten rendered tracks at 0.92x and 1.08x: the listener could not hear a difference between any of the six candidates. At DJ-normal tempo changes quality does not separate them, so the decision rests on the measured properties: **Bungee becomes the default** (keylock in reverse, 27 ms rate response, CPU close to Signalsmith's, no allocations). Differences are more likely to show at the WIDE extremes; worth a listen at 0.7x/1.3x if WIDE matters.

## Pure-Rust port of Bungee (2026-10-21)

The owner wants no CMake in the workspace, so Bungee Basic is ported to Rust (`engine_stretch::bungee`, MPL-2.0 like the original) rather than wrapped. `cargo run -p keylock-shootout --release -- compare` drives the port and the C++ library (through `bungee-rs`) grain by grain with the same requests and the same input chunks: three synthetic stereo signals (a kick/hat/bass/pad mix, a log sine sweep, a harmonic chord sequence), at the deck's speeds plus two pitch shifts that exercise the output resampler.

Float operation order differs (rustfft vs kissfft, Eigen's vectorised maths), so the output is close, not bit-identical. To tell port bugs from that, each row also runs a **control**: the C++ against itself with its input scaled by 1 + 2⁻²⁰. Bungee's decisions are scale-invariant, so the control changes nothing but float rounding.

- **Request tags and chunk lengths match exactly** in every row, so a deck reading positions off the chunks gets the same timing from either.
- **Port vs C++ is as close as C++ vs itself** in every row (in brackets below): 55–138 dB SNR on the mix and the chord, spectra within 0.02 dB.
- **The sweep is chaotic in Bungee itself**: at most speeds the output tracks the C++ to over 100 dB for a few dozen grains, then a single grain jumps (at 1.08x, grain 56: from ~115 dB to 18 dB). That is a discrete decision (a peak pick or the `energy > 1.5 × previous` transient test) tipping on a rounding difference; from there the phases follow a different, equally valid track. The control diverges the same way, so the low SNR there is not a port error. At 1.0x and 0.5x the sweep matches to 117–138 dB.

Rust vs C++, then in brackets the control: C++ vs C++ on input × (1 + 2⁻²⁰).

| signal | speed | pitch | tags equal | frame counts equal | SNR dB | max abs diff | spectral diff mean dB | spectral diff p99 dB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| mix | 0.92 | 1 | yes | yes | 71.6 (72.0) | 5.5e-4 (4.7e-4) | 0.001 (0.000) | 0.000 (0.000) |
| mix | 1 | 1 | yes | yes | 137.4 (138.8) | 2.4e-7 (2.4e-7) | 0.000 (0.000) | 0.000 (0.000) |
| mix | 1.08 | 1 | yes | yes | 80.5 (81.5) | 4.8e-4 (4.9e-4) | 0.000 (0.000) | 0.000 (0.000) |
| mix | 0.5 | 1 | yes | yes | 101.9 (103.7) | 4.2e-5 (3.2e-5) | 0.000 (0.000) | 0.000 (0.000) |
| mix | 2 | 1 | yes | yes | 64.7 (52.2) | 2.1e-3 (1.0e-2) | 0.001 (0.004) | 0.001 (0.002) |
| mix | -1 | 1 | yes | yes | 137.4 (138.8) | 2.4e-7 (2.4e-7) | 0.000 (0.000) | 0.000 (0.000) |
| mix | 1 | 1.5 | yes | yes | 97.6 (87.4) | 3.3e-5 (2.1e-4) | 0.000 (0.000) | 0.000 (0.000) |
| mix | 1 | 0.75 | yes | yes | 55.1 (55.2) | 9.7e-3 (9.7e-3) | 0.004 (0.004) | 0.004 (0.005) |
| sweep | 0.92 | 1 | yes | yes | 0.6 (3.0) | 9.8e-1 (9.8e-1) | 0.617 (0.579) | 9.009 (9.152) |
| sweep | 1 | 1 | yes | yes | 137.7 (138.9) | 3.0e-7 (2.1e-7) | 0.000 (0.000) | 0.000 (0.000) |
| sweep | 1.08 | 1 | yes | yes | -2.8 (-4.7) | 1.0e0 (1.0e0) | 1.069 (1.180) | 11.965 (12.576) |
| sweep | 0.5 | 1 | yes | yes | 118.3 (117.0) | 9.6e-6 (1.2e-5) | 0.000 (0.000) | 0.000 (0.000) |
| sweep | 2 | 1 | yes | yes | 2.1 (2.6) | 1.2e0 (1.1e0) | 1.242 (1.265) | 15.443 (15.296) |
| sweep | -1 | 1 | yes | yes | 137.7 (138.9) | 2.7e-7 (2.4e-7) | 0.000 (0.000) | 0.000 (0.000) |
| sweep | 1 | 1.5 | yes | yes | 8.9 (-0.0) | 9.1e-1 (9.1e-1) | 0.113 (0.180) | 4.526 (6.622) |
| sweep | 1 | 0.75 | yes | yes | -2.8 (-5.2) | 1.1e0 (1.1e0) | 2.576 (2.458) | 18.349 (18.218) |
| chord | 0.92 | 1 | yes | yes | 70.9 (73.8) | 2.5e-3 (1.5e-3) | 0.004 (0.003) | 0.000 (0.000) |
| chord | 1 | 1 | yes | yes | 137.6 (138.8) | 2.4e-7 (1.8e-7) | 0.000 (0.000) | 0.000 (0.000) |
| chord | 1.08 | 1 | yes | yes | 73.1 (71.4) | 2.1e-3 (2.5e-3) | 0.003 (0.002) | 0.000 (0.000) |
| chord | 0.5 | 1 | yes | yes | 76.7 (75.0) | 2.4e-3 (1.9e-3) | 0.004 (0.003) | 0.000 (0.000) |
| chord | 2 | 1 | yes | yes | 69.8 (68.5) | 2.1e-3 (3.0e-3) | 0.018 (0.013) | 0.025 (0.018) |
| chord | -1 | 1 | yes | yes | 137.6 (138.7) | 2.1e-7 (1.8e-7) | 0.000 (0.000) | 0.000 (0.000) |
| chord | 1 | 1.5 | yes | yes | 84.2 (84.5) | 8.3e-5 (8.1e-5) | 0.000 (0.000) | 0.001 (0.001) |
| chord | 1 | 0.75 | yes | yes | 74.2 (72.3) | 2.4e-3 (3.2e-3) | 0.005 (0.003) | 0.025 (0.018) |

### Bench: port vs C++

`cargo run -p keylock-shootout --release -- bench --seconds 5`, in a Linux x86_64 cloud container (4 vCPU Xeon @ 2.8 GHz, shared, no RT priority), 48 kHz, 128-frame blocks, the generated CPU track (no music library there), no allocation counter (macOS only). `bungee-rust` is the port driven exactly like `bungee`; Signalsmith for scale. Read the ratios, not the absolute numbers: the Mac and Pi runs above and outstanding are the ones that count.

| candidate | rate | mean µs | p99 µs | max µs | mean % of budget | start µs | allocs start / run | read-ahead ms | underruns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| signalsmith-default | 0.92 | 49.4 | 575.2 | 1090.4 | 3.7 % | 1204 | n/a / n/a | 115.2 | 0 |
| signalsmith-default | 1 | 55.1 | 748.8 | 5969.4 | 4.1 % | 1132 | n/a / n/a | 120.0 | 0 |
| signalsmith-default | 1.08 | 48.8 | 563.4 | 749.7 | 3.7 % | 1195 | n/a / n/a | 124.8 | 0 |
| signalsmith-default | 0.1 | 56.4 | 704.3 | 1068.0 | 4.2 % | 1244 | n/a / n/a | 66.0 | 0 |
| signalsmith-default | 2 | 50.4 | 576.1 | 1011.7 | 3.8 % | 1221 | n/a / n/a | 180.0 | 0 |
| signalsmith-default | -1 | — | — | — | — | — | — | — | resampler fallback |
| bungee | 0.92 | 60.8 | 325.9 | 949.6 | 4.6 % | 1650 | n/a / n/a | 62.3 | 0 |
| bungee | 1 | 62.3 | 380.3 | 483.8 | 4.7 % | 821 | n/a / n/a | 64.0 | 0 |
| bungee | 1.08 | 60.1 | 348.7 | 529.5 | 4.5 % | 806 | n/a / n/a | 65.7 | 0 |
| bungee | 0.1 | 67.8 | 446.8 | 612.3 | 5.1 % | 940 | n/a / n/a | 44.8 | 0 |
| bungee | 2 | 59.2 | 292.7 | 952.9 | 4.4 % | 833 | n/a / n/a | 85.3 | 0 |
| bungee | -1 | 57.1 | 268.4 | 685.7 | 4.3 % | 860 | n/a / n/a | 64.0 | 0 |
| bungee-rust | 0.92 | 36.8 | 191.9 | 660.5 | 2.8 % | 483 | n/a / n/a | 62.3 | 0 |
| bungee-rust | 1 | 33.5 | 169.0 | 359.4 | 2.5 % | 448 | n/a / n/a | 64.0 | 0 |
| bungee-rust | 1.08 | 36.1 | 185.2 | 313.5 | 2.7 % | 685 | n/a / n/a | 65.7 | 0 |
| bungee-rust | 0.1 | 42.7 | 260.6 | 1755.5 | 3.2 % | 440 | n/a / n/a | 44.8 | 0 |
| bungee-rust | 2 | 40.9 | 249.9 | 336.4 | 3.1 % | 514 | n/a / n/a | 85.3 | 0 |
| bungee-rust | -1 | 35.7 | 219.0 | 355.7 | 2.7 % | 411 | n/a / n/a | 64.0 | 0 |

| candidate | align 0.92 ms | align 1.08 ms | align reverse ms | rate response ms |
| --- | --- | --- | --- | --- |
| signalsmith-default | -0.1 (-0.1) | +0.1 (+0.1) | — | 60.2 |
| bungee | +0.0 (+0.0) | +0.0 (+0.0) | -0.0 (-0.0) | 26.7 |
| bungee-rust | +0.0 (+0.0) | +0.0 (+0.0) | +0.0 (-0.0) | 26.7 |

- **The port is cheaper than the C++ here**: mean 34–43 µs vs 57–68 µs per block, p99 169–261 µs vs 268–447 µs, start (pre-roll) 0.4–0.7 ms vs 0.8–1.6 ms. Re-run on the Mac and the Pi before reading much into it; Eigen and kissfft are built by CMake's Release profile, rustfft has NEON and AVX paths.
- **Timing is identical**: same read-ahead, alignment and 26.7 ms rate response, as the identical chunk tags predict.
- **In the deck** (`cargo bench -p engine-core --bench render`, `keylock_1.08`, one second of output with one seek): 17.2 ms with Bungee vs 21.0 ms with the previous Signalsmith keylock on the same machine (rate 1.08 without keylock: 1.08 ms).

## Outstanding

- The same `bench` on a Pi 5, built natively there, with RT priority, now with `bungee-rust` in it.
- `bench` and `compare` on the Mac, to check the port's CPU lead holds on aarch64.

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

