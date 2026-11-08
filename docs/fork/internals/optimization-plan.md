# amane `emaki`: resource-optimization plan

## Context
`emaki-desktop/amane` (branch `emaki`, a fork of the MystiaFin Wayland shell library, ~21k LOC Rust, vello 0.11 + wgpu 30) runs the `emaki` bar. The goal is to keep every feature but cut CPU wake-ups, RAM, VRAM and binary size as far as possible.

The overall design is already sound. It only redraws when something changed, it requests no frame callbacks while idle, it shares one wgpu device, and glyph caches are bounded. Most of the waste is in four places:
1. **Polling:** background services poll on timers (~470 wake-ups/min when idle).
2. **Popup views:** they run on every event-loop tick.
3. **Per-frame work:** text and layout are measured and allocated again several times per frame.
4. **Window-sized GPU textures:** they are kept per window and never trimmed.

The work is split into phases ordered by impact ÷ risk. Each phase is a separate commit or PR so it can be measured and reverted on its own.

> Line numbers refer to `emaki` at `fe941a9` and may drift.

## Status

Partly implemented. Done from this plan:

| Item | Commit |
|---|---|
| 1.5 Battery: UPower `PropertiesChanged` instead of the 5 s poll (sysfs poll stays as the fallback) | `dd6f110` |
| 2.7 Skip identical frames: the GPU pass is skipped when the commands, size and motion equal the frame on screen | `e56b843` |
| 2.8 Pointer motion (partly): `on_track` follows the pointer without a redraw | `6b0099b` |

Related render and redraw work done since, outside the phases above:

| Change | Commit |
|---|---|
| A new `Animation` is at rest, so making one draws nothing | `527c538` |
| `Renderer::blend` makes no group for what's invisible or a single shape | `3f57058` |
| A vello run draws into its own box, not the whole window (GPU bounds) | `6d2123b` |
| A backdrop reading redraws only the windows that read that shape's | `e71c91e` |
| A window title change redraws only what shows a title | `45e0edc` |
| `Overview`, `WorkspaceSwitch` and `Desktop` split out of `Gestures`, and frozen layer windows | `14c6994`, `916aecd` |
| vello's scratch buffers sized for a shell, through a fork of vello (−130 MB of VRAM per process at the first vello run; see [memory](memory.md)) | see memory.md |

The rest is still planned: event-driven network, media, bluetooth, brightness and palette;
pausing unread services; the allocator thread; popup reconcile; text and layout caches; GPU
state sharing, present mode and power preference; caches and fonts; the release profile. How
the done items work is in [Architecture](architecture.md).

---

## Phase 0: Baseline measurement (do first)
- Build a small bar from `examples/` plus the real `emaki` against a path dep.
- Record the following, both idle (5 min) and with mouse/animation activity:
  - CPU wake-ups: `perf stat -e sched:sched_wakeup`, `powertop`, or `pidstat -w`.
  - RSS/PSS: `smem`.
  - VRAM: `nvtop` / `radeontop` / `intel_gpu_top`, or `amdgpu` debugfs.
  - Thread count: `ls /proc/PID/task | wc -l`.
  - Stripped binary size.
- `AMANE_TIMING` (`src/timing.rs`) already reports frame times; use it before and after each phase.

## Phase 1: Idle wake-ups and background services (largest idle win)
1. **Network: make it event-driven.**
   - Today `src/services/network.rs:48-60,163-192` runs a full `fetch()` every 1 s, which is 10 to 100 system-bus calls.
   - Change: subscribe to NetworkManager `PropertiesChanged`, `DeviceAdded/Removed` and `AccessPointAdded/Removed` using the existing signal plumbing in `src/dbus/signal.rs` (the same pattern Notifications/Audio use). Override `listen()`.
   - Cache the profiles and the AP list, and refetch only the object that a signal named. Keep a slow fallback poll (e.g. 30 s) for safety.
2. **Media: make it event-driven.**
   - `src/services/media.rs`, `media/player.rs`.
   - Change: use `NameOwnerChanged` for players appearing or vanishing, and `PropertiesChanged` on `org.mpris.MediaPlayer2.Player`. Fetch `Identity` once per player.
   - Keep the `Position` interpolation local: extrapolate from `Rate` and the last timestamp instead of polling.
