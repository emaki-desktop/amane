# Following the clipboard: `ClipboardWatcher` (fork addition)

The clipboard's selection as it changes, whoever copied it, for a clipboard history.
[`Clipboard`](../windows/clipboard.md) is the other side: copying and pasting from a window.

```rust,ignore
use amane::{ClipboardWatcher, Service};

ClipboardWatcher::subscribe(|clipboard| {
    if clipboard.secret() { return }           // a password manager's copy
    let Some(kind) = clipboard.text_type() else { return };
    let kind = kind.to_owned();
    // contents() blocks, so not on this thread: hand it to a worker
    worker(move || remember(ClipboardWatcher::contents(&kind)));
}).forever();
```

| API | Meaning |
|---|---|
| `available()` | the compositor has `ext-data-control-v1` |
| `mime_types()` | the kinds the selection is offered as; empty when the clipboard is empty |
| `changes()` | goes up with each new selection, the same text copied twice included |
| `secret()` | the selection is offered as `x-kde-passwordManagerHint`, as `Clipboard::copy_secret` and password managers offer passwords |
| `text_type()` | the best text kind on offer |
| `ClipboardWatcher::contents(mime)` | the selection's data; blocks up to two seconds, and refuses a copy over 64 MiB |

## How it works

The service opens a Wayland connection of its own on its thread and binds
`ext_data_control_manager_v1` (from wayland-protocols' staging set, which sctk already brings).
Each selection arrives as an offer with its MIME types; the current offer is kept, and
`contents()` asks it to write into a pipe and reads to the end.

- **Never from the UI thread.** When this program owns the selection, its own UI thread is the
  one that writes the data, so `contents()` there would wait on itself.
- **No wlr fallback.** niri has `ext-data-control-v1`; without it `available()` stays false.
- **The primary selection** (middle-click paste) isn't followed.

Code: `src/services/clipboard_watcher.rs`.
