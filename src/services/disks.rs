//! Removable drives, as UDisks2 sees them: each filesystem on a USB stick, an SD card or an
//! external disk, whether and where it's mounted, and mounting, unmounting and ejecting it.
//! Internal disks, and anything UDisks says to leave alone, aren't listed.
//!
//! One read of UDisks's whole tree at the start and again after each burst of its signals,
//! which come in dozens as a drive is plugged in; nothing polls. Without UDisks running there
//! are no drives.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::{Argument, Bus, Service, ServiceError, Value};

const UDISKS: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";

/// How long a burst of signals is waited out before reading again.
const SETTLE: Duration = Duration::from_millis(250);

/// The removable drives' filesystems.
#[derive(Default, PartialEq)]
pub struct Disks {
    volumes: Vec<Volume>,
    running: bool,
}

/// One filesystem on a removable drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// UDisks's object for the block device, which [`Disks::mount`] and the rest take.
    pub object: String,
    /// The drive it's on, which [`Disks::eject`] powers off.
    pub drive: String,
    /// Its label, else the drive's maker and model, else its size.
    pub name: String,
    /// In bytes.
    pub size: u64,
    /// Where it's mounted; empty while it isn't.
    pub mount_points: Vec<String>,
    /// The drive can be powered off or ejected, so it's safe to pull out after.
    pub ejectable: bool,
    /// UDisks says to mount it as it appears.
    pub auto: bool,
}

impl Volume {
    /// Mounted somewhere.
    pub fn mounted(&self) -> bool {
        !self.mount_points.is_empty()
    }
}

impl Service for Disks {
    fn new() -> Self {
        fetch()
    }

    fn listen() {
        let bus = Bus::system();
        // watched before reading, so nothing that happens in between is missed
        let signals = bus.signals_from(UDISKS);
        let (wake, woken) = mpsc::channel::<()>();
        thread::spawn(move || {
            for signal in signals {
                if signal.path().starts_with(ROOT) && wake.send(()).is_err() {
                    break;
                }
            }
        });
        // a restart is a new tree
        let restarts = bus.owner_changes(UDISKS);
        thread::spawn(move || {
            for _ in restarts {
                publish(fetch());
            }
        });

        publish(fetch());
        while woken.recv().is_ok() {
            // the rest of the burst
            while woken.recv_timeout(SETTLE).is_ok() {}
            publish(fetch());
        }
    }
}

impl Disks {
    /// The removable filesystems, in UDisks's order.
    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    /// UDisks answers; without it nothing can be mounted.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Mounts the filesystem, as the user, and gives where. Blocks on the bus, so call it off
    /// the UI thread.
    pub fn mount(object: &str) -> Result<String, ServiceError> {
        Bus::system()
            .try_call(UDISKS, object, FILESYSTEM, "Mount", &[no_options()])
            .map(|path| path.text().to_string())
            .map_err(ServiceError::from)
    }

    /// Unmounts the filesystem; fails while a program still has a file open on it. Blocks.
    pub fn unmount(object: &str) -> Result<(), ServiceError> {
        Bus::system()
            .try_call(UDISKS, object, FILESYSTEM, "Unmount", &[no_options()])
            .map(|_| ())
            .map_err(ServiceError::from)
    }

    /// Unmounts every filesystem on `volume`'s drive, then powers the drive off (or ejects
    /// its medium), so it's safe to pull out. Blocks.
    pub fn eject(volume: &Volume) -> Result<(), ServiceError> {
        let siblings: Vec<Volume> = Self::read()
            .volumes
            .iter()
            .filter(|other| other.drive == volume.drive && other.mounted())
            .cloned()
            .collect();
        for sibling in &siblings {
            Self::unmount(&sibling.object)?;
        }
        if volume.drive.is_empty() || volume.drive == "/" {
            return Ok(());
        }
        let bus = Bus::system();
        bus.try_call(UDISKS, &volume.drive, DRIVE, "PowerOff", &[no_options()])
            .or_else(|_| bus.try_call(UDISKS, &volume.drive, DRIVE, "Eject", &[no_options()]))
            .map(|_| ())
            .map_err(ServiceError::from)
    }
}

fn no_options() -> Argument {
    Argument::Map(std::collections::BTreeMap::new())
}

fn publish(fresh: Disks) {
    if *Disks::peek() == fresh {
        return;
    }
    *Disks::write() = fresh;
}

// UDisks's whole tree, read for its removable filesystems
fn fetch() -> Disks {
    let bus = Bus::system();
    match bus.try_call(UDISKS, ROOT, OBJECT_MANAGER, "GetManagedObjects", &[]) {
        Ok(managed) => Disks {
            volumes: volumes(&managed),
            running: true,
        },
        Err(_) => Disks::default(),
    }
}

