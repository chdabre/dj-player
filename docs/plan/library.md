# Library format (design)

Status 2026-10-21: rebuilt on SQLite after the spike (`library-sqlite.md`, summarised under "Why SQLite" below); the build is in `crates/library` and wired into `engined` (see "Status"). It makes concrete the "Own library on the stick" and "Cloud library" sections of the engine research (`docs/research/engine.md`). The owner's decisions below win over anything else in this doc.

## Decisions (owner, 2026-10-21)

These settle the open questions and extend the design; where this section and the rest of the doc disagree, this section wins.

**Libraries and writing**
- A player sees several libraries at once: its own slots, linked players' sticks, later the cloud. Tracks are addressed as (library, track).
- Data stays per library: no automatic merge across sticks, and no cross-encode cue copy in version 1.
- One writer per stick: the player it's plugged into. Linked players can't write (merge strategies for sync come later).
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

**Format (owner, 2026-10-21, after the SQLite spike; replaces the journals)**
- One SQLite file, `DJPLAYER/library.db`; blobs stay files. No op log, HLC, CRDT, version vectors or snapshots. Concurrent offline edits can't happen (a stick is in one player, and linked players don't write), so there is nothing to merge; a sync layer for the cloud is designed later, on the `updated_at` and `device` every row carries.
- Yanks: edits are queued in RAM and committed about once a second; a failed write keeps the queue, shown as unsaved, and it is written when the same library mounts again.
- A backup, `library.db.bak`, on each mount that writes.
- `synchronous=FULL`.
- After an unclean unmount, the integrity check runs in the background.
- A stick with no library: the browser asks whether to scan it; otherwise it offers only the folder tree (records still come lazily, on load).

**After the first build (owner, 2026-10-21)**
- **Alias merges:** the older track survives (as above): the one this library saw first.
- **Cue point at load:** the first stored memory cue if there is one; else, with auto cue on, the first sound at or above the auto cue level; else no cue point and the playhead at 0. Auto cue is a player setting (on by default at −36 dB; `--auto-cue off|<dB>`).
- **Hot cue bank B:** stored and merged now; the deck's bank switch comes later.
- **Snapshot size:** no smaller encoding or compression in version 1.
- **Default library location:** automatic only on removable volumes (USB sticks, SD cards), never internal storage. The internal-storage mode will name its own library folder.
- **Stored analysis:** once one analyzer reruns, every later one reruns too.
- **Device id on dev machines:** `~/.config/djplayer/device-id`, overridden by `$DJPLAYER_CONFIG`.

## Layout

```
DJPLAYER/
  library.db         SQLite, WAL with exclusive locking (no -shm file)
  library.db-wal     only while mounted (or after a crash)
  library.db.bak     the file as it was before this mount's first write
  blobs/<aa>/<hash>  BLAKE3-named waveforms and analysis results, written once
```

## Schema (version 1)

