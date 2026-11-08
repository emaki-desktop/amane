# Spec: Presence & appearance for the Amane fork

> **Status: implemented** in `2953df9` (Idle) and `317943e`, `b1187e6` (Appearance). Kept as
> design history; the current docs are [Idle](../system/idle.md) and
> [Appearance](../system/appearance.md). The "draft" status line below is the original.
 
Status: draft · Target: `emaki-desktop/amane` (`emaki`) + `emaki` · Group 3 of 3
(ambient system state the bar *reacts to*: whether the user is present, and the system's
light/dark + accent preference).
 
Covers two freedesktop/Wayland features absent today: **idle & screensaver-inhibit** (idle
detection via `ext-idle-notify`, plus serving `org.freedesktop.ScreenSaver` so apps can inhibit
idle), and **appearance** via the XDG **Settings portal** (`color-scheme` + `accent-color`).
 
> **Conventions.** Session-bus D-Bus via Amane's `Bus`; Wayland protocols via the existing SCTK
> backend (`src/wayland.rs`). Immediate-mode `Service` (view reads `X::read()`, change →
> `write()`, no-op → `quiet()`); `listen()` runs the event loop on a helper thread. New module,
> additive `services.rs` edit, `docs/<name>.md`, kept thin. Background:
> `amane-tray-service-spec.md` §3.
 
---
 
## A. Presence — idle detection + screensaver inhibit
 
Two halves that belong together: **knowing** the user went idle, and **respecting** apps that
ask not to idle (video/presentations).
 
### A.1 Idle detection — `ext-idle-notify-v1` (Wayland)
 
The compositor (niri, Hyprland, Sway, Mango — all wlroots-style) implements
`ext-idle-notify-v1`. Amane binds the global, creates one or more
`ext_idle_notification_v1` at chosen timeouts, and receives `idled` / `resumed` events.
 
- Bind `ext_idle_notifier_v1` in the SCTK setup (optional global — degrade gracefully if
  absent, as Amane already does for fractional-scaling/viewporter).
- The shell declares timeouts (e.g. dim at 2 min, lock at 5 min). The service creates a
  notification per timeout and reports which thresholds are currently crossed.
- On `idled`/`resumed`, `write()` the new state → the bar can dim, start DND, or the `Session`
  service (Group 1) can lock / `SetIdleHint(true)`.
### A.2 Idle inhibit — `org.freedesktop.ScreenSaver` (serve it)
 
Apps call `org.freedesktop.ScreenSaver.Inhibit(app, reason) -> cookie` / `UnInhibit(cookie)` to
say "don't idle now" (browsers, video players, Zoom all do this). On a bare session nothing
serves it, so inhibits are lost. Amane should **own and serve** the name:
 
```
own  org.freedesktop.ScreenSaver   (also org.kde.screensaver as alias where practical)
path /org/freedesktop/ScreenSaver   iface org.freedesktop.ScreenSaver
  Inhibit(application_name: s, reason_for_inhibit: s) -> cookie: u
  UnInhibit(cookie: u)
  GetActive() -> b   (optional)
```
 
Keep the set of live cookies. While any inhibitor is held, the service reports
`inhibited = true`; the shell then suppresses its own dim/lock idle actions (and may also pause
the idle notifications). This also lets Amane bridge to the Wayland `idle-inhibit` where useful.
 
> Own-if-absent like the tray's watcher: if a DE already owns `org.freedesktop.ScreenSaver`,
> stay passive and just consume our own idle timers.
 
### A.3 Amane service
 
```rust
pub struct Idle {
    idle_levels: Vec<Duration>,   // configured thresholds
    crossed: Vec<Duration>,       // which are currently idle
    inhibited: bool,              // any ScreenSaver inhibitor held
}
impl Idle {
    pub fn is_idle(&self)->bool;              // any threshold crossed
    pub fn crossed(&self, level: Duration)->bool;
    pub fn inhibited(&self)->bool;            // respect this before dim/lock
}
```
 
