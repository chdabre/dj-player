//! Loading a download while it arrives: when the DJ starts a download with the deck paused
//! and nothing else downloading, the track goes onto the deck as soon as enough of it is
//! there to open, and decodes and analyses as the rest comes, so it can be cued and played
//! long before the download ends. The screen stays where it is.
//!
//! A deck that starts playing before the track is loaded is left alone (the track is not
//! loaded early, nor when complete). A load that comes too early is tried again once more
//! has arrived. When the download ends, the player hears it: a complete file gets its
//! library record, a stopped one stays on the deck as far as it arrived.

use std::path::{Path, PathBuf};

use soulseek::{Hit, HitKey, Snapshot, Transfer};

use super::App;
use super::track::Track;

/// Bytes that must have arrived before a load is tried, and how many more before the next
/// try if it was too early.
const FIRST_TRY: u64 = 256 * 1024;
const RETRY_STEP: u64 = 256 * 1024;

/// A download the deck follows.
#[derive(Debug)]
pub(super) struct Arrival {
    key: HitKey,
    /// The file once complete.
    path: PathBuf,
    /// On the deck, still arriving.
    loaded: bool,
    /// Bytes to wait for before the next load.
    next_try: u64,
}

/// Whether to load `hit` while it arrives: the deck is paused, nothing else is downloading,
/// and its format plays from its start (`downloads` is SoulSeek as of this frame).
pub(super) fn follow(playing: bool, downloads: Option<&Snapshot>, hit: &Hit) -> bool {
    !playing && hit.quality.format.streams() && downloads.is_none_or(|s| s.progress().0 == 0)
}

/// What to do about the followed download this frame.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    /// Load it as it arrives, `len` bytes in all.
    Load {
        len: u64,
    },
    /// It is complete: tell the player (if on the deck), or load it now.
    Complete(PathBuf),
    /// It stopped (failed or cancelled): tell the player, if on the deck.
    Stopped,
    /// Let it go: the deck plays something else.
    Drop,
}

impl Arrival {
    pub(super) fn new(hit: &Hit, downloads_dir: &Path) -> Self {
        Self {
            key: hit.key(),
            path: downloads_dir.join(hit.name()),
            loaded: false,
            next_try: FIRST_TRY,
        }
    }

    /// The next step, from the download's state and the deck's (`on_deck`: the file there).
    fn step(&self, downloads: &Snapshot, playing: bool, on_deck: Option<&Path>) -> Step {
        let left = self.loaded && on_deck != Some(self.path.as_path());
        match downloads.queued(&self.key).map(|q| &q.transfer) {
            None | Some(Transfer::Failed(_)) => Step::Stopped,
            _ if left => Step::Drop,
            Some(Transfer::Done(path)) if self.loaded || !playing => Step::Complete(path.clone()),
            Some(Transfer::Done(_)) => Step::Drop,
            _ if self.loaded => Step::Wait,
            _ if playing => Step::Drop,
            Some(Transfer::Receiving { done, total }) if *done >= self.next_try.min(*total) => {
                Step::Load { len: *total }
            }
            Some(Transfer::Receiving { .. } | Transfer::Queued) => Step::Wait,
        }
    }
}

impl App {
    /// Follow the download the deck waits for, once per frame.
    pub(super) fn follow_arrival(&mut self) {
        let (Some(a), Some(s)) = (self.arrival.as_mut(), self.soulseek_seen.as_ref()) else {
            return;
        };
        let on_deck = self.on_deck.as_ref().map(|d| d.file.as_path());
        match a.step(s, self.deck.playing, on_deck) {
            Step::Wait => {}
            Step::Load { len } => {
                let done = s.queued(&a.key).map_or(0, |q| match q.transfer {
                    Transfer::Receiving { done, .. } => done,
                    _ => 0,
                });
                let part = soulseek::part(&a.path);
                match self.player.load_arriving(&a.path, &part, len) {
                    Ok(()) => {
                        a.loaded = true;
                        let path = a.path.clone();
                        self.took(&path);
                        eprintln!("loaded {} as it arrives", path.display());
                    }
                    Err(_) => a.next_try = done + RETRY_STEP,
                }
            }
            Step::Complete(path) => {
                if a.loaded {
                    if let Some(line) = self.player.arrived(&path, true) {
                        eprintln!("{line}");
                    }
                } else if self.player.load(&path).is_ok() {
                    self.took(&path);
                }
                self.arrival = None;
            }
            Step::Stopped => {
                if a.loaded
                    && let Some(line) = self.player.arrived(&a.path.clone(), false)
                {
                    eprintln!("{line}");
                }
                self.arrival = None;
            }
            Step::Drop => self.arrival = None,
        }
    }

