use std::ffi::c_int;
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

// glibc's mallopt settings: the size served by its own mapping, and how many pools threads share
const M_MMAP_THRESHOLD: c_int = -3;
const M_ARENA_MAX: c_int = -8;

const ONE_MEGABYTE: c_int = 1024 * 1024;

const POOLS: c_int = 2;

// how long after a big release the pools are trimmed, so a burst of releases trims once
const SETTLE: Duration = Duration::from_secs(2);

// set by `trim_soon`, taken by the trimming thread
static ASKED: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

unsafe extern "C" {
    fn mallopt(param: c_int, value: c_int) -> c_int;

    fn malloc_trim(pad: usize) -> c_int;
}

/*
 * glibc raises the mapping size after the first big block is freed, so
 * later decode buffers stay in a pool after they are freed; fixing it
 * keeps big blocks on their own mappings, which go back to the system as
 * soon as an image is done with them. each new thread also gets a pool of
 * its own, so every image decoded on its own thread left freed memory
 * behind in yet another pool; two pools let that memory be used again
 */
pub fn limit() {
    let mapped = unsafe { mallopt(M_MMAP_THRESHOLD, ONE_MEGABYTE) };
    let pooled = unsafe { mallopt(M_ARENA_MAX, POOLS) };

    assert!(
        mapped == 1 && pooled == 1,
        "failed to set the allocator's limits"
    );

    let _ = thread::Builder::new()
        .name(String::from("amane-trim"))
        .spawn(trim);
}

/*
 * a pool only shrinks from its end, so the gaps decoding, d-bus and niri's
 * events leave in the middle stay with the process; trimming gives them back.
 * it runs once after something big let go (a window hiding, images forgotten),
 * not on a clock, so an idle process never wakes for it
 */
pub(crate) fn trim_soon() {
    *ASKED.lock().unwrap_or_else(PoisonError::into_inner) = true;
    WAKE.notify_one();
}

fn trim() {
    loop {
        let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
        while !*asked {
            asked = WAKE.wait(asked).unwrap_or_else(PoisonError::into_inner);
        }
        *asked = false;
        drop(asked);

        // what's freed right after the ask (the rest of the hide) goes in the same trim
        thread::sleep(SETTLE);
        *ASKED.lock().unwrap_or_else(PoisonError::into_inner) = false;

        unsafe { malloc_trim(0) };
    }
}
