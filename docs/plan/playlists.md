# Playlists (design, decided 2026-10-21)

Playlists are ordered lists of tracks in one library, arranged in a tree of folders, made and edited on the player and stored in the library file (`DJPLAYER/library.db`, schema version 2). The tag list is one more playlist with its own button. Rekordbox playlists are shown read-only beside ours. The owner settled the open questions on 2026-10-21 (Decisions, at the end).

**Status:** built. The library side (schema version 2, the edits and reads below, `crates/library/tests/playlists.rs`); the worker (files added in the background with records made where missing, the playlists published after every change); the browser (PLAYLISTS, the TAG LIST tab, TAG TRACK, the MENU actions, `ui/src/playlists.rs`). Not yet: moving a playlist or folder on the device (the library supports it; the MENU doesn't offer it), Rekordbox playlists (waits for Rekordbox reading).

## What the reference does

From the XDJ reference spec (and the XDJ-1000MK2 manual):

- **Playlists come from Rekordbox.** The player browses them (as a category, with Rekordbox's sort keys) and never creates or edits one.
- **The tag list is the player's own list,** one per medium, up to 100 tracks. TAG TRACK adds the selected or loaded track, a long press removes it. The tag list screen adds a track, a folder or a playlist, removes one or all, and converts the list into a playlist. It is written to the stick and shared over Link.

We go further: playlists are made and edited on the player, because a stick with no Rekordbox export has nowhere else to get them.

## Model

- **A playlist is an ordered list of entries,** each entry a track of the same library (by `TrackId`, so a moved or re-tagged file keeps its place). The same track may appear twice.
- **Playlists live in a tree:** folders hold playlists and folders, as in Rekordbox. Names are free text, unique only within a folder (not enforced).
- **The tag list is a playlist of kind `tag`:** one per library, always present, at the top of the tree, with no size limit. It stays on the stick until cleared. Converting it makes a normal playlist from its entries and empties it.
- **Per library.** A playlist never points into another library (data stays per library). Two sticks have two sets of playlists.
- **Missing files** keep their entries, shown dimmed; the track can't load until its file is found again.
- **Merged tracks:** an entry whose track was merged into another reads through the `aliases` table to the survivor, so a merge doesn't touch playlists.
- **Files without a record** (folder mode) get one when added, as on load (find or create by file hash).

## Schema version 2

| Table | Key | Holds |
| --- | --- | --- |
| `playlists` | `id` (16 random bytes) | `parent` (a folder's id, null at the top), `kind` (`folder`, `list`, `tag`), `name`, `position` (fractional index among its siblings), `created`, `updated_at`, `device` |
| `playlist_entries` | `id` (16 random bytes) | `playlist`, `track`, `position` (fractional index within the playlist), `added`, `updated_at`, `device` |

- **Positions are fractional indices** (strings that sort between their neighbours, as in Figma's or Linear's lists). A move or an insert writes one row, and two devices' edits interleave cleanly when sync comes.
- **Deleting a folder deletes what it holds** (cascade); deleting a playlist deletes its entries, never the tracks.
- **Versions:** `user_version` 2 adds both tables; `min_reader_version` stays 1, so an older player still opens the stick (and ignores the playlists).
- **Edits:** `Edit::CreatePlaylist`, `RenamePlaylist`, `MovePlaylist`, `DeletePlaylist`, `AddEntries` (tracks before an entry or at the end), `MoveEntry`, `RemoveEntries`, `ClearPlaylist`, `ConvertTagList`, queued and committed like other edits. Positions are worked out when an edit is applied, so the queued row operations replay exactly (all are idempotent).
- **Reads:** `Library::playlists()` (the tag list first, then the tree depth first, with entry counts), `playlist(id)`, `entries(id)` (in order, tracks resolved through merges).
- **Refused:** a second tag list, deleting or moving the tag list, a folder into itself, playlists inside a list, entries in a folder.
- **Positions** (`library::position`): base-36 fractions that never end in `0`; appends step the last digit, so 1,000 appends need about 30 characters; `between` halves the gap.
- **The tag list** has a fixed id (`PlaylistId::TAG_LIST`) and is made by every mount that finds it missing (a new file, a migrated one).

## On the player

- **Browsing:** LIBRARY's menu is PLAYLISTS, FOLDERS, TRACKS. PLAYLISTS shows the tag list, then our tree (Rekordbox's to follow, marked). A playlist and the tree list in their own order (SORT: `#`, the default) until another sort is picked; sorting is a view and never reorders the playlist. An entry whose file isn't known shows as "Missing track", dimmed, and doesn't load.
- **TAG LIST** (the tab): the tag list, with ADD FOLDER, REMOVE, REMOVE ALL and MAKE PLAYLIST.
- **TAG TRACK** (stand-in button, key `x`): adds the selected track in the browser (else the loaded track) to the tag list; a long press (0.6 s; key `shift+x`) removes it. A tagged track shows an amber dot before its title in every list.
- **Making and editing:** the MENU tab opens a list of what the selection allows: on a track ADD TO / REMOVE FROM TAG LIST and ADD TO PLAYLIST… (pick a list by its place in the tree, or NEW PLAYLIST named on the keyboard); on a folder MAKE PLAYLIST, and adding all its tracks; in a list REMOVE FROM THIS LIST and MOVE (the knob moves the entry, push or BACK drops it); on a playlist RENAME and DELETE (asks first); in PLAYLISTS NEW PLAYLIST and NEW FOLDER; on the tag list MAKE PLAYLIST FROM TAG LIST and REMOVE ALL (asks first). New entries go at the end.
- **Rekordbox playlists** (once Rekordbox is read): listed read-only after ours, marked; COPY TO MY PLAYLISTS makes one of ours from it, the original untouched.
- **Folders as playlists:** in FOLDERS (library) or USB, MENU on a folder offers MAKE PLAYLIST (its tracks, in name order, recursing into subfolders).
- **Search** looks through the shown playlist (or the whole library from the menu), as elsewhere.

## Later

- Smart playlists (rules over the tag cache: BPM range, key, genre, rating, analysed).
- M3U8 import and export, and Rekordbox playlist import (copies a Rekordbox playlist into ours).
- Prefetching the next tracks of the playing playlist into RAM (engine.md, cloud).
- Sync of playlists between a stick and the cloud, on `updated_at`, `device` and the fractional positions.

## Decisions (owner, 2026-10-21)

1. Playlists are made and edited on the player.
2. The tag list has no size limit (the reference's 100 bounds Link sharing, which we don't have).
3. The tag list stays on the stick until cleared; MAKE PLAYLIST turns it into a playlist and empties it.
4. A playlist may hold the same track more than once.
5. Playlists sit in a tree of folders, as in Rekordbox.
6. Rekordbox playlists are shown read-only beside ours, with COPY TO MY PLAYLISTS.
7. MAKE PLAYLIST on a folder (USB or the library's FOLDERS) takes its tracks, subfolders included, in name order.
8. Positions are fractional indices; `min_reader_version` stays 1.
9. Smart playlists come later.

Defaults taken without a question (say if any should change): TAG TRACK tags the selected track on the browse screen and the loaded track elsewhere; new entries go at the end; deleting a playlist or folder asks first; a tagged track shows a mark in every list.
