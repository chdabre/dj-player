//! The input vocabulary, from the shared `surface` crate: what the player's physical controls
//! report, whoever stands in for them. On the device the RP2040 control surface sends these;
//! on a laptop the stand-in (`standin/`) and the dev keyboard (`app/keymap.rs`) do. What a
//! control *does* is decided in one place, `engined::controls`, so the stand-in, the keyboard
//! and the hardware behave alike.

#[cfg(feature = "standin")]
pub use engined::controls::Jog;
#[cfg(feature = "standin")]
pub use surface::SECONDS_PER_TURN;
pub use surface::{Control, ControlEvent, Nav};
