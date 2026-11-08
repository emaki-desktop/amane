#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! One connection to niri's event stream for the whole program. Each listener gets the events
//! it asked for through a channel of its own, instead of opening its own connection and parsing
//! every line again. An event no listener wants wakes none of them: a terminal's title spinner
//! wakes the windows listener alone, and a swipe's progress only the gesture listeners. A listener that starts late first gets the state so far, as niri sends a
//! new connection: the workspaces, the windows, the overview and the gesture values, each once
//! niri has sent it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;

use super::{Event, Gesture, State, apply_gesture, apply_window, parse, send};
use crate::WindowInfo;

static HUB: Mutex<Hub> = Mutex::new(Hub {
    started: false,
    ended: false,
    listeners: Vec::new(),
    mirror: None,
});

struct Hub {
    started: bool,

    // niri closed the stream, or wasn't there to open it
    ended: bool,

    listeners: Vec<Listener>,

    mirror: Option<Mirror>,
}

/// Which events a listener is sent.
pub(super) type Wants = fn(&Event) -> bool;

struct Listener {
    sender: Sender<Event>,
    wants: Wants,
}

/// The events of niri's stream that `wants` takes, from now on, after the state so far. The
/// channel closes when niri closes the stream, or at once when niri isn't running.
pub(super) fn subscribe(wants: Wants) -> Receiver<Event> {
    let (sender, receiver) = mpsc::channel();
    let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);

    if !hub.started {
        hub.started = true;
        match send("\"EventStream\"") {
            Some(stream) => {
                let started = thread::Builder::new()
                    .name(String::from("amane-niri-events"))
                    .spawn(move || read(stream));
                if started.is_err() {
                    hub.ended = true;
                }
            }
            None => hub.ended = true,
        }
    }

    if hub.ended {
        return receiver;
    }

    for event in hub.mirror.get_or_insert_with(Mirror::default).replay() {
        if wants(&event) {
            let _ = sender.send(event);
        }
    }
    hub.listeners.push(Listener { sender, wants });

    receiver
}

fn read(stream: UnixStream) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            break;
        };
        let Some(event) = parse(&line) else {
            continue;
        };

        let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);
        hub.mirror.get_or_insert_with(Mirror::default).apply(&event);
        // a listener that stopped listening drops out at the next event it wants
        hub.listeners.retain(|listener| {
            !(listener.wants)(&event) || listener.sender.send(event.clone()).is_ok()
        });
    }

    let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);
    hub.ended = true;
    hub.listeners.clear();
}

/// What niri has said so far, for a listener that starts late.
#[derive(Default)]
struct Mirror {
    workspaces: Option<State>,
    windows: Option<HashMap<u64, WindowInfo>>,
    overview: Option<bool>,
    gesture: Gesture,
    progress_seen: bool,
    view_seen: bool,
    switch_seen: bool,
    keyboard: Option<(Vec<String>, u8)>,
    lock_keys: Option<(bool, bool)>,
    casts: Option<Vec<super::client::Cast>>,
}

impl Mirror {
    fn apply(&mut self, event: &Event) {
        match event {
            Event::Workspaces(_) => {
                self.workspaces.get_or_insert_with(State::default);
            }
            Event::Windows(_) => {
                self.windows.get_or_insert_with(HashMap::new);
            }
            Event::Overview { .. } => {
                self.overview.get_or_insert(false);
            }
            Event::OverviewProgress { .. } => self.progress_seen = true,
            Event::ViewOffset { .. } => self.view_seen = true,
            Event::WorkspaceSwitch { .. } => self.switch_seen = true,
            Event::KeyboardLayouts { names, current } => {
                self.keyboard = Some((names.clone(), *current));
            }
            Event::KeyboardLayoutSwitched { current } => {
                if let Some((_, idx)) = &mut self.keyboard {
                    *idx = *current;
                }
            }
            Event::LockKeys {
                caps_lock,
                num_lock,
            } => self.lock_keys = Some((*caps_lock, *num_lock)),
            Event::Casts(_) | Event::CastChanged(_) | Event::CastStopped { .. } => {
                let casts = self.casts.get_or_insert_with(Vec::new);
                super::apply_cast(casts, event.clone());
            }
            _ => {}
        }

        if let Some(state) = &mut self.workspaces
            && !matches!(event, Event::WindowLayouts(_))
        {
            state.apply(event.clone());
        }
        if let Some(windows) = &mut self.windows {
            let mut overview = self.overview.unwrap_or(false);
            apply_window(windows, &mut overview, event.clone());
            if self.overview.is_some() {
                self.overview = Some(overview);
            }
        } else if let Event::Overview { open } = event {
            self.overview = Some(*open);
        }
        apply_gesture(&mut self.gesture, event.clone());
    }

