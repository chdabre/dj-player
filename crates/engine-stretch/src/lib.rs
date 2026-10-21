//! Time-stretchers for keylock: change speed without changing pitch.
//!
//! [`bungee`] is a pure-Rust port of Bungee Basic with its pull (grain) API, the decks'
//! keylock (see `docs/spikes/keylock-shootout.md` and `docs/research/engine.md`, "Rate,
//! scratching and keylock").
//!
//! Construct a stretcher off the audio thread: construction allocates. After that its grain
//! calls are real-time safe.

pub mod bungee;
