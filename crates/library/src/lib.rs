//! Our own library format: what the player writes (cues, hot cues, ratings, analysis,
//! waveforms), on a USB stick or a cloud folder, next to a read-only Rekordbox export.
//!
//! See `docs/plan/library.md` for the design. In short:
//!
//! - **Layout:** one `DJPLAYER/` folder: a `FORMAT` file, `devices/`, one journal folder per
//!   writer device (`journal/<device>/<segment>.log`), `snapshots/` and BLAKE3-named
//!   `blobs/<aa>/<hash>`, written once via `tmp/` and a rename.
//! - **Records** ([`codec`]): CRC-32C-framed JSON lines; four HLC-stamped operations, `set`,
//!   `add`, `rm` and `event`, each carrying its author device.
//! - **Merge** ([`state`]): registers are last-writer-wins by HLC, sets have per-element ids
//!   and tombstones, logs are unions. Replaying journals in any order, twice, or after a
//!   snapshot gives the same state.
//! - **Identity** ([`hash`]): a track id comes from the file's content hash at first sight;
//!   an exact duplicate is another location of the same track; a tag-edited copy merges
//!   by its audio hash through an alias; a different encode is a separate track.
//!
//! There is no I/O here except through [`Store`], so everything tests offline.

pub mod codec;
pub mod hash;
pub mod hlc;
pub mod ids;
mod library;
pub mod model;
pub mod state;
pub mod store;

pub use codec::{FORMAT_VERSION, Op, OpKind};
pub use hash::{AUDIO_HASH_VERSION, AudioHash, ContentHasher, FileHash, track_id};
pub use hlc::{Clock, Hlc, Stamp};
pub use ids::{BadId, BlobId, DeviceId, LibraryId, TrackId, TrackRef};
pub use library::{
    Edit, Found, Libraries, Library, MountError, MountOptions, MountStats, Persistence,
    SEGMENT_ROTATE_BYTES, WriteError, rfc3339, system_ms,
};
pub use model::{AnalysisRef, Cue, CueKind, FileIdentity, HOT_CUES, HotCue, Track, WaveformRef};
pub use state::State;
pub use store::{FolderStore, MemStore, Store};

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
