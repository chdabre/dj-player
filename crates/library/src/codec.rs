//! Record format: CRC-32C-framed JSON lines.
//!
//! Each line is `<crc32c, 8 hex> <json>\n`; the CRC covers the JSON bytes. A line with a bad
//! CRC, bad JSON or no newline (a torn write) is skipped and counted, and reading goes on.
//!
//! A journal segment starts with a header line (`{"segment":7,"device":…,"format":1,
//! "opened":{"hlc":…}}`), a snapshot with its own (`{"snapshot":1,…,"vector":{…}}`); every
//! other line is one [`Op`]. Fields this version does not know are kept and written back.

use serde_json::{Map, Value, json};

use crate::hlc::{Hlc, Stamp};
use crate::ids::DeviceId;

/// The record format version, in `FORMAT` and in every header.
pub const FORMAT_VERSION: u32 = 1;

/// One operation. That is the whole grammar: a new feature adds paths, not operations.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    /// HLC and author device. The author is who made the edit, not necessarily the device
    /// whose journal holds it (a linked player forwards its edits to the stick's holder).
    pub stamp: Stamp,
    /// Entity and field, e.g. `track/<id>/rating`.
    pub path: String,
    pub kind: OpKind,
    /// Fields of the line this version does not know; written back unchanged.
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpKind {
    /// A register; `null` clears it.
    Set(Value),
    /// An element of a set, by its id.
    Add {
        id: String,
        value: Value,
    },
    Rm {
        id: String,
    },
    /// Appended to a log.
    Event(Value),
    /// An operation a newer version wrote; kept and written back, never interpreted.
    Unknown(String),
}

impl Op {
    pub fn set(stamp: Stamp, path: impl Into<String>, value: Value) -> Self {
        Self::new(stamp, path, OpKind::Set(value))
    }

    pub fn add(stamp: Stamp, path: impl Into<String>, id: impl Into<String>, value: Value) -> Self {
        let id = id.into();
        Self::new(stamp, path, OpKind::Add { id, value })
    }

    pub fn rm(stamp: Stamp, path: impl Into<String>, id: impl Into<String>) -> Self {
        Self::new(stamp, path, OpKind::Rm { id: id.into() })
    }

    pub fn event(stamp: Stamp, path: impl Into<String>, value: Value) -> Self {
        Self::new(stamp, path, OpKind::Event(value))
    }

    fn new(stamp: Stamp, path: impl Into<String>, kind: OpKind) -> Self {
        Op {
            stamp,
            path: path.into(),
            kind,
            extra: Map::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("hlc".into(), self.stamp.hlc.to_string().into());
        m.insert("dev".into(), self.stamp.device.to_string().into());
        m.insert("path".into(), self.path.clone().into());
        let op = match &self.kind {
            OpKind::Set(v) => {
                m.insert("value".into(), v.clone());
                "set"
            }
            OpKind::Add { id, value } => {
                m.insert("id".into(), id.clone().into());
                m.insert("value".into(), value.clone());
                "add"
            }
            OpKind::Rm { id } => {
                m.insert("id".into(), id.clone().into());
                "rm"
            }
            OpKind::Event(v) => {
                m.insert("value".into(), v.clone());
                "event"
            }
            OpKind::Unknown(name) => name,
        };
        m.insert("op".into(), op.into());
        Value::Object(m)
    }

    /// Parse an operation line's JSON. `None` if it lacks what every operation has (`hlc`,
    /// `dev`, `op`, `path`) or a known operation lacks its own fields.
    pub fn from_json(v: Value) -> Option<Op> {
        let Value::Object(mut m) = v else {
            return None;
        };
        let hlc: Hlc = take_str(&mut m, "hlc")?.parse().ok()?;
        let device: DeviceId = take_str(&mut m, "dev")?.parse().ok()?;
        let op = take_str(&mut m, "op")?;
        let path = take_str(&mut m, "path")?;
        let kind = match op.as_str() {
            "set" => OpKind::Set(m.remove("value")?),
            "add" => OpKind::Add {
                id: take_str(&mut m, "id")?,
                value: m.remove("value")?,
            },
            "rm" => OpKind::Rm {
                id: take_str(&mut m, "id")?,
            },
            "event" => OpKind::Event(m.remove("value")?),
            _ => OpKind::Unknown(op),
        };
        Some(Op {
            stamp: Stamp { hlc, device },
            path,
            kind,
            extra: m,
        })
    }
}

fn take_str(m: &mut Map<String, Value>, key: &str) -> Option<String> {
    match m.remove(key)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// The first line of a journal segment.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentHeader {
    pub segment: u32,
    /// The device whose folder holds the segment (its only writer).
    pub device: DeviceId,
    pub format: u32,
    /// The writer's HLC when it opened the segment.
    pub opened: Hlc,
    pub extra: Map<String, Value>,
}

impl SegmentHeader {
    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("segment".into(), self.segment.into());
        m.insert("device".into(), self.device.to_string().into());
        m.insert("format".into(), self.format.into());
        m.insert("opened".into(), json!({ "hlc": self.opened.to_string() }));
        Value::Object(m)
    }

