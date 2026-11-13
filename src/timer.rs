#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! Timers on the event loop: run a job on the UI thread at a time, instead of a thread that
//! sleeps for it. Callable from any thread, and before the event loop exists (the timers start
//! with it). A job that changes what windows show marks it the usual way
//! ([`mark_all`](crate::mark_all), a service write); [`redraw_after`] does only that.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::channel::{self, Channel, Sender};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer as LoopTimer};
use smithay_client_toolkit::reexports::calloop::{LoopHandle, RegistrationToken};

type Job = Box<dyn FnOnce() + Send>;

struct Request {
    id: u64,
    at: Instant,
    job: Job,
    cancelled: Arc<AtomicBool>,
}

// what the event loop is told: a timer to start, or one to take off the loop again
enum Message {
    Start(Request),
    Cancel(u64),
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Starts jobs on the event loop at their time.
pub struct Timer;

/// A timer that hasn't fired yet can be cancelled. Dropping the handle doesn't cancel it.
#[derive(Clone)]
pub struct TimerHandle {
    id: u64,
    cancelled: Arc<AtomicBool>,
    // the loop's channel, to take the timer off it: none for one asked before the loop ran
    loop_: Option<Sender<Message>>,
}

impl TimerHandle {
    /// Stops the job from running, if it hasn't yet, and takes its timer off the event loop,
    /// so the loop doesn't wake for it.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some(loop_) = &self.loop_ {
            // a loop that's gone has nothing to take off
            let _ = loop_.send(Message::Cancel(self.id));
        }
    }

    /// Whether [`cancel`](Self::cancel) was called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

impl Timer {
    /// Runs `job` on the event loop once `delay` has passed.
    pub fn after(delay: Duration, job: impl FnOnce() + Send + 'static) -> TimerHandle {
        Self::at(Instant::now() + delay, job)
    }

    /// Runs `job` on the event loop at `at`, or at its next turn if that has passed.
    pub fn at(at: Instant, job: impl FnOnce() + Send + 'static) -> TimerHandle {
        GLOBAL.schedule(at, Box::new(job))
    }
}

/// Draws every window again once `delay` has passed: for something that changes with time
/// alone, like a countdown or the end of a hover delay.
pub fn redraw_after(delay: Duration) -> TimerHandle {
    Timer::after(delay, crate::changes::mark_all)
}

static GLOBAL: Queue = Queue::new();

/// Puts the timers on the event loop; the ones asked for before it existed start now.
pub(crate) fn insert<D: 'static>(handle: &LoopHandle<'static, D>) {
    GLOBAL.attach(handle);
}

/// Where timer requests go: the event loop's channel once it is attached, and a list before.
struct Queue {
    inner: Mutex<Inner>,
}

struct Inner {
    sender: Option<Sender<Message>>,
    early: Vec<Request>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                sender: None,
                early: Vec::new(),
            }),
        }
    }

    fn schedule(&self, at: Instant, job: Job) -> TimerHandle {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = Request {
            id,
            at,
            job,
            cancelled: Arc::clone(&cancelled),
        };

        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let loop_ = inner.sender.clone();
        match &inner.sender {
            Some(sender) => {
                if let Err(err) = sender.send(Message::Start(request)) {
                    // the event loop is gone; nothing would run it anyway
                    drop(err);
                }
            }
            None => inner.early.push(request),
        }

        TimerHandle {
            id,
            cancelled,
            loop_,
        }
    }

    fn attach<D: 'static>(&self, handle: &LoopHandle<'static, D>) {
        let (sender, channel): (Sender<Message>, Channel<Message>) = channel::channel();

        // the timers on the loop now, by id, so a cancel can take one off
        let running: Running = Rc::default();

        let timers = handle.clone();
        let on_loop = Rc::clone(&running);
        let inserted = handle.insert_source(channel, move |event, _, _| {
            let channel::Event::Msg(message) = event else {
                return;
            };
            match message {
                Message::Start(request) => start(&timers, &on_loop, request),
                Message::Cancel(id) => {
                    if let Some(token) = on_loop.borrow_mut().remove(&id) {
                        timers.remove(token);
                    }
                }
            }
        });
        if let Err(err) = inserted {
            eprintln!("amane: failed to start timers: {err}");
            return;
        }

        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for request in inner.early.drain(..) {
            start(handle, &running, request);
        }
        inner.sender = Some(sender);
    }
}

type Running = Rc<RefCell<HashMap<u64, RegistrationToken>>>;

