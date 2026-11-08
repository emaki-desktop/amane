# Spec: Blur behind the bar & activation-token launching for the Amane fork

> **Status: implemented.** Part A shipped as blur regions in `db581d0`, then as blur shapes
> over the niri fork's `niri-blur-shapes-v1` in `bcc53ac` and `2ea1d46`
> ([Blur](../graphics/blur.md)). Part B shipped in `82337f2`: `src/wayland/activation.rs`,
> `spawn_activated` ([Activation](../windows/activation.md)). §A.6, amane writing its own niri
> layer rules, was not built as an amane API: amane-shell generates its layer rules instead
> ([layer rules](https://github.com/098765434/amane-shell/blob/main/docs/integration/layer-rules.md)).
> Kept as design history; the "draft" status line below is the original.
 
Status: draft · Target: `098765434/amane` (`ocbk`) + `amane-shell` · Two small library
features built on Wayland protocols niri already serves. **No niri changes are required**
for either (one optional niri nicety in §A.9). On niri, Amane can also write its own layer
rules into an included config file for smooth blur corners (§A.6).
 
| Part | Protocol | What the user sees |
|------|----------|--------------------|
| **A. Blur behind the bar** | `ext-background-effect-v1` | A translucent bar, popup or launcher with the wallpaper/windows blurred behind it, shaped to its rounded corners |
| **B. Launch with activation tokens** | `xdg-activation-v1` | Apps launched from the bar or launcher reliably take focus, on niri and every other compositor |
 
> **Conventions.** Wayland via the existing SCTK backend (`src/wayland.rs`,
> `src/wayland/*`). Optional globals degrade to a no-op, like `fractional_scale` and
> `viewporter` today. Immediate mode: the view describes what it wants each frame; the
> backend diffs against what it last sent and only talks to the compositor on change (the
> same pattern as `update_input_region` in `src/wayland/update.rs`). Additive edits, a
> `docs/<name>.md` each, thin for rebasing. Background: `amane-tray-service-spec.md` §3.
 
---
 
## Part A — Blur behind the bar (`ext-background-effect-v1`)
 
### A.1 Why
 
A translucent bar over a wallpaper looks unfinished without blur, and the only way to get it
today is a hand-written niri `layer-rule` matching Amane's namespace. That rule can only blur
the whole surface (rounded with the rule's `geometry-corner-radius`), so a floating "island"
bar — a rounded box inside a larger transparent surface — blurs the wrong area.
 
The protocol fixes that: the client says exactly which region to blur, and the compositor
blurs that region. niri's docs say blur requested through the protocol "exactly follows the
shape requested", which is what Amane needs for island bars, popups and partial shapes. The
catch, covered in §A.5, is that the shape can only be built from rectangles.
 
### A.2 What already exists
 
**niri (`v26.04`, no changes needed):**
 
- Serves `ext-background-effect-v1` (`src/handlers/background_effect.rs`) and advertises the
  `Blur` capability. It's implemented for toplevels, **layer surfaces** and popups.
- A layer surface's blur defaults to "blur when the surface requests it" (`blur: None` in the
  resolved `BackgroundEffect`), so no niri config is needed.
- Global look is user config: `blur { passes 3; offset 3; noise 0.02; saturation 1.5 }`
  (defaults). `blur { off }` disables all blur.
- **Xray** is on by default whenever blur is active: the surface shows the *blurred wallpaper*,
  ignoring windows beneath it. That's the right default for a bar with an exclusive zone,
  since only wallpaper is behind it, and it's cheap (computed once, shared). A floating bar
  over windows wants real blur, which a layer rule turns on (§A.8).
**Amane:**
 
- SCTK 0.21.1 ships `smithay_client_toolkit::background_effect::{BackgroundEffectState,
  BackgroundEffectHandler}` — bind, capability event, and `get_background_effect(surface)`.
  The bindings come from `wayland-protocols 0.32.13` (already in `Cargo.lock`; staging
  feature, enabled through SCTK).
- `LayerWindow::input_region(Vec<InputArea>)` is the exact precedent: a per-window region,
  stored in `OpenWindow`, diffed in `update_input_region`, built with
  `compositor.create_region`, scaled by `scaled_region`. Blur copies this shape.
### A.3 Public API
 
```rust
// src/layer_window.rs (and the same two methods on Popup and Window)
 
/// Blur behind the whole window, with rounded corners.
pub fn blur(self, radius: impl Into<Radius>) -> Self;
 
/// Blur behind these areas only (window-local logical px), e.g. an island bar
/// drawn inside a larger transparent surface.
pub fn blur_region(self, areas: Vec<BlurArea>) -> Self;
 
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlurArea {
    pub x: i32, pub y: i32, pub width: i32, pub height: i32,
    pub radius: Radius,        // reuses the existing per-corner Radius type
}
 
/// Whether the compositor can blur right now. Read it in a view to pick a
/// more opaque background when blur isn't available.
pub fn blur_supported() -> bool;
```
 
Internally the window holds `blur: Option<BlurSpec>` where
`BlurSpec = Whole(Radius) | Areas(Vec<BlurArea>)`. `Whole` is resolved against the window's
*current* size when applied, because a `WindowSize::Full` bar doesn't know its width until the
compositor configures it.
 
No new layout vocabulary: blur is a window property like `input_region`, not a widget
property. The shell draws the translucent background itself (a `Rectangle` with an alpha
`Fill`); blur only shows where that background is see-through.
 
### A.4 Backend
 
**Binding** (`src/wayland.rs`): add `background_effect: BackgroundEffectState` to
`WaylandState`, created with `BackgroundEffectState::new(&globals, &qh)`. It holds the global
as an optional proxy, so a missing global costs nothing. Implement `BackgroundEffectHandler`:
`update_capabilities()` stores `capabilities().contains(Capability::Blur)` in a static
`AtomicBool` read by `blur_supported()`, then marks all views dirty (`changes::mark_all`) so
views that pick a fallback background redraw once.
 
**Per surface** (`src/wayland/surface.rs`, `src/wayland/update.rs`):
 
- `OpenWindow` gains `blur: Option<BlurSpec>` (last sent) and
  `blur_effect: Option<ExtBackgroundEffectSurfaceV1>` (created lazily).
- New `update_blur(&mut self, spec: Option<&BlurSpec>)`, called next to
  `update_input_region` and again from `resize()` when the spec is `Whole`:
  1. Resolve the spec to rects at the current size (§A.5). If the result equals what was
     last sent, return (**coalesce** — a resting bar sends nothing).
  2. No global, or no `Blur` capability → return.
  3. Create the effect object on first use with `get_background_effect(surface)`.
  4. `Some(rects)` → build a `wl_region` from the rects,
     `set_blur_region(Some(&region))`, destroy the region. `None` → `set_blur_region(None)`.
  5. The request is double-buffered; it takes effect on the window's next commit, which the
     normal redraw already does.
- Create the effect object **once** per surface: a second `get_background_effect` on the
  same surface is a protocol error. On window close, destroy the effect object before the
  `wl_surface`, and never send `set_blur_region` after the surface is gone — that raises the
  protocol's `surface_destroyed` error.
- Scaling uses the existing `scaled_region` helper, so `ScaleFactor` (Amane's UI scale)
  applies exactly as it does for input regions.
### A.5 Rounded corners
 
**Why strips are needed.** Checked against the protocol XML and niri's renderer:
 
- The protocol's only shape input is `set_blur_region(wl_region)`. A `wl_region` is a union of
  integer rectangles in surface-local logical pixels. Version 1 has no radius or mask
  argument.
- niri won't round it for us. When a surface sets its own region, niri's
  `render_params_for_tile` (`src/render_helpers/background_effect.rs`) only applies the
  rounded clip if `clip_to_geometry` is true, and for layer surfaces
  (`src/layer/mapped.rs`) that argument is always `false`. The rule's
  `geometry-corner-radius` is only used when blur comes from a layer rule, not from the
  protocol.
So a rounded blur requested by the client has to be built from rectangles:
 
```
for each corner row i in 0..r (top and bottom):
    inset = ceil(r - sqrt(r² - (r - i - 0.5)²))  // round UP: never poke outside the shape
    rect  = (x + inset, y + i, w - 2·inset, 1)   // per-corner radii use their own r and inset
middle = (x, y + r_top, w, h - r_top - r_bottom)
merge consecutive rows with equal insets into one rect
```
 
That's at most `r_top + r_bottom + 1` rects before merging, usually far fewer after. Put it in
a pure function `rounded_strips(area, scale) -> Vec<Rect>` with unit tests.
 
**What it looks like.** The steps are 1 logical pixel with no antialiasing, so at output
scale 2 each step is 2 physical pixels. Rounding the inset *up* keeps the blur inside the
drawn shape; the cost is a few unblurred pixels just inside the antialiased corner edge.
Under a half-transparent fill this is normally hard to see; it can show at large radii over a
high-contrast wallpaper. Check it visually (§A.11 test 3) before deciding it's good enough.
 
**Smooth corners on niri: let niri round the blur.** For a window whose blur is the whole
surface, skip the protocol and use a niri layer rule instead. niri then clips the blur with
its rounded-corner shader, antialiased at physical precision:
 
```kdl
layer-rule {
    match namespace="^amane-bar$"
    geometry-corner-radius 12
    background-effect { blur true }
}
```
 
This only works when the shell does **not** call `.blur()` on that window: once the surface
sets a protocol region, niri drops the rounded clip. It can't do island or partial shapes.
Written by hand, it would also put the radius in two places — the shell's code and the
user's niri config. §A.6 removes that problem by having the shell write the rule itself.
 
**Which to use:**
 
| Window | niri, include line present (§A.6) | Any other case |
|--------|------------------------------------|----------------|
| Full-width bar, launcher, any whole-surface blur | Shell-owned niri rule: smooth corners | `.blur(radius)`: protocol strips |
| Island bar, partial shapes | `.blur_region(...)`: protocol strips | `.blur_region(...)`: protocol strips |
| Popups | Rule's `popups { }` block, if one radius fits all popups; otherwise `.blur()` | `.blur(radius)` |
 
### A.6 Shell-owned niri rules through `include`
 
niri's config can include other files (`include "file.kdl"`, since 25.11, so in the fork's
`v26.04` base). Since 26.04, `include optional=true "file.kdl"` doesn't fail when the file is
missing. Included files are watched: niri's watcher (`src/utils/watcher.rs`) polls each
file's mtime, so the config reloads by itself when the file changes, and an optional file is
picked up once it appears. Rules from an include are inserted where the `include` line sits,
and anything after it in the main config overrides it.
 
So Amane can **own one small KDL file** and write its rules there, from the same values the
shell draws with. The radius is then defined once, in the shell's code.
 
**One-time user setup** — one line near the end of `~/.config/niri/config.kdl`:
 
```kdl
include optional=true "~/.config/niri/amane.kdl"
```
 
`optional=true` means niri still starts if Amane has never run, and removing Amane leaves
nothing broken. Putting the line near the end lets Amane's rules override earlier defaults,
while anything the user writes after it still wins.
 
**Library mechanism** (`src/compositor/niri_config.rs`, new; niri backend only). The library
writes the file safely; the shell decides what goes in it.
 
```rust
/// A niri layer rule for one of the shell's own namespaces.
pub struct NiriLayerRule { /* namespace, radius, blur, xray, popups */ }
 
impl NiriLayerRule {
    pub fn namespace(ns: &str) -> Self;           // emits match namespace="^<escaped>$"
    pub fn corner_radius(self, r: impl Into<Radius>) -> Self;
    pub fn blur(self, on: bool) -> Self;
    pub fn xray(self, on: bool) -> Self;
    pub fn popups(self, radius: impl Into<Radius>, blur: bool) -> Self;
}
 
pub enum FragmentStatus {
    Written,                 // file changed; niri will reload
    Unchanged,               // identical content already there; nothing written
    NotIncluded,             // written, but the user's config has no include line for it
    Invalid(String),         // niri rejected it; nothing installed
    NotNiri,                 // not running under niri; nothing written
}
 
/// Write the shell's niri rules to ~/.config/niri/<file>. Call once at startup
/// (and again if the shell's own settings change) — never from a view.
pub fn write_niri_rules(file: &str, rules: &[NiriLayerRule]) -> FragmentStatus;
```
 
Rules for `write_niri_rules`:
 
1. **Not niri** (`NIRI_SOCKET` unset) → `NotNiri`.
2. Generate the KDL with a header comment saying the file is generated by Amane and will be
   overwritten. Escape the namespace as a regex and a KDL string. The builder is there so
   shells never hand-format KDL.
3. **Unchanged content → write nothing.** This matters: every write triggers a full niri
   config reload, so a shell that rewrites on every start would reload niri every login.
4. **Validate before installing:** write to a temp file next to the target and run
   `niri validate -c <temp>`. A fragment containing only layer rules is a complete, valid
   config on its own, so this checks it fully. On failure, delete the temp file and return
   `Invalid` — an invalid include would make niri reject every reload until it's fixed.
5. **Install atomically:** fsync, then rename over the target. The rename changes the mtime,
   which niri's poller sees.
6. **Check the include line:** read the main config (`$NIRI_CONFIG`, else
   `$XDG_CONFIG_HOME/niri/config.kdl`) and look for an `include` line naming the file. Not
   found → `NotIncluded`, and log the exact line the user should add. This is a heuristic —
   it doesn't follow nested includes — so `docs/blur.md` should say so.
**Shell usage:**
 
```rust
// main(), before App::run()
let niri_rounds = matches!(
    amane::write_niri_rules("amane.kdl", &[
        NiriLayerRule::namespace("amane-bar").corner_radius(BAR_RADIUS).blur(true),
        NiriLayerRule::namespace("amane-launcher").corner_radius(16.0).blur(true).xray(false),
    ]),
    FragmentStatus::Written | FragmentStatus::Unchanged,
);
// NotIncluded / Invalid / NotNiri all fall back to protocol blur.
 
fn bar() -> LayerWindow {
    let w = LayerWindow::new() /* … */;
    if niri_rounds() { w } else { w.blur(BAR_RADIUS) }   // never both: the region disables niri's clip
}
```
 
`FragmentStatus::NotIncluded` falls back to protocol blur, so a user who never adds the
include line still gets blur — just with stepped corners.
 
**Scope.** This is deliberately a narrow, niri-only helper for the shell's *own* layer
namespaces. It is not a general niri-config editor, and it never touches the user's main
config file.
 
### A.7 Popups and normal windows
 
`Popup::blur` / `Popup::blur_region` and the same on `Window` reuse the same `update_blur`.
niri already blurs popups that request it, defaulting them to non-xray — right for menus
that open over windows. Lock surfaces are out of scope: niri doesn't support the effect on
them.
 
### A.8 Shell usage (`amane-shell`)
 
```rust
fn bar() -> LayerWindow {
    let bg = if amane::blur_supported() { Color::rgba(20, 20, 28, 0.55) }
             else                       { Color::rgba(20, 20, 28, 0.92) };
 
    LayerWindow::new().width(Full).height(Fixed(36.0))
        .anchor_vertical(Top)
        .blur(12.0)                      // matches the Rectangle's radius below
        .child(Rectangle::new().fill(bg).radius(12.0) /* … */)
}
```
 
For a floating bar over windows (no exclusive zone), turn off xray in niri so windows, not
just the wallpaper, are blurred. With §A.6 the shell writes this itself
(`.xray(false)` on its `NiriLayerRule`); without it, the user adds:
 
```kdl
layer-rule {
    match namespace="^amane-bar$"
    background-effect { xray false }
}
```
 
niri marks non-xray effects experimental (they disappear briefly during window open/close
animations); document that in `docs/blur.md`.
 
### A.9 Optional niri nicety (not required)
 
niri always advertises the `Blur` capability, even with `blur { off }` in the config, so
`blur_supported()` reports `true` while nothing blurs and the shell keeps its translucent
look. A small fork change — advertise no capabilities when blur is off — fixes that. The protocol
says capabilities are sent on bind *and every time they change*, so niri should re-send them
on config reload, and Amane's `update_capabilities` picks the change up live.
 
### A.10 Files touched
 
| File | Change |
|------|--------|
| `src/layer_window.rs`, `src/popup.rs`, `src/window.rs` | `blur`, `blur_region`, `BlurArea` |
| `src/wayland.rs` | bind `BackgroundEffectState`; `BackgroundEffectHandler`; `blur_supported` flag |
| `src/wayland/surface.rs`, `update.rs` | `update_blur`, resize hook, destroy order, `rounded_strips` |
| `src/compositor/niri_config.rs` (new) | `NiriLayerRule`, `write_niri_rules`, `FragmentStatus` |
| `src/lib.rs` | export `BlurArea`, `blur_supported`, `NiriLayerRule`, `write_niri_rules`, `FragmentStatus` |
| `docs/blur.md`, `FORK.md` | document, incl. the include line, the xray tip, and the "never both" rule |
 
### A.11 Verification
 
1. **Unit:** `rounded_strips` never extends outside the ideal rounded rect, leaves no gaps
   or overlaps between strips, and misses at most one pixel per row at each corner — for
   uniform and per-corner radii, at UI scales 1.0 and 1.25. Zero radius gives one rect.
2. **Protocol:** `WAYLAND_DEBUG=1 amane-shell 2>&1 | grep background_effect` shows one
   `get_background_effect` and one `set_blur_region` per window at start, then **nothing**
   while the bar is idle (coalescing). Resizing the bar (output scale change) sends exactly
   one new region.
3. **Visual on niri:** translucent bar shows the blurred wallpaper; an island bar blurs only
   the island; a popup menu blurs the window under it. Zoom in on a corner at output scale 1
   and 2 over a high-contrast wallpaper, and compare against the shell-owned niri rule
   (§A.6) to judge whether the stair-steps are acceptable.
4. **Degrade:** on a compositor without the global (or with blur unavailable),
   `blur_supported()` is false, the fallback background shows, and no protocol errors occur.
5. **Teardown:** closing and reopening a popup 100 times produces no protocol error
   (one effect object per surface, destroyed before the surface).
6. **Rules file (§A.6), unit:** the builder escapes namespaces containing regex and KDL
   special characters; generated KDL passes `niri validate -c`.
7. **Rules file, write-if-changed:** start the shell twice with the same settings — the
   second start returns `Unchanged`, leaves the file's mtime alone, and niri logs no config
   reload. Change the bar radius → one `Written`, one niri reload, smooth corners at the
   new radius.
8. **Rules file, bad output:** force an invalid rule in a test build → `Invalid`, the
   installed file is untouched, and niri's config keeps reloading normally.
9. **Rules file, fallbacks:** remove the include line → `NotIncluded` with the line logged,
   and the bar falls back to protocol blur. Run under Hyprland → `NotNiri`, no file written.
---
 
## Part B — Launch with activation tokens (`xdg-activation-v1`)
 
### B.1 Why
 
When a launcher starts an app, the new window should get focus. Compositors decide that with
**activation tokens**: the launcher asks the compositor for a token tied to the click or key
press, passes it to the child in `XDG_ACTIVATION_TOKEN` (and the legacy
`DESKTOP_STARTUP_ID`), and the app hands it back when its window appears.
 
Amane's launcher path skips this. `DesktopApp::launch()` calls `spawn()`, which runs
`sh -c <Exec>` with no token. Results today:
 
- **niri, default policy:** focus falls to niri's "smart" heuristic, which usually works but
  not always.
- **niri with `debug { strict-new-window-focus-policy }`, or any strict compositor:** windows
  launched from the bar open **unfocused**.
- **Worse: a stale inherited token.** niri's `spawn-at-startup` gives Amane its own
  `XDG_ACTIVATION_TOKEN`, and `std::process::Command` passes it to every child. Apps started
  later present an expired token. Remove it from children's environments.
`Windows::spawn` (the compositor action) is fine on niri, because niri creates a token for
IPC `Spawn` actions itself. It isn't portable, though, and `DesktopApp::launch` doesn't use it.
 
### B.2 What already exists
 
**niri (no changes needed).** `XdgActivationHandler::token_created` in
`src/handlers/mod.rs`:
 
- A token **with a serial** is valid when that serial is no older than the seat's last
  keyboard *or* pointer enter. The comment notes this covers layer-shell surfaces with no
  keyboard focus, which is a bar.
- A token **without a serial** is marked urgency-only: the window gets the urgent hint, not
  focus.
- Tokens expire after 10 s (`XDG_ACTIVATION_TOKEN_TIMEOUT`).
**Amane:**
 
- SCTK 0.21.1 ships `smithay_client_toolkit::activation::{ActivationState, ActivationHandler,
  RequestData}`: bind, `request_token(qh, RequestData { app_id, seat_and_serial, surface,
  udata })`, and `new_token(token, data)` when the compositor answers.
- `WaylandState` already tracks `last_input_serial` (set in `keyboard.rs` and `pointer.rs`)
  and `keyboard_focus`, and has `self.seat.seats()`.
- The cross-thread request pattern already exists: `Lock::start()` sets a static flag and
  wakes the event loop; the wake source (`src/wayland/wake.rs`) calls
  `start_lock_if_asked()`. Launch requests use the same path.
### B.3 Public API
 
```rust
// unchanged signature, new behaviour
impl DesktopApp {
    pub fn launch(&self);            // now activation-aware
    pub fn id(&self) -> &str;        // new: desktop file id, e.g. "org.mozilla.firefox"
}
 
/// Run a command like `spawn`, but with an activation token so its window
/// gets focus. For shell buttons that start programs.
pub fn launch(command: &str);
```
 
`amane::spawn` stays as it is for scripts and background commands, except that it now strips
inherited activation variables (§B.5).
 
### B.4 Flow
 
```mermaid
sequenceDiagram
    participant V as view / input handler
    participant W as Wayland thread
    participant C as compositor
    V->>W: push LaunchRequest, wake the event loop
    Note over W: launch_requested()
    alt no xdg-activation global
        Note over W: spawn now, no token
    else
        W->>C: request_token(app_id, seat+serial, surface)
        Note over W: remember the request, arm a 500 ms timer
        alt the token comes
            C-->>W: new_token(token)
            Note over W: spawn with XDG_ACTIVATION_TOKEN=token
        else the timer fires first
            Note over W: spawn without a token
        end
    end
```
 
1. **Queue.** `launch()` pushes
   `LaunchRequest { command, app_id: Option<String>, id: u64 }` into a static
   `Mutex<Vec<_>>` and wakes the loop. Nothing blocks the view.
2. **Request.** `launch_requested()` drains the queue. If `ActivationState` is bound, it
   calls `request_token` with:
   - `app_id`: the desktop file id, so the compositor knows which app the token is for;
   - `seat_and_serial`: the first seat and **`last_activation_serial`** (below);
   - `surface`: the surface that received that input;
   - `udata`: the request id.
3. **Answer.** `ActivationHandler::new_token` takes the matching request and spawns it with
   the token.
4. **Fallback.** A calloop timer spawns any request still waiting after **500 ms** without a
   token. The compositor normally answers within one round trip, so a user never feels this.
   No global at all → spawn immediately.
**Use the right serial.** `last_input_serial` today is also updated by pointer *enter* and
*leave*. A leave serial makes a poor token: niri compares against the seat's latest enter,
and moving the pointer off the bar to a window before launching makes it stale. Add
`last_activation_serial: Option<(u32, WlSurface)>`, set **only** on key press and pointer
button press, with the surface that got the event. Use it for tokens; popups keep using
`last_input_serial` as now.
 
### B.5 Spawning (`src/process.rs`)
 
- `shell(command)` gains `.env_remove("XDG_ACTIVATION_TOKEN")` and
  `.env_remove("DESKTOP_STARTUP_ID")`, so no child ever inherits Amane's own stale startup
  token. This applies to `spawn`, `output` and `lines`.
- New internal `spawn_with_token(command, token: Option<&str>)` sets both variables when
  a token is present; `launch()` and `DesktopApp::launch()` use it.
- The existing reaper (`thread::spawn(move || child.wait())`) stays.
### B.6 `DesktopApp` changes (`src/services/apps/*`)
 
- `parse::read` keeps the desktop file id it already computes (`parse::id`) and stores it in
  `DesktopApp::id`. It's used as the token's `app_id`.
- Leave `Exec` parsing as it is (field codes stripped, run through `sh -c`).
**Phase 2, separate commit: `DBusActivatable=true` entries.** The desktop entry spec says
such apps should be started with `org.freedesktop.Application.Activate(platform_data)` on
the session bus, with the token in `platform_data` as `activation-token` and
`desktop-startup-id`. Their `Exec` line usually works too, so v1 doesn't need this. Do it
once Amane's `Bus` wrapper can encode an `a{sv}` argument (confirm in `src/dbus/`); bus name
= desktop id, object path = the id with `.` → `/`.
 
### B.7 Files touched
 
| File | Change |
|------|--------|
| `src/wayland.rs` | bind `ActivationState` (optional); `ActivationHandler`; pending map + timer |
| `src/wayland/keyboard.rs`, `pointer.rs` | set `last_activation_serial` on press only |
| `src/wayland/wake.rs` | call `launch_requested()` |
| `src/process.rs` | strip inherited vars; `spawn_with_token`; public `launch` |
| `src/services/apps/desktop_app.rs`, `parse.rs` | `id`; `launch()` goes through activation |
| `src/lib.rs` | export `launch` |
| `docs/launching.md`, `FORK.md` | document |
 
### B.8 Verification
 
1. **Strict-policy test (the clear pass/fail).** In niri config set
   `debug { strict-new-window-focus-policy }`, focus a terminal, launch Firefox from the
   bar's launcher: **before** the change it opens unfocused; **after**, it opens focused.
2. **Protocol:** `WAYLAND_DEBUG=1` shows `get_activation_token`, `set_serial`, `set_app_id`,
   `set_surface`, `commit`, then `done` with the token, then the spawn.
3. **Child env:** `tr '\0' '\n' < /proc/<child-pid>/environ | grep -E
   'XDG_ACTIVATION_TOKEN|DESKTOP_STARTUP_ID'` shows the fresh token for `launch()`, and
   **nothing** for plain `spawn()` children.
4. **Keyboard launcher:** type a name and press Enter in a launcher with exclusive keyboard
   focus → focused, because the key press serial is used.
5. **Pointer moved away first:** click the launcher entry, then move the pointer onto another
   window before the app opens → still focused, because the press serial and surface were
   kept rather than the later leave serial.
6. **Fallback:** on a compositor without `xdg-activation`, launching still works (no token,
   no delay). A unit test of the pending map drives the 500 ms timeout with a fake clock.
7. **Other compositors:** repeat test 1's scenario on Hyprland and Sway; launched windows
   take focus.
---
 
## Fit against the Amane rubric
 
| Check | A. Blur | B. Activation |
|-------|---------|---------------|
| Live state / clean API | ✓ `blur()`, `blur_region()`, `blur_supported()` | ✓ `launch()` — same call sites, better behaviour |
| Coalesces when idle | ✓ diffed like `input_region`; idle bar sends nothing | ✓ nothing happens unless a launch is requested |
| Degrades | ✓ no global or capability → no-op, `blur_supported()` false | ✓ no global → plain spawn |
| One module + one doc, additive | ✓ new methods + backend; `docs/blur.md` | ✓ `docs/launching.md` |
| Presentation left to shell | ✓ shell chooses colors, radius, fallback, and what goes in the niri rules file | ✓ shell decides what to launch |
| No new layout vocabulary | ✓ window property, not a widget | ✓ n/a |
| Plays to immediate mode | ✓ region described per frame, diffed | ✓ fire-and-forget from input handlers |
 
## Open questions / risks
 
- **Corner quality.** Protocol blur corners are 1-logical-pixel steps with no antialiasing
  (§A.5). On niri, the shell-owned rules file (§A.6) gives smooth corners for whole-window
  blur. Island shapes, and other compositors, stay stepped unless a future version of the
  protocol adds a radius or mask request.
- **The rules file writes into the user's config directory.** It's one clearly labelled,
  generated file that Amane fully owns, validated before install and only written on change.
  Detecting the include line is a heuristic (no nested includes); a missed detection only
  means falling back to protocol blur.
- **Include position.** Rules the user writes *after* the include line override Amane's.
  That's intended — the user stays in control — but `docs/blur.md` should say so.
- **Region vs. drawn shape drift.** The blur region is declared separately from the widgets
  that draw the translucent background, so a shell can change one and forget the other.
  `blur(radius)` covers the common whole-window case. A later "blur behind this widget"
  option could collect widget rects during layout, but it would add a widget-level property
  to the core vocabulary — wait for a concrete need.
- **Capability doesn't reflect `blur { off }`** on niri (§A.9).
- **Xray surprises.** Users with a floating bar may expect windows blurred and see only the
  wallpaper. The layer-rule tip in `docs/blur.md` covers it.
- **Apps that ignore tokens.** Some apps (Discord, Telegram) mint their own tokens with
  invalid serials; niri has a debug flag for that. It's an app problem, outside this spec.
- **Token request with no recent input** (a timer-driven launch, e.g. from a scheduled
  action) will be urgency-only on niri. That's correct behaviour, not a bug; document it.
- **Rebase safety.** All additive: new optional globals, new methods, one env change in
  `process.rs`. Keep `docs/blur.md` and `docs/launching.md`.
## Dependencies
 
None new. SCTK 0.21.1 (already pinned) provides both `background_effect` and `activation`
helpers; `wayland-protocols 0.32.13` (already in `Cargo.lock`) carries the bindings.
 
### Related docs
- `wayland-feature-candidates.md` — where these two were picked from.
- `amane-design-values-and-feature-fit.md` — the rubric above.
