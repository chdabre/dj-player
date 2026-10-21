//! One mounted library: the merged state, this device's open journal segment and its HLC.

use std::collections::{BTreeMap, HashMap};
use std::io;

use serde_json::{Value, json};

use crate::codec::{self, FORMAT_VERSION, Line, Op, OpKind, SegmentHeader, SnapshotHeader};
use crate::hash::{AudioHash, FileHash, track_id};
use crate::hlc::{Clock, Hlc, Stamp};
use crate::ids::{BlobId, DeviceId, LibraryId, TrackId, TrackRef};
use crate::model::{self, AnalysisRef, Cue, FileIdentity, HOT_CUES, HotCue, Track, WaveformRef};
use crate::state::State;
use crate::store::Store;

/// A segment is closed and the next one opened once it passes this size.
pub const SEGMENT_ROTATE_BYTES: usize = 64 * 1024;

/// The `format` field of `FORMAT`.
pub const FORMAT_NAME: &str = "djplayer-library";

/// How to mount.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// This device: the author of its edits and the owner of its journal folder.
    pub device: DeviceId,
    /// Never write to the store: edits stay in RAM, shown as unsaved (a write-protected or
    /// locked stick).
    pub read_only: bool,
    /// Written once to `devices/<device-id>.json` (name, model, …).
    pub device_info: Value,
    /// Wall-clock milliseconds since the Unix epoch; 0 if the device has no clock.
    pub now: fn() -> u64,
}

impl MountOptions {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            read_only: false,
            device_info: json!({}),
            now: system_ms,
        }
    }
}

/// Milliseconds since the Unix epoch from the system clock (0 if it is before 1970).
pub fn system_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug)]
pub enum MountError {
    Io(io::Error),
    /// `FORMAT` is unreadable or not ours.
    BadFormat(String),
    /// Written by a newer major version this build can't read.
    NewerFormat(u64),
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::Io(e) => write!(f, "library: {e}"),
            MountError::BadFormat(e) => write!(f, "library FORMAT file: {e}"),
            MountError::NewerFormat(v) => {
                write!(f, "library format version {v} is newer than this player")
            }
        }
    }
}

impl std::error::Error for MountError {}

impl From<io::Error> for MountError {
    fn from(e: io::Error) -> Self {
        MountError::Io(e)
    }
}

/// An edit that can't be made.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    NoSuchTrack(TrackId),
    BadHotCueSlot(usize),
    BadAnalyzerName(String),
    NoSuchCue(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::NoSuchTrack(t) => write!(f, "no track {t}"),
            WriteError::BadHotCueSlot(s) => write!(f, "hot cue slot {s} out of range"),
            WriteError::BadAnalyzerName(n) => write!(f, "bad analyzer name {n:?}"),
            WriteError::NoSuchCue(c) => write!(f, "no cue {c}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// An edit to one track of this library.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    SetRating(TrackId, Option<u8>),
    /// A new memory cue or loop.
    AddCue(TrackId, Cue),
    /// Change a cue: its old element is removed and the new value added under a new id.
    ReplaceCue(TrackId, String, Cue),
    RemoveCue(TrackId, String),
    /// Hot cue slot 0–15 (two banks of 8); `None` deletes it.
    SetHotCue(TrackId, usize, Option<HotCue>),
    SetAnalysis(TrackId, String, AnalysisRef),
    SetWaveform(TrackId, WaveformRef),
}

/// Whether edits reach the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persistence {
    /// Everything is written and flushed.
    Saved,
    /// Written, not yet flushed.
    Pending,
    /// Edits are kept in RAM only (read-only or failing store) and lost at power-off.
    Unsaved { edits: usize, reason: String },
}

/// What mounting read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountStats {
    pub snapshots: usize,
    pub segments: usize,
    /// Segments not read because a snapshot holds them.
    pub segments_covered: usize,
    pub ops: usize,
    /// Torn or corrupt lines skipped.
    pub skipped_lines: usize,
}

