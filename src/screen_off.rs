#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! The screen being off, as the shell says when it turns the monitors off: nothing a window
//! shows can be seen, so windows aren't asked to draw (a lock screen's still are), and what
//! they read no longer keeps polling services going (`services::store`, `idle_stop`). The
//! first input clears it, through a one-second ext-idle-notify timer made when it's set, so a
//! wake that nobody reports (a key the shell never hears about) still brings everything back:
//! every window then draws afresh, and the services they read are watched again.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::changes;

static OFF: AtomicBool = AtomicBool::new(false);

// a change the event loop hasn't acted on yet (the input watch, the frames on waking)
static ASKED: AtomicBool = AtomicBool::new(false);

/// Says the screen is off (`true`), or on again. While off, windows other than a lock
/// screen's draw nothing and keep no service watched; the first input turns it on again by
/// itself. For the call that turns the monitors off, and the one that turns them back on.
pub fn set_screen_off(off: bool) {
    if OFF.swap(off, Ordering::Relaxed) != off {
        ASKED.store(true, Ordering::Relaxed);
        changes::wake();
    }
}

/// Whether [`set_screen_off`] says the screen is off.
pub fn screen_off() -> bool {
    OFF.load(Ordering::Relaxed)
}

// for the event loop: whether it changed since the last call
pub(crate) fn take_change() -> bool {
    ASKED.swap(false, Ordering::Relaxed)
}