    fn from_json(m: &Map<String, Value>) -> Option<Self> {
        let mut extra = m.clone();
        let segment = u32::try_from(extra.remove("segment")?.as_u64()?).ok()?;
        let device = take_str(&mut extra, "device")?.parse().ok()?;
        let format = u32::try_from(extra.remove("format")?.as_u64()?).ok()?;
        let opened = extra.remove("opened")?.get("hlc")?.as_str()?.parse().ok()?;
        Some(Self {
            segment,
            device,
            format,
            opened,
            extra,
        })
    }
}

/// The first line of a snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotHeader {
    /// The device that wrote it.
    pub device: DeviceId,
    pub format: u32,
    /// The highest HLC in the state it holds.
    pub hlc: Hlc,
    /// Per writer device, the highest journal segment whose every line the snapshot holds.
    /// Later lines of that segment may exist; replaying them again is harmless.
    pub vector: std::collections::BTreeMap<DeviceId, u32>,
    pub extra: Map<String, Value>,
}

impl SnapshotHeader {
    pub fn to_json(&self) -> Value {
        let mut m = self.extra.clone();
        m.insert("snapshot".into(), 1.into());
        m.insert("device".into(), self.device.to_string().into());
        m.insert("format".into(), self.format.into());
        m.insert("hlc".into(), self.hlc.to_string().into());
        let vector: Map<String, Value> = self
            .vector
            .iter()
            .map(|(d, s)| (d.to_string(), Value::from(*s)))
            .collect();
        m.insert("vector".into(), Value::Object(vector));
        Value::Object(m)
    }

    fn from_json(m: &Map<String, Value>) -> Option<Self> {
        let mut extra = m.clone();
        extra.remove("snapshot")?;
        let device = take_str(&mut extra, "device")?.parse().ok()?;
        let format = u32::try_from(extra.remove("format")?.as_u64()?).ok()?;
        let hlc = take_str(&mut extra, "hlc")?.parse().ok()?;
        let Value::Object(v) = extra.remove("vector")? else {
            return None;
        };
        let mut vector = std::collections::BTreeMap::new();
        for (d, s) in v {
            vector.insert(d.parse().ok()?, u32::try_from(s.as_u64()?).ok()?);
        }
        Some(Self {
            device,
            format,
            hlc,
            vector,
            extra,
        })
    }
}

/// One line, framed: `<crc32c 8 hex> <json>\n`.
pub fn frame(json: &Value) -> String {
    let text = json.to_string();
    format!("{:08x} {text}\n", crc32c::crc32c(text.as_bytes()))
}

/// What a line turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Segment(SegmentHeader),
    Snapshot(SnapshotHeader),
    Op(Op),
}

/// A file read back: every good line, and how many were skipped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    pub lines: Vec<Line>,
    /// Lines with a bad CRC, bad JSON, an unusable header or op, or no newline (torn).
    pub skipped: usize,
    /// Of those, a torn last line.
    pub torn: bool,
}

/// Read a segment or snapshot file. Never fails: what can't be read is skipped and counted.
pub fn decode(bytes: &[u8]) -> Decoded {
    let mut out = Decoded::default();
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
            out.skipped += 1;
            out.torn = true;
            break;
        };
        let line = &rest[..nl];
        rest = &rest[nl + 1..];
        match parse_line(line) {
            Some(l) => out.lines.push(l),
            None => out.skipped += 1,
        }
    }
    out
}