3. **Bluetooth: make it event-driven.**
   - `src/services/bluetooth.rs:50`.
   - Change: use `InterfacesAdded/Removed` and `PropertiesChanged` from the BlueZ ObjectManager instead of `GetManagedObjects` every 2 s.
   - Route adapter and device calls through the shared `src/services/worker.rs` instead of spawning a thread per call (`bluetooth.rs:157,175`).
4. **Do D-Bus calls outside the write lock.**
   - The default `Service::listen` (`src/services.rs:68-77`) holds `Self::write()` across `update()`.
   - Change: add a `fetch()` → `apply()` split (or a `poll() -> Option<Snapshot>` default method), so the I/O runs unlocked and only the swap takes the lock. This removes render-thread stalls on slow players.
5. **Brightness and battery: replace polling with event sources.**
   - Brightness (500 ms, `brightness.rs:44`): replace with inotify (`IN_MODIFY` on `brightness` / `actual_brightness`) using the existing `src/files.rs` inotify helper. Read `max_brightness` once.
   - Battery (5 s): listen to UPower `PropertiesChanged` when UPower is present. Otherwise fall back to a 30 s poll and react to AC `online` changes (inotify does not fire on sysfs `capacity`, so a poll fallback stays).
   - Palette (500 ms stat): use inotify on the wallpaper path.
6. **Cpu and memory: keep the poll, make each tick cheap.**
   - `cpu.rs:64`, `memory.rs:39`: keep the file open, `pread` into a reused buffer, and parse only the first line or the needed keys.
7. **Pause services nobody reads.**
   - `src/services/store.rs` already knows which windows read which services (`reads` sets in `src/wayland/windows.rs:124-138`).
   - Change: add a reader refcount or "last read" epoch. Polled services skip `update()` (long sleep or park) while no live window has read them for N seconds, and also while the session is locked and only the lock surface exists. Unpark when read again.
8. **Stop blocking the first read.**
   - `store.rs:13-45` runs `S::new()` on the render thread.
   - Change: construct with an empty or default state, then let the service thread do the first fetch and `write()`, so it wakes the window as usual.
9. **Allocator trim.**
   - `src/allocator.rs:14,47-53` runs a 10 s `malloc_trim` thread forever.
   - Change: remove the thread. Call `malloc_trim(0)` after a window/popup closes, after an image cache eviction, and after a large decode finishes, rate-limited to once every 10 s.
10. **Apps.**
    - `apps.rs:34-50` rescans every 30 s. Replace that with inotify on each `applications/` dir.
    - Sort with cached lowercase keys (`sort_by_cached_key`).
    - Build the icon index lazily per requested name with a small memo (`icons.rs:14-19`) instead of indexing the whole theme. Add a symlink loop guard in `walk.rs:25`.
11. **Compositor IPC.**
    - niri (`compositor/niri.rs:99-220`): one shared `EventStream` thread fans out to Workspaces, Windows and Gestures, so each line is parsed once.
    - Skip `count_windows`, the clone and the sort on gesture-only events.
    - Sort once (`niri.rs:143-145` vs `windows.rs:23`).
    - Hyprland (`hyprland.rs:148-157`): refetch only on workspace/monitor events, not on title/focus events.
    - Sway (`sway.rs:101-114`): handle `window` title events without `GET_TREE`.
12. **PulseAudio.** Share one connection per thread role. Don't open a connection on the render thread in `Audio::new()` (`pulse.rs:50-53,129-138`).
13. **Notification images.**
    - Today `src/dbus/convert.rs:12,28` expands each byte to a `Value`.
    - Change: add a byte-array fast path (`Value::Bytes(Vec<u8>)`) and convert the image hint directly.
    - Keep RGBA in memory and hand it to the image cache, instead of a PNG encode, a write to `XDG_RUNTIME_DIR`, then a decode (`image_hint.rs:76-94`).

## Phase 2: Main thread / per-frame CPU
1. **Popup reconcile on every tick.**
   - Today `src/wayland.rs:236` → `src/wayland/popup.rs:29-70` runs every popup view after every dispatch.
   - Change: record each popup view's service `reads`, the same way `run_view` does. Reconcile only when one of those services changed (hook into `windows.rs:124-138`) or an input handler fired.
   - Drop the `popup_views.clone()` by iterating indices or using `Rc<[..]>`.
   - Replace the `AMANE_DEBUG` `env::var_os` calls with a `LazyLock<bool>`, like `timing::ENABLED`.
2. **Text font lookup.**
   - `graphics/font.rs:44-66`, `widgets/text.rs:94-96`: resolve the `FontRef` once when the `Text` is built or styled and store it.
   - Make the `LOADED` key `(family, Weight)` without `format!`.
