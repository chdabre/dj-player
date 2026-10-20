# Dev setup

## macOS (primary for now)

- Rust stable via rustup (`rustup default stable`).
- Xcode command line tools (`xcode-select --install`). `engine-stretch` compiles Signalsmith Stretch (C++) with `cc` and generates its bindings with bindgen, which needs libclang.
- Build and run: `cargo run -p engined --release -- <path to audio file>`.
- Options: `--list-devices`, `--device <id or name substring>`, `--buffer <frames>` (e.g. `--buffer 128`; must be in the device's supported range).
- Debug builds (`cargo run -p engined -- <file>`) install `assert_no_alloc`'s allocator and abort if the audio callback allocates. Use them to check new callback code; use `--release` for listening and load numbers.
- Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Offline render bench (ns per frame, compare Mac vs Pi): `cargo bench -p engine-core --bench render`. The offline render test writes `target/tmp/offline_render.wav` for listening.

## Test media

Real tracks for manual testing live outside the repo, on the project owner's Mac:

```
/Users/chdabre/Documents/Home/50-59 Medien/52 Audio/DJ
```

Never copy them into the repo (copyright and size). Automated tests use generated signals (sines, clicks, sweeps) so they run anywhere, including CI.

## Linux / Pi (later)

- ALSA headers and the stretcher's build deps: `sudo apt install libasound2-dev pkg-config g++ libclang-dev`.
- Cross-compile in CI: the `cross-aarch64` job in `.github/workflows/ci.yml` uses Ubuntu multiarch on an x86_64 runner: `gcc-aarch64-linux-gnu` as linker, `g++-aarch64-linux-gnu` for the Signalsmith C++ code, `libasound2-dev:arm64` from `ports.ubuntu.com` as the ALSA sysroot, pkg-config pointed at `/usr/lib/aarch64-linux-gnu/pkgconfig`, then `cargo build --target aarch64-unknown-linux-gnu` and the `engine-core`/`engine-media`/`engine-stretch` tests under `qemu-aarch64`. The same commands work on any Ubuntu 24.04 box; see the job for the exact environment variables.
- Cross-compile from the Mac: `cargo install cargo-zigbuild` then `cargo zigbuild --target aarch64-unknown-linux-gnu`. **Caveat:** zig supplies libc but not ALSA, and cpal's `alsa-sys` needs `libasound.so` and `alsa.pc` for aarch64. Either point `PKG_CONFIG_SYSROOT_DIR`/`PKG_CONFIG_PATH` at a Pi OS (Debian bookworm arm64) sysroot that has `libasound2-dev`, or build only the device-independent crates (`cargo zigbuild -p engine-core -p engine-media --target aarch64-unknown-linux-gnu`). CI uses gcc multiarch rather than zigbuild because the ALSA sysroot comes for free there.
- The Pi build links against Ubuntu 24.04's glibc (2.39) in CI. Pi OS bookworm ships glibc 2.36, so a binary from CI may not start on the Pi; build on the Pi, or use `cargo zigbuild --target aarch64-unknown-linux-gnu.2.36` with a bookworm sysroot, when it is time to run there.
