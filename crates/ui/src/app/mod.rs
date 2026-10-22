//! The application around the screens: the engine link, what the UI holds about the loaded
//! track, and (as the migration in `docs/plan/ui-architecture.md` goes on) input, effects and
//! the frame loop.

pub mod effects;
pub mod engine;
pub mod keymap;
pub mod surface;
pub mod track;
