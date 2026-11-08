use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Notification, Timeout, Urgency};

/*
 * what of a notification is worth keeping across a restart: its words,
 * pictures and sender, but not its actions or id, since the sender that
 * could answer them may be gone
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub app_name: String,

    pub summary: String,

    #[serde(default)]
    pub body: String,

    #[serde(default)]
    pub icon: String,

    // a file path; a shell drops it when the file is gone
    #[serde(default)]
    pub image: String,

    // 0 low, 1 normal, 2 critical, as the spec numbers them
    #[serde(default = "normal")]
    pub urgency: u8,

    #[serde(default)]
    pub category: String,

    #[serde(default)]
    pub desktop_entry: String,

    // unix seconds
    pub received: u64,
}

fn normal() -> u8 {
    1
}

impl Notification {
    // what a shell saves to give back to Notifications::restore
    pub fn kept(&self) -> Kept {
        let received = self
            .received
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let urgency = match self.urgency {
            Urgency::Low => 0,
            Urgency::Normal => 1,
            Urgency::Critical => 2,
        };

        Kept {
            app_name: self.app_name.clone(),
            summary: self.summary.clone(),
            body: self.body.clone(),
            icon: self.icon.clone(),
            image: self.image.clone(),
            urgency,
            category: self.category.clone(),
            desktop_entry: self.desktop_entry.clone(),
            received,
        }
    }

    // whether it came back from a saved history, with no sender to answer it
    pub fn restored(&self) -> bool {
        self.restored
    }
}

impl Kept {
    // back into a notification without actions; add() picks its id
    pub(crate) fn into_notification(self, icon_path: Option<std::path::PathBuf>) -> Notification {
        Notification {
            id: 0,
            app_name: self.app_name,
            summary: self.summary,
            body: self.body,
            icon: self.icon,
            image: self.image,
            icon_path,
            urgency: Urgency::from_level(f64::from(self.urgency)),
            actions: Vec::new(),
            has_default: false,
            resident: false,
            transient: false,
            category: self.category,
            desktop_entry: self.desktop_entry,
            timeout: Timeout::Default,
            received: UNIX_EPOCH + Duration::from_secs(self.received),
            restored: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Service;
    use crate::services::Notifications;

    fn kept(summary: &str, received: u64) -> Kept {
        Kept {
            app_name: String::from("Mail"),
            summary: String::from(summary),
            body: String::new(),
            icon: String::new(),
            image: String::new(),
            urgency: 2,
            category: String::new(),
            desktop_entry: String::new(),
            received,
        }
    }

    #[test]
    fn a_kept_notification_comes_back_as_it_was() {
        let first = kept("hello", 1_700_000_000);
        let back = first.clone().into_notification(None);

        assert!(back.restored());
        assert!(back.actions().is_empty());
        assert_eq!(back.urgency(), Urgency::Critical);
        assert_eq!(back.kept(), first);
    }

    #[test]
    fn restored_ones_come_first_oldest_first_with_new_ids() {
        let mut notifications = Notifications::new();
        let live = kept("live", 30).into_notification(None);
        let live_id = notifications.add(live, 0);

        notifications.put_back(vec![
            kept("newer", 20).into_notification(None),
            kept("older", 10).into_notification(None),
        ]);

        let summaries: Vec<&str> = notifications.list().iter().map(|n| n.summary()).collect();
        assert_eq!(summaries, ["older", "newer", "live"]);

        let mut ids: Vec<u32> = notifications.list().iter().map(|n| n.id()).collect();
        assert!(ids.contains(&live_id));
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);

        let next = notifications.add(kept("later", 40).into_notification(None), 0);
        assert!(!ids.contains(&next));
    }
}
