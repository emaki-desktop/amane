use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::services::apps;
use crate::{Argument, Bus, Method, Notification, Notifications, Service, Timeout, Urgency, Value};

use super::{Action, Reason, image_hint};

const NAME: &str = "org.freedesktop.Notifications";

const PATH: &str = "/org/freedesktop/Notifications";

const INTERFACE: &str = "org.freedesktop.Notifications";

// the key of what clicking the notification itself does
pub const DEFAULT_ACTION: &str = "default";

/*
 * any program on the session bus may notify, so one can't flood the screen or the history: so
 * many a sender in a while (more are answered, but not shown), and the text and buttons cut to
 * what a card can show
 */
const PER_SENDER: usize = 30;
const PER_WHILE: Duration = Duration::from_secs(10);
const MAX_SUMMARY: usize = 1024;
const MAX_BODY: usize = 16 * 1024;
const MAX_ACTIONS: usize = 8;

// when each sender last notified, newest last
static RECENT: Mutex<Option<HashMap<String, VecDeque<Instant>>>> = Mutex::new(None);

// whether `sender` may show another now; counts it if so
fn allowed(sender: &str) -> bool {
    let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    let recent = recent.get_or_insert_with(HashMap::new);
    let now = Instant::now();

    // senders gone quiet are let go, so the map stays small
    recent.retain(|_, times| {
        times
            .back()
            .is_some_and(|at| now.duration_since(*at) < PER_WHILE)
    });

    let times = recent.entry(sender.to_string()).or_default();
    while times
        .front()
        .is_some_and(|at| now.duration_since(*at) >= PER_WHILE)
    {
        times.pop_front();
    }

    if times.len() >= PER_SENDER {
        return false;
    }

    times.push_back(now);
    true
}

// `text` cut to at most `limit` bytes, at a letter's edge
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return String::from(text);
    }

    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }

    format!("{}…", &text[..end])
}

pub fn run() {
    let bus = Bus::session();

    // watched before owning the name, so no early call is missed
    let methods = bus.methods(PATH, INTERFACE);

    if !bus.own(NAME) {
        return;
    }

    Notifications::write().running = true;

    for method in methods {
        answer(&method);
    }
}

fn answer(method: &Method) {
    match method.name() {
        "Notify" => notify(method),
        "CloseNotification" => close(method),
        "GetCapabilities" => send_capabilities(method),
        "GetServerInformation" => send_information(method),

        // every call waits for an answer, even one amane does not know
        _ => method.reply(&[]),
    }
}

fn notify(method: &Method) {
    let [
        app_name,
        replaces_id,
        icon,
        summary,
        body,
        actions,
        hints,
        timeout,
    ] = method.arguments()
    else {
        method.reply(&[]);

        return;
    };

    if !method.sender().is_some_and(|sender| allowed(&sender)) {
        // answered, so the sender carries on, but not shown
        method.reply(&[Argument::from(0u32)]);

        return;
    }

    let (mut actions, has_default) = read_actions(actions);
    actions.truncate(MAX_ACTIONS);

    let desktop_entry = hints.get("desktop-entry").text();

    let notification = Notification {
        // add() picks the real id
        id: 0,

        app_name: String::from(app_name.text()),
        summary: cut(summary.text(), MAX_SUMMARY),
        body: cut(body.text(), MAX_BODY),
        icon: String::from(icon.text()),
        image: image_hint::read(hints),
        icon_path: find_icon(icon.text(), desktop_entry),

        urgency: Urgency::from_level(hints.get("urgency").number()),

        actions,
        has_default,

        resident: hints.get("resident").bool(),
        transient: hints.get("transient").bool(),

        category: String::from(hints.get("category").text()),
        desktop_entry: String::from(desktop_entry),

        timeout: Timeout::from_millis(timeout.number()),

        received: SystemTime::now(),

        restored: false,
    };

    let replaces_id = replaces_id.number() as u32;

    let id = Notifications::write().add(notification, replaces_id);

    method.reply(&[Argument::from(id)]);
}

/*
 * actions come as one flat list: key, label, key, label;
 * the "default" one is what clicking the notification itself does
 */
fn read_actions(list: &Value) -> (Vec<Action>, bool) {
    let mut actions = Vec::new();
    let mut has_default = false;

    for pair in list.list().as_chunks::<2>().0 {
        let key = pair[0].text();
        let label = pair[1].text();

        if key == DEFAULT_ACTION {
            has_default = true;

            continue;
        }

        actions.push(Action {
            key: String::from(key),
            label: String::from(label),
        });
    }

    (actions, has_default)
}

/*
 * the icon a sender names, or its program's own icon when it names none;
 * found here on the bus thread, since the first lookup walks the themes
 */
pub(super) fn find_icon(icon: &str, desktop_entry: &str) -> Option<PathBuf> {
    let icon = icon.strip_prefix("file://").unwrap_or(icon);

    let name = if icon.is_empty() { desktop_entry } else { icon };

    if name.is_empty() {
        return None;
    }

    apps::find_icon(name)
}

fn close(method: &Method) {
    let id = method.arguments().first().map(|id| id.number() as u32);

    if let Some(id) = id {
        Notifications::write().close(id, Reason::Closed);
    }

    method.reply(&[]);
}

fn send_capabilities(method: &Method) {
    let capabilities = vec![
        String::from("actions"),
        String::from("body"),
        String::from("icon-static"),
        // a shell keeps notifications in a history until they are dismissed
        String::from("persistence"),
    ];

    method.reply(&[Argument::from(capabilities)]);
}

fn send_information(method: &Method) {
    let information = [
        Argument::from("amane"),
        Argument::from("amane"),
        Argument::from(env!("CARGO_PKG_VERSION")),
        Argument::from("1.2"),
    ];

    method.reply(&information);
}

// the sender hears which button was pressed, and does the rest itself
pub fn announce_action(id: u32, key: &str) {
    let arguments = [Argument::from(id), Argument::from(key)];

    Bus::session().emit(PATH, INTERFACE, "ActionInvoked", &arguments);
}

pub fn announce_closed(id: u32, reason: Reason) {
    let arguments = [Argument::from(id), Argument::from(reason as u32)];

    Bus::session().emit(PATH, INTERFACE, "NotificationClosed", &arguments);
}

#[cfg(test)]
mod tests {

    use super::*;

    // paths never reach the theme walk, so these stay quick
    #[test]
    fn finds_an_icon_given_as_a_path() {
        let path = Some(PathBuf::from("/tmp/avatar.png"));

        assert_eq!(find_icon("/tmp/avatar.png", ""), path);
        assert_eq!(find_icon("file:///tmp/avatar.png", "firefox"), path);
        assert_eq!(find_icon("", ""), None);
    }

    #[test]
    fn leaves_the_default_action_out_of_the_buttons() {
        let text = |text: &str| Value::Text(String::from(text));

        let list = Value::List(vec![
            text("default"),
            text("Open"),
            text("yes"),
            text("Yes"),
        ]);

        let (actions, has_default) = read_actions(&list);

        assert!(has_default);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].key(), "yes");
        assert_eq!(actions[0].label(), "Yes");
    }
}

#[cfg(test)]
mod limits {
    use super::*;

    #[test]
    fn a_sender_may_only_notify_so_often() {
        let sender = ":test.flood";
        for _ in 0..PER_SENDER {
            assert!(allowed(sender));
        }
        assert!(!allowed(sender));
        // another sender isn't held back by it
        assert!(allowed(":test.other"));
    }

    #[test]
    fn long_text_is_cut_at_a_letter() {
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("ééé", 3), "é…");
    }
}
