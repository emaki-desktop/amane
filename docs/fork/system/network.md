# Network: signals and connection profiles (fork addition)

Upstream's `Network` covers Wi-Fi: the link, the access points, joining and leaving. The fork
makes it follow NetworkManager's signals instead of asking every second, and adds the profiles
beyond Wi-Fi.

| API | Meaning |
|---|---|
| `profiles()` | every saved `NetworkProfile`: `uuid`, `name`, `kind` (NetworkManager's type), `wifi_mode`, `active`, `ssid`, `autoconnect`, `last_used` (seconds since 1970, 0 for never), `declared` (written at boot by the system's configuration, its file under `/run/`), and `ipv4` (`Ipv4Settings`: `method`, `addresses`, `gateway`, `dns`, `ignore_auto_dns`, as saved) |
| `vpns()` | the `vpn` and `wireguard` profiles, as GNOME counts them; a tunnel NetworkManager doesn't manage (tailscale) isn't one |
| `hotspot()` | an active Wi-Fi profile in access-point mode |
| `wired()` | the first Ethernet device with a cable in (`Wired`: `device`, `connected`, `name`) |
| `Network::activate(uuid)`, `deactivate(uuid)` | a profile up or down |
| `Network::set_device(interface, connected)` | connect a device with its best profile, as `nmcli device connect`, or disconnect it |
| `Network::details(uuid)` | the connection as it is while up, or `None`: `ConnectionDetails` with `interface`, `mac`, `speed` (Ethernet, Mb/s), `frequency` (Wi-Fi, MHz), `ipv4` and `ipv6` addresses with their prefixes, `gateway`, `dns`. Blocking: call it off the UI thread |
| `Network::forget(uuid)` | deletes a saved profile, password and all; a `declared` one is refused as `NotSupported`, since boot would write it again |
| `Network::connect_hidden(ssid, password)` | joins a network that doesn't announce its name, saving a profile with `802-11-wireless.hidden`; failures under `failure(ssid)` as joining |
| `Network::operation(ssid)` | `Some(NetworkOperation::Connecting)` while that network is being joined |
| `Network::failure(key)` | why the last try on a network (by name), profile (by UUID) or device (by interface) failed, as a [`ServiceError`](errors.md), until the next try; a wrong password is `AuthenticationFailed` |

## How it works

- **Signals.** Each signal NetworkManager sends is sorted first (`network/signals.rs`):
  - The joined network's signal strength is taken from the signal itself, with no read.
  - Other networks' strength, `LastSeen` and a scan's `LastScan` cause a read only while a window
    listed the networks (`access_points()`) in the last 30 s.
  - Device traffic counters are ignored.
  - Everything else (devices, active connections, settings) causes a full read. A burst counts
    as one change: the read happens once the signals pause for 250 ms, or after a second at most.
- **Profiles' settings** (`GetSettings` per profile) are read again only when a signal came from
  under `/org/freedesktop/NetworkManager/Settings`.
- **A resync every 60 s** in case a signal went missing, only while something watches.
- Nothing is written when nothing changed that a view shows; signal strength counts in five
  steps, as the Wi-Fi glyph shows it.

- **Details** are read only when asked (`Network::details`): the active connection's
  `Ip4Config` (`AddressData`, `Gateway`, `NameserverData`) and `Ip6Config` (`AddressData`,
  `Nameservers`), its first device (`Interface`, `HwAddress`, and `Device.Wired`'s `Speed`), and
  for Wi-Fi the `SpecificObject` access point's `Frequency`.
- **Saved IPv4 settings** come from the `GetSettings` already read. DNS comes from `dns-data`,
  or from the older `dns` (`au`, each server in network byte order).
- Changing a profile's settings isn't here. A D-Bus `Update` replaces the whole profile, secrets
  included, so emaki's Settings changes them with `nmcli connection modify`.

Code: `src/services/network.rs`, `src/services/network/{connections,details,signals}.rs`. Tests: the
sorting of recorded `PropertiesChanged` signals, and profiles and
the wired device from recorded `GetSettings` answers, the same profiles as emaki's `nmcli`
fixtures.