fn parse_line(line: &[u8]) -> Option<Line> {
    let crc_hex = std::str::from_utf8(line.get(..8)?).ok()?;
    let crc = u32::from_str_radix(crc_hex, 16).ok()?;
    if line.get(8) != Some(&b' ') {
        return None;
    }
    let body = &line[9..];
    if crc32c::crc32c(body) != crc {
        return None;
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    let m = v.as_object()?;
    if m.contains_key("segment") {
        SegmentHeader::from_json(m).map(Line::Segment)
    } else if m.contains_key("snapshot") {
        SnapshotHeader::from_json(m).map(Line::Snapshot)
    } else {
        Op::from_json(v).map(Line::Op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(ms: u64) -> Stamp {
        Stamp {
            hlc: Hlc::new(ms, 0),
            device: "3f2a0000-0000-4000-8000-000000000001".parse().unwrap(),
        }
    }

    fn ops() -> Vec<Op> {
        vec![
            Op::set(stamp(1), "track/9c4e/rating", json!(4)),
            Op::add(
                stamp(2),
                "track/9c4e/cue",
                "x@y",
                json!({"kind": "memory", "at": 61.284}),
            ),
            Op::rm(stamp(3), "track/9c4e/cue", "x@y"),
            Op::event(stamp(4), "history", json!({"track": "9c4e"})),
        ]
    }

    #[test]
    fn every_operation_round_trips() {
        let text: String = ops().iter().map(|o| frame(&o.to_json())).collect();
        let d = decode(text.as_bytes());
        assert_eq!(d.skipped, 0);
        let back: Vec<Op> = d
            .lines
            .into_iter()
            .map(|l| match l {
                Line::Op(o) => o,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(back, ops());
    }

    #[test]
    fn torn_and_corrupt_lines_are_skipped() {
        let lines: Vec<String> = ops().iter().map(|o| frame(&o.to_json())).collect();
        let mut text = String::new();
        text += &lines[0];
        // A flipped byte in the JSON: CRC mismatch.
        text += &lines[1].replacen("61.284", "61.285", 1);
        text += "garbage\n";
        text += &lines[2];
        // Torn: the last line lost its end.
        text += &lines[3][..lines[3].len() - 5];
        let d = decode(text.as_bytes());
        assert_eq!(d.lines.len(), 2);
        assert_eq!(d.skipped, 3);
        assert!(d.torn);
    }

    #[test]
    fn unknown_fields_and_operations_survive() {
        let line = r#"{"hlc":"000000000005-0000","dev":"3f2a0000-0000-4000-8000-000000000001","op":"move","path":"playlist/1/entry","from":2,"to":7,"sig":"abc"}"#;
        let op = Op::from_json(serde_json::from_str(line).unwrap()).unwrap();
        assert_eq!(op.kind, OpKind::Unknown("move".into()));
        assert_eq!(op.extra.get("sig"), Some(&json!("abc")));
        let again: Value = serde_json::from_str(line).unwrap();
        assert_eq!(op.to_json(), again);

        let mut set = ops()[0].clone();
        set.extra.insert("future".into(), json!({"a": 1}));
        let d = decode(frame(&set.to_json()).as_bytes());
        assert_eq!(d.lines, vec![Line::Op(set)]);
    }

    #[test]
    fn headers_round_trip() {
        let dev: DeviceId = "3f2a0000-0000-4000-8000-000000000001".parse().unwrap();
        let seg = SegmentHeader {
            segment: 7,
            device: dev,
            format: FORMAT_VERSION,
            opened: Hlc::new(9, 1),
            extra: Map::new(),
        };
        let snap = SnapshotHeader {
            device: dev,
            format: FORMAT_VERSION,
            hlc: Hlc::new(10, 0),
            vector: [(dev, 7)].into_iter().collect(),
            extra: Map::new(),
        };
        let text = frame(&seg.to_json()) + &frame(&snap.to_json());
        let d = decode(text.as_bytes());
        assert_eq!(d.lines, vec![Line::Segment(seg), Line::Snapshot(snap)]);
    }
}
