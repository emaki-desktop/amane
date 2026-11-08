mod hyprland;
mod mango;
pub(crate) mod niri;
mod sway;

use std::collections::HashMap;
use std::env;

use crate::{WindowInfo, Workspace};

enum Compositor {
    Niri,
    Hyprland,
    Sway,
    Mango,
}

/// A control request sent to the running compositor. niri implements every
/// variant; the other backends implement what maps cleanly and ignore the
/// rest (see each backend's `act`).
pub(crate) enum Action {
    FocusWindow(u64),
    CloseWindow(u64),
    FullscreenWindow(u64),
    ToggleFloating(u64),
    // focus: whether the focus follows the window there
    MoveWindowToWorkspace {
        window: u64,
        workspace: i64,
        focus: bool,
    },
    // the window's column to `index` on its workspace, from 1, the focus staying put
    MoveColumnToIndex {
        window: u64,
        index: usize,
    },
    FocusColumnLeft,
    FocusColumnRight,
    Spawn(Vec<String>),
    Screenshot,
    ToggleOverview,
}

/// Live, per-frame gesture values from the compositor: overview open/close
/// progress, the focused output's horizontal view offset, and its vertical
/// (fractional) workspace-switch progress. Only niri reports these (via the
/// gesture-IPC fork); other backends never call `on_change`, so the snapshot
/// stays at its default.
#[derive(Debug, Clone, Default)]
pub(crate) struct Gesture {
    /// Overview progress: 0.0 closed .. 1.0 fully open.
    pub overview_progress: f64,
    /// Horizontal scroll position of the focused output, in logical pixels.
    pub view_pos: f64,
    /// Index of the active column on the focused output.
    pub active_column_idx: usize,
    /// Current, possibly fractional, workspace position of the focused output.
    pub workspace_idx: f64,
    /// Index of the active workspace on the focused output.
    pub active_workspace_idx: usize,
    /// Name of the focused output the values above belong to.
    pub output: String,
    /// Each output's desktop obscured: nothing under the workspaces shows there.
    pub obscured: HashMap<String, bool>,
}

// each compositor sets its own variable for the programs it starts
fn running() -> Option<Compositor> {
    if env::var_os("NIRI_SOCKET").is_some() {
        return Some(Compositor::Niri);
    }

    if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return Some(Compositor::Hyprland);
    }

    if env::var_os("SWAYSOCK").is_some() {
        return Some(Compositor::Sway);
    }

    if env::var_os("MANGO_INSTANCE_SIGNATURE").is_some() {
        return Some(Compositor::Mango);
    }

    None
}

/*
 * hands over the full workspace list after every compositor event,
 * until the compositor exits; returns right away on an unsupported one
 */
pub fn listen(on_change: impl FnMut(Vec<Workspace>)) {
    let Some(compositor) = running() else {
        return;
    };

    match compositor {
        Compositor::Niri => niri::listen(on_change),
        Compositor::Hyprland => hyprland::listen(on_change),
        Compositor::Sway => sway::listen(on_change),
        Compositor::Mango => mango::listen(on_change),
    }
}

pub fn focus_workspace(id: i64) {
    let Some(compositor) = running() else {
        return;
    };

    match compositor {
        Compositor::Niri => niri::focus_workspace(id),
        Compositor::Hyprland => hyprland::focus_workspace(id),
        Compositor::Sway => sway::focus_workspace(id),
        Compositor::Mango => mango::focus_workspace(id),
    }
}

/*
 * hands over the full window list after every compositor event, like
 * `listen` but for windows; returns right away on an unsupported one
 */
pub(crate) fn listen_windows(on_change: impl FnMut(Vec<WindowInfo>, bool)) {
    let Some(compositor) = running() else {
        return;
    };

    match compositor {
        Compositor::Niri => niri::listen_windows(on_change),
        Compositor::Hyprland => hyprland::listen_windows(on_change),
        Compositor::Sway => sway::listen_windows(on_change),
        Compositor::Mango => mango::listen_windows(on_change),
    }
}

/*
 * hands over the live gesture snapshot after every gesture event, like
 * `listen_windows` but for overview/view/workspace progress. Only niri
 * reports these; on any other compositor this returns right away, so the
 * Gestures service simply stays at its default.
 */
/// Hands over the keyboard layouts and the index in use whenever either changes; returns
/// right away on a compositor other than niri.
pub(crate) fn listen_keyboard_layouts(on_change: impl FnMut(Vec<String>, u8)) {
    if let Some(Compositor::Niri) = running() {
        niri::listen_keyboard_layouts(on_change);
    }
}

/// Caps Lock and Num Lock whenever either changes; returns right away unless this is the niri
/// fork with `lock-keys`.
pub(crate) fn listen_lock_keys(on_change: impl FnMut(bool, bool)) {
    if let Some(Compositor::Niri) = running()
        && niri::client::features().has("lock-keys")
    {
        niri::listen_lock_keys(on_change);
    }
}

/// Calls back whenever the outputs changed; returns right away unless this is the niri fork with
/// `outputs-changed`.
pub(crate) fn listen_outputs_changed(on_change: impl FnMut()) {
    if let Some(Compositor::Niri) = running()
        && niri::client::features().has("outputs-changed")
    {
        niri::listen_outputs_changed(on_change);
    }
}

/// Every running screencast whenever the set changes; returns right away on a compositor other
/// than niri.
pub(crate) fn listen_casts(on_change: impl FnMut(Vec<niri::client::Cast>)) {
    if let Some(Compositor::Niri) = running() {
        niri::listen_casts(on_change);
    }
}

/// Which of the gesture values a listener follows; it is woken only for those.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GestureValues {
    /// The overview's progress, the view offset and the workspace switch.
    Movement,
    /// The overview's progress alone.
    Overview,
    /// The workspace switch alone.
    WorkspaceSwitch,
    /// Whether each output's desktop is obscured.
    Obscured,
}

pub(crate) fn listen_gestures(values: GestureValues, on_change: impl FnMut(Gesture)) {
    let Some(compositor) = running() else {
        return;
    };

    match compositor {
        Compositor::Niri => niri::listen_gestures(niri::gesture_wants(values), on_change),
        Compositor::Hyprland | Compositor::Sway | Compositor::Mango => {}
    }
}

/// The built-in panel while the compositor drives it in HDR. Only the niri
/// fork reports HDR; elsewhere there's none.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrPanel {
    /// Its connector, such as `eDP-1`.
    pub output: String,
    /// How bright SDR white (a white window) is, in nits.
    pub sdr_white: f64,
    /// The brightest the panel goes, in nits, when its EDID says.
    pub peak: Option<f64>,
}

pub(crate) fn hdr_panel() -> Option<HdrPanel> {
    match running()? {
        Compositor::Niri => niri::hdr_panel(),
        Compositor::Hyprland | Compositor::Sway | Compositor::Mango => None,
    }
}

pub(crate) fn set_sdr_white(output: &str, nits: f64) -> bool {
    match running() {
        Some(Compositor::Niri) => niri::set_sdr_white(output, nits),
        _ => false,
    }
}

pub(crate) fn act(action: Action) {
    let Some(compositor) = running() else {
        return;
    };

    match compositor {
        Compositor::Niri => niri::act(action),
        Compositor::Hyprland => hyprland::act(action),
        Compositor::Sway => sway::act(action),
        Compositor::Mango => mango::act(action),
    }
}
