//! The stick's files: a folder's listing, and every audio file under a folder.

use std::path::{Path, PathBuf};

use super::{Entry, Kind};
use crate::persist::LIBRARY_DIR;

/// File extensions the browser lists as tracks (lower case).
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "aif", "aiff", "flac", "m4a", "mp4", "aac", "alac", "ogg", "oga",
];

/// Folders at a stick's root that hold no tracks: ours, and Rekordbox's database.
const HIDDEN_AT_ROOT: &[&str] = &[LIBRARY_DIR, "PIONEER", "System Volume Information"];

/// The folders and tracks in `dir`: folders first, then tracks, each sorted by name without
/// regard to case. Hidden files are left out, and at the root (`root`) the folders that hold
/// no tracks.
pub(crate) fn list(dir: &Path, root: bool) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (root && HIDDEN_AT_ROOT.contains(&name.as_str())) {
            continue;
        }
        let path = e.path();
        // Follows symlinks: a linked folder browses like a folder.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let kind = if meta.is_dir() {
            Kind::Folder
        } else if meta.is_file() && is_audio(&path) {
            Kind::Track
        } else {
            continue;
        };
        out.push(Entry {
            name,
            path,
            kind,
            entry: None,
        });
    }
    out.sort_by(|a, b| {
        (a.kind == Kind::Track)
            .cmp(&(b.kind == Kind::Track))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Every audio file under `root`, in path order, skipping what [`list`] hides. Links to
/// folders are not followed, so a link loop can't make it run forever.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), true)];
    while let Some((dir, at_root)) = dirs.pop() {
        let Ok(entries) = list(&dir, at_root) else {
            continue;
        };
        // Folders come first in a listing; pushed in reverse they are walked in order, after
        // this folder's tracks.
        let mut sub = Vec::new();
        for e in entries {
            match e.kind {
                Kind::Track => out.push(e.path),
                Kind::Folder => {
                    let is_link = std::fs::symlink_metadata(&e.path)
                        .is_ok_and(|m| m.file_type().is_symlink());
                    if !is_link {
                        sub.push((e.path, false));
                    }
                }
                Kind::Playlist(_) => {}
            }
        }
        dirs.extend(sub.into_iter().rev());
    }
    out.sort();
    out
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::{temp_dir, touch};

    #[test]
    fn lists_folders_then_tracks_and_hides_what_holds_no_tracks() {
        let d = temp_dir("list");
        for f in ["b.mp3", "A.WAV", "notes.txt", ".hidden.mp3"] {
            touch(&d.join(f));
        }
        for f in ["Zed", "alpha", LIBRARY_DIR, "PIONEER", ".Trashes"] {
            std::fs::create_dir(d.join(f)).unwrap();
        }
        let names = |root| -> Vec<String> {
            list(&d, root)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect()
        };
        assert_eq!(names(true), ["alpha", "Zed", "A.WAV", "b.mp3"]);
        assert_eq!(
            names(false),
            ["alpha", LIBRARY_DIR, "PIONEER", "Zed", "A.WAV", "b.mp3"]
        );
    }

    #[test]
    fn walks_every_track_below_a_folder() {
        let d = temp_dir("walk");
        std::fs::create_dir_all(d.join("x/y")).unwrap();
        std::fs::create_dir_all(d.join(LIBRARY_DIR)).unwrap();
        for f in [
            "a.mp3",
            "x/b.mp3",
            "x/y/c.wav",
            "x/notes.txt",
            "DJPLAYER/d.mp3",
        ] {
            touch(&d.join(f));
        }
        let rel: Vec<PathBuf> = walk(&d)
            .into_iter()
            .map(|p| p.strip_prefix(&d).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            rel,
            [
                PathBuf::from("a.mp3"),
                PathBuf::from("x/b.mp3"),
                PathBuf::from("x/y/c.wav")
            ]
        );
    }
}
