# Compositor integration

Amane talks to the running Wayland compositor over its native IPC to
surface live **workspaces** and **windows**, and to send **control
actions** back. The backend is picked at runtime from the environment
([src/compositor.rs](../../../src/compositor.rs)); niri is the fully implemented
one, with partial support for hyprland, sway, and mango.

Everything here is exposed as a `Service` (read with `::read()` inside a
view, which subscribes the surface to changes) plus static control methods,
matching the rest of Amane's service layer.

The niri side of the fork's events and actions is documented in the niri fork:
[IPC events](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/ipc/events.md)
(`OverviewProgress`, `ViewOffsetChanged`, `WorkspaceSwitchProgress`, `DesktopObscured`),
[IPC actions](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/ipc/actions.md)
(`MoveColumnToIndex` by window id) and the
[desktop layer](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/layout/desktop-layer.md);
the index is [niri `docs/fork/README.md`](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/README.md).

## Services

### `Workspaces`

The ordered workspace list (upstream feature). `Workspaces::read().list()`
gives `&[Workspace]`, sorted by monitor then position. Each `Workspace`
exposes `id`, `index`, `name`, `output`, `active`, `focused`, `urgent`,
and a `windows` count. Control: `Workspaces::focus(id)`.

### `Windows` (fork addition)

The open-window list and the focused window — niri has no server-side
titlebars, so this is the only place a window title or app-id is available
to shell chrome.

```rust
let windows = Windows::read();

if let Some(window) = windows.focused() {
    let title = window.title().unwrap_or("");
    let app = window.app_id().unwrap_or("");
}

for window in windows.list() { /* build a dock / switcher */ }
```

`WindowInfo` fields (all via getters): `id() -> u64`,
`app_id() -> Option<&str>`, `title() -> Option<&str>`,
`focused() -> bool`, `floating() -> bool`, `workspace() -> Option<i64>`,
`column() -> Option<(usize, usize)>` (its column and its place in that column
in niri's scrolling layout, both from 1; `None` while floating, and on other
backends; kept current from niri's `WindowLayoutsChanged`, which only counts
when a window moved, not when it resized).
The list is sorted by `id`, so id-keyed UI stays stable across updates.
`focused()` returns `None` when focus is on a non-window surface (e.g. a
layer-shell panel).

`Windows::read().overview()` reports whether niri's overview is currently
open — handy for a shell that wants to expand or restyle itself while the
overview is up. It tracks the `OverviewOpenedOrClosed` event and is always
`false` on backends without overview support.

> Named `WindowInfo`, not `Window`, because the crate root already exports
> [`Window`](../../../src/window.rs) for the xdg-toplevel *surface role* a shell
> opens itself.

### `Overview` (fork addition, niri only)

Just the overview's open/close progress, the same value as
`Gestures::read().overview_progress()`:

```rust
let progress = Overview::read().progress(); // 0.0 closed .. 1.0 fully open
```

Read this one when the overview is all a view follows. `Gestures` changes
every time a view scrolls or a workspace switches, and every window that
reads it is drawn again each time; `Overview` stays quiet unless the
progress itself moves. It listens on its own stream, like the other
services. Under stock niri it stays at `0.0`.

### `Gestures` (fork addition, niri only)

Live, **per-frame** gesture values from niri, so a shell can animate in
lockstep with the compositor's own overview zoom, column scroll, and
workspace switch — rather than snapping on a discrete event. They require
the gesture-IPC niri fork; under stock niri (or another compositor) every
value stays at its default and the service simply never updates.

```rust
let g = Gestures::read();

let progress = g.overview_progress();     // 0.0 closed .. 1.0 fully open
let bar_height = compact + (tall - compact) * progress as f32;
```

