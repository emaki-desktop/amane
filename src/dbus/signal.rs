use zbus::MatchRule;
use zbus::blocking::MessageIterator;
use zbus::message::Type;

use crate::Value;

use super::{Bus, convert};

#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    // the sender's unique name like :1.42, not a well-known name like org.mpris.MediaPlayer2.mpv
    pub(crate) sender: String,

    pub(crate) path: String,

    // like org.freedesktop.DBus.Properties and PropertiesChanged
    pub(crate) interface: String,

    pub(crate) name: String,

    pub(crate) arguments: Vec<Value>,
}

// empty when the bus could not be reached
pub struct Signals {
    messages: Option<MessageIterator>,
}

impl Signal {
    pub fn sender(&self) -> &str {
        &self.sender
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments(&self) -> &[Value] {
        &self.arguments
    }
}

impl Bus {
    // waits for each matching signal, so call it from a service thread, not from view()
    pub fn signals(&self, interface: &str, name: &str) -> Signals {
        self.watch_signals(rule(interface, name).build())
    }

    /*
     * the signals `interface`.`name` sent from objects at or under `path`, like upower's
     * PropertiesChanged under /org/freedesktop/UPower: the bus daemon filters, so another
     * program's PropertiesChanged never reaches this process at all
     */
    pub fn signals_at(&self, path: &str, interface: &str, name: &str) -> Signals {
        let rule = rule(interface, name)
            .path_namespace(path)
            .expect("failed to watch signals: bad object path")
            .build();

        self.watch_signals(rule)
    }

    /*
     * every signal one service sends, by its well-known name like org.bluez,
     * in the order it sent them, so another program's PropertiesChanged never
     * wakes the loop and an object's changes never overtake its arrival
     */
    pub fn signals_from(&self, sender: &str) -> Signals {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(sender)
            .expect("failed to watch signals: bad sender name")
            .build();

        self.watch_signals(rule)
    }

    /*
     * each time `name` changes hands: its program started, quit or was
     * restarted; the arguments are the name, the old owner and the new one,
     * an empty owner meaning nobody
     */
    pub fn owner_changes(&self, name: &str) -> Signals {
        let rule = rule("org.freedesktop.DBus", "NameOwnerChanged")
            .sender("org.freedesktop.DBus")
            .expect("failed to watch signals: bad sender name")
            .arg(0, name)
            .expect("failed to watch signals: bad bus name")
            .build();

        self.watch_signals(rule)
    }

    /*
     * every signal of `interface` whoever sends it, like each tray item's
     * NewIcon, since a tray hears from programs it doesn't know beforehand
     */
    pub fn interface_signals(&self, interface: &str) -> Signals {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(interface)
            .expect("failed to watch signals: bad interface name")
            .build();

        self.watch_signals(rule)
    }

    pub(crate) fn watch_signals(&self, rule: MatchRule<'_>) -> Signals {
        let messages = self
            .connection
            .and_then(|connection| MessageIterator::for_match_rule(rule, connection, None).ok());

        Signals { messages }
    }
}

fn rule<'m>(interface: &'m str, name: &'m str) -> zbus::match_rule::Builder<'m> {
    MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(interface)
        .expect("failed to watch signals: bad interface name")
        .member(name)
        .expect("failed to watch signals: bad signal name")
}

impl Iterator for Signals {
    type Item = Signal;

    fn next(&mut self) -> Option<Signal> {
        loop {
            // a message that fails to read is skipped, the next one may be fine
            let Ok(message) = self.messages.as_mut()?.next()? else {
                continue;
            };

            let header = message.header();

            let sender = header.sender().map(|name| name.to_string());
            let path = header.path().map(|path| path.to_string());
            let interface = header.interface().map(|interface| interface.to_string());
            let name = header.member().map(|name| name.to_string());

            let signal = Signal {
                sender: sender.unwrap_or_default(),
                path: path.unwrap_or_default(),
                interface: interface.unwrap_or_default(),
                name: name.unwrap_or_default(),

                arguments: convert::arguments(&message),
            };

            return Some(signal);
        }
    }
}
