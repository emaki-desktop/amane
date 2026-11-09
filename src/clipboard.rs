#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use crate::changes;

/// The clipboard Ctrl+C and Ctrl+V use, shared with every other program.
pub struct Clipboard;

/// The text selected last, which a middle click pastes.
pub struct PrimarySelection;

impl Clipboard {
    /// Puts text on the clipboard, for other programs to paste; it has to come from
    /// input, like a key or a click, or the compositor may turn it down.
    pub fn copy(text: impl Into<String>) {
        request(Request::Copy(Board::Clipboard, text.into()));
    }

    /// Reads the clipboard and calls `then` with its text, once the program that copied it
    /// has answered; it runs on the event loop like a handler, and not at all when there is
    /// no text on it.
    pub fn paste(then: impl FnOnce(String) + Send + 'static) {
        request(Request::Paste(Board::Clipboard, Box::new(then)));
    }

    /// Puts a password on the clipboard the way password managers do: it's also offered as
    /// x-kde-passwordManagerHint, so clipboard histories leave it out, and it's taken off the
    /// clipboard after `clear_after` if nothing was copied since. The text is wiped once gone.
    pub fn copy_secret(mut text: zeroize::Zeroizing<String>, clear_after: std::time::Duration) {
        // moved out, not copied: the one copy is the clipboard's, which wipes it when it goes
        let text = std::mem::take(&mut *text);
        request(Request::CopySecret(text, clear_after));
    }
}

impl PrimarySelection {
    /// The same as Clipboard::copy, for the text a middle click pastes.
    pub fn copy(text: impl Into<String>) {
        request(Request::Copy(Board::Primary, text.into()));
    }

    /// The same as Clipboard::paste.
    pub fn paste(then: impl FnOnce(String) + Send + 'static) {
        request(Request::Paste(Board::Primary, Box::new(then)));
    }
}

/// What a rectangle hands over when it is dragged out to another program.
#[derive(Clone, Debug, PartialEq)]
pub enum DragData {
    /// Text, offered in the usual text types.
    Text(String),
    /// Files, offered as a uri-list and as their paths in text.
    Files(Vec<PathBuf>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Board {
    Clipboard,
    Primary,
}

pub(crate) type Then = Box<dyn FnOnce(String) + Send>;

pub(crate) enum Request {
    Copy(Board, String),
    CopySecret(String, std::time::Duration),
    Paste(Board, Then),
}

// done by the event loop when it wakes, which holds what the compositor needs
pub(crate) static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());

fn request(request: Request) {
    REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);

    changes::wake();
}
