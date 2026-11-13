#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! Asking niri directly: its outputs, windows, screencasts, and which of the niri fork's
//! features it has. Each call opens a connection, sends one request and waits up to two
//! seconds for the answer, so call them from a service thread, never from a view. Under
//! another compositor they answer [`NiriError::NotRunning`].
//!
//! The live state (workspaces, windows, the overview, gestures, keyboard layouts) comes from
//! the services instead, which follow niri's event stream.

use std::fmt;
use std::io::{BufRead, BufReader};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{Value, json};

use super::send;

/// Why niri didn't answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NiriError {
    /// `NIRI_SOCKET` isn't set, or nothing listens on it.
    NotRunning,
    /// The connection failed, or no answer came in time.
    Io(String),
    /// niri answered with an error, such as stock niri asked for a fork feature.
    Refused(String),
    /// The answer wasn't what amane reads.
    Unexpected(String),
}

impl fmt::Display for NiriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "niri isn't running"),
            Self::Io(err) => write!(f, "no answer from niri: {err}"),
            Self::Refused(err) => write!(f, "niri refused: {err}"),
            Self::Unexpected(what) => write!(f, "unexpected answer from niri: {what}"),
        }
    }
}

impl std::error::Error for NiriError {}

/// Sends one request and reads its `Ok` answer under `name`.
fn ask(request: &Value, name: &str) -> Result<Value, NiriError> {
    let stream = send(&request.to_string()).ok_or(NiriError::NotRunning)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| NiriError::Io(err.to_string()))?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|err| NiriError::Io(err.to_string()))?;
    let mut reply: Value =
        serde_json::from_str(&line).map_err(|err| NiriError::Unexpected(err.to_string()))?;

    if let Some(err) = reply.get("Err") {
        return Err(NiriError::Refused(
            err.as_str().unwrap_or("an error").to_owned(),
        ));
    }
    reply
        .get_mut("Ok")
        .and_then(|ok| ok.get_mut(name))
        .map(Value::take)
        .ok_or_else(|| NiriError::Unexpected(format!("no {name}")))
}

/// The niri fork's features this niri has, with each one's contract version. Empty on stock
/// niri, which can't parse the request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Features {
    list: Vec<(String, u32)>,
}

impl Features {
    /// The version of a feature, like `features().version("blur-shapes") == Some(2)`.
    pub fn version(&self, name: &str) -> Option<u32> {
        self.list
            .iter()
            .find(|(feature, _)| feature == name)
            .map(|(_, version)| *version)
    }

    /// Whether the feature is there, at any version.
    pub fn has(&self, name: &str) -> bool {
        self.version(name).is_some()
    }

    /// Whether this is the niri fork at all.
    pub fn is_fork(&self) -> bool {
        !self.list.is_empty()
    }

    /// Every feature and its version.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u32)> {
        self.list
            .iter()
            .map(|(name, version)| (name.as_str(), *version))
    }

    fn parse(list: &Value) -> Self {
        let list = list
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|feature| {
                        let name = feature.get("name")?.as_str()?.to_owned();
                        let version = u32::try_from(feature.get("version")?.as_u64()?).ok()?;
                        Some((name, version))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { list }
    }
}

/// What niri answered to `Features`, asked once for the program's life. Empty when niri isn't
/// running, or is stock niri.
pub fn features() -> &'static Features {
    static FEATURES: OnceLock<Features> = OnceLock::new();
    FEATURES.get_or_init(|| {
        ask(&json!("Features"), "Features")
            .map(|list| Features::parse(&list))
            .unwrap_or_default()
    })
}

