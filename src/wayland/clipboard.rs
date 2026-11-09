#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::fs::File;
use std::io::{Read, Write};
use std::mem;
use std::os::fd::OwnedFd;
use std::sync::PoisonError;
use std::thread;

use smithay_client_toolkit::data_device_manager::{
    ReadPipe, WritePipe, data_source::CopyPasteSource,
};
use smithay_client_toolkit::primary_selection::{
    device::PrimarySelectionDeviceHandler,
    selection::{PrimarySelectionSource, PrimarySelectionSourceHandler},
};
use smithay_client_toolkit::reexports::protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
};
use wayland_client::{Connection, QueueHandle};

use crate::changes;
use crate::clipboard::{Board, REQUESTS, Request, Then};

use super::WaylandState;

// what text is offered as, the first one another program asks for is sent
pub const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

// what marks a copy as a password, for clipboard histories (KDE's, and the shell's)
pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

// what amane has put on the clipboard and the primary selection, kept to send when asked
#[derive(Default)]
pub struct Copied {
    pub clipboard: Option<(CopyPasteSource, String)>,
    pub primary: Option<(PrimarySelectionSource, String)>,

    // the clipboard's copy is a password, wiped when it goes
    pub secret: bool,
}

impl Copied {
    // the clipboard's copy goes, a password's text wiped
    pub fn drop_clipboard(&mut self) {
        if let Some((_, mut text)) = self.clipboard.take()
            && self.secret
        {
            zeroize::Zeroize::zeroize(&mut text);
        }

        self.secret = false;
    }
}

impl WaylandState {
    // the copies and pastes asked for since the loop last woke
    pub fn handle_clipboard_requests(&mut self) {
        let mut queue = REQUESTS.lock().unwrap_or_else(PoisonError::into_inner);

        let requests = mem::take(&mut *queue);

        drop(queue);

        for request in requests {
            match request {
                Request::Copy(board, text) => self.copy(board, text),
                Request::CopySecret(text, clear_after) => self.copy_secret(text, clear_after),
                Request::Paste(board, then) => self.paste(board, then),
            }
        }
    }

    fn copy(&mut self, board: Board, text: String) {
        // the compositor only takes a selection made in answer to input
        let Some(serial) = self.last_input_serial else {
            return;
        };

        match board {
            Board::Clipboard => {
                let (Some(manager), Some(device)) = (&self.data_device_manager, &self.data_device)
                else {
                    return;
                };

                let source = manager.create_copy_paste_source(&self.qh, TEXT_TYPES);

                source.set_selection(device, serial);

                self.copied.drop_clipboard();
                self.copied.clipboard = Some((source, text));
            }

            Board::Primary => {
                let (Some(manager), Some(device)) = (&self.primary_manager, &self.primary_device)
                else {
                    return;
                };

                let source = manager.create_selection_source(&self.qh, TEXT_TYPES);

                source.set_selection(device, serial);

                self.copied.primary = Some((source, text));
            }
        }
    }

    fn copy_secret(&mut self, mut text: String, clear_after: std::time::Duration) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        use wayland_client::Proxy;

        let (Some(serial), Some(manager), Some(device)) = (
            self.last_input_serial,
            &self.data_device_manager,
            &self.data_device,
        ) else {
            zeroize::Zeroize::zeroize(&mut text);
            return;
        };

        let mut types = TEXT_TYPES.to_vec();
        types.push(PASSWORD_HINT);

        let source = manager.create_copy_paste_source(&self.qh, types);
        source.set_selection(device, serial);

        let id = source.inner().id();

        self.copied.drop_clipboard();
        self.copied.clipboard = Some((source, text));
        self.copied.secret = true;

        // taken off the clipboard later, unless something else was copied since
        if let Some(loop_handle) = &self.loop_handle {
            let _ =
                loop_handle.insert_source(Timer::from_duration(clear_after), move |_, _, state| {
                    if state
                        .copied
                        .clipboard
                        .as_ref()
                        .is_some_and(|(source, _)| source.inner().id() == id)
                    {
                        state.copied.drop_clipboard();
                    }

                    TimeoutAction::Drop
                });
        }
    }

    fn paste(&mut self, board: Board, then: Then) {
        // reading our own offer would wait on this very thread to write it
        let own = match board {
            Board::Clipboard => self.copied.clipboard.as_ref().map(|(_, text)| text),
            Board::Primary => self.copied.primary.as_ref().map(|(_, text)| text),
        };

        if let Some(text) = own {
            then(text.clone());

            return;
        }

        let pipe = match board {
            Board::Clipboard => self
                .data_device
                .as_ref()
                .and_then(|device| device.data().selection_offer())
                .and_then(|offer| {
                    let kind = offer.with_mime_types(text_type)?;

                    offer.receive(kind).ok()
                }),

            Board::Primary => self
                .primary_device
                .as_ref()
                .and_then(|device| device.data().selection_offer())
                .and_then(|offer| {
                    let kind = offer.with_mime_types(text_type)?;

                    offer.receive(kind).ok()
                }),
        };

        let Some(pipe) = pipe else {
            return;
        };

        // the receive request has to reach the other program before it can write anything
        let _ = self.connection.flush();

        read_text(pipe, then);
    }
}

// the best kind of text an offer has
pub fn text_type(types: &[String]) -> Option<String> {
    TEXT_TYPES
        .iter()
        .find(|kind| types.iter().any(|offered| offered == *kind))
        .map(|kind| String::from(*kind))
}

// the most text read from a paste
const MAX_TEXT: u64 = 16 * 1024 * 1024;

// reads on its own thread, so a slow program never holds up a frame, and hands the text back
pub fn read_text(pipe: ReadPipe, then: Then) {
    let mut file = File::from(OwnedFd::from(pipe));

    thread::spawn(move || {
        let mut bytes = Vec::new();

        // the program that copied says how much: pasted text is far less than this
        let read = (&mut file).take(MAX_TEXT + 1).read_to_end(&mut bytes);
        if read.is_err() || bytes.len() as u64 > MAX_TEXT {
            return;
        }

        let text = String::from_utf8_lossy(&bytes).into_owned();

        changes::later(move || then(text));
    });
}

// writes on its own thread, a reader that is slow to take it can't stall drawing
pub fn write_bytes(pipe: WritePipe, bytes: Vec<u8>) {
    let mut file = File::from(OwnedFd::from(pipe));

    // wiped once written, as it can be a password copied with copy_secret
    let bytes = zeroize::Zeroizing::new(bytes);

    thread::spawn(move || {
        let _ = file.write_all(&bytes);
    });
}

impl PrimarySelectionDeviceHandler for WaylandState {
    // offers are read when pasting, nothing to do as they come
    fn selection(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpPrimarySelectionDeviceV1,
    ) {
    }
}

impl PrimarySelectionSourceHandler for WaylandState {
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
        _: String,
        pipe: WritePipe,
    ) {
        let Some((copied, text)) = &self.copied.primary else {
            return;
        };

        if copied.inner() == source {
            write_bytes(pipe, text.clone().into_bytes());
        }
    }

    // something else was selected since
    fn cancelled(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
    ) {
        if let Some((copied, _)) = &self.copied.primary
            && copied.inner() == source
        {
            self.copied.primary = None;
        }
    }
}