    /// The events that bring a new listener to the same state.
    fn replay(&self) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(state) = &self.workspaces {
            events.push(Event::Workspaces(state.list.clone()));
        }
        if let Some(windows) = &self.windows {
            let mut list: Vec<WindowInfo> = windows.values().cloned().collect();
            list.sort_by_key(|window| window.id);
            events.push(Event::Windows(list));
        }
        if let Some(open) = self.overview {
            events.push(Event::Overview { open });
        }
        let gesture = &self.gesture;
        if self.progress_seen {
            events.push(Event::OverviewProgress {
                progress: gesture.overview_progress,
            });
        }
        if self.view_seen {
            events.push(Event::ViewOffset {
                output: gesture.output.clone(),
                view_pos: gesture.view_pos,
                active_column_idx: gesture.active_column_idx,
            });
        }
        if self.switch_seen {
            events.push(Event::WorkspaceSwitch {
                output: gesture.output.clone(),
                workspace_idx: gesture.workspace_idx,
                active_workspace_idx: gesture.active_workspace_idx,
            });
        }
        let mut obscured: Vec<_> = gesture.obscured.iter().collect();
        obscured.sort();
        for (output, obscured) in obscured {
            events.push(Event::DesktopObscured {
                output: output.clone(),
                obscured: *obscured,
            });
        }
        if let Some((names, current)) = &self.keyboard {
            events.push(Event::KeyboardLayouts {
                names: names.clone(),
                current: *current,
            });
        }
        if let Some((caps_lock, num_lock)) = self.lock_keys {
            events.push(Event::LockKeys {
                caps_lock,
                num_lock,
            });
        }
        if let Some(casts) = &self.casts {
            events.push(Event::Casts(casts.clone()));
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror(lines: &[&str]) -> Mirror {
        let mut mirror = Mirror::default();
        for line in lines {
            let event = parse(line).expect("a line amane reads");
            mirror.apply(&event);
        }
        mirror
    }

    fn names(events: &[Event]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                Event::Workspaces(_) => "Workspaces",
                Event::Windows(_) => "Windows",
                Event::Overview { .. } => "Overview",
                Event::OverviewProgress { .. } => "OverviewProgress",
                Event::ViewOffset { .. } => "ViewOffset",
                Event::WorkspaceSwitch { .. } => "WorkspaceSwitch",
                Event::DesktopObscured { .. } => "DesktopObscured",
                Event::KeyboardLayouts { .. } => "KeyboardLayouts",
                Event::LockKeys { .. } => "LockKeys",
                Event::Casts(_) => "Casts",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_late_listener_gets_the_state_so_far() {
        let mirror = mirror(&[
            r#"{"WorkspacesChanged":{"workspaces":[{"id":3,"idx":1,"name":null,"output":"eDP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":null}]}}"#,
            r#"{"WindowsChanged":{"windows":[]}}"#,
            r#"{"WindowOpenedOrChanged":{"window":{"id":7,"title":"foot","app_id":"foot","workspace_id":3,"is_focused":true,"is_floating":false}}}"#,
            r#"{"OverviewOpenedOrClosed":{"is_open":true}}"#,
            r#"{"DesktopObscured":{"output":"eDP-1","obscured":true}}"#,
            r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["English (US)","Norwegian"],"current_idx":0}}}"#,
            r#"{"KeyboardLayoutSwitched":{"idx":1}}"#,
        ]);
        let replay = mirror.replay();
        assert_eq!(
            names(&replay),
            [
                "Workspaces",
                "Windows",
                "Overview",
                "DesktopObscured",
                "KeyboardLayouts"
            ]
        );

        let Event::Windows(windows) = &replay[1] else {
            panic!("windows second");
        };
        assert_eq!(windows.len(), 1);
        assert!(matches!(replay[2], Event::Overview { open: true }));
        assert!(matches!(
            replay[4],
            Event::KeyboardLayouts { current: 1, .. }
        ));

        // the window counted on its workspace, as a listener from the start would have it
        let Event::Workspaces(list) = &replay[0] else {
            panic!("workspaces first");
        };
        assert_eq!(list[0].windows, 1);
    }

    #[test]
    fn lock_keys_and_casts_are_replayed() {
        let mirror = mirror(&[
            r#"{"KeyboardLockKeysChanged":{"caps_lock":true,"num_lock":false}}"#,
            r#"{"OutputsChanged":{}}"#,
            r#"{"CastsChanged":{"casts":[]}}"#,
            r#"{"CastStartedOrChanged":{"cast":{"stream_id":2,"session_id":1,"kind":"PipeWire","target":{"Output":{"name":"eDP-1"}},"is_dynamic_target":false,"is_active":true,"pid":99,"pw_node_id":55}}}"#,
            r#"{"CastStartedOrChanged":{"cast":{"stream_id":3,"session_id":1,"kind":"PipeWire","target":{"Nothing":{}},"is_dynamic_target":true,"is_active":false,"pid":null,"pw_node_id":56}}}"#,
            r#"{"CastStopped":{"stream_id":2}}"#,
        ]);
        let replay = mirror.replay();
        assert_eq!(names(&replay), ["LockKeys", "Casts"]);
        assert!(matches!(
            replay[0],
            Event::LockKeys {
                caps_lock: true,
                num_lock: false
            }
        ));
        let Event::Casts(casts) = &replay[1] else {
            panic!("casts second");
        };
        assert_eq!(casts.len(), 1);
        assert_eq!(casts[0].stream_id, 3);
    }

    #[test]
    fn nothing_is_replayed_before_niri_said_it() {
        assert!(Mirror::default().replay().is_empty());
        let mirror = mirror(&[r#"{"OverviewProgress":{"progress":0.5}}"#]);
        assert_eq!(names(&mirror.replay()), ["OverviewProgress"]);
    }
}
