# Library format (design, draft)

Status 2026-10-21: design settled with the owner (see "Decisions" below, which win over anything else in this doc), no code yet. It makes concrete the "Own library on the stick" and "Cloud library" sections of the engine research (`docs/research/engine.md`). The decisions marked **(to confirm)** are the owner's.

## Decisions (owner, 2026-10-21)

These settle the open questions and extend the design; where this section and the rest of the doc disagree, this section wins.

**Libraries and writing**
- A player sees several libraries at once: its own slots, linked players' sticks, later the cloud. Tracks are addressed as (library, track).
- Data stays per library: no automatic merge across sticks, and no cross-encode cue copy in version 1.
- One writer per stick: the player it's plugged into. Linked players forward operations (stamped with their own HLC and device id) and blobs to the holder.
- Guest sticks are written by default. A per-stick lock flag (in the stick's settings, never copied by a settings import), or a write-protected stick, keeps edits in RAM, shown as unsaved.
- With no writable library mounted, edits live in RAM only, shown as unsaved. There is no home library in version 1.

**Rekordbox**
- Rekordbox data is read-only and overlaid at mount. Rekordbox's tree is always browsable.
- Tracks are imported explicitly: importing creates our record plus a mapping to the Rekordbox id. Their cues, grid and rating stay overlaid live until edited.
- The first cue edit switches a track to our cues; "revert to Rekordbox cues" switches back.
- A track that isn't imported plays fine. Its first edit or first counted play auto-imports it.
- Folder-mode files (no Rekordbox) get their record lazily, on first load, edit or analysis. There is no import step and no scan of the whole stick.

**Identity**
- Track id derived from the file's content hash at first sight (Dropbox `content_hash` algorithm).
- An exact duplicate is one track with several locations, marked as a duplicate in the UI.
- Same decoded audio with different tags is the same track (an alias to the older id). A different encode is a separate track.
- The audio hash is versioned (`{v, hash}`) and recomputed on load when stale.
- Missing files keep their record and cues, shown as missing; only a DJ deletes records. "Remove from library" deletes our data, never the file. A later load gets a fresh, empty record.

**Data**
- Positions in f64 seconds on the gapless-trimmed timeline; Rekordbox values are converted with the start trim.
- 16 hot cues per track (two banks of 8).
- Analysis: only the latest result per analyzer; older blobs are collected.
- History: full play events.
  - A play counts after 60 s of cumulative playback per load (Pioneer's rule, XDJ-1000MK2 manual p. 31). Loading or cueing alone never counts. A track played again right after itself on the same deck adds no second entry.
  - Each mount starts a history session (event with date and player), shown as "History › date (player)".
  - Marking a track played or unplayed in a playlist adds or removes an entry flagged `manual`.

**Settings**
- The player's current settings persist in a small config file on its boot storage (next to the device id), not in a library.
- Saving settings to a stick is an explicit action. When the stick in the player's own slot has different saved settings, the player asks once per mount (Yes / No / show differences).
- Loading from another stick or from a linked player is a manual menu action.
- A load replaces DJ preferences only (quantize, tempo range, waveform mode and theme, auto cue, hot-cue banks). Hardware and device settings are never imported.

**Format and defaults**
- `DJPLAYER/` folder; CRC-32C-framed JSON-lines journals and snapshots; BLAKE3 blobs; HLC-stamped `set` / `add` / `rm` / `event` operations.
- Tuning: flush within 2 s; segment per mount and every 64 KB; snapshot past about 50 segments or 5 MB; tombstones kept 90 days; cloud segments sealed 30 s after their last write.

**First build (scope)**
- The `library` crate on a folder store: the record format, HLC and merge, with property tests.
- Identity: content hash, versioned audio hash, aliases.
- Lazy records for loaded files; cues, 16 hot cues, rating, analysis and waveform blobs.
- Wired into `engined`, so cues and waveforms persist across loads. Snapshots as soon as the replay test needs them.
- Later: Rekordbox import (needs the Rekordbox reader and a real stick), playlists, tags, history, settings, compaction, the link protocol and the cloud.

## Short answer

- **One folder at the root of the stick** (or of a cloud folder) holds everything we write. Rekordbox files are never touched.
- **Four kinds of files:**
  - one `FORMAT` file;
  - snapshots;
  - one append-only journal folder per device;
  - content-addressed blobs.

  A device only ever creates files in its own journal folder, so no two writers ever touch the same file.
- **Journals are text:** one record per line, a CRC-32C and then a JSON object **(to confirm)**. A torn last line (a yanked stick) or a corrupt line is detected and skipped.
- **The state is a merge of operations.** Every edit is one operation stamped with a hybrid logical clock (HLC). Replaying all journals in any order gives the same state:
  - plain fields are last-writer-wins registers;
  - cues, tags and playlist entries are sets with per-element ids and tombstones;
  - play history is an append-only event log.
- **Tracks have a stable id derived from their file hash at first sight.** Two devices that discover the same file give it the same id. A file whose tags were edited elsewhere is re-matched by its audio hash and merged.
- **Big data lives in blobs.** Waveforms, analysis results and later stems are blobs named by their BLAKE3 hash, written once and never changed. Records only point at them.
- **Positions are seconds** (f64) on the engine's timeline, the gapless-trimmed one. Every record that imports Rekordbox data converts with the track's recorded start trim.
- **A player can hold several libraries at once:** its own USB and SD slots, the libraries in linked players, and a cloud library. Every track is addressed as (library, track). Each library has exactly one writer at a time: the player it is plugged into. Edits for a track in another player's library travel over the link to that player, which journals them.

## Layout

```
<stick or cloud root>/
  DJPLAYER/                       name to confirm
    FORMAT                        format id, version, library id (UUID), created
    devices/<device-id>.json      a device's name and model, written once by that device
    journal/<device-id>/
      00000001.log                append-only segments, one per mount (+ rotation)
      00000002.log
    snapshots/
      <hlc>-<device-id>.snap      full state + version vector; newest wins, older ones deleted
    blobs/<aa>/<hash>             content-addressed, BLAKE3 hex; <aa> = first two hex digits
    tmp/                          partial writes, renamed into place when complete
```

- **Device id:** a random UUID generated on first boot. It is kept on the device's boot storage, never on the stick, so a stick moved between players never mixes their journals.
- **FORMAT:** a small JSON file: `{"format":"djplayer-library","version":1,"library":"<uuid>","created":"2026-10-21T…"}`. Readers refuse a higher major version and ignore unknown fields.
- **Names stay within exFAT and FAT32 rules:** ASCII, no colons, short.

## Records

A journal segment is a sequence of lines. Each line is `<crc32c, 8 hex> <json>\n`, where the CRC covers the JSON bytes. A line with a bad CRC or no newline is skipped, with a warning, and replay continues.

The first line of a segment is a header:

```
5e1a03c2 {"segment":7,"device":"3f2a…","format":1,"opened":{"hlc":"…"}}
```

Every other line is one operation:

```
a91c44e0 {"hlc":"0192f3c4a1b2-0003","op":"set","path":"track/9c4e1f0a2b3d4e5f/rating","value":4}
17bd0e9f {"hlc":"0192f3c4a1b3-0000","op":"add","path":"track/9c4e1f0a2b3d4e5f/cue","id":"0192f3c4a1b3-0000@3f2a","value":{"kind":"memory","at":61.284,"color":"rose","comment":""}}
c2f0aa13 {"hlc":"0192f3c4a1c0-0000","op":"rm","path":"track/9c4e1f0a2b3d4e5f/cue","id":"0192f3c4a1b3-0000@3f2a"}
0e77b1d5 {"hlc":"0192f3c4a2f0-0000","op":"event","path":"history","value":{"track":"9c4e1f0a2b3d4e5f","at":"2026-10-21T22:41:07Z","deck":1,"played_s":312.4}}
```

- **`hlc`:** milliseconds since the Unix epoch (48 bits, hex) and a counter (16 bits). Ties are broken by the device id. Each device's HLC never goes backwards: on mount it starts above the highest HLC already in the library, so a player with no clock source (no RTC battery, no network) still orders its edits after everything it has seen.
- **Four operations:**
  - `set`: a register; `value: null` clears it.
  - `add` / `rm`: an element of a set, by its `id`.
  - `event`: appended to a log.

  That is the whole grammar. A new feature adds paths, not operations.
- **Paths** name an entity and a field. Unknown paths are kept and passed through (a newer device's data survives an older device's snapshot) but not interpreted.

## Who sees what, when

Journals are split by device for writing only. Every device reads all journal folders and merges them, so nothing one device writes is hidden from another.

- **USB stick moved between players:** on mount, a player replays every device's journals, so last night's edits from another player are there.
- **Cloud folder:** a running device picks up other devices' sealed segments through change notifications. They arrive within about 30 s (sealing) plus up to about 90 s (Dropbox long-poll lag); a device that was offline catches up on reconnect.
- **The same field edited on two devices:** the later edit by HLC wins everywhere. **Sets** (memory cues, tags, playlist entries) keep both devices' additions.
- **While a stick is mounted,** its player doesn't watch for changes: it is the stick's only writer. Linked players see its edits live over the link (next section), not through the files.

## Multiple libraries and linked players

Linked players share their sticks, as Pro DJ Link does: player B browses and loads tracks from the USB in player A. So a player works with a set of libraries, not one.

- **Library id:** the UUID in `FORMAT`. A player's library set holds each mounted library with its id, where it is (a local slot, a linked player, the cloud) and whether it can be written. The browse tree shows them all.
- **Track address:** `(library id, track id)`. Track ids come from the file hash, so the same file on two sticks has the same track id in both. Its data stays separate per library, and the UI can offer "copy cues from the other stick". Nothing merges across libraries on its own.
- **One writer per stick:** the player a stick is plugged into is its only writer. Player B never writes A's stick directly. It sends its operations over the link, stamped with B's HLC and device id, and A appends them to its own journal segment on that stick. Each operation carries its origin device, and merging orders by HLC and origin, not by the folder the operation sits in. So the "one folder per device" rule stays true: a folder is per writer, and operations are per author.
- **Blobs from linked players:** an analysis or waveform computed on B for a track on A's stick is sent to A as a blob, and A writes it. Until A confirms, B keeps it in RAM.
- **Edits that can't be delivered:** if A or its stick goes away while B still has the track loaded, the track keeps playing from B's RAM. B queues the edits for that library id and delivers them when that library shows up on the link again. Undelivered edits are shown ("3 unsaved edits for USB 'Friday'") and are lost at power-off.
- **History:** a play is recorded in the library that holds the track, with the playing device as the author. A session log across all libraries (what played tonight, from which sticks) is a later, separate record in the player's home library, if it has one.
- **Two copies of the same library** on the link (the same library id, e.g. a stick prepared from the cloud and the cloud itself, or two copies of one stick): they are replicas. Each holder writes only its own copy, and the link shares edits live so both players show the merged state. The copies themselves converge the next time they sync (cloud upload, or a copy-back to the original).
- **Depends on** the unit-to-unit link protocol (`engine-sync`), not designed yet. The library side needs four messages from it: browse, load (stream the audio file), apply-operations (with acknowledgement) and put-blob.

## Data model

| Path | Kind | Value | Notes |
| --- | --- | --- | --- |
| `track/<id>/file` | set of identities | `{path, size, file_hash, audio_hash?}` | Where the file is and what it is; see Identity |
| `track/<id>/meta/<field>` | register | text | Only our edits (title, artist, …). Rekordbox and tags fill the rest at mount |
| `track/<id>/rating`, `color`, `comment` | register | small | Overlay: ours wins when set |
| `track/<id>/cue` | set | `{kind: memory \| loop, at, end?, color, comment}` | Memory cues and loops |
| `track/<id>/hotcue/<0..15>` | register | `{at, end?, color, comment}` or null | 16 hot cues: two banks of 8 **(to confirm)** |
| `track/<id>/cues_from` | register | `ours` \| `rekordbox` | Overlay switch per track: set to `ours` on the first cue edit; "revert to Rekordbox cues" sets it back |
| `track/<id>/grid` | register | `{blob}` or `{first, bpm, bar_phase}` | A hand-edited grid; absent = use analysis |
| `track/<id>/analysis/<analyzer>` | register | `{version, params, blob, created}` | One per analyzer (gridfit, tempocnn, hpcpkey); the selection policy picks among them and Rekordbox's |
| `track/<id>/waveform` | register | `{blob, overview_blob}` | DJWF blobs; their header carries params and timeline |
| `track/<id>/tag` | set | tag id | My Tag-style tags |
| `tag/<id>/name`, `color` | register | text | |
| `playlist/<id>/name`, `parent`, `order` | register | text | Folders are playlists with children |
| `playlist/<id>/entry` | set | `{track, order}` | `order` is a fractional index, so concurrent inserts never collide |
| `history` | log | `{track, at, deck, played_s}` | Play counts are derived from it |
| `settings/<key>` | register | any | Per-stick settings (our MY SETTINGS) |
| `alias/<id>` | register | `<id>` | A merged duplicate track points at the survivor |

Rekordbox's data is never copied in. It is read at mount and overlaid: for each field, ours if we have a value, else Rekordbox's (cues switch per track via `cues_from`). So a re-export from Rekordbox never conflicts with anything.

## Merge rules

| Kind | Rule |
| --- | --- |
| Register | Highest HLC wins (last writer wins) |
| Set | An element is present if its `add` has no `rm` with the same id (add-wins for a re-add, which gets a new id). Editing a cue is an `rm` + `add` |
| Log | Union of events, ordered by HLC; duplicates (same HLC + device) dropped |
| Track identity | Two track ids whose identities share a `file_hash` or `audio_hash` are merged: the lower id survives, the other gets an `alias`. Each field merges by its own rule |

The rules are commutative and idempotent. Replaying segments in any order, or twice, gives the same state; a property test checks this on random histories.

## Identity

- **Track id:** the first 16 bytes of BLAKE3 over `"track" ‖ file_hash`, as hex, taken when the file is first seen. Two devices that find the same file independently create the same id.
- **`file_hash`:** Dropbox's `content_hash`, computed locally for USB files too: SHA-256 over the concatenated SHA-256 digests of 4 MiB blocks. The same file on a stick and in Dropbox then matches without a download.
- **`audio_hash`:** BLAKE3 over the decoded audio as 16-bit PCM at the file's own rate, computed once during analysis. A tag edit changes the file bytes but not this, so it re-matches an edited file, which then gets a new `file` identity on the same track.
- **Paths:** the relative path from the stick root, with `/` separators, NFC-normalised. A path is a hint for finding the file, never its identity.

## Blobs

- **Naming:** `blobs/<aa>/<64 hex>`, the BLAKE3 hash of the bytes. A blob's own header says what it is (waveforms are DJWF); analysis results are a small versioned JSON or binary per analyzer.
- **Writing:** write into `tmp/`, flush, then rename into place. A reader never sees a half-written blob, and a blob that already exists is never written again.
- **Garbage collection:** when a snapshot is written, any blob that no live record references (after the tombstone grace period) is deleted. Stems will dominate the space, so the UI gets a size display and "remove stems".

## Snapshots and compaction

- **What a snapshot holds:** a header line with the version vector (per device, the highest segment and line it includes), then the current state as `set` / `add` / `event` lines with their original HLCs. It also keeps tombstones for 90 days, so a late journal can't resurrect a deleted cue.
- **When:** any device writes one when replay passes about 50 segments or 5 MB of journal, under `tmp/`, then renames it into `snapshots/`.
- **Deleting:** segments at or below a device's entry in the newest snapshot's vector are deleted. A device that was offline appends higher segment numbers, so nothing it wrote is lost.
- **Concurrent snapshots:** two snapshots written at once are both loaded and merged; the one whose vector is dominated is deleted.

## Durability on a USB stick

- **Segments:** one per mount, plus rotation every 64 KB.
- **Flushing:** small records (cue, rating, history) are flushed within 2 s of the edit; each analysis result once its blob is written.
- **Yanked stick:** loses at most the unflushed seconds. The torn line fails its CRC and is skipped.
- **Read-only or full stick:** the library runs from RAM for the session and says so on screen.
- **Mount:** load the newest snapshot, replay newer segments, and build the in-RAM index. Rough size for 10,000 tracks: about 15 MB of snapshot, read and parsed in well under a second on a Pi 5 (to measure). zstd compression of snapshots is a later option, not version 1.

## Cloud

The same files work unchanged on a cloud folder. The difference is that cloud APIs (Dropbox) can't append:

- **Sealed segments only:** a segment is sealed (closed, never written again) on unmount, on rotation, or 30 s after its last write. Only sealed segments are uploaded.
- **Hold-back:** at most 30 s of edits stay unshared while a device is online.
- **"Prepare for gig":** copies the folder to a stick byte for byte; the stick's new segments sync back later.

## Crate

A new crate, `library`, with no I/O of its own beyond a storage trait, so it tests offline like `engine-core`.

```rust
pub trait Store {                       // a stick folder, RAM, later Dropbox
    fn list(&self, dir: &str) -> io::Result<Vec<String>>;
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()>;     // own segments only
    fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()>;     // write-once (tmp + rename)
    fn remove(&self, path: &str) -> io::Result<()>;
    fn flush(&self, path: &str) -> io::Result<()>;
}

pub struct Library { /* one mounted library: merged state, the open segment, HLC */ }
pub struct Libraries { /* every library this player sees, by library id */ }
impl Library {
    pub fn mount(store: impl Store, device: DeviceId) -> Result<Self, MountError>;
    pub fn track(&self, id: TrackId) -> Option<TrackView<'_>>;           // the merged fields
    pub fn apply(&mut self, edit: Edit) -> Result<(), WriteError>;     // stamps, journals, merges
    pub fn apply_remote(&mut self, ops: &[Op]) -> Result<(), WriteError>; // from a linked player
    pub fn put_blob(&mut self, bytes: &[u8]) -> Result<BlobId, WriteError>;
    pub fn flush(&mut self) -> Result<(), WriteError>;
    pub fn snapshot(&mut self) -> Result<(), WriteError>;
}
```

`engined` owns the `Library` on a worker thread. The deck never sees it: the deck gets cues, grids and waveforms as it does now, from the loader, the analysis and the commands.

## Tests (planned)

- **Codec:** each operation round-trips; torn and corrupt lines are skipped; unknown paths and fields survive a snapshot.
- **Merge, property-tested:** random edit histories from three simulated devices give the same state under any replay order, duplicated segments and a snapshot taken at any point.
- **Identity:** two devices discovering the same file get the same id; a tag-edited copy merges by audio hash; aliases resolve.
- **Durability:** a store that fails or truncates writes at random points never loses a flushed record, and never shows a half-written blob.
- **Compaction:** after a snapshot and segment deletion the state is unchanged; a late offline journal still applies; tombstones stop resurrection.
- **Scale:** mount and replay 10,000 synthetic tracks, timed on the Mac and later on the Pi.

## Open questions

All answered on 2026-10-21; see "Decisions" at the top.
