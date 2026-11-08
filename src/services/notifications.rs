mod image_hint;
mod kept;
mod notification;
mod server;

pub use kept::Kept;
pub use notification::{Action, Notification, Timeout, Urgency};

use crate::Service;

pub struct Notifications {
    list: Vec<Notification>,

    // the spec keeps 0 for "no notification", so ids start at 1
    next_id: u32,

    running: bool,
}

// the numbers the spec gives each way a notification can close
#[derive(Clone, Copy)]
pub enum Reason {
    // the shell let it go after showing it, like a transient one
    Expired = 1,

    Dismissed = 2,

    // the sender asked with CloseNotification
    Closed = 3,
}

/*
 * amane is the notification server here: programs send
 * their notifications to it, instead of amane asking anyone
 */
impl Service for Notifications {
    fn new() -> Self {
        Self {
            list: Vec::new(),
            next_id: 1,
            running: false,
        }
    }

    fn listen() {
        server::run();
    }
}

impl Notifications {
    // oldest first
    pub fn list(&self) -> &[Notification] {
        &self.list
    }

    // false when another daemon like mako or dunst already owns the name
    pub fn running(&self) -> bool {
        self.running
    }

    pub fn dismiss(id: u32) {
        Self::write().close(id, Reason::Dismissed);
    }

    // the shell is done showing it, without the user dismissing it
    pub fn expire(id: u32) {
        Self::write().close(id, Reason::Expired);
    }

    pub fn clear() {
        let mut notifications = Self::write();

        let mut ids = Vec::new();

        for notification in &notifications.list {
            ids.push(notification.id);
        }

        for id in ids {
            notifications.close(id, Reason::Dismissed);
        }
    }

    /*
     * puts back what a shell saved before it last stopped, oldest first and
     * before anything that arrived since; each gets a new id, and has no
     * actions, since its sender can't hear them now
     */
    pub fn restore(kept: Vec<Kept>) {
        let mut restored = Vec::with_capacity(kept.len());

        for kept in kept {
            let icon_path = server::find_icon(&kept.icon, &kept.desktop_entry);

            restored.push(kept.into_notification(icon_path));
        }

        Self::write().put_back(restored);
    }

    // a button was pressed, pass the key from notification.actions()
    pub fn invoke(id: u32, key: &str) {
        Self::write().run_action(id, key);
    }

    // the notification itself was clicked: its default action, or a dismiss when it has none
    pub fn click(id: u32) {
        let mut notifications = Self::write();

        let has_default = notifications
            .list
            .iter()
            .any(|notification| notification.id == id && notification.has_default);

        if has_default {
            notifications.run_action(id, server::DEFAULT_ACTION);
        } else {
            notifications.close(id, Reason::Dismissed);
        }
    }

    // the spec closes a notification once an action ran, unless it asked to stay
    fn run_action(&mut self, id: u32, key: &str) {
        let Some(notification) = self.list.iter().find(|notification| notification.id == id) else {
            return;
        };

        let resident = notification.resident;

        server::announce_action(id, key);

        if !resident {
            self.close(id, Reason::Dismissed);
        }
    }

    // a sender may replace its own earlier notification, which keeps its id and place
    fn add(&mut self, mut notification: Notification, replaces_id: u32) -> u32 {
        let replaced = self.list.iter_mut().find(|old| old.id == replaces_id);

        if let Some(old) = replaced {
            notification.id = replaces_id;

            image_hint::remove(&old.image);

            *old = notification;

            return replaces_id;
        }

        let id = self.next_id;

        self.next_id += 1;

        notification.id = id;

        self.list.push(notification);

        id
    }

    fn put_back(&mut self, mut restored: Vec<Notification>) {
        restored.sort_by_key(|notification| notification.received);

        for notification in &mut restored {
            notification.id = self.next_id;

            self.next_id += 1;
        }

        restored.append(&mut self.list);

        self.list = restored;
    }

    fn close(&mut self, id: u32, reason: Reason) {
        let Some(index) = self
            .list
            .iter()
            .position(|notification| notification.id == id)
        else {
            return;
        };

        let closed = self.list.remove(index);

        image_hint::remove(&closed.image);

        // nobody in this session sent a restored one, so nobody is told
        if !closed.restored {
            server::announce_closed(id, reason);
        }
    }
}
