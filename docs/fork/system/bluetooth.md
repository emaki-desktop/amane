# Bluetooth manager (fork addition)

Upstream's `Bluetooth` polls BlueZ every 2 seconds and only reports success. This fork's version:

- follows BlueZ's signals instead of polling
- keeps what each device is in the middle of, and why its last action failed
- answers BlueZ's pairing questions through an agent, so devices that need a PIN or passkey can pair

Upstream's guide is [Bluetooth](../../src/bluetooth.md). Everything it describes still works.

## State

- **Start-up:** the service reads the whole tree once with `GetManagedObjects`. After that it applies `InterfacesAdded`, `InterfacesRemoved` and `PropertiesChanged` from `org.bluez`, in the order BlueZ sent them (`services/bluetooth/model.rs`).
- **Re-reads:** the tree is read again when `org.bluez` changes owner (bluetoothd restarted or came back), and every 5 min while something watches, in case a signal was missed.
- **Redraws:** the service is written only when what it shows changed. Signal strength counts in whole bars (`signal()`, 1–4), so a search doesn't redraw at every dBm.
- **Adapter:** the first powered adapter by path, otherwise the first one. Only its devices are listed.

### Getters

| Function | Gives |
|---|---|
| `adapter()` | `BluetoothAdapter`: `name()` (what others see), `address()`, `power()`, `discovering()`, `discoverable()` |
| `power()` | `PowerState`: `Off`, `On`, `TurningOn`, `TurningOff`, `Blocked` (rfkill) |
| `discoverable()` | visible to devices searching |
| `device(path)` | one device |
| `operation(path)` | `Option<BluetoothOperation>`: `Connecting`, `Disconnecting`, `Pairing`, `Forgetting` |
| `failure(path)` | `Option<&ServiceError>`, kept until the device is tried again; the adapter's path for turning it on |
| `pairing_request()` | the question BlueZ is asking, with `serve_agent()` |

`BluetoothDevice` adds `trusted()`, `blocked()`, `signal()` and `named()`. `named()` is false when BlueZ only has the address. BlueZ then makes the alias from the address with dashes.

A failure is a [`ServiceError`](errors.md): its kind is read from BlueZ's error name, or from the reason in a plain `org.bluez.Error.Failed`, such as `br-connection-page-timeout`, which reads as `Unreachable`, and its message is BlueZ's own. `AlreadyConnected` and `AlreadyExists` count as success. amane keeps no words for a failure and doesn't clear it on a timer: emaki words it and shows it for 10 s.

## Actions

Every action returns at once.

| Function | Does |
|---|---|
| `connect` / `disconnect` / `pair` / `forget` | one at a time per device, each on its own thread; a second ask while one runs is ignored |
| `pair(path)` | pairs, sets `Trusted`, then connects. A device with nothing to connect (`NotSupported`) still counts as paired. |
| `cancel_pairing(path)` | stops a pairing; the "canceled" it ends with isn't shown |
| `set_trusted(path, on)` | whether it reconnects by itself |
| `set_blocked(path, on)` | refuses the device |
| `rename(path, name)` | sets the alias; an empty name brings back its own |
| `set_powered(on)` | when rfkill holds the radio off, lifts the soft block first, by writing a change-all event to `/dev/rfkill` as `rfkill unblock bluetooth` does |
| `set_discoverable(on)` | sets Pairable and Discoverable; BlueZ's `DiscoverableTimeout` ends it if nobody does |
| `start_scan` / `stop_scan` | `start_scan` sets a discovery filter (`Transport: auto`, `DuplicateData: false`). A search the radio drops is started again, at most every 5 s. |

An operation that BlueZ hasn't answered within 45 s is ended as `TimedOut`, and BlueZ is asked to stop it with `CancelPairing` and `Disconnect`.

## Pairing agent

`Bluetooth::serve_agent()` answers `org.bluez.Agent1` at `/org/amane/bluetooth/agent` with the `KeyboardDisplay` capability. It registers as BlueZ's default agent and registers again when bluetoothd restarts. Only one program should serve it, so the shell calls it and other amane programs don't.

- Each question becomes a `PairingRequest { id, device, kind }`. Its `PairingKind` is one of:
  - `Confirm(passkey)`
  - `EnterPin`
  - `EnterPasskey`
  - `DisplayPin(pin)`
  - `DisplayPasskey { passkey, entered }`
  - `Authorize`
  - `AuthorizeService(uuid)`
- `Bluetooth::answer(id, PairingAnswer::{Accept, Reject, Pin, Passkey})` answers it. BlueZ waits on the call until then.
- A code that's only shown (`asks()` is false) is answered at once, and stays until its pairing ends.
- `AuthorizeService` from a paired device is accepted without asking.
- One question waits at a time. A second is rejected.
- BlueZ's `Cancel` and `Release` clear the request.

## D-Bus additions

| Addition | For |
|---|---|
| `Bus::try_call` / `try_set_property` → `Result<_, BusError>` | the error's name and message; `call` and `set_property` are wrappers |
| `Bus::signals_from(sender)` | every signal one service sends, in order |
| `Bus::owner_changes(name)` | `NameOwnerChanged` for one name |
| `Method::reply_error(name, message)` | refusing a call, like `org.bluez.Error.Rejected` |
| `Signal::interface()`, `Signal::name()` | telling the signals in one stream apart |

zbus can't match a well-known sender locally, and the system connection is shared. So the service also compares each signal's sender with BlueZ's unique name, which it gets from `GetNameOwner`.
