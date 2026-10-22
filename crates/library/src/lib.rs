//! Our own library: what the player writes (cues, hot cues, ratings, analysis, waveforms),
//! on a USB stick or a cloud folder, next to a read-only Rekordbox export.
//!
//! See `docs/plan/library.md` for the design. In short:
//!
//! - **Layout:** one `DJPLAYER/` folder holding `library.db` (SQLite, WAL with exclusive
//!   locking, `synchronous=FULL`), `library.db.bak` (made before each mount's first write)
//!   and BLAKE3-named `blobs/<aa>/<hash>` files (waveforms, analysis results).
//! - **Writes** ([`Library`]): the library worker is the only writer. Edits are queued and
//!   committed in one transaction about once a second; if the stick fails, the queue stays
//!   in RAM and is written when the same library is opened again.
//! - **Identity** ([`FileHash`], [`AudioHash`]): a track id comes from the file's content hash at first sight;
//!   an exact duplicate is another location of the same track; a tag-edited copy merges by
//!   its audio hash into the track seen first; a different encode is a separate track.

mod db;
mod hash;
mod ids;
mod library;
mod model;
mod position;
mod time;

pub use hash::{AUDIO_HASH_VERSION, AudioHash, FileHash, track_id};
pub use ids::{BadId, BlobId, DeviceId, EntryId, LibraryId, PlaylistId, TrackId, TrackRef};
pub use library::{
    Edit, Found, Library, MountError, MountOptions, MountStats, Persistence, ReadError, WriteError,
};
pub use model::{
    AnalysisRef, Cue, CueKind, FileIdentity, HOT_CUES, HotCue, Listed, Playlist, PlaylistEntry,
    PlaylistKind, Track, TrackTags, WaveformRef,
};

use std::path::Path;

/// The folder at a stick's (or cloud folder's) root that holds the library.
pub const LIBRARY_DIR: &str = "DJPLAYER";

/// True if `dir` (a [`LIBRARY_DIR`] folder) holds a library file.
pub fn has_library(dir: &Path) -> bool {
    dir.join(db::DB_FILE).is_file()
}

/// `file` as the library stores it: relative to `root`, `/`-separated and NFC. A file
/// outside the root keeps its absolute path (a hint only: identity is the hash).
pub fn stored_path(file: &Path, root: Option<&Path>) -> String {
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = root
        .and_then(|r| r.canonicalize().ok())
        .and_then(|r| abs.strip_prefix(r).ok().map(Path::to_path_buf));
    normalize_path(&rel.unwrap_or(abs).to_string_lossy())
}

/// A file's path as the library stores it: `/` separators and Unicode NFC, so the same
/// name written by macOS (NFD) and anything else compares equal.
fn normalize_path(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.replace('\\', "/").nfc().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn paths_are_stored_relative_to_the_library_root() {
        let dir = std::env::temp_dir().join(format!("library-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Music")).unwrap();
        let f = dir.join("Music").join("a.mp3");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(super::stored_path(&f, Some(&dir)), "Music/a.mp3");
        let outside = super::stored_path(&f, Some(&dir.join("elsewhere")));
        assert!(outside.ends_with("/Music/a.mp3") && outside.starts_with('/'));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paths_are_nfc_with_forward_slashes() {
        // "é" as e + combining acute (NFD, as macOS writes it) becomes the single code point.
        assert_eq!(
            super::normalize_path("Music\\Cafe\u{301}.mp3"),
            "Music/Caf\u{e9}.mp3"
        );
    }
}
