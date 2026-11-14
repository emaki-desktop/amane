# Disks (new)

Removable drives through UDisks2, as `Disks`: each USB stick, SD card or external disk's
filesystem, where it's mounted, and mounting, unmounting and ejecting it. Internal disks, and
anything UDisks says to leave alone (`HintIgnore`, `HintSystem`), aren't listed. Without UDisks
running there are no drives.

```rust
use amane::{Disks, Service};

for volume in Disks::read().volumes() {
    println!("{} · {} GB · {:?}", volume.name, volume.size / 1_000_000_000, volume.mount_points);
}

// mount, then open where it was mounted
if let Ok(place) = Disks::mount(&volume.object) {
    amane::launch::open(place);
}

// unmount every filesystem on the drive, then power it off
Disks::eject(&volume)?;
```

| Function | Gives |
| --- | --- |
| `Disks::read().volumes()` | every removable filesystem now, as `Volume { object, drive, name, size, mount_points, ejectable, auto }` |
| `Volume::mounted()` | `!mount_points.is_empty()` |
| `Disks::read().running()` | whether UDisks answered |
| `Disks::mount(object)` | mounts and gives where |
| `Disks::unmount(object)` | fails while a program has a file open |
| `Disks::eject(volume)` | unmounts every filesystem on the drive, then powers it off (or ejects its medium) |

The service does one read of UDisks's whole tree at the start and again after each burst of its
signals (coalesced over 250 ms), which come in dozens as a drive is plugged in. Nothing polls.
UDisks restarting starts the tree over.

A `Volume`'s **name** is its `IdLabel`, else its drive's `Vendor` and `Model`, else "N GB Drive".
A drive's object path, not its block device, is what `eject` takes.

The three write functions block on the system bus, so call them off the UI thread. They return
`ServiceError`; `ErrorKind::PermissionDenied` means UDisks's polkit rules didn't allow it (its
defaults let the active session mount and eject removable drives without a password).
