mod access_point;
mod connections;
mod details;
mod link;
mod manager;
mod profile;
mod signals;
mod wifi;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::services::{IDLE_STOP, store, worker};
use crate::{Bus, ErrorKind, Service, ServiceError, Value};

pub use access_point::AccessPoint;
pub use connections::{Profile as NetworkProfile, Wired};
pub use details::{ConnectionDetails, Ipv4Settings};
pub use link::Link;

// a burst of signals is read as one change: the refetch waits until they pause this long
const SETTLE: Duration = Duration::from_millis(250);
// but no longer than this from the first
const BURST: Duration = Duration::from_secs(1);
// a refetch without a signal, in case one went missing, only while something watches
const RESYNC: Duration = Duration::from_secs(60);
// strength and scan noise from networks not joined is read only while the list of networks was
// read this recently, by a window that shows it
const LISTED_FOR: Duration = Duration::from_secs(30);

// the joined access point's object path, for sorting the signals
static JOINED: Mutex<String> = Mutex::new(String::new());
// when the list of networks was last read, in milliseconds since START
static LISTED_AT: AtomicU64 = AtomicU64::new(u64::MAX);
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Default)]
pub struct Network {
    link: Link,

    // empty on a wired link
    ssid: String,

    // 0 to 100, and 0 on a wired link
    strength: u8,

    wifi_enabled: bool,

    // joining a wifi network that isn't up yet
    connecting: bool,

    // empty without a wifi device, or with the radio off
    access_points: Vec<AccessPoint>,

    // every saved profile, and whether each is up
    profiles: Vec<NetworkProfile>,

    wired: Option<Wired>,

    // the joined access point's object path, empty when none
    joined: String,

    // the network last asked to join, while it may still be joining
    joining: Option<String>,

    // why the last try failed, by network name, profile UUID or interface, until the next try
    failures: BTreeMap<String, ServiceError>,
}

/// What a network is in the middle of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkOperation {
    Connecting,
}

/*
 * follows networkmanager's signals: a device, an access point or a connection changing sends
 * one, and the profiles' settings are read again only when one of those changed
 */
impl Service for Network {
    fn new() -> Self {
        fetch(&connections::saved())
    }

    /*
     * networkmanager is asked outside the lock, so a slow answer never
     * holds up a frame, and nothing is written when nothing changed, so
     * the windows aren't redrawn for no reason
     */
    fn listen() {
        let (sender, signals) = mpsc::channel::<Change>();
        thread::spawn(move || {
            for signal in Bus::system().signals_from(manager::NAME) {
                let joined = JOINED
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let change = signals::classify(
                    signal.path(),
                    signal.interface(),
                    signal.name(),
                    signal.arguments(),
                    connections::SETTINGS_PATH,
                    &joined,
                );
                if let Some(failure) =
                    signals::device_failure(signal.interface(), signal.name(), signal.arguments())
                {
                    joining_failed(failure);
                }
                if change != Change::Nothing && sender.send(change).is_err() {
                    break;
                }
            }
        });

        let mut saved = connections::saved();
        loop {
            // Some(settings too) once a full read is due
            let mut wanted = None;
            match signals.recv_timeout(RESYNC) {
                Ok(change) => take(change, &mut wanted),
                Err(RecvTimeoutError::Timeout) => {
                    if store::unwatched::<Self>(IDLE_STOP) {
                        continue;
                    }
                    wanted = Some(true);
                }
                // no system bus: nothing to follow
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let Some(mut settings) = wanted else {
                continue;
            };

            let first = Instant::now();
            while first.elapsed() < BURST {
                match signals.recv_timeout(SETTLE) {
                    Ok(change) => take(change, &mut wanted),
                    Err(_) => break,
                }
            }
            settings |= wanted.unwrap_or(false);

            if settings {
                saved = connections::saved();
            }
            let fresh = fetch(&saved);
            JOINED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone_from(&fresh.joined);

            // read as the service's own, which doesn't count as watching it
            let same = store::find::<Self>()
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .shows_same(&fresh);

            if same {
                continue;
            }

            let mut network = Self::write();
            let failures = std::mem::take(&mut network.failures);
            let joining = network.joining.take();
            *network = fresh;
            network.failures = failures;
            network.joining = joining;
        }
    }
}

use signals::Change;

// one sorted signal: a full read noted in `wanted`, or a strength applied straight away
fn take(change: Change, wanted: &mut Option<bool>) {
    match change {
        Change::Full { settings } => *wanted = Some(wanted.unwrap_or(false) | settings),
        Change::Strength(strength) => set_strength(strength),
        Change::Listed => {
            let listed = LISTED_AT.load(Ordering::Relaxed);
            let now = millis_since_start();
            if listed != u64::MAX && now.saturating_sub(listed) < LISTED_FOR.as_millis() as u64 {
                wanted.get_or_insert(false);
            }
        }
        Change::Nothing => {}
    }
}

// the joined network's strength, redrawing only when its wedge changes
fn set_strength(strength: u8) {
    let mut network = Network::write();
    let same = step(network.strength) == step(strength);
    network.strength = strength;
    if let Some(joined) = network.access_points.iter_mut().find(|point| point.active) {
        joined.strength = strength;
    }
    if same {
        network.quiet();
    }
}

fn millis_since_start() -> u64 {
    START.elapsed().as_millis() as u64
}

// a signal shows as the nearest of five wedges (kit's wifi_glyph), 20 apart
fn step(strength: u8) -> u16 {
    (u16::from(strength.min(100)) * 5 + 50) / 100
}

impl Network {
    /*
     * whether `other` shows the same: strength wavers all the time, and a
     * change within a step would redraw everything that shows the network
     * for nothing
     */
    fn shows_same(&self, other: &Self) -> bool {
        let same_points = self.access_points.len() == other.access_points.len()
            && self
                .access_points
                .iter()
                .zip(&other.access_points)
                .all(|(a, b)| {
                    a.ssid == b.ssid
                        && step(a.strength) == step(b.strength)
                        && a.secured == b.secured
                        && a.active == b.active
                        && a.saved == b.saved
                });

        same_points
            && self.link == other.link
            && self.ssid == other.ssid
            && step(self.strength) == step(other.strength)
            && self.wifi_enabled == other.wifi_enabled
            && self.connecting == other.connecting
            && self.profiles == other.profiles
            && self.wired == other.wired
            && self.joined == other.joined
    }

