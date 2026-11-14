use crate::{Argument, Bus, BusError, Value};

pub const NAME: &str = "org.freedesktop.NetworkManager";

pub const PATH: &str = "/org/freedesktop/NetworkManager";

// networkmanager's own state number for "online, with internet"
const CONNECTED_GLOBAL: f64 = 70.0;

pub fn connected() -> bool {
    let state = Bus::system().property(NAME, PATH, NAME, "State");

    state.number() == CONNECTED_GLOBAL
}

// the connection that carries the default route, as a map of its properties
pub fn primary_connection() -> Value {
    let primary = Bus::system().property(NAME, PATH, NAME, "PrimaryConnection");

    properties(
        primary.text(),
        "org.freedesktop.NetworkManager.Connection.Active",
    )
}

// the wifi network a connection is joined to, as a map of its properties
pub fn access_point(path: &str) -> Value {
    properties(path, "org.freedesktop.NetworkManager.AccessPoint")
}

// the radio switch, which turns every wifi device off or on
pub fn wifi_enabled() -> bool {
    Bus::system()
        .property(NAME, PATH, NAME, "WirelessEnabled")
        .bool()
}

// a captive portal: networkmanager's connectivity check was answered by a sign-in page
pub fn behind_portal() -> bool {
    const PORTAL: f64 = 2.0;
    Bus::system()
        .property(NAME, PATH, NAME, "Connectivity")
        .number()
        == PORTAL
}

pub fn set_wifi(enabled: bool) {
    Bus::system().set_property(NAME, PATH, NAME, "WirelessEnabled", Argument::from(enabled));
}

// "/" lets networkmanager pick the access point itself
pub fn activate(profile: &str, device: &str) -> Result<(), BusError> {
    let arguments = [
        Argument::Path(String::from(profile)),
        Argument::Path(String::from(device)),
        Argument::Path(String::from("/")),
    ];

    Bus::system()
        .try_call(NAME, PATH, NAME, "ActivateConnection", &arguments)
        .map(|_| ())
}

// saves a new profile from these settings and joins it straight away
pub fn add_and_activate(settings: Argument, device: &str) -> Result<(), BusError> {
    let arguments = [
        settings,
        Argument::Path(String::from(device)),
        Argument::Path(String::from("/")),
    ];

    Bus::system()
        .try_call(NAME, PATH, NAME, "AddAndActivateConnection", &arguments)
        .map(|_| ())
}

pub fn properties(path: &str, interface: &str) -> Value {
    let arguments = [Argument::from(interface)];

    Bus::system().call(
        NAME,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &arguments,
    )
}
