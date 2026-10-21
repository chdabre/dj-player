//! Hybrid logical clock: wall-clock milliseconds (48 bits) and a counter (16 bits).
//!
//! Each device's clock never goes backwards. It ticks past the wall clock when that is behind
//! what the device has already seen, so a player with no clock source (no RTC battery, no
//! network) still orders its edits after everything in the library it mounted.

use std::fmt;
use std::str::FromStr;

use crate::ids::{BadId, DeviceId};

/// One HLC value: `ms << 16 | counter`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hlc(pub u64);

const MS_MAX: u64 = (1 << 48) - 1;

/// 2200-01-01T00:00:00Z in ms: no correct device stamps anything later. An HLC beyond it is
/// corrupt or hostile (the 48-bit field reaches the year 10889): merging drops the operation
/// and clocks never observe it, so one bad line can't pin every device's clock at the top.
pub const MAX_PLAUSIBLE_MS: u64 = 7_258_118_400_000;

impl Hlc {
    pub const ZERO: Hlc = Hlc(0);

    pub fn new(ms: u64, counter: u16) -> Self {
        Hlc((ms.min(MS_MAX) << 16) | u64::from(counter))
    }

    pub fn ms(self) -> u64 {
        self.0 >> 16
    }

    pub fn counter(self) -> u16 {
        (self.0 & 0xffff) as u16
    }

    /// The next value after this one.
    pub fn next(self) -> Hlc {
        Hlc(self.0.saturating_add(1))
    }

    /// At or before [`MAX_PLAUSIBLE_MS`].
    pub fn is_plausible(self) -> bool {
        self.ms() <= MAX_PLAUSIBLE_MS
    }
}

/// Written as `<12 hex ms>-<4 hex counter>`, e.g. `0192f3c4a1b2-0003`.
impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:012x}-{:04x}", self.ms(), self.counter())
    }
}

impl fmt::Debug for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hlc({self})")
    }
}

impl FromStr for Hlc {
    type Err = BadId;
    fn from_str(s: &str) -> Result<Self, BadId> {
        let bad = || BadId(s.into());
        let (ms, c) = s.split_once('-').ok_or_else(bad)?;
        if ms.len() != 12 || c.len() != 4 {
            return Err(bad());
        }
        let ms = u64::from_str_radix(ms, 16).map_err(|_| bad())?;
        let c = u16::from_str_radix(c, 16).map_err(|_| bad())?;
        Ok(Hlc::new(ms, c))
    }
}

/// When and by whom an operation was made: the total order every merge uses. Ties in the
/// HLC are broken by the author device id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub hlc: Hlc,
    pub device: DeviceId,
}

/// A device's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    last: Hlc,
}

impl Clock {
    /// The last value handed out or observed.
    pub fn last(&self) -> Hlc {
        self.last
    }

    /// Never hand out anything at or below `seen` (an HLC read from the library or a peer).
    /// An implausible HLC (see [`MAX_PLAUSIBLE_MS`]) is ignored.
    pub fn observe(&mut self, seen: Hlc) {
        if seen.is_plausible() {
            self.last = self.last.max(seen);
        }
    }

    /// A new value for an edit made now (`wall_ms`, milliseconds since the Unix epoch, or 0
    /// without a clock), strictly greater than every earlier one. A wall clock set beyond
    /// [`MAX_PLAUSIBLE_MS`] counts as no clock: clamping it to the ceiling instead would pin
    /// every device that mounts the library there, and once the counter carried past the
    /// ceiling their edits would be dropped as implausible.
    pub fn tick(&mut self, wall_ms: u64) -> Hlc {
        let wall_ms = if wall_ms > MAX_PLAUSIBLE_MS {
            0
        } else {
            wall_ms
        };
        let wall = Hlc::new(wall_ms, 0);
        self.last = if wall > self.last {
            wall
        } else {
            // Counter overflow carries into the milliseconds: still strictly increasing.
            self.last.next()
        };
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        let h = Hlc::new(0x0192_f3c4_a1b2, 3);
        assert_eq!(h.to_string(), "0192f3c4a1b2-0003");
        assert_eq!("0192f3c4a1b2-0003".parse::<Hlc>(), Ok(h));
        assert!("0192f3c4a1b2".parse::<Hlc>().is_err());
    }

    #[test]
    fn clock_follows_the_wall_and_never_goes_back() {
        let mut c = Clock::default();
        assert_eq!(c.tick(1000), Hlc::new(1000, 0));
        assert_eq!(c.tick(1000), Hlc::new(1000, 1));
        // The wall clock went backwards: keep counting from the last value.
        assert_eq!(c.tick(500), Hlc::new(1000, 2));
        // The library holds something from the future: start above it.
        c.observe(Hlc::new(9000, 7));
        assert_eq!(c.tick(1001), Hlc::new(9000, 8));
        assert_eq!(c.tick(9001), Hlc::new(9001, 0));
    }

    #[test]
    fn implausible_hlcs_are_not_observed() {
        let mut c = Clock::default();
        c.observe(Hlc::new(1000, 0));
        // A corrupt or hostile line stamped at the top of the range.
        c.observe(Hlc(u64::MAX));
        c.observe(Hlc::new(MAX_PLAUSIBLE_MS + 1, 0));
        assert_eq!(c.tick(500), Hlc::new(1000, 1));
        assert!(Hlc::new(MAX_PLAUSIBLE_MS, 0xffff).is_plausible());
        // A wall clock set to the year 9000 counts as none: the clock keeps counting from
        // what it has seen, far from the ceiling.
        assert_eq!(c.tick(u64::MAX), Hlc::new(1000, 2));
        assert_eq!(c.tick(MAX_PLAUSIBLE_MS + 1), Hlc::new(1000, 3));
        // Right at the ceiling is still a (very late) clock.
        assert_eq!(c.tick(MAX_PLAUSIBLE_MS), Hlc::new(MAX_PLAUSIBLE_MS, 0));
    }

    #[test]
    fn counter_overflow_carries() {
        let mut c = Clock::default();
        c.observe(Hlc::new(5, u16::MAX));
        assert_eq!(c.tick(0), Hlc::new(6, 0));
    }
}
