#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use super::fingerprint::Fingerprint;
use super::{Apps, DesktopApp, data_dirs};
use crate::{ErrorKind, Service, ServiceError};

const DEFAULTS: &str = "Default Applications";
const ADDED: &str = "Added Associations";

/// Which app opens each kind of file and link: the freedesktop mimeapps.list
/// files, read in the spec's order, so it answers as xdg-open does. Changes go
/// to the user's own ~/.config/mimeapps.list, a line at a time, leaving the
/// rest of the file as it was.
#[derive(Default, PartialEq)]
pub struct MimeApps {
    // every app listed as the default for a kind, the first file's first
    defaults: HashMap<String, Vec<String>>,

    // apps said to open a kind they don't list themselves
    added: HashMap<String, Vec<String>>,

    // the defaults the user's own file sets, in its order
    own: Vec<(String, String)>,

    // with no default listed, what xdg-open falls back to: the apps the data
    // dirs' mimeinfo.cache lists for a kind, earlier dirs first
    cached: HashMap<String, Vec<String>>,
}

/*
 * the user's own list is watched, so a change from xdg-mime or a settings app shows up at
 * once; the system's lists (and mimeinfo.cache) change with a system switch, so they're looked
 * at when inotify says one may have (the links to them replaced included, see watch.rs), and
 * read again when one of them moved
 */
impl Service for MimeApps {
    fn new() -> Self {
        load()
    }

    fn interval() -> Duration {
        super::watch::POLL
    }

    fn update(&mut self) -> bool {
        let now = load();

        if *self == now {
            return false;
        }

        *self = now;

        true
    }

    fn listen() {
        if let Some(own) = config_home().map(|home| home.join("mimeapps.list")) {
            std::thread::spawn(move || {
                if let Some(folder) = own.parent() {
                    let _ = fs::create_dir_all(folder);
                }
                for () in crate::watch_file(&own.to_string_lossy()) {
                    reload();
                }
            });
        }

        let mut seen = system_fingerprint();
        loop {
            super::watch::wait(&system_files());
            let now = system_fingerprint();
            if now != seen {
                seen = now;
                reload();
            }
        }
    }
}

fn reload() {
    let mut apps = MimeApps::write();
    if !apps.update() {
        apps.quiet();
    }
}

fn system_fingerprint() -> Fingerprint {
    Fingerprint::of(system_files())
}

// the lists and every applications folder's cache
fn system_files() -> Vec<PathBuf> {
    let caches = data_dirs::find()
        .into_iter()
        .map(|dir| dir.join("applications/mimeinfo.cache"));
    lists().into_iter().chain(caches).collect()
}

impl MimeApps {
    /// The desktop file ids listed as the default for `kind`, most wanted first;
    /// the first one installed is the one that opens it.
    pub fn defaults_for(&self, kind: &str) -> &[String] {
        self.defaults.get(kind).map_or(&[], Vec::as_slice)
    }

    /// The defaults the user has set themselves, kind and app id, in their file's order.
    pub fn own(&self) -> &[(String, String)] {
        &self.own
    }

    /// The installed app that opens `kind`: the first listed default installed,
    /// else the one xdg-open falls back to from mimeinfo.cache.
    pub fn default_app(kind: &str) -> Option<DesktopApp> {
        let (ids, cached) = {
            let lists = Self::read();

            let cached = lists.cached.get(kind).cloned().unwrap_or_default();

            (lists.defaults_for(kind).to_vec(), cached)
        };

        let apps = Apps::read();

        ids.iter()
            .chain(&cached)
            .find_map(|id| apps.get(id).cloned())
    }

    /// The apps that can open `kind`, by name: the ones menus show that list it
    /// (or a kind it comes from), or are said to open it, and the default, even
    /// one menus don't show.
    pub fn handlers(kind: &str) -> Vec<DesktopApp> {
        let (added, defaults) = {
            let lists = Self::read();

            let added = lists.added.get(kind).cloned().unwrap_or_default();

            (added, lists.defaults_for(kind).to_vec())
        };

        let apps = Apps::read();

        apps.all()
            .iter()
            .filter(|app| {
                (app.shown() && app.opens_type(kind))
                    || added.contains(&app.id)
                    || defaults.contains(&app.id)
            })
            .cloned()
            .collect()
    }

