#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! What a settings page shows about one connection: a profile's IPv4
//! settings as saved, and, while it's up, the addresses, gateway, DNS and
//! device it has.

use crate::{Argument, Bus, ErrorKind, ServiceError, Value};

use super::connections::{self, SETTINGS_PATH};
use super::manager::{NAME, properties};

const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const IP4: &str = "org.freedesktop.NetworkManager.IP4Config";
const IP6: &str = "org.freedesktop.NetworkManager.IP6Config";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRED: &str = "org.freedesktop.NetworkManager.Device.Wired";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";

/// A profile's IPv4 settings as saved (its `ipv4` group).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ipv4Settings {
    /// `auto` (DHCP), `manual`, `link-local`, `shared` or `disabled`; empty
    /// for a profile without IPv4, like most VPNs before they're up.
    pub method: String,
    /// The addresses set by hand, each with its prefix length.
    pub addresses: Vec<(String, u8)>,
    /// The gateway set by hand, empty for none.
    pub gateway: String,
    /// DNS servers set by hand, in order.
    pub dns: Vec<String>,
    /// Whether the DNS servers DHCP hands out are left out, so only
    /// [`dns`](Self::dns) is used.
    pub ignore_auto_dns: bool,
}

/// A connection as it is while up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionDetails {
    /// The interface it is on, like `wlp1s0`; empty for a VPN without one.
    pub interface: String,
    /// The device's hardware address, empty when it has none.
    pub mac: String,
    /// An Ethernet link's speed in Mb/s.
    pub speed: Option<u32>,
    /// A Wi-Fi network's frequency in MHz.
    pub frequency: Option<u32>,
    /// Its IPv4 addresses, each with its prefix length.
    pub ipv4: Vec<(String, u8)>,
    /// Its IPv4 gateway, empty for none.
    pub gateway: String,
    /// Its DNS servers, IPv4 then IPv6.
    pub dns: Vec<String>,
    /// Its IPv6 addresses, each with its prefix length.
    pub ipv6: Vec<(String, u8)>,
}

pub(super) fn ipv4_settings(group: &Value) -> Ipv4Settings {
    let dns = if group.get("dns-data").list().is_empty() {
        // older NetworkManager: each server a u32 in network order
        group
            .get("dns")
            .list()
            .iter()
            .map(|server| octets(server.number() as u32))
            .collect()
    } else {
        texts(group.get("dns-data"))
    };
    Ipv4Settings {
        method: group.get("method").text().to_owned(),
        addresses: address_data(group.get("address-data")),
        gateway: group.get("gateway").text().to_owned(),
        dns,
        ignore_auto_dns: group.get("ignore-auto-dns").bool(),
    }
}

fn octets(server: u32) -> String {
    let [a, b, c, d] = server.to_le_bytes();
    format!("{a}.{b}.{c}.{d}")
}

fn texts(list: &Value) -> Vec<String> {
    list.list()
        .iter()
        .map(|item| item.text().to_owned())
        .filter(|text| !text.is_empty())
        .collect()
}

// aa{sv} with "address" and "prefix", as both settings and IP configs give them
fn address_data(list: &Value) -> Vec<(String, u8)> {
    list.list()
        .iter()
        .filter_map(|entry| {
            let address = entry.get("address").text();
            (!address.is_empty()).then(|| (address.to_owned(), entry.get("prefix").number() as u8))
        })
        .collect()
}

// an IPv6 nameserver comes as 16 bytes
fn ipv6_text(bytes: &Value) -> Option<String> {
    let raw: Vec<u8> = bytes.list().iter().map(|b| b.number() as u8).collect();
    let raw: [u8; 16] = raw.try_into().ok()?;
    Some(std::net::Ipv6Addr::from(raw).to_string())
}

