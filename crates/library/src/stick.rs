//! What is on a stick (or a folder standing in for one): its folders and audio files, with
//! the folders at its root that hold no tracks left out. Used by the browser and by the
//! library's scan alike.

use std::path::{Path, PathBuf};

use crate::LIBRARY_DIR;

/// File extensions listed as tracks (lower case).
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "aif", "aiff", "flac", "m4a", "mp4", "aac", "alac", "ogg", "oga",
];

/// Rekordbox's export folder at a stick's root.
pub const REKORDBOX_DIR: &str = "PIONEER";

/// Folders at a stick's root that hold no tracks: ours, and Rekordbox's database.
const HIDDEN_AT_ROOT: &[&str] = &[LIBRARY_DIR, REKORDBOX_DIR, "System Volume Information"];

/// One folder or audio file in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub folder: bool,
}

/// The folders and audio files in `dir`: folders first, then files, each sorted by name
/// without regard to case. Hidden files are left out, and at the root (`root`) the folders
/// that hold no tracks.
pub fn read_dir(dir: &Path, root: bool) -> std::io::Result<Vec<Item>> {
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
        let folder = meta.is_dir();
        if folder || (meta.is_file() && is_audio(&path)) {
            out.push(Item { name, path, folder });
        }
    }
    out.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Every audio file under `root`, in path order, skipping what [`read_dir`] hides. Links to
/// folders are not followed, so a link loop can't make it run forever.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), true)];
    while let Some((dir, at_root)) = dirs.pop() {
        let Ok(items) = read_dir(&dir, at_root) else {
            continue;
        };
        // Folders come first in a listing; pushed in reverse they are walked in order, after
        // this folder's tracks.
        let mut sub = Vec::new();
        for i in items {
            if !i.folder {
                out.push(i.path);
            } else if !std::fs::symlink_metadata(&i.path).is_ok_and(|m| m.file_type().is_symlink())
            {
                sub.push((i.path, false));
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

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stick-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        std::fs::write(p, b"").unwrap();
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