    pub fn connected(&self) -> bool {
        self.link != Link::Offline
    }

    pub fn link(&self) -> Link {
        self.link
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    pub fn strength(&self) -> u8 {
        self.strength
    }

    pub fn wifi_enabled(&self) -> bool {
        self.wifi_enabled
    }

    pub fn connecting(&self) -> bool {
        self.connecting
    }

    /// What the network named `ssid` is in the middle of, if anything.
    pub fn operation(&self, ssid: &str) -> Option<NetworkOperation> {
        (self.connecting && self.joining.as_deref() == Some(ssid))
            .then_some(NetworkOperation::Connecting)
    }

    /// Why the last try on a network (by name), profile (by UUID) or device (by interface)
    /// failed, until the next try. The UI words it and decides how long it shows.
    pub fn failure(&self, key: &str) -> Option<&ServiceError> {
        self.failures.get(key)
    }

    pub fn access_points(&self) -> &[AccessPoint] {
        // a window lists them: strength changes are worth reading while it does
        LISTED_AT.store(millis_since_start(), Ordering::Relaxed);

        &self.access_points
    }

    /// Every saved connection profile, with whether each is up.
    pub fn profiles(&self) -> &[NetworkProfile] {
        &self.profiles
    }

    /// The VPN and WireGuard profiles.
    pub fn vpns(&self) -> impl Iterator<Item = &NetworkProfile> {
        self.profiles.iter().filter(|profile| profile.is_vpn())
    }

    /// The hotspot this machine runs, if any.
    pub fn hotspot(&self) -> Option<&NetworkProfile> {
        self.profiles.iter().rfind(|profile| profile.is_hotspot())
    }

    /// The first Ethernet device with a cable in.
    pub fn wired(&self) -> Option<&Wired> {
        self.wired.as_ref()
    }
}

/*
 * these talk to networkmanager right away, and the
 * next update shows what changed
 */
impl Network {
    // new networks show up in access_points a few seconds later
    pub fn scan() {
        worker::run(|| {
            let Some(device) = wifi::device() else {
                return;
            };

            wifi::scan(&device);
        });
    }

    pub fn connect(ssid: &str, password: Option<&str>) {
        let ssid = String::from(ssid);
        let password = password.map(String::from);

        worker::run(move || {
            begin(&ssid, true);
            let result = join(&ssid, password.as_deref(), false);
            finish(&ssid, result);
        });
    }

    /// Joins a network that doesn't announce its name, saving a profile that
    /// looks for it; a failure is under [`failure`](Self::failure) by `ssid`.
    pub fn connect_hidden(ssid: &str, password: Option<&str>) {
        let ssid = String::from(ssid);
        let password = password.map(String::from);

        worker::run(move || {
            begin(&ssid, true);
            let result = join(&ssid, password.as_deref(), true);
            finish(&ssid, result);
        });
    }

    /// Deletes the saved profile with `uuid`, password and all; a failure,
    /// like a profile the system's configuration declares, is under
    /// [`failure`](Self::failure) by `uuid`.
    pub fn forget(uuid: &str) {
        let uuid = uuid.to_owned();
        worker::run(move || {
            begin(&uuid, false);
            finish(&uuid, details::forget(&uuid));
        });
    }

