//! Merge, property-tested: random edit histories from three devices give the same state under
//! any replay order, with duplicated segments, and from a snapshot taken at any point.

use std::sync::Arc;

use library::codec::{self, FORMAT_VERSION, SegmentHeader};
use library::{Clock, DeviceId, Library, MemStore, MountOptions, Op, OpKind, Stamp, State, Store};
use proptest::prelude::*;
use serde_json::json;

#[derive(Debug, Clone)]
enum Action {
    Set(u8, u8),
    Add(u8, u8, u8),
    Rm(u8, u8),
    Event(u8, u8),
    Unknown(u8),
    /// Read another device's journal so far (the clock observes it).
    Observe(u8),
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        (0..4u8, 0..5u8).prop_map(|(p, v)| Action::Set(p, v)),
        (0..3u8, 0..6u8, 0..5u8).prop_map(|(p, i, v)| Action::Add(p, i, v)),
        (0..3u8, 0..6u8).prop_map(|(p, i)| Action::Rm(p, i)),
        (0..2u8, 0..5u8).prop_map(|(p, v)| Action::Event(p, v)),
        (0..3u8).prop_map(Action::Unknown),
        (0..3u8).prop_map(Action::Observe),
    ]
}

/// Per device, segments of (wall clock ms, action). Small wall clocks give equal HLCs on
/// different devices, so the device tie-break gets exercised; a wall clock may go backwards.
fn histories() -> impl Strategy<Value = Vec<Vec<Vec<(u64, Action)>>>> {
    prop::collection::vec(
        prop::collection::vec(prop::collection::vec((0..40u64, action()), 0..8), 1..5),
        3,
    )
}

fn device(n: usize) -> DeviceId {
    DeviceId::from_bytes([n as u8 + 1; 16])
}

/// Stamp every action with its device's clock: (device, segment number, ops).
fn stamp(h: &[Vec<Vec<(u64, Action)>>]) -> Vec<(usize, u32, Vec<Op>)> {
    let mut clocks = [Clock::default(); 3];
    // Interleave the devices round-robin, so `Observe` sees a growing journal.
    let mut out = Vec::new();
    let max_segs = h.iter().map(Vec::len).max().unwrap_or(0);
    for seg in 0..max_segs {
        for (d, segs) in h.iter().enumerate() {
            let Some(actions) = segs.get(seg) else {
                continue;
            };
            let mut ops = Vec::new();
            for (wall, a) in actions {
                if let Action::Observe(other) = a {
                    let seen = clocks[usize::from(*other)].last();
                    clocks[d].observe(seen);
                    continue;
                }
                let s = Stamp {
                    hlc: clocks[d].tick(*wall),
                    device: device(d),
                };
                ops.push(match *a {
                    Action::Set(p, v) => Op::set(s, format!("reg/{p}"), json!(v)),
                    Action::Add(p, i, v) => {
                        Op::add(s, format!("set/{p}"), format!("e{i}"), json!({"v": v}))
                    }
                    Action::Rm(p, i) => Op::rm(s, format!("set/{p}"), format!("e{i}")),
                    Action::Event(p, v) => Op::event(s, format!("log/{p}"), json!(v)),
                    Action::Unknown(p) => {
                        let mut op = Op::set(s, format!("future/{p}"), json!(null));
                        op.kind = OpKind::Unknown("move".into());
                        op.extra.insert("to".into(), json!(p));
                        op
                    }
                    Action::Observe(_) => unreachable!(),
                });
            }
            out.push((d, seg as u32 + 1, ops));
        }
    }
    out
}

fn segment_bytes(d: usize, seg: u32, ops: &[Op]) -> Vec<u8> {
    let header = SegmentHeader {
        segment: seg,
        device: device(d),
        format: FORMAT_VERSION,
        opened: Default::default(),
        extra: Default::default(),
    };
    let mut text = codec::frame(&header.to_json());
    for op in ops {
        text += &codec::frame(&op.to_json());
    }
    text.into_bytes()
}

fn segment_path(d: usize, seg: u32) -> String {
    format!("journal/{}/{seg:08}.log", device(d))
}

fn store_with(segments: &[(usize, u32, Vec<Op>)]) -> Arc<MemStore> {
    let store = Arc::new(MemStore::new());
    store
        .create(
            "FORMAT",
            br#"{"format":"djplayer-library","version":1,"library":"00000000-0000-4000-8000-000000000000"}"#,
        )
        .unwrap();
    for (d, seg, ops) in segments {
        store.put(&segment_path(*d, *seg), segment_bytes(*d, *seg, ops));
    }
    store
}

fn mount(store: &Arc<MemStore>) -> Library {
    // A fourth device reads: it owns none of the journals.
    Library::mount(Arc::clone(store), MountOptions::new(device(9))).unwrap()
}