/// What [`Library::find_or_create`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub track: TrackId,
    /// No record existed: a new one was made.
    pub created: bool,
    /// The record existed, but not at this path: a new location (an exact duplicate or a
    /// moved file) was added.
    pub new_location: bool,
}

enum Mode {
    /// Journaling into `journal/<device>/<segment>.log`.
    Journal {
        segment: u32,
        bytes: usize,
        open: bool,
    },
    /// Not writing (read-only or failed store).
    Ram { reason: String },
}

#[derive(Default)]
struct Index {
    by_file: HashMap<FileHash, Vec<TrackId>>,
    by_audio: HashMap<AudioHash, Vec<TrackId>>,
    /// Root id → ids aliased into it (not including the root).
    members: HashMap<TrackId, Vec<TrackId>>,
}

pub struct Library {
    store: Box<dyn Store>,
    id: LibraryId,
    opts: MountOptions,
    state: State,
    clock: Clock,
    mode: Mode,
    /// Per writer device, the highest segment the state holds completely.
    vector: BTreeMap<DeviceId, u32>,
    /// Path of a segment with appended, unflushed lines.
    unflushed: Option<String>,
    /// Edits applied but not written.
    unsaved: usize,
    /// Blobs that could not be written: kept in RAM.
    ram_blobs: HashMap<BlobId, Vec<u8>>,
    index: Option<Index>,
    stats: MountStats,
    warnings: Vec<String>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("id", &self.id)
            .field("device", &self.opts.device)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

/// Parse `NNNNNNNN.log`.
fn segment_number(name: &str) -> Option<u32> {
    name.strip_suffix(".log")?.parse().ok()
}

impl Library {
    /// Mount the library in `store`: read `FORMAT` (made if missing and writable), load
    /// every snapshot, replay every journal segment no snapshot holds, and start this
    /// device's clock above the highest HLC found. This device's edits go to a new segment,
    /// opened on the first edit.
    pub fn mount(store: impl Store + 'static, opts: MountOptions) -> Result<Self, MountError> {
        let store: Box<dyn Store> = Box::new(store);
        let mut warnings = Vec::new();
        let mut read_only = opts.read_only;
        let id = match store.read("FORMAT") {
            Ok(bytes) => parse_format(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let id = LibraryId::random();
                if !read_only {
                    let format = json!({
                        "format": FORMAT_NAME,
                        "version": FORMAT_VERSION,
                        "library": id.to_string(),
                        "created": rfc3339((opts.now)()),
                    });
                    let text = format!("{format}\n");
                    if let Err(e) = store.create("FORMAT", text.as_bytes()) {
                        warnings.push(format!("can't create the library ({e}): RAM only"));
                        read_only = true;
                    }
                }
                id
            }
            Err(e) => return Err(e.into()),
        };

        let mut state = State::new();
        let mut vector: BTreeMap<DeviceId, u32> = BTreeMap::new();
        let mut stats = MountStats::default();
        for name in store.list("snapshots")? {
            if !name.ends_with(".snap") {
                continue;
            }
            let decoded = codec::decode(&store.read(&format!("snapshots/{name}"))?);
            let Some(Line::Snapshot(header)) = decoded.lines.first() else {
                warnings.push(format!("snapshot {name} has no header; ignored"));
                continue;
            };
            if u64::from(header.format) > u64::from(FORMAT_VERSION) {
                return Err(MountError::NewerFormat(header.format.into()));
            }
            for (d, s) in &header.vector {
                let v = vector.entry(*d).or_default();
                *v = (*v).max(*s);
            }
            state.observe(header.hlc);
            stats.snapshots += 1;
            stats.skipped_lines += decoded.skipped;
            for line in &decoded.lines[1..] {
                if let Line::Op(op) = line {
                    state.apply(op);
                    stats.ops += 1;
                }
            }
        }

        let covered = vector.clone();
        let mut own_last = 0;
        for dev_name in store.list("journal")? {
            let Ok(dev) = dev_name.parse::<DeviceId>() else {
                continue;
            };
            let mut segments: Vec<u32> = store
                .list(&format!("journal/{dev_name}"))?
                .iter()
                .filter_map(|n| segment_number(n))
                .collect();
            segments.sort_unstable();
            if dev == opts.device {
                own_last = segments.last().copied().unwrap_or(0);
            }
            for seg in segments {
                if covered.get(&dev).is_some_and(|&c| seg < c) {
                    stats.segments_covered += 1;
                    continue;
                }
                let bytes = store.read(&format!("journal/{dev_name}/{seg:08}.log"))?;
                let decoded = codec::decode(&bytes);
                stats.segments += 1;
                stats.skipped_lines += decoded.skipped;
                for line in &decoded.lines {
                    if let Line::Op(op) = line {
                        state.apply(op);
                        stats.ops += 1;
                    }
                }
                let v = vector.entry(dev).or_default();
                *v = (*v).max(seg);
            }
        }
        if stats.skipped_lines > 0 {
            warnings.push(format!(
                "{} torn or corrupt journal lines skipped",
                stats.skipped_lines
            ));
        }

        let mut clock = Clock::default();
        clock.observe(state.max_hlc());
        let mode = if read_only {
            Mode::Ram {
                reason: "the library is read-only".into(),
            }
        } else {
            Mode::Journal {
                segment: own_last + 1,
                bytes: 0,
                open: false,
            }
        };
        Ok(Library {
            store,
            id,
            opts,
            state,
            clock,
            mode,
            vector,
            unflushed: None,
            unsaved: 0,
            ram_blobs: HashMap::new(),
            index: None,
            stats,
            warnings,
        })
    }

