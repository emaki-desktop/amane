# Notifications (fork additions)

The fork adds what a shell needs to draw toasts and a notification
center on top of upstream's `Notifications` server: how long the sender
asked a notification to show, the hints that say what kind it is, an
icon as a file, and a way for the shell to let one go. amane still
never closes a notification by itself. The shell decides when a toast
ends and what stays in its history. Upstream's guide is
[Notifications](../../src/notifications.md).

## Usage

```rust
let notifications = Notifications::read();

for notification in notifications.list() {
    let lifetime = match notification.timeout() {
        Timeout::After(duration) => Some(duration),
        Timeout::Default => Some(Duration::from_secs(5)),
        Timeout::Never => None,
    };

    // group by program, draw notification.icon_path() …
}

// once a transient toast is done, from an input handler or a Service
Notifications::expire(id);
```

| Function | Gives |
| --- | --- |
| `timeout()` | `Timeout::Default` (the sender left it to the shell), `Timeout::Never`, or `Timeout::After(duration)` |
| `icon_path()` | the icon as a file: a theme icon found by its name, a path as it was sent, or the sender's program icon from `desktop-entry` |
| `category()` | the kind of event, like `"email.arrived"` or `"device.added"`, or empty |
| `desktop_entry()` | the sender's desktop file id, like `"firefox"`, or empty. It is steadier than `app_name()` for grouping. |
| `transient()` | `true` when it should be left out of a history once shown |
| `resident()` | `true` when it stays open after an action |
| `Notifications::expire(id)` | closes it with the reason *expired*, for a toast the shell is done with |
| `kept()` | a `Kept` (re-exported as `amane::KeptNotification`): the words, pictures, sender and time, serde-ready, for a shell to save |
| `Notifications::restore(list)` | puts saved `Kept`s back, oldest first, before anything that arrived since |
| `restored()` | `true` for one that came back through `restore` |

To hide popups after a few seconds, compare `received()` plus the timeout with the current
time in a Service that polls, and only show the recent ones in your popup window. Keep the full
list for a notification center. When a `transient()` notification's popup ends, call
`Notifications::expire(id)` so it leaves the list too.

## Design / internals

**Timeout.** `Notify` sends `expire_timeout` in milliseconds: `-1` for
the server's default, `0` for never, or a duration. Upstream ignored it.
The fork keeps it as `Timeout` and leaves the policy to the shell. A
shell with a notification center usually hides the toast when the
timeout runs out and keeps the notification in its list.

**Hints.** `category`, `desktop-entry` and `transient` are read next to
`urgency` and `resident`. A missing hint reads as empty or `false`.

**Icon.** `icon_path()` is found once, when the notification arrives, on
the bus thread. The first lookup by name walks the icon themes, which
takes seconds, so a view never waits for it. A `file://` link is
unwrapped. When the sender names no icon, the `desktop-entry` hint is
looked up instead. The `image()` hints, such as an avatar, are separate
and unchanged.

**Limits.** Any program on the session bus may notify, so what one sends is bounded before
it's kept:

- **Pictures.** `image-data` is checked against the bytes it carries before anything is
  allocated: 8 bits a sample, 3 or 4 channels, a row stride at least a row of pixels, enough
  bytes for the last row, and at most 16384 pixels a side and 72 MP, all in checked arithmetic.
  It's written as a png only under `$XDG_RUNTIME_DIR/amane-notifications` (made `0700`), as a new
  `0600` file, never through a link left there; without a runtime directory nothing is saved, as
  a shared `/tmp` would let another user swap the file. An `image-path` or icon file is read like
  any image (see [hardening](hardening.md)): a device such as `/dev/zero`, or a fifo, isn't.
- **Rate.** A sender gets 30 notifications in 10 seconds; more are answered with id 0 and not
  shown, so a flood can't bury the screen or the history.
- **Text.** The summary is cut at 1 KiB and the body at 16 KiB (at a letter's edge, with "…"),
  and at most 8 buttons are kept.

**Expire.** `expire` closes the notification and emits
`NotificationClosed` with reason 1 (*expired*), where `dismiss` sends
2. A sender can tell that its notification timed out, rather than being
dismissed by the user.

**Restore.** amane keeps nothing on disk; a shell that wants its
history to outlive a restart saves `kept()` for each notification and
hands the list to `Notifications::restore` when it starts. Each one
gets a fresh id. It comes back without actions, a default action,
`resident` or a timeout, since the sender that could answer them may be
gone: clicking it dismisses it. Closing a restored one emits no
`NotificationClosed`, since nobody in this session sent it. `Kept`
holds the urgency as the spec's number (0, 1, 2) and `received` as
unix seconds.

**Capabilities.** The server now also lists `persistence`: notifications
stay until they are dismissed. It does not list `body-markup`, so
senders should send plain text. Some still send simple tags like `<b>`,
so a shell should strip them.
