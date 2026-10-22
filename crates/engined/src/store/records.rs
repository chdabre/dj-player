//! What the worker does to records: find or make a file's record, keep its tag cache, store
//! waveforms and analysis results, and publish the listing and the playlists.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

use library::{
    AnalysisRef, Edit, FileHash, FileIdentity, Library, Track, TrackId, TrackTags, WaveformRef,
};
use waveform::compute::Waveform;

use library::stored_path;

use super::locate::Located;
use super::{FreshAnalysis, Listing, Loaded, Playlists, Reply};

pub(super) fn store_waveform(
    lib: &mut Library,
    track: TrackId,
    w: &Waveform,
    say: &dyn Fn(String),
) {
    let detail = w.to_blob();
    let overview = w.overview().to_blob(&w.params, w.provenance);
    let r = WaveformRef::new(lib.put_blob(&detail), lib.put_blob(&overview));
    if let Err(e) = lib.apply(Edit::SetWaveform(track, r)) {
        say(format!("library: {e}"));
    }
}

pub(super) fn store_analysis(
    lib: &mut Library,
    track: TrackId,
    fresh: Vec<FreshAnalysis>,
    say: &dyn Fn(String),
) {
    for f in fresh {
        let a = AnalysisRef::new(f.version, f.params, lib.put_blob(&f.blob));
        if let Err(e) = lib.apply(Edit::SetAnalysis(track, f.name, a)) {
            say(format!("library: {e}"));
        }
    }
}

/// Put our analysed BPM and key in the track's tag cache; true if it changed.
pub(super) fn store_bpm_key(
    lib: &mut Library,
    track: TrackId,
    bpm: Option<f64>,
    key: Option<String>,
    say: &dyn Fn(String),
) -> bool {
    let Ok(Some(t)) = lib.track(track) else {
        return false;
    };
    let tags = TrackTags {
        bpm: bpm.or(t.tags.bpm),
        key: key.or(t.tags.key.clone()),
        ..t.tags.clone()
    };
    if tags == t.tags {
        return false;
    }
    if let Err(e) = lib.apply(Edit::SetTags(track, tags)) {
        say(format!("library: {e}"));
        return false;
    }
    true
}

/// Scan the next of a scan's `files` (taken from the end): its record and its tags.
pub(super) fn scan_one(loc: &mut Located, files: &mut Vec<PathBuf>, say: &dyn Fn(String)) {
    let Some(file) = files.pop() else {
        return;
    };
    match open(loc, &file, &|_| {}) {
        Ok((track, _)) => {
            refresh_tags(&mut loc.lib, track, &file, say);
        }
        Err(e) => say(format!("library: scan: {e}")),
    }
}

/// Fill the track's tag cache from `file`'s tags. The BPM and key stay as our analysis set
/// them once the track has been analysed. True if the cache changed.
pub(super) fn refresh_tags(
    lib: &mut Library,
    track: TrackId,
    file: &Path,
    say: &dyn Fn(String),
) -> bool {
    let Ok(Some(record)) = lib.track(track) else {
        return false;
    };
    let Ok(read) = engine_media::tags(file) else {
        return false;
    };
    let analysed = !record.analysis.is_empty();
    let tags = TrackTags {
        title: read.title,
        artist: read.artist,
        album: read.album,
        genre: read.genre,
        bpm: if analysed { record.tags.bpm } else { read.bpm },
        key: if analysed {
            record.tags.key.clone()
        } else {
            read.key
        },
        duration: read.duration,
    };
    if tags == record.tags {
        return false;
    }
    if let Err(e) = lib.apply(Edit::SetTags(track, tags)) {
        say(format!("library: {e}"));
        return false;
    }
    true
}

/// Send the library's listing to the player.
pub(super) fn publish(lib: &Library, replies: &Sender<Reply>) {
    if let Ok(rows) = lib.listing() {
        let listing: Listing = rows.into_iter().map(|l| (l.path.clone(), l)).collect();
        let _ = replies.send(Reply::Listing(Arc::new(listing)));
    }
    publish_playlists(lib, replies);
}

/// Send the playlists and their entries to the player.
pub(super) fn publish_playlists(lib: &Library, replies: &Sender<Reply>) {
    let Ok(lists) = lib.playlists() else {
        return;
    };
    let entries = lists
        .iter()
        .filter(|p| p.kind != library::PlaylistKind::Folder)
        .filter_map(|p| Some((p.id, lib.entries(p.id).ok()?)))
        .collect();
    let _ = replies.send(Reply::Playlists(Arc::new(Playlists { lists, entries })));
}

pub(super) fn is_playlist_edit(e: &Edit) -> bool {
    matches!(
        e,
        Edit::CreatePlaylist { .. }
            | Edit::RenamePlaylist(..)
            | Edit::MovePlaylist { .. }
            | Edit::DeletePlaylist(_)
            | Edit::AddEntries { .. }
            | Edit::MoveEntry { .. }
            | Edit::RemoveEntries(_)
            | Edit::ClearPlaylist(_)
            | Edit::ConvertTagList { .. }
    )
}

/// Find or make `file`'s record: its track, and whether the record or the location is new.
pub(super) fn open(
    loc: &mut Located,
    file: &Path,
    say: &dyn Fn(String),
) -> Result<(TrackId, bool), String> {
    let (identity, size) =
        FileHash::of_file(file).map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let path = stored_path(file, loc.root.as_deref());
    let found = loc
        .lib
        .find_or_create(FileIdentity::new(path, size, identity))
        .map_err(|e| e.to_string())?;
    let track = found.track;
    if found.created {
        say(format!("library: new track {track}"));
    } else if found.new_location {
        say(format!(
            "library: track {track} (known file at a new location)"
        ));
    }
    Ok((track, found.new_location))
}

/// The record of `track`, with its stored waveform and analysis blobs read.
pub(super) fn loaded(lib: &mut Library, track: TrackId, serial: u64) -> Loaded {
    let record = lib
        .track(track)
        .ok()
        .flatten()
        .unwrap_or_else(|| Track::empty(track));
    let waveform = record
        .waveform
        .as_ref()
        .and_then(|w| lib.read_blob(w.blob).ok())
        .and_then(|b| Waveform::from_blob(&b).ok());
    let analysis = record
        .analysis
        .iter()
        .filter_map(|(name, a)| {
            let blob = lib.read_blob(a.blob).ok()?;
            Some((name.clone(), (a.clone(), blob)))
        })
        .collect();
    Loaded {
        serial,
        track: lib.track_ref(record.id),
        record,
        waveform,
        analysis,
    }
}
