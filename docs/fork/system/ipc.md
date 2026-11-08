# IPC (fork additions)

What the fork changes on upstream's [IPC](../../src/ipc.md) page: a socket per app-id instead of
one per session, a single running copy of each app, and calling another app. Upstream had one
socket, `amane.sock`, and a second listener stopped with `amane is already running`.

## Usage

### One copy at a time

Each app listens on its own socket, named after its app-id ([`App::app_id`](../windows/app-windows.md#an-app-of-its-own)):

| App-id | Socket in `$XDG_RUNTIME_DIR` |
|---|---|
| `amane`, the default and the shell's | `amane.sock` |
| `emaki-settings`, `emaki`, `emaki-drop` | `emaki-settings.sock`, `emaki.sock`, `emaki-drop.sock` |
| `org.example.Notes`, `code.desktop` | `amane-org.example.Notes.sock`, `amane-code.desktop.sock` |

A plain name (lowercase letters and digits, words joined by `-`) is its own socket, so a family of programs names its sockets itself. Any other id gets the `amane-` prefix, so it can't land on another program's socket.

So the shell and your apps listen side by side, and only one copy of each runs. Starting an app whose socket already answers doesn't start a second copy: the new launch hands its arguments to the running copy and exits, printing `amane: <app id> is already running, it takes this launch`. The running copy then:

- runs `App::on_activate(handler)` with the new launch's arguments, like a file to open, or
- without `on_activate`, brings its window to the front, opening it again if it was closed (an app kept running by `App::keep_alive`).

When the launch came from a launcher with an activation token, the running copy's window takes the focus with it ([Activation](../windows/activation.md)). From a terminal there's no token, and the compositor decides.

```rust,ignore
fn main() {
    App::new()
        .app_id("amane-notes")
        .normal_window("main", main_window)
        .on_activate(|files| {
            for file in files {
                let file = file.clone();

                open_window_with(file.clone(), move || note(&file));
            }
        })
        .run();
}
```

`amane run` checks for a running shell first and says so in your terminal, and `amane restart` stops the running shell before starting the new one. `amane dev` restarts its own shell, but it can't stop a shell started some other way. Stop your everyday shell (the one from `amane run`) before running `amane dev`.

For a window that should run as many copies as it's started, use `App::new().without_ipc()`. It listens on no socket, so `on_activate` and IPC handlers aren't used.

### Calling another app

`amane ipc call --app <app id> <name> [arguments...]` calls a handler in the app with that app-id instead of the shell. From Rust, `amane::ipc_call(app_id, name, arguments)` does the same and returns the reply, so an app can ask the shell to do something:

```rust,ignore
// a settings app telling the shell to look at its settings again
let _ = amane::ipc_call("amane", "reload", &[]);
```

`ipc_call` waits for the reply, which an Amane app gives within a second. It returns an error when that app isn't running.

## Design / internals

- `ipc_socket_for(app_id)` names the socket. The IPC socket is one of the event loop's three
  sources: `amane ipc call <name>` from a terminal, or another app's `ipc_call`.
- Before any window opens, `run()` checks whether the app's socket already answers. If it does,
  it hands the launch to that running copy (`hand_over` in `src/app.rs`, an `activate` call
  carrying the activation token, then argv) and returns. So only one copy of each app-id runs.
- History: phase 6 of [App windows](../windows/app-windows.md#phase-6-apps-as-separate-processes-done) (`82337f2`).
