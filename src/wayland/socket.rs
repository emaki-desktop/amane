#![warn(clippy::undocumented_unsafe_blocks)]
use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::channel::{self, Channel, Event, Sender};
use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, generic::Generic,
};

use crate::ipc::{ACTIVATE, IpcCall, IpcHandlers, ipc_socket_for};

use super::WaylandState;

/*
 * a call is read on a thread of its own, never on the event loop: a client that sends a byte
 * a second, or never stops, can't hold up the shell. The whole call must come within this
 * long and fit in these limits, and only a few are read at once
 */
const READ_TIME: Duration = Duration::from_secs(2);
const READING_AT_ONCE: usize = 8;

// a call read, with the stream its answer goes back on
type Read = (UnixStream, IpcCall);

// how long a client may take to take the answer
const WRITE_TIME: Duration = Duration::from_secs(1);

pub fn insert(handle: &LoopHandle<'static, WaylandState>, handlers: IpcHandlers, app_id: &str) {
    let source = Generic::new(listen(app_id), Interest::READ, Mode::Level);

    // calls read off the loop come back to it to be answered, the handlers being the UI's
    let (calls, read): (Sender<Read>, Channel<Read>) = channel::channel();

    let reading = Arc::new(AtomicUsize::new(0));

    handle
        .insert_source(source, move |_, listener, _| {
            // a failed call only loses that call, the shell keeps running
            while let Ok((stream, _)) = listener.accept() {
                take(stream, &calls, &reading);
            }

            Ok(PostAction::Continue)
        })
        .expect("failed to insert ipc socket");

    handle
        .insert_source(read, move |event, _, state| {
            if let Event::Msg((stream, call)) = event {
                answer(stream, &call, &handlers, state);

                state.request_frames();
            }
        })
        .expect("failed to insert ipc calls");
}

fn listen(app_id: &str) -> UnixListener {
    let path = ipc_socket_for(app_id);

    // a socket that still answers belongs to another running copy, which took this launch
    if UnixStream::connect(&path).is_ok() {
        panic!("failed to listen: {app_id} is already running");
    }

    // what is left is a leftover file from a shell that did not exit cleanly
    let _ = fs::remove_file(&path);

    let listener = UnixListener::bind(&path).expect("failed to create ipc socket");

    // the runtime directory is the user's own already; the socket is too
    if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
        eprintln!("amane: ipc socket {}: {error}", path.display());
    }

    // the event loop only wakes this up when a client is waiting, so accept must never block
    listener
        .set_nonblocking(true)
        .expect("failed to make ipc socket non-blocking");

    listener
}

// checks the caller, then reads its call on a thread and hands it to the loop
fn take(stream: UnixStream, calls: &Sender<Read>, reading: &Arc<AtomicUsize>) {
    // only this user's own programs; the folder should see to that, and this holds if not
    if peer_uid(&stream) != Some(own_uid()) {
        return;
    }

    if reading.fetch_add(1, Ordering::AcqRel) >= READING_AT_ONCE {
        reading.fetch_sub(1, Ordering::AcqRel);
        return;
    }

    let calls = calls.clone();
    let reading = reading.clone();

    let spawned = thread::Builder::new()
        .name(String::from("amane-ipc"))
        .spawn(move || {
            let call = read(&stream);

            reading.fetch_sub(1, Ordering::AcqRel);

            if let Some(call) = call {
                let _ = calls.send((stream, call));
            }
        });

    if spawned.is_err() {
        eprintln!("amane: no thread to read an ipc call on");
    }
}

fn read(stream: &UnixStream) -> Option<IpcCall> {
    stream.set_nonblocking(false).ok()?;

    IpcCall::read_within(stream, Instant::now() + READ_TIME).ok()
}

fn answer(
    mut stream: UnixStream,
    call: &IpcCall,
    handlers: &IpcHandlers,
    state: &mut WaylandState,
) {
    // a client that never reads its answer must not hold the loop either
    if stream.set_write_timeout(Some(WRITE_TIME)).is_err() {
        return;
    }

    // amane answers a second launch itself, the app only hears its arguments
    if call.name == ACTIVATE {
        state.activate(&call.arguments, handlers.on_activate);

        let _ = stream.write_all(b"activated");

        return;
    }

    let reply = handlers.run(call);

    let _ = stream.write_all(reply.as_bytes());
}

// the user id of the process at the other end, as the kernel says
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    // SAFETY: all-zero is a valid ucred
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    // SAFETY: the pointers are to live locals, and length is the ucred's size
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut length,
        )
    };

    (status == 0).then_some(credentials.uid)
}

fn own_uid() -> u32 {
    // SAFETY: getuid can't fail
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_on_this_machine_is_this_user() {
        let (ours, theirs) = UnixStream::pair().expect("failed to make a socket pair");

        assert_eq!(peer_uid(&ours), Some(own_uid()));
        drop(theirs);
    }
}
