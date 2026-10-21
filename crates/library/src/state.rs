//! The merged state: every operation folded in by rules that are commutative, associative and
//! idempotent, so replaying journals in any order, or twice, gives the same state.
//!
//! - **Register** (`set`): the highest stamp wins (last writer wins).
//! - **Set** (`add` / `rm`): an element is present if its id was added and never removed. A
//!   removed id stays a tombstone, so a late `add` of it can't resurrect it; a re-add gets a
//!   new id. Two adds of one id keep the higher stamp's value.
//! - **Log** (`event`): the union of events, keyed by stamp, so duplicates collapse.
//!
//! Paths are not interpreted here: an unknown path merges by its operation like any other,
//! and an unknown operation is kept as it is.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::codec::{Op, OpKind};
use crate::hlc::{Hlc, Stamp};

/// A register's value, or a present set element's, or an event, with the op that wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub stamp: Stamp,
    pub value: Value,
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Added(Entry),
    /// A tombstone: the highest stamp that removed this id.
    Removed {
        stamp: Stamp,
        extra: Map<String, Value>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    registers: BTreeMap<String, Entry>,
    sets: BTreeMap<String, BTreeMap<String, Element>>,
    logs: BTreeMap<String, BTreeMap<Stamp, Entry>>,
    unknown: BTreeMap<(Stamp, String), Op>,
    max_hlc: Hlc,
}

