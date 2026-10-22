//! Where the library lives: the folder chosen, `DJPLAYER/` on the track's removable
//! volume, or RAM; and how file paths are stored in it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use library::{DeviceId, Library, MountOptions};

use super::{LIBRARY_DIR, LibraryChoice};

/// Where the library is and how paths in it are written.
pub(super) struct Located {
    pub(super) lib: Library,
    /// Paths of files are stored relative to this (the folder holding `DJPLAYER/`).
    pub(super) root: Option<PathBuf>,
    pub(super) what: String,
}

/// Mount the chosen library (the automatic choice: on `file`'s stick) and say what it is.
pub(super) fn mount(
    choice: &LibraryChoice,
    file: &Path,
    device: DeviceId,
    say: &dyn Fn(String),
) -> Result<Located, String> {
    let started = Instant::now();
    let mut loc = locate(choice, file, device)?;
    let stats = loc.lib.stats().clone();
    say(format!(
        "library: {} ({} tracks, opened in {:.0} ms{})",
        loc.what,
        stats.tracks,
        started.elapsed().as_secs_f64() * 1e3,
        if stats.created { ", new" } else { "" },
    ));
    if stats.replayed > 0 {
        say(format!(
            "library: {} edits kept from an earlier mount written",
            stats.replayed
        ));
    }
    for w in loc.lib.take_warnings() {
        say(format!("library: {w}"));
    }
    Ok(loc)
}

/// `file` relative to `root`, `/`-separated and NFC; a file outside the root keeps its
/// absolute path (a hint only: identity is the hash).
pub fn stored_path(file: &Path, root: Option<&Path>) -> String {
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = root
        .and_then(|r| r.canonicalize().ok())
        .and_then(|r| abs.strip_prefix(r).ok().map(Path::to_path_buf));
    library::normalize_path(&rel.unwrap_or(abs).to_string_lossy())
}

/// True if a file can be made and removed in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".write-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The mount point holding `file`: the highest ancestor on the same device.
fn volume_root(file: &Path) -> PathBuf {
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        let mut cur = abs.parent().unwrap_or(&abs).to_path_buf();
        while let Some(parent) = cur.parent() {
            if dev(parent).is_none() || dev(parent) != dev(&cur) {
                break;
            }
            cur = parent.to_path_buf();
        }
        cur
    }
    #[cfg(not(unix))]
    {
        abs.ancestors().last().unwrap_or(&abs).to_path_buf()
    }
}

/// True if `root` (a mount point) is a removable volume: a USB or SD-card device that is not
/// the disk the system runs from. On macOS, a volume under `/Volumes`.
fn is_removable(root: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        // The disk (not the partition) a path's file system lives on, from sysfs.
        let disk = |p: &Path| -> Option<PathBuf> {
            let dev = std::fs::metadata(p).ok()?.dev();
            let (major, minor) = ((dev >> 8) & 0xfff, (dev & 0xff) | ((dev >> 12) & 0xfff00));
            let sys = std::fs::canonicalize(format!("/sys/dev/block/{major}:{minor}")).ok()?;
            Some(if sys.join("partition").exists() {
                sys.parent()?.to_path_buf()
            } else {
                sys
            })
        };
        let Some(d) = disk(root) else {
            return false;
        };
        if disk(Path::new("/")).is_some_and(|r| r == d) {
            return false;
        }
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let flagged = std::fs::read_to_string(d.join("removable")).is_ok_and(|s| s.trim() == "1");
        flagged || d.to_string_lossy().contains("/usb") || name.starts_with("mmcblk")
    }
    #[cfg(target_os = "macos")]
    {
        let abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        abs.starts_with("/Volumes") && abs != Path::new("/")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = root;
        false
    }
}

fn mount_options(device: DeviceId, read_only: bool) -> MountOptions {
    MountOptions {
        read_only,
        ..MountOptions::new(device)
    }
}

fn ram(device: DeviceId, why: String) -> Located {
    Located {
        lib: Library::ram(mount_options(device, false), format!("no library: {why}")),
        root: None,
        what: format!("RAM only, edits are not saved ({why})"),
    }
}

fn locate(choice: &LibraryChoice, file: &Path, device: DeviceId) -> Result<Located, String> {
    let folder = |dir: &Path| -> Result<Located, String> {
        let _ = std::fs::create_dir_all(dir);
        let read_only = !writable(dir);
        let lib = Library::mount(dir, mount_options(device, read_only))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let what = if read_only {
            format!("{} (read-only: edits stay in RAM, unsaved)", dir.display())
        } else {
            dir.display().to_string()
        };
        Ok(Located {
            lib,
            root: dir.parent().map(Path::to_path_buf),
            what,
        })
    };
    match choice {
        LibraryChoice::Ram => Ok(ram(device, "--no-library".into())),
        LibraryChoice::Folder(dir) => folder(dir),
        LibraryChoice::Auto => {
            let root = volume_root(file);
            if !is_removable(&root) {
                return Ok(ram(
                    device,
                    format!(
                        "{} is not a removable volume; use --library <dir>",
                        root.display()
                    ),
                ));
            }
            let dir = root.join(LIBRARY_DIR);
            // Making the folder is the write test at the stick's root; nothing else is written
            // outside it.
            if dir.is_dir() || std::fs::create_dir(&dir).is_ok() {
                folder(&dir)
            } else {
                Ok(ram(
                    device,
                    format!("{} is not writable; use --library <dir>", root.display()),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_stored_relative_to_the_library_root() {
        let dir = std::env::temp_dir().join(format!("engined-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Music")).unwrap();
        let f = dir.join("Music").join("a.mp3");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(stored_path(&f, Some(&dir)), "Music/a.mp3");
        let outside = stored_path(&f, Some(&dir.join("elsewhere")));
        assert!(outside.ends_with("/Music/a.mp3") && outside.starts_with('/'));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_system_disk_is_not_removable() {
        assert!(!is_removable(Path::new("/")));
        let here = std::env::current_dir().unwrap();
        // A checkout on the system disk; CI and dev machines alike.
        if volume_root(&here.join("Cargo.toml")) == Path::new("/") {
            assert!(!is_removable(&volume_root(&here.join("Cargo.toml"))));
        }
    }

    #[test]
    fn the_volume_root_is_an_ancestor() {
        let here = std::env::current_dir().unwrap();
        let root = volume_root(&here.join("Cargo.toml"));
        assert!(
            here.starts_with(&root),
            "{} {}",
            root.display(),
            here.display()
        );
    }
}
