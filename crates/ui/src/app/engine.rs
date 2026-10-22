//! The UI's view of the player. Everything the UI knows about the deck comes from one
//! [`DeckSnapshot`], read once per frame through the [`player_api::Port`]; the library's
//! state from [`LibraryView`]. When the player moves to its own process, a client fills these
//! instead and nothing above this module changes.

pub use player_api::DeckSnapshot;

/// The library as input handlers may look at it: what it lists and what it is doing. Changes
/// go through `Effect`s.
pub type LibraryView<'a> = player_api::LibraryState<'a>;