Getters: `overview_progress() -> f64` (global, `0.0`..`1.0`),
`view_pos() -> f64` (focused output's horizontal scroll, logical px) with
`active_column_idx() -> usize`, and `workspace_idx() -> f64` (focused
output's fractional workspace position) with `active_workspace_idx() ->
usize`. `output() -> &str` names the focused output the per-output values
(`view_pos`/`workspace_idx`) belong to.

niri emits these only while something is moving and coalesces sub-epsilon
changes, so the service is silent at rest; the Wayland frame callback then
collapses the 60–144 Hz stream into one redraw per refresh. Because the
values are continuous, feed them straight into widget geometry — niri has
already done the easing.

### `WorkspaceSwitch` (fork addition, niri only)

The focused output's workspace switch alone: the same `output`, `workspace_idx` and
`active_workspace_idx` as `Gestures`, without the overview and the view scroll.

```rust
let switch = WorkspaceSwitch::read();

let offset = switch.workspace_idx() - switch.active_workspace_idx() as f64; // -1.0 .. 1.0 mid-switch
```

Read this one when a view follows workspace switches only: an event that leaves the switch where
it was (a column scroll, the overview) is quiet, so it doesn't redraw the view. Under stock niri
the values stay at 0.

### `Desktop` (fork addition, niri only)

Whether each output's desktop is **obscured**: nothing under its workspaces shows, as with the
overview fully open or windows covering the screen. It follows the niri fork's
`DesktopObscured` event, per output; elsewhere no output is ever obscured.

```rust
fn widgets() -> LayerWindow {
    let output = "eDP-1";               // the output this window is on
    let hidden = Desktop::read().obscured(output);

    LayerWindow::new()
        .layer(Layer::Background)
        .namespace("amane-desktop")     // matched by niri's `layer-rule { desktop true }`
        .frozen(hidden)
        .child(clock())
}
```

`LayerWindow::frozen(true)` keeps a layer window's last frame on screen, settings and all, and
draws and commits nothing. Its view still runs when something it read changes, so a frozen view
should read only what tells it when to thaw (here `Desktop`); the first frame after thawing
draws everything afresh. A window not shown yet ignores it. This is how a view on niri's desktop
layer stops animating while it can't be seen.

## Control actions (fork addition)

Typed requests to the compositor, each dispatched on a shared worker thread
so a slow socket never stalls drawing (like `Workspaces::focus`):

| Method | niri action |
| --- | --- |
| `Windows::focus(id)` | `FocusWindow` |
| `Windows::close(id)` | `CloseWindow` |
| `Windows::fullscreen(id)` | `FullscreenWindow` |
| `Windows::toggle_floating(id)` | `ToggleWindowFloating` |
| `Windows::move_to_workspace(id, ws)` | `MoveWindowToWorkspace` (the focus follows) |
| `Windows::move_to(id, ws, index)` | `MoveWindowToWorkspace` with `focus: false` when `ws` is `Some`, then `MoveColumnToIndex` with the window's id: its column to `index` (from 1), the focus staying put. That needs the niri fork ([actions](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/ipc/actions.md)); on stock niri (no `move-column-to-index-window-id` in `Features`) the window is focused first, then its column moved |
| `Windows::focus_column_left()` | `FocusColumnLeft` |
| `Windows::focus_column_right()` | `FocusColumnRight` |
| `Windows::spawn(cmd)` | `Spawn` |
| `Windows::screenshot()` | `Screenshot` |
| `Windows::toggle_overview()` | `ToggleOverview` |

`Windows::spawn(cmd)` asks the *compositor* to launch a command, so it is
tracked like any keybinding-launched program; use the crate-level
[`spawn`](../../../src/process.rs) when the shell should own the child itself.

Frozen windows are `src/layer_window.rs` and `src/wayland/redraw.rs`: a frozen window's
redraw stops after the view, before drawing (`916aecd`).

## How it works

Each `Service` owns its thread and a blocking `listen()` loop
([src/services.rs](../../../src/services.rs)). All of them follow **one** niri `EventStream`
connection ([src/compositor/niri/stream.rs](../../../src/compositor/niri/stream.rs)): a reader
thread parses each line once and hands the event to every listener through a channel of its
own. A listener that starts late (services start on their first read) is first given the state
so far, as niri gives a new connection: the workspaces, the windows, the overview, the gesture
values, each output's `DesktopObscured` and the keyboard layouts, each once niri has sent it.
Each listener keeps only the slice it needs:

- `compositor::listen(|workspaces| …)` — the workspace list, re-emitted
  after every relevant event.
- `compositor::listen_windows(|windows| …)` — the window list.
- `compositor::listen_gestures(|gesture| …)` — the live gesture snapshot, with each
  output's `obscured` flag (niri only; a no-op on other backends). `Gestures`, `Overview`,
  `WorkspaceSwitch` and `Desktop` each listen to it and keep only their slice, calling
  `quiet()` when their slice didn't change.

All parse the same niri event set
([src/compositor/niri.rs](../../../src/compositor/niri.rs)) —
`WorkspacesChanged`, `WorkspaceActivated`, `WorkspaceUrgencyChanged`,
`WindowsChanged`, `WindowOpenedOrChanged`, `WindowClosed`,
`WindowFocusChanged`, `WindowLayoutsChanged` (windows only; the workspace
listener skips it), `OverviewOpenedOrClosed`, and the fork's
`OverviewProgress` / `ViewOffsetChanged` / `WorkspaceSwitchProgress` / `DesktopObscured` — and
ignore the rest. An event that changes nothing a
service reads (e.g. a workspace event seen by the window listener) leaves
its list equal, and the service calls `quiet()` to redraw nothing.

One shared stream replaced a connection per service (six, each parsing every line) in
`dd6ae32`; each service still has its own thread and its own slice. When niri closes the
stream, every listener's channel closes and its `listen` returns, as before.

### `KeyboardLayouts` (fork addition, niri only)

The configured layouts and the one in use, from `KeyboardLayoutsChanged` and
`KeyboardLayoutSwitched` on the same stream: `names()`, `current()`, `current_index()`,
`switchable()`. Empty elsewhere. `niri::switch_keyboard_layout(index)` switches.

### `Casts` (fork addition, niri only)

The screencasts niri runs, from `CastsChanged`, `CastStartedOrChanged` and `CastStopped` on the
same stream: `all()`, `active()`, `any_active()`, for a privacy indicator without polling.

### `LockKeys` (fork addition)

Caps Lock and Num Lock, from the fork's `KeyboardLockKeysChanged` on the same stream. See
[lock-keys](../system/lock-keys.md).

### `Outputs` (fork addition, niri only)

The connected outputs as `niri::outputs()` answers them (`all()`, `get(name)`), read again when
the niri fork sends `OutputsChanged` (feature `outputs-changed`), which carries nothing. On
stock niri they're asked every 5 s, only while watched. `Brightness` follows the same event for
HDR instead of asking every two seconds.

## Asking niri: `amane::niri` (fork addition)

For a question the services don't keep the answer to, `amane::niri` sends one request and waits
up to two seconds for its answer, so call it from a service thread, not a view. Each returns
`Result<_, NiriError>` (`NotRunning` under another compositor); none panics.

| Function | Answers |
|---|---|
| `niri::features()` | the fork's [`Features`](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/ipc/features.md): `.has("hdr")`, `.version("blur-shapes")`, `.is_fork()`. Asked once and kept; empty on stock niri, which can't parse the request |
| `niri::outputs()` | each output's name, make, model, serial, size in millimetres, logical place and `Transform`, VRR, and the fork's HDR fields |
| `niri::windows()`, `niri::focused_window()` | windows with their `pid`, workspace, focus, floating, and the fork's `is_fullscreen` |
| `niri::casts()` | screencasts: kind, target (an output or a window), whether active, the casting `pid` |

Each output also lists its `modes` and `current_mode` (`Output::mode()`).

Actions, each `Result<(), NiriError>`: `focus_window(id)`, `switch_keyboard_layout(index)`,
`power_off_monitors()`, `quit()` (without asking), `set_output(output, change)` and
`set_output_mode(output, mode)`, and `stop_cast(session_id)`. An `OutputChange` turns an
output on or off, or sets its mode (or a custom one), scale, `Transform`, position, VRR, HDR
or SDR white; niri keeps it until it reads its config again. `follow_config_loads(on_load)`
calls back at each read (it blocks, so give it a thread), for making such changes again.

The fork-only actions check `features()` first: `SdrBrightness` is sent only when niri has
`hdr`, and `Windows::move_to` on stock niri focuses the window before `MoveColumnToIndex`,
which has no `window_id` there. Nothing needs a cargo feature: one build runs on both.

## Backend support

| | Workspaces | `focus_workspace` | Windows | actions | Gestures, Overview, WorkspaceSwitch, Desktop |
| --- | --- | --- | --- | --- | --- |
| niri | ✅ | ✅ | ✅ | ✅ | ✅ |
| hyprland | ✅ | ✅ | — | — | — |
| sway | ✅ | ✅ | — | — | — |
| mango | ✅ | ✅ | — | — | — |

On a backend without window support `Windows::read().list()` is empty and
the control methods are no-ops; the per-backend `listen_windows`/`act`
stubs (e.g. [hyprland.rs](../../../src/compositor/hyprland.rs)) are where that
support would be added.

## niri version note

Action request shapes and window-event fields are verified against the
niri IPC (`niri-ipc` crate) for **niri 26.04**. niri's IPC has changed
field names across versions; a mismatched request fails silently (niri
replies with an error that `act` currently drains and discards). If actions
stop working after a niri upgrade, re-check the JSON against the installed
`niri msg --json` / the `niri_ipc::Action` enum.
