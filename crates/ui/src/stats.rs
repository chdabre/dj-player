//! Frame timing: intervals between frames (what vsync and missed frames look like), time
//! blocked acquiring the surface texture, and CPU time per frame.

use std::time::{Duration, Instant};

use crate::render::FrameTiming;

#[derive(Debug, Default)]
pub struct FrameStats {
    last: Option<Instant>,
    since: Option<Instant>,
    /// Milliseconds.
    intervals: Vec<f64>,
    acquire: Vec<f64>,
    cpu: Vec<f64>,
    build: Vec<f64>,
    /// Per-frame playhead step error against the frame interval at the playing speed (ms of
    /// audio): how far the shown playhead is from moving perfectly smoothly.
    playhead: Vec<f64>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

impl FrameStats {
    /// One presented frame that started at `now`; `build` is the screen layout time.
    pub fn record(&mut self, now: Instant, build: Duration, timing: FrameTiming) {
        if let Some(last) = self.last {
            self.intervals.push(ms(now - last));
        }
        self.last = Some(now);
        self.since.get_or_insert(now);
        self.acquire.push(ms(timing.acquire));
        self.cpu.push(ms(timing.cpu) + ms(build));
        self.build.push(ms(build));
    }

    pub fn record_playhead(&mut self, step_error_ms: f64) {
        self.playhead.push(step_error_ms.abs());
    }

    /// A one-line summary every `period`, resetting the window.
    pub fn take_if_due(&mut self, now: Instant, period: Duration) -> Option<String> {
        let since = self.since?;
        if now - since < period || self.intervals.is_empty() {
            return None;
        }
        let mut iv = std::mem::take(&mut self.intervals);
        iv.sort_by(f64::total_cmp);
        let p50 = percentile(&iv, 0.5);
        let late = iv.iter().filter(|&&x| x > 1.5 * p50).count();
        let fps = iv.len() as f64 / (now - since).as_secs_f64();
        let cpu_max = self.cpu.iter().copied().fold(0.0, f64::max);
        let line = format!(
            "{fps:.1} fps · frame p50 {p50:.1} p99 {:.1} max {:.1} ms · late {late} · \
             acquire {:.1} · cpu {:.2} (max {cpu_max:.2}) · layout {:.3} ms · \
             playhead step error p99 {:.2} ms",
            percentile(&iv, 0.99),
            iv.last().copied().unwrap_or(0.0),
            mean(&self.acquire),
            mean(&self.cpu),
            mean(&self.build),
            {
                let mut p = std::mem::take(&mut self.playhead);
                p.sort_by(f64::total_cmp);
                percentile(&p, 0.99)
            },
        );
        self.acquire.clear();
        self.cpu.clear();
        self.build.clear();
        self.since = Some(now);
        Some(line)
    }
}