/// The connection with `uuid` as it is now; none while it's down.
pub(super) fn read(uuid: &str) -> Option<ConnectionDetails> {
    let (_, active) = connections::active()
        .into_iter()
        .find(|(active, _)| active == uuid)?;
    let bus = Bus::system();
    let connection = properties(&active, ACTIVE);
    let mut details = ConnectionDetails::default();

    let ip4 = connection.get("Ip4Config").text();
    if !ip4.is_empty() && ip4 != "/" {
        let config = properties(ip4, IP4);
        details.ipv4 = address_data(config.get("AddressData"));
        details.gateway = config.get("Gateway").text().to_owned();
        details.dns = config
            .get("NameserverData")
            .list()
            .iter()
            .map(|server| server.get("address").text().to_owned())
            .filter(|server| !server.is_empty())
            .collect();
    }
    let ip6 = connection.get("Ip6Config").text();
    if !ip6.is_empty() && ip6 != "/" {
        let config = properties(ip6, IP6);
        details.ipv6 = address_data(config.get("AddressData"));
        details.dns.extend(
            config
                .get("Nameservers")
                .list()
                .iter()
                .filter_map(ipv6_text),
        );
    }

    if let Some(device) = connection.get("Devices").list().first().map(Value::text) {
        let all = properties(device, DEVICE);
        details.interface = all.get("Interface").text().to_owned();
        details.mac = all.get("HwAddress").text().to_owned();
        let speed = bus.property(NAME, device, WIRED, "Speed").number() as u32;
        details.speed = (speed > 0).then_some(speed);
    }
    let access_point = connection.get("SpecificObject").text();
    if access_point.starts_with("/org/freedesktop/NetworkManager/AccessPoint/") {
        let frequency = bus
            .property(NAME, access_point, ACCESS_POINT, "Frequency")
            .number() as u32;
        details.frequency = (frequency > 0).then_some(frequency);
    }
    Some(details)
}

/// The settings path of the profile with `uuid`.
fn path(uuid: &str) -> Result<String, ServiceError> {
    let path = Bus::system().try_call(
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
    Ok(path.to_owned())
}

/// Deletes the profile with `uuid`, unless the system's configuration
/// declares it, which would only bring it back.
pub(super) fn forget(uuid: &str) -> Result<(), ServiceError> {
    let path = path(uuid)?;
    let bus = Bus::system();
    let file = bus.property(NAME, &path, CONNECTION, "Filename");
    if declared(file.text()) {
        return Err(ServiceError::new(
            ErrorKind::NotSupported,
            "set in the system's configuration",
        ));
    }
    bus.try_call(NAME, &path, CONNECTION, "Delete", &[])?;
    Ok(())
}

/// Whether a profile's file is one the system's configuration writes at
/// boot (NixOS's `ensureProfiles`, say), not one NetworkManager keeps.
pub(super) fn declared(file: &str) -> bool {
    file.starts_with("/run/")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    #[test]
    fn manual_settings_read_with_their_dns() {
        let group = map(vec![
            ("method", text("manual")),
            (
                "address-data",
                Value::List(vec![map(vec![
                    ("address", text("192.168.1.20")),
                    ("prefix", Value::Number(24.0)),
                ])]),
            ),
            ("gateway", text("192.168.1.1")),
            (
                "dns-data",
                Value::List(vec![text("1.1.1.1"), text("9.9.9.9")]),
            ),
            ("ignore-auto-dns", Value::Bool(true)),
        ]);
        assert_eq!(
            ipv4_settings(&group),
            Ipv4Settings {
                method: String::from("manual"),
                addresses: vec![(String::from("192.168.1.20"), 24)],
                gateway: String::from("192.168.1.1"),
                dns: vec![String::from("1.1.1.1"), String::from("9.9.9.9")],
                ignore_auto_dns: true,
            }
        );
    }

    #[test]
    fn older_dns_comes_in_network_order() {
        // 8.8.4.4 as NetworkManager's au has it
        let server = u32::from_le_bytes([8, 8, 4, 4]);
        let group = map(vec![
            ("method", text("auto")),
            ("dns", Value::List(vec![Value::Number(f64::from(server))])),
        ]);
        let read = ipv4_settings(&group);
        assert_eq!(read.dns, ["8.8.4.4"]);
        assert!(read.addresses.is_empty());
        assert!(!read.ignore_auto_dns);
    }

    #[test]
    fn ipv6_servers_read_from_bytes() {
        let mut bytes = vec![0u8; 16];
        bytes[0] = 0x26;
        bytes[1] = 0x06;
        bytes[2] = 0x47;
        bytes[3] = 0x00;
        bytes[15] = 0x11;
        let value = Value::List(
            bytes
                .into_iter()
                .map(|b| Value::Number(f64::from(b)))
                .collect(),
        );
        assert_eq!(ipv6_text(&value).as_deref(), Some("2606:4700::11"));
        assert_eq!(ipv6_text(&Value::List(Vec::new())), None);
    }

    #[test]
    fn only_profiles_written_at_boot_are_declared() {
        assert!(declared(
            "/run/NetworkManager/system-connections/home-wifi.nmconnection"
        ));
        assert!(!declared(
            "/etc/NetworkManager/system-connections/Cafe.nmconnection"
        ));
        assert!(!declared(""));
    }
}
