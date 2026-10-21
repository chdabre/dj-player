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
//! - **Identity** ([`hash`]): a track id comes from the file's content hash at first sight;
//!   an exact duplicate is another location of the same track; a tag-edited copy merges by
//!   its audio hash into the track seen first; a different encode is a separate track.

pub mod db;
pub mod hash;
pub mod ids;
mod library;
pub mod model;

pub use hash::{AUDIO_HASH_VERSION, AudioHash, ContentHasher, FileHash, track_id};
pub use ids::{BadId, BlobId, DeviceId, LibraryId, TrackId, TrackRef};
pub use library::{
    Edit, Found, Library, MountError, MountOptions, MountStats, Persistence, ReadError, WriteError,
    rfc3339, system_ms,
};
pub use model::{AnalysisRef, Cue, CueKind, FileIdentity, HOT_CUES, HotCue, Track, WaveformRef};

/// A file's path as the library stores it: `/` separators and Unicode NFC, so the same
/// name written by macOS (NFD) and anything else compares equal.
pub fn normalize_path(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.replace('\\', "/").nfc().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn paths_are_nfc_with_forward_slashes() {
        // "é" as e + combining acute (NFD, as macOS writes it) becomes the single code point.
        assert_eq!(
            super::normalize_path("Music\\Cafe\u{301}.mp3"),
            "Music/Caf\u{e9}.mp3"
        );
    }
}
