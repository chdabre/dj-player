//! A stand-in network for screenshots and for trying the screens without an account or a
//! connection: every search finds the same spread of qualities for the query, and the
//! downloads stop where a screenshot wants them (the first is done, the second at 43 %, the
//! third waits for a slot, the rest fail). Nothing is written to disk.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};

use super::{Credentials, Format, Hit, HitKey, Network, Progress, Quality};

#[derive(Default)]
pub struct Demo {
    /// The search's hits are still to be handed out.
    fresh: Option<String>,
    /// Downloads asked for so far; their senders are kept so they don't end.
    downloads: Vec<Sender<Progress>>,
}

impl Network for Demo {
    fn has_account(&self) -> bool {
        true
    }

    fn set_account(&mut self, _: Option<Credentials>) {}

    fn connect(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn search(&mut self, query: &str) -> Result<(), String> {
        self.fresh = Some(query.to_string());
        Ok(())
    }

    fn hits(&mut self, _: &str) -> Option<Vec<Hit>> {
        self.fresh.take().map(|q| hits(&q))
    }

    fn forget(&mut self, _: &str) {}

    fn download(&mut self, hit: &Hit, _: &Path) -> Result<Receiver<Progress>, String> {
        let (tx, rx) = mpsc::channel();
        let total = hit.size;
        let steps = match self.downloads.len() {
            0 => vec![Progress::Receiving { done: 0, total }, Progress::Done],
            1 => vec![Progress::Receiving {
                done: total * 43 / 100,
                total,
            }],
            2 => vec![Progress::Queued],
            _ => vec![Progress::Failed("the peer is offline".into())],
        };
        for p in steps {
            let _ = tx.send(p);
        }
        self.downloads.push(tx);
        Ok(rx)
    }

    fn cancel(&mut self, _: &HitKey) {}
}

/// A file every search finds: its version, extension, attributes, size in MB, a free slot,
/// and the peer's speed.
type File = (
    &'static str,
    &'static str,
    &'static [(u32, u32)],
    u64,
    bool,
    u32,
);

/// What every search finds: `query` in several versions and qualities.
fn hits(query: &str) -> Vec<Hit> {
    let files: [File; 10] = [
        (
            "Original Mix",
            "flac",
            &[(5, 24), (4, 96_000)],
            212,
            true,
            2_400_000,
        ),
        (
            "Original Mix",
            "flac",
            &[(5, 16), (4, 44_100)],
            41,
            true,
            900_000,
        ),
        (
            "Extended Mix",
            "wav",
            &[(5, 16), (4, 44_100)],
            66,
            false,
            300_000,
        ),
        (
            "Original Mix",
            "aiff",
            &[(5, 16), (4, 44_100)],
            65,
            true,
            120_000,
        ),
        ("Original Mix", "mp3", &[(0, 320)], 15, true, 1_800_000),
        ("Radio Edit", "mp3", &[(0, 320)], 9, false, 600_000),
        ("Original Mix", "m4a", &[(0, 256)], 12, true, 50_000),
        (
            "Original Mix",
            "mp3",
            &[(0, 245), (2, 1)],
            11,
            true,
            400_000,
        ),
        ("Dub", "ogg", &[(0, 192)], 8, true, 80_000),
        ("Original Mix", "mp3", &[(0, 128)], 6, true, 2_000_000),
    ];
    let users = [
        "deepcrate",
        "vinyl_junkie",
        "ukg4life",
        "selector",
        "nightbus",
    ];
    files
        .iter()
        .enumerate()
        .filter_map(|(i, &(version, ext, attrs, mb, free_slot, speed))| {
            let name = format!("{query} ({version}).{ext}");
            let mut attributes: std::collections::HashMap<u32, u32> =
                attrs.iter().copied().collect();
            attributes.insert(1, 372);
            Some(Hit {
                user: users[i % users.len()].to_string(),
                path: format!("@@music\\Electronic\\{name}"),
                size: mb * 1_000_000,
                quality: Quality::new(Format::of(&name)?, &attributes),
                free_slot,
                speed,
            })
        })
        .collect()
}
