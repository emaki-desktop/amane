#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::HashMap;

use crate::Service;
use crate::compositor;

/// Whether each output's desktop is obscured: nothing under its workspaces
/// shows, as with the overview fully open or windows covering the screen. A
/// view drawn on the desktop (a background layer niri's `desktop` rule puts
/// there) can freeze meanwhile, see
/// [`LayerWindow::frozen`](crate::LayerWindow::frozen).
///
/// Reported only under the niri fork; elsewhere no output is ever obscured.
pub struct Desktop {
    obscured: HashMap<String, bool>,
}

impl Service for Desktop {
    fn new() -> Self {
        Self {
            obscured: HashMap::new(),
        }
    }

    // on an unsupported compositor this returns at once and nothing is obscured
    fn listen() {
        compositor::listen_gestures(compositor::GestureValues::Obscured, |gesture| {
            let mut desktop = Self::write();

            // the stream also carries the gestures, which leave the desktop as it was
            if desktop.obscured == gesture.obscured {
                desktop.quiet();

                return;
            }

            desktop.obscured = gesture.obscured;
        });
    }
}

impl Desktop {
    /// Whether nothing under the workspaces of the output by this name shows.
    pub fn obscured(&self, output: &str) -> bool {
        self.obscured.get(output).copied().unwrap_or(false)
    }
}
