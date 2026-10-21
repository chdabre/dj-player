//! Identifiers: devices and libraries (random UUIDs), tracks (from the file hash) and blobs
//! (BLAKE3 of their bytes). All are written as lowercase hex in the files.

use std::fmt;
use std::str::FromStr;

/// A string that is not a valid id of the expected kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadId(pub String);

impl fmt::Display for BadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad id {:?}", self.0)
    }
}

impl std::error::Error for BadId {}

/// A random version-4 UUID, as 16 bytes.
fn random_uuid() -> [u8; 16] {
    let mut b = [0u8; 16];
    // Without an entropy source there is no way to make a unique id; that is fatal.
    getrandom::fill(&mut b).expect("no entropy source for a UUID");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b
}

fn format_uuid(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let groups: Vec<&str> = s.split('-').collect();
    let lens: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    if lens != [8, 4, 4, 4, 12] {
        return None;
    }
    unhex(&groups.concat())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 15)] as char);
    }
    s
}

pub(crate) fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != 2 * N || !s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            /// A new random id.
            pub fn random() -> Self {
                Self(random_uuid())
            }

            pub fn from_bytes(b: [u8; 16]) -> Self {
                Self(b)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&format_uuid(&self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = BadId;
            fn from_str(s: &str) -> Result<Self, BadId> {
                parse_uuid(s).map(Self).ok_or_else(|| BadId(s.into()))
            }
        }
    };
}

uuid_id!(
    /// A device (player, laptop): a random UUID made on first boot and kept on the device's
    /// boot storage, never on a stick.
    DeviceId
);

uuid_id!(
    /// A library: the UUID in its `FORMAT` file.
    LibraryId
);

/// A track: the first 16 bytes of BLAKE3 over `"track" ‖ file_hash` when the file was first
/// seen (see [`crate::hash::track_id`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub [u8; 16]);

impl fmt::Display for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for TrackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TrackId({self})")
    }
}

impl FromStr for TrackId {
    type Err = BadId;
    fn from_str(s: &str) -> Result<Self, BadId> {
        unhex(s).map(Self).ok_or_else(|| BadId(s.into()))
    }
}

/// A blob: BLAKE3 of its bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Where the blob lives: `blobs/<aa>/<64 hex>`.
    pub fn path(&self) -> String {
        let h = hex(&self.0);
        format!("blobs/{}/{h}", &h[..2])
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobId({self})")
    }
}

impl FromStr for BlobId {
    type Err = BadId;
    fn from_str(s: &str) -> Result<Self, BadId> {
        unhex(s).map(Self).ok_or_else(|| BadId(s.into()))
    }
}

/// A track in one of the libraries a player sees: tracks are always addressed this way, as
/// the same track id can exist in several libraries with separate data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackRef {
    pub library: LibraryId,
    pub track: TrackId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_text() {
        let d = DeviceId::random();
        assert_eq!(d.to_string().parse::<DeviceId>(), Ok(d));
        assert_eq!(d.to_string().len(), 36);
        let t = TrackId([0xab; 16]);
        assert_eq!(t.to_string().parse::<TrackId>(), Ok(t));
        let b = BlobId::of(b"x");
        assert_eq!(b.to_string().parse::<BlobId>(), Ok(b));
        assert!(
            b.path()
                .starts_with(&format!("blobs/{}/", &b.to_string()[..2]))
        );
        assert!("not-a-uuid".parse::<DeviceId>().is_err());
        assert!("ABCD".parse::<TrackId>().is_err());
    }

    #[test]
    fn random_uuids_are_version_4() {
        let s = LibraryId::random().to_string();
        assert_eq!(&s[14..15], "4");
        assert!(matches!(&s[19..20], "8" | "9" | "a" | "b"));
    }
}
