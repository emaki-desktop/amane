# Subscriptions and idle services (fork addition)

Two additions to `Service`: one service can follow another without polling it, and a polling
service stops while nothing shows it.

## Following a service: `Service::subscribe`

```rust,ignore
use amane::{Battery, Service};

// whenever the battery changes, after the write that changed it
Battery::subscribe(|battery| warn_if_low(battery)).forever();
```

- The callback runs after a `Write` that wasn't made `quiet()`, with the service read for it.
- Callbacks run one at a time on one thread of their own (`amane-subscriptions`), in the order
  the changes came. A service whose callbacks are already waiting isn't queued again, so a burst
  of writes costs one call, which sees the latest value.
- Dropping the returned `Subscription` removes the callback; `.forever()` keeps it.
- Keep callbacks short, and don't write the same service from its own callback: the read it was
  given would wait forever (a debug build asserts). Writing another service is the point.
- A callback that panics is logged and the others still run.

## Polling only while watched: `Service::idle_stop`

```rust,ignore
impl Service for Cpu {
    fn interval() -> Duration { Duration::from_secs(2) }

    // stop reading /proc 30 s after nothing shows it
    fn idle_stop() -> Option<Duration> { Some(Duration::from_secs(30)) }
    // …
}
```

A service is **watched** while an open window's last frame read it, or something else (another
service, a program's own thread) called `read()` within `idle_stop`. A window that draws
nothing new reads nothing either, so a bar showing a value all day keeps it watched through its
last frame's reads, not through reads. After each turn of the event loop amane publishes which
services the open windows' last frames read (`WaylandState::publish_watched`).

The default polling `listen` checks after each sleep; an unwatched service parks its thread
until it is watched again, then polls at once. A service in amane with its own polling loop
(`Palette`, and `Rfkill` or `LockKeys` when they fall back to polling) calls `store::wait_while_unread::<Self>(idle)` after its sleep, and reads
itself through `store::find` there, since its own `read()` would count as watching.

The first frame of a window that starts showing a stopped service draws the value from before
it stopped; the poll that follows a moment later draws the fresh one.

Services with an idle stop (30 s, `services::IDLE_STOP`): `Cpu`, `Memory`, `Media` and
`Palette`, and the polling fallbacks of `Rfkill` and `LockKeys`. `Network` follows
NetworkManager's signals and resyncs only while watched. `Brightness` polls only where the
kernel's uevents can't be read, since the OSD reads it once per key press and a stale value
there would step from the wrong level.

## Design / internals

- `src/services/subscribe.rs`: the callbacks by service, the pending set, the thread.
- `src/services/store.rs`: each service's `Slot` holds its lock and its `Use` (the last read,
  whether its listener is parked); `set_watched` wakes a service a window starts reading.
- `src/services/write.rs`: the write lock is released before the change is marked and the
  callbacks are queued.
- Tests: a subscription that follows another service and ignores a quiet write; an unread
  service that parks until it's read; a service an open window reads that keeps polling and
  stops when the window goes.

## Frozen windows and the screen being off

A frozen window's reads ([`LayerWindow::frozen`](../../../src/layer_window.rs)) only say when to
thaw it, so they don't keep a polling service watched. Hidden windows' reads still do: a hidden
window runs its view on a change to see whether it should show, as the shell's on-screen display
does.

### The screen off: `set_screen_off`

```rust,ignore
// the shell turning the monitors off for the idle service
amane::niri::power_off_monitors()?;
amane::set_screen_off(true);
```

While the screen is off, windows other than a lock screen's aren't asked to draw, and their reads
keep no polling service watched (`idle_stop` then parks them). `set_screen_off(false)` ends it,
and so does the first input by itself: setting it makes a one-second ext-idle-notify timer per
seat, whose `resumed` clears it. Then every window draws afresh, and the services they read are
watched again (`WaylandState::update_screen_off`).

