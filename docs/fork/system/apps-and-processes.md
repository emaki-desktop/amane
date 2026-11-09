# Apps and processes (fork additions)

What the fork adds to upstream's [Apps](../../src/apps.md) and
[Commands and Files](../../src/processes.md) pages: more of a desktop entry (id, categories,
keywords, actions, the files it opens), opening files with an app, an icon lookup, launching with
an activation token, and `watch_file` seeing deletes.

## Usage

### `DesktopApp`

New functions, next to upstream's `name`, `description`, `exec`, `icon`, `icon_path` and `launch`:

| Function | Gives or does |
|---|---|
| `id()` | its desktop file's id, like `"org.mozilla.firefox.desktop"`: stays the same while it's installed, a key to remember it by |
| `categories()` | its freedesktop `Categories`, like `["Network", "WebBrowser"]` |
| `keywords()` | its `Keywords`, extra search terms the author added |
| `actions()` | its desktop actions, other ways to start it like "New Private Window" (see below) |
| `mime_types()` | the kinds of file it says it opens (its `MimeType`), like `["text/markdown", "text/plain"]` |
| `opens(file)` | whether it opens `file`, by the kind of file its name says it is |
| `commands_with(files)` | the commands, as argument lists, that open `files` with it (see below) |
| `launch_with(files)` | starts it on `files`, without waiting |

Each `DesktopAction` is one of the `[Desktop Action …]` sections the entry's `Actions` lists, in
that order; one without a `Name` or `Exec` is left out. It has `name()`, `exec()` (without the
placeholders), `icon()` (its own icon's name, or `None` to use the app's) and `launch()`:

```rust,ignore
for action in app.actions() {
    println!("{}: {}", action.name(), action.exec());
}
```

### Opening files

`opens(file)` finds the file's kind from its name in the shared MIME database (`mime/globs2` in the XDG data dirs; a folder is `inode/directory`), and says yes when the app lists that kind or one it comes from (`mime/subclasses`; every `text/…` kind is `text/plain` too). An app with no `MimeType` opens nothing.

`commands_with(files)` gives argument lists (`Vec<Vec<String>>`), not shell lines. The entry's `Exec` is split into arguments as the Desktop Entry spec says: spaces between them, double quotes around one, with `\` taking the next letter as it is inside quotes. Then the files are filled in: `%F` and `%U` take them all, as paths and as `file://` links, each an argument of its own, while `%f` and `%u` take one, so there's a command per file; a code inside an argument (`--file=%f`) is one file in its place. A code inside quotes isn't filled in, as the spec has none there. So a file is never text a shell reads: one named `$(rm -rf ~).txt` stays only a name, even for an `Exec` that is `sh -c "…"`. An `Exec` with no codes opens no files, and you get the plain command; one that can't be split (a quote left open) gives no command, and says so on stderr. `launch_with(files)` runs them with `Launch::argv`; with no files it's `launch()`. To have the compositor run them instead, hand each list to `Windows::spawn(command)`.

```rust,ignore
let file = PathBuf::from("/home/me/notes.md");

if app.opens(&file) {
    app.launch_with(&[file]);
}
```

### Icons without a desktop file

`Apps::find_icon(name)` looks up an icon by its name in the icon theme (or takes a full path as it is) and gives its image file, or `None`. Use it for something with no `.desktop` file, like a window whose app_id names an icon:

```rust,ignore
let icon = Apps::find_icon("org.gnome.Nautilus");
```

### Launching apps

| Function | Does | Waits? |
|---|---|---|
| `Launch::argv(&[..]).activated()` | starts a program and its arguments as they are, letting it take the focus | no |
| `Launch::shell(line).activated()` | the same for a command line, through `sh -c` | no |
| `amane::spawn_activated(command)` | `Launch::shell(command).activated()` | no |

A launcher, a dock or a button that opens an app should launch this way instead of with `spawn`,
and call it from input, since the compositor's token stands for that input. The program starts
once the token comes, on the event loop; without an event loop, or on a compositor that gives no
tokens, it starts at once. One that can't start is logged. Prefer `argv`: a file name or link
passed to it needs no quoting for the shell.

```rust,ignore
.on_click(|_| amane::Launch::argv(&["xdg-open", &link]).activated())
```

