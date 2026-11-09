# Maintaining the fork

How the fork is kept on top of upstream, which commits make up each feature, and the housekeeping
rules. What each feature does is in the [index](README.md).

The fork is never upstreamed. It is kept cheap to rebase instead: one commit, or a few, per
feature, fork code in files of its own, and as few lines as can be in upstream's files.

## Provenance

| | |
|---|---|
| Upstream | `https://github.com/MystiaFin/amane` (remote: `upstream`) |
| Fork point | upstream commit `544cbfc` (tags `fork-base` and `fork-base-2026-11-07`), upstream's `main` of 2026-10-21, taken as the base on 2026-11-07 |
| Before that | `72e23eb` (tag `fork-base-2026-10-16`), the commit the fork was first written against; that history is kept on `ocbk-pre-tidy` |
| Fork branch | `emaki`; the fork's `main` mirrors upstream's `main` (fast-forward it: `git push origin upstream/main:main`) |
| The delta | `git log --oneline fork-base..emaki`: 118 commits as of 2026-11-08 (`git rev-list --count fork-base..emaki`) |

The fork point is a real upstream commit, so `git log fork-base..HEAD` and `git range-diff` line
up against genuine upstream history. `ocbk-pre-tidy` stays on origin so the revisions emaki
pinned before the rebase can still be fetched.

Branches: `emaki` is the only work branch, `main` only follows upstream, and `ocbk-pre-tidy` is the
history before the last rebase. Scratch branches a rebase makes (the reference merge, the
rebased copy) are deleted once the rebase lands; nothing may refer to them.

## Upstream files stay as they were

All fork documentation lives in `docs/fork/`. Upstream's mdBook (`docs/src/`) and
`ARCHITECTURE.md` are byte-identical to `fork-base`, in every fork commit, not only at the tip;
the fork's additions to them are in [`docs/fork/`](README.md) instead. Check it with:

```sh
git diff --stat fork-base HEAD -- docs/src ARCHITECTURE.md   # prints nothing
```

Outside `src/`, `examples/` and `docs/fork/`, these are the files the fork touches, and why
(`git diff fork-base --stat -- . ':!docs/fork' ':!src' ':!examples'`):

| File | Why |
|---|---|
| `Cargo.toml` | the fork's dependencies, its two features, `[lints]` for the whole crate, and the `[patch.crates-io]` for the vello fork |
| `Cargo.lock` | follows `Cargo.toml` |
| `flake.nix` | `cargoLock.outputHashes` for the vello fork, a git dependency Nix must hash |
| `clippy.toml` | new: `unwrap` and `dbg!` allowed in tests |
| `cli/src/call.rs`, `cli/src/main.rs` | `amane ipc call --app <id>` reaches an app by app-id through the library's `ipc_call` |
| `cli/src/target.rs`, `cli/src/main.rs` | rustfmt's line wrapping of the usage lines, so `cargo fmt --check` passes in CI |
| `protocols/niri-blur-shapes-v1.xml`, `niri-vibrancy-v1.xml`, `niri-secure-fill-v1.xml` | new: the niri fork's protocols the fork binds |
| `README.md` | a one-line banner at the top saying this is a fork; the rest is upstream's |
| `FORK.md` | new: the fork notice |
| `.github/workflows/fork.yml` | new: the fork's CI, so upstream's `test.yml` is never edited |

One upstream file is removed: `examples/text_stroke.rs` calls a `Text::stroke` that upstream
hasn't committed, so it doesn't build upstream either
(`95aa453`). Bring it back when upstream adds the method.

`Cargo.lock` conflicts on nearly every rebase: take upstream's, then let cargo add the fork's
crates back.

## Where a rebase conflicts

By lines added, measured against the fork point before (`72e23eb`) and after (`544cbfc`) the
Phase 12 tidy, which moved fork code out into files of its own:

