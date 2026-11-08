# Network: signals and connection profiles (fork addition)

Upstream's `Network` covers Wi-Fi: the link, the access points, joining and leaving. The fork
makes it follow NetworkManager's signals instead of asking every second, and adds the profiles
beyond Wi-Fi.

| API | Meaning |
|---|---|
| `profiles()` | every saved `NetworkProfile`: `uuid`, `name`, `kind` (NetworkManager's type), `wifi_mode`, `active` |
| `vpns()` | the `vpn` and `wireguard` profiles, as GNOME counts them; a tunnel NetworkManager doesn't manage (tailscale) isn't one |
| `hotspot()` | an active Wi-Fi profile in access-point mode |
| `wired()` | the first Ethernet device with a cable in (`Wired`: `device`, `connected`, `name`) |
| `Network::activate(uuid)`, `deactivate(uuid)` | a profile up or down |
| `Network::set_device(interface, connected)` | connect a device with its best profile, as `nmcli device connect`, or disconnect it |
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

Code: `src/services/network.rs`, `src/services/network/{connections,signals}.rs`. Tests: the
sorting of recorded `PropertiesChanged` signals, and profiles and
the wired device from recorded `GetSettings` answers, the same profiles as emaki's `nmcli`
fixtures.
