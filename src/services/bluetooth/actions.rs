#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::services::worker;
use crate::{Argument, Bus, Service};

use super::{ADAPTER, BLUEZ, Bluetooth, DEVICE, PowerState, errors};
use crate::{ErrorKind, ServiceError};

/// What a device is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Connecting, until bluez answers or 45 seconds pass.
    Connecting,
    /// Disconnecting.
    Disconnecting,
    /// Pairing, then connecting.
    Pairing,
    /// Unpairing and removing it.
    Forgetting,
}

/*
 * bluez answers a connect within this on its own, unless the device keeps
 * it hanging; then amane stops waiting and asks bluez to stop too
 */
const GIVE_UP_AFTER: Duration = Duration::from_secs(45);

// a search the radio dropped is started again at most this often
const RESTART_SCAN_EVERY: Duration = Duration::from_secs(5);

// after an rfkill unblock the radio takes a moment before it can be powered
const UNBLOCK_WAIT: Duration = Duration::from_secs(2);

static NEXT: AtomicU64 = AtomicU64::new(1);

/*
 * which run of an operation is the current one, by device path, so a run the
 * watchdog already ended doesn't end the next one when it finally answers
 */
static RUNNING: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

// pairings stopped from here, whose "canceled" isn't worth showing
static CANCELED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/*
 * a search was asked for, and is started again if the radio drops it; not
 * in the service, so a view can start one while it reads the service
 */
static SEARCHING: AtomicBool = AtomicBool::new(false);

static LAST_RESTART: Mutex<Option<Instant>> = Mutex::new(None);

/*
 * when the search was asked for: one the radio drops after this long isn't
 * started again, so a search whose stop_scan() never came (a view gone
 * without leaving) can't keep the radio scanning for good
 */
static SEARCH_STARTED: Mutex<Option<Instant>> = Mutex::new(None);

const RESTART_SCAN_FOR: Duration = Duration::from_secs(60);

impl Bluetooth {
    /// Turns on with rfkill unblocked first, like turning it on in airplane mode.
    pub fn set_powered(powered: bool) {
        worker::run(move || {
            let Some(adapter) = adapter() else {
                return;
            };

            let result = power(&adapter, powered);

            report(&adapter, result);
        });
    }

    /// New devices show up in devices() while searching, until stop_scan().
    pub fn start_scan() {
        SEARCHING.store(true, Ordering::Relaxed);
        *lock(&SEARCH_STARTED) = Some(Instant::now());

        worker::run(scan);
    }

    /// Ends a search start_scan() began; one another program started keeps going.
    pub fn stop_scan() {
        SEARCHING.store(false, Ordering::Relaxed);

        worker::run(|| {
            // bluez refuses this when another program's search is the one running, that's fine
            let _ = adapter_call("StopDiscovery", &[]);
        });
    }

    /// Visible to phones and other computers searching, and open to their
    /// pairing; bluez turns it off again after its DiscoverableTimeout.
    pub fn set_discoverable(discoverable: bool) {
        worker::run(move || {
            let Some(adapter) = adapter() else {
                return;
            };

            let bus = Bus::system();

            let set = |name: &str| {
                let value = Argument::from(discoverable);

                bus.try_set_property(BLUEZ, &adapter, ADAPTER, name, value)
            };

            // pairable first, so a phone that finds it can pair at once
            let order = if discoverable {
                ["Pairable", "Discoverable"]
            } else {
                ["Discoverable", "Pairable"]
            };

            for name in order {
                let _ = set(name);
            }
        });
    }

    /// Connects to a paired device, by its d-bus path; how it went shows in the service.
    pub fn connect(device: &str) {
        run(device, Operation::Connecting, |device, _| {
            device_call(device, "Connect")
        });
    }

    /// Disconnects the device with this d-bus path.
    pub fn disconnect(device: &str) {
        run(device, Operation::Disconnecting, |device, _| {
            device_call(device, "Disconnect")
        });
    }

