#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::HashMap;

use crate::Service;
use crate::changes::{self, Source};
use crate::timing;

/// How bright the compositor says the backdrop is behind each blur shape that
/// has an id (niri-blur-shapes version 2, for materials it reports for), from
/// 0 for black to 1 for white; empty on a compositor that doesn't say.
#[derive(Default)]
pub struct Backdrop {
    shapes: HashMap<u32, f32>,
}

// the readings arrive with the compositor's events, so nothing is polled
impl Service for Backdrop {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

impl Backdrop {
    /// The last reading for the shape with this id, none before the first;
    /// a later reading for it draws again only the windows that asked.
    pub fn luminance(&self, id: u32) -> Option<f32> {
        changes::note_source(Source::Backdrop(id));

        self.shapes.get(&id).copied()
    }
}

/*
 * how far a reading has to move from the one readers have before it counts:
 * niri reports up to 4 times a second per shape, and a video or a scrolling
 * page behind glass moves it a little each time, while what reads it only
 * turns at thresholds (with hysteresis); smaller moves would redraw the
 * window for nothing
 */
const STEP: f32 = 0.02;

/*
 * a reading from the compositor; one that didn't move by STEP is dropped and
 * redraws nothing, one that did redraws only the windows that read that
 * shape's
 */
pub(crate) fn report(id: u32, luminance: f32) {
    let luminance = luminance.clamp(0.0, 1.0);
    let mut backdrop = Backdrop::write();

    // not a change to every reader of the service
    backdrop.quiet();

    if moved(backdrop.shapes.get(&id).copied(), luminance) {
        backdrop.shapes.insert(id, luminance);
        drop(backdrop);

        if timing::enabled() {
            eprintln!("change Backdrop {id}");
        }

        changes::mark_source(Source::Backdrop(id));
    }
}

// a first reading, or one STEP or more from the last one kept
fn moved(before: Option<f32>, now: f32) -> bool {
    before.is_none_or(|before| (now - before).abs() >= STEP)
}

#[cfg(test)]
mod tests {
    use super::moved;

    #[test]
    fn small_moves_are_dropped() {
        assert!(moved(None, 0.5));
        assert!(!moved(Some(0.5), 0.51));
        assert!(moved(Some(0.5), 0.53));
        assert!(moved(Some(0.5), 0.47));
    }
}
