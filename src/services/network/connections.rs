#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! NetworkManager's connection profiles and the wired device, beyond the Wi-Fi the rest of
//! [`Network`](super::Network) covers: VPN and WireGuard profiles, a hotspot (a Wi-Fi profile
//! in access-point mode), and the first Ethernet device with a cable in.

use crate::{Argument, Bus, ErrorKind, ServiceError, Value};

use super::details::{self, Ipv4Settings};
use super::manager::{self, NAME, PATH};

pub(super) const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";

// NetworkManager's device type and states
const ETHERNET: f64 = 1.0;
const UNMANAGED: f64 = 10.0;
const UNAVAILABLE: f64 = 20.0;
const ACTIVATED: f64 = 100.0;

/// A saved connection profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// NetworkManager's id for it, the same across renames.
    pub uuid: String,
    /// Its name, like "Home" or "Wired connection 1".
    pub name: String,
    /// NetworkManager's type for it: `802-3-ethernet`, `802-11-wireless`, `vpn`, `wireguard`
    /// and so on.
    pub kind: String,
    /// A Wi-Fi profile's mode: `infrastructure`, `ap` (a hotspot) or `adhoc`.
    pub wifi_mode: Option<String>,
    /// Whether it's up now.
    pub active: bool,
    /// A Wi-Fi profile's network name.
    pub ssid: Option<String>,
    /// Whether NetworkManager brings it up by itself when it can.
    pub autoconnect: bool,
    /// When it was last up, in seconds since 1970; 0 for never.
    pub last_used: u64,
    /// Whether the system's configuration writes it at boot (NixOS's
    /// `ensureProfiles`, say), so forgetting or changing it here wouldn't last.
    pub declared: bool,
    /// Its IPv4 settings as saved.
    pub ipv4: Ipv4Settings,
}

impl Profile {
    /// A VPN, as GNOME counts them: a `vpn` or `wireguard` profile. A tunnel NetworkManager
    /// doesn't manage, like tailscale's, isn't one.
    pub fn is_vpn(&self) -> bool {
        self.kind == "vpn" || self.kind == "wireguard"
    }

    /// A hotspot this machine is running: an active Wi-Fi profile in access-point mode.
    pub fn is_hotspot(&self) -> bool {
        self.active && self.kind == "802-11-wireless" && self.wifi_mode.as_deref() == Some("ap")
    }
}

/// An Ethernet device with a cable in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wired {
    /// Its interface, like `enp2s0`.
    pub device: String,
    /// Whether the connection on it is up.
    pub connected: bool,
    /// The connection on it, or the interface when there's none.
    pub name: String,
}

/// A profile's settings, read once and kept until NetworkManager says they changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Saved {
    pub uuid: String,
    pub name: String,
    pub kind: String,
    pub wifi_mode: Option<String>,
    pub ssid: Option<String>,
    pub autoconnect: bool,
    pub last_used: u64,
    pub declared: bool,
    pub ipv4: Ipv4Settings,
}

/// Every saved profile's settings.
pub(super) fn saved() -> Vec<Saved> {
    let bus = Bus::system();
    let list = bus.call(NAME, SETTINGS_PATH, SETTINGS, "ListConnections", &[]);
    list.list()
        .iter()
        .filter_map(|path| {
            let settings = bus.call(NAME, path.text(), CONNECTION, "GetSettings", &[]);
            let file = bus.property(NAME, path.text(), CONNECTION, "Filename");
            parse_settings(&settings).map(|saved| Saved {
                declared: details::declared(file.text()),
                ..saved
            })
        })
        .collect()
}

pub(super) fn parse_settings(settings: &Value) -> Option<Saved> {
    let connection = settings.get("connection");
    let uuid = connection.get("uuid").text();
    if uuid.is_empty() {
        return None;
    }
    let wireless = settings.get("802-11-wireless");
    let mode = wireless.get("mode").text();
    let ssid = super::ssid(wireless.get("ssid"));
    // NetworkManager leaves defaults out: autoconnect is on unless it says off
    let autoconnect = !matches!(connection.get("autoconnect"), Value::Bool(false));
    Some(Saved {
        uuid: uuid.to_owned(),
        name: connection.get("id").text().to_owned(),
        kind: connection.get("type").text().to_owned(),
        wifi_mode: (!mode.is_empty()).then(|| mode.to_owned()),
        ssid: (!ssid.is_empty()).then_some(ssid),
        autoconnect,
        last_used: connection.get("timestamp").number() as u64,
        declared: false,
        ipv4: details::ipv4_settings(settings.get("ipv4")),
    })
}

