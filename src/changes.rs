use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::timing;

// the backend sets this once, so services never name the event loop's types
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/*
 * what a window's drawing can depend on: a service, or the compositor's
 * reading behind one blur shape, which only the windows that asked for that
 * shape's reading show
 */
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Service(TypeId),
    Backdrop(u32),

    // a video feed's newest frame
    Video(u64),

    // windows' titles, which change far more often than what else is known of them
    Titles,
}

/*
 * which sources changed since the windows last looked, and whether
 * something changed that no source names, which every window may show
 */
static CHANGED: Mutex<Changes> = Mutex::new(Changes {
    services: None,
    everything: false,
});

struct Changes {
    services: Option<HashSet<Source>>,
    everything: bool,
}

/*
 * what some window's last frame read, hidden windows included (a hidden view still runs
 * when what it read changes, as the shell's OSD does to notice a change). A change to a
 * source no window reads wakes nobody; it is still kept, for the next frame that looks.
 * None until the event loop first says, so early changes all wake it.
 */
static READ_BY_WINDOWS: Mutex<Option<HashSet<Source>>> = Mutex::new(None);

// the event loop's list of what its windows' last frames read, after each dispatch
pub fn set_read_by_windows(read: HashSet<Source>) {
    let mut known = READ_BY_WINDOWS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if known.as_ref() != Some(&read) {
        *known = Some(read);
    }
}

fn read_by_a_window(source: &Source) -> bool {
    // AMANE_WAKE_ALL=1 wakes the loop for every change, as before
    static WAKE_ALL: OnceLock<bool> = OnceLock::new();
    if *WAKE_ALL.get_or_init(|| std::env::var_os("AMANE_WAKE_ALL").is_some_and(|v| v == "1")) {
        return true;
    }

    READ_BY_WINDOWS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .is_none_or(|read| read.contains(source))
}

// what the window being drawn right now has read
thread_local! {
    static READ: RefCell<HashSet<Source>> = RefCell::new(HashSet::new());
}

pub fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    if WAKE.set(Box::new(wake)).is_err() {
        panic!("failed to set wake: already set");
    }
}

// asks the event loop to draw every window again, from any thread
pub fn mark_all() {
    if timing::enabled() {
        eprintln!("change everything");
    }

    CHANGED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .everything = true;

    ping();
}

// work handed back from another thread, run on the event loop like a handler
type Job = Box<dyn FnOnce() + Send>;

static LATER: Mutex<Vec<Job>> = Mutex::new(Vec::new());

// runs `job` on the event loop at its next wake, like a paste or a picked file arriving
pub(crate) fn later(job: impl FnOnce() + Send + 'static) {
    LATER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Box::new(job));

    ping();
}

pub(crate) fn take_later() -> Vec<Job> {
    std::mem::take(&mut *LATER.lock().unwrap_or_else(PoisonError::into_inner))
}

// whether an event loop is running to be woken
pub fn has_loop() -> bool {
    WAKE.get().is_some()
}

// wakes the event loop for work that draws nothing, like a copy
pub fn wake() {
    ping();
}

// only the windows that read this service are drawn again
pub fn mark(service: TypeId) {
    mark_source(Source::Service(service));
}

// only the windows that read this source are drawn again; when none reads it, the event
// loop isn't woken at all (a terminal's title spinner with no window showing titles)
pub fn mark_source(source: Source) {
    let mut changes = CHANGED.lock().unwrap_or_else(PoisonError::into_inner);

    changes
        .services
        .get_or_insert_with(HashSet::new)
        .insert(source);

    drop(changes);

    if read_by_a_window(&source) {
        ping();
    }
}

// none means every window should draw
pub fn take() -> Option<HashSet<Source>> {
    let mut changes = CHANGED.lock().unwrap_or_else(PoisonError::into_inner);

    let services = changes.services.take().unwrap_or_default();

    if std::mem::take(&mut changes.everything) {
        return None;
    }

    Some(services)
}

pub fn note_read(service: TypeId) {
    note_source(Source::Service(service));
}

pub fn note_source(source: Source) {
    READ.with_borrow_mut(|read| read.insert(source));
}

// what was read since the last call, which starts a fresh list
pub fn take_read() -> HashSet<Source> {
    READ.take()
}

fn ping() {
    // a write before the loop exists is picked up by the first frame anyway
    let Some(wake) = WAKE.get() else {
        return;
    };

    wake();
}