/// A connected output.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// The connector, like `eDP-1`.
    pub name: String,
    /// The maker its EDID names; empty when it names none.
    pub make: String,
    /// The model its EDID names; empty when it names none.
    pub model: String,
    /// The serial number its EDID gives, when it gives one.
    pub serial: Option<String>,
    /// Width and height in millimetres, when known.
    pub physical_size: Option<(u32, u32)>,
    /// Where it is in the layout, when it's turned on.
    pub logical: Option<Logical>,
    /// Whether it could show HDR (the fork; false on stock niri).
    pub hdr_supported: bool,
    /// Whether it shows HDR now (the fork).
    pub hdr_enabled: bool,
    /// SDR white in nits while it shows HDR (the fork).
    pub sdr_brightness: Option<f64>,
    /// The panel's peak brightness in nits, from its EDID (the fork).
    pub peak_luminance: Option<f64>,
    /// Every mode it offers.
    pub modes: Vec<Mode>,
    /// The index in [`modes`](Self::modes) of the one it shows, when it's on.
    pub current_mode: Option<usize>,
    /// Whether it shows a mode it doesn't list (a custom one).
    pub is_custom_mode: bool,
    /// Whether it can vary its refresh rate.
    pub vrr_supported: bool,
    /// Whether it varies its refresh rate now.
    pub vrr_enabled: bool,
}

/// A mode an output offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Width in physical pixels.
    pub width: u16,
    /// Height in physical pixels.
    pub height: u16,
    /// In millihertz, like 120000 for 120 Hz.
    pub refresh_rate: u32,
    /// Whether the output says it's the mode to use.
    pub is_preferred: bool,
}

impl Output {
    /// The mode it shows, when it's on.
    pub fn mode(&self) -> Option<&Mode> {
        self.modes.get(self.current_mode?)
    }

    /// Whether it's on: an output that's off shows no mode.
    pub fn on(&self) -> bool {
        self.current_mode.is_some()
    }
}

impl Mode {
    /// The refresh rate in hertz.
    pub fn hz(self) -> f64 {
        f64::from(self.refresh_rate) / 1000.0
    }
}

/// How an output is turned, counter-clockwise, as niri turns them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transform {
    /// Not turned.
    #[default]
    Normal,
    /// Turned 90 degrees.
    _90,
    /// Turned 180 degrees.
    _180,
    /// Turned 270 degrees.
    _270,
    /// Mirrored left to right.
    Flipped,
    /// Mirrored, then turned 90 degrees.
    Flipped90,
    /// Mirrored, then turned 180 degrees.
    Flipped180,
    /// Mirrored, then turned 270 degrees.
    Flipped270,
}

impl Transform {
    /// All eight, in niri's order.
    pub const ALL: [Transform; 8] = [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    /// Its name in niri's IPC.
    pub fn ipc_name(self) -> &'static str {
        match self {
            Transform::Normal => "Normal",
            Transform::_90 => "90",
            Transform::_180 => "180",
            Transform::_270 => "270",
            Transform::Flipped => "Flipped",
            Transform::Flipped90 => "Flipped90",
            Transform::Flipped180 => "Flipped180",
            Transform::Flipped270 => "Flipped270",
        }
    }

    fn from_ipc(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.ipc_name() == name)
    }

    /// Turned a quarter, so its width and height swap.
    pub fn sideways(self) -> bool {
        matches!(
            self,
            Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
        )
    }
}

/// An output's place in the layout, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Logical {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// The width, after scale and transform.
    pub width: u32,
    /// The height, after scale and transform.
    pub height: u32,
    /// Physical pixels per logical pixel, like 1.5.
    pub scale: f64,
    /// How it's turned.
    pub transform: Transform,
}