3. **Text measurement cache.** Add a `OnceCell<(f32, f32)>` for the measured size and line breaks on `Text` (`text.rs:100-127`, `text/lines.rs:14-22`). Content is immutable after build.
4. **Layout placement.**
   - `widgets/layout.rs:106-175`: cache `place(area)` results in a `RefCell<Option<(Area, Vec<Area>)>>` so `draw` and `collect_targets` share them.
   - Use `with_capacity`.
5. **`lines::arrange`.** Return borrowed `&str` slices or `Cow`, not `vec![String::from(content)]` (`text/lines.rs:50-54`).
6. **Reuse per-frame buffers.**
   - Keep the command `Vec`, the targets `Vec` and the `reads` `HashSet` on `OpenWindow` and Gpu, and `clear()` them instead of reallocating (`frame.rs:64,73`, `renderer.rs:29`, `changes.rs:82-83`, `redraw.rs:24`).
7. **Skip identical frames.** Before `present`, compare the new command list to last frame's (cheap hash or `PartialEq`). If equal and the size is unchanged, skip the GPU pass and the commit (`gpu/present.rs:9`).
8. **Pointer motion.**
   - `input/pointer.rs:107-123`: redraw on motion only if the handler wrote a service (a `Write` mark) or hover state changed. Today any `on_move` under the cursor forces a full frame.
   - Use a scratch Vec in `move_to` and do a single `find_topmost` per event.
   - **This is an API-visible change.** Keep the old behavior behind a flag if handlers rely on it.

## Phase 3: GPU / VRAM
1. **Present mode and frame latency.**
   - `gpu/target.rs:88-101`: use `Fifo` + `desired_maximum_frame_latency: 1` by default, which saves one swapchain image per surface. Frame callbacks already pace drawing.
   - Verify that hidden-window Fifo blocking (a Mesa issue) doesn't occur. Hidden windows already get no callbacks (`update.rs:92-104`), so we never present while hidden.
   - Add an env override.
2. **Power preference.** Set `power_preference: PowerPreference::LowPower` in `gpu/shared.rs:66-69`, so hybrid laptops stay on the iGPU.
3. **Spare texture pool.** Cap `gpu/pool.rs:32-44`, e.g. keep at most 2 spares beyond last frame's peak, and drop extras after N idle frames.
4. **Blur.** Take blur textures from the pool and return them, instead of 2 new textures per call (`blur.rs:50-51`). Cache the blurred result while its input region's commands are unchanged.
5. **Kept vello results** (`painted.rs:29-52`). Limit them to segments that are actually reused, and release them after a frame where a segment wasn't reused. Avoid the `Scene` clone by keeping a hash of the encoding.
6. **Share GPU state across windows.**
   - Move into the `Shared` thread-local in `gpu/shared.rs`: pass shader modules and pipelines (`pass.rs:37`, `cut::erase`), the quads pipelines and sampler (`quads/pipeline.rs:20-96`), custom shader cache (`gpu.rs:87`), and the **picture texture cache** (`quads.rs:47`, `picture.rs`).
   - This means each image (e.g. a wallpaper on 3 monitors) is uploaded once.
   - Keep the per-window glyph atlas, or share it too.
7. **Per-draw allocations.** Use a persistent, growable uniform and storage buffer with `queue.write_buffer`, and cache bind groups (`pass.rs:84-103`, `quads/draw.rs:46-82`). Batch the per-frame `queue.submit` calls into one encoder submit where ordering allows.
8. **Vello atlas reupload** (`gpu/image.rs:31-53`). Track `atlas_drops` per window/renderer generation rather than globally, so one window's image-free scene doesn't force another window to reupload everything.

## Phase 4: RAM caches and fonts
1. **Fonts.**
   - Use `mmap` instead of `fs::read` + `Vec::leak` (`font.rs:87-93`). Add `memmap2`, or a small libc `mmap`, since libc is already linked.
   - Key the cache by `(path, index)` for both the weighted and fallback paths (`font.rs:54,288`), so a `.ttc` is mapped once for all its faces.
   - Make `Fontconfig` one `LazyLock` instance.
2. **Image cache** (`graphics/image.rs:62-129`).
   - Evict shrunk/blurred copies and failed `None` entries when no window shows them (same rule as full-size entries), with a small LRU budget.
   - Decode on a bounded pool (reuse `worker.rs` or 1–2 threads) instead of one thread per decode (`image.rs:85`).
