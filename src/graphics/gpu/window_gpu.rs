use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::{Rc, Weak};

use super::shared::{DEVICE_IDLE, release_device};
use super::{Gpu, KEEP_HIDDEN};
use crate::graphics::renderer::Command;

/*
 * a window's gpu, made at its first frame rather than when the window is added, so a window
 * that never shows (a hidden panel, a drop shelf waiting in the bar) makes none. Hiding
 * releases its memory and keeps its pipelines, as before, so showing again is quick. Once
 * every window of the program has been hidden for AMANE_DEVICE_IDLE seconds, every gpu goes
 * whole and the shared device and vello with them: a program waiting hidden holds no gpu
 * context, no vello and no driver memory
 */
pub(crate) struct WindowGpu {
    // libwayland's display and surface, which outlive the window
    display: *mut c_void,
    handle: *mut c_void,

    gpu: Rc<RefCell<Option<Gpu>>>,

    // whether this window counts as shown, from its first frame to its hide
    shown: bool,
}

thread_local! {
    // every window's gpu, for letting them all go at once
    static ALL: RefCell<Vec<Weak<RefCell<Option<Gpu>>>>> = const { RefCell::new(Vec::new()) };

    // how many windows show now, and which hide-all the timer waits for
    static SHOWN: Cell<usize> = const { Cell::new(0) };
    static HIDDEN_AT: Cell<u64> = const { Cell::new(0) };
}

impl WindowGpu {
    pub(crate) fn new(display: *mut c_void, handle: *mut c_void) -> Self {
        let gpu = Rc::new(RefCell::new(None));
        ALL.with_borrow_mut(|all| {
            all.retain(|weak| weak.strong_count() > 0);
            all.push(Rc::downgrade(&gpu));
        });
        Self {
            display,
            handle,
            gpu,
            shown: false,
        }
    }

    pub(crate) fn vello_runs(&self) -> u32 {
        self.gpu.borrow().as_ref().map_or(0, Gpu::vello_runs)
    }

    pub(crate) fn shows(&self, commands: &[Command], width: u32, height: u32) -> bool {
        self.gpu
            .borrow()
            .as_ref()
            .is_some_and(|gpu| gpu.shows(commands, width, height))
    }

    pub(crate) fn draw(&mut self, commands: Vec<Command>, width: u32, height: u32) -> bool {
        if !self.shown {
            self.shown = true;
            SHOWN.set(SHOWN.get() + 1);
        }
        let (display, handle) = (self.display, self.handle);
        self.gpu
            .borrow_mut()
            .get_or_insert_with(|| Gpu::new(display, handle))
            .draw(commands, width, height)
    }

    pub(crate) fn forget_shown(&mut self) {
        if let Some(gpu) = self.gpu.borrow_mut().as_mut() {
            gpu.forget_shown();
        }
    }

    pub(crate) fn release(&mut self) {
        if let Some(gpu) = self.gpu.borrow_mut().as_mut() {
            gpu.release();
        }
        self.unshow();
    }

    fn unshow(&mut self) {
        if !std::mem::take(&mut self.shown) {
            return;
        }
        let left = SHOWN.get().saturating_sub(1);
        SHOWN.set(left);
        if left > 0 || *KEEP_HIDDEN {
            return;
        }
        let Some(idle) = *DEVICE_IDLE else {
            return;
        };
        // a window shown and hidden again meanwhile starts the wait over
        let at = HIDDEN_AT.get() + 1;
        HIDDEN_AT.set(at);
        // a timer job runs on the event loop's thread, where these thread locals are
        crate::timer::Timer::after(idle, move || release_all(at));
    }
}

impl Drop for WindowGpu {
    fn drop(&mut self) {
        self.unshow();
    }
}

fn release_all(at: u64) {
    if SHOWN.get() > 0 || HIDDEN_AT.get() != at {
        return;
    }
    let gpus: Vec<_> = ALL.with_borrow(|all| all.iter().filter_map(Weak::upgrade).collect());
    let mut any = false;
    for gpu in gpus {
        any |= gpu.borrow_mut().take().is_some();
    }
    if !any {
        return;
    }
    release_device();
    if crate::timing::enabled() {
        eprintln!("released: every window's gpu and the device, no window shows");
    }
    crate::allocator::trim_soon();
}