`DesktopApp::launch` and `launch_with` launch this way. How the token works, and how an amane app
uses one, is in [Activation](../windows/activation.md).

### Each program in its own scope

Every program amane starts goes into a transient systemd scope of its own,
`app-amane-<app>-<pid>.scope` under `app.slice`, the way niri and GNOME start apps. Restarting the
program that started it (a shell's own `systemd --user` service) then no longer stops it: before,
an app opened from the dock lived in the shell's cgroup and went down with every restart. The
scope is asked for over the session bus (`StartTransientUnit`) right after the program starts,
on the worker thread, so nothing waits on systemd; without a systemd session it stays where it
started, said once on stderr. A program that forks within the first milliseconds can leave that
child behind, the trade GNOME and niri make too. `<app>` is the desktop id for a `DesktopApp`,
otherwise the program's file name.

### `Launch`: starting without panicking

| API | Does |
|---|---|
| `Launch::shell(line)` | a command line through `sh -c` |
| `Launch::argv(&[program, args…])` | a program and its arguments as they are |
| `.token(Some(token))` | the activation token, as `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` |
| `.app_id(Some(id))` | the app its scope is named for |
| `.spawn() -> Result<u32, LaunchError>` | starts it in its scope; the process id, or why it didn't start |

`spawn`, `spawn_activated` and the desktop launches go through it and log a program that won't
start instead of panicking. `output` gives an empty string, and `lines` no lines, for a command
that can't run. Code: `src/launch.rs`; commit `b2f770a`.

### Watching files

`watch_file` gives an item each time the file is written, created, replaced, deleted or moved away (upstream missed the last two, `7c36e11`).

## Design / internals

- The MIME lookup is `src/services/apps/mime.rs`, read from the shared-mime-info files.
- `services/apps/exec.rs` has the spec's tokenizer (`split`) and the filling (`fill`); its tests try
  thousands of random `Exec` lines and file names.
- Commits: `0ebcfd2` (categories, keywords), `d9d3d46` (actions), `7f48e10` (`Apps::find_icon`), `c029184` (MIME, `opens`, `launch_with`), `3c3d56b` (`id`), `82337f2` (`spawn_activated`), `b2f770a` (`Launch`, scopes).

## Apps rescans

`Apps` looks at the data dirs' `applications/` folders every 30 s and scans again (every
desktop file and icon theme) only when one of them changed. The comparison is a fingerprint
(`src/services/apps/fingerprint.rs`): where each folder really is, its modification time and
size. On NixOS a system switch points `/run/current-system` at a new store path, whose files all
carry the same time, so the real path is what changes.

## Default apps: `MimeApps` (fork addition)

`MimeApps` answers which app opens a kind of file or link, the way `xdg-open` does. It reads
every `mimeapps.list` in the spec's order: config dirs before data dirs' `applications/`, and in
each dir a `$desktop-mimeapps.list` before the plain one. With no default listed it falls back to
the data dirs' `mimeinfo.cache`. The user's own `~/.config/mimeapps.list` is watched with
`watch_file`, so a change shows at once; the other lists and the caches change with a system
switch, so every 30 s their [fingerprint](#apps-rescans) is compared, and they're read again only
when it moved.

- `default_app(kind)` is the first listed default that is installed, else the cache's.
- `handlers(kind)` lists the apps that open the kind: the shown ones that list it (or a kind it
  comes from), the ones `[Added Associations]` says open it, and any listed default.
- `set_default(kinds, id)` writes `kind=id;` lines into `[Default Applications]` of
  `~/.config/mimeapps.list`. A kind the app doesn't list itself also goes under
  `[Added Associations]`. The same key is taken out of `~/.config/$desktop-mimeapps.list`, which
  would otherwise still win. Every other line stays as it was, and a symlinked file (home-manager)
  is refused.
- `reset(kinds)` takes those lines away again. `own()` lists the defaults the user's file sets.

`Apps` also keeps `NoDisplay` entries now, since they still open links (a password manager's
`x-scheme-handler`, for example). `Apps::list` still shows only the shown ones. `Apps::all` and
`Apps::get(id)` include the rest, and `DesktopApp::shown` tells them apart.
`DesktopApp::opens_type(kind)` and `Apps::mime_type(name)` expose the shared MIME database.
