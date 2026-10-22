//! Tags and artwork, read from the files on a background thread and kept for the session.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

pub use engine_media::{Tags, tags as read_tags};

use super::{Entry, Kind};

/// Artwork scaled down for the jacket column: square RGBA8.
#[derive(Debug)]
pub struct Thumb {
    /// Unique per picture, for the renderer's texture cache.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Side of a [`Thumb`] for the list, pixels.
pub(crate) const THUMB_SIZE: u32 = 64;
/// Side of the selected track's artwork in the info panel, pixels.
pub const LARGE_SIZE: u32 = 288;

/// A file's tags (without the picture) and its artwork thumbnail; `tags` is `None` if the
/// file could not be read.
#[derive(Debug, Clone)]
pub(super) struct Read {
    pub(super) tags: Option<Tags>,
    pub(super) art: Option<Arc<Thumb>>,
}

/// Reads tags on its own thread: the wanted files in order, the newest wish first.
pub(super) struct TagReader {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<TagState>,
    wake: Condvar,
    /// Bumped whenever a file's tags arrive.
    generation: AtomicU64,
}

#[derive(Default)]
struct TagState {
    queue: VecDeque<PathBuf>,
    done: HashMap<PathBuf, Read>,
    /// The file whose large artwork is wanted, and the last one read.
    large_wanted: Option<PathBuf>,
    large: Option<(PathBuf, Option<Arc<Thumb>>)>,
    stop: bool,
}

impl TagReader {
    pub(super) fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let s = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tags".into())
            .spawn(move || tag_thread(&s))
            .ok();
        Self { shared, thread }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TagState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the tracks among `entries` next, before anything wanted earlier.
    pub(super) fn want(&self, entries: &[Entry]) {
        let mut s = self.lock();
        let fresh: Vec<PathBuf> = entries
            .iter()
            .filter(|e| e.kind == Kind::Track && !s.done.contains_key(&e.path))
            .map(|e| e.path.clone())
            .collect();
        let set: std::collections::HashSet<&PathBuf> = fresh.iter().collect();
        s.queue.retain(|p| !set.contains(p));
        for p in fresh.iter().rev() {
            s.queue.push_front(p.clone());
        }
        self.shared.wake.notify_one();
    }

    pub(super) fn get(&self, path: &Path) -> Option<Read> {
        self.lock().done.get(path).cloned()
    }

    pub(super) fn large(&self, path: &Path) -> Option<Option<Arc<Thumb>>> {
        let mut s = self.lock();
        if let Some((p, art)) = &s.large
            && p == path
        {
            return Some(art.clone());
        }
        if s.large_wanted.as_deref() != Some(path) {
            s.large_wanted = Some(path.to_path_buf());
            self.shared.wake.notify_one();
        }
        None
    }

    pub(super) fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
}

impl Drop for TagReader {
    fn drop(&mut self) {
        self.lock().stop = true;
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn tag_thread(shared: &Shared) {
    let lock = || shared.state.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        // The info panel's picture first: someone is looking at it.
        let (path, large) = {
            let mut s = lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some(p) = s.large_wanted.take() {
                    break (p, true);
                }
                if let Some(p) = s.queue.pop_front() {
                    break (p, false);
                }
                s = shared.wake.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let mut tags = engine_media::tags(&path).ok();
        let artwork = tags.as_mut().and_then(|t| t.artwork.take());
        if large {
            let art = artwork
                .and_then(|a| thumbnail(&a.data, LARGE_SIZE))
                .map(Arc::new);
            lock().large = Some((path, art));
            shared.generation.fetch_add(1, Ordering::Release);
            continue;
        }
        let art = artwork
            .and_then(|a| thumbnail(&a.data, THUMB_SIZE))
            .map(Arc::new);
        lock().done.insert(path, Read { tags, art });
        shared.generation.fetch_add(1, Ordering::Release);
    }
}

/// Decode a picture and scale it to fill a `size` square (centre-cropped).
pub fn thumbnail(data: &[u8], size: u32) -> Option<Thumb> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let img = image::load_from_memory(data).ok()?;
    let small = img
        .resize_to_fill(size, size, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Some(Thumb {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        width: small.width(),
        height: small.height(),
        rgba: small.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_becomes_a_square_thumbnail() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 30, 30, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let t = thumbnail(&png, THUMB_SIZE).unwrap();
        assert_eq!((t.width, t.height), (THUMB_SIZE, THUMB_SIZE));
        assert_eq!(&t.rgba[..4], &[200, 30, 30, 255]);
        assert!(thumbnail(b"not a picture", THUMB_SIZE).is_none());
    }
}