    /// Pairs, trusts so it reconnects by itself later, then connects, like
    /// other bluetooth menus do; a device that needs a code asks through
    /// the agent (serve_agent).
    pub fn pair(device: &str) {
        run(device, Operation::Pairing, |device, token| {
            device_call(device, "Pair")?;

            let _ = set_device(device, "Trusted", true);

            moved_on(device, token, Operation::Connecting);

            // a phone or keyboard with nothing to connect is still paired, that's done
            match device_call(device, "Connect") {
                Err(error) if error.kind == ErrorKind::NotSupported => Ok(()),
                result => result,
            }
        });
    }

    /// Stops a pairing that's waiting, like on a code nobody is going to type.
    pub fn cancel_pairing(device: &str) {
        CANCELED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device.to_string());

        let device = device.to_string();

        thread::spawn(move || {
            let _ = device_call(&device, "CancelPairing");
        });
    }

    /// Unpairs and removes the device, it comes back only after a new search.
    pub fn forget(device: &str) {
        run(device, Operation::Forgetting, |device, _| {
            let path = Argument::Path(device.to_string());

            match adapter_call("RemoveDevice", &[path]) {
                // already gone is what was asked for
                Err(error) if error.kind == ErrorKind::Gone => Ok(()),
                result => result,
            }
        });
    }

    /// Whether it reconnects by itself when it's nearby and on.
    pub fn set_trusted(device: &str, trusted: bool) {
        set_later(device, "Trusted", Argument::from(trusted));
    }

    /// A blocked device can't connect, and any connection it has ends.
    pub fn set_blocked(device: &str, blocked: bool) {
        set_later(device, "Blocked", Argument::from(blocked));
    }

    /// The name this computer shows for it, an empty one brings back its own.
    pub fn rename(device: &str, name: &str) {
        set_later(device, "Alias", Argument::from(name.trim()));
    }
}

/*
 * runs `job` for `device` on a thread of its own, so a slow connect never
 * holds up another device or the radio switch; a device already busy
 * ignores the new ask, like a second click
 */
fn run(
    device: &str,
    operation: Operation,
    job: impl FnOnce(&str, u64) -> Result<(), ServiceError> + Send + 'static,
) {
    let Some(token) = begin(device, operation) else {
        return;
    };

    let path = device.to_string();

    thread::spawn(move || {
        let result = job(&path, token);

        finish(&path, token, result);
    });

    let path = device.to_string();

    thread::spawn(move || {
        thread::sleep(GIVE_UP_AFTER);

        if !current(&path, token) {
            return;
        }

        // bluez stops a connect or pairing still going when asked to disconnect
        if operation == Operation::Pairing {
            let _ = device_call(&path, "CancelPairing");
        }

        let _ = device_call(&path, "Disconnect");

        finish(
            &path,
            token,
            Err(ServiceError::new(ErrorKind::TimedOut, "no answer in time")),
        );
    });
}

fn begin(device: &str, operation: Operation) -> Option<u64> {
    let mut bluetooth = Bluetooth::write();

    if bluetooth.operations.contains_key(device) {
        bluetooth.quiet();

        return None;
    }

    let token = NEXT.fetch_add(1, Ordering::Relaxed);

    lock(&RUNNING).insert(device.to_string(), token);
    lock(&CANCELED).remove(device);

    bluetooth.operations.insert(device.to_string(), operation);
    bluetooth.failures.remove(device);

    Some(token)
}

fn current(device: &str, token: u64) -> bool {
    lock(&RUNNING).get(device) == Some(&token)
}

// the same run, on to its next step, like from pairing to connecting
fn moved_on(device: &str, token: u64, operation: Operation) {
    if !current(device, token) {
        return;
    }

    Bluetooth::write()
        .operations
        .insert(device.to_string(), operation);
}

fn finish(device: &str, token: u64, result: Result<(), ServiceError>) {
    {
        let mut running = lock(&RUNNING);

        // the watchdog ended this run already
        if running.get(device) != Some(&token) {
            return;
        }

        running.remove(device);
    }

    let canceled_here = lock(&CANCELED).remove(device);

    let result = match result {
        Err(error) if error.kind == ErrorKind::Canceled && canceled_here => Ok(()),
        result => result,
    };

    let mut bluetooth = Bluetooth::write();

    bluetooth.operations.remove(device);

    // a code shown for this pairing has nothing left to show
    if bluetooth
        .request
        .as_ref()
        .is_some_and(|request| request.device() == device && !request.asks())
    {
        bluetooth.request = None;
    }

    drop(bluetooth);

    if let Err(error) = result {
        fail(device, error);
    }
}