    pub fn id(&self) -> LibraryId {
        self.id
    }

    pub fn device(&self) -> DeviceId {
        self.opts.device
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn stats(&self) -> &MountStats {
        &self.stats
    }

    /// The highest HLC handed out or seen.
    pub fn hlc(&self) -> Hlc {
        self.clock.last()
    }

    /// Messages worth showing (skipped lines, a stick that stopped taking writes); each
    /// once.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn persistence(&self) -> Persistence {
        match &self.mode {
            Mode::Ram { reason } => Persistence::Unsaved {
                edits: self.unsaved,
                reason: reason.clone(),
            },
            Mode::Journal { .. } if self.unflushed.is_some() => Persistence::Pending,
            Mode::Journal { .. } => Persistence::Saved,
        }
    }

    pub fn is_writable(&self) -> bool {
        matches!(self.mode, Mode::Journal { .. })
    }

    fn stamp(&mut self) -> Stamp {
        Stamp {
            hlc: self.clock.tick((self.opts.now)()),
            device: self.opts.device,
        }
    }

    fn element_id(stamp: Stamp) -> String {
        format!("{}@{}", stamp.hlc, stamp.device)
    }

    fn stop_saving(&mut self, why: String) {
        if !matches!(self.mode, Mode::Ram { .. }) {
            self.warnings.push(format!(
                "library stopped saving ({why}): edits are kept in RAM"
            ));
            self.mode = Mode::Ram { reason: why };
        }
    }

