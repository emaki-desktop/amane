use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct Notification {
    pub(crate) id: u32,

    pub(crate) app_name: String,

    pub(crate) summary: String,

    pub(crate) body: String,

    pub(crate) icon: String,

    pub(crate) image: String,

    // the icon as a file, found once when it arrives
    pub(crate) icon_path: Option<PathBuf>,

    pub(crate) urgency: Urgency,

    // the "default" action is kept apart, it has no button of its own
    pub(crate) actions: Vec<Action>,

    pub(crate) has_default: bool,

    // stays open after an action, until it is dismissed or the sender closes it
    pub(crate) resident: bool,

    // shown, but not worth keeping once it is gone
    pub(crate) transient: bool,

    // the kind of event, like "email.arrived" or "device.added"
    pub(crate) category: String,

    // the sender's .desktop file name without ".desktop", like "firefox"
    pub(crate) desktop_entry: String,

    pub(crate) timeout: Timeout,

    pub(crate) received: SystemTime,

    // came back from a saved history, see Notifications::restore
    pub(crate) restored: bool,
}

// how long the sender asked for its notification to show
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Timeout {
    // the sender left it to the shell
    #[default]
    Default,

    // until the user acts on it
    Never,

    After(Duration),
}

// a button a notification asks for, like "reply" or "open"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub(crate) key: String,

    pub(crate) label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    Low,

    // what a notification without the hint gets
    #[default]
    Normal,

    Critical,
}

impl Notification {
    // pass this to Notifications::dismiss
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    // the one-line title
    pub fn summary(&self) -> &str {
        &self.summary
    }

    // may be empty, and may hold simple markup like <b>
    pub fn body(&self) -> &str {
        &self.body
    }

    // an icon name like "firefox", a file path, or empty
    pub fn icon(&self) -> &str {
        &self.icon
    }

    // a picture for this one notification, like a sender's avatar: a file path, a file url, or empty
    pub fn image(&self) -> &str {
        &self.image
    }

    // the icon or image as a file to draw: a theme icon found by name, or a path as it was sent
    pub fn icon_path(&self) -> Option<&Path> {
        self.icon_path.as_deref()
    }

    pub fn urgency(&self) -> Urgency {
        self.urgency
    }

    // the buttons to show, in the order the sender gave them
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    // whether clicking the notification itself does something, see Notifications::click
    pub fn has_default_action(&self) -> bool {
        self.has_default
    }

    // whether the sender asked to keep it open after an action
    pub fn resident(&self) -> bool {
        self.resident
    }

    // whether it should be left out of a history once it has been shown
    pub fn transient(&self) -> bool {
        self.transient
    }

    // like "email.arrived", or empty; the spec lists the common ones
    pub fn category(&self) -> &str {
        &self.category
    }

    // the sending program's desktop file id, like "firefox", or empty
    pub fn desktop_entry(&self) -> &str {
        &self.desktop_entry
    }

    // the shell decides how long to show it, amane never closes one by itself
    pub fn timeout(&self) -> Timeout {
        self.timeout
    }

    // when it arrived, or when the sender last replaced it
    pub fn received(&self) -> SystemTime {
        self.received
    }
}

impl Action {
    // pass this to Notifications::invoke
    pub fn key(&self) -> &str {
        &self.key
    }

    // the text to show on the button
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Timeout {
    // the spec sends milliseconds: -1 leaves it to the server, 0 never expires
    pub(crate) fn from_millis(millis: f64) -> Self {
        if millis < 0.0 {
            Self::Default
        } else if millis == 0.0 {
            Self::Never
        } else {
            Self::After(Duration::from_millis(millis as u64))
        }
    }
}

impl Urgency {
    // the spec sends urgency as a byte: 0 low, 1 normal, 2 critical
    pub(crate) fn from_level(level: f64) -> Self {
        match level as u8 {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_timeout() {
        assert_eq!(Timeout::from_millis(-1.0), Timeout::Default);
        assert_eq!(Timeout::from_millis(0.0), Timeout::Never);
        assert_eq!(
            Timeout::from_millis(2000.0),
            Timeout::After(Duration::from_secs(2))
        );
    }

    #[test]
    fn reads_the_urgency() {
        assert_eq!(Urgency::from_level(0.0), Urgency::Low);
        assert_eq!(Urgency::from_level(1.0), Urgency::Normal);
        assert_eq!(Urgency::from_level(2.0), Urgency::Critical);
    }
}
