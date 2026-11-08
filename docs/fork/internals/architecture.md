# Architecture (fork additions)

What the fork changes in how amane works inside, as additions to upstream's
[ARCHITECTURE.md](../../../ARCHITECTURE.md). Each section names the upstream section it extends.
What this means for the code a shell writes (scoped redraws, quiet services, animations made
once, what opacity costs on the GPU) is in emaki's
[performance guide](https://github.com/emaki-desktop/emaki/blob/main/docs/developer/performance.md).

## Start-up: one copy per app-id

*Extends "Startup".*

One check comes before everything else: when the app's IPC socket already answers, `run()` hands
the launch to that running copy (`hand_over` in `src/app.rs`) and returns, so only one copy of
each app-id runs.

Apps listen on their app-id's IPC socket by default; `App::without_ipc` leaves it unused, for an
app that runs as many copies as it's started. The IPC socket source of the event loop serves
`amane ipc call <name>` from a terminal, or another app's `ipc_call`; there is one socket per
app-id, see `ipc_socket_for` ([IPC](../system/ipc.md)).

## Normal window settings

*Extends "One frame".*

`update_surface` compares the layer settings the view asked for with the ones it last sent. A
normal window does the same with its own settings (title, min and max size, maximized,
fullscreen) in `OpenWindow::update_normal` (`src/wayland/normal.rs`).

## Sources finer than a Service

*Extends "What makes a window redraw".*

Some things are tracked finer than a whole Service, as a `Source` (`changes.rs`). The
compositor's reading behind one blur shape is `Source::Backdrop(id)`, noted by
`Backdrop::luminance(id)`, so a reading wakes only the windows that asked about that shape.
Window titles are `Source::Titles`, noted by `WindowInfo::title()`. An event that changed nothing
but titles is quiet for `Windows` and marks only `Titles`.

`open_window`, `close_window`, `minimize_window` and `Lock::start` call `changes::mark_all`.
Window requests wait in one queue (`window::REQUESTS`), so an open followed by a close happens
in that order.

## Pointer motion and `on_track`

Input redraws only the window it happened in. Pointer motion is the exception: it asks for a
frame only when the target under the pointer has `on_move`. `on_track` is told the position
too, but redraws nothing, for a handler that only remembers where the pointer is
([Input](../input/input.md), `6b0099b`).

## Resting animations

Reading `Animation::value()` before the animation has arrived sets the event loop thread's
`moving` flag. A new `Animation` is at rest until `to()` sends it somewhere, so making one draws
nothing (`527c538`).

## Skipped frames and frozen windows

A redraw isn't always drawn. When a frame's commands equal the frame on screen, at the same size
and with nothing moving, the GPU is skipped and the window only commits (`Gpu::shows`,
`e56b843`). Hiding a window forgets its last frame, since showing it again needs a buffer. A
frozen layer window (`LayerWindow::frozen`) keeps it instead: its view still runs, but it neither
draws nor commits, so the last frame stays on screen until a view says otherwise (`916aecd`).

## Groups the renderer avoids

*Extends "Drawing".*

A faded group, or a clip with an effect inside it, is drawn on its own canvas as big as the
whole surface, so a lasting faded group costs a full extra pass every frame. `Renderer::blend`
avoids the group where the result would be the same. At opacity 0, or with nothing in it, it
draws nothing. A group of exactly one shape takes the opacity in that shape's color, unless the
faded color would land on another vibrancy layer (`3f57058`).

## GPU bounds

While the commands go into a vello scene, `bounds.rs` adds up how far they reach on the canvas:
fills, strokes, and vello's rectangles and borders, but not images, gradients, shadows, or
clips. A scene that only reaches a box draws into a texture the size of that box, in 64-pixel
steps so the spares fit again, moved so the box's corner is the texture's corner. It is then laid
down only inside the box. So a ring or a clock's digits that animate cost their own size, not
the whole window. A box that covers most of the window, or a scene with anything unmeasured in
it, is drawn whole as before (`6d2123b`).

## Files dragged in

*Extends "Input".*

Files dragged in from a file manager are handled in `src/wayland/dropped.rs`. When the drag
comes in, the list of files is read straight away, and every move goes to the target's
`on_files_over`, whose answer accepts or refuses the drop at that spot (with no such target, the
window takes files anywhere). Letting go tells that target the files have gone, then hands them
to the target's `on_drop`.
