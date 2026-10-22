use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Stops a decode between packets: the loader thread ends with
/// [`LoadError::Cancelled`](crate::LoadError::Cancelled) and the source keeps what was decoded.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// The shared flag, for cancelling other work with the same token.
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sine, tmp, write_wav};
    use crate::{LoadError, decode_native_until, load_file};

    #[test]
    fn a_cancelled_decode_stops() {
        let path = tmp("cancel.wav");
        write_wav(&path, 48_000, 1, &sine(48_000, 440.0, 48_000 * 60));
        // Cancelled right away: the loader stops at its next packet (or has already finished).
        let loading = load_file(&path, 48_000).unwrap();
        loading.cancel.cancel();
        assert!(matches!(
            loading.thread.join().unwrap(),
            Err(LoadError::Cancelled) | Ok(())
        ));
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(
            decode_native_until(&path, &cancel),
            Err(LoadError::Cancelled)
        ));
    }
}