    /// Makes `id` the default for each of `kinds`, said to open those it doesn't list.
    pub fn set_default(kinds: &[String], id: &str) -> Result<(), ServiceError> {
        let app = Apps::read().get(id).cloned();

        edit(|text| {
            let mut text = text.to_string();

            for kind in kinds {
                text = with_value(&text, DEFAULTS, kind, Some(&format!("{id};")));

                let listed = app.as_ref().is_some_and(|app| app.opens_type(kind));

                if !listed {
                    let others = values(&text, ADDED, kind)
                        .into_iter()
                        .filter(|other| other != id);

                    let list: String = std::iter::once(id.to_string())
                        .chain(others)
                        .map(|id| format!("{id};"))
                        .collect();

                    text = with_value(&text, ADDED, kind, Some(&list));
                }
            }

            text
        })?;

        // a desktop's own list would still win over the user's
        for path in desktop_lists() {
            if let Ok(text) = fs::read_to_string(&path) {
                let mut changed = text.clone();

                for kind in kinds {
                    changed = with_value(&changed, DEFAULTS, kind, None);
                }

                if changed != text {
                    write_atomic(&path, &changed)?;
                }
            }
        }

        Self::write().update();

        Ok(())
    }

    /// Takes the user's own default for each of `kinds` away, back to the system's.
    pub fn reset(kinds: &[String]) -> Result<(), ServiceError> {
        edit(|text| {
            kinds.iter().fold(text.to_string(), |text, kind| {
                with_value(&text, DEFAULTS, kind, None)
            })
        })?;

        Self::write().update();

        Ok(())
    }
}

fn config_home() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        && dir.is_absolute()
    {
        return Some(dir);
    }

    Some(PathBuf::from(env::var_os("HOME")?).join(".config"))
}

// the desktops named in XDG_CURRENT_DESKTOP, lowercased, as the file names use them
fn desktops() -> Vec<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|name| !name.is_empty())
        .map(str::to_lowercase)
        .collect()
}

// the user's own desktop-specific lists, like ~/.config/niri-mimeapps.list
fn desktop_lists() -> Vec<PathBuf> {
    let Some(home) = config_home() else {
        return Vec::new();
    };

    desktops()
        .iter()
        .map(|desktop| home.join(format!("{desktop}-mimeapps.list")))
        .collect()
}

// every list in the spec's order: config dirs, then data dirs' applications/
fn lists() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = config_home().into_iter().collect();

    let config_dirs = env::var("XDG_CONFIG_DIRS").unwrap_or_default();

    let config_dirs = if config_dirs.is_empty() {
        String::from("/etc/xdg")
    } else {
        config_dirs
    };

    dirs.extend(
        config_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );

    dirs.extend(
        data_dirs::find()
            .into_iter()
            .map(|dir| dir.join("applications")),
    );

    let desktops = desktops();

    let mut files = Vec::new();

    for dir in dirs {
        for desktop in &desktops {
            files.push(dir.join(format!("{desktop}-mimeapps.list")));
        }

        files.push(dir.join("mimeapps.list"));
    }

    files
}

fn load() -> MimeApps {
    let mut apps = MimeApps::default();

    let own = config_home().map(|home| home.join("mimeapps.list"));

    for path in lists() {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };

        for (section, kind, ids) in entries(&text) {
            let list = match section.as_str() {
                DEFAULTS => apps.defaults.entry(kind.clone()).or_default(),
                ADDED => apps.added.entry(kind.clone()).or_default(),
                _ => continue,
            };

            for id in &ids {
                if !list.contains(id) {
                    list.push(id.clone());
                }
            }

            if section == DEFAULTS
                && Some(&path) == own.as_ref()
                && let Some(first) = ids.first()
            {
                apps.own.push((kind, first.clone()));
            }
        }
    }

    for dir in data_dirs::find() {
        let Ok(text) = fs::read_to_string(dir.join("applications/mimeinfo.cache")) else {
            continue;
        };

        for (section, kind, ids) in entries(&text) {
            if section != "MIME Cache" {
                continue;
            }

            let list = apps.cached.entry(kind).or_default();

            for id in ids {
                if !list.contains(&id) {
                    list.push(id);
                }
            }
        }
    }

    apps
}

// each `kind=id;id;` line, with the section it's in
fn entries(text: &str) -> Vec<(String, String, Vec<String>)> {
    let mut section = String::new();

    let mut entries = Vec::new();

    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_string();
        } else if let Some((kind, ids)) = line.split_once('=')
            && !line.starts_with('#')
        {
            let ids = ids
                .split(';')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(String::from)
                .collect();

            entries.push((section.clone(), kind.trim().to_string(), ids));
        }
    }

    entries
}

fn values(text: &str, section: &str, kind: &str) -> Vec<String> {
    entries(text)
        .into_iter()
        .find(|(at, key, _)| at == section && key == kind)
        .map(|(.., ids)| ids)
        .unwrap_or_default()
}

