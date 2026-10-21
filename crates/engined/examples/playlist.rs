//! Make playlists on a stick from the command line (for testing the browser):
//!
//! ```text
//! cargo run -p engined --example playlist -- <stick root> tag <files…>
//! cargo run -p engined --example playlist -- <stick root> list <name> [--in <folder>] <files…>
//! cargo run -p engined --example playlist -- <stick root> folder <name>
//! cargo run -p engined --example playlist -- <stick root> show
//! ```
//!
//! Folders are found by name at the top level. Files get library records as on the player.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use engined::persist::{LIBRARY_DIR, LibraryChoice, LibraryWorker, Playlists, Reply};
use library::{Edit, PlaylistId, PlaylistKind};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("give the stick's root folder")?);
    let what = args.next().ok_or("tag, list, folder or show")?;
    let w = LibraryWorker::start(None, LibraryChoice::Folder(root.join(LIBRARY_DIR)))
        .map_err(|e| e.to_string())?;
    let first = wait(&w, |_| true)?;
    let rest: Vec<String> = args.collect();
    let p = match what.as_str() {
        "tag" => {
            let n = first.entries.get(&PlaylistId::TAG_LIST).map_or(0, Vec::len);
            let files: Vec<PathBuf> = rest.iter().map(PathBuf::from).collect();
            let want = n + files.len();
            w.add_files(PlaylistId::TAG_LIST, files);
            wait(&w, |p| {
                p.entries.get(&PlaylistId::TAG_LIST).map_or(0, Vec::len) >= want
            })?
        }
        "list" => {
            let (name, files) = rest.split_first().ok_or("list needs a name")?;
            let mut parent = None;
            let mut files: Vec<String> = files.to_vec();
            if files.first().map(String::as_str) == Some("--in") {
                let folder = files.get(1).ok_or("--in needs a folder name")?.clone();
                parent = first
                    .lists
                    .iter()
                    .find(|p| p.kind == PlaylistKind::Folder && p.name == folder)
                    .map(|p| p.id);
                if parent.is_none() {
                    return Err(format!("no folder {folder:?}"));
                }
                files.drain(..2);
            }
            let id = PlaylistId::random();
            let want = files.len();
            w.new_playlist(
                id,
                parent,
                name.clone(),
                files.iter().map(PathBuf::from).collect(),
            );
            wait(&w, |p| p.entries.get(&id).map_or(0, Vec::len) >= want)?
        }
        "folder" => {
            let name = rest.first().ok_or("folder needs a name")?.clone();
            let id = PlaylistId::random();
            w.edit(Edit::CreatePlaylist {
                id,
                parent: None,
                kind: PlaylistKind::Folder,
                name,
            });
            wait(&w, |p| p.get(id).is_some())?
        }
        "show" => first,
        other => return Err(format!("unknown command {other:?}")),
    };
    for l in &p.lists {
        println!("{:?} {} ({} entries)", l.kind, l.name, l.entries);
    }
    Ok(())
}

/// The next playlists sent that satisfy `done` (the worker sends them after each change).
fn wait(w: &LibraryWorker, done: impl Fn(&Playlists) -> bool) -> Result<Playlists, String> {
    let until = Instant::now() + Duration::from_secs(60);
    let mut last = None;
    while Instant::now() < until {
        match w.try_recv() {
            Some(Reply::Playlists(p)) => {
                if done(&p) {
                    return Ok((*p).clone());
                }
                last = Some(p);
            }
            Some(Reply::Failed(e)) => return Err(e),
            Some(Reply::Message(m)) => eprintln!("{m}"),
            Some(_) => {}
            None if last.as_ref().is_some_and(|p| done(p)) => {
                return Ok((**last.as_ref().expect("checked")).clone());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    Err("timed out".into())
}