| File | Hunks (lines added) before | After |
|---|---|---|
| `graphics/gpu/paint.rs` | 24 (+764) | 18 (+77): the shapes are in `paint/shapes.rs` |
| `graphics/gpu/quads/draw.rs` | 11 (+566) | 6 (+31): the fork's tests are in `draw/fork_tests.rs` |
| `graphics/gpu/quads/batch.rs` | 22 (+332) | 19 (+79): the shapes are in `quads/shapes.rs` |
| `compositor/niri.rs` | 22 (+628) | 17 (+229): `niri/{ipc,listen,events}.rs` |
| `graphics/path.rs` | 14 (+401) | 12 (+54): `path/trim.rs` |
| `wayland/dropped.rs` | 38 (+388) | 34 (+161): `dropped/drag.rs` |
| `wayland/keyboard.rs` | 20 (+165) | 20 (+103): `keyboard/target.rs` |
| `widgets/text_input/field.rs` | 26 (+782) | 26 (+782), fork-owned |
| `wayland/normal.rs` | 30 (+243) | 30 (+243), fork-owned |
| `graphics/font.rs` | 39 (+86) | 39 (+86), fork-owned |

The moves are commits at the tip, so they cut the conflicts of future fork work, not those of the
commits before them: a rebase still replays the earlier commits that wrote the code in the
upstream file. List the files again with:

```sh
git diff --stat fork-base HEAD -- src Cargo.lock | sort -t'|' -k2 -rn | head -20
```

### Fork-owned files

These upstream files are now mostly the fork's: `src/widgets/text_input/field.rs` (selection,
undo, chords, secrets, multiline, the input method), `src/graphics/font.rs` (fontique and parley
in place of the fontconfig lookup), `src/wayland/normal.rs` (app windows) and
`src/widgets/scroll_area/offsets.rs` (coasting, the end of the content). On a conflict there, take
the fork's side, then port what upstream meant by hand. A security fix needs the most care:
upstream's `f6a96b8` clears a password field after submit, which the fork does in
`text_input/targets.rs` by wiping the field (`fields::take`), with tests.

### Upstream's single submit

Upstream submits each frame's GPU work once (`e2e29c9`): passes write into the frame's encoder
(`take_encoder`/`keep_encoder` in `graphics/gpu/encoder.rs`) and `submit()` mails it before
vello and at present. Fork code that draws must do the same, or it runs ahead of the frame's
earlier work. Two places need more: `Quads::letter` submits the frame so far before the glyph
atlas starts over, since a queue's texture writes land ahead of any encoder submitted after them;
and the fork's GPU tests draw through a helper that submits.

## Lints

