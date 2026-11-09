#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! How loud the default microphone is right now, for a level meter.
//!
//! It records only while something reads it: the sound server works out
//! each peak itself (`PEAK_DETECT`, as pavucontrol does), about 25 a second,
//! so nothing here sees the sound itself. The stream keeps the microphone
//! awake while it records, or no peaks would come. A second after the last read the
//! recording stops, and the next read starts it again.

use std::thread;
use std::time::{Duration, Instant};

use libpulse_binding::def::BufferAttr;
use libpulse_binding::proplist::Proplist;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::{FlagSet, PeekResult, State, Stream};

use super::pulse::Connection;
use crate::Service;
use crate::services::store;

/// The default microphone's level, 0 to 1, while something reads it.
///
/// Its recording stream carries the property `amane.level-meter = "1"`, so a
/// privacy indicator can tell it from an app listening.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct AudioLevel {
    peak: f32,
}

impl AudioLevel {
    /// The loudest sample of the last moment, 0 to 1 on a linear scale (pulse's
    /// peak), and 0 while nothing reads it.
    pub fn peak(&self) -> f32 {
        self.peak
    }
}

// peaks a second
const RATE: u32 = 25;

// how long without a read before the recording stops
const WATCH: Duration = Duration::from_secs(1);

// how long to wait before trying again when the microphone can't be opened
const RETRY: Duration = Duration::from_secs(2);

impl Service for AudioLevel {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {
        loop {
            store::wait_while_unread::<Self>(WATCH);
            let heard = Connection::open().is_some_and(meter);
            set(0.0);
            if !heard {
                thread::sleep(RETRY);
            }
        }
    }
}

fn set(peak: f32) {
    let mut level = AudioLevel::write();
    if (level.peak - peak).abs() < 0.002 {
        level.quiet();
    } else {
        level.peak = peak;
    }
}

// records until nothing reads the level, and says whether any came
fn meter(mut connection: Connection) -> bool {
    let spec = Spec {
        format: Format::FLOAT32NE,
        channels: 1,
        rate: RATE,
    };
    let Some(mut properties) = Proplist::new() else {
        return false;
    };
    let _ = properties.set_str("application.name", "amane");
    let _ = properties.set_str("media.name", "Input level");
    let _ = properties.set_str("amane.level-meter", "1");
    let Some(mut stream) = Stream::new_with_proplist(
        &mut connection.context,
        "Input level",
        &spec,
        None,
        &mut properties,
    ) else {
        return false;
    };
    // one sample a fragment, so each peak arrives as it's measured
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: 4,
    };
    let flags = FlagSet::PEAK_DETECT | FlagSet::ADJUST_LATENCY;
    if stream.connect_record(None, Some(&attr), flags).is_err() {
        return false;
    }

    let mut heard = false;
    let mut checked = Instant::now();
    while connection.iterate() {
        match stream.get_state() {
            State::Failed | State::Terminated => break,
            State::Ready => {}
            _ => continue,
        }
        if let Some(peak) = drain(&mut stream) {
            heard = true;
            set(peak);
        }
        if checked.elapsed() >= WATCH / 4 {
            checked = Instant::now();
            if !AudioLevel::watched_within(WATCH) {
                break;
            }
        }
    }
    let _ = stream.disconnect();
    heard
}

// the loudest of the peaks waiting, none when none came
fn drain(stream: &mut Stream) -> Option<f32> {
    let mut loudest: Option<f32> = None;
    loop {
        match stream.peek() {
            Ok(PeekResult::Empty) | Err(_) => return loudest,
            Ok(PeekResult::Hole(_)) => {}
            Ok(PeekResult::Data(data)) => {
                if let Some(peak) = last_sample(data) {
                    loudest = Some(loudest.map_or(peak, |was| was.max(peak)));
                }
            }
        }
        if stream.discard().is_err() {
            return loudest;
        }
    }
}

fn last_sample(data: &[u8]) -> Option<f32> {
    let start = data.len().checked_sub(4)?;
    let bytes: [u8; 4] = data.get(start..)?.try_into().ok()?;
    Some(f32::from_ne_bytes(bytes).abs().clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fragments_peak_is_its_last_sample() {
        let mut data = 0.1f32.to_ne_bytes().to_vec();
        data.extend_from_slice(&(-0.6f32).to_ne_bytes());
        assert_eq!(last_sample(&data), Some(0.6));
        assert_eq!(last_sample(&[1, 2]), None);
        assert_eq!(last_sample(&2.0f32.to_ne_bytes()), Some(1.0));
    }
}
