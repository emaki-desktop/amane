#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use crate::Service;
use crate::compositor;

/// The focused output's workspace switch alone: its fractional workspace
/// position and the active workspace. It's the same as
/// [`Gestures`](crate::Gestures)' values, on its own so a view that follows a
/// switch isn't drawn again every time a workspace's view scrolls.
///
/// Reported only under the gesture-IPC niri fork; elsewhere it stays at 0.
pub struct WorkspaceSwitch {
    output: String,
    workspace_idx: f64,
    active_workspace_idx: usize,
}

impl Service for WorkspaceSwitch {
    fn new() -> Self {
        Self {
            output: String::new(),
            workspace_idx: 0.0,
            active_workspace_idx: 0,
        }
    }

    // on an unsupported compositor this returns at once and the values stay put
    fn listen() {
        compositor::listen_gestures(compositor::GestureValues::WorkspaceSwitch, |gesture| {
            let mut switch = Self::write();

            // the stream also carries the overview and view movement, which leave
            // the switch where it was
            if switch.output == gesture.output
                && switch.workspace_idx == gesture.workspace_idx
                && switch.active_workspace_idx == gesture.active_workspace_idx
            {
                switch.quiet();

                return;
            }

            switch.output = gesture.output;
            switch.workspace_idx = gesture.workspace_idx;
            switch.active_workspace_idx = gesture.active_workspace_idx;
        });
    }
}

impl WorkspaceSwitch {
    /// Name of the focused output the values below belong to.
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Current, possibly fractional, workspace position, from 0 at the top.
    pub fn workspace_idx(&self) -> f64 {
        self.workspace_idx
    }

    /// Index of the active workspace, from 0 at the top.
    pub fn active_workspace_idx(&self) -> usize {
        self.active_workspace_idx
    }
}
