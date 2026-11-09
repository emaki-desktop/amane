#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! What each app is playing: the sound server's playback streams (pulse's
//! sink inputs), each with its own level and mute.

use std::cell::RefCell;
use std::rc::Rc;

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::proplist::Proplist;
use libpulse_binding::volume::ChannelVolumes;

use super::pulse::{Connection, level, percent, with_connection};

/// One app's sound, playing or paused, with a level of its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioStream {
    pub(super) index: u32,
    pub(super) app: String,
    pub(super) icon: String,
    pub(super) binary: String,
    pub(super) media: String,
    pub(super) volume: u8,
    pub(super) muted: bool,
    pub(super) corked: bool,
}

impl AudioStream {
    /// The sound server's number for it, which the stream controls on
    /// [`Audio`](super::Audio) take. It stays the same while the stream lives.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The app's name as it gives it, like "Firefox"; empty when it doesn't.
    pub fn app(&self) -> &str {
        &self.app
    }

    /// A freedesktop icon name the app gave, like "firefox"; often empty.
    pub fn icon(&self) -> &str {
        &self.icon
    }

    /// The program's file name, like "mpv", to find an icon by when the app
    /// gave none.
    pub fn binary(&self) -> &str {
        &self.binary
    }

    /// What is playing, like a song or a tab's title; empty when the app
    /// doesn't say.
    pub fn media(&self) -> &str {
        &self.media
    }

    /// 0 to 100.
    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Whether this stream alone is muted.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Whether it is paused, which apps like browsers do instead of ending it.
    pub fn paused(&self) -> bool {
        self.corked
    }
}

/// What pulse says about a stream, before it is shown.
pub(super) struct Reported<'a> {
    pub index: u32,
    pub volumes: ChannelVolumes,
    pub muted: bool,
    pub corked: bool,
    pub has_volume: bool,
    pub property: &'a dyn Fn(&str) -> String,
}

/*
 * event sounds (a notification's, a click) come and go too fast to set,
 * and a stream without a level can't be set at all
 */
pub(super) fn shown(reported: &Reported) -> Option<AudioStream> {
    let property = reported.property;
    let role = property("media.role");
    if !reported.has_volume || role == "event" || role == "Notification" {
        return None;
    }
    Some(AudioStream {
        index: reported.index,
        app: property("application.name"),
        icon: property("application.icon_name"),
        binary: property("application.process.binary"),
        media: property("media.name"),
        volume: percent(reported.volumes.max()),
        muted: reported.muted,
        corked: reported.corked,
    })
}

/// Every stream shown, in the sound server's order.
pub(super) fn read() -> Vec<AudioStream> {
    with_connection(Connection::streams)
}

/// Sets a stream to `volume`, keeping its balance.
pub(super) fn set_volume(index: u32, volume: u8) {
    with_connection(|connection| {
        let Some(mut volumes) = connection.stream_volumes(index) else {
            return;
        };
        volumes.scale(level(volume));
        let operation = connection
            .context
            .introspect()
            .set_sink_input_volume(index, &volumes, None);
        connection.wait(operation);
    });
}

pub(super) fn set_muted(index: u32, muted: bool) {
    with_connection(|connection| {
        let operation = connection
            .context
            .introspect()
            .set_sink_input_mute(index, muted, None);
        connection.wait(operation);
    });
}

pub(super) fn muted(index: u32) -> Option<bool> {
    read()
        .into_iter()
        .find(|stream| stream.index == index)
        .map(|stream| stream.muted)
}

impl Connection {
    fn streams(&mut self) -> Vec<AudioStream> {
        let found = Rc::new(RefCell::new(Vec::new()));
        let list = Rc::clone(&found);
        let operation = self
            .context
            .introspect()
            .get_sink_input_info_list(move |result| {
                let ListResult::Item(input) = result else {
                    return;
                };
                let property = |key: &str| text(&input.proplist, key);
                let reported = Reported {
                    index: input.index,
                    volumes: input.volume,
                    muted: input.mute,
                    corked: input.corked,
                    has_volume: input.has_volume && input.volume_writable,
                    property: &property,
                };
                if let Some(stream) = shown(&reported) {
                    list.borrow_mut().push(stream);
                }
            });
        self.wait(operation);
        found.take()
    }

    fn stream_volumes(&mut self, index: u32) -> Option<ChannelVolumes> {
        let found = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&found);
        let operation = self
            .context
            .introspect()
            .get_sink_input_info(index, move |result| {
                if let ListResult::Item(input) = result {
                    *slot.borrow_mut() = Some(input.volume);
                }
            });
        self.wait(operation);
        found.take()
    }
}

fn text(properties: &Proplist, key: &str) -> String {
    properties.get_str(key).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use libpulse_binding::volume::Volume;

    use super::*;

    fn reported<'a>(property: &'a dyn Fn(&str) -> String) -> Reported<'a> {
        let mut volumes = ChannelVolumes::default();
        volumes.set(2, Volume(Volume::NORMAL.0 / 2));
        Reported {
            index: 7,
            volumes,
            muted: false,
            corked: false,
            has_volume: true,
            property,
        }
    }

    fn properties(pairs: &[(&str, &str)]) -> impl Fn(&str) -> String {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (String::from(*k), String::from(*v)))
            .collect();
        move |key| map.get(key).cloned().unwrap_or_default()
    }

    #[test]
    fn an_apps_stream_shows_with_its_names() {
        let property = properties(&[
            ("application.name", "Firefox"),
            ("application.process.binary", "firefox"),
            ("media.name", "A song"),
        ]);
        let stream = shown(&reported(&property)).expect("shown");
        assert_eq!(stream.index(), 7);
        assert_eq!(stream.app(), "Firefox");
        assert_eq!(stream.binary(), "firefox");
        assert_eq!(stream.media(), "A song");
        assert_eq!(stream.volume(), 50);
    }

    #[test]
    fn event_sounds_and_fixed_streams_dont() {
        let event = properties(&[("media.role", "event")]);
        assert!(shown(&reported(&event)).is_none());
        let notification = properties(&[("media.role", "Notification")]);
        assert!(shown(&reported(&notification)).is_none());
        let plain = properties(&[]);
        let mut fixed = reported(&plain);
        fixed.has_volume = false;
        assert!(shown(&fixed).is_none());
    }
}