// keeps `error` on `device` (or the adapter) until its next try; how long it shows is the UI's
fn fail(device: &str, error: ServiceError) {
    Bluetooth::write()
        .failures
        .insert(device.to_string(), error);
}

// a one-off call's result: a failure kept, or an old one gone now that a try worked
fn report(device: &str, result: Result<(), ServiceError>) {
    match result {
        Err(error) => fail(device, error),
        Ok(()) => {
            let mut bluetooth = Bluetooth::write();
            if bluetooth.failures.remove(device).is_none() {
                bluetooth.quiet();
            }
        }
    }
}

fn power(adapter: &str, powered: bool) -> Result<(), ServiceError> {
    let bus = Bus::system();

    let set = || {
        bus.try_set_property(BLUEZ, adapter, ADAPTER, "Powered", Argument::from(powered))
            .or_else(ok_if_done)
    };

    if !powered || Bluetooth::read().power() != PowerState::Blocked {
        return set();
    }

    // a hardware switch can't be lifted from here, and then this stays blocked
    crate::Rfkill::set_blocked(Some("bluetooth"), false)
        .map_err(|error| ServiceError::new(ErrorKind::Blocked, error.to_string()))?;

    let since = Instant::now();

    loop {
        match set() {
            Err(error) if error.kind == ErrorKind::Blocked && since.elapsed() < UNBLOCK_WAIT => {
                thread::sleep(Duration::from_millis(100));
            }
            result => return result,
        }
    }
}

fn scan() {
    // every kind of device, and each one once rather than at every advertisement
    let filter = [
        (String::from("Transport"), Argument::from("auto")),
        (String::from("DuplicateData"), Argument::from(false)),
    ];

    let _ = adapter_call("SetDiscoveryFilter", &[Argument::Map(filter.into())]);

    let _ = adapter_call("StartDiscovery", &[]);
}

/// A search asked for, recently enough to start again when the radio drops it.
pub(super) fn searching() -> bool {
    SEARCHING.load(Ordering::Relaxed)
        && lock(&SEARCH_STARTED).is_some_and(|started| started.elapsed() < RESTART_SCAN_FOR)
}

// a search the radio dropped, like when it was turned off and on
pub(super) fn restart_scan() {
    let mut last = lock(&LAST_RESTART);

    if last.is_some_and(|last| last.elapsed() < RESTART_SCAN_EVERY) {
        return;
    }

    *last = Some(Instant::now());

    drop(last);

    worker::run(scan);
}

// looked up again on each call, so input handlers don't have to read the service
fn adapter() -> Option<String> {
    Bluetooth::read()
        .adapter
        .as_ref()
        .map(|adapter| adapter.path().to_string())
}

fn adapter_call(method: &str, arguments: &[Argument]) -> Result<(), ServiceError> {
    let Some(adapter) = adapter() else {
        return Err(ServiceError::new(ErrorKind::NotReady, "no adapter"));
    };

    Bus::system()
        .try_call(BLUEZ, &adapter, ADAPTER, method, arguments)
        .map(|_| ())
        .or_else(ok_if_done)
}

fn device_call(device: &str, method: &str) -> Result<(), ServiceError> {
    Bus::system()
        .try_call(BLUEZ, device, DEVICE, method, &[])
        .map(|_| ())
        .or_else(ok_if_done)
}

fn set_device(device: &str, name: &str, value: impl Into<Argument>) -> Result<(), ServiceError> {
    Bus::system()
        .try_set_property(BLUEZ, device, DEVICE, name, value.into())
        .or_else(ok_if_done)
}

fn set_later(device: &str, name: &'static str, value: Argument) {
    let device = device.to_string();

    worker::run(move || {
        let result = set_device(&device, name, value);

        report(&device, result);
    });
}

// a call that failed only because it was already so counts as done
fn ok_if_done(error: crate::BusError) -> Result<(), ServiceError> {
    match errors::from_bus(&error) {
        None => Ok(()),
        Some(error) => Err(error),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
