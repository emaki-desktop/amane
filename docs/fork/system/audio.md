# Audio devices, app streams, balance and level (fork)

Upstream's `Audio` service only follows the *default* output and input. The fork adds:
- the list of devices, what kind each one is, and switching the default, so a shell can offer a
  sound menu and show a headphones icon while headphones are in use;
- each app's playback stream, with its own level and mute;
- the default output's balance;
- `AudioLevel`, a microphone level meter that runs only while read.

Everything else in [the Audio page](../../src/audio.md) is unchanged, apart from the volume fix
under [Balance](#balance).

## Usage

```rust
use amane::{Audio, AudioKind, Service};

let audio = Audio::read();

for output in audio.outputs() {
    println!("{} {:?} default={}", output.description(), output.kind(), output.default());
}

if audio.output_kind() == AudioKind::Headphones {
    // show a headphones glyph
}

Audio::set_output("bluez_output.AA_BB_CC_DD_EE_FF.1");
```

| Function | Gives or does |
|---|---|
| `outputs()` / `inputs()` | every output / input as an `AudioDevice`, in the sound server's order; monitors of outputs are left out |
| `output_kind()` / `input_kind()` | the default device's `AudioKind`, `Internal` when there is none |
| `Audio::set_output(name)` / `Audio::set_input(name)` | makes that device the default; PipeWire moves what is playing along |
| `AudioDevice::name()` | the sound server's name, which the setters take |
| `AudioDevice::description()` | the label to show (falls back to the name) |
| `AudioDevice::kind()` | `Internal`, `Speaker`, `Headphones`, `Headset`, `Display`, `Car`, `Phone`, `Webcam` or `Microphone` |
| `AudioDevice::default()` | `true` for the current default |

## Design / internals: how the kind is found

First match wins (`src/services/audio/device.rs`, unit-tested):

1. **The jack in use** — the active port and node name contain `headset`, `headphone`, `hdmi` or
   `displayport`. One sound card switches between its speakers and plugged-in headphones by
   changing its port (or, with UCM, its node), so this catches wired headphones.
2. **`device.form_factor`**, which USB and Bluetooth devices set: `internal`, `speaker` / `hifi` /
   `portable` / `computer`, `headphone`, `headset` / `hands-free`, `tv`, `car`, `handset`,
   `webcam`, `microphone`.
3. A port or node named `speaker` is the built-in speaker.
4. **`device.icon_name`** for devices that only set an icon: `audio-headset*`, `audio-headphones*`,
   `audio-speakers*`, `video-display*`, `camera-web*`.
5. Otherwise `Internal`.

A device that lands on the wrong kind can be checked with
`pw-dump | grep -E 'form.factor|icon.name|node.name'` (or `pactl list sinks` with pulseaudio).

The device list re-reads on every sink, source and server event, which covers plugging, Bluetooth
connects and default changes. See `examples/audio.rs`.

## App streams

```rust
use amane::{Audio, Service};

for stream in Audio::read().streams() {
    println!("{} ({}) {}% muted={}", stream.app(), stream.media(), stream.volume(), stream.muted());
}
Audio::set_stream_volume(42, 60);
Audio::toggle_stream_mute(42);
```

| Function | Gives or does |
|---|---|
| `streams()` | every app's playback stream (pulse's sink inputs), in the sound server's order |
| `AudioStream::index()` | the sound server's number for it, which the setters take |
| `app()`, `icon()`, `binary()`, `media()` | `application.name`, `application.icon_name`, `application.process.binary`, `media.name`; any may be empty |
| `volume()`, `muted()`, `paused()` | its level (0 to 100, the loudest channel), its own mute, and whether it's corked |
| `Audio::set_stream_volume(index, volume)` | scales its channels to `volume`, keeping their balance |
| `Audio::set_stream_muted(index, muted)` / `toggle_stream_mute(index)` | mutes it alone |

These streams are left out:
- event sounds (`media.role` `event`, or PipeWire's `Notification`), such as a notification's or a
  click, which end before they could be set;
- streams without a writable volume.

The listener now also subscribes to `SINK_INPUT`, so a stream starting, stopping or changing
redraws (`src/services/audio/streams.rs`, unit-tested).

## Balance

| Function | Gives or does |
|---|---|
| `balance()` | the default output's lean, −1 (all left) to 1 (all right); `None` without a left and right (`Map::can_balance`) |
| `Audio::set_balance(balance)` | leans it (`ChannelVolumes::set_balance`), keeping the volume |

**The volume fix.**
- `set_volume` and `set_microphone_volume` used to set every channel to the same level. That wiped
  any balance on each volume key press.
- They now scale the channels to the new loudest one (`ChannelVolumes::scale`).
- `volume()` reads the loudest channel instead of the average, as pulse's own mixers do. The two
  only differ once a balance leans.

See `src/services/audio/balance.rs`, unit-tested with libpulse's own arithmetic.

## Input level

```rust
use amane::{AudioLevel, Service};

let peak = AudioLevel::read().peak(); // 0 to 1, linear
```

`AudioLevel` is a service of its own (`src/services/audio/level.rs`).
- It opens a recording stream on the default source with `PEAK_DETECT` (as pavucontrol does), at
  one mono float sample per fragment and 25 a second, so the server sends peaks, not sound.
- The stream runs only while something reads the service: its listener waits in
  `store::wait_while_unread`. A second after the last read it disconnects, and `peak()` goes to 0.
- Reading it from a view each frame keeps it running while that view shows.
- The stream carries `application.name = "amane"`, `media.name = "Input level"` and
  `amane.level-meter = "1"`. A privacy indicator can match that property to tell it from an app
  listening.

## Fix: dropping the pulse context

`dedcda4` also fixes an abort when a thread that had used audio exited: the pulse context is now
dropped before its mainloop (`src/services/audio/pulse.rs`).
