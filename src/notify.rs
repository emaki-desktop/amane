#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::services::notifications::{DEFAULT_ACTION, find_icon};
use crate::services::worker;
use crate::{Action, Argument, Bus, Notification, Notifications, Service, Timeout, Urgency};

// the app-id App::run started with, which notifications are sent under
pub(crate) static APP_NAME: OnceLock<String> = OnceLock::new();

type Callback = Arc<dyn Fn() + Send + Sync>;

// what each sent notification's buttons do, by its id and then the button's key
static CALLBACKS: Mutex<Option<HashMap<u32, HashMap<String, Callback>>>> = Mutex::new(None);

/// A desktop notification for the notification server to show, like the shell's:
/// Notify::new("Saved").body("notes.txt").send().
pub struct Notify {
    summary: String,
    body: String,
    icon: String,
    image: String,
    urgency: Urgency,
    transient: bool,
    resident: bool,
    category: String,
    desktop_entry: String,
    timeout: Timeout,
    replaces: u32,
    actions: Vec<(String, String, Callback)>,
    on_click: Option<Callback>,
}

/// A notification once sent, to replace or close it later. Its id arrives once the server
/// answers, so a handle used before then replaces nothing and closes nothing.
#[derive(Clone, Debug, Default)]
pub struct Handle(Arc<AtomicU32>);

impl Handle {
    /// The server's id, 0 until it has answered or when it couldn't be shown.
    pub fn id(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }

    /// Takes the notification down, as its sender closing it.
    pub fn close(&self) {
        let id = self.id();
        if id == 0 {
            return;
        }
        if Notifications::read().running() {
            Notifications::close_sent(id);
        } else {
            worker::run(move || {
                Bus::session().call(
                    "org.freedesktop.Notifications",
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    "CloseNotification",
                    &[Argument::Unsigned(id)],
                );
            });
        }
    }
}

impl Notify {
    /// A notification with this title; send() shows it.
    pub fn new(summary: &str) -> Self {
        Self {
            summary: String::from(summary),
            body: String::new(),
            icon: String::new(),
            image: String::new(),
            urgency: Urgency::Normal,
            transient: false,
            resident: false,
            category: String::new(),
            desktop_entry: String::new(),
            timeout: Timeout::Default,
            replaces: 0,
            actions: Vec::new(),
            on_click: None,
        }
    }

    /// The text under the title.
    pub fn body(mut self, body: &str) -> Self {
        self.body = String::from(body);

        self
    }

    /// An icon theme name, like "document-save", or a file path.
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = String::from(icon);

        self
    }

    /// A picture of a file this notification is about, like a screenshot, shown on the card.
    pub fn image(mut self, path: &str) -> Self {
        self.image = String::from(path);

        self
    }

    /// Low, normal or critical; servers often keep a critical one up until it is dismissed.
    pub fn urgency(mut self, urgency: Urgency) -> Self {
        self.urgency = urgency;

        self
    }

    /// Shown, but not kept in a history once it goes.
    pub fn transient(mut self, transient: bool) -> Self {
        self.transient = transient;

        self
    }

    /// Stays up after a button is pressed, until it is dismissed or replaced.
    pub fn resident(mut self, resident: bool) -> Self {
        self.resident = resident;

        self
    }

    /// The kind of event, like "device.added" or "network.disconnected".
    pub fn category(mut self, category: &str) -> Self {
        self.category = String::from(category);

        self
    }

    /// Who it is from, as a .desktop name without ".desktop": a shell groups and mutes by it.
    pub fn desktop_entry(mut self, desktop_entry: &str) -> Self {
        self.desktop_entry = String::from(desktop_entry);

        self
    }

    /// How long it shows; Duration::ZERO keeps it until it is acted on.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = if timeout.is_zero() {
            Timeout::Never
        } else {
            Timeout::After(timeout)
        };

        self
    }

    /// Takes the place of an earlier one, keeping its spot, like "Ejecting…" turning into
    /// "Ejected".
    pub fn replaces(mut self, earlier: &Handle) -> Self {
        self.replaces = earlier.id();

        self
    }

    /// A button; `run` runs on amane's worker thread when it is pressed.
    pub fn action(
        mut self,
        key: &str,
        label: &str,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.actions
            .push((String::from(key), String::from(label), Arc::new(run)));

        self
    }

    /// What clicking the notification itself does, in place of dismissing it.
    pub fn on_click(mut self, run: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_click = Some(Arc::new(run));

        self
    }

    /// Shows it. Straight into the list when this program is the notification server, else
    /// sent over the bus on the worker thread, so a slow server never holds up a frame.
    pub fn send(self) -> Handle {
        let handle = Handle::default();

        let mut callbacks: HashMap<String, Callback> = HashMap::new();
        for (key, _, run) in &self.actions {
            callbacks.insert(key.clone(), run.clone());
        }
        if let Some(run) = &self.on_click {
            callbacks.insert(String::from(DEFAULT_ACTION), run.clone());
        }

        if Notifications::read().running() {
            let replaces = self.replaces;
            let id = Notifications::write().add_sent(self.into_notification(), replaces);
            remember(id, callbacks);
            handle.0.store(id, Ordering::Release);

            return handle;
        }

        if !callbacks.is_empty() {
            hear_actions();
        }

        let sent = handle.clone();
        worker::run(move || {
            let id = self.send_over_bus();
            remember(id, callbacks);
            sent.0.store(id, Ordering::Release);
        });

        handle
    }

    fn app_name() -> String {
        APP_NAME
            .get()
            .cloned()
            .unwrap_or_else(|| String::from("amane"))
    }

    fn into_notification(self) -> Notification {
        let icon_path = if self.image.is_empty() {
            find_icon(&self.icon, &self.desktop_entry)
        } else {
            Some(self.image.clone().into())
        };

        Notification {
            id: 0,
            app_name: Self::app_name(),
            summary: self.summary,
            body: self.body,
            icon: self.icon,
            image: self.image,
            icon_path,
            urgency: self.urgency,
            actions: self
                .actions
                .into_iter()
                .map(|(key, label, _)| Action { key, label })
                .collect(),
            has_default: self.on_click.is_some(),
            resident: self.resident,
            transient: self.transient,
            category: self.category,
            desktop_entry: self.desktop_entry,
            timeout: self.timeout,
            received: SystemTime::now(),
            restored: false,
        }
    }

    // the id the server gave, 0 when it couldn't be reached
    fn send_over_bus(&self) -> u32 {
        let urgency = match self.urgency {
            Urgency::Low => 0,
            Urgency::Normal => 1,
            Urgency::Critical => 2,
        };

        let mut hints = BTreeMap::from([(String::from("urgency"), Argument::Byte(urgency))]);
        let mut text_hint = |name: &str, value: &str| {
            if !value.is_empty() {
                hints.insert(String::from(name), Argument::Text(String::from(value)));
            }
        };
        text_hint("category", &self.category);
        text_hint("desktop-entry", &self.desktop_entry);
        text_hint("image-path", &self.image);
        if self.transient {
            hints.insert(String::from("transient"), Argument::Bool(true));
        }
        if self.resident {
            hints.insert(String::from("resident"), Argument::Bool(true));
        }

        let mut actions = Vec::new();
        if self.on_click.is_some() {
            actions.push(String::from(DEFAULT_ACTION));
            actions.push(String::new());
        }
        for (key, label, _) in &self.actions {
            actions.push(key.clone());
            actions.push(label.clone());
        }

        let timeout = match self.timeout {
            Timeout::Default => -1,
            Timeout::Never => 0,
            Timeout::After(after) => i32::try_from(after.as_millis()).unwrap_or(i32::MAX),
        };

        let reply = Bus::session().call(
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            &[
                Argument::Text(Self::app_name()),
                Argument::Unsigned(self.replaces),
                Argument::Text(self.icon.clone()),
                Argument::Text(self.summary.clone()),
                Argument::Text(self.body.clone()),
                Argument::TextList(actions),
                Argument::Map(hints),
                Argument::Int(timeout),
            ],
        );

        reply.number() as u32
    }
}