    /// Apply `ops` to the state and append them to this device's segment.
    fn commit(&mut self, ops: &[Op]) {
        for op in ops {
            self.state.apply(op);
            self.update_index(op);
        }
        let Mode::Journal {
            segment,
            bytes,
            open,
        } = &mut self.mode
        else {
            self.unsaved += ops.len();
            return;
        };
        let path = format!("journal/{}/{:08}.log", self.opts.device, segment);
        let mut text = String::new();
        if !*open {
            let header = SegmentHeader {
                segment: *segment,
                device: self.opts.device,
                format: FORMAT_VERSION,
                opened: self.clock.last(),
                extra: Default::default(),
            };
            text += &codec::frame(&header.to_json());
        }
        for op in ops {
            text += &codec::frame(&op.to_json());
        }
        let (seg, was_open) = (*segment, *open);
        match self.store.append(&path, text.as_bytes()) {
            Ok(()) => {
                *bytes += text.len();
                *open = true;
                self.vector.insert(self.opts.device, seg);
                self.unflushed = Some(path);
                if *bytes >= SEGMENT_ROTATE_BYTES {
                    *segment += 1;
                    *bytes = 0;
                    *open = false;
                    // The full segment gets its flush now; the next starts clean.
                    self.flush();
                }
                if !was_open {
                    self.write_device_info();
                }
            }
            Err(e) => {
                self.unsaved += ops.len();
                self.stop_saving(e.to_string());
            }
        }
    }

    fn write_device_info(&mut self) {
        let mut info = self.opts.device_info.clone();
        if let Value::Object(m) = &mut info {
            m.insert("device".into(), self.opts.device.to_string().into());
        }
        let path = format!("devices/{}.json", self.opts.device);
        match self.store.create(&path, format!("{info}\n").as_bytes()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => self.warnings.push(format!("{path}: {e}")),
        }
    }

    /// Make every edit so far durable. A failure switches the library to RAM.
    pub fn flush(&mut self) {
        if let Some(path) = self.unflushed.take()
            && let Err(e) = self.store.flush(&path)
        {
            self.stop_saving(e.to_string());
        }
    }

    /// Apply operations from a linked player: stamped with its HLC and device id, journaled
    /// here as they are.
    pub fn apply_remote(&mut self, ops: &[Op]) {
        for op in ops {
            self.clock.observe(op.stamp.hlc);
        }
        self.commit(ops);
    }

    /// Make an edit. Edits to a track that was merged into another go to the survivor.
    /// Returns the new element id for [`Edit::AddCue`] and [`Edit::ReplaceCue`].
    pub fn apply(&mut self, edit: Edit) -> Result<Option<String>, WriteError> {
        let track = match &edit {
            Edit::SetRating(t, _)
            | Edit::AddCue(t, _)
            | Edit::ReplaceCue(t, _, _)
            | Edit::RemoveCue(t, _)
            | Edit::SetHotCue(t, _, _)
            | Edit::SetAnalysis(t, _, _)
            | Edit::SetWaveform(t, _) => *t,
        };
        if !self.exists(track) {
            return Err(WriteError::NoSuchTrack(track));
        }
        let t = self.resolve(track);
        let mut new_id = None;
        let ops = match edit {
            Edit::SetRating(_, r) => {
                let s = self.stamp();
                vec![Op::set(
                    s,
                    model::rating_path(t),
                    r.map_or(Value::Null, Value::from),
                )]
            }
            Edit::AddCue(_, cue) => {
                let s = self.stamp();
                let id = Self::element_id(s);
                new_id = Some(id.clone());
                vec![Op::add(s, model::cue_path(t), id, cue.to_json())]
            }
            Edit::ReplaceCue(_, old, cue) => {
                let path = self.cue_set_holding(t, &old)?;
                let s1 = self.stamp();
                let s2 = self.stamp();
                let id = Self::element_id(s2);
                new_id = Some(id.clone());
                vec![
                    Op::rm(s1, path, old),
                    Op::add(s2, model::cue_path(t), id, cue.to_json()),
                ]
            }
            Edit::RemoveCue(_, old) => {
                let path = self.cue_set_holding(t, &old)?;
                let s = self.stamp();
                vec![Op::rm(s, path, old)]
            }
            Edit::SetHotCue(_, slot, cue) => {
                if slot >= HOT_CUES {
                    return Err(WriteError::BadHotCueSlot(slot));
                }
                let s = self.stamp();
                let v = cue.map_or(Value::Null, |c| c.to_json());
                vec![Op::set(s, model::hot_cue_path(t, slot), v)]
            }
            Edit::SetAnalysis(_, name, a) => {
                if !model::is_component(&name) {
                    return Err(WriteError::BadAnalyzerName(name));
                }
                let s = self.stamp();
                vec![Op::set(s, model::analysis_path(t, &name), a.to_json())]
            }
            Edit::SetWaveform(_, w) => {
                let s = self.stamp();
                vec![Op::set(s, model::waveform_path(t), w.to_json())]
            }
        };
        self.commit(&ops);
        Ok(new_id)
    }

