mod data_dirs;
mod defaults;
mod desktop_app;
mod exec;
pub(crate) mod fingerprint;
mod icons;
mod mime;
mod parse;
mod walk;
pub(crate) mod watch;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::Service;

pub use defaults::MimeApps;
pub use desktop_app::{DesktopAction, DesktopApp};

// the programs a launcher can show, read from the .desktop files in the xdg data dirs
#[derive(Default)]
pub struct Apps {
    // the ones menus show
    list: Vec<DesktopApp>,

    // those and the NoDisplay ones, which only open links or files
    all: Vec<DesktopApp>,
}

/*
 * the applications folders are looked at when inotify says one may have changed (a system
 * switch replacing the links to them included, see watch.rs), and scanned again only when one
 * did, so programs installed while the shell runs show up without the scan (which reads every
 * icon theme) running each time; without inotify they're looked at every half minute
 */
impl Service for Apps {
    // empty at first: the scan walks every icon theme, which takes seconds
    fn new() -> Self {
        Self::default()
    }

    fn interval() -> Duration {
        watch::POLL
    }

    // scanned outside the lock, so a view reading the list never waits on the disk
    fn listen() {
        let mut seen = None;
        loop {
            let folders: Vec<_> = data_dirs::find()
                .into_iter()
                .map(|dir| dir.join("applications"))
                .collect();
            let now = fingerprint::Fingerprint::of(&folders);
            if seen.as_ref() == Some(&now) {
                watch::wait(&folders);
                continue;
            }
            seen = Some(now);

            let all = scan();

            let mut apps = Self::write();

            if apps.all == all {
                apps.quiet();
            } else {
                apps.list = all.iter().filter(|app| app.shown).cloned().collect();
                apps.all = all;
            }

            drop(apps);

            watch::wait(&folders);
        }
    }
}

impl Apps {
    // sorted by name, without the ones marked hidden or not to be shown
    pub fn list(&self) -> &[DesktopApp] {
        &self.list
    }

    // every entry, the NoDisplay ones too: those open links or files without showing in menus
    pub fn all(&self) -> &[DesktopApp] {
        &self.all
    }

    // the entry with the desktop file id `id`, like "org.mozilla.firefox.desktop"
    pub fn get(&self, id: &str) -> Option<&DesktopApp> {
        self.all.iter().find(|app| app.id == id)
    }

    // what kind of file a name like "notes.md" is, as the shared mime database says
    pub fn mime_type(name: &str) -> Option<String> {
        mime::mime_type(std::path::Path::new(name))
    }

    // an icon file for `icon`, a theme icon name or a full path, for an app with no desktop entry
    pub fn find_icon(icon: &str) -> Option<PathBuf> {
        find_icon(icon)
    }
}

/*
 * an icon name from the user's theme, or a full path as it is; the theme's folders are read
 * the first time and kept, so an icon theme switched while the program runs shows after a
 * restart here (a rescan of the apps reads them again)
 */
pub(crate) fn find_icon(icon: &str) -> Option<PathBuf> {
    static FOLDERS: std::sync::OnceLock<Vec<Vec<PathBuf>>> = std::sync::OnceLock::new();

    parse::find_icon(icon, FOLDERS.get_or_init(icons::read_icon_folders))
}

fn scan() -> Vec<DesktopApp> {
    let icon_folders = icons::read_icon_folders();

    // a file in an earlier dir overrides one with the same id in a later dir
    let mut seen = HashSet::new();

    let mut apps = Vec::new();

    for dir in data_dirs::find() {
        let applications = dir.join("applications");

        for path in walk::files(&applications) {
            let Some(id) = parse::id(&applications, &path) else {
                continue;
            };

            if !seen.insert(id.clone()) {
                continue;
            }

            let Some(mut app) = parse::read(&path, &icon_folders) else {
                continue;
            };

            app.id = id;
            apps.push(app);
        }
    }

    apps.sort_by_key(|app| app.name().to_lowercase());

    apps
}
