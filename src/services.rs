mod appearance;
mod apps;
mod audio;
pub(crate) mod backdrop;
mod battery;
mod bluetooth;
mod brightness;
mod casts;
mod clipboard_watcher;
mod cpu;
mod desktop;
mod error;
pub(crate) mod focused_field;
mod gestures;
mod idle;
mod keyboard_layouts;
mod lock;
mod lock_keys;
mod logind;
mod media;
mod memory;
mod network;
mod notifications;
mod outputs;
mod overview;
mod palette;
mod pam;
mod polkit;
mod power_profile;
mod rfkill;
pub mod store;
mod subscribe;
mod tray;
pub(crate) mod uevent;
mod window_info;
mod windows;
pub(crate) mod worker;
mod workspace;
mod workspace_switch;
mod workspaces;
mod write;

use std::any::TypeId;
use std::sync::{PoisonError, RwLockReadGuard};
use std::thread;
use std::time::Duration;

use crate::changes;

pub use appearance::{Appearance, Scheme};
pub use apps::{Apps, DesktopAction, DesktopApp, MimeApps};
pub use audio::{Audio, AudioDevice, AudioKind};
pub use backdrop::Backdrop;
pub use battery::{Battery, BatteryState, DeviceKind, PowerDevice, WarningLevel};
pub use bluetooth::{
    Bluetooth, BluetoothAdapter, BluetoothDevice, Operation as BluetoothOperation, PairingAnswer,
    PairingKind, PairingRequest, PowerState,
};
pub use brightness::Brightness;
pub use casts::Casts;
pub use clipboard_watcher::ClipboardWatcher;
pub use cpu::Cpu;
pub use desktop::Desktop;
pub use error::{ErrorKind, ServiceError, kind_of_bus};
pub use focused_field::{FieldOwner, FillError, FocusedField, Hints, Purpose};
pub use gestures::Gestures;
pub use idle::Idle;
pub use keyboard_layouts::KeyboardLayouts;
pub use lock::Lock;
pub use lock_keys::LockKeys;
pub use logind::Logind;
pub use media::{Media, MediaPlayer};
pub use memory::Memory;
pub use network::{AccessPoint, Link, Network, NetworkOperation, NetworkProfile, Wired};
pub use notifications::{Action, Kept, Notification, Notifications, Timeout, Urgency};
pub use outputs::Outputs;
pub use overview::Overview;
pub use palette::Palette;
pub use pam::{Pam, PamConfig, PamError, PamMessage, PamMessageKind, PamResult};
#[cfg(feature = "polkit-agent")]
pub use polkit::PolkitAgent;
pub use polkit::{AuthPrompt, AuthRequest, Polkit};
pub use power_profile::PowerProfile;
pub use rfkill::{Radio, Rfkill};
pub use subscribe::Subscription;
pub use tray::{Tray, TrayItem, TrayMenuItem, TrayStatus, TrayToggle};
pub use window_info::WindowInfo;
pub use windows::Windows;
pub use workspace::Workspace;
pub use workspace_switch::WorkspaceSwitch;
pub use workspaces::Workspaces;
pub use write::Write;

/// How long a polling service amane ships keeps polling once nothing watches it; a good
/// default for [`Service::idle_stop`] and [`Service::watched_within`] in services of your own.
pub const IDLE_STOP: Duration = Duration::from_secs(30);

pub trait Service: Send + Sync + Sized + 'static {
    fn new() -> Self;

    fn interval() -> Duration {
        Duration::from_secs(1)
    }

    /// How long the default polling [`listen`](Self::listen) keeps polling after the last
    /// [`read`](Self::read). Then it stops until the next read, which polls at once. `None`
    /// polls for as long as the program runs.
    fn idle_stop() -> Option<Duration> {
        None
    }

    // reads the source again and says whether anything a window shows changed
    fn update(&mut self) -> bool {
        false
    }

    /*
     * runs on the service's own thread, so waiting here never
     * freezes drawing; event-driven services replace it with their
     * own loop, and services that only change through input with
     * an empty one
     */
    fn listen() {
        loop {
            thread::sleep(Self::interval());

            if let Some(idle) = Self::idle_stop() {
                store::wait_while_unread::<Self>(idle);
            }

            let mut service = Self::write();

            // a poll that found nothing new redraws nothing
            if !service.update() {
                service.quiet();
            }
        }
    }

    /// Whether something watched the service within `idle`: an open window's last frame read
    /// it, or something read it that recently. For a listener of its own that resyncs only
    /// while watched.
    fn watched_within(idle: Duration) -> bool {
        !store::unwatched::<Self>(idle)
    }

    /// Reads the service without counting as watching it, and without redrawing anything
    /// when it changes: for the service's own listener, comparing a fresh reading with what it
    /// has. Views use [`read`](Self::read).
    fn peek() -> RwLockReadGuard<'static, Self> {
        store::find::<Self>()
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn read() -> RwLockReadGuard<'static, Self> {
        // remembered, so a later change redraws only the windows that read it
        changes::note_read(TypeId::of::<Self>());

        let slot = store::slot::<Self>();
        slot.use_.touch();
        slot.lock.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Calls `callback` after each write that changed the service (one not made
    /// `quiet`), so another service can follow this one without polling it.
    ///
    /// Callbacks run one at a time on amane's subscription thread, with the service read for
    /// them; writes in quick succession may share one call. Keep them short, and don't write
    /// this same service from its own callback: the read it is given would wait forever. The
    /// callback stays until the returned [`Subscription`] is dropped, or forever after
    /// [`Subscription::forever`].
    fn subscribe(callback: impl Fn(&Self) + Send + Sync + 'static) -> Subscription {
        subscribe::add::<Self>(callback)
    }

    /*
     * for input handlers: inside view() a read of the same
     * service is still held, so this would wait forever
     */
    fn write() -> Write<Self> {
        subscribe::check_not_in_callback::<Self>();

        let guard = store::find::<Self>()
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        Write {
            guard: Some(guard),
            quiet: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[derive(Default)]
    struct Counter(u32);

    impl Service for Counter {
        fn new() -> Self {
            Self::default()
        }

        fn listen() {}
    }

    #[test]
    fn reads_after_a_write_panicked() {
        let failed = thread::spawn(|| {
            let _counter = Counter::write();

            panic!("a write that fails halfway");
        });

        assert!(failed.join().is_err());

        assert_eq!(Counter::read().0, 0);
    }
}
