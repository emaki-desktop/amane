#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! The kernel's device events (uevents) for the whole program: one netlink socket on one
//! thread, handed out per subsystem. A service that would otherwise poll sysfs, like the
//! backlight, waits on its subsystem's events instead.
//!
//! Only the kernel's own broadcast is read (netlink group 1), which needs no privileges and
//! doesn't depend on udev running.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;

/// One device event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uevent {
    /// `add`, `remove`, `change`, `move`, `online`, `offline`, `bind` or `unbind`.
    pub action: String,
    /// The device's path under `/sys`, like `/devices/…/backlight/amdgpu_bl1`.
    pub devpath: String,
    /// Like `backlight`, `leds` or `power_supply`.
    pub subsystem: String,
    /// Every `KEY=value` the kernel sent.
    pub vars: HashMap<String, String>,
}

static HUB: Mutex<Hub> = Mutex::new(Hub {
    state: Started::No,
    listeners: Vec::new(),
});

#[derive(PartialEq, Eq)]
enum Started {
    No,
    Yes,
    // the socket couldn't be opened, or the reader stopped
    Failed,
}

struct Hub {
    state: Started,
    listeners: Vec<(&'static str, Sender<Uevent>)>,
}

/// The events for `subsystem` from now on. `None` when the kernel's events can't be read, so
/// the caller keeps polling.
pub fn listen(subsystem: &'static str) -> Option<Receiver<Uevent>> {
    let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);

    if hub.state == Started::No {
        hub.state = match open() {
            Ok(socket) => {
                let started = thread::Builder::new()
                    .name(String::from("amane-uevents"))
                    .spawn(move || read(&socket));
                if started.is_ok() {
                    Started::Yes
                } else {
                    Started::Failed
                }
            }
            Err(err) => {
                eprintln!("amane: no device events, polling instead: {err}");
                Started::Failed
            }
        };
    }
    if hub.state == Started::Failed {
        return None;
    }

    let (sender, receiver) = mpsc::channel();
    hub.listeners.push((subsystem, sender));
    Some(receiver)
}

fn open() -> io::Result<OwnedFd> {
    // SAFETY: a plain socket(2) call; the result is checked before it is owned
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a fresh descriptor nothing else owns
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };

    // SAFETY: sockaddr_nl is plain data; all zeroes is a valid start
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    // group 1: the kernel's own events
    address.nl_groups = 1;

    // SAFETY: address is a valid sockaddr_nl and its size is passed with it
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(socket)
}

fn read(socket: &OwnedFd) {
    let mut buffer = vec![0u8; 8192];

    loop {
        // SAFETY: the buffer is valid for its length for the whole call
        let read = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if read < 0 {
            let err = io::Error::last_os_error();
            // the kernel dropped events for a full buffer; the next ones still come
            if matches!(
                err.raw_os_error(),
                Some(libc::EINTR | libc::ENOBUFS | libc::EAGAIN)
            ) {
                continue;
            }
            eprintln!("amane: device events stopped: {err}");
            break;
        }

        let Some(event) = parse(&buffer[..read as usize]) else {
            continue;
        };

        let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);
        hub.listeners.retain(|(subsystem, listener)| {
            *subsystem != event.subsystem || listener.send(event.clone()).is_ok()
        });
    }

    let mut hub = HUB.lock().unwrap_or_else(PoisonError::into_inner);
    hub.state = Started::Failed;
    // closing the channels tells each listener to poll again
    hub.listeners.clear();
}

/// A kernel uevent: `action@devpath`, then `KEY=value` fields, each ending in a zero byte.
fn parse(message: &[u8]) -> Option<Uevent> {
    let mut parts = message
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(String::from_utf8_lossy);

    let header = parts.next()?;
    // udev's own re-broadcasts start with "libudev"; only the kernel's are read
    let (_, _) = header.split_once('@')?;

    let mut event = Uevent::default();
    for part in parts {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key {
            "ACTION" => event.action = value.to_owned(),
            "DEVPATH" => event.devpath = value.to_owned(),
            "SUBSYSTEM" => event.subsystem = value.to_owned(),
            _ => {}
        }
        event.vars.insert(key.to_owned(), value.to_owned());
    }

    (!event.action.is_empty() && !event.subsystem.is_empty()).then_some(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_kernel_event() {
        let message = b"change@/devices/pci0000:00/0000:00:08.1/0000:c4:00.0/backlight/amdgpu_bl1\0ACTION=change\0DEVPATH=/devices/pci0000:00/0000:00:08.1/0000:c4:00.0/backlight/amdgpu_bl1\0SUBSYSTEM=backlight\0SOURCE=sysfs\0SEQNUM=5121\0";
        let event = parse(message).expect("an event");
        assert_eq!(event.action, "change");
        assert_eq!(event.subsystem, "backlight");
        assert!(event.devpath.ends_with("/amdgpu_bl1"));
        assert_eq!(event.vars.get("SOURCE").map(String::as_str), Some("sysfs"));
    }

    #[test]
    fn skips_what_isnt_one() {
        assert!(parse(b"").is_none());
        assert!(parse(b"libudev\0\xfe\xed\xca\xfe").is_none());
        // a header without the fields
        assert!(parse(b"add@/devices/x\0").is_none());
    }
}
