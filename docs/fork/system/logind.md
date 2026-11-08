# The login session: `Logind` (fork addition)

The session amane runs in, as logind sees it, from logind's signals on the system bus.

```rust,ignore
use amane::{Logind, Service};

// lock when asked to (`loginctl lock-session`, or logind's idle action)
Logind::subscribe(|logind| {
    if logind.lock_requests() > LAST.swap(logind.lock_requests()) { lock_screen() }
}).forever();

// keep the computer awake while this is held
let inhibitor = Logind::inhibit("idle:sleep", "emaki", "Keep awake is on", "block")?;
```

| API | Meaning |
|---|---|
| `present()` | logind knows the session |
| `active()` | the session is the one in front on its seat |
| `locked_hint()`, `idle_hint()` | the hints the session's locker and idle tracker set |
| `preparing_for_sleep()` | between `PrepareForSleep(true)` and the wake-up |
| `lock_requests()`, `unlock_requests()` | counts of the session's `Lock` and `Unlock` signals; a subscriber notices a new one by the count going up |
| `wakeups()` | how often the computer woke from sleep since amane started |
| `Logind::inhibit(what, who, why, mode)` | an inhibitor lock, held until the returned `OwnedFd` drops |
| `Logind::set_locked_hint(locked)` | for a screen locker |

## How it works

The session is `/org/freedesktop/login1/session/auto`, which logind resolves to the caller's
session or, for a program outside any session like a systemd user service, the user's graphical
one. Signals come from the real object path, so the service asks for its `Id` and `GetSession`
once. It then reads every signal logind sends: the session's `Lock`, `Unlock` and
`PropertiesChanged` (`Active`, `LockedHint`, `IdleHint`), and the manager's `PrepareForSleep`.

Code: `src/services/logind.rs`.