    /// The cue set (of the track or one merged into it) holding element `id`.
    fn cue_set_holding(&mut self, root: TrackId, id: &str) -> Result<String, WriteError> {
        self.members(root)
            .into_iter()
            .map(model::cue_path)
            .find(|p| self.state.present(p).any(|(e, _)| e == id))
            .ok_or_else(|| WriteError::NoSuchCue(id.to_owned()))
    }

    // Identity

    fn index(&mut self) -> &Index {
        if self.index.is_none() {
            self.index = Some(self.build_index());
        }
        self.index.as_ref().expect("just built")
    }

    /// Keep the index current after `op`. Index entries are candidates: a removed location
    /// may leave one behind, so lookups check the state.
    fn update_index(&mut self, op: &Op) {
        let p = op.path.as_str();
        if p.starts_with("alias/") {
            // Rare: rebuild on the next lookup.
            self.index = None;
            return;
        }
        let Some(ix) = self.index.as_mut() else {
            return;
        };
        if let (OpKind::Add { value, .. }, true) = (&op.kind, p.ends_with("/file"))
            && let Some(t) = model::track_of_path(p)
            && let Some(f) = FileIdentity::from_json(value)
        {
            push_unique(ix.by_file.entry(f.file_hash).or_default(), t);
            if let Some(a) = f.audio_hash {
                push_unique(ix.by_audio.entry(a).or_default(), t);
            }
        }
    }

    /// Present locations of `track` itself (not of tracks merged into it).
    fn own_locations(&self, track: TrackId) -> impl Iterator<Item = FileIdentity> + '_ {
        self.state
            .present(&model::file_path(track))
            .filter_map(|(_, e)| FileIdentity::from_json(&e.value))
    }

    fn build_index(&self) -> Index {
        let mut ix = Index::default();
        for path in self.state.sets_under("track/") {
            if !path.ends_with("/file") {
                continue;
            }
            let Some(t) = model::track_of_path(path) else {
                continue;
            };
            for (_, e) in self.state.present(path) {
                if let Some(f) = FileIdentity::from_json(&e.value) {
                    ix.by_file.entry(f.file_hash).or_default().push(t);
                    if let Some(a) = f.audio_hash {
                        ix.by_audio.entry(a).or_default().push(t);
                    }
                }
            }
        }
        for (path, _) in self.state.registers_under("alias/") {
            if let Some(t) = model::track_of_path(path) {
                let root = self.resolve(t);
                if root != t {
                    ix.members.entry(root).or_default().push(t);
                }
            }
        }
        for v in ix.by_file.values_mut().chain(ix.by_audio.values_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        ix
    }

    /// The track `id` was merged into, following aliases (itself if none). In an alias
    /// cycle (two devices merging the same pair opposite ways), the lowest id in the cycle.
    pub fn resolve(&self, id: TrackId) -> TrackId {
        let mut seen = vec![id];
        let mut cur = id;
        while let Some(next) = self
            .state
            .value(&model::alias_path(cur))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<TrackId>().ok())
        {
            if let Some(pos) = seen.iter().position(|&s| s == next) {
                return *seen[pos..].iter().min().expect("non-empty cycle");
            }
            seen.push(next);
            cur = next;
        }
        cur
    }

