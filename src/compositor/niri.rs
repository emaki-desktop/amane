use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use serde_json::{Value, json};

use super::Gesture;
use crate::{WindowInfo, Workspace};

pub mod client;
mod stream;

// the fork's additions: asking niri once, more of its events, and following them
mod events;
mod ipc;
mod listen;

use events::*;
pub(crate) use ipc::{hdr_panel, set_sdr_white};
pub(crate) use listen::gesture_wants;
pub use listen::{
    act, listen_casts, listen_gestures, listen_keyboard_layouts, listen_lock_keys,
    listen_outputs_changed, listen_windows,
};
use listen::{apply_cast, apply_gesture, apply_window};

// the niri events amane uses, everything else niri sends is skipped
#[derive(Clone)]
pub enum Event {
    // the full list, sent first and again whenever one is added or removed
    Workspaces(Vec<Workspace>),

    // focused is false when it only became the one shown on its monitor
    Activated {
        id: i64,
        focused: bool,
    },

    Urgent {
        id: i64,
        urgent: bool,
    },

    // every window, sent first and whenever the set changes
    Windows(Vec<WindowInfo>),

    // a window opened, or any of its fields changed (title, workspace, …)
    WindowChanged(WindowInfo),

    WindowClosed {
        id: u64,
    },

    // windows moved in the layout: each one's id and new column, if tiled
    WindowLayouts(Vec<(u64, Option<(usize, usize)>)>),

    // focus moved; id is None when no window is focused (e.g. a layer
    // surface took it). All other windows are no longer focused.
    WindowFocus {
        id: Option<u64>,
    },

    // niri's overview was opened or closed
    Overview {
        open: bool,
    },

    // overview open/close animation progress: 0.0 closed .. 1.0 fully open
    OverviewProgress {
        progress: f64,
    },

    // horizontal scroll position of the focused output's active workspace
    ViewOffset {
        output: String,
        view_pos: f64,
        active_column_idx: usize,
    },

    // vertical (fractional) workspace position of the focused output
    WorkspaceSwitch {
        output: String,
        workspace_idx: f64,
        active_workspace_idx: usize,
    },

    // nothing under an output's workspaces shows, or something does again (the niri fork)
    DesktopObscured {
        output: String,
        obscured: bool,
    },

    // the keyboard layouts, sent first and whenever the config changes them
    KeyboardLayouts {
        names: Vec<String>,
        current: u8,
    },

    // the layout in use changed
    KeyboardLayoutSwitched {
        current: u8,
    },

    // Caps Lock or Num Lock turned on or off (the niri fork)
    LockKeys {
        caps_lock: bool,
        num_lock: bool,
    },

    // the answer to Outputs changed; it carries nothing (the niri fork)
    OutputsChanged,

    // niri read its config, at the start and on each change; failed when it couldn't
    ConfigLoaded {
        failed: bool,
    },

    // every screencast, sent first and whenever the set is replaced
    Casts(Vec<client::Cast>),

    // a screencast started, or one changed
    CastChanged(client::Cast),

    CastStopped {
        stream_id: u64,
    },
}

// niri's events only say what changed, so the rest is remembered here
#[derive(Default)]
struct State {
    list: Vec<Workspace>,

    // every window by its id, and the workspace it is on
    windows: HashMap<u64, Option<i64>>,
}

/*
 * niri reads one json request per line, then answers on the same connection;
 * none when niri is not running, like under another compositor
 */
pub fn send(request: &str) -> Option<UnixStream> {
    let path = env::var("NIRI_SOCKET").ok()?;

    let mut stream = UnixStream::connect(path).ok()?;

    let line = format!("{request}\n");

    stream.write_all(line.as_bytes()).ok()?;

    Some(stream)
}

pub fn focus_workspace(id: i64) {
    let request = json!({
        "Action": {
            "FocusWorkspace": {
                "reference": { "Id": id }
            }
        }
    });

    let Some(stream) = send(&request.to_string()) else {
        return;
    };

    // waiting for the one-line answer keeps niri from seeing a connection that closed mid-request
    let mut reply = String::new();

    let _ = BufReader::new(stream).read_line(&mut reply);
}

