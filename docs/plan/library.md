# Library format (design, draft)

Status 2026-10-21: design for review, no code yet. It makes concrete the "Own library on the stick" and "Cloud library" sections of the engine research (`docs/research/engine.md`). The decisions marked **(to confirm)** are the owner's.

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

pub struct Library { /* merged state, this device's open segment, HLC */ }
impl Library {
    pub fn mount(store: impl Store, device: DeviceId) -> Result<Self, MountError>;
    pub fn track(&self, id: TrackId) -> Option<TrackView<'_>>;           // the merged fields
    pub fn apply(&mut self, edit: Edit) -> Result<(), WriteError>;     // stamps, journals, merges
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

## Open questions (to confirm)

1. **Encoding:** JSON lines with a CRC (readable, easy to debug and to document as an open format, about 2× bigger) or CBOR (compact, binary)? Recommended: JSON lines; the journals are small either way, and blobs are already binary.
2. **Folder name** at the stick root: `DJPLAYER/` (recommended; plain and visible), or a hidden `.djplayer/` (hidden on Mac and Linux, but visible on Windows anyway).
3. **Hot cues:** 16 (two banks of 8, as the research says), or 8 like the reference?
4. **History:** store full play history (when, which deck, how long it played), or only play counts and a last-played date?
5. **Scope of the first implementation:** I'd start with the crate on a folder store: identity, cues, hot cues, ratings, analysis and waveform blobs, wired into `engined` so cues and waveforms persist across loads. Playlists, tags, history and the cloud come after.