The lints in `Cargo.toml` (`[lints.rust]`, `[lints.clippy]`) are ones the whole crate already
passes, upstream's code included. A lint upstream's code fails is allowed there for the crate,
with the reason beside it (`needless_range_loop`, for upstream's image filters), rather than with
an attribute in an upstream file, which a rebase would have to carry.

The fork's own files hold themselves to more: each module file the fork added starts with

```rust
#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo)]
```

and its submodules inherit it. Give a new fork module the same line. `clippy.toml` allows
`unwrap` and `dbg!` in tests. CI (`.github/workflows/fork.yml`) runs clippy with `-D warnings`.

## Commit map

| Feature | Commits | Files |
|---|---|---|
| xdg-popups | `a22c212`, `64012ca` | `src/popup.rs`, `src/wayland/popup.rs`; adds `bitflags` |
| `Windows` service + compositor actions | `f9984b0`, `e9ab40f` | `src/services/{windows,window_info}.rs`, `src/compositor/` |
| `Gestures`, `Overview`, `WorkspaceSwitch`, `Desktop`, frozen windows | `f9984b0`, `ee75390`, `fd8bc60` | `src/services/{gestures,overview,workspace_switch,desktop}.rs`, `src/layer_window.rs`, `src/wayland/redraw.rs` |
| Battery via UPower, `PowerProfile` | `615618f` | `src/services/battery/`, `src/services/power_profile.rs` |
| `Idle` | `ad29879` | `src/services/idle.rs`, `src/wayland/idle.rs`; `examples/idle.rs` |
| `Appearance` | `1425781`, `b6eb729` | `src/services/appearance.rs`; `examples/appearance.rs` |
| Audio devices (and a pulse teardown fix) | `72a1314` | `src/services/audio/{device,pulse}.rs` |
| Audio app streams, balance, input level | `4e924c9` | `src/services/audio/{streams,balance,level,pulse}.rs` |
| Network profile details, connection details, forget, hidden networks | `d4693f9` | `src/services/network/{connections,details,profile}.rs` |
| Notification timeout, hints, expire | `fb2da2c` | `src/services/notifications/` |
| Blur regions, blur shapes, `Backdrop` | `6307b38`, `8a8f66e`, `ab1afbd`, `7cee525` | `src/blur.rs`, `src/services/backdrop.rs`, `src/wayland/{background_effect,blur_shapes,update}.rs`, `protocols/niri-blur-shapes-v1.xml`; `examples/blur_shapes.rs` |
| Drag release hook, drag cursor | `6307b38`, `4eed9b7` | `src/input/pointer.rs`, `src/widgets/rectangle/targets.rs` |
| Vibrancy | `fbc1c55` | `src/graphics/vibrancy.rs`, `src/wayland/vibrancy.rs`, `protocols/niri-vibrancy-v1.xml` |
| Paths: `Path::svg`, `place`, `trim`, joins | `e0e7fd0`, `5557957` | `src/widgets/shape/path.rs`, `src/graphics/{path,join}.rs`, `src/graphics/path/`; `examples/stroke_clock.rs` |
| 64 shader value rows | `7fb3382` | `src/graphics/renderer/effect.rs`, `src/graphics/gpu/shader.rs` |
| Shader effects (backdrop and image inputs), shader hot reload and errors | `98edbf6` | `src/style/effect.rs`, `src/graphics/{shader_files.rs,gpu/effect.rs,gpu/shade.rs,gpu/shader.rs}`; `examples/effect.rs` |
| Parley text | `69f8ced` | `src/widgets/text.rs`, `src/widgets/text/{style,layout}.rs`, `src/graphics/font.rs`; `examples/text.rs` |
| Spring easing, retargeting, `retime` | `7aa44a5`, `ab037d2`, `af00d63` | `src/animation.rs` |
| Scroll source, modifiers, Ctrl chords | `0077bd8`, `8a68453` | `src/input.rs`, `src/input/modifiers.rs`, `src/wayland/{pointer,keyboard}.rs`, `src/wayland/keyboard/` |
| `on_track` | `ad1714a` | `src/widgets/rectangle/targets.rs`, `src/input/pointer.rs` |
| Apps: categories, keywords, actions, id, MIME, `find_icon` | `fc0d711`, `87bfbee`, `e9ab40f`, `304b0bc` | `src/services/apps/` |
| App windows, keyboard, clipboard, text input, single instance, activation, platform helpers | `7ce88d6`, `82a63d3`, `8352a62`, `89b50b4`, `8a3cc63`, `f451b56`, `1353c98`, `ee22b7d` | `src/window.rs`, `src/wayland/{normal,clipboard,text_input,activation}.rs`, `src/{clipboard,config,portal,notify}.rs`, `src/widgets/text_input/`; `examples/{app_window,text_editor}.rs` |
| Render and redraw work | `56acc25`, `00c7dc0`, `36e5e98`, `3fd32c8`, `94d8f59`, `2838c10` | `src/graphics/gpu/{present,bounds}.rs`, `src/graphics/renderer.rs`, `src/changes.rs`, `src/services/{network,write,windows}.rs` |
| Glyph atlas filling mid-frame | `ac05dbf` | `src/graphics/gpu/{atlas,paint}.rs`, `src/graphics/gpu/quads/batch.rs`, `src/wayland/{redraw,update}.rs` |
| vello's scratch buffers sized for a shell (the vello fork), `vello-stats`, `AMANE_VELLO_SCRATCH` | `073320f`, `70b1dce` | `Cargo.toml` (`[patch.crates-io]`), `flake.nix`, `src/graphics/gpu/{flush,shared}.rs`; [memory](internals/memory.md) |
| Hidden windows release their GPU memory, spares age out, Fifo presents | `b116a86` | `src/graphics/gpu.rs`, `src/graphics/gpu/{pool,present,target,atlas,quads}.rs`, `src/wayland/update.rs`; [memory](internals/memory.md) |
| `vello N` in the frame log; an empty vibrant layer unmaps and releases; a hide waits for the gpu; wgpu counts under `vello-stats` | `db0a245`, `e0b6596`, `f8edd68` | `src/timing.rs`, `src/wayland/{redraw,vibrancy}.rs`, `src/graphics/gpu.rs`, `src/graphics/gpu/present.rs` |
| vello's scratch pool let go when idle (`AMANE_VELLO_IDLE`) | `d3ac921` | `Cargo.toml` (`[patch.crates-io]`), `flake.nix`, `src/wayland/idle_gpu.rs`, `src/graphics/gpu/shared.rs`; [memory](internals/memory.md) |
| Drop shadows drawn by the quads (`AMANE_VELLO_SHADOWS`) | `73fe03b` | `src/graphics/gpu/quads.wgsl`, `src/graphics/gpu/quads/{batch,draw,shapes}.rs`, `src/graphics/gpu/paint/`; [memory](internals/memory.md) |
| Arcs, lines and areas under a line drawn by the quads (`AMANE_VELLO_SHAPES`); paths know when they are one arc | `c4837d9` | `src/graphics/path.rs`, `src/graphics/gpu/quads.wgsl`, `src/graphics/gpu/quads/{batch,draw,shapes}.rs`, `src/graphics/gpu/paint/`; [memory](internals/memory.md) |
| Quads: small filled outlines, two-stop gradients | `e71d32c` | `src/graphics/gpu/quads.wgsl`, `src/graphics/gpu/quads/{batch,draw,pipeline,shapes}.rs`, `src/graphics/gpu/paint/` |
| Fork code out of hot upstream files (no behaviour change) | `03135a2`, `f0d0d29`, `12d3296`, `c91f77b`, `5557957`, `6792811`, `1d61b83` | the child modules in the table above |
| Shadow spread, a pausable shader clock, frame stats; shader reloads follow links | `1fca43d` | `src/style/shadow.rs`, `src/graphics/shader_files.rs`, `src/graphics/gpu/{shade,present}.rs` |
| Color emoji: color fonts' letters drawn as pictures | `1e0a9f1` | `src/graphics/{image,letter}.rs`, `src/graphics/renderer/text.rs` |
| ScrollArea coasting after a flick, eased wheel steps; scrolled to the end, kept at the bottom | `028ae40`, `96bdb3d`, `4965401` | `src/widgets/scroll_area.rs`, `src/widgets/scroll_area/{offsets,targets}.rs` |
| Layer windows: `Zone::Thickness`; settings go with the frame's commit and clicks survive a redraw; an app's window comes back after its monitor goes; windows put on a monitor, popups under the pointer's monitor | `aac4eca`, `c85d3ba`, `bc6eb7f`, `dee8bff` | `src/layer_window.rs`, `src/monitor.rs`, `src/wayland/{layer,output,windows,redraw,update,compositor}.rs` |
| Drops read off the event loop | `05b3cef`, `6792811` | `src/wayland/{dropped,wake}.rs`, `src/wayland/dropped/` |
| D-Bus: errors with their reason, one sender's signals, owner changes, error replies; lists of variants; a signal for one program | `384d58f`, `96bdb3d`, `2996208` | `src/dbus/{bus,convert,error,method,signal,value}.rs` |
| Battery on AC from the adapter | `f938dc4` | `src/services/battery.rs` |
| Bluetooth: BlueZ's signals, operations and failures; the pairing agent | `7084d91` | `src/services/bluetooth/{actions,adapter,agent,errors,model}.rs` |
| Tray: StatusNotifierItem watcher and host, dbusmenu menus | `164a247` | `src/services/tray/`, `src/dbus/signal.rs` |
| Polkit: the session's authentication agent, behind `polkit-agent` | `10a70d9`, `7bf33ea` | `src/services/polkit.rs`, `src/services/polkit/agent.rs` |
| Video frames | `2fe33fa` | `src/video.rs`, `src/widgets/video.rs`, `src/graphics/gpu/quads/video.rs`; adds `ffmpeg-next` |
| A multiline input that sends with Enter, `Image::size` | `4965401` | `src/widgets/text_input.rs`, `src/widgets/text_input/targets.rs`, `src/style/image.rs` |
| Passwords that leave no copies; cleared after submit | `e63caaa` | `src/widgets/text_input/`, `src/clipboard.rs`, `src/wayland/clipboard.rs`, `src/config.rs`; adds `zeroize` |
| Input method, secure fill | `cf8949f` | `src/wayland/{input_method_v2,secure_fill}.rs`, `src/services/focused_field.rs`, `protocols/niri-secure-fill-v1.xml`; `examples/input_method.rs` |
| MimeApps: default apps the way xdg-open picks them | `9b2cf10` | `src/services/apps/{defaults,mime}.rs` |
| `WindowInfo::fullscreen` and `::workspace` | `c8c15ce` | `src/services/window_info.rs`, `src/compositor/niri.rs` |
| Brightness: SDR white in place of the backlight while the panel is in HDR | `450bf0c` | `src/services/brightness.rs`, `src/compositor/niri.rs` |
| Timers, subscriptions, services that stop while unwatched | `b6ef61e` | `src/timer.rs`, `src/services/{subscribe,store,write}.rs`, `src/services.rs`, `src/wayland/windows.rs`; [timer](system/timer.md), [subscriptions](system/subscriptions.md) |
| One niri event stream, `amane::niri`, `Features`, `KeyboardLayouts`; every output change, config reads | `8f40ae1`, `3b26f5e`, `c91f77b` | `src/compositor/niri/`, `src/compositor/niri.rs`, `src/services/keyboard_layouts.rs` |
| `Launch`: no panics, each program in its own systemd scope; `activated()` waits for the compositor's token | `1e4d0ec`, `742e771` | `src/launch.rs`, `src/process.rs`, `src/wayland/activation.rs` |
| uevents, `Rfkill`, `Logind`, `ClipboardWatcher`, `LockKeys`, `Casts`, NetworkManager signals and profiles, fewer rescans | `efae128`, `bc6d503`, `2a8d7d2`, `247460d` | `src/services/{uevent,rfkill,logind,clipboard_watcher,lock_keys,casts,outputs,media}.rs`, `src/services/network/`; [rfkill](system/rfkill.md), [logind](system/logind.md), [clipboard-watcher](system/clipboard-watcher.md), [network](system/network.md), [lock-keys](system/lock-keys.md) |
| `ServiceError`, and the helpers a program's own services share | `b463864`, `224ba58` | `src/services/error.rs`, the fork's services; [errors](system/errors.md) |
| Notifications kept across a shell's restarts, 200 at most | `8b20755`, `4ef7616` | `src/services/notifications.rs`, `src/services/notifications/kept.rs`; [notifications](system/notifications.md) |
| Idle RAM: a window's `Gpu` made at its first frame and all gone when none shows; a source no window reads doesn't wake the loop; each niri listener gets only its events; the allocator trims after a hide or an image let go; `blocking` pinned at 1.6.2 so zbus's idle threads leave | `509d6ee`, `a3fde99`, `b5bed62`, `4a9d1fd`, `5117639` | `src/graphics/gpu/{window_gpu,shared}.rs`, `src/changes.rs`, `src/compositor/niri/{client,listen,stream}.rs`, `src/allocator.rs`, `Cargo.toml`; [memory](internals/memory.md) |
| Images decoded as big as they're drawn, under a byte cap (`AMANE_IMAGE_CACHE_MB`, `AMANE_IMAGE_FULL`), on two threads | `4273783` | `src/graphics/image.rs`, `src/graphics/renderer.rs`, `src/style/image.rs` |
| `Bus::signals_at`: one object's signals alone | `0551b16` | `src/dbus/signal.rs`, the services that watch one object |
| CI, lints and formatting, the terms check | `6ac64aa`, `0439e96` | `.github/workflows/fork.yml`, `Cargo.toml` (`[lints]`), `clippy.toml`, the fork's files |
| Fork docs and housekeeping | `c187555`, `2a02010`, `9da2da0`, `85b7605`, `50eae0b`, and this page | `FORK.md`, `docs/fork/` |

KDE Connect, phone messages, adb and the status-item client were written here and moved to
emaki (`emaki-phone`, `emaki-status`) in its Phase 9. The tidy took them out of every commit
rather than keeping them and their removal; the generic pieces they brought (D-Bus conversions,
`Bus::emit_to`, the `ScrollArea` end, a multiline input that sends with Enter, video frames) stayed.

## Reviewing the fork

```sh
git log --oneline --stat fork-base..emaki     # every fork commit and the files it touched
git diff --stat fork-base..emaki              # the whole delta, by file
git diff fork-base..emaki -- <paths>          # one feature (paths in the commit map)
```

## Consumed by

The [`emaki`](https://github.com/emaki-desktop/emaki) bar, as a git dependency pinned to a
`rev` on this branch. For local co-development the shell carries a git-ignored
`.cargo/local.toml` with `paths = ["../amane"]`, used as `cargo --config .cargo/local.toml …`
(`cargo clippy --config …` for clippy, which doesn't see a `--config` given before it).

The upstream `amane` *CLI* uses its own bundled library copy in `~/.cache/amane/library`, not
this fork.

## Rebasing onto newer upstream

The same routine as the niri fork's:

1. **Tag the new base.** `git fetch upstream`, then `git tag fork-base-<date> upstream/main`.
   Keep the branch as it was: `git branch -f ocbk-pre-tidy emaki` and push it, so revisions
   pinned before the rebase stay fetchable.
2. **Make a reference merge.** On a scratch branch, `git merge upstream/main` into `emaki` and
   resolve it once. Build it, run the tests, and build emaki against it. Its tree is what the
   rebase must end with.
3. **Rebase.** `git rebase --onto fork-base-<date> fork-base emaki`. Resolve each conflict toward
   the reference merge; on a fork-owned file, take the fork's side and port upstream's intent.
   Afterwards, `git diff <reference> emaki` must be empty, or show only what was decided on purpose.
4. **Check every commit.** `git rebase -x 'cargo check --all-targets' fork-base-<date>`, putting
   `Cargo.lock` back after each check (cargo rewrites it), then fmt, clippy with and without
   `polkit-agent`, `cargo test --lib`, the GPU tests (`-- --ignored`) and `nix build` at the tip.
5. **Compare.** `git range-diff <old fork-base>..ocbk-pre-tidy fork-base-<date>..emaki`: what changed
   in each commit beyond the conflicts.
6. **Move `fork-base`** to the new base, check that `docs/src` and `ARCHITECTURE.md` still match
   it, fast-forward the fork's `main`, force-push `emaki` with a lease, and update this page's
   commit map (the hashes change).
7. **Bump emaki**: the `rev`, and the `outputHashes` entry in its `flake.nix`.
8. **A live session** with the shell on the new revision: text fields and password fill, popups,
   gestures, glass.

## Keeping the fork light

- Put new features in their own module; a feature in an upstream file goes in a child module
  (`foo.rs` declares `mod bar;` for `foo/bar.rs`), leaving a `mod` line and a call behind.
- Keep edits to existing files surgical and additive. Keep upstream's API too: `spawn(&str)`
  stays, as upstream has it, while new code uses `Launch`.
- Document each feature under `docs/fork/<area>/`, starting with **Usage**, then
  **Design / internals**, and add a row to the [index](README.md). Never edit `docs/src/` or
  `ARCHITECTURE.md`.
- Implemented specs move to [`design/`](README.md#design-history) with a "Status: implemented"
  header.
- Each feature is one commit, or a few that sit together; fold its docs into it.
