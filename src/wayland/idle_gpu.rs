#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::cell::{Cell, RefCell};

use smithay_client_toolkit::reexports::calloop::{
    LoopHandle,
    timer::{TimeoutAction, Timer},
};

use crate::graphics::{VELLO_IDLE, on_vello_run, release_idle_scratch};

use super::WaylandState;

thread_local! {
    static HANDLE: RefCell<Option<LoopHandle<'static, WaylandState>>> = const { RefCell::new(None) };
    // a timer is on the loop
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/*
 * a timer that lets vello's scratch go once nothing drew through it for the idle time; armed
 * when vello runs and gone once the scratch is let go, so an idle desktop that draws only
 * quads (or nothing) has no timer waking it
 */
pub fn insert(handle: &LoopHandle<'static, WaylandState>) {
    if VELLO_IDLE.is_none() {
        return;
    }

    HANDLE.set(Some(handle.clone()));
    on_vello_run(arm);
}

fn arm() {
    let Some(idle) = *VELLO_IDLE else {
        return;
    };
    if ARMED.replace(true) {
        return;
    }

    HANDLE.with_borrow(|handle| {
        let Some(handle) = handle else {
            ARMED.set(false);
            return;
        };

        // checked every idle period: at most that long after the last run, at least one period
        let inserted = handle.insert_source(Timer::from_duration(idle), move |_, _, _| {
            if release_idle_scratch() {
                return TimeoutAction::ToDuration(idle);
            }

            ARMED.set(false);

            TimeoutAction::Drop
        });
        if let Err(err) = inserted {
            ARMED.set(false);
            eprintln!("amane: failed to start the gpu idle timer: {err}");
        }
    });
}