/// The connected outputs, by name.
pub fn outputs() -> Result<Vec<Output>, NiriError> {
    let outputs = ask(&json!("Outputs"), "Outputs")?;
    let outputs = outputs
        .as_object()
        .ok_or_else(|| NiriError::Unexpected(String::from("outputs aren't a map")))?;

    let mut list: Vec<Output> = outputs.values().filter_map(parse_output).collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

fn parse_output(output: &Value) -> Option<Output> {
    let text = |key: &str| output.get(key)?.as_str().map(str::to_owned);
    let logical = output
        .get("logical")
        .filter(|l| !l.is_null())
        .and_then(|l| {
            Some(Logical {
                x: i32::try_from(l.get("x")?.as_i64()?).ok()?,
                y: i32::try_from(l.get("y")?.as_i64()?).ok()?,
                width: u32::try_from(l.get("width")?.as_u64()?).ok()?,
                height: u32::try_from(l.get("height")?.as_u64()?).ok()?,
                scale: l.get("scale")?.as_f64()?,
                transform: l
                    .get("transform")
                    .and_then(Value::as_str)
                    .and_then(Transform::from_ipc)
                    .unwrap_or_default(),
            })
        });
    Some(Output {
        name: text("name")?,
        make: text("make").unwrap_or_default(),
        model: text("model").unwrap_or_default(),
        serial: text("serial"),
        physical_size: output
            .get("physical_size")
            .and_then(Value::as_array)
            .and_then(|size| {
                let side = |at: usize| u32::try_from(size.get(at)?.as_u64()?).ok();
                Some((side(0)?, side(1)?))
            }),
        logical,
        hdr_supported: output.get("hdr_supported").and_then(Value::as_bool) == Some(true),
        hdr_enabled: output.get("hdr_enabled").and_then(Value::as_bool) == Some(true),
        sdr_brightness: output.get("sdr_brightness").and_then(Value::as_f64),
        peak_luminance: output.get("peak_luminance").and_then(Value::as_f64),
        modes: output
            .get("modes")
            .and_then(Value::as_array)
            .map(|modes| modes.iter().filter_map(parse_mode).collect())
            .unwrap_or_default(),
        current_mode: output
            .get("current_mode")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok()),
        is_custom_mode: output.get("is_custom_mode").and_then(Value::as_bool) == Some(true),
        vrr_supported: output.get("vrr_supported").and_then(Value::as_bool) == Some(true),
        vrr_enabled: output.get("vrr_enabled").and_then(Value::as_bool) == Some(true),
    })
}

fn parse_mode(mode: &Value) -> Option<Mode> {
    Some(Mode {
        width: u16::try_from(mode.get("width")?.as_u64()?).ok()?,
        height: u16::try_from(mode.get("height")?.as_u64()?).ok()?,
        refresh_rate: u32::try_from(mode.get("refresh_rate")?.as_u64()?).ok()?,
        is_preferred: mode.get("is_preferred").and_then(Value::as_bool) == Some(true),
    })
}

/// Sends one action and waits for niri to take it.
fn act(action: Value) -> Result<(), NiriError> {
    ask_handled(&json!({ "Action": action }))
}

/// Sends a request whose answer is `"Handled"`.
fn ask_handled(request: &Value) -> Result<(), NiriError> {
    let stream = send(&request.to_string()).ok_or(NiriError::NotRunning)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| NiriError::Io(err.to_string()))?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|err| NiriError::Io(err.to_string()))?;
    let reply: Value =
        serde_json::from_str(&line).map_err(|err| NiriError::Unexpected(err.to_string()))?;
    match reply.get("Err") {
        Some(err) => Err(NiriError::Refused(
            err.as_str().unwrap_or("an error").to_owned(),
        )),
        None => Ok(()),
    }
}

/// Switches to the keyboard layout at `index`, in the order [`KeyboardLayouts`](crate::KeyboardLayouts) lists them.
pub fn switch_keyboard_layout(index: u8) -> Result<(), NiriError> {
    act(json!({ "SwitchLayout": { "layout": { "Index": index } } }))
}

/// Focuses a window, switching to its workspace.
pub fn focus_window(id: u64) -> Result<(), NiriError> {
    act(json!({ "FocusWindow": { "id": id } }))
}

/// Turns every monitor off until the next input.
pub fn power_off_monitors() -> Result<(), NiriError> {
    act(json!({ "PowerOffMonitors": {} }))
}

/// Turns the monitors on again, as the next input would.
pub fn power_on_monitors() -> Result<(), NiriError> {
    act(json!({ "PowerOnMonitors": {} }))
}

/// Ends the session, without asking.
pub fn quit() -> Result<(), NiriError> {
    act(json!({ "Quit": { "skip_confirmation": true } }))
}