// follows niri's events until niri exits, handing over the full list after each one that
// changed it: a title or focus change leaves the workspaces as they were and wakes nobody
pub fn listen(mut on_change: impl FnMut(Vec<Workspace>)) {
    let mut state = State::default();
    let mut last: Option<Vec<Workspace>> = None;

    // what moves a workspace, or a window between workspaces
    let wants: stream::Wants = |event| {
        matches!(
            event,
            Event::Workspaces(_)
                | Event::Activated { .. }
                | Event::Urgent { .. }
                | Event::Windows(_)
                | Event::WindowChanged(_)
                | Event::WindowClosed { .. }
        )
    };

    for event in stream::subscribe(wants) {
        state.apply(event);

        if last.as_ref() == Some(&state.list) {
            continue;
        }
        last = Some(state.list.clone());
        on_change(state.list.clone());
    }
}

fn parse(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line).ok()?;

    // every event is an object with one key, the event's name
    let (name, body) = value.as_object()?.iter().next()?;

    match name.as_str() {
        "WorkspacesChanged" => workspaces_changed(body),
        "WorkspaceActivated" => workspace_activated(body),
        "WorkspaceUrgencyChanged" => urgency_changed(body),
        "WindowsChanged" => windows_changed(body),
        "WindowOpenedOrChanged" => window_changed(body),
        "WindowClosed" => window_closed(body),
        "WindowLayoutsChanged" => window_layouts(body),
        "WindowFocusChanged" => window_focus(body),
        "OverviewOpenedOrClosed" => overview_changed(body),
        "OverviewProgress" => overview_progress(body),
        "ViewOffsetChanged" => view_offset(body),
        "WorkspaceSwitchProgress" => workspace_switch(body),
        "DesktopObscured" => desktop_obscured(body),
        "KeyboardLayoutsChanged" => keyboard_layouts(body),
        "KeyboardLayoutSwitched" => keyboard_switched(body),
        "KeyboardLockKeysChanged" => Some(Event::LockKeys {
            caps_lock: body["caps_lock"].as_bool()?,
            num_lock: body["num_lock"].as_bool()?,
        }),
        "OutputsChanged" => Some(Event::OutputsChanged),
        "ConfigLoaded" => Some(Event::ConfigLoaded {
            failed: body["failed"].as_bool()?,
        }),
        "CastsChanged" => Some(Event::Casts(
            body["casts"]
                .as_array()?
                .iter()
                .filter_map(client::parse_cast)
                .collect(),
        )),
        "CastStartedOrChanged" => Some(Event::CastChanged(client::parse_cast(&body["cast"])?)),
        "CastStopped" => Some(Event::CastStopped {
            stream_id: body["stream_id"].as_u64()?,
        }),
        _ => None,
    }
}

fn windows_changed(body: &Value) -> Option<Event> {
    let mut windows = Vec::new();

    for window in body["windows"].as_array()? {
        windows.push(parse_window(window)?);
    }

    Some(Event::Windows(windows))
}

fn window_changed(body: &Value) -> Option<Event> {
    Some(Event::WindowChanged(parse_window(&body["window"])?))
}

fn window_closed(body: &Value) -> Option<Event> {
    let id = body["id"].as_u64()?;

    Some(Event::WindowClosed { id })
}

fn workspaces_changed(body: &Value) -> Option<Event> {
    let mut list = Vec::new();

    for value in body["workspaces"].as_array()? {
        list.push(parse_workspace(value)?);
    }

    Some(Event::Workspaces(list))
}

fn workspace_activated(body: &Value) -> Option<Event> {
    let id = body["id"].as_i64()?;

    let focused = body["focused"].as_bool()?;

    Some(Event::Activated { id, focused })
}

fn urgency_changed(body: &Value) -> Option<Event> {
    let id = body["id"].as_i64()?;

    let urgent = body["urgent"].as_bool()?;

    Some(Event::Urgent { id, urgent })
}

fn parse_workspace(value: &Value) -> Option<Workspace> {
    let index = value["idx"].as_u64()?;

    let workspace = Workspace {
        id: value["id"].as_i64()?,
        index: index as u32,

        // null for workspaces the user never named
        name: value["name"].as_str().map(String::from),
        output: value["output"].as_str().map(String::from),

        active: value["is_active"].as_bool()?,
        focused: value["is_focused"].as_bool()?,
        urgent: value["is_urgent"].as_bool()?,

        // counted from the window events, after every event
        windows: 0,
    };

    Some(workspace)
}