    /// `root` and every id merged into it.
    fn members(&mut self, root: TrackId) -> Vec<TrackId> {
        let mut m = vec![root];
        if let Some(more) = self.index().members.get(&root) {
            m.extend_from_slice(more);
        }
        m
    }

    fn exists(&self, id: TrackId) -> bool {
        self.state.elements(&model::file_path(id)).next().is_some()
            || self
                .state
                .registers_under(&model::track_prefix(id))
                .next()
                .is_some()
            || self.state.register(&model::alias_path(id)).is_some()
    }

    /// When a track was first seen: the earliest HLC among its file elements' ids (which
    /// stay as tombstones when a location is replaced).
    fn first_seen(&self, id: TrackId) -> Hlc {
        self.state
            .elements(&model::file_path(id))
            .filter_map(|(eid, _)| eid.split('@').next()?.parse::<Hlc>().ok())
            .min()
            .unwrap_or(Hlc(u64::MAX))
    }

    /// Find the track a file belongs to by its file hash, or make its record. A known file
    /// at a new path gets that path as another location (an exact duplicate, or a moved
    /// file); a new file gets the id derived from its hash.
    pub fn find_or_create(&mut self, file: FileIdentity) -> Found {
        let hash = file.file_hash;
        let known: Vec<TrackId> = self.index().by_file.get(&hash).cloned().unwrap_or_default();
        let known: Vec<TrackId> = known
            .into_iter()
            .filter(|&t| self.own_locations(t).any(|l| l.file_hash == hash))
            .collect();
        let mut roots: Vec<TrackId> = known.iter().map(|&t| self.resolve(t)).collect();
        roots.sort_by_key(|&t| (self.first_seen(t), t));
        roots.dedup();
        if let Some(&root) = roots.first() {
            let have = self.track(root).is_some_and(|t| {
                t.locations
                    .iter()
                    .any(|(_, l)| l.file_hash == hash && l.path == file.path)
            });
            if !have {
                self.add_location(root, file);
            }
            return Found {
                track: root,
                created: false,
                new_location: !have,
            };
        }
        let id = track_id(&hash);
        self.add_location(id, file);
        Found {
            track: self.resolve(id),
            created: true,
            new_location: true,
        }
    }

    fn add_location(&mut self, track: TrackId, file: FileIdentity) {
        let s = self.stamp();
        let op = Op::add(
            s,
            model::file_path(track),
            Self::element_id(s),
            file.to_json(),
        );
        self.commit(&[op]);
    }

    /// Record the audio hash of the file with `file_hash` on `track` (replacing a stale
    /// one), then merge every other track with the same audio hash: the one seen first
    /// survives and the others alias to it. Returns the surviving track.
    pub fn set_audio_hash(
        &mut self,
        track: TrackId,
        file_hash: FileHash,
        audio: AudioHash,
    ) -> TrackId {
        let root = self.resolve(track);
        let mut ops = Vec::new();
        for member in self.members(root) {
            let path = model::file_path(member);
            let stale: Vec<(String, FileIdentity)> = self
                .state
                .present(&path)
                .filter_map(|(id, e)| Some((id.to_owned(), FileIdentity::from_json(&e.value)?)))
                .filter(|(_, f)| f.file_hash == file_hash && f.audio_hash != Some(audio))
                .collect();
            for (old, mut f) in stale {
                f.audio_hash = Some(audio);
                let s1 = self.stamp();
                let s2 = self.stamp();
                ops.push(Op::rm(s1, path.clone(), old));
                ops.push(Op::add(s2, path.clone(), Self::element_id(s2), f.to_json()));
            }
        }
        if !ops.is_empty() {
            self.commit(&ops);
        }
        let same: Vec<TrackId> = self
            .index()
            .by_audio
            .get(&audio)
            .cloned()
            .unwrap_or_default();
        let same: Vec<TrackId> = same
            .into_iter()
            .filter(|&t| self.own_locations(t).any(|l| l.audio_hash == Some(audio)))
            .collect();
        let mut roots: Vec<TrackId> = same.iter().map(|&t| self.resolve(t)).collect();
        roots.push(root);
        roots.sort_by_key(|&t| (self.first_seen(t), t));
        roots.dedup();
        let survivor = roots[0];
        let aliases: Vec<Op> = roots[1..]
            .iter()
            .map(|&t| {
                let s = self.stamp();
                Op::set(s, model::alias_path(t), survivor.to_string().into())
            })
            .collect();
        if !aliases.is_empty() {
            self.commit(&aliases);
        }
        survivor
    }

