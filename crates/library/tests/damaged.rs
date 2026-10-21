//! Unclean closes, damaged files and files from newer versions.

use std::path::{Path, PathBuf};

use library::{DeviceId, Edit, FileHash, FileIdentity, HotCue, Library, MountError, MountOptions};
use rusqlite::Connection;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("library-damaged-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("DJPLAYER")
}

fn mount(dir: &Path) -> Library {
    Library::mount(dir, MountOptions::new(DeviceId::from_bytes([1; 16]))).unwrap()
}

fn file(i: usize) -> FileIdentity {
    let bytes = format!("track {i}");
    FileIdentity::new(
        format!("Music/{i:04} with a long enough name to fill pages.mp3"),
        bytes.len() as u64,
        FileHash::of(bytes.as_bytes()),
    )
}

/// A library of `n` tracks, written in two mounts so that a backup exists.
fn library(dir: &Path, n: usize) -> library::TrackId {
    let first = {
        let mut lib = mount(dir);
        let t = lib.find_or_create(file(0)).unwrap().track;
        lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(1.0))))
            .unwrap();
        t
    };
    let mut lib = mount(dir);
    for i in 1..n {
        lib.find_or_create(file(i)).unwrap();
    }
    first
}

/// What the player does after a crash: the file says it was not closed.
fn mark_unclean(dir: &Path) {
    let c = Connection::open(dir.join("library.db")).unwrap();
    c.execute("UPDATE meta SET value = 0 WHERE key = 'clean_unmount'", [])
        .unwrap();
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_sound_file_after_an_unclean_close_passes_the_check() {
    let dir = temp("sound");
    library(&dir, 10);
    mark_unclean(&dir);
    let mut lib = mount(&dir);
    assert!(lib.stats().needs_check);
    assert_eq!(lib.check(), None);
    assert_eq!(lib.check(), None, "once per mount");
    drop(lib);
    assert!(!mount(&dir).stats().needs_check, "closed cleanly again");
    cleanup(&dir);
}

#[test]
fn a_damaged_page_sets_the_file_aside_and_goes_back_to_the_backup() {
    let dir = temp("page");
    let t = library(&dir, 2000);
    mark_unclean(&dir);
    // Zero the last pages: rows of the second mount, which the backup predates.
    let path = dir.join("library.db");
    let mut bytes = std::fs::read(&path).unwrap();
    let n = bytes.len();
    bytes[n - 4 * 4096..].fill(0);
    std::fs::write(&path, bytes).unwrap();
    let mut lib = mount(&dir);
    let t2 = lib.find_or_create(file(5000)).unwrap().track;
    let problem = lib.check().expect("damage found");
    assert!(!problem.is_empty());
    let warnings = lib.take_warnings().join("\n");
    assert!(warnings.contains("back to the backup"), "{warnings}");
    // The backup's track and this mount's edit are both there.
    assert_eq!(
        lib.track(t).unwrap().unwrap().hot_cues[0]
            .as_ref()
            .unwrap()
            .at,
        1.0
    );
    assert!(lib.track(t2).unwrap().is_some());
    drop(lib);
    let kept = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("library.db.corrupt-")
        })
        .count();
    assert!(kept >= 1);
    assert_eq!(mount(&dir).track(t2).unwrap().map(|r| r.id), Some(t2));
    cleanup(&dir);
}

#[test]
fn a_file_that_is_not_a_database_is_set_aside() {
    let dir = temp("garbage");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("library.db"), vec![0x5a; 8192]).unwrap();
    let mut lib = mount(&dir);
    assert!(
        lib.take_warnings()
            .join("")
            .contains("starting an empty library")
    );
    assert!(lib.tracks().unwrap().is_empty());
    drop(lib);
    cleanup(&dir);
}

#[test]
fn a_newer_version_is_written_unless_it_says_not_to() {
    let dir = temp("newer");
    let t = library(&dir, 1);
    {
        // A newer version added a column and a table, but says this one may still write.
        let c = Connection::open(dir.join("library.db")).unwrap();
        c.execute_batch(
            "ALTER TABLE hot_cues ADD COLUMN quantized INTEGER; \
             UPDATE hot_cues SET quantized = 1; PRAGMA user_version = 3;",
        )
        .unwrap();
    }
    {
        let mut lib = mount(&dir);
        lib.apply(Edit::SetHotCue(t, 0, Some(HotCue::at(2.0))))
            .unwrap();
    }
    let c = Connection::open(dir.join("library.db")).unwrap();
    let (at, q): (f64, i64) = c
        .query_row("SELECT at, quantized FROM hot_cues", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((at, q), (2.0, 1), "the newer column survives the edit");
    c.execute(
        "UPDATE meta SET value = 3 WHERE key = 'min_reader_version'",
        [],
    )
    .unwrap();
    drop(c);
    assert!(matches!(
        Library::mount(&dir, MountOptions::new(DeviceId::random())),
        Err(MountError::Newer(3))
    ));
    cleanup(&dir);
}
