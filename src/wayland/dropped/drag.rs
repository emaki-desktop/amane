//! Drags read off the event loop, so a slow source never stalls a frame, and data dragged out
//! of amane to other programs.

use std::fs::File;
use std::io::Read;
use std::mem;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::thread;

use smithay_client_toolkit::data_device_manager::{data_offer::DragOffer, data_source::DragSource};
use wayland_client::protocol::{wl_data_device_manager::DndAction, wl_surface::WlSurface};

use crate::changes;
use crate::clipboard::DragData;

use super::super::{
    WaylandState,
    clipboard::{TEXT_TYPES, text_type},
};

use super::*;

/*
 * what another program offers is read on a thread of its own, so a slow
 * source never stalls a frame, and handed back here for the event loop
 */
pub(super) static ARRIVED: Mutex<Vec<Arrived>> = Mutex::new(Vec::new());

pub(super) enum Arrived {
    // the files held over a window, for the drag that entered as `epoch`
    Held(u64, Vec<PathBuf>),

    // what was let go, none when it couldn't be read
    Dropped(u64, Option<DragData>),
}

// the drags whose data is still being read
#[derive(Default)]
pub(crate) struct Drags {
    // counts each drag that enters, so a list read for one that already left is let go
    pub(super) epoch: u64,

    // the surface a list is being read for
    pub(super) reading: Option<(u64, WlSurface)>,

    // where the pointer is meanwhile, so the widget under it is asked once the list arrives
    pub(super) at: (f64, f64),

    // offers let go, waiting for their data before they are finished
    pub(super) dropped: Vec<(u64, DragOffer)>,
}

impl WaylandState {
    /*
     * starts dragging data out of the window the left button went down on; the compositor
     * takes over the pointer from here until it is let go
     */
    pub fn start_drag_out(&mut self, surface: &WlSurface, data: DragData) {
        let (Some(manager), Some(device), Some(serial)) = (
            &self.data_device_manager,
            &self.data_device,
            self.last_press_serial,
        ) else {
            return;
        };

        let types: Vec<&str> = match &data {
            DragData::Files(_) => vec![URI_LIST, TEXT_TYPES[0], TEXT_TYPES[2]],
            DragData::Text(_) => TEXT_TYPES.to_vec(),
        };

        let source: DragSource =
            manager.create_drag_and_drop_source(&self.qh, types, DndAction::Copy);

        source.start_drag(device, surface, None, serial);

        self.dragging_out = Some((source, data));
    }

    // asks the other program to write what it offers, none when it wouldn't
    pub(super) fn receive(&self, offer: &DragOffer, kind: String) -> Option<File> {
        let pipe = offer.receive(kind).ok()?;

        // the receive request has to reach the other program before it can write anything
        let _ = self.connection.flush();

        Some(File::from(OwnedFd::from(pipe)))
    }

    // what the reading threads handed back, run from the event loop's wake
    pub(crate) fn handle_arrived_drags(&mut self) {
        let arrived = mem::take(&mut *ARRIVED.lock().unwrap_or_else(PoisonError::into_inner));

        for arrived in arrived {
            match arrived {
                Arrived::Held(epoch, files) => {
                    let Some((reading, _)) = &self.drags.reading else {
                        continue;
                    };

                    // the drag left, or another came in, while its list was read
                    if *reading != epoch {
                        continue;
                    }

                    let Some((_, surface)) = self.drags.reading.take() else {
                        continue;
                    };

                    self.files_held = Some((surface, files));

                    let (x, y) = self.drags.at;

                    self.files_moved(x, y);
                }

                Arrived::Dropped(epoch, data) => {
                    let Some(index) = self.drags.dropped.iter().position(|(e, _)| *e == epoch)
                    else {
                        continue;
                    };

                    let (_, offer) = self.drags.dropped.remove(index);

                    self.let_go(&offer, data);
                }
            }
        }
    }

    // finishes a let-go offer and hands what it carried to the widget under it
    pub(super) fn let_go(&mut self, offer: &DragOffer, dropped: Option<DragData>) {
        offer.finish();
        offer.destroy();

        let surface = offer.surface.clone();

        let Some(window) = self.window(&surface) else {
            return;
        };

        let x = offer.x as f32;
        let y = offer.y as f32;

        let handled = match dropped {
            Some(DragData::Files(files)) if !files.is_empty() => {
                window.pointer.drop_files(x, y, files)
            }
            Some(DragData::Text(text)) => window.pointer.drop_text(x, y, text),
            _ => false,
        };

        if handled {
            self.request_frame_on(&surface);
        }
    }

    // the held files moved to (x, y): the widget under them says whether they can be let go there
    pub(super) fn files_moved(&mut self, x: f64, y: f64) {
        let Some((surface, files)) = self.files_held.clone() else {
            return;
        };

        let Some(offer) = self.drag_offer() else {
            return;
        };

        let Some(window) = self.window(&surface) else {
            return;
        };

        // a window that doesn't ask takes files anywhere, as before widgets could say
        let answer = window.pointer.files_over(x as f32, y as f32, &files);

        if answer == Some(false) {
            offer.accept_mime_type(offer.serial, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
        } else {
            offer.accept_mime_type(offer.serial, Some(String::from(URI_LIST)));
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        }

        if answer.is_some() {
            self.request_frame_on(&surface);
        }
    }
}

// the most read from a drop
const MAX_DROPPED: u64 = 16 * 1024 * 1024;

// reads a pipe to its end on its own thread and wakes the event loop with what came
pub(super) fn read_later(
    file: File,
    then: impl FnOnce(Option<Vec<u8>>) -> Arrived + Send + 'static,
) {
    thread::spawn(move || {
        let mut bytes = Vec::new();

        // the other program says how much: a drop of links or text is far less than this
        let read = file
            .take(MAX_DROPPED + 1)
            .read_to_end(&mut bytes)
            .ok()
            .filter(|_| bytes.len() as u64 <= MAX_DROPPED)
            .map(|_| bytes);

        ARRIVED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(then(read));

        changes::wake();
    });
}

// whether a drag offers files, and the best kind of text it offers
pub(super) fn offered(offer: &DragOffer) -> (bool, Option<String>) {
    offer.with_mime_types(|types| (types.iter().any(|kind| kind == URI_LIST), text_type(types)))
}

// files as other programs take them, one file:// link per line
pub(super) fn uri_list(files: &[PathBuf]) -> String {
    let mut list = String::new();

    for file in files {
        list.push_str("file://");
        list.push_str(&encode(&file.to_string_lossy()));
        list.push_str("\r\n");
    }

    list
}

// files as text, for a program that only takes text
pub(super) fn paths(files: &[PathBuf]) -> String {
    let lines: Vec<String> = files
        .iter()
        .map(|file| file.to_string_lossy().into_owned())
        .collect();

    lines.join("\n")
}

// the letters a link can't hold as they are become %20 and the like
pub(super) fn encode(path: &str) -> String {
    let mut encoded = String::new();

    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }

    encoded
}
