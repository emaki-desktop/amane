# amane fork docs

This fork of [amane](https://github.com/MystiaFin/amane) adds what the
[emaki](https://github.com/emaki-desktop/emaki) desktop needs: xdg-popups, niri
compositor services that follow gestures frame by frame, app windows that real apps run in,
blur shapes and vibrancy, parley text, spring animation, and more system services. The work is
on branch `emaki`, on top of upstream commit `544cbfc` (tag `fork-base`, upstream's `main` of
2026-10-21).

Upstream's own guide ([`docs/src/`](../src/), the mdBook) and `ARCHITECTURE.md` are left exactly
as upstream has them; everything the fork adds is here.

This page is in two parts. [Using the fork](#using-the-fork) is for a program that depends on
it: what it adds, how to pin it, what it reads from the environment. [Maintaining the
fork](#maintaining-the-fork) is for whoever rebases it onto upstream.

## Using the fork

### What it adds

| Feature | Summary | Key types | Doc | Example | Status |
|---|---|---|---|---|---|
| Workspaces | the workspace list (upstream) | `Workspaces`, `Workspace` | [services](compositor/services.md#workspaces) | `examples/workspaces.rs` | upstream |
| Windows | open windows, the focused one, and compositor actions | `Windows`, `WindowInfo` | [services](compositor/services.md#windows-fork-addition) | — | done |
| niri client | one shared event stream; asking niri for outputs, windows, casts and the fork's features | `amane::niri`, `niri::features()` | [services](compositor/services.md#asking-niri-amaneniri-fork-addition) | — | done |
| Keyboard layouts | the layouts and the one in use | `KeyboardLayouts` | [services](compositor/services.md#keyboardlayouts-fork-addition-niri-only) | — | done, niri |
| Overview | niri's overview progress alone | `Overview` | [services](compositor/services.md#overview-fork-addition-niri-only) | — | done, niri fork |
| Gestures | per-frame overview, view scroll and workspace switch | `Gestures` | [services](compositor/services.md#gestures-fork-addition-niri-only) | — | done, niri fork |
| WorkspaceSwitch | the workspace switch alone | `WorkspaceSwitch` | [services](compositor/services.md#workspaceswitch-fork-addition-niri-only) | — | done, niri fork |
| Desktop | whether each output's desktop is obscured; frozen layer windows | `Desktop`, `LayerWindow::frozen` | [services](compositor/services.md#desktop-fork-addition-niri-only) | — | done, niri fork |
| Popups | xdg-popups over layer and normal windows, grabs, nesting | `Popup`, `open_popup` | [popups](windows/popups.md) | `examples/app_window.rs` | done |
| App windows | normal windows with app-ids, live settings, state, close veto, dialogs | `Window`, `window_state`, `open_window_with` | [app-windows](windows/app-windows.md) | `examples/app_window.rs` | done |
| Activation | launching with tokens, being activated | `spawn_activated`, `App::on_activate` | [activation](windows/activation.md) | `examples/app_window.rs` | done |
| Clipboard | clipboard, primary selection, text drops, dragging out | `Clipboard`, `PrimarySelection`, `DragData` | [clipboard](windows/clipboard.md) | `examples/app_window.rs` | done |
| Portals | file dialog and sending notifications | `FileDialog`, `Notify` | [portals](windows/portals.md) | `examples/app_window.rs` | done |
| Config | settings files shared live between programs | `Config`, `Settings` | [config](windows/config.md) | — | done |
| Blur | blur regions and rounded, fading blur shapes with materials | `BlurShape`, `blur_region`, `blur_shapes` | [blur](graphics/blur.md) | `examples/blur_shapes.rs` | done; shapes need the niri fork |
| Backdrop | niri's luminance readings behind each shape | `Backdrop` | [blur](graphics/blur.md#blur-shapes--niri-blur-shapes-v1) | — | done, niri fork |
| Vibrancy | grey content blended with the frosted backdrop | `vibrant` | [blur](graphics/blur.md#vibrancy-niri-vibrancy-v1) | — | done, niri fork |
| Canvas paths | SVG paths, placing, trimming, line joins; 64 shader rows | `Path::svg`, `Join` | [canvas](graphics/canvas.md) | `examples/stroke_clock.rs` | done |
| Shader effects | multi-pass WGSL over a rectangle's drawing, its backdrop or an image; hot reload; compile errors; a pausable shader clock; frame stats; shadow spread | `Effect`, `Input`, `shader_errors`, `pause_shader_time`, `frame_stats`, `Shadow::spread` | [effects](graphics/effects.md) | `examples/effect.rs` | done |
| Text styling | parley layout: italic, line height, spacing, case, decoration, align, direction, axes; color emoji | `Case`, `Decoration`, `TextAlign`, `TextDirection`, `Weight::Number` | [text](graphics/text.md) | `examples/text.rs` | done |
| Video | NV12 frames from another thread, drawn as quads | `VideoFeed`, `VideoFrame`, `Video` | [video](graphics/video.md) | — | done |
| Animation | spring easing that keeps its speed, `retime` | `Easing::Spring`, `Animation::retime` | [animation](graphics/animation.md) | — | done |
| Scrolling | coasting after a flick, eased wheel steps, following a chat's end, keeping the view as content is added above | `ScrollArea::scroll_to_end`, `at_end`, `keep_bottom` | [input](input/input.md#scrolling) | — | done |
| `on_track` | pointer motion that redraws nothing; also scroll source, modifiers, shortcuts, double clicks | `on_track`, `Scroll`, `modifiers` | [input](input/input.md) | — | done |
| Text input | selection, editing keys, undo, multiline, IME | `TextInput` | [text-input](input/text-input.md) | `examples/text_editor.rs` | done |
| Input method | be the seat's input method: a popup by the focused field in any app, and checked fill through niri-secure-fill | `App::input_method`, `FocusedField`, `FieldOwner`, `FillError` | [input-method](input/input-method.md) | `examples/input_method.rs` | done, niri fork for fill |
| Battery | UPower: state, warning, time left, peripherals | `Battery`, `PowerDevice` | [battery](system/battery.md) | `examples/system.rs` | done |
| Power profile | power-profiles-daemon | `PowerProfile` | [power-profile](system/power-profile.md) | — | done |
| Bluetooth | signals, operations and failures, pairing agent, rfkill, discoverable | `Bluetooth::serve_agent`, `PairingRequest`, `ServiceError` | [bluetooth](system/bluetooth.md) | — | done |
| Tray | the system tray: StatusNotifierItem watcher and host, item icons (theme, own folder or pixels), dbusmenu menus, clicks and scrolling | `Tray`, `TrayItem`, `TrayMenuItem`, `TrayStatus`, `TrayToggle` | [tray](system/tray.md) | — | done |
| Brightness | in HDR, the panel's SDR white in place of the backlight; uevents instead of polling | `Brightness::hdr`, `HdrPanel` | [brightness](system/brightness.md) | — | done |
| Idle | ext-idle-notify levels and ScreenSaver inhibits | `Idle` | [idle](system/idle.md) | `examples/idle.rs` | done |
| Polkit agent | the session's authentication agent: polkit's prompts as the shell's own; `polkit-agent` feature | `Polkit::serve_agent`, `serve_agent_with`, `AuthRequest` | [polkit](system/polkit.md) | — | done |
| Appearance | color scheme, accent, contrast, reduced motion | `Appearance`, `Scheme` | [appearance](system/appearance.md) | `examples/appearance.rs` | done |
| Audio | device lists, kinds, default switching | `AudioDevice`, `AudioKind` | [audio](system/audio.md) | `examples/audio.rs` | done |
| Notifications | timeout, hints, icon path, expire | `Timeout`, `Notifications::expire` | [notifications](system/notifications.md) | `examples/notifications.rs` | done |
| Apps and processes | desktop entry id, categories, actions, MIME, opening files | `DesktopApp`, `DesktopAction` | [apps-and-processes](system/apps-and-processes.md) | `examples/apps.rs` | done |
| Radios | rfkill switches from `/dev/rfkill`, airplane mode, blocking | `Rfkill`, `Radio` | [rfkill](system/rfkill.md) | — | done |
| Session | logind: active, locked and idle hints, lock requests, sleep, inhibitors | `Logind` | [logind](system/logind.md) | — | done |
| Clipboard watcher | the selection as it changes, from any program, for a history | `ClipboardWatcher` | [clipboard-watcher](system/clipboard-watcher.md) | — | done |
| Service errors | one failure type for the fork's services, and the helpers a program's own services share | `ServiceError`, `ErrorKind`, `run_in_order`, `Service::watched_within`, `Service::peek` | [errors](system/errors.md) | — | done |
| Media | players followed by their MPRIS signals instead of a poll; the position worked out from the last reading | `Media`, `MediaPlayer::position` | [media](system/media.md) | — | done |
| Network | NetworkManager's signals; VPN, WireGuard, hotspot and wired profiles | `Network::profiles`, `NetworkProfile`, `Wired` | [network](system/network.md) | — | done |
| Lock keys | Caps Lock and Num Lock | `LockKeys` | [lock-keys](system/lock-keys.md) | — | done |
| Timers | jobs and redraws on the event loop at a time | `Timer`, `TimerHandle`, `redraw_after` | [timer](system/timer.md) | — | done |
| Subscriptions | one service following another; polling only while watched | `Service::subscribe`, `Subscription`, `Service::idle_stop` | [subscriptions](system/subscriptions.md) | — | done |
| Launching | starting programs without panics, each in its own systemd scope, with the compositor's token when asked | `Launch`, `Launch::activated`, `LaunchError` | [apps-and-processes](system/apps-and-processes.md#each-program-in-its-own-scope) | — | done |
| IPC | a socket per app-id, one copy per app, calling another app | `ipc_call`, `App::on_activate` | [ipc](system/ipc.md) | `examples/app_window.rs` | done |
### Groups

- **Compositor:** niri services and actions, each a `Service` like upstream's.
  [compositor/services.md](compositor/services.md).
- **Windows:** surfaces beyond layer windows, and what apps need around them.
  [popups](windows/popups.md), [app-windows](windows/app-windows.md),
  [activation](windows/activation.md), [clipboard](windows/clipboard.md),
  [portals](windows/portals.md), [config](windows/config.md).
- **Graphics:** what is drawn and how it moves. [blur](graphics/blur.md),
  [canvas](graphics/canvas.md), [text](graphics/text.md), [video](graphics/video.md),
  [animation](graphics/animation.md).
- **Input:** pointer and keyboard additions. [input](input/input.md),
  [text-input](input/text-input.md), [input-method](input/input-method.md).
- **System:** D-Bus and portal services. [battery](system/battery.md),
  [power-profile](system/power-profile.md), [bluetooth](system/bluetooth.md),
  [tray](system/tray.md), [idle](system/idle.md),
  [polkit](system/polkit.md),
  [appearance](system/appearance.md), [audio](system/audio.md),
  [notifications](system/notifications.md), [apps-and-processes](system/apps-and-processes.md),
  [ipc](system/ipc.md), [rfkill](system/rfkill.md), [logind](system/logind.md),
  [clipboard-watcher](system/clipboard-watcher.md), [network](system/network.md),
  [lock-keys](system/lock-keys.md), [media](system/media.md), [errors](system/errors.md).
- **Internals:** how the fork changes amane inside, and the resource plan.
  [architecture](internals/architecture.md) (sources, `on_track`, frame skip, GPU bounds, frozen
  windows), [memory](internals/memory.md) (what a process holds in GPU memory and RAM, the vello
  fork's shell-sized scratch buffers, how to measure), [optimization-plan](internals/optimization-plan.md).

### Pinning it

The fork is a git dependency, pinned to a commit on `emaki`:

```toml
[dependencies]
amane = { git = "https://github.com/emaki-desktop/amane", rev = "<emaki commit>", features = ["polkit-agent"] }
```

Features: `polkit-agent` (be the session's authentication agent, [polkit](system/polkit.md));
`vello-stats` (with `AMANE_FRAMES=1`, what each vello run took of the scratch buffers,
[memory](internals/memory.md)). Both are off by default.

The fork draws through a [vello fork](https://github.com/emaki-desktop/vello) (branch `emaki`) whose
scratch buffers are sized for a shell and let go when idle. Cargo applies a `[patch.crates-io]`
only from the root manifest, so a program that depends on amane repeats the two lines from
amane's `Cargo.toml`, at the same `rev`:

```toml
[patch.crates-io]
vello = { git = "https://github.com/emaki-desktop/vello", rev = "<vello commit>" }
vello_encoding = { git = "https://github.com/emaki-desktop/vello", rev = "<vello commit>" }
```

A Nix build (`rustPlatform.buildRustPackage`) needs a content hash for each git dependency in
`cargoLock.outputHashes`: one for `amane-<version>` and one for `vello-<version>`, which covers
every crate from the same vello rev. After a `rev` bump the first build prints the expected
`sha256-…` on mismatch; emaki's `flake.nix` is the model.

### Environment variables

What the library reads from the environment. None is needed; each is a switch for debugging
or for comparing against the behaviour before a change.

| Variable | Read in | Meaning |
|---|---|---|
| `AMANE_FRAMES=1` | `timing.rs` (upstream's) | print a line for every drawn frame, to see where frame time goes; the fork adds which service woke the windows and, under `vello-stats`, each vello run's scratch use |
| `AMANE_DEBUG=1` | `wayland/popup.rs` | print each popup open and close, and an open skipped because its parent surface is not up yet |
| `AMANE_WAKE_ALL=1` | `changes.rs` | wake the event loop for every source change, not only those a window reads |
| `AMANE_PRESENT=mailbox` | `graphics/gpu/target.rs` | present with Mailbox instead of Fifo |
| `AMANE_KEEP_HIDDEN=1` | `graphics/gpu.rs` | keep a hidden window's GPU memory instead of releasing it on hide |
| `AMANE_KEEP_SPARES=1` | `graphics/gpu/pool.rs` | keep every spare texture until the window resizes, instead of ageing them out |
| `AMANE_DEVICE_IDLE=<s>` | `graphics/gpu/shared.rs` | seconds a process whose windows are all hidden keeps its GPU device and vello; 60 by default, 0 for life |
| `AMANE_VELLO_IDLE=<s>` | `graphics/gpu/shared.rs` | seconds vello's scratch buffers stay once no window draws with vello; 30 by default, 0 for life |
| `AMANE_VELLO_SCRATCH=upstream` | `graphics/gpu/shared.rs` | upstream's scratch sizes instead of the vello fork's shell-sized ones, for a scene that overflowed them and came out blank |
| `AMANE_VELLO_SHADOWS=1` | `graphics/gpu/paint/shapes.rs` | draw drop shadows with vello instead of the quads |
| `AMANE_VELLO_SHAPES=1` | `graphics/gpu/paint/shapes.rs` | draw arcs, lines and the areas under them with vello instead of the quads |
| `AMANE_IMAGE_CACHE_MB=<n>` | `graphics/image.rs` | the decoded-image cache's cap in MB; 64 by default |
| `AMANE_IMAGE_FULL=1` | `graphics/image.rs` | decode every image at its own size instead of only as big as it is drawn |
| `AMANE_NO_BLUR_SHAPES=1` | `wayland.rs` | ignore niri's blur shapes global: the blur region a compositor without shapes would give |
| `AMANE_NO_VIBRANCY=1` | `wayland.rs` | ignore niri's vibrancy global: vibrant windows as a compositor without it would draw them |

`AMANE_CONFIG=<folder>` is read by the upstream `amane` CLI (`cli/src/paths.rs`), not by the
library: it points the CLI at another shell folder without changing its saved target.

### API cheatsheet

[api-cheatsheet.md](api-cheatsheet.md) has the fork's rows for upstream's
[cheatsheet](../src/cheatsheet.md).

### Moved to emaki

Features only emaki and its apps use went to crates of their own in
[emaki](https://github.com/emaki-desktop/emaki) (Phase 9 of its roadmap), so this fork
stays a generic library:

| Was | Now |
|---|---|
| `status_item` (`StatusItem`, `LiveActivity`, the `org.emaki.StatusBar1` wire types) | `emaki-status` (`crates/status`) |
| `KdeConnect`, `PhoneMessages` and the phone types | `emaki-phone` (`crates/phone`) |
| `Adb`, `amane::adb`, mDNS discovery (and the `mdns-sd` dependency) | `emaki-phone` |

They build on public amane API: `Bus`, `Service`, `run_in_order`, `Service::watched_within`,
`Service::peek` and `ServiceError` ([errors](system/errors.md)).
### Other repos

- **Server side:** the [niri fork's docs](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/README.md)
  cover the protocols and IPC these features use: blur shapes, vibrancy, materials, gesture
  events, `DesktopObscured`, `MoveColumnToIndex` by window id.
- **Main consumer:** [emaki's docs](https://github.com/emaki-desktop/emaki/blob/main/docs/README.md).

## Maintaining the fork

The fork is never upstreamed; it is kept cheap to rebase instead. One commit, or a few, per
feature; fork code in files of its own; as few lines as can be in upstream's files; and
upstream's `docs/src/` and `ARCHITECTURE.md` byte-identical to `fork-base` in every commit.
[maintaining.md](maintaining.md) has the rebase routine, where a rebase conflicts, the commit
map, the lints and the housekeeping rules.

### Branches and tags

| | |
|---|---|
| `emaki` | the work branch: the fork's commits over `fork-base` (`git log --oneline fork-base..emaki`) |
| `main` | mirrors upstream's `main`, nothing of the fork's |
| `ocbk-pre-tidy` | the history before the last rebase, kept on origin so revisions pinned before it can still be fetched |
| `fork-base` | the upstream commit the fork sits on: `544cbfc`, upstream's `main` of 2026-10-21 |
| `fork-base-<date>` | each base the fork has sat on: `fork-base-2026-10-16` (`72e23eb`, the first) and `fork-base-2026-11-07` (`544cbfc`, the current) |

### Design history

Implemented specs, kept for their reasoning:

- [spec-presence-appearance.md](design/spec-presence-appearance.md): Idle and Appearance.
- [spec-write-own-layer-rules.md](design/spec-write-own-layer-rules.md): blur behind the bar and
  activation tokens.