    /// The merged record of a track (following aliases), if the library has one. Fields
    /// merge across the merged tracks by their own rules: the latest register wins, sets
    /// are united.
    pub fn track(&mut self, id: TrackId) -> Option<Track> {
        if !self.exists(id) {
            return None;
        }
        let root = self.resolve(id);
        let members = self.members(root);
        let s = &self.state;
        let latest = |path: &dyn Fn(TrackId) -> String| {
            members
                .iter()
                .filter_map(|&m| s.register(&path(m)))
                .max_by_key(|e| e.stamp)
                .map(|e| &e.value)
                .filter(|v| !v.is_null())
        };
        let mut locations = Vec::new();
        let mut cues = Vec::new();
        let mut analysis = BTreeMap::new();
        let mut analysis_stamp = BTreeMap::new();
        for &m in &members {
            for (eid, e) in s.present(&model::file_path(m)) {
                if let Some(f) = FileIdentity::from_json(&e.value) {
                    locations.push((eid.to_owned(), f));
                }
            }
            for (eid, e) in s.present(&model::cue_path(m)) {
                if let Some(c) = Cue::from_json(&e.value) {
                    cues.push((eid.to_owned(), c));
                }
            }
            let prefix = format!("track/{m}/analysis/");
            for (path, e) in s.registers_under(&prefix) {
                let name = &path[prefix.len()..];
                let newer = analysis_stamp.get(name).is_none_or(|old| e.stamp > *old);
                if newer {
                    analysis_stamp.insert(name.to_owned(), e.stamp);
                    match AnalysisRef::from_json(&e.value) {
                        Some(a) => analysis.insert(name.to_owned(), a),
                        None => analysis.remove(name),
                    };
                }
            }
        }
        cues.sort_by(|a, b| a.1.at.total_cmp(&b.1.at).then_with(|| a.0.cmp(&b.0)));
        let hot_cues = std::array::from_fn(|slot| {
            latest(&|m| model::hot_cue_path(m, slot)).and_then(HotCue::from_json)
        });
        let rating = latest(&model::rating_path)
            .and_then(Value::as_u64)
            .and_then(|r| u8::try_from(r).ok());
        let waveform = latest(&model::waveform_path).and_then(WaveformRef::from_json);
        Some(Track {
            id: root,
            locations,
            rating,
            cues,
            hot_cues,
            analysis,
            waveform,
            merged: members[1..].to_vec(),
        })
    }

    /// Every track id that is not merged into another.
    pub fn tracks(&self) -> Vec<TrackId> {
        let mut ids: Vec<TrackId> = self
            .state
            .sets_under("track/")
            .filter(|p| p.ends_with("/file"))
            .filter_map(model::track_of_path)
            .filter(|&t| self.resolve(t) == t)
            .collect();
        ids.dedup();
        ids
    }

    pub fn track_ref(&self, track: TrackId) -> TrackRef {
        TrackRef {
            library: self.id,
            track,
        }
    }

    // Blobs

