# DJ Player

An open-source standalone DJ player inspired by the Pioneer XDJ-700 and XDJ-1000MK2: a Raspberry Pi CM5 on a custom PCB in its own enclosure, an RP2040 control surface with a platter, a raw wgpu UI, and its own real-time audio engine written in Rust. Tracks are decoded into RAM and played by one lock-free audio callback that owns every deck. Development starts on macOS (Apple Silicon) and moves to the Pi later. Right now the engine plays one track on one deck (step 1).

## Run it on macOS

Install Rust with [rustup](https://rustup.rs), then:

```sh
cargo run -p engined --release -- <file>
```

Any MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF or Ogg file works. Keys: space play/pause, ←/→ seek ±5 s, `+`/`-` rate ±1 %, `0` reset the rate, `r` reverse, `s` slip, `q` quit. Once per second the engine prints the playhead, the rate and the callback load meter. `--list-devices`, `--device <name>` and `--buffer <frames>` select the output. See [docs/dev-setup.md](docs/dev-setup.md) for more.

## Layout

| Crate | What it holds |
| --- | --- |
| `crates/engine-core` | Source, Transport (with slip), Scaler, Deck, Engine; no I/O, no threads |
| `crates/engine-media` | Symphonia decode and load-time resample on a loader thread |
| `crates/engine-io` | `AudioBackend` trait and its cpal implementation |
| `crates/engined` | The engine binary; for now a terminal player |

## Contributing

Start with [AGENTS.md](AGENTS.md). It lists the decisions that must not change quietly and links to the ground truth in [docs/](docs/): the engine research ([docs/research/engine.md](docs/research/engine.md)), the project findings, the current step plan ([docs/plan/step-1.md](docs/plan/step-1.md)) and the dev setup.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