| Table | Key | Holds |
| --- | --- | --- |
| `meta` | key | `library_id` (16 random bytes), `min_reader_version`, `clean_unmount` |
| `tracks` | `id` (16 bytes, from the file hash at first sight) | tag cache (title, artist, album, genre, BPM, key, duration: the file's tags, with our analysed BPM and key once there is an analysis), rating, colour, comment, audio hash and its version, waveform and overview blob names |
| `locations` | (track, path) | size, file hash; indexed by file hash |
| `hot_cues` | (track, slot 0–15) | at, end, colour, comment |
| `cues` | id (16 random bytes) | track, kind (memory or loop), at, end, colour, comment |
| `analysis` | (track, analyzer) | version, params, blob, created |
| `aliases` | old track id | the track it was merged into |

Every row of an edit carries `updated_at` (wall-clock ms, 0 without a clock) and `device`. Playlists, tags, history, settings and the FTS5 search index come with the features that use them, as schema version 2 and later.

**Versions.** `PRAGMA user_version` is the schema version, and `meta.min_reader_version` the oldest version that may still read and write the file. A newer file whose `min_reader_version` this build meets is used as is: writes name their columns (`INSERT … ON CONFLICT DO UPDATE SET` the columns this version knows, never `INSERT OR REPLACE`), so columns a newer version added survive. Otherwise it is refused.

## Writes and yanks

- **One writer:** the library worker on the player holds the file with exclusive locking, which also keeps any second writer out.
- **An edit** runs against the file at once, inside an open write transaction, so reads see it; it also goes into the RAM queue. `flush` (the worker calls it about 1 s after the first uncommitted edit) commits; the queue is cleared on commit.
- **A failed write** (pulled, full or read-only stick) rolls the transaction back and keeps the queue; the status band shows UNSAVED (`Persistence::Unsaved`). Each later flush tries the whole queue again. When the library is dropped with edits queued, they wait in the process (with any blobs kept in RAM) and are written when a library with the same `library_id` mounts. A pulled stick loses nothing it had committed: decks keep playing from RAM, and with WAL + FULL the spike lost nothing in any crash or yank test.
- **Unmount:** commit, set `clean_unmount`, `wal_checkpoint(TRUNCATE)`.
- **Backup:** before the first write of a mount, `VACUUM INTO library.db.bak.tmp`, synced and renamed over `library.db.bak`. About 80 ms at 10k tracks on an SSD; seconds on a stick at 50k (est.).
- **Check:** a mount that finds `clean_unmount` unset (crash or yank) runs `PRAGMA quick_check` after the loaded track's record is out, so the load isn't held up. A write that finds damage asks for the same check.
- **Damage:** a file that is damaged or not a database is moved aside as `library.db.corrupt-<date>` (with its WAL), the backup copied in its place (or an empty library made), and this mount's edits written onto it.
- **Read-only stick:** the file is read once into RAM (`immutable=1`, then the backup API); edits stay in RAM, unsaved, and are written when the stick mounts writable. A WAL left by a crash on a read-only stick is not read.
- **No library:** an in-memory database; its edits carry over between loads in one process, and are lost at exit.

## Identity

- **Track id:** the first 16 bytes of BLAKE3 over `"track" ‖ file_hash`, taken when the file is first seen. Two devices that find the same file independently create the same id.
- **`file_hash`:** Dropbox's `content_hash`, computed locally for USB files too: SHA-256 over the concatenated SHA-256 digests of 4 MiB blocks. The same file on a stick and in Dropbox then matches without a download.
- **Exact duplicate:** a file hash already known at a new path is another location of that track.
- **`audio_hash`:** `{v: 1, hash}`, BLAKE3 over our decoder's interleaved output at the file's rate as 16-bit LE (`round(x·32768)`, clamped), computed once during analysis and kept on the track. Recomputed on load when its version is stale.
- **Merge:** tracks with the same current audio hash (tag-edited copies) merge into the one this library saw first (lowest rowid). The other's locations and memory cues move over; its hot cues and analysis results fill empty slots or replace older ones (by `updated_at`); its rating, colour, comment and waveform fill empty fields. Its row is deleted and `aliases` points its id (and any ids aliased to it) at the survivor. Edits to an aliased id go to the survivor. A different encode stays a separate track.
- **Paths:** relative to the folder holding `DJPLAYER/`, `/`-separated, NFC-normalised. A path is a hint for finding the file, never its identity. Missing files keep their record.

## Blobs

- **Naming:** `blobs/<aa>/<64 hex>`, the BLAKE3 hash of the bytes. A blob's own header says what it is (waveforms are DJWF); analysis results are a small versioned format per analyzer.
- **Writing:** tmp file in the same folder, sync, rename, sync the folder: before the row that names it is committed. A blob that can't be written fails the transaction (so no row names a missing blob) and stays in RAM with the queue.
- **Reading** checks the bytes against the name.

## Multiple libraries and linked players

A player sees several libraries (its slots, linked players' sticks, later the cloud); tracks are addressed as `TrackRef { library, track }`, and data stays per library. The player a stick is plugged into is its only writer; linked players read through the link and don't write (until a sync design says how). This depends on the link protocol (`engine-sync`), not designed yet.

## Cloud

Later. The same `library.db` in a cloud folder can't be written by two devices at once; the sync layer (per-row `updated_at` and `device`, merge strategies) is designed when the cloud library is built.

## Why SQLite

The first build (CRC-framed JSON-lines journals per device, HLC, CRDT merge, version vectors, snapshots, a writer lock: about 3,000 lines) solved concurrent offline writes, which can't happen here. The spike measured one SQLite file on the same workload: at 50k tracks, open in 3 ms, 4–31 MB of RAM, a 103 MB file, and a 20-edit batch on exFAT in 4.4 ms; WAL + FULL lost nothing in the crash, yank and fault-injection tests (ran). A torn page on a stick that lies about flushes costs that page's rows; the check and the backup cover it (a salvage path is still to do). rusqlite 0.40 with the bundled SQLite (3.53) cross-builds for the Pi.

## Status (2026-10-21)

**Built**

- **`crates/library`** (about 2,100 lines of source, 1,200 of them `library.rs`, plus 560 of tests; the journal version had 3,700 and 1,060): the schema above (`db.rs`), `Library` (`library.rs`): `mount(dir, MountOptions)`, `ram`, `find_or_create`, `set_audio_hash`, `apply(Edit)`, `track`, `tracks`, `put_blob`, `read_blob`, `flush`, `check`, `persistence`; the queue, replay on remount, backup, check and damage handling as described. Tests: identity, duplicates, merges, every edit kind, the folder layout and backup, a read-only stick, a failing write and its retry, RAM carry-over, an unclean close, a damaged page restored from the backup, a file that isn't a database, and a newer schema.
- **`engined`:** the `Player` runs the library on a worker thread (`persist.rs`). `--library <dir>` (or `--no-library`); the default is `DJPLAYER/` at the root of the track's volume if it is removable and writable (an existing read-only one is read-only), else RAM only, and it says which. On load it finds or creates the record and restores hot cues 1–8, the cue point and a stored loop into the deck through `DeckCommand::SetCues`. The first restore only fills what the deck doesn't have, so cues the DJ set while the library was being read stay and are written back. It uses the stored waveform and stored analysis results whose analyzer version and parameters match (no decode at all if the audio hash is current too), and writes new waveform and analysis blobs, the audio hash, hot-cue edits and memory cues (key `m`) back, committed within about 1 s. The check runs on the worker after the record is out. The UI's status band shows SAVED / SAVING / UNSAVED.
- **Loading at run time:** one library worker per player mounts once (the automatic choice on the first loaded track's stick) and is told each load with a serial (`LibraryWorker::open`); answers for an earlier load are ignored. The previous track's cue edits are written before the next loads. A file that can't be read gets no record (`Reply::Unrecorded`) and is analysed from scratch.
- **Browser** (`engined::browse`, the UI's browse screen): USB shows the stick's folders and audio files (folders first, without our `DJPLAYER/`, Rekordbox's `PIONEER/` or hidden files); LIBRARY opens on a menu: FOLDERS (the library's tracks in the stick's folder structure, built from their stored paths, so a scanned stick keeps its folders) and TRACKS (every track in one list). Tags, length and artwork are read from the files on a background thread, the shown folder first, and kept in RAM for the session. Sorting by name, title, artist, BPM, key or length; search over everything under the shown folder (or the whole library) by file name and tags. On the dev host `LIBRARY_DIR=<dir>` makes a folder the stick's root, with the library at `<dir>/DJPLAYER`.
- **Tag cache:** `Edit::SetTags` and `Library::listing()` (every location with its track's tags). A load fills the cache from the file's tags; an analysis puts our BPM and key in it (and a later load keeps them). The worker publishes the listing (`Reply::Listing`) after the mount and whenever tracks or tags change; the browser shows it and prefers its BPM and key over the files'.
- **Scan:** a stick without `DJPLAYER/library.db` asks once per mount whether to scan it. A scan (`LibraryWorker::scan`, also the SCAN chip in LIBRARY) walks every audio file under the root and gives it a record with its tags, one file at a time between other requests, so loads aren't held up; it reports progress (`Reply::Scan`) and republishes the listing twice a second. It does not analyse.
- **ANALYZE ALL** (`engined::batch`, the LIBRARY header while any track lacks a waveform or analysis): a background job analyses the library's unanalysed tracks one at a time: tempo, grid and key as a loaded track gets them, the waveform and the audio hash, from one decode. Results go to the worker by file (`LibraryWorker::store`), so the deck's track stays the one other requests are for. The job waits while the deck's own track decodes or analyses, and STOP cancels the file under way (what is done stays stored). The listing marks tracks `analysed`.
- **Replacing a track** cancels the previous one's decode and analysis (`engine_media::Cancel`).

**Measured** (`cargo run --release -p library --example scale`, Linux cloud container, SSD; Mac and Pi still to do). 10,000 synthetic tracks, each with a location and audio hash, 8 hot cues, 3 memory cues, a rating, 3 analysis results and a waveform, written in 20 mounts:

| Step | Result |
| --- | --- |
| `library.db` | 17.8 MB (folder with blobs 33.5 MB) |
| Open | 0.6 ms |
| Read one track | 0.16 ms |
| First commit of a mount (20 edits + the backup) | 81 ms |
| A later commit of 20 edits | 0.8 ms |

**Left**

- Library views beyond the track list (artist, album, BPM, key categories), and an FTS5 search index once libraries get large (search is a linear match over tags in RAM today).
- Artwork is read from the files each session; it isn't stored as a blob.
- Rekordbox import and overlay, playlists, tags, history and play counts, settings, "remove from library", blob GC, the link protocol and the cloud with its sync layer.
- Hot cues 9–16 (bank B) are stored but the deck has 8 pads and no bank switch yet. Memory cues are stored and restored as the load cue point, but there is no CALL / memory-delete on the deck yet.
- A salvage path (the recovery extension) for a damaged file with no usable backup; today it starts empty and keeps the damaged file.
- Two decks loading from one stick at once: both workers would open the file, and the second waits up to 5 s for the lock. Share one library per stick when the player gets its second deck.
- The scale numbers on the Mac and the Pi, and on a real stick.
