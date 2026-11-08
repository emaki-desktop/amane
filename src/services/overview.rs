#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use crate::Service;
use crate::compositor;

/// niri's overview open/close progress alone: 0.0 closed .. 1.0 fully open.
/// It's the same value as [`Gestures`](crate::Gestures)' `overview_progress`,
/// on its own so a view that follows the overview isn't drawn again every
/// time a workspace's view scrolls.
///
/// Reported only under the gesture-IPC niri fork; elsewhere it stays 0.
pub struct Overview {
    progress: f64,
}

impl Service for Overview {
    fn new() -> Self {
        Self { progress: 0.0 }
    }

    // on an unsupported compositor this returns at once and the value stays put
    fn listen() {
        compositor::listen_gestures(compositor::GestureValues::Overview, |gesture| {
            let mut overview = Self::write();

            // the stream also carries view and workspace movement, which
            // leaves the overview where it was
            if overview.progress == gesture.overview_progress {
                overview.quiet();

                return;
            }

            overview.progress = gesture.overview_progress;
        });
    }
}

impl Overview {
    /// 0.0 closed .. 1.0 fully open.
    pub fn progress(&self) -> f64 {
        self.progress
    }
}
