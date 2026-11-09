mod balance;
mod device;
mod level;
mod pulse;
mod streams;

use crate::Service;
use crate::services::worker;

pub use device::{AudioDevice, AudioKind};
pub use level::AudioLevel;
pub use streams::AudioStream;

use pulse::Device;

#[derive(Default, Clone, PartialEq)]
pub struct Audio {
    // 0 to 100, for the default output
    volume: u8,

    muted: bool,

    // 0 to 100, for the default microphone
    microphone_volume: u8,

    microphone_muted: bool,

    // in the sound server's order, which stays put as devices come and go
    outputs: Vec<AudioDevice>,

    inputs: Vec<AudioDevice>,

    // what each app is playing, see streams.rs
    streams: Vec<AudioStream>,

    // the default output's lean, none without a left and right
    balance: Option<f32>,
}

// event-driven, the sound server announces every volume change
impl Service for Audio {
    fn new() -> Self {
        let mut audio = Self::default();

        audio.update();

        audio
    }

    fn update(&mut self) -> bool {
        let before = self.clone();

        let output = pulse::read(Device::Output);
        let input = pulse::read(Device::Input);

        self.volume = output.volume;
        self.muted = output.muted;

        self.microphone_volume = input.volume;
        self.microphone_muted = input.muted;

        self.outputs = devices(Device::Output);
        self.inputs = devices(Device::Input);

        self.streams = streams::read();
        self.balance = balance::read();

        *self != before
    }

    // the sound server also announces changes to devices nothing shows
    fn listen() {
        pulse::watch(|| {
            let mut audio = Self::write();

            if !audio.update() {
                audio.quiet();
            }
        });
    }
}

impl Audio {
    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn microphone_volume(&self) -> u8 {
        self.microphone_volume
    }

    pub fn microphone_muted(&self) -> bool {
        self.microphone_muted
    }

    pub fn outputs(&self) -> &[AudioDevice] {
        &self.outputs
    }

    pub fn inputs(&self) -> &[AudioDevice] {
        &self.inputs
    }

    // what the default output is, like headphones, so a bar can show it
    pub fn output_kind(&self) -> AudioKind {
        default_kind(&self.outputs)
    }

    pub fn input_kind(&self) -> AudioKind {
        default_kind(&self.inputs)
    }

    /// What each app is playing, in the sound server's order; an app's
    /// event sounds (a notification's, a click) aren't here.
    pub fn streams(&self) -> &[AudioStream] {
        &self.streams
    }

    /// The default output's balance, −1 (all left) to 1 (all right), or none
    /// when it has no left and right.
    pub fn balance(&self) -> Option<f32> {
        self.balance
    }

    /// Leans the default output, −1 to 1, keeping its volume.
    pub fn set_balance(balance: f32) {
        worker::run(move || balance::set(balance));
    }

    /// Sets one app's stream, by its [`AudioStream::index`]; above 100 is
    /// treated as 100.
    pub fn set_stream_volume(index: u32, volume: u8) {
        worker::run(move || streams::set_volume(index, volume.min(100)));
    }

    /// Mutes or unmutes one app's stream.
    pub fn set_stream_muted(index: u32, muted: bool) {
        worker::run(move || streams::set_muted(index, muted));
    }

    /// Mutes one app's stream, or unmutes it when it was muted.
    pub fn toggle_stream_mute(index: u32) {
        worker::run(move || {
            if let Some(muted) = streams::muted(index) {
                streams::set_muted(index, !muted);
            }
        });
    }

    // anything above 100 is treated as 100
    pub fn set_volume(volume: u8) {
        worker::run(move || pulse::set_volume(Device::Output, volume.min(100)));
    }

    pub fn toggle_mute() {
        worker::run(|| toggle(Device::Output));
    }

    // anything above 100 is treated as 100
    pub fn set_microphone_volume(volume: u8) {
        worker::run(move || pulse::set_volume(Device::Input, volume.min(100)));
    }

    pub fn toggle_microphone_mute() {
        worker::run(|| toggle(Device::Input));
    }

    // takes an output's name(), and what is playing moves to it
    pub fn set_output(name: &str) {
        let name = String::from(name);

        worker::run(move || pulse::set_default(Device::Output, &name));
    }

    // takes an input's name()
    pub fn set_input(name: &str) {
        let name = String::from(name);

        worker::run(move || pulse::set_default(Device::Input, &name));
    }
}

fn toggle(device: Device) {
    let level = pulse::read(device);

    pulse::set_muted(device, !level.muted);
}

fn devices(device: Device) -> Vec<AudioDevice> {
    let (endpoints, default) = pulse::devices(device);

    endpoints
        .iter()
        .map(|endpoint| AudioDevice::from_endpoint(endpoint, &default))
        .collect()
}

// built-in when nothing is the default, which is what most machines fall back to
fn default_kind(devices: &[AudioDevice]) -> AudioKind {
    devices
        .iter()
        .find(|device| device.default)
        .map(AudioDevice::kind)
        .unwrap_or_default()
}