    /// `path` is on the deck now: the UI's view of the track follows (the screen stays).
    pub(super) fn took(&mut self, path: &Path) {
        let tags = self.browser.as_ref().and_then(|b| b.info(path));
        self.track = Track::new(path, tags.as_ref(), self.player.output().sample_rate);
        self.on_deck = self.player.on_deck();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soulseek::{Format, Quality, Queued};

    fn hit(name: &str) -> Hit {
        Hit {
            user: "peer".into(),
            path: format!("@@share\\{name}"),
            size: 10_000_000,
            quality: Quality::new(Format::of(name).unwrap(), &Default::default()),
            free_slot: true,
            speed: 1,
        }
    }

    fn downloading(h: &Hit, transfer: Transfer) -> Snapshot {
        Snapshot {
            transfers: vec![Queued {
                hit: h.clone(),
                transfer,
                path: PathBuf::from("/stick/SoulSeek Downloads").join(h.name()),
            }],
            ..Snapshot::default()
        }
    }

    #[test]
    fn only_a_paused_deck_with_nothing_else_downloading_follows_a_streamable_file() {
        let flac = hit("a.flac");
        assert!(follow(false, None, &flac));
        assert!(!follow(true, None, &flac), "the deck plays");
        assert!(
            !follow(false, None, &hit("a.m4a")),
            "its index may be at the end"
        );
        let busy = downloading(&hit("b.mp3"), Transfer::Queued);
        assert!(
            !follow(false, Some(&busy), &flac),
            "another download is first"
        );
        let done = downloading(&hit("b.mp3"), Transfer::Done("/x".into()));
        assert!(
            follow(false, Some(&done), &flac),
            "finished ones don't count"
        );
    }

    #[test]
    fn it_loads_once_enough_arrived_then_hands_over_on_completion() {
        let h = hit("a.flac");
        let dir = Path::new("/stick/SoulSeek Downloads");
        let mut a = Arrival::new(&h, dir);
        let path = dir.join("a.flac");
        let at = |done| {
            downloading(
                &h,
                Transfer::Receiving {
                    done,
                    total: 10_000_000,
                },
            )
        };
        assert_eq!(
            a.step(&downloading(&h, Transfer::Queued), false, None),
            Step::Wait
        );
        assert_eq!(a.step(&at(1000), false, None), Step::Wait, "too little yet");
        assert_eq!(
            a.step(&at(FIRST_TRY), false, None),
            Step::Load { len: 10_000_000 }
        );
        assert_eq!(
            a.step(&at(FIRST_TRY), true, None),
            Step::Drop,
            "the deck started"
        );
        a.loaded = true;
        assert_eq!(
            a.step(&at(5_000_000), true, Some(&path)),
            Step::Wait,
            "it plays"
        );
        let done = downloading(&h, Transfer::Done(path.clone()));
        assert_eq!(
            a.step(&done, true, Some(&path)),
            Step::Complete(path.clone())
        );
        assert_eq!(
            a.step(&done, false, Some(Path::new("/other.mp3"))),
            Step::Drop,
            "the DJ loaded another track"
        );
        let failed = downloading(&h, Transfer::Failed("gone".into()));
        assert_eq!(a.step(&failed, false, Some(&path)), Step::Stopped);
        assert_eq!(
            a.step(&Snapshot::default(), false, Some(&path)),
            Step::Stopped,
            "cancelled"
        );
    }

    #[test]
    fn a_file_that_finished_before_it_could_load_loads_complete_unless_the_deck_plays() {
        let h = hit("a.flac");
        let dir = Path::new("/stick/SoulSeek Downloads");
        let a = Arrival::new(&h, dir);
        let done = downloading(&h, Transfer::Done(dir.join("a.flac")));
        assert_eq!(
            a.step(&done, false, None),
            Step::Complete(dir.join("a.flac"))
        );
        assert_eq!(a.step(&done, true, None), Step::Drop);
    }
}
