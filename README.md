# DJ Player

An open-source standalone DJ player inspired by the Pioneer XDJ-700 and XDJ-1000MK2: a Raspberry Pi CM5 on a custom PCB in its own enclosure, an RP2040 control surface with a platter, a raw wgpu UI, and its own real-time audio engine written in Rust. Tracks are decoded into RAM and played by one lock-free audio callback that owns every deck. Development starts on macOS (Apple Silicon) and moves to the Pi later. Right now one deck plays a track with its own analysis (tempo, beatgrid, key), with cues, hot cues, loops, beat jump, slip and keylock. A UI shell draws the main screen at the display's 1024×600, with real waveforms.

## Run it on macOS

Install Rust with [rustup](https://rustup.rs) and the Xcode command line tools (`xcode-select --install`), then:

```sh
cargo run -p engined --release -- <file>
```

Or the UI, with on-screen stand-ins for the player's controls around its screen until the hardware exists (same keys, plus touch or mouse; `w` cycles the waveform mode, `p` switches to the performance screen with hot cue and beat jump pads and beat loops, `d` shows frame timing):

```sh
cargo run -p ui --release -- <file>
```

Any MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF or Ogg file works. Keys: space play/pause, ←/→ seek ±5 s, `+`/`-` rate ±1 %, `0` reset the rate, `k` keylock, `r` reverse, `s` slip, `z` quantize, `c` cue, `1`–`4` hot cues (`!@#$` delete), `i`/`o` loop in/out, `l` reloop/exit, `b` 4-beat loop, `[`/`]` loop 1/2X/2X, `,`/`.` beat jump ∓4, `q` quit. Once per second the engine prints the playhead, the rate and the callback load meter. `--list-devices`, `--device <name>` and `--buffer <frames>` select the output. See [docs/dev-setup.md](docs/dev-setup.md) for more.

## Layout

| Crate | What it holds |
| --- | --- |
| `crates/engine-core` | Source, Transport (with slip), Scaler, Keylock, Deck, Engine; no I/O, no threads |
| `crates/engine-stretch` | Time-stretcher for keylock: Bungee Basic, ported to Rust (MPL-2.0) |
| `crates/engine-analysis` | Tempo, beatgrid and key behind pluggable backends (gridfit, TempoCNN, hpcpkey), a selection policy and a ground-truth scorer (`engine-analyze`) |
| `crates/engine-media` | Symphonia decode and load-time resample on a loader thread, with a tap for the waveform |
| `crates/engine-io` | `AudioBackend` trait and its cpal implementation |
| `crates/waveform` | 3-band waveform data, its blob format, display modes and a CPU reference renderer (`waveform-preview`) |
| `crates/engined` | `Player` (device, decode, analysis, waveform, stream for one deck) and the terminal player |
| `crates/ui` | The UI shell (`dj-ui`): raw wgpu + glyphon, the main screen, themes |

## Contributing

Start with [AGENTS.md](AGENTS.md). It lists the decisions that must not change quietly and links to the ground truth in [docs/](docs/): the engine research ([docs/research/engine.md](docs/research/engine.md)), the project findings, the step plans ([docs/plan/](docs/plan/)), the spikes ([docs/spikes/](docs/spikes/)) and the dev setup.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
