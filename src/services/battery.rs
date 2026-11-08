mod device;
mod upower;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread;
use std::time::Duration;

use crate::{Bus, Service, Value};

pub use device::{BatteryState, DeviceKind, PowerDevice, WarningLevel};

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

#[derive(Default, PartialEq)]
pub struct Battery {
    // read from upower, picked once at the start; otherwise from the kernel
    upower: bool,

    // none on a desktop without a battery, and when upower is used
    path: Option<PathBuf>,

    present: bool,

    // 0 to 100
    percent: u8,

    charging: bool,

    // plugged in and not charging, usually because it's full
    full: bool,

    state: BatteryState,

    // a freedesktop icon name like "battery-good-charging-symbolic", only from upower
    icon: String,

    // none while upower is still estimating, and always without upower
    time_to_empty: Option<Duration>,

    time_to_full: Option<Duration>,

    warning: WarningLevel,

    on_ac: bool,

    // mice, keyboards, headsets, only from upower
    peripherals: Vec<PowerDevice>,
}

/*
 * upower announces every change with a signal, so nothing is polled;
 * without upower the kernel only exposes the charge as files, so those are polled
 */
impl Service for Battery {
    fn new() -> Self {
        if let Some(battery) = upower::fetch() {
            return battery;
        }

        let path = find();

        let mut battery = Self {
            present: path.is_some(),
            path,
            ..Self::default()
        };

        battery.update();

        battery
    }

    fn interval() -> Duration {
        Duration::from_secs(5)
    }

    fn update(&mut self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };

        let before = (self.percent, self.state, self.on_ac);

        let capacity = read(path, "capacity");
        let status = read(path, "status");

        self.percent = capacity.parse().unwrap_or(0);

        self.state = match status.as_str() {
            "Charging" => BatteryState::Charging,
            "Discharging" => BatteryState::Discharging,
            "Full" => BatteryState::Full,
            "Not charging" => BatteryState::PendingCharge,
            _ => BatteryState::Unknown,
        };

        self.charging = self.state == BatteryState::Charging;
        self.full = matches!(self.state, BatteryState::Full | BatteryState::PendingCharge);
        /*
         * the adapter says whether it's plugged in; the battery's status alone can't, it reads
         * "Discharging" or "Unknown" on some laptops with the charger in
         */
        self.on_ac = mains_online(Path::new(POWER_SUPPLIES)).unwrap_or(self.charging || self.full);

        (self.percent, self.state, self.on_ac) != before
    }

    fn listen() {
        if !Self::read().upower {
            poll();

            return;
        }

        let bus = Bus::system();

        // devices coming and going, like a mouse turned on, each wait on their own thread
        static WATCH_DEVICES: Once = Once::new();

        WATCH_DEVICES.call_once(|| {
            for member in ["DeviceAdded", "DeviceRemoved"] {
                thread::spawn(move || {
                    for _ in bus.signals(upower::UPOWER, member) {
                        refresh();
                    }
                });
            }
        });

        let changes = bus.signals_at(
            upower::ROOT,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
        );

        // a change between new() and watching would otherwise wait for the next one
        refresh();

        for signal in changes {
            // the bus sends only what's under upower's root; only its devices count
            let interface = signal.arguments().first().map(Value::text);

            if interface != Some(upower::UPOWER) && interface != Some(upower::DEVICE) {
                continue;
            }

            refresh();
        }
    }
}

impl Battery {
    pub fn present(&self) -> bool {
        self.present
    }

    pub fn percent(&self) -> u8 {
        self.percent
    }

    pub fn charging(&self) -> bool {
        self.charging
    }

    pub fn full(&self) -> bool {
        self.full
    }

    pub fn state(&self) -> BatteryState {
        self.state
    }

    pub fn icon(&self) -> &str {
        &self.icon
    }

    pub fn time_to_empty(&self) -> Option<Duration> {
        self.time_to_empty
    }

    pub fn time_to_full(&self) -> Option<Duration> {
        self.time_to_full
    }

    pub fn warning(&self) -> WarningLevel {
        self.warning
    }

    pub fn on_ac(&self) -> bool {
        self.on_ac
    }

    pub fn peripherals(&self) -> &[PowerDevice] {
        &self.peripherals
    }
}

// the same loop every polled service runs
fn poll() {
    loop {
        thread::sleep(Battery::interval());

        let mut battery = Battery::write();

        if !battery.update() {
            battery.quiet();
        }
    }
}

// upower is asked outside the lock, and nothing is written when nothing changed
fn refresh() {
    let Some(fresh) = upower::fetch() else {
        return;
    };

    if *Battery::read() == fresh {
        return;
    }

    *Battery::write() = fresh;
}

// the first battery that powers the computer, an AC adapter or a mouse's battery is skipped
fn find() -> Option<PathBuf> {
    let entries = fs::read_dir(POWER_SUPPLIES).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();

        if read(&path, "type") != "Battery" {
            continue;
        }

        if read(&path, "scope") == "Device" {
            continue;
        }

        return Some(path);
    }

    None
}

// whether a mains adapter is plugged in; none when the kernel lists no adapter
fn mains_online(supplies: &Path) -> Option<bool> {
    let mut adapters = fs::read_dir(supplies)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| read(path, "type") == "Mains")
        .peekable();

    adapters.peek()?;

    Some(adapters.any(|path| read(&path, "online") == "1"))
}

fn read(folder: &Path, name: &str) -> String {
    let text = fs::read_to_string(folder.join(name)).unwrap_or_default();

    String::from(text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, kind: &str, online: &str) {
        let folder = root.join(name);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("type"), format!("{kind}\n")).unwrap();
        fs::write(folder.join("online"), format!("{online}\n")).unwrap();
    }

    #[test]
    fn the_adapter_says_whether_it_is_plugged_in() {
        let root = std::env::temp_dir().join(format!("amane-supplies-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        // no adapter listed: the battery's status has to do
        fs::create_dir_all(&root).unwrap();
        assert_eq!(mains_online(&root), None);

        supply(&root, "BAT0", "Battery", "");
        supply(&root, "AC", "Mains", "0");
        assert_eq!(mains_online(&root), Some(false));

        supply(&root, "AC", "Mains", "1");
        assert_eq!(mains_online(&root), Some(true));

        let _ = fs::remove_dir_all(&root);
    }
}
