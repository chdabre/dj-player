# Dev setup

## macOS (primary for now)

- Rust stable via rustup (`rustup default stable`).
- Build and run: `cargo run -p engined --release -- <path to audio file>`.

## Test media

Real tracks for manual testing live outside the repo, on the project owner's Mac:

```
/Users/chdabre/Documents/Home/50-59 Medien/52 Audio/DJ
```

Never copy them into the repo (copyright and size). Automated tests use generated signals (sines, clicks, sweeps) so they run anywhere, including CI.

## Linux / Pi (later)

- ALSA headers: `sudo apt install libasound2-dev pkg-config`.
- Cross-compile from the Mac: `cargo install cargo-zigbuild` then `cargo zigbuild --target aarch64-unknown-linux-gnu`. CI covers this.
