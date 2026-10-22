//! The one place the UI names the player's implementation: it starts `engined`'s [`Player`]
//! in this process and hands it on as a [`Port`]. A UI talking to a player in another process
//! would connect here instead.

use std::path::Path;
use std::process::ExitCode;

use engined::{CpalBackend, LibraryChoice, Options, Player};
use player_api::Port;

use crate::args::Args;

/// Start the player with the library `args` (or the stick) asks for; also whether to ask
/// "scan this stick?" (a stick with no library is asked once, before mounting makes one).
pub(super) fn start_player(
    args: &Args,
    root: Option<&Path>,
) -> Result<(Box<dyn Port>, bool), ExitCode> {
    let library = match (args.library.clone(), root) {
        (Some(choice), _) => choice,
        (None, Some(r)) => LibraryChoice::Folder(r.join(engined::store::LIBRARY_DIR)),
        (None, None) => LibraryChoice::Auto,
    };
    let ask_scan = args.library.is_none() && root.is_some_and(|r| !engined::store::has_library(r));
    let backend = CpalBackend::new();
    let options = Options {
        library,
        auto_cue: args.auto_cue,
        ..Options::default()
    };
    let player = match Player::start(&backend, &args.request, options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dj-ui: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    eprintln!(
        "{} Hz, {} ch, buffer {}, {}",
        player.format.sample_rate,
        player.format.channels,
        player
            .buffer_frames()
            .map_or("default".into(), |n| format!("{n} frames")),
        player.device_name()
    );
    Ok((Box::new(player), ask_scan))
}
