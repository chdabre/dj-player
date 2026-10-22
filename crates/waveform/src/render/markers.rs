//! The overlays both views draw: cue, memory cues, hot cues and the active loop.

use crate::display::Rgb;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HotCue {
    pub time: f64,
    /// A–H.
    pub label: char,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    pub start: f64,
    pub end: f64,
    /// Length in beats, shown in the zoom.
    pub beats: u32,
}

/// Cue, memory cue, hot cue and loop overlays.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Markers {
    pub cue: Option<f64>,
    pub memory_cues: Vec<f64>,
    pub hot_cues: Vec<HotCue>,
    pub active_loop: Option<Loop>,
}