// keeps `callbacks` for `id`; a replaced notification's old buttons go with it
fn remember(id: u32, callbacks: HashMap<String, Callback>) {
    if id == 0 {
        return;
    }
    let mut all = CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner);
    let all = all.get_or_insert_with(HashMap::new);
    if callbacks.is_empty() {
        all.remove(&id);
    } else {
        all.insert(id, callbacks);
    }
}

/// Runs the button `key` of notification `id` if this program sent it with a callback, on the
/// worker thread so the caller's locks are let go first. True when there was one.
pub(crate) fn run_callback(id: u32, key: &str) -> bool {
    let run = CALLBACKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|all| all.get(&id))
        .and_then(|callbacks| callbacks.get(key))
        .cloned();

    match run {
        Some(run) => {
            worker::run(move || run());
            true
        }
        None => false,
    }
}

/// The notification `id` is gone, and its buttons with it.
pub(crate) fn forget(id: u32) {
    if let Some(all) = CALLBACKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        all.remove(&id);
    }
}

// another server shows them: listens for its ActionInvoked and NotificationClosed, once
fn hear_actions() {
    static STARTED: Once = Once::new();

    STARTED.call_once(|| {
        let _ = thread::Builder::new()
            .name(String::from("amane-notify-actions"))
            .spawn(|| {
                let bus = Bus::session();
                let invoked = bus.signals("org.freedesktop.Notifications", "ActionInvoked");
                let _closed = thread::Builder::new()
                    .name(String::from("amane-notify-closed"))
                    .spawn(|| {
                        let closed = Bus::session()
                            .signals("org.freedesktop.Notifications", "NotificationClosed");
                        for signal in closed {
                            if let Some(id) = signal.arguments().first() {
                                forget(id.number() as u32);
                            }
                        }
                    });
                for signal in invoked {
                    if let [id, key] = signal.arguments() {
                        run_callback(id.number() as u32, key.text());
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn a_remembered_button_runs_once_pressed_and_is_forgotten_on_close() {
        let pressed = Arc::new(AtomicBool::new(false));
        let seen = pressed.clone();
        let run: Callback = Arc::new(move || seen.store(true, Ordering::SeqCst));

        remember(9001, HashMap::from([(String::from("open"), run)]));

        assert!(!run_callback(9001, "other"));
        assert!(run_callback(9001, "open"));

        // the worker runs it in order: wait for a job behind it
        let (done, wait) = std::sync::mpsc::channel();
        worker::run(move || {
            let _ = done.send(());
        });
        let _ = wait.recv();
        assert!(pressed.load(Ordering::SeqCst));

        forget(9001);
        assert!(!run_callback(9001, "open"));
    }

    #[test]
    fn a_timeout_of_zero_means_never() {
        assert_eq!(
            Notify::new("x").timeout(Duration::ZERO).timeout,
            Timeout::Never
        );
        assert_eq!(
            Notify::new("x").timeout(Duration::from_secs(3)).timeout,
            Timeout::After(Duration::from_secs(3))
        );
    }
}
