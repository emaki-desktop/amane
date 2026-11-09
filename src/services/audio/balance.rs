#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

//! Left and right on the default output: pulse keeps a level per channel,
//! and the balance is how far they lean to one side.

use libpulse_binding::channelmap::Map;
use libpulse_binding::volume::ChannelVolumes;

use super::pulse::{Device, with_connection};

/// The default output's balance, −1 (all left) to 1 (all right); none when
/// it has no left and right, like a mono speaker.
pub(super) fn read() -> Option<f32> {
    with_connection(|connection| {
        let levels = connection.levels(Device::Output)?;
        of(&levels.volumes, &levels.map)
    })
}

pub(super) fn of(volumes: &ChannelVolumes, map: &Map) -> Option<f32> {
    if !map.can_balance() {
        return None;
    }
    // pulse's own sum leaves a hair off centre after a round trip
    let balance = volumes.get_balance(map);
    Some(if balance.abs() < 0.005 { 0.0 } else { balance })
}

/// Leans the default output to `balance`, keeping its volume.
pub(super) fn set(balance: f32) {
    with_connection(|connection| {
        let Some(levels) = connection.levels(Device::Output) else {
            return;
        };
        let mut volumes = levels.volumes;
        if lean(&mut volumes, &levels.map, balance).is_none() {
            return;
        }
        let operation = connection.context.introspect().set_sink_volume_by_name(
            Device::Output.name(),
            &volumes,
            None,
        );
        connection.wait(operation);
    });
}

pub(super) fn lean(volumes: &mut ChannelVolumes, map: &Map, balance: f32) -> Option<()> {
    if !map.can_balance() {
        return None;
    }
    volumes.set_balance(map, balance.clamp(-1.0, 1.0))?;
    Some(())
}

#[cfg(test)]
mod tests {
    use libpulse_binding::channelmap::MapDef;
    use libpulse_binding::volume::Volume;

    use super::*;

    fn stereo() -> (ChannelVolumes, Map) {
        let mut map = Map::default();
        map.init_stereo();
        let mut volumes = ChannelVolumes::default();
        volumes.set(2, Volume::NORMAL);
        (volumes, map)
    }

    #[test]
    fn a_lean_reads_back_and_keeps_the_volume() {
        let (mut volumes, map) = stereo();
        assert_eq!(of(&volumes, &map), Some(0.0));
        lean(&mut volumes, &map, -0.5).expect("stereo leans");
        let balance = of(&volumes, &map).expect("stereo has a balance");
        assert!((balance + 0.5).abs() < 0.01, "{balance}");
        assert_eq!(volumes.max(), Volume::NORMAL);
    }

    #[test]
    fn scaling_the_volume_keeps_the_lean() {
        let (mut volumes, map) = stereo();
        lean(&mut volumes, &map, 0.4).expect("stereo leans");
        volumes.scale(Volume(Volume::NORMAL.0 / 2));
        let balance = of(&volumes, &map).expect("stereo has a balance");
        assert!((balance - 0.4).abs() < 0.01, "{balance}");
    }

    #[test]
    fn mono_has_none() {
        let mut map = Map::default();
        map.init_auto(1, MapDef::ALSA);
        let mut volumes = ChannelVolumes::default();
        volumes.set(1, Volume::NORMAL);
        assert_eq!(of(&volumes, &map), None);
        assert!(lean(&mut volumes, &map, 0.5).is_none());
    }
}