/// The UUIDs of the connections up now, with their active-connection paths.
pub(super) fn active() -> Vec<(String, String)> {
    let bus = Bus::system();
    let paths = bus.property(NAME, PATH, NAME, "ActiveConnections");
    paths
        .list()
        .iter()
        .filter_map(|path| {
            let uuid = bus.property(NAME, path.text(), ACTIVE, "Uuid");
            let uuid = uuid.text();
            (!uuid.is_empty()).then(|| (uuid.to_owned(), path.text().to_owned()))
        })
        .collect()
}

pub(super) fn profiles(saved: &[Saved], active: &[(String, String)]) -> Vec<Profile> {
    saved
        .iter()
        .map(|saved| Profile {
            uuid: saved.uuid.clone(),
            name: saved.name.clone(),
            kind: saved.kind.clone(),
            wifi_mode: saved.wifi_mode.clone(),
            active: active.iter().any(|(uuid, _)| *uuid == saved.uuid),
            ssid: saved.ssid.clone(),
            autoconnect: saved.autoconnect,
            last_used: saved.last_used,
            declared: saved.declared,
            ipv4: saved.ipv4.clone(),
        })
        .collect()
}

/// One device as NetworkManager lists it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Device {
    pub interface: String,
    pub kind: f64,
    pub state: f64,
    /// The name of the connection on it, if any.
    pub connection: String,
}

pub(super) fn devices() -> Vec<Device> {
    let bus = Bus::system();
    let paths = bus.call(NAME, PATH, NAME, "GetDevices", &[]);
    paths
        .list()
        .iter()
        .map(|path| {
            let path = path.text();
            let all = manager::properties(path, DEVICE);
            let active = all.get("ActiveConnection").text();
            let connection = if active.is_empty() || active == "/" {
                String::new()
            } else {
                bus.property(NAME, active, ACTIVE, "Id").text().to_owned()
            };
            Device {
                interface: all.get("Interface").text().to_owned(),
                kind: all.get("DeviceType").number(),
                state: all.get("State").number(),
                connection,
            }
        })
        .collect()
}

/// The first Ethernet device with a cable in: GNOME hides one without.
pub(super) fn wired(devices: &[Device]) -> Option<Wired> {
    devices
        .iter()
        .find(|device| {
            device.kind == ETHERNET && device.state != UNAVAILABLE && device.state != UNMANAGED
        })
        .map(|device| Wired {
            device: device.interface.clone(),
            connected: device.state == ACTIVATED,
            name: if device.connection.is_empty() {
                device.interface.clone()
            } else {
                device.connection.clone()
            },
        })
}

/// Brings the profile with `uuid` up, on whatever device NetworkManager picks.
pub(super) fn activate(uuid: &str) -> Result<(), ServiceError> {
    let bus = Bus::system();
    let path = bus.try_call(
        NAME,
        SETTINGS_PATH,
        SETTINGS,
        "GetConnectionByUuid",
        &[Argument::from(uuid)],
    )?;
    let path = path.text();
    if path.is_empty() {
        return Err(ServiceError::new(ErrorKind::Gone, "no such profile"));
    }
    let arguments = [
        Argument::Path(path.to_owned()),
        Argument::Path(String::from("/")),
        Argument::Path(String::from("/")),
    ];
    bus.try_call(NAME, PATH, NAME, "ActivateConnection", &arguments)?;
    Ok(())
}

/// Takes the profile with `uuid` down.
pub(super) fn deactivate(uuid: &str) -> Result<(), ServiceError> {
    for (active_uuid, path) in active() {
        if active_uuid == uuid {
            Bus::system().try_call(
                NAME,
                PATH,
                NAME,
                "DeactivateConnection",
                &[Argument::Path(path)],
            )?;
        }
    }
    Ok(())
}