/// A change niri makes to an output at once, and keeps until it reads its config again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutputChange {
    /// Turns the output on.
    On,
    /// Turns the output off.
    Off,
    /// One of the modes the output lists.
    Mode(Mode),
    /// A mode the output doesn't list: width, height and refresh rate in hertz.
    CustomMode {
        /// Width in physical pixels.
        width: u16,
        /// Height in physical pixels.
        height: u16,
        /// Refresh rate in hertz.
        hz: f64,
    },
    /// Physical pixels per logical pixel, like 1.5.
    Scale(f64),
    /// How the output is turned.
    Transform(Transform),
    /// Its top-left corner in the layout, in logical pixels.
    Position {
        /// The left edge.
        x: i32,
        /// The top edge.
        y: i32,
    },
    /// A varying refresh rate: on, and if so whether only while a window asks for it.
    Vrr {
        /// Whether the refresh rate may vary.
        on: bool,
        /// Only while a window asks for it.
        on_demand: bool,
    },
    /// HDR on or off (the fork).
    Hdr(bool),
    /// SDR white in nits while it shows HDR (the fork).
    SdrBrightness(f64),
}

impl OutputChange {
    fn action(self) -> Value {
        match self {
            OutputChange::On => json!("On"),
            OutputChange::Off => json!("Off"),
            OutputChange::Mode(mode) => json!({ "Mode": { "mode": { "Specific": {
                "width": mode.width,
                "height": mode.height,
                "refresh": mode.hz(),
            } } } }),
            OutputChange::CustomMode { width, height, hz } => json!({ "CustomMode": { "mode": {
                "width": width,
                "height": height,
                "refresh": hz,
            } } }),
            OutputChange::Scale(scale) => json!({ "Scale": { "scale": { "Specific": scale } } }),
            OutputChange::Transform(transform) => {
                json!({ "Transform": { "transform": transform.ipc_name() } })
            }
            OutputChange::Position { x, y } => {
                json!({ "Position": { "position": { "Specific": { "x": x, "y": y } } } })
            }
            OutputChange::Vrr { on, on_demand } => {
                json!({ "Vrr": { "vrr": { "vrr": on, "on_demand": on_demand } } })
            }
            OutputChange::Hdr(hdr) => json!({ "Hdr": { "hdr": hdr } }),
            OutputChange::SdrBrightness(nits) => json!({ "SdrBrightness": { "nits": nits } }),
        }
    }
}

/// Makes `change` to `output` until niri reads its config again.
pub fn set_output(output: &str, change: OutputChange) -> Result<(), NiriError> {
    ask_handled(&json!({ "Output": { "output": output, "action": change.action() } }))
}

/// Shows `mode` on `output` until niri reads its config again.
pub fn set_output_mode(output: &str, mode: Mode) -> Result<(), NiriError> {
    set_output(output, OutputChange::Mode(mode))
}

/// Calls `on_load` each time niri reads its config, with whether the read failed (and so
/// changed nothing), until niri exits. It blocks, so run it on a thread of its own. Changes made
/// with [`set_output`] are lost at each read, so this is where to make them again.
pub fn follow_config_loads(mut on_load: impl FnMut(bool)) {
    let wants: super::stream::Wants = |event| matches!(event, super::Event::ConfigLoaded { .. });
    for event in super::stream::subscribe(wants) {
        if let super::Event::ConfigLoaded { failed } = event {
            on_load(failed);
        }
    }
}

/// Stops a screencast.
pub fn stop_cast(session_id: u64) -> Result<(), NiriError> {
    act(json!({ "StopCast": { "session_id": session_id } }))
}

/// An open window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The id niri gives the window.
    pub id: u64,
    /// Its title, when the app set one.
    pub title: Option<String>,
    /// Its app id, when the app set one.
    pub app_id: Option<String>,
    /// The process that made it, when niri knows.
    pub pid: Option<i32>,
    /// The id of the workspace it's on, if any.
    pub workspace_id: Option<u64>,
    /// Whether it has the keyboard focus.
    pub is_focused: bool,
    /// Whether it floats rather than tiles.
    pub is_floating: bool,
    /// Whether it's fullscreen (the fork; false on stock niri).
    pub is_fullscreen: bool,
}

