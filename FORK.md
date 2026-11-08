# Fork notice

This is a fork of [amane](https://github.com/MystiaFin/amane), MystiaFin's declarative Rust
library for Wayland shells, kept for the
[emaki](https://github.com/emaki-desktop/emaki) desktop. It is not meant to be
upstreamed; it is kept cheap to rebase onto upstream instead.

## What it adds

- xdg-popups over layer and normal windows, with grabs and nesting
- app windows that real programs run in: app-ids, state, dialogs, clipboard, drag and drop,
  activation tokens, portals, one socket per app-id
- niri compositor services: windows, gestures frame by frame, overview, workspace switch, the
  desktop's state, keyboard layouts, one shared event stream
- blur regions, blur shapes with materials, backdrop readings and vibrancy, through the
  [niri fork](https://github.com/emaki-desktop/niri)'s protocols
- parley text (italic, spacing, decoration, direction, color emoji), text input with
  selection, undo and IME, and being the seat's input method
- SVG paths, multi-pass WGSL shader effects with hot reload, video frames, spring animation,
  coasting scroll
- system services over D-Bus and the kernel: battery, power profile, Bluetooth with its
  pairing agent, the tray, brightness in HDR, idle, a polkit agent, appearance, audio,
  notifications kept across restarts, apps and MIME defaults, rfkill, logind, lock keys,
  media, NetworkManager, a clipboard watcher, timers and subscriptions
- less memory: a [vello fork](https://github.com/emaki-desktop/vello) with shell-sized scratch
  buffers let go when idle, GPU memory released on hide, shadows and simple shapes drawn as
  quads, images decoded only as big as they are drawn

## Branch and base

The work is branch `emaki`, a series of commits over tag `fork-base`, upstream commit `544cbfc`
(upstream's `main` of 2026-10-21). `main` mirrors upstream's `main`; `ocbk-pre-tidy` is the
history before the last rebase.

## Using it

Pin a commit of `emaki` as a git dependency, repeat the vello `[patch.crates-io]` lines in your
root manifest, and read the feature table and the environment variables in
[docs/fork/README.md](docs/fork/README.md#using-the-fork).

## Maintaining it

[docs/fork/maintaining.md](docs/fork/maintaining.md): the rebase routine, where a rebase
conflicts, the commit map and the housekeeping rules.

Upstream's files stay as upstream has them. Fork code lives in files of its own; upstream's
`docs/src/` and `ARCHITECTURE.md` are byte-identical to `fork-base` in every commit; and the
only upstream files the fork edits outside `src/` and `examples/` are the ones listed in
maintaining.md (`Cargo.toml`, `Cargo.lock`, `flake.nix`, three files of the CLI, the one-line
banner at the top of `README.md`) beside the files it adds (`FORK.md`, `clippy.toml`, the niri
protocols, `.github/workflows/fork.yml`). All fork documentation is under `docs/fork/`.
