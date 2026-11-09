# Fork API cheatsheet

The fork's rows for upstream's [Cheatsheet](../src/cheatsheet.md): only what the fork adds or
changes. Each section matches a section there.

## Windows

| Code | Does |
|---|---|
| `.app_id("emaki-settings")` | what the compositor and docks know it by, `App::app_id` by default |
| `.resizable(true)` | lets the user resize it, `true` by default (upstream: `false`) |
| `.min_size(w, h)` / `.max_size(w, h)` | limits for the user's resizing |
| `.maximized(true)` / `.fullscreen(true)` | asks the compositor, again only when the answer changes |
| `.parent("name")` / `.modal(true)` | a dialog over another normal window |
| `.on_shortcut(handler)` | chords with Ctrl/Alt/Super, before a focused text input, `Fn(Key) -> bool` |
| `.on_key_release(handler)` | keys going up, `Fn(Key)` |
| `Clipboard::copy(text)` / `Clipboard::paste(then)` | the Ctrl+C/Ctrl+V clipboard; `then` runs once the text arrives |
| `PrimarySelection::copy` / `::paste` | the middle-click selection |
| `Rectangle::on_text_drop(handler)` / `.drag_out(data)` | text dragged in; dragging `DragData` out |
| `Config::<T>::read()` / `::change(edit)` | a settings file in `~/.config`, shared live between programs |
| `FileDialog::open(title)...show(then)` | the desktop's file dialog through the portal |
| `Notify::new(summary).body(..).send()` | sends a desktop notification |
| `click_count()` | 1, 2 or 3 for the click being handled |
| `.on_close(handler)` | asked when the user closes it, `Fn() -> bool`; `false` keeps it open |
| `open_window_with(name, closure)` | `open_window` with a view that carries its own data |
| `close_window("name")` | closes it, without asking `on_close` |
| `minimize_window("name")` | asks the compositor to minimize it |
| `window_state()` | `focused`, `maximized`, `fullscreen`, `tiled`, `resizing`, `server_decorated` |
| `current_window()` | the name of the normal window being drawn |
| `App::app_id("id")` / `App::keep_alive()` | the app's default app-id and IPC socket; keep running with no windows |
| `App::on_activate(handler)` | the app was launched again, `fn(&[String])` with that launch's arguments |
| `ipc_call(app_id, name, args)` | calls another app's IPC handler, returns its reply |
| `spawn_activated(command)` | starts a program with an activation token, for launchers |

## Input

| Code | Gets |
|---|---|
| `.on_scroll(\|scroll\| ...)` | `Scroll { x, y, source, stop }`: `x`, `y` in lines, positive `y` is down; `source` is `Wheel`, `Finger`, `Continuous` or `Tilt`; `stop` when the fingers lift; `pixels()` |
| `.on_track(\|point\| ...)` | `Point` on every move, like `on_move`, without a redraw |
| `.on_release(\|point\| ...)` | `Point`, once when a drag's button is let go (before `on_click`) |
| `.on_drop(\|files\| ...)` | the `Vec<PathBuf>` dragged in from another program |
| `.on_files_over(\|at, files\| ...)` | `Option<Point>` and the held files; answer `true` if they can be let go there |
| `modifiers()` | the keys held now: `ctrl`, `shift`, `alt`, `logo`; `command()` for Ctrl, Alt or Super. `Ctrl`+letter reaches `on_key` as `Character(letter)` |

## Services