    /// The connection with `uuid` as it is now (its addresses, gateway, DNS
    /// and device), or none while it's down. It asks NetworkManager there
    /// and then, so call it off the UI thread.
    pub fn details(uuid: &str) -> Option<ConnectionDetails> {
        details::read(uuid)
    }

    pub fn disconnect() {
        worker::run(|| {
            let Some(device) = wifi::device() else {
                return;
            };

            wifi::disconnect(&device);
        });
    }

    pub fn set_wifi(enabled: bool) {
        worker::run(move || manager::set_wifi(enabled));
    }

    /// Brings a profile up by its UUID, like a VPN or a hotspot.
    pub fn activate(uuid: &str) {
        let uuid = uuid.to_owned();
        worker::run(move || {
            begin(&uuid, false);
            finish(&uuid, connections::activate(&uuid));
        });
    }

    /// Takes a profile down by its UUID.
    pub fn deactivate(uuid: &str) {
        let uuid = uuid.to_owned();
        worker::run(move || {
            begin(&uuid, false);
            finish(&uuid, connections::deactivate(&uuid));
        });
    }

    /// Connects a device, like the [`wired`](Self::wired) one, with its best profile, or
    /// disconnects it.
    pub fn set_device(interface: &str, connected: bool) {
        let interface = interface.to_owned();
        worker::run(move || {
            begin(&interface, false);
            finish(&interface, connections::set_device(&interface, connected));
        });
    }
}

/*
 * a saved profile already holds its password, so it is joined as it is;
 * otherwise networkmanager saves a new one, password included
 */
fn join(ssid: &str, password: Option<&str>, hidden: bool) -> Result<(), ServiceError> {
    let Some(device) = wifi::device() else {
        return Err(ServiceError::new(ErrorKind::NotReady, "no Wi-Fi device"));
    };

    if !hidden && let Some(profile) = profile::find(ssid) {
        return Ok(manager::activate(&profile, &device)?);
    }

    let settings = profile::settings(ssid, password, hidden);

    Ok(manager::add_and_activate(settings, &device)?)
}

// a new try on `key`: its last failure goes, and a network to join is remembered
fn begin(key: &str, joining: bool) {
    let mut network = Network::write();
    network.failures.remove(key);
    if joining {
        network.joining = Some(key.to_owned());
    }
}

fn finish(key: &str, result: Result<(), ServiceError>) {
    let Err(error) = result else {
        return;
    };
    let mut network = Network::write();
    if network.joining.as_deref() == Some(key) {
        network.joining = None;
    }
    network.failures.insert(key.to_owned(), error);
}

// the device gave up joining, like on a wrong password: the network being joined failed
fn joining_failed(failure: ServiceError) {
    let mut network = Network::write();
    let Some(ssid) = network.joining.take() else {
        network.quiet();
        return;
    };
    network.failures.insert(ssid, failure);
}

// the ssid comes as raw bytes, and is almost always utf-8
fn ssid(bytes: &Value) -> String {
    let mut raw = Vec::new();

    for byte in bytes.list() {
        raw.push(byte.number() as u8);
    }

    String::from_utf8_lossy(&raw).into_owned()
}

// asks networkmanager for everything shown, with the profiles' settings as last read
fn fetch(saved: &[connections::Saved]) -> Network {
    // networks to join are listed even while offline
    let mut network = Network {
        wifi_enabled: manager::wifi_enabled(),
        profiles: connections::profiles(saved, &connections::active()),
        wired: connections::wired(&connections::devices()),
        ..Network::default()
    };

    if let Some(device) = wifi::device() {
        network.access_points = wifi::access_points(&device);
        network.connecting = wifi::connecting(&device);
    }

    if !manager::connected() {
        return network;
    }

    let connection = manager::primary_connection();

    network.link = Link::from_type(connection.get("Type").text());

    if network.link != Link::Wifi {
        return network;
    }

    let joined = connection.get("SpecificObject").text();
    network.joined = joined.to_owned();
    let access_point = manager::access_point(joined);

    network.ssid = ssid(access_point.get("Ssid"));
    network.strength = access_point.get("Strength").number() as u8;

    network
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(strength: u8) -> Network {
        Network {
            link: Link::Wifi,
            ssid: String::from("home"),
            strength,
            ..Network::default()
        }
    }

    #[test]
    fn a_signal_wavering_within_its_wedge_shows_the_same() {
        assert!(at(52).shows_same(&at(68)));
        assert!(
            !at(68).shows_same(&at(71)),
            "70 is where the next wedge starts"
        );
        assert!(!at(52).shows_same(&Network {
            ssid: String::from("work"),
            ..at(52)
        }));
    }
}
