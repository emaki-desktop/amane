#![warn(
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    clippy::dbg_macro,
    clippy::todo,
    missing_docs
)]

//! The clipboard's selection as it changes, whoever copied it: what a clipboard history needs.
//!
//! It uses the compositor's `ext-data-control-v1` on a Wayland connection of its own, on the
//! service's thread, so it works without a focused window and never waits on the UI thread.
//! [`Clipboard`](crate::Clipboard) is the other side: copying and pasting from a window.
//!
//! A password copied the way password managers do it is offered with
//! `x-kde-passwordManagerHint`; [`ClipboardWatcher::secret`] says so, so a history can leave it
//! out. Without the protocol (another compositor, or niri before it had it),
//! [`available`](ClipboardWatcher::available) is false and nothing changes.

use std::collections::HashMap;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{wl_registry, wl_seat::WlSeat};
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
};
use smithay_client_toolkit::reexports::protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};

use crate::wayland::clipboard::PASSWORD_HINT;
use crate::{Service, ServiceError};

// how long a read waits for the program that copied to hand the data over
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The latest selection on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardWatcher {
    available: bool,
    mime_types: Vec<String>,
    changes: u64,
}

// the connection and the current selection's offer, for read() on another thread
struct Current {
    connection: Connection,
    offer: Option<ExtDataControlOfferV1>,
}

static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

impl Service for ClipboardWatcher {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {
        if let Err(err) = watch() {
            eprintln!("amane: no clipboard watcher: {err}");
        }
    }
}

impl ClipboardWatcher {
    /// Whether the compositor lets amane watch the clipboard.
    pub fn available(&self) -> bool {
        self.available
    }

    /// The kinds the selection is offered as, like `text/plain;charset=utf-8` or `image/png`;
    /// empty when the clipboard is empty.
    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    /// Goes up by one with each new selection, the same copied twice included.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Whether the selection is a password, marked the way password managers mark them.
    pub fn secret(&self) -> bool {
        self.mime_types.iter().any(|mime| mime == PASSWORD_HINT)
    }

    /// The best text kind on offer, if any.
    pub fn text_type(&self) -> Option<&str> {
        const TEXT: [&str; 4] = [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
        ];
        TEXT.iter()
            .find_map(|wanted| self.mime_types.iter().find(|mime| mime == wanted))
            .map(String::as_str)
    }

    /// Reads the current selection as `mime`. Blocks until the program that copied hands it
    /// over, for up to two seconds, so call it from a service thread, never from a view or
    /// input handler: when this program owns the selection, its UI thread is the one that
    /// answers.
    pub fn contents(mime: &str) -> Result<Vec<u8>, ServiceError> {
        let (read, write) = pipe()?;
        {
            let current = CURRENT.lock().unwrap_or_else(PoisonError::into_inner);
            let current = current.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "no clipboard watcher")
            })?;
            let offer = current
                .offer
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the clipboard is empty"))?;
            offer.receive(mime.to_owned(), write.as_fd());
            current
                .connection
                .flush()
                .map_err(|err| io::Error::other(err.to_string()))?;
        }
        // the other end has it now; without closing ours the read never ends
        drop(write);
        Ok(read_all(read, READ_TIMEOUT)?)
    }
}

struct State {
    // offers whose kinds are still arriving, by object
    describing: HashMap<ObjectId, Vec<String>>,
}

fn watch() -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|err| err.to_string())?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).map_err(|err| err.to_string())?;
    let qh = queue.handle();

    let manager: ExtDataControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| String::from("the compositor has no ext-data-control-v1"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| String::from("no seat"))?;
    let _device = manager.get_data_device(&seat, &qh, ());

    *CURRENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(Current {
        connection: connection.clone(),
        offer: None,
    });
    {
        let mut watcher = ClipboardWatcher::write();
        watcher.available = true;
    }

    let mut state = State {
        describing: HashMap::new(),
    };
    loop {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|err| err.to_string())?;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.describing.insert(id.id(), Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                let mime_types = id
                    .as_ref()
                    .and_then(|offer| state.describing.remove(&offer.id()))
                    .unwrap_or_default();

                let previous = {
                    let mut current = CURRENT.lock().unwrap_or_else(PoisonError::into_inner);
                    current
                        .as_mut()
                        .and_then(|current| std::mem::replace(&mut current.offer, id))
                };
                if let Some(previous) = previous {
                    previous.destroy();
                }

                let mut watcher = ClipboardWatcher::write();
                watcher.mime_types = mime_types;
                watcher.changes += 1;
            }
            // the primary selection (middle-click paste) isn't followed
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.describing.remove(&offer.id());
                offer.destroy();
            }
            ext_data_control_device_v1::Event::Finished => {
                let mut watcher = ClipboardWatcher::write();
                watcher.available = false;
                watcher.mime_types.clear();
            }
            _ => {}
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some(types) = state.describing.get_mut(&offer.id())
        {
            types.push(mime_type);
        }
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills the two-element array on success
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both are fresh descriptors nothing else owns
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

// the most read of one copy: a big screenshot fits
const MAX_CONTENTS: usize = 64 * 1024 * 1024;

// everything until the writer closes, or until `timeout` passes without it
fn read_all(fd: OwnedFd, timeout: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut file = std::fs::File::from(fd);
    let mut data = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the program that copied didn't hand it over",
            ));
        }
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd for the length passed
        let ready = unsafe { libc::poll(&raw mut poll, 1, millis) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ready == 0 {
            continue;
        }
        match file.read(&mut chunk) {
            Ok(0) => return Ok(data),
            Ok(read) => {
                // the program that copied says how much
                if data.len() + read > MAX_CONTENTS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the copy is too big to read",
                    ));
                }
                data.extend_from_slice(&chunk[..read]);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn picks_the_best_text_kind_and_spots_a_password() {
        let watcher = ClipboardWatcher {
            available: true,
            mime_types: vec![
                String::from("STRING"),
                String::from("text/plain;charset=utf-8"),
                String::from(PASSWORD_HINT),
            ],
            changes: 1,
        };
        assert_eq!(watcher.text_type(), Some("text/plain;charset=utf-8"));
        assert!(watcher.secret());
        assert_eq!(ClipboardWatcher::default().text_type(), None);
    }

    #[test]
    fn reads_a_pipe_to_its_end_or_gives_up() {
        let (read, write) = pipe().expect("pipe");
        let mut writer = std::fs::File::from(write);
        writer.write_all(b"copied").expect("write");
        drop(writer);
        assert_eq!(
            read_all(read, Duration::from_secs(1)).expect("read"),
            b"copied"
        );

        let (read, _write) = pipe().expect("pipe");
        let err = read_all(read, Duration::from_millis(50)).expect_err("nobody writes");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
