#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! Waiting for a set of files and folders to maybe change, instead of looking every half
//! minute: inotify on each path, on the folder it sits in, and on the folder of every link on
//! the way to it. On NixOS the data folders are links into the store (`/run/current-system`,
//! `/etc/profiles/per-user/…`) that a switch replaces, and a watch on the store folder itself
//! would never fire; the link being replaced in its own folder does.
//!
//! A wake only says something may have changed: the caller compares a
//! [`Fingerprint`](super::fingerprint::Fingerprint) as before, so a stray event costs a few
//! `stat`s. The watches are made again after each wake, as the links may point elsewhere now.
//! Without inotify it falls back to looking every [`POLL`].

use std::collections::BTreeSet;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use inotify::{Inotify, WatchMask};

/// How often to look anyway: without inotify, and as a backstop with it.
pub(crate) const POLL: Duration = Duration::from_secs(30);
const BACKSTOP: Duration = Duration::from_secs(600);
// a system switch or a package install changes many files at once
const SETTLE: Duration = Duration::from_millis(500);

/// Waits until `paths` may have changed, or the fallback time runs out.
pub(crate) fn wait(paths: &[PathBuf]) {
    let Some(inotify) = watch(paths) else {
        std::thread::sleep(POLL);
        return;
    };
    if !readable(&inotify, BACKSTOP) {
        return;
    }
    // let the rest of a burst land, then drop it: the caller looks once
    std::thread::sleep(SETTLE);
    let mut buffer = [0; 4096];
    let mut inotify = inotify;
    while readable(&inotify, Duration::ZERO) {
        if inotify.read_events(&mut buffer).is_err() {
            break;
        }
    }
}

fn watch(paths: &[PathBuf]) -> Option<Inotify> {
    let inotify = Inotify::init().ok()?;
    let contents = WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MOVED_TO
        | WatchMask::MOVED_FROM
        | WatchMask::CLOSE_WRITE
        | WatchMask::ATTRIB;
    let mut any = false;
    for folder in folders(paths) {
        any |= inotify.watches().add(&folder, contents).is_ok();
    }
    any.then_some(inotify)
}

/// The folders to watch for `paths`: each one that is a folder, the folder each sits in (or
/// the nearest one that exists), and the folder of each link on the way to it.
fn folders(paths: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut folders = BTreeSet::new();
    for path in paths {
        if path.is_dir() {
            folders.insert(path.clone());
        }
        let mut at: &Path = path;
        while let Some(parent) = at.parent() {
            if (at.is_symlink() || !at.exists()) && parent.is_dir() {
                folders.insert(parent.to_path_buf());
            }
            at = parent;
        }
        if let Some(parent) = path.parent().filter(|parent| parent.is_dir()) {
            folders.insert(parent.to_path_buf());
        }
    }
    folders
}

// whether an event is waiting, within `timeout`
fn readable(inotify: &Inotify, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd: inotify.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd, which outlives the call; the fd is the inotify's own.
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    ready > 0 && poll.revents & libc::POLLIN != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_on_the_way_is_watched_in_its_folder() {
        let root = std::env::temp_dir().join(format!("amane-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("store/share/applications")).expect("folder");
        std::fs::create_dir_all(root.join("run")).expect("folder");
        std::os::unix::fs::symlink(root.join("store"), root.join("run/current")).expect("link");

        let apps = root.join("run/current/share/applications");
        let missing = root.join("home/applications");
        let folders = folders(&[apps.clone(), missing]);
        // the folder itself, and where the link is replaced
        assert!(folders.contains(&apps));
        assert!(folders.contains(&root.join("run")));
        // a folder not made yet: the nearest that exists
        assert!(folders.contains(&root));

        let _ = std::fs::remove_dir_all(&root);
    }
}
