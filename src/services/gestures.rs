#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use crate::Service;
use crate::compositor;

/// Live, per-frame gesture values from niri: overview open/close progress, the
/// focused output's horizontal view offset (+ active column), and its vertical
/// (fractional) workspace-switch progress (+ active workspace). These track the
/// compositor's own animations and gestures, so a shell can animate in sync.
///
/// Reported only under the gesture-IPC niri fork; on any other compositor the
/// values stay at their defaults (overview `0.0`, everything else zero).
pub struct Gestures {
    overview_progress: f64,
    view_pos: f64,
    active_column_idx: usize,
    workspace_idx: f64,
    active_workspace_idx: usize,
    output: String,
}

impl Service for Gestures {
    fn new() -> Self {
        Self {
            overview_progress: 0.0,
            view_pos: 0.0,
            active_column_idx: 0,
            workspace_idx: 0.0,
            active_workspace_idx: 0,
            output: String::new(),
        }
    }

    // on an unsupported compositor this returns at once and the values stay put
    fn listen() {
        compositor::listen_gestures(compositor::GestureValues::Movement, |gesture| {
            let mut gestures = Self::write();

            // niri already coalesces, so a delivered snapshot is normally a
            // real change; skip the rare no-op so a resting bar never redraws
            if gestures.overview_progress == gesture.overview_progress
                && gestures.view_pos == gesture.view_pos
                && gestures.active_column_idx == gesture.active_column_idx
                && gestures.workspace_idx == gesture.workspace_idx
                && gestures.active_workspace_idx == gesture.active_workspace_idx
                && gestures.output == gesture.output
            {
                gestures.quiet();

                return;
            }

            gestures.overview_progress = gesture.overview_progress;
            gestures.view_pos = gesture.view_pos;
            gestures.active_column_idx = gesture.active_column_idx;
            gestures.workspace_idx = gesture.workspace_idx;
            gestures.active_workspace_idx = gesture.active_workspace_idx;
            gestures.output = gesture.output;
        });
    }
}

impl Gestures {
    /// Overview open/close progress: 0.0 closed .. 1.0 fully open.
    pub fn overview_progress(&self) -> f64 {
        self.overview_progress
    }

    /// Horizontal scroll position of the focused output, in logical pixels.
    pub fn view_pos(&self) -> f64 {
        self.view_pos
    }

    /// Index of the active column on the focused output.
    pub fn active_column_idx(&self) -> usize {
        self.active_column_idx
    }

    /// Current, possibly fractional, workspace position of the focused output.
    pub fn workspace_idx(&self) -> f64 {
        self.workspace_idx
    }

    /// Index of the active workspace on the focused output.
    pub fn active_workspace_idx(&self) -> usize {
        self.active_workspace_idx
    }

    /// Name of the focused output the values above belong to.
    pub fn output(&self) -> &str {
        &self.output
    }
}