/// The removable filesystems in an answer to GetManagedObjects.
pub(crate) fn volumes(managed: &Value) -> Vec<Volume> {
    let Value::Map(objects) = managed else {
        return Vec::new();
    };
    objects
        .iter()
        .filter_map(|(object, interfaces)| {
            let block = interfaces.get(BLOCK);
            let filesystem = interfaces.get(FILESYSTEM);
            if matches!(block, Value::Nothing) || matches!(filesystem, Value::Nothing) {
                return None;
            }
            if block.get("HintIgnore").bool() || block.get("HintSystem").bool() {
                return None;
            }
            let drive_path = block.get("Drive").text().to_string();
            let drive = objects
                .get(&drive_path)
                .map_or(&Value::Nothing, |interfaces| interfaces.get(DRIVE));
            let removable = drive.get("Removable").bool() || drive.get("MediaRemovable").bool();
            if !removable {
                return None;
            }
            let size = block.get("Size").number() as u64;
            Some(Volume {
                object: object.clone(),
                drive: drive_path,
                name: name(block, drive, size),
                size,
                mount_points: filesystem
                    .get("MountPoints")
                    .list()
                    .iter()
                    .map(bytes_text)
                    .filter(|point| !point.is_empty())
                    .collect(),
                ejectable: drive.get("CanPowerOff").bool() || drive.get("Ejectable").bool(),
                auto: block.get("HintAuto").bool(),
            })
        })
        .collect()
}

fn name(block: &Value, drive: &Value, size: u64) -> String {
    let label = block.get("IdLabel").text().trim();
    if !label.is_empty() {
        return label.to_string();
    }
    let maker = format!(
        "{} {}",
        drive.get("Vendor").text().trim(),
        drive.get("Model").text().trim()
    );
    let maker = maker.trim();
    if !maker.is_empty() {
        return maker.to_string();
    }
    format!("{} GB Drive", (size as f64 / 1e9).round() as u64)
}

// a path sent as bytes ending in a zero, as UDisks sends mount points
fn bytes_text(bytes: &Value) -> String {
    let raw: Vec<u8> = bytes
        .list()
        .iter()
        .map(|byte| byte.number() as u8)
        .take_while(|byte| *byte != 0)
        .collect();
    String::from_utf8_lossy(&raw).into_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect(),
        )
    }

    fn path_bytes(path: &str) -> Value {
        Value::List(
            path.bytes()
                .chain([0])
                .map(|byte| Value::Number(f64::from(byte)))
                .collect(),
        )
    }

    fn tree(removable: bool, mounted: Option<&str>) -> Value {
        let drive = "/org/freedesktop/UDisks2/drives/SanDisk_1";
        let points = mounted.map(path_bytes).into_iter().collect();
        let mut objects = BTreeMap::new();
        objects.insert(
            String::from(drive),
            map(&[(
                DRIVE,
                map(&[
                    ("Removable", Value::Bool(removable)),
                    ("CanPowerOff", Value::Bool(true)),
                    ("Vendor", Value::Text(String::from("SanDisk"))),
                    ("Model", Value::Text(String::from("Ultra"))),
                ]),
            )]),
        );
        objects.insert(
            String::from("/org/freedesktop/UDisks2/block_devices/sda1"),
            map(&[
                (
                    BLOCK,
                    map(&[
                        ("Drive", Value::Text(String::from(drive))),
                        ("Size", Value::Number(32e9)),
                        ("IdLabel", Value::Text(String::from("PHOTOS"))),
                        ("HintAuto", Value::Bool(true)),
                    ]),
                ),
                (FILESYSTEM, map(&[("MountPoints", Value::List(points))])),
            ]),
        );
        // the whole disk has no filesystem, so it isn't one to list
        objects.insert(
            String::from("/org/freedesktop/UDisks2/block_devices/sda"),
            map(&[(BLOCK, map(&[("Drive", Value::Text(String::from(drive)))]))]),
        );
        Value::Map(objects)
    }

    #[test]
    fn lists_a_removable_drives_filesystem() {
        let volumes = volumes(&tree(true, Some("/run/media/me/PHOTOS")));
        assert_eq!(volumes.len(), 1);
        let volume = &volumes[0];
        assert_eq!(volume.name, "PHOTOS");
        assert_eq!(
            volume.mount_points,
            vec![String::from("/run/media/me/PHOTOS")]
        );
        assert!(volume.mounted() && volume.ejectable && volume.auto);
    }

    #[test]
    fn leaves_out_internal_disks() {
        assert!(volumes(&tree(false, None)).is_empty());
    }

    #[test]
    fn a_drive_without_a_label_is_named_by_its_maker() {
        let block = map(&[]);
        let drive = map(&[
            ("Vendor", Value::Text(String::from("SanDisk"))),
            ("Model", Value::Text(String::from("Ultra"))),
        ]);
        assert_eq!(name(&block, &drive, 0), "SanDisk Ultra");
        assert_eq!(name(&block, &map(&[]), 64_000_000_000), "64 GB Drive");
    }
}
