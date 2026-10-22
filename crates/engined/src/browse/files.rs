//! A folder's listing as browser entries (see [`crate::stick`] for what is listed).

use std::path::Path;

use super::{Entry, Kind};
use crate::stick;

/// The folders and tracks in `dir`, as [`stick::read_dir`] lists them.
pub(crate) fn list(dir: &Path, root: bool) -> std::io::Result<Vec<Entry>> {
    Ok(stick::read_dir(dir, root)?
        .into_iter()
        .map(|i| Entry {
            name: i.name,
            kind: if i.folder { Kind::Folder } else { Kind::Track },
            path: i.path,
            entry: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::{temp_dir, touch};
    use library::LIBRARY_DIR;

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
}