/// Every open window.
pub fn windows() -> Result<Vec<Window>, NiriError> {
    let windows = ask(&json!("Windows"), "Windows")?;
    Ok(windows
        .as_array()
        .map(|list| list.iter().filter_map(parse_window).collect())
        .unwrap_or_default())
}

/// The window with the keyboard focus, if any.
pub fn focused_window() -> Result<Option<Window>, NiriError> {
    let window = ask(&json!("FocusedWindow"), "FocusedWindow")?;
    Ok(parse_window(&window))
}

fn parse_window(window: &Value) -> Option<Window> {
    let text = |key: &str| window.get(key)?.as_str().map(str::to_owned);
    let flag = |key: &str| window.get(key).and_then(Value::as_bool) == Some(true);
    Some(Window {
        id: window.get("id")?.as_u64()?,
        title: text("title"),
        app_id: text("app_id"),
        pid: window
            .get("pid")
            .and_then(Value::as_i64)
            .and_then(|pid| i32::try_from(pid).ok()),
        workspace_id: window.get("workspace_id").and_then(Value::as_u64),
        is_focused: flag("is_focused"),
        is_floating: flag("is_floating"),
        is_fullscreen: flag("is_fullscreen"),
    })
}

/// A screencast niri is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cast {
    /// The id niri gives the cast session, which stop_cast takes.
    pub session_id: u64,
    /// The id niri gives the cast's stream.
    pub stream_id: u64,
    /// `PipeWire` (a portal screencast) or `WlrScreencopy`.
    pub kind: String,
    /// What it shows.
    pub target: CastTarget,
    /// Whether it's casting now, rather than waiting to start.
    pub is_active: bool,
    /// The program casting, when niri knows.
    pub pid: Option<i32>,
}

/// What a screencast shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CastTarget {
    /// Nothing yet, or nothing niri names.
    Nothing,
    /// An output, by its connector name.
    Output(String),
    /// A window, by the id niri gives it.
    Window(u64),
}

/// The screencasts running now.
pub fn casts() -> Result<Vec<Cast>, NiriError> {
    let casts = ask(&json!("Casts"), "Casts")?;
    Ok(casts
        .as_array()
        .map(|list| list.iter().filter_map(parse_cast).collect())
        .unwrap_or_default())
}