impl State {
    fn apply(&mut self, event: Event) {
        match event {
            Event::Workspaces(list) => self.list = list,
            Event::Activated { id, focused } => self.activate(id, focused),
            Event::Urgent { id, urgent } => self.mark_urgent(id, urgent),

            Event::Windows(windows) => {
                self.windows = windows.into_iter().map(|w| (w.id, w.workspace)).collect();
            }

            Event::WindowChanged(window) => {
                self.windows.insert(window.id, window.workspace);
            }

            Event::WindowClosed { id } => {
                self.windows.remove(&id);
            }

            // neither focus, overview, nor the live gesture values move a
            // window between workspaces, so the counts this state tracks are
            // unaffected
            Event::WindowFocus { .. }
            | Event::Overview { .. }
            | Event::OverviewProgress { .. }
            | Event::ViewOffset { .. }
            | Event::WorkspaceSwitch { .. }
            | Event::DesktopObscured { .. }
            | Event::KeyboardLayouts { .. }
            | Event::KeyboardLayoutSwitched { .. }
            | Event::LockKeys { .. }
            | Event::OutputsChanged
            | Event::ConfigLoaded { .. }
            | Event::Casts(_)
            | Event::CastChanged(_)
            | Event::CastStopped { .. }
            | Event::WindowLayouts(_) => {}
        }

        self.count_windows();
    }

    fn activate(&mut self, id: i64, focused: bool) {
        let Some(activated) = self.list.iter().find(|workspace| workspace.id == id) else {
            return;
        };

        let output = activated.output.clone();

        // only one workspace is shown per monitor, and only one has focus overall
        for workspace in &mut self.list {
            if workspace.output == output {
                workspace.active = workspace.id == id;
            }

            if focused {
                workspace.focused = workspace.id == id;
            }
        }
    }

    fn mark_urgent(&mut self, id: i64, urgent: bool) {
        for workspace in &mut self.list {
            if workspace.id == id {
                workspace.urgent = urgent;
            }
        }
    }

    // a new workspace list starts at 0 windows each, so the counts are made again after any event
    fn count_windows(&mut self) {
        for workspace in &mut self.list {
            let mut count = 0;

            for on in self.windows.values() {
                if *on == Some(workspace.id) {
                    count += 1;
                }
            }

            workspace.windows = count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // lines copied from niri's event stream, trimmed to the fields amane reads plus a few it skips
    #[test]
    fn reads_niri_event_lines() {
        let line = r#"{"WorkspacesChanged":{"workspaces":[{"id":3,"idx":1,"name":null,"output":"eDP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":null}]}}"#;

        let Some(Event::Workspaces(list)) = parse(line) else {
            panic!("failed to parse workspaces");
        };

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 3);
        assert_eq!(list[0].index, 1);
        assert_eq!(list[0].name, None);
        assert!(list[0].focused);

        let line = r#"{"WindowOpenedOrChanged":{"window":{"id":7,"title":"foot","app_id":"foot","workspace_id":null,"is_focused":true,"is_floating":false}}}"#;

        let Some(Event::WindowChanged(window)) = parse(line) else {
            panic!("failed to parse window");
        };

        assert_eq!(window.id, 7);
        assert_eq!(window.title.as_deref(), Some("foot"));
        assert_eq!(window.app_id.as_deref(), Some("foot"));
        assert!(window.focused);
        assert!(!window.floating);
        assert_eq!(window.workspace, None);
        assert_eq!(window.column, None);

        let line = r#"{"WindowLayoutsChanged":{"changes":[[7,{"pos_in_scrolling_layout":[2,1],"tile_size":[800.0,600.0]}],[8,{"pos_in_scrolling_layout":null}]]}}"#;

        let Some(Event::WindowLayouts(changes)) = parse(line) else {
            panic!("failed to parse window layouts");
        };

        assert_eq!(changes, [(7, Some((2, 1))), (8, None)]);

        // focus changes arrive as their own event, with the newly focused id
        assert!(matches!(
            parse(r#"{"WindowFocusChanged":{"id":7}}"#),
            Some(Event::WindowFocus { id: Some(7) })
        ));

        // null id means no window is focused
        assert!(matches!(
            parse(r#"{"WindowFocusChanged":{"id":null}}"#),
            Some(Event::WindowFocus { id: None })
        ));

        // overview open/close drives the taskbar's expanded mode
        assert!(matches!(
            parse(r#"{"OverviewOpenedOrClosed":{"is_open":true}}"#),
            Some(Event::Overview { open: true })
        ));

        // the keyboard layouts, for the layout service
        assert!(matches!(
            parse(
                r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["US"],"current_idx":0}}}"#
            ),
            Some(Event::KeyboardLayouts { current: 0, .. })
        ));

        // a config read, for changes that only last until the next one
        assert!(matches!(
            parse(r#"{"ConfigLoaded":{"failed":true}}"#),
            Some(Event::ConfigLoaded { failed: true })
        ));

        // events amane doesn't use are skipped
        assert!(parse(r#"{"ConfigReloadFailed":{}}"#).is_none());
    }
}
