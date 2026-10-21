//! Track identity hashes.
//!
//! - **File hash:** Dropbox's `content_hash`, computed locally for every file: SHA-256 over
//!   the concatenated SHA-256 digests of the file's 4 MiB blocks. The same file on a stick and
//!   in Dropbox then matches without a download.
//! - **Track id:** the first 16 bytes of BLAKE3 over `"track" ‖ file_hash` (the 32 raw digest
//!   bytes), taken when the file is first seen. Two devices that find the same file get the
//!   same id.
//! - **Audio hash:** versioned BLAKE3 over the decoded audio as 16-bit PCM at the file's own
//!   rate. A tag edit changes the file hash but not this.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::ids::{TrackId, hex, unhex};

/// Dropbox's block size for `content_hash`.
pub const BLOCK: usize = 4 * 1024 * 1024;

/// The file hash (Dropbox `content_hash`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileHash(pub [u8; 32]);

impl FileHash {
    pub fn of(bytes: &[u8]) -> Self {
        let mut h = ContentHasher::new();
        h.update(bytes);
        h.finish()
    }

    pub fn parse(s: &str) -> Option<Self> {
        unhex(s).map(Self)
    }
}

impl fmt::Display for FileHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for FileHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileHash({self})")
    }
}

/// Incremental [`FileHash`]: feed the file in pieces of any size.
#[derive(Clone, Default)]
pub struct ContentHasher {
    overall: Sha256,
    block: Sha256,
    in_block: usize,
}

impl ContentHasher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = (BLOCK - self.in_block).min(bytes.len());
            self.block.update(&bytes[..n]);
            self.in_block += n;
            bytes = &bytes[n..];
            if self.in_block == BLOCK {
                self.end_block();
            }
        }
    }

    fn end_block(&mut self) {
        let digest = std::mem::take(&mut self.block).finalize();
        self.overall.update(digest);
        self.in_block = 0;
    }

    pub fn finish(mut self) -> FileHash {
        if self.in_block > 0 {
            self.end_block();
        }
        FileHash(self.overall.finalize().into())
    }
}

/// The track id a file gets when it is first seen.
pub fn track_id(file_hash: &FileHash) -> TrackId {
    let mut h = blake3::Hasher::new();
    h.update(b"track");
    h.update(&file_hash.0);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h.finalize().as_bytes()[..16]);
    TrackId(id)
}

/// Version of the audio hash this build computes. Bump it when the decoder or the
/// conversion changes in a way that changes the PCM; stored hashes of an older version are
/// recomputed on load.
pub const AUDIO_HASH_VERSION: u32 = 1;

/// A versioned hash of the decoded audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AudioHash {
    pub v: u32,
    pub hash: [u8; 32],
}

impl AudioHash {
    /// Version 1: BLAKE3 over interleaved samples as signed 16-bit little-endian, each
    /// `round(x · 32768)` clamped to the 16-bit range. `samples` is the decoder's output at
    /// the file's own rate (our decoder gives interleaved stereo, gapless-trimmed), so a
    /// 16-bit source hashes its own sample values.
    pub fn of_pcm(samples: &[f32]) -> Self {
        let mut h = blake3::Hasher::new();
        let mut buf = [0u8; 8192];
        for chunk in samples.chunks(buf.len() / 2) {
            for (s, out) in chunk.iter().zip(buf.chunks_exact_mut(2)) {
                let v = (f64::from(*s) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                out.copy_from_slice(&v.to_le_bytes());
            }
            h.update(&buf[..chunk.len() * 2]);
        }
        AudioHash {
            v: AUDIO_HASH_VERSION,
            hash: *h.finalize().as_bytes(),
        }
    }

    pub fn is_current(&self) -> bool {
        self.v == AUDIO_HASH_VERSION
    }

    pub fn hash_hex(&self) -> String {
        hex(&self.hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> [u8; 32] {
        Sha256::digest(b).into()
    }

    #[test]
    fn content_hash_is_sha256_of_block_digests() {
        let data: Vec<u8> = (0..BLOCK + BLOCK / 2)
            .map(|i| (i * 7 % 251) as u8)
            .collect();
        let mut cat = Vec::new();
        cat.extend_from_slice(&sha(&data[..BLOCK]));
        cat.extend_from_slice(&sha(&data[BLOCK..]));
        let expected = FileHash(sha(&cat));
        assert_eq!(FileHash::of(&data), expected);
        // Fed in odd pieces, the result is the same.
        let mut h = ContentHasher::new();
        for piece in data.chunks(999_999) {
            h.update(piece);
        }
        assert_eq!(h.finish(), expected);
    }

    #[test]
    fn exact_multiple_of_the_block_has_no_empty_trailing_block() {
        let data = vec![1u8; BLOCK];
        assert_eq!(FileHash::of(&data), FileHash(sha(&sha(&data))));
        // An empty file hashes no blocks at all.
        assert_eq!(FileHash::of(&[]), FileHash(sha(&[])));
    }

    #[test]
    fn track_ids_depend_only_on_the_file_hash() {
        let a = FileHash::of(b"one");
        assert_eq!(track_id(&a), track_id(&FileHash::of(b"one")));
        assert_ne!(track_id(&a), track_id(&FileHash::of(b"two")));
    }

    #[test]
    fn audio_hash_of_16_bit_values_is_exact() {
        let pcm: Vec<f32> = [0i16, 1, -1, 12345, -32768, 32767]
            .iter()
            .map(|&s| f32::from(s) / 32768.0)
            .collect();
        let mut bytes = Vec::new();
        for s in [0i16, 1, -1, 12345, -32768, 32767] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let h = AudioHash::of_pcm(&pcm);
        assert_eq!(h.hash, *blake3::hash(&bytes).as_bytes());
        assert!(h.is_current());
        // Out-of-range float samples clamp rather than wrap.
        assert_eq!(
            AudioHash::of_pcm(&[2.0]),
            AudioHash::of_pcm(&[32767.0 / 32768.0])
        );
    }
}
