use std::path::{Path, PathBuf};

use crate::launch::Launch;
use crate::spawn_activated;

use super::{exec, mime};

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopApp {
    // the desktop file's id, like "org.mozilla.firefox.desktop"; set once it's read
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) exec: String,
    // the Exec line as written, with its %f style placeholders
    pub(crate) exec_codes: String,
    // the kinds of file it opens (MimeType)
    pub(crate) mime_types: Vec<String>,
    pub(crate) icon: Option<String>,
    pub(crate) icon_path: Option<PathBuf>,
    pub(crate) description: Option<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) actions: Vec<DesktopAction>,
    // false for NoDisplay: kept out of menus, but it may still open links or files
    pub(crate) shown: bool,
}

// one of an app's [Desktop Action] sections: another way to start it, like "New Private Window"
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopAction {
    pub(crate) name: String,
    pub(crate) exec: String,
    pub(crate) icon: Option<String>,
}

impl DesktopApp {
    // the desktop file's id, its path under applications/ with / as -, which stays the
    // same while the app is installed: a key to remember it by
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    // the command line, without the %f style placeholders
    pub fn exec(&self) -> &str {
        &self.exec
    }

    // the icon's name in the icon theme, like "firefox"
    pub fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    // the image file for the icon, a png when the theme has one
    pub fn icon_path(&self) -> Option<&Path> {
        self.icon_path.as_deref()
    }

    // the entry's Comment, or its GenericName like "Web Browser" when it has none
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    // the freedesktop Categories, like "Development" or "Network" (no trailing empty)
    pub fn categories(&self) -> &[String] {
        &self.categories
    }

    // the entry's Keywords, extra search terms the author added
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    // the entry's Actions, in the order it lists them, each with its own command line
    pub fn actions(&self) -> &[DesktopAction] {
        &self.actions
    }

    // starts the program and moves on without waiting for it, in a scope named for it
    pub fn launch(&self) {
        Launch::shell(&self.exec).app_id(Some(&self.id)).activated();
    }

    // the kinds of file it says it opens, like "text/markdown"
    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    // whether it opens `file`, by the kind of file its name says it is
    pub fn opens(&self, file: &Path) -> bool {
        mime::mime_type(file).is_some_and(|kind| self.opens_type(&kind))
    }

    // whether it opens files of `kind`, like "image/png", or of a kind that one comes from
    pub fn opens_type(&self, kind: &str) -> bool {
        mime::opens(&self.mime_types, kind)
    }

    // whether menus show it; a NoDisplay entry only opens links or files
    pub fn shown(&self) -> bool {
        self.shown
    }

    // the commands, as argument lists, that open `files` with it, one a file for a program that
    // takes one at a time; a file is always one argument, never text a shell reads. None when
    // the entry's Exec line can't be read
    pub fn commands_with(&self, files: &[PathBuf]) -> Vec<Vec<String>> {
        exec::fill_field_codes(&self.exec_codes, files).unwrap_or_else(|| {
            eprintln!("amane: {}'s Exec line can't be read", self.id);
            Vec::new()
        })
    }

    // starts it on `files`, and moves on without waiting; with none, as launch() does
    pub fn launch_with(&self, files: &[PathBuf]) {
        if files.is_empty() {
            self.launch();
            return;
        }

        for command in self.commands_with(files) {
            Launch::argv(&command).app_id(Some(&self.id)).activated();
        }
    }
}

impl DesktopAction {
    pub fn name(&self) -> &str {
        &self.name
    }

    // the command line, without the %f style placeholders
    pub fn exec(&self) -> &str {
        &self.exec
    }

    // the icon's name in the icon theme, or none to use the app's
    pub fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    // starts the action and moves on without waiting for it
    pub fn launch(&self) {
        spawn_activated(&self.exec);
    }
}