/*
 * `text` with `kind` set to `value` in `section`, or taken out for none;
 * a missing section goes at the end, a new line after the section's last
 */
fn with_value(text: &str, section: &str, kind: &str, value: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

    let header = format!("[{section}]");

    let line = value.map(|value| format!("{kind}={value}"));

    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        if let Some(line) = line {
            if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                lines.push(String::new());
            }

            lines.push(header);
            lines.push(line);
        }

        return finish(lines);
    };

    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim().starts_with('['))
        .map_or(lines.len(), |at| start + 1 + at);

    let at = (start + 1..end).find(|&at| {
        lines[at]
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == kind)
    });

    match (at, line) {
        (Some(at), Some(line)) => lines[at] = line,
        (Some(at), None) => {
            lines.remove(at);
        }
        (None, Some(line)) => {
            // after the section's last entry, before the blank lines between sections
            let last = (start..end)
                .rev()
                .find(|&at| !lines[at].trim().is_empty())
                .unwrap_or(start);

            lines.insert(last + 1, line);
        }
        (None, None) => {}
    }

    finish(lines)
}

fn finish(lines: Vec<String>) -> String {
    let mut text = lines.join("\n");

    text.push('\n');

    text
}

// changes the user's own list, made if it's missing; a link (home-manager's) is left alone
fn edit(change: impl FnOnce(&str) -> String) -> Result<(), ServiceError> {
    let path = config_home()
        .ok_or_else(|| ServiceError::new(ErrorKind::Gone, "no home folder"))?
        .join("mimeapps.list");

    // managed elsewhere, like by home-manager's xdg.mimeApps
    if path.is_symlink() {
        return Err(ServiceError::new(
            ErrorKind::Blocked,
            format!(
                "{} is a link, managed elsewhere (home-manager's xdg.mimeApps?)",
                path.display()
            ),
        ));
    }

    let text = fs::read_to_string(&path).unwrap_or_default();

    let changed = change(&text);

    if changed == text {
        return Ok(());
    }

    write_atomic(&path, &changed)
}

// into place by a rename, so nothing reads half a file
fn write_atomic(path: &std::path::Path, text: &str) -> Result<(), ServiceError> {
    let temp = path.with_extension("list.new");

    let failed = |path: &std::path::Path, error: std::io::Error| {
        let mut failure = ServiceError::from(error);
        failure.message = format!("{}: {}", path.display(), failure.message);
        failure
    };

    fs::write(&temp, text).map_err(|error| failed(&temp, error))?;

    fs::rename(&temp, path).map_err(|error| failed(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "[Default Applications]\nx-scheme-handler/bitwarden=bitwarden.desktop\n\n[Added Associations]\n";

    #[test]
    fn sets_a_new_default_in_its_section() {
        let text = with_value(LIST, DEFAULTS, "text/html", Some("firefox.desktop;"));

        assert_eq!(
            text,
            "[Default Applications]\nx-scheme-handler/bitwarden=bitwarden.desktop\ntext/html=firefox.desktop;\n\n[Added Associations]\n"
        );
    }

    #[test]
    fn replaces_and_takes_away() {
        let text = with_value(
            LIST,
            DEFAULTS,
            "x-scheme-handler/bitwarden",
            Some("other.desktop;"),
        );

        assert!(text.contains("x-scheme-handler/bitwarden=other.desktop;\n"));

        let text = with_value(&text, DEFAULTS, "x-scheme-handler/bitwarden", None);

        assert!(!text.contains("bitwarden"));
        assert!(text.contains("[Added Associations]"));
    }

    #[test]
    fn adds_a_missing_section() {
        let text = with_value("", DEFAULTS, "inode/directory", Some("nautilus.desktop;"));

        assert_eq!(
            text,
            "[Default Applications]\ninode/directory=nautilus.desktop;\n"
        );

        let text = with_value(&text, ADDED, "image/png", Some("loupe.desktop;"));

        assert!(
            text.ends_with("nautilus.desktop;\n\n[Added Associations]\nimage/png=loupe.desktop;\n")
        );
    }

    #[test]
    fn an_empty_section_gets_its_line() {
        let text = with_value(LIST, ADDED, "image/png", Some("loupe.desktop;"));

        assert!(text.ends_with("[Added Associations]\nimage/png=loupe.desktop;\n"));
    }

    #[test]
    fn reads_every_id_in_order() {
        let text = "[Default Applications]\ntext/plain=a.desktop;b.desktop;\n# note=x\n";

        assert_eq!(
            values(text, DEFAULTS, "text/plain"),
            vec![String::from("a.desktop"), String::from("b.desktop")]
        );
        assert_eq!(entries(text).len(), 1);
    }
}
