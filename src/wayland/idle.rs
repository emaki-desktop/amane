#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::time::Duration;

use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
};
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::Idle;

use super::WaylandState;

impl WaylandState {
    // runs on every wake, since that is when new levels from Idle::set_levels show up
    pub fn update_idle_if_asked(&mut self) {
        if !Idle::take_request() {
            return;
        }

        self.make_idle_notifications();
    }

    // one timer per seat and level, all made again so each starts from zero
    pub fn make_idle_notifications(&mut self) {
        // a compositor without ext-idle-notify never says the user is idle, which is not an error
        let Some(notifier) = &self.idle_notifier else {
            return;
        };

        if !Idle::wanted() {
            return;
        }

        for notification in self.idle_notifications.drain(..) {
            notification.destroy();
        }

        let levels = Idle::current_levels();

        for seat in self.seat.seats() {
            for &level in &levels {
                let timeout = u32::try_from(level.as_millis()).unwrap_or(u32::MAX);

                let notification = notifier.get_idle_notification(timeout, &seat, &self.qh, level);

                self.idle_notifications.push(notification);
            }
        }

        Idle::reset_crossed();
    }
}

/// The user data of the timer that ends screen_off at the first input.
pub struct ScreenOffWatch;

impl WaylandState {
    /*
     * the screen went off or on again (screen_off::set_screen_off): off, a one-second idle
     * timer per seat says when input comes; on, every window draws afresh. Either way the
     * watched services follow at once
     */
    pub fn update_screen_off(&mut self) {
        if !crate::screen_off::take_change() {
            return;
        }

        for watch in self.screen_off_watch.drain(..) {
            watch.destroy();
        }

        if crate::screen_off::screen_off() {
            if let Some(notifier) = &self.idle_notifier {
                for seat in self.seat.seats() {
                    let watch =
                        notifier.get_idle_notification(1000, &seat, &self.qh, ScreenOffWatch);

                    self.screen_off_watch.push(watch);
                }
            }
        } else {
            self.request_frames();
        }

        self.publish_watched();
    }
}

impl Dispatch<ExtIdleNotificationV1, ScreenOffWatch> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &ScreenOffWatch,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // idled comes a second after it's made (nothing moved); resumed is the first input
        if let ext_idle_notification_v1::Event::Resumed = event {
            crate::screen_off::set_screen_off(false);
        }
    }
}

impl Dispatch<ExtIdleNotificationV1, Duration> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        level: &Duration,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => Idle::idled(*level),
            ext_idle_notification_v1::Event::Resumed => Idle::resumed(*level),

            _ => {}
        }
    }
}

// the notifier never sends events, but wayland-client still needs somewhere to send them
impl Dispatch<ExtIdleNotifierV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
