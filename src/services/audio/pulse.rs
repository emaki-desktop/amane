use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::channelmap::Map;
use libpulse_binding::context::subscribe::InterestMaskSet;
use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::{self, Operation};
use libpulse_binding::proplist::Proplist;
use libpulse_binding::volume::{ChannelVolumes, Volume};

#[derive(Clone, Copy)]
pub enum Device {
    // speakers or headphones
    Output,

    // the microphone
    Input,
}

impl Device {
    // pulse resolves these to whatever device is the default right now
    pub(super) fn name(self) -> &'static str {
        match self {
            Device::Output => "@DEFAULT_SINK@",
            Device::Input => "@DEFAULT_SOURCE@",
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Level {
    // 0 to 100
    pub volume: u8,

    pub muted: bool,
}

// one output or input the sound server offers, as it describes itself
#[derive(Default, Clone)]
pub struct Endpoint {
    // pulse's own name, used to make it the default
    pub name: String,

    // the name people see, like "Built-in Audio Analog Stereo"
    pub description: String,

    // the jack or connector in use, like "[Out] Headphones"
    pub port: String,

    // what the device says it is, like "headset", empty when it doesn't say
    pub form_factor: String,

    // a freedesktop icon name like "audio-headphones-bluetooth"
    pub icon_name: String,
}

impl Endpoint {
    fn new(
        name: Option<&str>,
        description: Option<&str>,
        port: Option<&str>,
        properties: &Proplist,
    ) -> Self {
        let property = |key| properties.get_str(key).unwrap_or_default();

        Self {
            name: String::from(name.unwrap_or_default()),
            description: String::from(description.unwrap_or_default()),
            port: String::from(port.unwrap_or_default()),
            form_factor: property("device.form_factor"),
            icon_name: property("device.icon_name"),
        }
    }
}

// what pulse reports for a default device, before converting to percent
#[derive(Clone, Copy)]
pub(super) struct Levels {
    pub volumes: ChannelVolumes,
    pub map: Map,
    pub muted: bool,
}

// how long to wait before trying the sound server again while it is down
const RETRY: Duration = Duration::from_secs(5);

thread_local! {
    // pulse objects can't move between threads, so each thread opens its own
    static CONNECTION: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

/*
 * runs work on this thread's connection, opened again when the sound server
 * went away, like when it restarts; with no sound server work gets the default
 */
pub(super) fn with_connection<T: Default>(work: impl FnOnce(&mut Connection) -> T) -> T {
    CONNECTION.with_borrow_mut(|slot| {
        let alive = slot.as_ref().is_some_and(Connection::ready);

        if !alive {
            *slot = Connection::open();
        }

        let Some(connection) = slot else {
            return T::default();
        };

        work(connection)
    })
}

pub fn read(device: Device) -> Level {
    with_connection(|connection| {
        let Some(levels) = connection.levels(device) else {
            return Level::default();
        };

        Level {
            // the loudest channel, as pulse's own mixers show it once a balance leans
            volume: percent(levels.volumes.max()),
            muted: levels.muted,
        }
    })
}

pub fn set_volume(device: Device, volume: u8) {
    with_connection(|connection| {
        let Some(levels) = connection.levels(device) else {
            return;
        };

        // the channels keep their ratios, so a balance survives the volume keys
        let mut volumes = levels.volumes;

        volumes.scale(level(volume));

        let mut introspect = connection.context.introspect();

        let operation = match device {
            Device::Output => introspect.set_sink_volume_by_name(device.name(), &volumes, None),
            Device::Input => introspect.set_source_volume_by_name(device.name(), &volumes, None),
        };

        connection.wait(operation);
    });
}

pub fn set_muted(device: Device, muted: bool) {
    with_connection(|connection| {
        let mut introspect = connection.context.introspect();

        let operation = match device {
            Device::Output => introspect.set_sink_mute_by_name(device.name(), muted, None),
            Device::Input => introspect.set_source_mute_by_name(device.name(), muted, None),
        };

        connection.wait(operation);
    });
}

// every output or every input, and the name of the one that is the default now
pub fn devices(device: Device) -> (Vec<Endpoint>, String) {
    with_connection(|connection| {
        let endpoints = connection.endpoints(device);

        let default = connection.default_name(device);

        (endpoints, default)
    })
}

// sounds that follow the default move along, which pipewire does for every stream
pub fn set_default(device: Device, name: &str) {
    with_connection(|connection| {
        let operation = match device {
            Device::Output => connection.context.set_default_sink(name, |_| {}),
            Device::Input => connection.context.set_default_source(name, |_| {}),
        };

        connection.wait(operation);
    });
}

/*
 * calls changed after every change to an output, an input, an app's
 * stream or the default choice, and never returns; it keeps its own
 * connection, so changed can still use read()
 */
pub fn watch(mut changed: impl FnMut()) {
    loop {
        if let Some(connection) = Connection::open() {
            follow(connection, &mut changed);
        }

        // the sound server is down or restarting, so it is tried again after a pause
        thread::sleep(RETRY);
    }
}

// returns once the connection breaks
fn follow(mut connection: Connection, changed: &mut impl FnMut()) {
    let dirty = Rc::new(Cell::new(false));
    let marked = Rc::clone(&dirty);

    /*
     * pulse can't answer questions from inside its own callback,
     * so the callback only marks the change and the loop reacts
     */
    connection
        .context
        .set_subscribe_callback(Some(Box::new(move |_, _, _| marked.set(true))));

    let interests = InterestMaskSet::SINK
        | InterestMaskSet::SOURCE
        | InterestMaskSet::SINK_INPUT
        | InterestMaskSet::SERVER;

    connection.context.subscribe(interests, |_| {});

    // a new connection may come after a restart that changed the levels
    changed();

    while connection.iterate() {
        if dirty.replace(false) {
            changed();
        }
    }
}

// fields drop in order, and pulse wants the context gone before its mainloop
pub(super) struct Connection {
    pub context: Context,
    mainloop: Mainloop,
}

impl Connection {
    // none when the sound server can't be reached
    pub(super) fn open() -> Option<Self> {
        let mainloop = Mainloop::new()?;

        let mut context = Context::new(&mainloop, "amane")?;

        context.connect(None, FlagSet::NOFLAGS, None).ok()?;

        let mut connection = Self { mainloop, context };

        loop {
            if !connection.iterate() {
                return None;
            }

            match connection.context.get_state() {
                State::Ready => return Some(connection),
                State::Failed | State::Terminated => return None,
                _ => {}
            }
        }
    }

    // none when there is no such device at all
    pub(super) fn levels(&mut self, device: Device) -> Option<Levels> {
        let found = Rc::new(Cell::new(None));
        let slot = Rc::clone(&found);

        let introspect = self.context.introspect();

        // outputs and inputs come back as different types, so each gets its own callback
        match device {
            Device::Output => {
                let operation = introspect.get_sink_info_by_name(device.name(), move |result| {
                    let ListResult::Item(sink) = result else {
                        return;
                    };

                    slot.set(Some(Levels {
                        volumes: sink.volume,
                        map: sink.channel_map,
                        muted: sink.mute,
                    }));
                });

                self.wait(operation);
            }

            Device::Input => {
                let operation = introspect.get_source_info_by_name(device.name(), move |result| {
                    let ListResult::Item(source) = result else {
                        return;
                    };

                    slot.set(Some(Levels {
                        volumes: source.volume,
                        map: source.channel_map,
                        muted: source.mute,
                    }));
                });

                self.wait(operation);
            }
        }

        found.take()
    }

    fn endpoints(&mut self, device: Device) -> Vec<Endpoint> {
        let found = Rc::new(RefCell::new(Vec::new()));
        let list = Rc::clone(&found);

        let introspect = self.context.introspect();

        match device {
            Device::Output => {
                let operation = introspect.get_sink_info_list(move |result| {
                    let ListResult::Item(sink) = result else {
                        return;
                    };

                    let port = sink
                        .active_port
                        .as_ref()
                        .and_then(|port| port.name.as_deref());

                    list.borrow_mut().push(Endpoint::new(
                        sink.name.as_deref(),
                        sink.description.as_deref(),
                        port,
                        &sink.proplist,
                    ));
                });

                self.wait(operation);
            }

            Device::Input => {
                let operation = introspect.get_source_info_list(move |result| {
                    let ListResult::Item(source) = result else {
                        return;
                    };

                    // every output also shows up as an input that records it, which nobody picks as a microphone
                    if source.monitor_of_sink.is_some() {
                        return;
                    }

                    let port = source
                        .active_port
                        .as_ref()
                        .and_then(|port| port.name.as_deref());

                    list.borrow_mut().push(Endpoint::new(
                        source.name.as_deref(),
                        source.description.as_deref(),
                        port,
                        &source.proplist,
                    ));
                });

                self.wait(operation);
            }
        }

        found.take()
    }

    // empty when nothing is the default, like with no sound card at all
    fn default_name(&mut self, device: Device) -> String {
        let found = Rc::new(RefCell::new(String::new()));
        let slot = Rc::clone(&found);

        let operation = self.context.introspect().get_server_info(move |info| {
            let name = match device {
                Device::Output => &info.default_sink_name,
                Device::Input => &info.default_source_name,
            };

            *slot.borrow_mut() = String::from(name.as_deref().unwrap_or_default());
        });

        self.wait(operation);

        found.take()
    }

    // blocks until pulse has answered
    pub(super) fn wait<F: ?Sized>(&mut self, operation: Operation<F>) {
        while operation.get_state() == operation::State::Running {
            if !self.iterate() {
                return;
            }
        }
    }

    fn ready(&self) -> bool {
        self.context.get_state() == State::Ready
    }

    // false once the connection to the sound server is broken
    pub(super) fn iterate(&mut self) -> bool {
        let result = self.mainloop.iterate(true);

        matches!(result, IterateResult::Success(_))
    }
}

// pulse counts volume with 100% at Volume::NORMAL
pub(super) fn percent(volume: Volume) -> u8 {
    let level = u64::from(volume.0);
    let normal = u64::from(Volume::NORMAL.0);

    let rounded = (level * 100 + normal / 2) / normal;

    rounded.min(100) as u8
}

pub(super) fn level(percent: u8) -> Volume {
    let normal = Volume::NORMAL.0;

    Volume(u32::from(percent) * normal / 100)
}