/// Connects the device with `interface` with the best profile for it, as `nmcli device
/// connect` does, or disconnects it.
pub(super) fn set_device(interface: &str, connected: bool) -> Result<(), ServiceError> {
    let bus = Bus::system();
    let path = bus.try_call(
        NAME,
        PATH,
        NAME,
        "GetDeviceByIpIface",
        &[Argument::from(interface)],
    )?;
    let path = path.text();
    if path.is_empty() {
        return Err(ServiceError::new(ErrorKind::Gone, "no such device"));
    }
    if connected {
        let arguments = [
            Argument::Path(String::from("/")),
            Argument::Path(path.to_owned()),
            Argument::Path(String::from("/")),
        ];
        bus.try_call(NAME, PATH, NAME, "ActivateConnection", &arguments)?;
    } else {
        bus.try_call(NAME, path, DEVICE, "Disconnect", &[])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    // GetSettings, as NetworkManager answers it for these profiles
    fn settings(id: &str, uuid: &str, kind: &str, mode: Option<&str>) -> Value {
        let mut groups = vec![(
            "connection",
            map(&[("id", text(id)), ("uuid", text(uuid)), ("type", text(kind))]),
        )];
        if let Some(mode) = mode {
            groups.push(("802-11-wireless", map(&[("mode", text(mode))])));
        }
        map(&groups)
    }

    #[test]
    fn reads_the_profiles_the_shell_shows() {
        // the same profiles as the shell's nmcli fixtures
        let saved: Vec<Saved> = [
            settings("Home", "1111", "802-11-wireless", Some("infrastructure")),
            settings("tailscale0", "2222", "tun", None),
            settings("Work", "3333", "vpn", None),
            settings("wg0", "4444", "wireguard", None),
            settings("Hotspot", "5555", "802-11-wireless", Some("ap")),
        ]
        .iter()
        .filter_map(parse_settings)
        .collect();
        assert_eq!(saved.len(), 5);

        let active = [
            (String::from("4444"), String::from("/a/1")),
            (String::from("5555"), String::from("/a/2")),
        ];
        let profiles = profiles(&saved, &active);
        let vpns: Vec<(&str, bool)> = profiles
            .iter()
            .filter(|p| p.is_vpn())
            .map(|p| (p.name.as_str(), p.active))
            .collect();
        assert_eq!(vpns, [("Work", false), ("wg0", true)]);
        let hotspots: Vec<&str> = profiles
            .iter()
            .filter(|p| p.is_hotspot())
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(hotspots, ["Hotspot"]);

        assert!(parse_settings(&map(&[])).is_none(), "no uuid, no profile");
    }

    #[test]
    fn a_wifi_profile_reads_its_network_and_when_it_was_used() {
        let ssid = Value::List(
            b"Cafe"
                .iter()
                .map(|b| Value::Number(f64::from(*b)))
                .collect(),
        );
        let settings = map(&[
            (
                "connection",
                map(&[
                    ("id", text("Cafe")),
                    ("uuid", text("6666")),
                    ("type", text("802-11-wireless")),
                    ("autoconnect", Value::Bool(false)),
                    ("timestamp", Value::Number(1_700_000_000.0)),
                ]),
            ),
            ("802-11-wireless", map(&[("ssid", ssid)])),
            ("ipv4", map(&[("method", text("auto"))])),
        ]);
        let saved = parse_settings(&settings).expect("a profile");
        assert_eq!(saved.ssid.as_deref(), Some("Cafe"));
        assert!(!saved.autoconnect);
        assert_eq!(saved.last_used, 1_700_000_000);
        assert_eq!(saved.ipv4.method, "auto");

        let plain = parse_settings(&settings_of("Work", "3333", "vpn")).expect("a profile");
        assert!(plain.autoconnect, "on unless it says off");
        assert_eq!(plain.ssid, None);
    }

    fn settings_of(id: &str, uuid: &str, kind: &str) -> Value {
        settings(id, uuid, kind, None)
    }

    #[test]
    fn a_wired_device_shows_only_with_a_cable_in() {
        let device = |interface: &str, kind: f64, state: f64, connection: &str| Device {
            interface: interface.to_owned(),
            kind,
            state,
            connection: connection.to_owned(),
        };
        assert_eq!(
            wired(&[
                device("wlp1s0", 2.0, ACTIVATED, "Home"),
                device("enp2s0", ETHERNET, UNAVAILABLE, "")
            ]),
            None
        );
        assert_eq!(
            wired(&[device("enp2s0", ETHERNET, 30.0, "")]),
            Some(Wired {
                device: String::from("enp2s0"),
                connected: false,
                name: String::from("enp2s0"),
            })
        );
        assert_eq!(
            wired(&[device("enp2s0", ETHERNET, ACTIVATED, "Wired connection 1")])
                .map(|w| (w.connected, w.name)),
            Some((true, String::from("Wired connection 1")))
        );
    }
}