3. **Palette.** Decode at reduced size: JPEG via zune-jpeg's scaled decoding, and PNG with early downsample while streaming rows. Do the decode outside the write lock (`palette.rs:89-97`).

## Phase 5: Binary size and build
1. Add a release profile to the workspace root `Cargo.toml`:
   ```toml
   [profile.release]
   lto = "fat"
   codegen-units = 1
   strip = true
   opt-level = 3   # try "s" and measure; keep 3 if frame time regresses
   ```
   Keep `panic = "unwind"`, because `app.rs:113` relies on unwinding.
2. Gate the GLSL user-shader reader behind a default-on cargo feature `glsl-shaders`, so `wgpu/glsl`, naga glsl-in and pp-rs become optional (`gpu/shader.rs:12-20`).
3. Trim the zbus features (`default-features = false, features = ["async-io"]`, plus what `blocking` needs). Check the result with `cargo tree -e features`.
4. Investigate vello `default-features = false` with an explicit wgpu backend list (Vulkan + GLES; GL is needed by `shared.rs:37-39`). Drop `renderdoc-sys` if possible. Do this only if it's measurable.

---

## Critical files
- **Services:** `src/services.rs`, `src/services/store.rs`, `network*.rs`, `media*.rs`, `bluetooth*.rs`, `brightness.rs`, `battery.rs`, `cpu.rs`, `memory.rs`, `apps/*`, `palette*`, `notifications/image_hint.rs`, `audio/pulse.rs`, `src/dbus/convert.rs`, `src/allocator.rs`.
- **Compositor:** `src/compositor/{niri,hyprland,sway}.rs`.
- **Main loop:** `src/wayland.rs`, `src/wayland/{popup,redraw,windows,update}.rs`, `src/frame.rs`, `src/changes.rs`, `src/input/pointer.rs`.
- **Widgets:** `src/widgets/{text.rs,text/lines.rs,layout.rs}`, `src/graphics/font.rs`, `src/graphics/renderer.rs`.
- **GPU:** `src/graphics/gpu/{shared,target,pool,blur,painted,pass,image,picture,present}.rs`, `gpu/quads/*`.
- **Build:** `Cargo.toml`.

## Reuse
- Signal plumbing: `src/dbus/signal.rs` / `bus.rs` (one connection per bus already).
- inotify helper: `src/files.rs`.
- Shared worker thread: `src/services/worker.rs`.
- Change/read tracking: `src/changes.rs` + `windows.rs:124-138`.
- Quiet polls: `src/services/write.rs` (`quiet()`).
- `LazyLock` env-flag pattern: `src/timing.rs:6`.

## Decisions
- **Scope:** all phases 1–5, one small commit per item, upstream-able.
- **Behavior changes:** these become the defaults, each with an opt-out env var listed in `README`/`FORK.md`:
  - motion redraw only on change
  - Fifo present with frame latency 1
  - pausing unread services
- **Opt-out vars:** `AMANE_REDRAW_ON_MOVE`, `AMANE_PRESENT=mailbox`, `AMANE_ALWAYS_POLL`. Each is read once via `LazyLock`.

## Fork hygiene
`FORK.md` asks for a thin, rebasable series. Almost everything above edits upstream files. Prefer one commit per item. Phases 1–5 are upstream-worthy, so offer them to MystiaFin/amane after the hiatus ends (mid-Dec 2026) to keep the fork small. Phase 2.1 (popups) and the niri stream sharing (1.11) are fork-specific.

## Verification
- `cargo build --release`, `cargo clippy --all-targets`, `cargo test` (pam tests and others). Use the existing CI in `.github/workflows/test.yml`.
- Run `examples/apps.rs`, `examples/text_input.rs` and `emaki` on niri (with the gesture fork) and, if available, on Sway/Hyprland.
- Functional checks:
  - network connect and disconnect, plus a Wi-Fi scan list update
  - bluetooth pairing
  - MPRIS play/pause and position
  - volume
  - brightness keys
  - battery unplug
  - notification with an image
  - popup open/close and outside-click dismiss
  - gestures animate smoothly
  - lock screen
- Re-run the Phase 0 measurements after each phase. Target: idle wake-ups ≥80% down, idle CPU ≈0%, at least one swapchain image less per surface, a single upload per shared image, smaller stripped binary, no frame-time regression in `AMANE_TIMING`.
