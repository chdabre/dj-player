# Dev setup

## macOS (primary for now)

- Rust stable via rustup (`rustup default stable`).
- Xcode command line tools (`xcode-select --install`) and CMake (`brew install cmake`, ≥ 3.30), for the keylock shootout spike, which builds the C++ Bungee with CMake and Rubber Band's bindings with bindgen (libclang).
- Build and run: `cargo run -p engined --release -- <path to audio file>` (terminal), or `cargo run -p ui --release -- <file>` for the UI shell at 1024×600.
- Library: both take `--library <dir>` (our library folder, made if missing) or `--no-library` (RAM only). Without either, the player uses `DJPLAYER/` at the root of the track's volume if it is removable (a USB stick or SD card) and writable, else RAM only, and says which. `--auto-cue off|<dB>` (default −36 dB): with no stored memory cue, the load cue point is the first sound at or above that level. The device id lives in `~/.config/djplayer/device-id` (`$DJPLAYER_CONFIG` overrides the folder). The library is `DJPLAYER/library.db` (SQLite) plus `blobs/`; see `docs/plan/library.md`.
- Browsing: `LIBRARY_DIR=<dir> cargo run -p ui --release` treats `<dir>` as the root of a USB stick. The UI starts on the browse screen (the folder tree, with tags read in the background), and loading a track gives it a record in `<dir>/DJPLAYER` (made if missing; `--library` / `--no-library` override). A file argument is then optional; `--browse` opens the browse screen with one loaded. A folder without `DJPLAYER/library.db` asks whether to scan it. For screenshots: `--source usb|library`, `--sort name|title|artist|bpm|key|time`, `--search <query>`, `--scan` (scan without asking), `--info` (the browser's info panel up), `--info-screen` (start on the INFO screen), `--analyze-all` (start ANALYZE ALL in LIBRARY). Don't point it at a folder you don't want a `DJPLAYER/` in; a scratch folder with a few copied tracks is enough.
- The UI window shows the hardware stand-in (the player's controls around the device screen, a debug shell) by default; `--no-standin` shows the device screen alone, and `cargo build -p ui --no-default-features` leaves the stand-in out, as the device build will. With the stand-in, `--screenshot` captures the whole window at 1512×950.
- UI options: `--fullscreen`, `--no-standin`, `--theme <name>`, `--wave-theme default|findings|spectrum|blended`, `--wave-mode 3band|rgb|single`, `--at <seconds>`, `--play`, `--run-for <seconds>` (timing runs), `--screenshot <out.png>` (renders the frame offscreen at exactly 1024×600 once the track is decoded and analysed, then exits), `--perform`, `--pad-mode hotcue|beatjump|delete`, `--hot-cues <s,s,…>` (set hot cues at start). Frame timing prints every 2 s; `d` toggles the overlay.
- Waveform previews without the UI: `cargo run --release -p waveform --bin waveform-preview -- <file> --out <dir>` (`--theme`, `--synth track|tones|sweep`).
- Options: `--list-devices`, `--device <id or name substring>`, `--buffer <frames>` (e.g. `--buffer 128`; must be in the device's supported range).
- Debug builds (`cargo run -p engined -- <file>`) install `assert_no_alloc`'s allocator and abort if the audio callback allocates. Use them to check new callback code; use `--release` for listening and load numbers.
- Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Offline render bench (ns per frame, compare Mac vs Pi): `cargo bench -p engine-core --bench render`. `keylock_1.08` is the keylock path. The offline render test writes `target/tmp/offline_render.wav` for listening.

## Test media

Real tracks for manual testing live outside the repo, on the project owner's Mac:

```
/Users/chdabre/Documents/Home/50-59 Medien/52 Audio/DJ
```

Never copy them into the repo (copyright and size). Automated tests use generated signals (sines, clicks, sweeps) so they run anywhere, including CI.

## Linux / Pi (later)

- ALSA headers: `sudo apt install libasound2-dev pkg-config`; for the keylock shootout spike also `g++ libclang-dev cmake` (CMake ≥ 3.30).
- Cross-compile in CI: the `cross-aarch64` job in `.github/workflows/ci.yml` uses Ubuntu multiarch on an x86_64 runner: `gcc-aarch64-linux-gnu` as linker, `libasound2-dev:arm64` from `ports.ubuntu.com` as the ALSA sysroot, pkg-config pointed at `/usr/lib/aarch64-linux-gnu/pkgconfig`, then `cargo build --target aarch64-unknown-linux-gnu` and the `engine-core`/`engine-media`/`engine-stretch` tests under `qemu-aarch64`. The same commands work on any Ubuntu 24.04 box; see the job for the exact environment variables.
- Cross-compile from the Mac: `cargo install cargo-zigbuild` then `cargo zigbuild --target aarch64-unknown-linux-gnu`. **Caveat:** zig supplies libc but not ALSA, and cpal's `alsa-sys` needs `libasound.so` and `alsa.pc` for aarch64. Either point `PKG_CONFIG_SYSROOT_DIR`/`PKG_CONFIG_PATH` at a Pi OS (Debian bookworm arm64) sysroot that has `libasound2-dev`, or build only the device-independent crates (`cargo zigbuild -p engine-core -p engine-media --target aarch64-unknown-linux-gnu`). CI uses gcc multiarch rather than zigbuild because the ALSA sysroot comes for free there.
- The Pi build links against Ubuntu 24.04's glibc (2.39) in CI. Pi OS bookworm ships glibc 2.36, so a binary from CI may not start on the Pi; build on the Pi, or use `cargo zigbuild --target aarch64-unknown-linux-gnu.2.36` with a bookworm sysroot, when it is time to run there.