fn replay<'a>(segments: impl IntoIterator<Item = &'a Vec<Op>>) -> State {
    let mut s = State::new();
    for ops in segments {
        for op in ops {
            s.apply(op);
        }
    }
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn any_order_and_duplicates_give_the_same_state(
        h in histories(),
        order in prop::collection::vec(any::<u32>(), 20),
        dups in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
    ) {
        let segments = stamp(&h);
        let reference = mount(&store_with(&segments));
        prop_assert_eq!(reference.state(), &replay(segments.iter().map(|s| &s.2)));

        let mut shuffled: Vec<&Vec<Op>> = segments.iter().map(|s| &s.2).collect();
        for d in &dups {
            shuffled.push(&segments[d.index(segments.len())].2);
        }
        let mut keyed: Vec<(u32, &Vec<Op>)> = shuffled
            .into_iter()
            .enumerate()
            .map(|(i, ops)| (order[i % order.len()].wrapping_mul(i as u32 + 1), ops))
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        prop_assert_eq!(reference.state(), &replay(keyed.into_iter().map(|(_, o)| o)));

        // A duplicated segment file (copied under another number) changes nothing either.
        let store = store_with(&segments);
        for d in &dups {
            let (dev, seg, ops) = &segments[d.index(segments.len())];
            store.put(&segment_path(*dev, seg + 100), segment_bytes(*dev, *seg, ops));
        }
        let m = mount(&store);
        prop_assert_eq!(reference.state(), m.state());
    }

    #[test]
    fn a_snapshot_at_any_point_gives_the_same_state(
        h in histories(),
        cut in prop::collection::vec(0..6u32, 3),
    ) {
        let segments = stamp(&h);
        let reference = mount(&store_with(&segments));

        // What a device saw when it snapshotted: the first `cut[d]` segments of each device.
        let seen: Vec<_> = segments
            .iter()
            .filter(|(d, seg, _)| *seg <= cut[*d])
            .cloned()
            .collect();
        let snap = mount(&store_with(&seen)).snapshot_bytes();

        // Snapshot plus every segment.
        let store = store_with(&segments);
        store.put("snapshots/0000000000000001-x.snap", snap.clone());
        let after = mount(&store);
        prop_assert_eq!(reference.state(), after.state());

        // Snapshot plus only the segments it does not cover (the rest compacted away).
        let store = store_with(&segments);
        store.put("snapshots/0000000000000001-x.snap", snap);
        for (d, seg, _) in &segments {
            if *seg < cut[*d] {
                store.remove(&segment_path(*d, *seg)).unwrap();
            }
        }
        let compacted = mount(&store);
        prop_assert_eq!(reference.state(), compacted.state());
        prop_assert_eq!(compacted.state().max_hlc(), reference.state().max_hlc());
    }

    #[test]
    fn two_snapshots_merge(h in histories(), a in 0..6u32, b in 0..6u32) {
        let segments = stamp(&h);
        let reference = mount(&store_with(&segments));
        let snap_of = |cut: u32| {
            let seen: Vec<_> = segments.iter().filter(|(_, s, _)| *s <= cut).cloned().collect();
            mount(&store_with(&seen)).snapshot_bytes()
        };
        let store = store_with(&segments);
        store.put("snapshots/0000000000000001-a.snap", snap_of(a));
        store.put("snapshots/0000000000000002-b.snap", snap_of(b));
        let m = mount(&store);
        prop_assert_eq!(reference.state(), m.state());
    }
}

#[test]
fn torn_and_corrupt_lines_are_skipped_on_mount() {
    let d = device(0);
    let s = |ms| Stamp {
        hlc: library::Hlc::new(ms, 0),
        device: d,
    };
    let ops = vec![
        Op::set(s(1), "reg/a", json!(1)),
        Op::set(s(2), "reg/b", json!(2)),
        Op::set(s(3), "reg/c", json!(3)),
    ];
    let mut bytes = segment_bytes(0, 1, &ops);
    // Corrupt the middle line's value and tear the last one.
    let text = String::from_utf8(bytes.clone()).unwrap();
    let corrupted = text.replacen("\"value\":2", "\"value\":7", 1);
    bytes = corrupted.into_bytes();
    bytes.truncate(bytes.len() - 4);
    let store = store_with(&[]);
    store.put(&segment_path(0, 1), bytes);
    let mut lib = mount(&store);
    assert_eq!(lib.state().value("reg/a"), Some(&json!(1)));
    assert_eq!(lib.state().value("reg/b"), None);
    assert_eq!(lib.state().value("reg/c"), None);
    assert_eq!(lib.stats().skipped_lines, 2);
    assert!(!lib.take_warnings().is_empty());
}
