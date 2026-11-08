# App windows (fork addition)

Upstream's normal windows (`Window`, `App::normal_window`, `open_window`) were set up once when they opened and never updated after that. The fork turns them into windows a regular app can be built on, so apps such as a settings app can run as their own processes next to the shell. Upstream's guide is [Windows](../../src/windows.md); this page has what the fork adds. `examples/app_window.rs` shows everything in one app.

Related pages: [Popups](popups.md), [Clipboard and Drag and Drop](clipboard.md), [Settings Files](config.md), [Desktop Dialogs and Notifications](portals.md), [IPC](../system/ipc.md), [Keys and shortcuts](../input/input.md#more-keys), [Text input](../input/text-input.md).

## Usage

### Window settings

```rust,ignore
Window::new()
    .title("Settings")
    .app_id("emaki-settings")
    .size(480.0, 320.0)
    .min_size(360.0, 240.0)
    .child(...)
```

- `title` can change while the window is open, for example to mark unsaved changes.
- `app_id` is what the compositor, docks and window rules know the app by, like `"emaki-settings"`. It's read once, when the window opens. Without it the window takes `App::app_id`, and without that `"amane"`.
- `resizable` is `true` by default (upstream: `false`).
- `min_size` and `max_size` limit how small or big the user can make it. A tiling compositor may still give a tiled window less.

Everything except `app_id` and `size` is read again on every frame, and only what changed is sent to the compositor, the same as for layer windows.

### Maximized and fullscreen

`.maximized(true)` and `.fullscreen(true)` ask the compositor to maximize the window or make it fullscreen. The request is only sent again when the view's answer changes, so the user can still unmaximize it from the compositor without the view fighting back. Leave them unset to let the user decide.

`minimize_window(name)` asks the compositor to hide the window until the user brings it back. Not every compositor can; niri can't, and does nothing.

### What the compositor says

Inside a view, `window_state()` tells you what the compositor last said about the window:

```rust,ignore
let state = window_state();

// grey the window out while another one has focus, like most apps do
let background = if state.focused { "#313244" } else { "#24273a" };
```

| Field | Meaning |
|---|---|
| `focused` | the window the user is working in |
| `maximized`, `fullscreen` | as the compositor has it, which may differ from what the view asked |
| `tiled` | placed edge to edge with other windows |
| `resizing` | the user is dragging its edges right now |
| `server_decorated` | the compositor draws its title bar. When `false`, the window has none, and Amane doesn't draw one |

In a layer window or a popup, all of them are `false`. A change redraws the window.

### Asking before closing

`on_close` runs when the user closes the window from the compositor, like with a keybind or the title bar. Return `false` to keep it open, for example to ask about unsaved changes first:

```rust,ignore
Window::new().on_close(|| {
    if Document::read().unsaved {
        open_window("unsaved", unsaved_dialog);

        return false;
    }

    true
})
```

`close_window` doesn't ask, so a dialog's "Discard" button can close the window for real.

### Dialogs

`.parent(name)` shows a window over the open window with that name, the way a dialog belongs to its app. `.modal(true)` also keeps the user out of the parent until the dialog closes, where the compositor supports it (`xdg-dialog`). Both are read once, when the window opens.

```rust,ignore
fn unsaved_dialog() -> Window {
    Window::new()
        .title("Unsaved changes")
        .size(300.0, 140.0)
        .resizable(false)
        .parent("main")
        .modal(true)
        .child(...)
}
```

### An app of its own

A normal window doesn't need a bar or the shell. An app that runs next to the shell sets its app-id, which also gives it its own IPC socket:

```rust,ignore
fn main() {
    App::new()
        .app_id("emaki-settings")
        .normal_window("main", main_window)
        .run();
}
```

The app exits when its last window closes. `App::keep_alive()` keeps it running anyway, for an app that only opens windows when asked.

Only one copy of an app runs: launching it again hands the launch to the running copy ([One copy at a time](../system/ipc.md#one-copy-at-a-time)). `without_ipc()` turns that off along with IPC.

### Many windows from one view

A view given to `open_window` is a plain function, so every window it opens shows the same thing. `open_window_with` takes a closure instead, so each window can carry its own data, like the document it shows:

```rust,ignore
open_window_with(format!("document {path}"), move || document(&path));
```

The name still tells windows apart, so give each one its own. Inside a view, `current_window()` gives the name of the window being drawn. Move it into a handler to close the window from inside it:

```rust,ignore
fn document(path: &str) -> Window {
    let me = current_window();

    Window::new().child(close_button().on_click(move |_| close_window(&me)))
}
```

`open_window`, `open_window_with`, `close_window` and `minimize_window` all work from anywhere: input handlers, IPC handlers, or a Service's thread.

## Design / internals: the phases

### Phase 1: normal windows that can change (done)

| Addition | Notes |
|---|---|
| `Window::app_id`, `App::app_id` | the app-id was hard-coded to `"amane"` |
| `Window::min_size`, `max_size` | `resizable(false)` still pins min = max |
| Live title, min/max, `maximized`, `fullscreen` | diffed like layer settings in `OpenWindow::update_normal` (`src/wayland/normal.rs`), and only what changed is sent |
| `window_state()` | `focused`, `maximized`, `fullscreen`, `tiled`, `resizing`, `server_decorated`, read from every configure |
| `Window::on_close` | `false` keeps the window open; `close_window` doesn't ask |
| `Window::parent`, `modal` | `xdg_toplevel.set_parent`, plus `xdg_wm_dialog_v1` for modal when the compositor has it |
| `open_window_with`, `current_window` | views can be closures with their own data, so one view can show many windows; window names are owned `String`s |
| `minimize_window` | niri ignores it |
| `App::keep_alive` | the app no longer exits when its last window closes |

Fixes along the way:
- A user-resized window keeps its size when a later configure leaves the size to the client.
- The first view run of a normal window goes through `frame::run_view`.
- `AMANE_FRAMES` names normal windows by their key.
- Open, close and minimize requests share one ordered queue (`window::REQUESTS`).

Not done: toplevel window geometry is not set, because without client-side decorations it would be the whole surface anyway.

### Phase 2: popups over app windows (done)

| Addition | Notes |
|---|---|
| `Popup::parent_window(name)` | parents a popup to a normal window's `xdg_surface` |
| `open_popup(closure)` | a slot opened from a handler and dropped once its popup has closed; a namespace that is already open is replaced |
| `pointer_position()` | where the pointer last was in its window, for anchoring a context menu |
| Popups close before their window | `WaylandState::close` closes a normal or layer window's popups first (xdg-shell forbids the reverse) |
| Keys to a grabbing popup | when the compositor keeps the focus on the window under a menu, keys go to the newest grabbing popup |

The popup docs are [popups.md](popups.md). Reading the configure's `position` (to know a popup was flipped) was planned but left out, because nothing needs it yet.

### Phase 3: keyboard (done)

| Addition | Notes |
|---|---|
| Key repeat | SCTK's `get_keyboard_with_repeat` on the event loop, plus `repeat_key` for compositors that repeat keys themselves; both run the same key-down path |
| `Key::{Delete, Insert, PageUp, PageDown, F(n)}` | keypad keys map to what they stand for; `ISO_Left_Tab` (Shift+Tab) is `Tab` |
| `on_key_release` | on `Window`, `LayerWindow` and `Popup` |
| `on_shortcut(Fn(Key) -> bool)` | runs before a focused `TextInput` for chords with Ctrl, Alt or Super; `true` keeps the key from the input |
| Focus redraw | a window redraws when it gets the keyboard, as it already did when it lost it |

Internally, a window's key handlers are one `KeyHandlers` struct (`src/input/target.rs`) instead of a lone `on_key`.

### Phase 4: clipboard and drag and drop (done)

| Addition | Notes |
|---|---|
| `Clipboard::{copy, paste}` | `wl_data_device` selection; text offered as `text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain`, `TEXT`, `STRING` |
| `PrimarySelection::{copy, paste}` | `zwp_primary_selection_v1`, optional |
| `Rectangle::on_text_drop` | text drags are taken anywhere and land on the innermost handler |
| `Rectangle::drag_out(Fn() -> DragData)` | starts `wl_data_device.start_drag` after 8 px of motion with the left button down, using the press serial |
| `changes::wake()` | wakes the loop without redrawing every window |

Data is written and pasted text read on their own threads (`src/wayland/clipboard.rs`). When amane owns the selection or the drag, it uses its own copy instead of reading a pipe it would have to write on the same thread.

Live-tested on niri with `wl-copy`/`wl-paste` in both directions for both selections. Drag-out and text drops are not live-tested, because no pointer automation tool is available; the uri-list encoding has a round-trip test. The shell's `text_edit.rs` still uses `wl-copy`/`wl-paste` until it pins an amane rev with this in it.

### Phase 5: text input (done)

| Addition | Notes |
|---|---|
| Selection | Shift with any movement, mouse drag, Ctrl+A; drawn behind the text (`selection_color`) |
| Mouse | a press places the cursor (Shift extends), a drag selects, a middle click pastes the primary selection there |
| Editing keys | Delete, word movement and deletion with Ctrl, Ctrl+Home/End |
| Clipboard | Ctrl+C/X/V; selections go to the primary selection; a password never leaves the input |
| Undo/redo | Ctrl+Z, Ctrl+Shift+Z/Ctrl+Y; typing is grouped a word at a time; 200 steps per input |
| `multiline(true)` | hard line breaks, Up/Down keep the column; no soft wrapping yet |
| Scrolling | a one-line input scrolls to keep the cursor in sight |
| IME | `zwp_text_input_v3` (`src/wayland/text_input.rs`): enabled while an input has focus and the window has the keyboard; surrounding text, cursor rectangle and content type (password, multiline) are sent on change; preedit drawn underlined |
| `TextInput::text(id)` | reads an input's text |

Other changes:
- The focused input's handler returns whether it used a key, so chords it doesn't know (Ctrl+Q) reach the window.
- `Clipboard::paste` callbacks now run on the event loop rather than the reader thread.

The editing model is `src/widgets/text_input/field.rs`, with unit tests. Live-tested on niri: typing, selection drawing, Shift+Home, cut and paste via the system clipboard, undo, Ctrl+Backspace, multiline Up/Down, one-line scrolling. Mouse selection and the IME are not live-tested, because no pointer automation tool or input method is installed; the IME's byte/letter conversions have unit tests.

### Phase 6: apps as separate processes (done)

| Addition | Notes |
|---|---|
| A socket per app-id | `ipc_socket_for(app_id)`: `amane.sock` for the default `"amane"` (the shell, unchanged), `emaki-settings.sock`, `amane-<id>.sock` |
| Single instance | `App::run` hands a launch to a running copy (`activate` call: the activation token, then argv) and exits instead of panicking |
| `App::on_activate(fn(&[String]))` | without it, the app's window comes to the front, or its windows open again when it was kept alive with none |
| `ipc_call(app_id, name, args)` | the client side, now shared by the CLI; `amane ipc call --app <id>` |
| xdg-activation | a window opened with `XDG_ACTIVATION_TOKEN` (or a handed-over token) takes the focus on its first configure; `spawn_activated` asks for a token for the last input and starts the program with it; `DesktopApp::launch`/`launch_with`/`DesktopAction::launch` use it |
| Spawned programs | `spawn` no longer passes on amane's own activation token |

Live-tested on niri: `ipc call --app`, a second launch handing over its argument, `spawn_activated` launching a copy, and a handed-over token moving the focus to an already open window.

Behaviour change: an example started without its own app-id beside the running shell now hands over to the shell and exits, instead of panicking with "already running".

### Phase 7: platform helpers (done)

| Addition | Notes |
|---|---|
| `Config<T: Settings>` | a generic Service over `~/.config/<APP>/<FILE>.json`: serde, atomic save, watched with `watch_file`, quiet when nothing changed; adds the `serde` dependency (already built for zbus) and re-exports it |
| `FileDialog` | the file chooser portal (`OpenFile`/`SaveFile`), filters, multiple, directory, name, folder; the answer arrives on its own thread and runs on the event loop |
| `Notify` | `org.freedesktop.Notifications.Notify` on the worker thread, under the app-id, with an urgency hint |
| `click_count()`, `Button::{Back, Forward}` | double and triple clicks within 400 ms and 4 px; `TextInput` selects a word on a double click and a line on a triple click |
| `Argument::Byte` | for D-Bus calls that take a single byte |
| `changes::later` | runs work handed back from another thread on the event loop; `Clipboard::paste` uses it too |

Not done: the file dialog's parent window (xdg-foreign export), and touch input.

Live-tested on niri: a notification reached the shell's notification server with the right arguments and showed as a toast; the file dialog opened through `xdg-desktop-portal-gnome` with its title and filters, and a file picked in it came back to the callback as its path.

### Phases 8 and 9: in emaki (done)

- **amane-kit:** the shell's controls, design tokens and theme moved into a workspace crate (now `crates/kit/`). The shell re-exports them under their old paths. New pieces for apps: `Checkbox`, `Radio`, `Dropdown`, `app_window`, `standard_shortcut`, `text_field`/`field_box`, `sidebar`, `Dialog`, and `examples/gallery.rs`.
- **Emaki Settings (`emaki-settings`):** an Appearance page (mode, accent, density, text size) that writes `~/.config/emaki/appearance.json` through `Config`. The kit's theme reads that file, so the shell follows it live.

The shell pins an amane rev that has phases 1–7.

Out of scope for now: client-side decorations, running without layer-shell, accessibility.