pub(super) fn parse_cast(cast: &Value) -> Option<Cast> {
    let target = cast.get("target")?;
    let target = if let Some(output) = target.get("Output") {
        CastTarget::Output(output.get("name")?.as_str()?.to_owned())
    } else if let Some(window) = target.get("Window") {
        CastTarget::Window(window.get("id")?.as_u64()?)
    } else {
        CastTarget::Nothing
    };
    Some(Cast {
        session_id: cast.get("session_id")?.as_u64()?,
        stream_id: cast.get("stream_id")?.as_u64()?,
        kind: cast.get("kind")?.as_str()?.to_owned(),
        target,
        is_active: cast.get("is_active").and_then(Value::as_bool) == Some(true),
        pid: cast
            .get("pid")
            .and_then(Value::as_i64)
            .and_then(|pid| i32::try_from(pid).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_changes_are_niri_actions() {
        assert_eq!(
            OutputChange::Scale(1.5).action(),
            json!({ "Scale": { "scale": { "Specific": 1.5 } } })
        );
        assert_eq!(
            OutputChange::Transform(Transform::Flipped90).action(),
            json!({ "Transform": { "transform": "Flipped90" } })
        );
        assert_eq!(
            OutputChange::Vrr {
                on: true,
                on_demand: true
            }
            .action(),
            json!({ "Vrr": { "vrr": { "vrr": true, "on_demand": true } } })
        );
        assert_eq!(
            OutputChange::SdrBrightness(203.0).action(),
            json!({ "SdrBrightness": { "nits": 203.0 } })
        );
    }

    #[test]
    fn reads_the_features_list() {
        let list = serde_json::from_str(
            r#"[{"name":"blur-shapes","version":2},{"name":"desktop-obscured","version":1}]"#,
        )
        .expect("json");
        let features = Features::parse(&list);
        assert_eq!(features.version("blur-shapes"), Some(2));
        assert!(features.has("desktop-obscured"));
        assert!(!features.has("hdr"));
        assert!(features.is_fork());
        assert!(!Features::default().is_fork());
    }

    #[test]
    fn reads_outputs_windows_and_casts() {
        let output: Value = serde_json::from_str(
            r#"{"name":"eDP-1","make":"Samsung","model":"ATNA40","logical":{"x":0,"y":0,"width":1600,"height":1000,"scale":1.75,"transform":"Normal"},"hdr_supported":true,"hdr_enabled":false,"sdr_brightness":null,"peak_luminance":600.0}"#,
        )
        .expect("json");
        let output = parse_output(&output).expect("an output");
        assert_eq!(output.name, "eDP-1");
        assert_eq!(output.logical.map(|l| l.width), Some(1600));
        assert!(output.hdr_supported && !output.hdr_enabled);
        assert_eq!(output.peak_luminance, Some(600.0));
        assert!(output.mode().is_none(), "no modes in this answer");

        let modes: Value = serde_json::from_str(
            r#"{"name":"eDP-1","make":"","model":"","logical":null,"modes":[{"width":2880,"height":1800,"refresh_rate":120000,"is_preferred":true},{"width":2880,"height":1800,"refresh_rate":60001,"is_preferred":false}],"current_mode":1}"#,
        )
        .expect("json");
        let modes = parse_output(&modes).expect("an output");
        assert_eq!(modes.modes.len(), 2);
        assert_eq!(modes.mode().map(|m| m.refresh_rate), Some(60001));

        // stock niri: no fork fields, and an output that's off has no logical place
        let stock: Value =
            serde_json::from_str(r#"{"name":"HDMI-A-1","make":"","model":"","logical":null}"#)
                .expect("json");
        let stock = parse_output(&stock).expect("an output");
        assert!(stock.logical.is_none() && !stock.hdr_supported);

        let full: Value = serde_json::from_str(
            r#"{"name":"eDP-1","make":"Lenovo","model":"ATNA40","serial":null,"physical_size":[300,190],"modes":[{"width":2880,"height":1800,"refresh_rate":120000,"is_preferred":true}],"current_mode":0,"is_custom_mode":false,"vrr_supported":true,"vrr_enabled":false,"logical":{"x":0,"y":0,"width":1645,"height":1028,"scale":1.75,"transform":"Flipped90"}}"#,
        )
        .expect("json");
        let full = parse_output(&full).expect("an output");
        assert_eq!(full.physical_size, Some((300, 190)));
        assert!(full.serial.is_none() && full.vrr_supported && !full.vrr_enabled);
        assert!(full.on());
        assert_eq!(full.mode().map(|m| m.hz()), Some(120.0));
        let transform = full.logical.map(|l| l.transform);
        assert_eq!(transform, Some(Transform::Flipped90));
        assert!(transform.is_some_and(Transform::sideways));

        let window: Value = serde_json::from_str(
            r#"{"id":7,"title":"foot","app_id":"foot","pid":4242,"workspace_id":3,"is_focused":true,"is_floating":false,"is_urgent":false}"#,
        )
        .expect("json");
        let window = parse_window(&window).expect("a window");
        assert_eq!((window.id, window.pid), (7, Some(4242)));
        assert!(window.is_focused && !window.is_fullscreen);

        let cast: Value = serde_json::from_str(
            r#"{"stream_id":2,"session_id":1,"kind":"PipeWire","target":{"Window":{"id":7}},"is_dynamic_target":false,"is_active":true,"pid":99,"pw_node_id":55}"#,
        )
        .expect("json");
        let cast = parse_cast(&cast).expect("a cast");
        assert_eq!(cast.target, CastTarget::Window(7));
        assert_eq!(cast.pid, Some(99));
    }
}
