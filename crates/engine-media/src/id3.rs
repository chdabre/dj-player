use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use symphonia::core::io::MediaSource;

use crate::arriving::MediaFile;

/// A file with any leading ID3v2 tags skipped: offset 0 is the first byte after them.
///
/// Symphonia 0.6.1 does not skip some large ID3v2.3 tags (seen with 150 KB of artwork): it
/// finds a false frame sync inside the picture, takes the wrong stream parameters and then
/// fails to decode every packet. Audio never needs the tag, and gapless (LAME/Xing) info sits
/// in the first audio frame after it, so skipping it ourselves loses nothing.
///
/// The file may still be arriving ([`crate::arriving`]): its length is then the final one.
pub(crate) struct AfterId3 {
    file: MediaFile,
    offset: u64,
    len: u64,
}

impl AfterId3 {
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = MediaFile::open(path)?;
        let total = file.len()?;
        let mut offset = 0u64;
        let mut header = [0u8; 10];
        // A file may carry more than one tag back to back.
        for _ in 0..4 {
            file.seek(SeekFrom::Start(offset))?;
            if file.read_exact(&mut header).is_err() || &header[..3] != b"ID3" {
                break;
            }
            let size = header[6..10]
                .iter()
                .fold(0u64, |acc, &b| (acc << 7) | u64::from(b & 0x7f));
            let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
            let end = offset + 10 + size + footer;
            if end >= total {
                break;
            }
            offset = end;
        }
        file.seek(SeekFrom::Start(offset))?;
        Ok(Self {
            file,
            offset,
            len: total - offset,
        })
    }

    /// Whether any tag was skipped, i.e. this reads different bytes than the plain file.
    pub(crate) fn skipped(&self) -> bool {
        self.offset > 0
    }
}

impl Read for AfterId3 {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for AfterId3 {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => self.offset + p,
            SeekFrom::End(d) => (self.offset + self.len).saturating_add_signed(d),
            SeekFrom::Current(d) => self.file.stream_position()?.saturating_add_signed(d),
        };
        let at = self.file.seek(SeekFrom::Start(target.max(self.offset)))?;
        Ok(at - self.offset)
    }
}

impl MediaSource for AfterId3 {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use crate::decode_native;
    use crate::testutil::{id3_tag, tmp, write_wav};

    #[test]
    fn files_behind_a_large_id3_tag_decode_completely() {
        let wav = tmp("tagged-src.wav");
        let frames = 48_000;
        write_wav(&wav, 48_000, 2, &[0.25; 2 * 48_000]);
        let mut bytes = id3_tag(150_000);
        bytes.extend(std::fs::read(&wav).unwrap());
        let tagged = tmp("tagged.wav");
        std::fs::write(&tagged, bytes).unwrap();
        let (audio, rate) = decode_native(&tagged).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(audio.len(), 2 * frames);
    }
}
