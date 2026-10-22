//! `dj-ui <file>`: the UI shell spike. The main playback screen at the panel's 1024×600,
//! drawn with raw wgpu and glyphon, driving one deck of the real engine in-process.
//!
//! Touch or click: SLIP, CALL ◀ ▶ (beat jump ∓4, or loop ½X / 2X in a loop), ZOOM, and the
//! overview to seek. Keys as in `engined`, plus a real CUE hold (the window sees key
//! releases): space play/pause, c cue, 1–8 hot cues (shift: delete), i/o/l loop,
//! b 4-beat loop, [ ] loop ½X / 2X, , . beat jump, ←/→ seek ±5 s, +/- rate, 0 reset,
//! k keylock, r reverse, s slip, m memory cue, z quantize, w waveform mode (3-band / RGB /
//! single colour), p performance screen (hot cue / beat jump pads, beat loops), d timing
//! overlay, f fullscreen, q quit.
//!
//! Browsing: `LIBRARY_DIR=<dir>` makes a folder act as the root of a USB stick: the browse
//! screen lists its folders and tracks, or its library's tracks (`screens/browse/`), and its
//! library is `<dir>/DJPLAYER` unless `--library`/`--no-library` say otherwise. A stick with
//! no library asks once whether to scan it. Without a file the app starts on the browse
//! screen. Tab (or the BROWSE tab, or the stand-in's knob) opens it; ↑/↓ or the wheel move,
//! Enter loads or opens, Esc / Backspace goes up, `/` searches (type the query; Esc ends it),
//! `i` (or the INFO tab) shows the selected track's details.
//!
//! With the `standin` feature (on by default) the window also shows the hardware stand-in:
//! the player's physical controls drawn around the device screen (`standin.rs`), as a
//! separate layer. `--no-standin` shows the device screen alone, as on the Pi.
//!
//! See docs/spikes/ui-shell.md for what this measures and what it found.

mod app;
mod args;
mod clock;
// Without the stand-in only the dev keyboard reports controls, until the RP2040 reader exists.
#[cfg_attr(not(feature = "standin"), allow(dead_code))]
mod input;
mod kit;
mod playlists;
mod render;
mod screens;
#[cfg(feature = "standin")]
mod standin;
mod stats;
mod theme;
mod waves;

use std::process::ExitCode;

use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::App;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() -> ExitCode {
    let args = match args::parse() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dj-ui: {e}");
            }
            eprintln!("{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let mut app = match App::start(args) {
        Ok(app) => app,
        Err(code) => return code,
    };
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dj-ui: {e}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("dj-ui: {e}");
        return ExitCode::FAILURE;
    }
    app.finish()
}