    /// Store a blob under its BLAKE3 name, written once (tmp + rename). On a store that
    /// takes no writes it stays in RAM.
    pub fn put_blob(&mut self, bytes: &[u8]) -> BlobId {
        let id = BlobId::of(bytes);
        if self.ram_blobs.contains_key(&id) {
            return id;
        }
        if self.is_writable() {
            match self.store.create(&id.path(), bytes) {
                Ok(()) => return id,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return id,
                Err(e) => self.stop_saving(e.to_string()),
            }
        }
        self.ram_blobs.insert(id, bytes.to_vec());
        id
    }

    /// A blob's bytes, checked against its name.
    pub fn read_blob(&self, id: BlobId) -> io::Result<Vec<u8>> {
        if let Some(b) = self.ram_blobs.get(&id) {
            return Ok(b.clone());
        }
        let bytes = self.store.read(&id.path())?;
        if BlobId::of(&bytes) != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("blob {id} is corrupt"),
            ));
        }
        Ok(bytes)
    }

    // Snapshots

    /// The current state as a snapshot file: a header with the version vector, then one
    /// line per register, element, tombstone and event, with their original stamps.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let header = SnapshotHeader {
            device: self.opts.device,
            format: FORMAT_VERSION,
            hlc: self.state.max_hlc(),
            vector: self.vector.clone(),
            extra: Default::default(),
        };
        let mut out = codec::frame(&header.to_json()).into_bytes();
        for op in self.state.ops() {
            out.extend_from_slice(codec::frame(&op.to_json()).as_bytes());
        }
        out
    }

    /// Write a snapshot to `snapshots/<hlc>-<device>.snap`. The segments it covers are left
    /// in place (compaction comes later); mounting skips them.
    pub fn snapshot(&mut self) -> io::Result<String> {
        self.flush();
        let path = format!(
            "snapshots/{:016x}-{}.snap",
            self.state.max_hlc().0,
            self.opts.device
        );
        self.store.create(&path, &self.snapshot_bytes())?;
        Ok(path)
    }
}

fn push_unique(v: &mut Vec<TrackId>, t: TrackId) {
    if !v.contains(&t) {
        v.push(t);
    }
}

fn parse_format(bytes: &[u8]) -> Result<LibraryId, MountError> {
    let bad = |e: &str| MountError::BadFormat(e.to_owned());
    let v: Value = serde_json::from_slice(bytes).map_err(|e| bad(&e.to_string()))?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT_NAME) {
        return Err(bad("not a djplayer library"));
    }
    let version = v
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("no version"))?;
    if version > u64::from(FORMAT_VERSION) {
        return Err(MountError::NewerFormat(version));
    }
    v.get("library")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("no library id"))
}

/// `ms` since the Unix epoch as RFC 3339 UTC, e.g. `2026-10-21T22:41:07Z`.
pub fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant), for the proleptic Gregorian calendar.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Every library a player sees (its own slots, later linked players' sticks and the cloud),
/// by library id. Data stays per library: nothing merges across them.
#[derive(Debug, Default)]
pub struct Libraries {
    libs: BTreeMap<LibraryId, Library>,
}

impl Libraries {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a mounted library; returns its id. A library with the same id (a second copy)
    /// replaces the first.
    pub fn insert(&mut self, lib: Library) -> LibraryId {
        let id = lib.id();
        self.libs.insert(id, lib);
        id
    }

    pub fn remove(&mut self, id: LibraryId) -> Option<Library> {
        self.libs.remove(&id)
    }

    pub fn get(&self, id: LibraryId) -> Option<&Library> {
        self.libs.get(&id)
    }

    pub fn get_mut(&mut self, id: LibraryId) -> Option<&mut Library> {
        self.libs.get_mut(&id)
    }

    pub fn track(&mut self, r: TrackRef) -> Option<Track> {
        self.libs.get_mut(&r.library)?.track(r.track)
    }

    pub fn ids(&self) -> impl Iterator<Item = LibraryId> + '_ {
        self.libs.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_civil_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_792_622_467_000), "2026-10-21T22:41:07Z");
    }
}
