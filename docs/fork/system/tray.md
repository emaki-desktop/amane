# Tray (fork addition)

The **system tray**: the icons programs like Steam, Discord, Nextcloud or
nm-applet put in a status bar, over the StatusNotifierItem protocol, with the
menus they send over dbusmenu (`com.canonical.dbusmenu`). It's what a bar needs
to show other programs' icons and menus.

## Usage

```rust
let tray = Tray::read();

for item in tray.items() {
    // Passive items have nothing worth showing
    if item.status() == TrayStatus::Passive {
        continue;
    }

    // a file to draw: a theme icon, the item's own, or its pixels as a png
    let icon = item.icon();
    let tooltip = item.tooltip();
    let key = item.key();
}

// clicks, at a point on the screen
Tray::activate(&key, x, y);
Tray::secondary_activate(&key, x, y);
Tray::scroll(&key, 1, true);

// a menu is read on a worker; it shows up for take_menu, and readers redraw
Tray::open_menu(&key);
if let Some(entries) = Tray::take_menu(&key) {
    // TrayMenuItem: id, label, enabled, separator, toggle, icon, children
}
Tray::select(&key, id);

// an item without a dbusmenu draws its own
Tray::context_menu(&key, x, y);
```

`TrayItem::id()` is what the program calls the item ("steam", "nm-applet"), the
same each run, which a bar can keep settings by. `key()` tells it apart from
every other item now (its unique bus name and object path).

## How it works

- **The watcher.** `Tray::listen` owns `org.kde.StatusNotifierWatcher` and
  serves it at `/StatusNotifierWatcher`: `RegisterStatusNotifierItem`,
  `RegisterStatusNotifierHost`, its three properties (`Get`, `GetAll`) and
  `Introspect`. It also owns `org.kde.StatusNotifierHost-<pid>` and emits
  `StatusNotifierHostRegistered`, so programs that started first register again.
  An item registers by its object path (its bus name is the caller), by a bus
  name (at `/StatusNotifierItem`), or by both run together.
- **Another watcher.** When another program owns the watcher name, amane only
  registers as a host there, reads `RegisteredStatusNotifierItems`, and follows
  its `…Registered`/`…Unregistered` signals. If that program then quits, the
  tray stays as it was until amane restarts.
- **Items.** Each item is read whole (`GetAll`) as it registers, and again on
  any of its signals (`NewIcon`, `NewStatus`, `NewTitle`, `NewToolTip`, …;
  `Bus::interface_signals` hears them from any sender). An item goes when its
  unique bus name does (`NameOwnerChanged`).
- **Icons** (`tray/icon.rs`): the named icon is looked up in the item's
  `IconThemePath` first, then in the user's themes (`apps::find_icon`).
  Otherwise the `IconPixmap` nearest 48 px (ARGB32, network byte order) is
  written once as an RGBA png under `$XDG_CACHE_HOME/amane/tray/`, named by a
  hash of its pixels, so a changed icon is a new path the image cache loads
  afresh. While `NeedsAttention`, `icon()` is the attention icon. Tooltip HTML
  is stripped.
- **What an item may name.** An icon name is a name, never a path: one with a `/` in it, or
  starting with `.`, isn't looked up, so it can't reach out of the folder. The item's own folder
  is walked at most 4 levels down and 64 entries a folder, so one naming `/` doesn't have the disk
  walked. A pixmap counts only when its bytes are exactly width × height × 4 (in checked
  arithmetic, so sizes can't wrap round) and it's no bigger than an image may be.
- **Menus** (`tray/menu.rs`): `AboutToShow(0)`, then `GetLayout(0, -1, [])`,
  read into a tree. Underscores marking access keys come out, hidden entries
  are left out, and `icon-data` (a png) is written to the cache. Picking an
  entry sends `Event(id, "clicked", 0, 0)`. Submenus come with the first read;
  a program that fills a submenu only as it opens shows it empty.

## Limits

- No `OverlayIconName` and no symbolic tinting: icons draw in their own colors.
- `ContextMenu` for an item without a dbusmenu leaves the menu to the program,
  which on Wayland usually opens it as a window of its own.
