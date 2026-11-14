# Kernel device events (fork additions)

One netlink socket for the whole program, read on one thread and handed out per subsystem, so a
service that would poll sysfs can wait on the kernel's events instead. Only the kernel's own
broadcast is read (netlink group 1), which needs no privileges and doesn't depend on udev
running.

```rust
use amane::device_events;

// changes to any power supply: AC in or out, a USB-C PD port negotiating
if let Some(events) = device_events("power_supply") {
    for event in events {
        // event.action = "change", event.devpath = "/devices/…/power_supply/AC", event.subsystem
    }
}
```

| Function | Gives |
| --- | --- |
| `device_events(subsystem)` | a `Receiver<DeviceEvent>` for one subsystem, or `None` when the socket can't be opened (the caller then keeps polling) |
| `DeviceEvent` | `action` (`add`, `remove`, `change`, `move`, `online`, `offline`, `bind`, `unbind`), `devpath` (under `/sys`), `subsystem`, and `vars` (every `KEY=value` the kernel sent) |

Each subsystem is one channel: a late listener and a paused one don't slow the others. A
listener that stops reading is dropped from the hub the next event. The socket opening or
reading failing is logged once; every listener's channel closes, so a caller falls back to
polling.

Used by: `Brightness` for `backlight`, and the shell's USB-device, charger and removable-drive
feedback for `usb` and `power_supply`.
