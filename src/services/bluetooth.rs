mod actions;
mod adapter;
mod agent;
mod device;
mod errors;
mod model;

use std::collections::BTreeMap;
use std::sync::{Mutex, Once, PoisonError};
use std::thread;
use std::time::Duration;

use crate::{Argument, Bus, Service, ServiceError};

pub use actions::Operation;
pub use adapter::{BluetoothAdapter, PowerState};
pub use agent::{PairingAnswer, PairingKind, PairingRequest};
pub use device::BluetoothDevice;

use model::Objects;

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// bluez is asked for everything again this often while something watches, in case a signal
// was missed
const RESYNC_EVERY: Duration = Duration::from_secs(300);

/*
 * what bluez has, kept up to date from its signals; the service shows what
 * this reads as, so a change to something nothing shows redraws nothing
 */
static OBJECTS: Mutex<Objects> = Mutex::new(Objects::new());

// bluez's unique name like :1.7, so only its signals change the tree
static OWNER: Mutex<String> = Mutex::new(String::new());

#[derive(Default)]
pub struct Bluetooth {
    // none when the machine has no bluetooth or bluez isn't running
    adapter: Option<BluetoothAdapter>,

    devices: Vec<BluetoothDevice>,

    // what each device is in the middle of, by its path
    operations: BTreeMap<String, Operation>,

    // why each device's last action failed, by its path, until it's tried again
    failures: BTreeMap<String, ServiceError>,

    // a device asking to pair, waiting for an answer; only with serve_agent()
    request: Option<PairingRequest>,
}

/*
 * one read of every object at the start, then bluez's own signals: objects
 * added and removed, and properties changed
 */
impl Service for Bluetooth {
    fn new() -> Self {
        let mut bluetooth = Self::default();

        let objects = fetch();

        (bluetooth.adapter, bluetooth.devices) = objects.view();

        *OBJECTS.lock().unwrap_or_else(PoisonError::into_inner) = objects;

        bluetooth
    }

    fn listen() {
        let bus = Bus::system();

        static WATCH: Once = Once::new();

        WATCH.call_once(|| {
            // bluez restarting is a new tree, and a new agent registration
            thread::spawn(move || {
                for _ in bus.owner_changes(BLUEZ) {
                    resync();

                    agent::bluez_restarted();
                }
            });

            thread::spawn(|| {
                loop {
                    thread::sleep(RESYNC_EVERY);

                    if !crate::services::store::unwatched::<Bluetooth>(RESYNC_EVERY) {
                        resync();
                    }
                }
            });
        });

        // watched before reading, so nothing that happens in between is missed
        let signals = bus.signals_from(BLUEZ);

        resync();

        for signal in signals {
            // the connection is shared, other services' signals come by too
            if signal.sender()
                != OWNER
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_str()
            {
                continue;
            }

            let mut objects = OBJECTS.lock().unwrap_or_else(PoisonError::into_inner);

            match (signal.interface(), signal.name()) {
                (OBJECT_MANAGER, "InterfacesAdded") => objects.added(signal.arguments()),
                (OBJECT_MANAGER, "InterfacesRemoved") => objects.removed(signal.arguments()),
                (PROPERTIES, "PropertiesChanged") => {
                    objects.changed(signal.path(), signal.arguments());
                }
                _ => continue,
            }

            publish(&objects);
        }
    }
}

impl Bluetooth {
    pub fn available(&self) -> bool {
        self.adapter.is_some()
    }

    pub fn adapter(&self) -> Option<&BluetoothAdapter> {
        self.adapter.as_ref()
    }

    pub fn power(&self) -> PowerState {
        self.adapter
            .as_ref()
            .map_or(PowerState::Off, BluetoothAdapter::power)
    }

    pub fn powered(&self) -> bool {
        self.power() == PowerState::On
    }

    pub fn scanning(&self) -> bool {
        self.adapter
            .as_ref()
            .is_some_and(BluetoothAdapter::discovering)
    }

    pub fn discoverable(&self) -> bool {
        self.adapter
            .as_ref()
            .is_some_and(BluetoothAdapter::discoverable)
    }

    pub fn devices(&self) -> &[BluetoothDevice] {
        &self.devices
    }

    pub fn device(&self, path: &str) -> Option<&BluetoothDevice> {
        self.devices.iter().find(|device| device.path() == path)
    }

    // what `device` (a path) is in the middle of, none when it's idle
    pub fn operation(&self, device: &str) -> Option<Operation> {
        self.operations.get(device).copied()
    }

    /// Why the last action on `device` failed, until it's tried again; the adapter's path for
    /// turning it on. How long it shows, and in what words, is the UI's choice.
    pub fn failure(&self, device: &str) -> Option<&ServiceError> {
        self.failures.get(device)
    }

    pub fn pairing_request(&self) -> Option<&PairingRequest> {
        self.request.as_ref()
    }
}

// bluez's whole tree, empty when it isn't running
fn fetch() -> Objects {
    let bus = Bus::system();

    let owner = bus.call(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        &[Argument::from(BLUEZ)],
    );

    *OWNER.lock().unwrap_or_else(PoisonError::into_inner) = owner.text().to_string();

    let managed = bus.call(BLUEZ, "/", OBJECT_MANAGER, "GetManagedObjects", &[]);

    Objects::from_managed(&managed)
}

// reads everything again, after bluez restarted or in case a signal was missed
fn resync() {
    let fresh = fetch();

    let mut objects = OBJECTS.lock().unwrap_or_else(PoisonError::into_inner);

    *objects = fresh;

    publish(&objects);
}

/*
 * shows what the tree reads as, writing only when that changed, and keeps
 * a search going that the radio dropped; called with the tree locked, so
 * two changes are always shown in the order they happened
 */
fn publish(objects: &Objects) {
    let (adapter, devices) = objects.view();

    // the listener's own look, which keeps nothing watching the service
    let bluetooth = Bluetooth::peek();

    if bluetooth.adapter == adapter && bluetooth.devices == devices {
        return;
    }

    drop(bluetooth);

    let mut bluetooth = Bluetooth::write();

    let gone = |path: &str| {
        adapter
            .as_ref()
            .is_none_or(|adapter| adapter.path() != path)
            && !devices.iter().any(|device| device.path() == path)
    };

    // a device that's gone has nothing left to wait for
    bluetooth.operations.retain(|path, _| !gone(path));
    bluetooth.failures.retain(|path, _| !gone(path));

    if bluetooth
        .request
        .as_ref()
        .is_some_and(|request| gone(request.device()))
    {
        bluetooth.request = None;
    }

    bluetooth.adapter = adapter;
    bluetooth.devices = devices;

    // turning the radio off and on, or bluez giving up, ends a search
    let restart = actions::searching() && bluetooth.powered() && !bluetooth.scanning();

    drop(bluetooth);

    if restart {
        actions::restart_scan();
    }
}