`listen()`: run the ScreenSaver server loop (if owned) + the Wayland idle events. The Wayland
side lives on the main backend loop (idle events arrive as SCTK dispatches); the ScreenSaver
method server runs as a helper thread writing cookies into `Idle`.
 
---
 
## B. Appearance — XDG Settings portal (`color-scheme` + `accent-color`)
 
### B.1 Why
 
Amane has a `palette` service but nothing reads the **system** light/dark + accent preference.
Reading the Settings portal lets the bar follow the OS theme (and, where a backend allows,
*set* it). This is the standard cross-desktop source apps already honor.
 
### B.2 D-Bus surface (session bus)
 
```
dest org.freedesktop.portal.Desktop
path /org/freedesktop/portal/desktop
iface org.freedesktop.portal.Settings
  Read("org.freedesktop.appearance", "color-scheme") -> v(u)   0 no-pref, 1 prefer-dark, 2 prefer-light
  Read("org.freedesktop.appearance", "accent-color") -> v((ddd))  sRGB 0..1, or unset
  signal SettingChanged(namespace: s, key: s, value: v)
```
 
Read both at startup; subscribe to `SettingChanged` filtered to the `org.freedesktop.appearance`
namespace to update live. Map `color-scheme` → a `Scheme::{Dark,Light,NoPreference}` and
`accent-color` → an optional RGB the shell/`palette` can fold into its theme.
 
> **Setting** the scheme is desktop-specific (there's no portal *write*). Leave "drive the
> system theme" out of v1; expose read only. If the shell wants to flip its *own* palette, it
> reads `Appearance::scheme()` and chooses.
 
### B.3 Amane service
 
```rust
pub enum Scheme { NoPreference, Dark, Light }
pub struct Appearance { scheme: Scheme, accent: Option<Rgb> }
impl Appearance {
    pub fn scheme(&self)->Scheme;
    pub fn prefers_dark(&self)->bool;        // scheme == Dark
    pub fn accent(&self)->Option<Rgb>;
}
```
 
Low-frequency: one read + one `SettingChanged` subscription.
 
---
 
## Shell usage
 
- **Auto dark/light**: the view picks colors from `Appearance::prefers_dark()`; the existing
  `palette` can seed from `Appearance::accent()`.
- **Idle-aware behavior**: before dimming or locking, check `Idle::inhibited()` (don't dim
  during a video); use `Idle::crossed(dim_after)` / `crossed(lock_after)` to drive the steps,
  and let `Session::lock()` (Group 1) do the actual lock.
- **DND on idle / presence chip**: optional, from `Idle::is_idle()`.
## Verification
 
1. Leave the machine untouched past a configured threshold → `Idle::crossed()` flips; move the
   mouse → it clears (`resumed`).
2. Play a fullscreen video in a browser → `Idle::inhibited()` becomes true (browser inhibits);
   confirm the shell does not dim; close the video → inhibit releases.
3. `busctl --user introspect org.freedesktop.ScreenSaver /org/freedesktop/ScreenSaver` shows
   Amane serving it (when no DE owns it); a manual `Inhibit` call is reflected.
4. Toggle the system color scheme (e.g. via a portal-aware settings tool) → `Appearance::scheme`
   updates live via `SettingChanged`; the bar re-themes.
5. On a system with no portal / no idle global, both services degrade quietly (no panic, no
   redraw storm).
## Open questions / risks
 
- **ext-idle-notify binding** goes in the SCTK setup next to the other optional globals; make it
  truly optional so compositors lacking it still run.
- **Who owns ScreenSaver**: own-if-absent; a running DE screensaver service means stay passive.
- **accent-color availability** varies by portal backend; treat as optional.
- **Config surface**: idle thresholds should be shell-configurable (the service takes a list);
  decide defaults (e.g. dim 120s, lock 300s) in `docs/fork/idle.md`.
- **Rebase safety**: new modules `src/services/{idle,appearance}.rs` + one optional Wayland
  global bind in `src/wayland.rs` (additive). Keep per-feature docs.
## Dependencies
 
- `zbus` via `Bus`, SCTK/Wayland backend — both present. No new crates (the idle protocol is a
  Wayland global, bound through the existing toolkit).