fn start<D: 'static>(handle: &LoopHandle<'static, D>, running: &Running, request: Request) {
    let Request {
        id,
        at,
        job,
        cancelled,
    } = request;
    // cancelled before the loop ran: never put on it
    if cancelled.load(Ordering::Relaxed) {
        return;
    }
    let mut job = Some(job);

    let fired = Rc::clone(running);
    let inserted = handle.insert_source(LoopTimer::from_deadline(at), move |_, _, _| {
        fired.borrow_mut().remove(&id);
        if !cancelled.load(Ordering::Relaxed)
            && let Some(job) = job.take()
        {
            job();
        }
        TimeoutAction::Drop
    });
    match inserted {
        Ok(token) => {
            running.borrow_mut().insert(id, token);
        }
        Err(err) => eprintln!("amane: failed to start a timer: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use smithay_client_toolkit::reexports::calloop::EventLoop;

    use super::*;

    /// Runs `queue`'s loop until `count` jobs have reported, or two seconds pass.
    fn run(queue: &Queue, count: usize, schedule: impl FnOnce(&Queue)) -> Vec<(u32, Duration)> {
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().expect("event loop");
        schedule(queue);
        queue.attach(&event_loop.handle());

        let (sent, seen) = mpsc::channel::<u32>();
        SENT.with(|s| *s.borrow_mut() = Some(sent));

        let start = Instant::now();
        let mut out = Vec::new();
        while out.len() < count && start.elapsed() < Duration::from_secs(2) {
            event_loop
                .dispatch(Some(Duration::from_millis(5)), &mut ())
                .expect("dispatch");
            while let Ok(id) = seen.try_recv() {
                out.push((id, start.elapsed()));
            }
        }
        out
    }

    thread_local! {
        static SENT: std::cell::RefCell<Option<mpsc::Sender<u32>>> =
            const { std::cell::RefCell::new(None) };
    }

    // jobs run on the loop's thread, which is the test's
    fn report(id: u32) -> impl FnOnce() + Send + 'static {
        move || {
            SENT.with(|sent| {
                if let Some(sent) = sent.borrow().as_ref() {
                    let _ = sent.send(id);
                }
            })
        }
    }

    #[test]
    fn jobs_run_in_time_order_on_the_loop() {
        let queue = Queue::new();
        let ran = run(&queue, 3, |queue| {
            let now = Instant::now();
            queue.schedule(now + Duration::from_millis(60), Box::new(report(3)));
            queue.schedule(now + Duration::from_millis(20), Box::new(report(1)));
            queue.schedule(now + Duration::from_millis(40), Box::new(report(2)));
        });
        let order: Vec<u32> = ran.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, [1, 2, 3]);
        assert!(
            ran[0].1 >= Duration::from_millis(15),
            "fired early: {:?}",
            ran[0].1
        );
    }

    #[test]
    fn a_cancelled_job_does_not_run() {
        let queue = Queue::new();
        // the cancelled one is due first, so it would be the first to report
        let ran = run(&queue, 1, |queue| {
            let now = Instant::now();
            let handle = queue.schedule(now + Duration::from_millis(10), Box::new(report(1)));
            handle.cancel();
            assert!(handle.is_cancelled());
            queue.schedule(now + Duration::from_millis(30), Box::new(report(2)));
        });
        let order: Vec<u32> = ran.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, [2]);
    }

    #[test]
    fn a_job_cancelled_on_the_loop_is_taken_off_it() {
        static LATE: Queue = Queue::new();
        let ran = run(&LATE, 1, |queue| {
            queue.schedule(
                Instant::now(),
                Box::new(|| {
                    // asked and cancelled once the loop runs: the cancel goes through the loop
                    let handle = LATE.schedule(
                        Instant::now() + Duration::from_millis(20),
                        Box::new(report(1)),
                    );
                    handle.cancel();
                    LATE.schedule(
                        Instant::now() + Duration::from_millis(40),
                        Box::new(report(2)),
                    );
                }),
            );
        });
        let order: Vec<u32> = ran.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, [2]);
    }

    #[test]
    fn a_job_can_schedule_another_and_late_requests_still_run() {
        static CHAINED: Queue = Queue::new();
        let ran = run(&CHAINED, 2, |queue| {
            queue.schedule(
                Instant::now() + Duration::from_millis(10),
                Box::new(|| {
                    report(1)();
                    // asked from inside a job, once the loop runs
                    CHAINED.schedule(Instant::now(), Box::new(report(2)));
                }),
            );
        });
        let order: Vec<u32> = ran.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, [1, 2]);
    }
}