| Service | Reads | Controls |
|---|---|---|
| `Battery` | adds `state`, `on_ac`, `warning`, `time_to_empty`, `time_to_full`, `icon`, `peripherals` | |
| `PowerProfile` | `present`, `active`, `available`, `degraded` | `set` |
| `Idle` | `is_idle`, `crossed`, `levels`, `inhibited`, `inhibitors`, `running` | `set_levels` |
| `Apps` | each `DesktopApp` adds `id`, `categories`, `keywords`, `actions`, `mime_types`, `opens(file)`, `commands_with(files)` | `launch_with(files)` on each `DesktopApp` |
| `Appearance` | `present`, `scheme`, `prefers_dark`, `accent`, `high_contrast`, `reduced_motion` | |
| `Audio` | adds `outputs`, `inputs` (each an `AudioDevice`: `name`, `description`, `kind`, `default`), `output_kind`, `input_kind`, `streams` (each an `AudioStream`: `index`, `app`, `icon`, `binary`, `media`, `volume`, `muted`, `paused`), `balance` | adds `set_output(name)`, `set_input(name)`, `set_balance(b)`, `set_stream_volume(index, v)`, `set_stream_muted(index, m)`, `toggle_stream_mute(index)` |
| `AudioLevel` (new) | `peak` (0 to 1), recorded only while read | — |
| `Notifications` | each `Notification` adds `timeout`, `icon_path`, `category`, `desktop_entry`, `transient` | adds `expire(id)` |
| `Windows` | `list`, `focused`, `overview`; each `WindowInfo` has `id`, `app_id`, `title`, `focused`, `floating`, `workspace`, `column` | `focus`, `close`, `fullscreen`, `toggle_floating`, `move_to_workspace`, `move_to`, `focus_column_left`, `focus_column_right`, `spawn`, `screenshot`, `toggle_overview` |
| `Overview` (niri fork) | `progress` | |
| `Gestures` (niri fork) | `overview_progress`, `view_pos`, `active_column_idx`, `workspace_idx`, `active_workspace_idx`, `output` | |
| `Backdrop` (niri fork) | `luminance(id)` | |
| `WorkspaceSwitch` (niri fork) | `output`, `workspace_idx`, `active_workspace_idx` | |
| `Desktop` (niri fork) | `obscured(output)` | |

`Bus::emit_to(destination, …)` sends a signal to one program.

## Layer windows and popups

| Code | Does |
|---|---|
| `.blur_region(areas)` | the compositor blurs behind these rectangles |
| `.blur_shapes(shapes)` | rounded, fading `BlurShape`s with a material and tint |
| `.space(Zone::Thickness(px))` | reserves this many logical pixels of the anchored edge, however tall the window is (a dock taller than its card) |
| `.frozen(true)` | keeps the last frame up and draws nothing, e.g. while `Desktop::obscured` |
| `.vibrant(true)` | grey content blends with the frosted backdrop (also on `Window` and `Popup`) |
| `blur_fades()` / `material_available(name)` | whether shapes are drawn exactly; whether a material exists |
| `Popup::new(namespace).size(w, h).anchor_rect(x, y, w, h)` | an xdg-popup; `.parent(..)`, `.parent_window(..)`, `.anchor(..)`, `.gravity(..)`, `.grab()`, `.on_dismiss(..)` |
| `App::popup(view)` / `open_popup(view)` | a popup given at start-up; one opened from a handler |
| `pointer_position()` | where the pointer last was in its window, for a context menu |

## Canvas and shaders

| Code | Does |
|---|---|
| `Path::svg(d)` | a path from an SVG `d` attribute, `None` if it doesn't parse |
| `.place(x, y, scale)` / `.trim(start, end)` | scale and move a path; draw only a part of its length |
| `.join(Join::Round)` | `Miter` (default), `Round` or `Bevel` corners |
| `shader_values(rows)` | up to 64 rows (upstream: 16) |

## Text

| Code | Does |
|---|---|
| `.weight(650)` | any weight, `Weight::Number(n)`; exact on a variable font |
| `.italic(true)` | italic |
| `.line_height(1.4)` | line spacing as a multiple of the size |
| `.letter_spacing(-0.2)` | pixels after every letter |
| `.case(Case::Upper)` | `AsWritten`, `Upper`, `Lower`, `Capitalize` |
| `.decoration(Decoration::Underline)` | `None`, `Underline`, `StrikeThrough` |
| `.align(TextAlign::Center)` | `Start`, `Center`, `End`, `Justify` |
| `.direction(TextDirection::RightToLeft)` | `Auto`, `LeftToRight`, `RightToLeft` |
| `.variation(b"wdth", 87.5)` | a variable font axis |

## Animation

| Code | Does |
|---|---|
| `.easing(Easing::Spring { bounce: 0.2 })` | a damped spring, `bounce` 0 to 0.9; keeps its speed when retargeted |
| `animation.retime(duration)` | the same target over a new duration, from where it is |
| `Easing::apply(progress)` | eased progress, 0 to 1, for things you move yourself |
