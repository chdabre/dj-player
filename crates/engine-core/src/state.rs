//! State out of the callback: plain atomics written by the audio thread and read by anyone.
//!
//! Each field is individually consistent; a reader may see fields from different callbacks.
//! That is fine for meters and displays. A seqlock'd fast-state block replaces this once the
//! UI needs a coherent snapshot.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// An `f64` stored as its bit pattern in an `AtomicU64`.
#[derive(Debug, Default)]
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }

    #[inline]
    pub fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Fast state of one deck, written once per callback.
#[derive(Debug, Default)]
pub struct DeckState {
    pub loaded: AtomicBool,
    pub playing: AtomicBool,
    pub reverse: AtomicBool,
    pub slip_enabled: AtomicBool,
    pub slip_active: AtomicBool,
    /// Keylock is on.
    pub keylock_enabled: AtomicBool,
    /// The last block went through the time-stretcher. False with keylock on while the
    /// deck is paused, reversing or at a rate the stretcher does not play.
    pub stretching: AtomicBool,
    /// Audible playhead, frames at the engine rate.
    pub position: AtomicF64,
    /// Slip shadow playhead, frames at the engine rate.
    pub shadow_position: AtomicF64,
    pub rate: AtomicF64,
    /// Track length in frames (estimate while loading).
    pub length: AtomicF64,
    /// True if the last block hit a chunk that was not loaded yet.
    pub starved: AtomicBool,
    /// Number of blocks rendered as silence because of an unloaded chunk.
    pub starved_blocks: AtomicU64,
}

/// Callback load meter: time spent in the callback divided by the buffer duration.
///
/// The audio thread calls [`LoadMeter::record`]; a reader calls [`LoadMeter::take`] once per
/// reporting period, which resets the accumulators.
#[derive(Debug, Default)]
pub struct LoadMeter {
    busy_ns: AtomicU64,
    budget_ns: AtomicU64,
    peak_ppm: AtomicU64,
    callbacks: AtomicU64,
}

/// One reporting period of the [`LoadMeter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadReport {
    /// Total callback time ÷ total buffer time over the period (0.0..; 1.0 = 100 %).
    pub average: f64,
    /// Worst single callback in the period.
    pub peak: f64,
    pub callbacks: u64,
}

impl LoadMeter {
    /// Record one callback that took `busy_ns` to produce `budget_ns` worth of audio.
    /// Real-time safe.
    #[inline]
    pub fn record(&self, busy_ns: u64, budget_ns: u64) {
        self.busy_ns.fetch_add(busy_ns, Ordering::Relaxed);
        self.budget_ns.fetch_add(budget_ns, Ordering::Relaxed);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        if let Some(ppm) = busy_ns.saturating_mul(1_000_000).checked_div(budget_ns) {
            self.peak_ppm.fetch_max(ppm, Ordering::Relaxed);
        }
    }

    /// Read and reset the accumulated values.
    pub fn take(&self) -> LoadReport {
        let busy = self.busy_ns.swap(0, Ordering::Relaxed);
        let budget = self.budget_ns.swap(0, Ordering::Relaxed);
        let peak = self.peak_ppm.swap(0, Ordering::Relaxed);
        let callbacks = self.callbacks.swap(0, Ordering::Relaxed);
        LoadReport {
            average: if budget > 0 {
                busy as f64 / budget as f64
            } else {
                0.0
            },
            peak: peak as f64 / 1e6,
            callbacks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_meter_averages_and_resets() {
        let m = LoadMeter::default();
        m.record(1_000, 10_000);
        m.record(3_000, 10_000);
        let r = m.take();
        assert_eq!(r.callbacks, 2);
        assert!((r.average - 0.2).abs() < 1e-12);
        assert!((r.peak - 0.3).abs() < 1e-12);
        assert_eq!(m.take().callbacks, 0);
    }

    #[test]
    fn atomic_f64_round_trips() {
        let a = AtomicF64::new(-1.25);
        assert_eq!(a.load(), -1.25);
        a.store(f64::MAX);
        assert_eq!(a.load(), f64::MAX);
    }
}