/// `a` beats `b`: higher stamp, and on an exact tie (never made by a correct device) the
/// larger value text, so even then every replica picks the same one.
fn wins(a: &Entry, b: &Entry) -> bool {
    match a.stamp.cmp(&b.stamp) {
        std::cmp::Ordering::Equal => {
            let (x, y) = (a.value.to_string(), b.value.to_string());
            x > y
        }
        o => o.is_gt(),
    }
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    /// The highest HLC of any operation applied.
    pub fn max_hlc(&self) -> Hlc {
        self.max_hlc
    }

    /// Raise [`State::max_hlc`] to at least `hlc`: a snapshot records the highest HLC it
    /// saw, which may belong to an operation it no longer needs to keep.
    pub fn observe(&mut self, hlc: Hlc) {
        self.max_hlc = self.max_hlc.max(hlc);
    }

    pub fn apply(&mut self, op: &Op) {
        self.max_hlc = self.max_hlc.max(op.stamp.hlc);
        let entry = |value: &Value| Entry {
            stamp: op.stamp,
            value: value.clone(),
            extra: op.extra.clone(),
        };
        match &op.kind {
            OpKind::Set(value) => {
                let new = entry(value);
                match self.registers.get_mut(&op.path) {
                    Some(old) if !wins(&new, old) => {}
                    Some(old) => *old = new,
                    None => {
                        self.registers.insert(op.path.clone(), new);
                    }
                }
            }
            OpKind::Add { id, value } => {
                let set = self.sets.entry(op.path.clone()).or_default();
                let new = entry(value);
                match set.get_mut(id) {
                    Some(Element::Removed { .. }) => {}
                    Some(Element::Added(old)) => {
                        if wins(&new, old) {
                            *old = new;
                        }
                    }
                    None => {
                        set.insert(id.clone(), Element::Added(new));
                    }
                }
            }
            OpKind::Rm { id } => {
                let set = self.sets.entry(op.path.clone()).or_default();
                let tomb = Element::Removed {
                    stamp: op.stamp,
                    extra: op.extra.clone(),
                };
                match set.get_mut(id) {
                    Some(Element::Removed { stamp, .. }) if *stamp >= op.stamp => {}
                    Some(e) => *e = tomb,
                    None => {
                        set.insert(id.clone(), tomb);
                    }
                }
            }
            OpKind::Event(value) => {
                let log = self.logs.entry(op.path.clone()).or_default();
                let new = entry(value);
                match log.get_mut(&op.stamp) {
                    Some(old) if !wins(&new, old) => {}
                    Some(old) => *old = new,
                    None => {
                        log.insert(op.stamp, new);
                    }
                }
            }
            OpKind::Unknown(_) => {
                self.unknown
                    .entry((op.stamp, op.path.clone()))
                    .or_insert_with(|| op.clone());
            }
        }
    }

    pub fn register(&self, path: &str) -> Option<&Entry> {
        self.registers.get(path)
    }

    /// The register's value, unless it is absent or cleared (`null`).
    pub fn value(&self, path: &str) -> Option<&Value> {
        self.register(path)
            .map(|e| &e.value)
            .filter(|v| !v.is_null())
    }

    /// Registers whose path starts with `prefix`, in path order.
    pub fn registers_under<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Entry)> + 'a {
        self.registers
            .range::<str, _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v))
    }

    /// Every element of a set, tombstones included.
    pub fn elements<'a>(
        &'a self,
        path: &str,
    ) -> impl Iterator<Item = (&'a str, &'a Element)> + use<'a> {
        self.sets
            .get(path)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.as_str(), v))
    }

    /// The present elements of a set.
    pub fn present<'a>(
        &'a self,
        path: &str,
    ) -> impl Iterator<Item = (&'a str, &'a Entry)> + use<'a> {
        self.elements(path).filter_map(|(id, e)| match e {
            Element::Added(entry) => Some((id, entry)),
            Element::Removed { .. } => None,
        })
    }

    /// Paths of the sets whose path starts with `prefix`.
    pub fn sets_under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.sets
            .range::<str, _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k.as_str())
    }

    /// A log's events in stamp order.
    pub fn events<'a>(&'a self, path: &str) -> impl Iterator<Item = &'a Entry> + use<'a> {
        self.logs.get(path).into_iter().flat_map(|l| l.values())
    }

    /// Every operation needed to rebuild this state, as a snapshot writes it: a `set` per
    /// register, an `add` per present element, an `rm` per tombstone, an `event` per event,
    /// and the unknown operations, each with its original stamp.
    pub fn ops(&self) -> impl Iterator<Item = Op> + '_ {
        let regs = self.registers.iter().map(|(path, e)| Op {
            stamp: e.stamp,
            path: path.clone(),
            kind: OpKind::Set(e.value.clone()),
            extra: e.extra.clone(),
        });
        let sets = self.sets.iter().flat_map(|(path, elems)| {
            elems.iter().map(move |(id, el)| match el {
                Element::Added(e) => Op {
                    stamp: e.stamp,
                    path: path.clone(),
                    kind: OpKind::Add {
                        id: id.clone(),
                        value: e.value.clone(),
                    },
                    extra: e.extra.clone(),
                },
                Element::Removed { stamp, extra } => Op {
                    stamp: *stamp,
                    path: path.clone(),
                    kind: OpKind::Rm { id: id.clone() },
                    extra: extra.clone(),
                },
            })
        });
        let logs = self.logs.iter().flat_map(|(path, events)| {
            events.values().map(move |e| Op {
                stamp: e.stamp,
                path: path.clone(),
                kind: OpKind::Event(e.value.clone()),
                extra: e.extra.clone(),
            })
        });
        regs.chain(sets)
            .chain(logs)
            .chain(self.unknown.values().cloned())
    }

    /// Number of registers, set elements (tombstones included) and events.
    pub fn len(&self) -> usize {
        self.registers.len()
            + self.sets.values().map(BTreeMap::len).sum::<usize>()
            + self.logs.values().map(BTreeMap::len).sum::<usize>()
            + self.unknown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::DeviceId;
    use serde_json::json;

    fn dev(n: u8) -> DeviceId {
        DeviceId::from_bytes([n; 16])
    }

    fn st(ms: u64, d: u8) -> Stamp {
        Stamp {
            hlc: Hlc::new(ms, 0),
            device: dev(d),
        }
    }

    #[test]
    fn registers_are_last_writer_wins_with_device_tiebreak() {
        let mut s = State::new();
        s.apply(&Op::set(st(2, 1), "r", json!("b")));
        s.apply(&Op::set(st(1, 9), "r", json!("a")));
        assert_eq!(s.value("r"), Some(&json!("b")));
        s.apply(&Op::set(st(2, 2), "r", json!("c")));
        assert_eq!(s.value("r"), Some(&json!("c")));
        s.apply(&Op::set(st(3, 0), "r", Value::Null));
        assert_eq!(s.value("r"), None);
    }

    #[test]
    fn a_removed_element_is_not_resurrected_by_a_late_add() {
        let mut s = State::new();
        s.apply(&Op::rm(st(5, 1), "set", "e1"));
        s.apply(&Op::add(st(1, 1), "set", "e1", json!(1)));
        s.apply(&Op::add(st(2, 2), "set", "e2", json!(2)));
        let present: Vec<_> = s.present("set").map(|(id, _)| id).collect();
        assert_eq!(present, ["e2"]);
    }

    #[test]
    fn duplicate_events_collapse() {
        let mut s = State::new();
        let e = Op::event(st(1, 1), "history", json!({"t": 1}));
        s.apply(&e);
        s.apply(&e);
        s.apply(&Op::event(st(1, 2), "history", json!({"t": 1})));
        assert_eq!(s.events("history").count(), 2);
    }

    #[test]
    fn ops_rebuild_the_same_state() {
        let mut s = State::new();
        s.apply(&Op::set(st(1, 1), "a/x", json!(1)));
        s.apply(&Op::add(st(2, 1), "a/s", "i", json!({"k": 1})));
        s.apply(&Op::rm(st(3, 1), "a/s", "j"));
        s.apply(&Op::event(st(4, 1), "log", json!(2)));
        let mut u = Op::set(st(5, 1), "z", json!(0));
        u.kind = OpKind::Unknown("move".into());
        s.apply(&u);
        let mut t = State::new();
        for op in s.ops() {
            t.apply(&op);
        }
        assert_eq!(s, t);
        assert_eq!(s.registers_under("a/").count(), 1);
        assert_eq!(s.sets_under("a/").collect::<Vec<_>>(), ["a/s"]);
    }
}
