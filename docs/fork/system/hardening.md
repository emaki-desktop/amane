# Untrusted input and secrets (fork addition)

What amane holds against what other programs send it, and how it keeps passwords in. A shell
built on amane is the session's notification server, tray host, polkit agent and lock screen,
and reads files and clipboards other programs name, so a bug there is one any program of the
session can reach. The pages of each service say what it bounds; this page has what they share,
and the parts of upstream's services the fork changed for it.

## Usage

Nothing to call: the limits hold for every program on amane. The numbers, for a shell that wants
to say why something didn't show:

| What | Limit |
|---|---|
| An image file (`Image`, an icon, a notification's or tray item's picture) | a regular file only, never a device, fifo or socket; at most 64 MiB |
| A decoded image (png, jpeg) | at most 16384 pixels a side and 72 MP, checked from its header before its pixels are allocated |
| An svg | at most 4 MiB, and no `<image>` naming another file (only `data:` images inside it) |
| Text pasted, a drop's links or text | 16 MiB |
| A copy read by `ClipboardWatcher::contents` | 64 MiB |
| An IPC call | 64 KiB, 256 arguments, 2 seconds ([ipc](ipc.md)) |
| A notification | 30 a sender in 10 seconds, its text and buttons cut ([notifications](notifications.md)) |

## Design / internals

### Images

`graphics/image.rs` `read()` opens a file non-blocking (so even opening a fifo can't hold a
decoder thread), checks with `fstat` that it's a regular file and small enough, and reads at
most the limit, so a file still growing stops there. The png decoder gets a `Limits` for its own
buffers and the header's size is checked with `fits()` before the frame is allocated, since a
few bytes may claim 100000 by 100000; jpeg's sides are capped in zune's options and checked
after its headers. An svg is parsed with usvg's `<image>` resolver for other files turned off
(`image/svg.rs` `options()`): usvg's own reads any path it's given, `/dev/zero` or a private
file alike. Tests try a device, a fifo, a huge png header, random png headers, and an svg naming
a file.

### Reading what another program writes

A paste (`wayland/clipboard.rs` `read_text`), a drop (`wayland/dropped/drag.rs` `read_later`) and
`ClipboardWatcher::contents` read a pipe the other program fills, each on a thread of its own,
up to their limit; past it the read is dropped rather than kept growing.

### PAM

Upstream's `Pam` (the lock screen's authentication) keeps its API; the fork changed what it does
with the user and the password:

- **The user** is the one the process runs as, from `getuid` and the user database (NSS), never
  `$USER`, which whoever starts the program sets (`services/pam/config.rs` `current_user`).
- **A password given ahead** (the lock screen's typed one) answers only the hidden prompt it's
  for (`PAM_PROMPT_ECHO_OFF`). A visible prompt, like a one-time code, goes to the user as a
  prompt, as it would without one.
- **Answers are wiped.** A response travels as a `Zeroizing<String>`, is written straight into
  the C string PAM takes (no `CString` or `strdup` copy), and an answer PAM never took is
  overwritten (`explicit_bzero`) before it's freed. PAM frees, and Linux-PAM's modules wipe,
  the ones it took.

### Secrets elsewhere

- `Clipboard::copy_secret` moves the text out of its `Zeroizing` into the clipboard, which wipes
  it when it's replaced or cleared, so there's one copy rather than two.
- The polkit agent's answers are wiped too ([polkit](polkit.md)).
- One copy is left that amane can't wipe: `secure_fill` hands the text to wayland-client's
  generated `fill(String)`, which owns and drops it.

### The users module

`services/users.rs` names users through NSS (`getpwuid_r`, growing its buffer as the entry
needs), for PAM and the polkit agent alike, so LDAP, sssd and systemd-homed users are found,
not only `/etc/passwd`'s.

### Unsafe code

The fork's files with `unsafe` blocks that this work touched (`services/pam/native.rs`,
`services/users.rs`, `wayland/socket.rs`, `services/clipboard_watcher.rs`,
`services/notifications/image_hint.rs`) also warn on `clippy::undocumented_unsafe_blocks`, so
each block says why it holds (`// SAFETY: …`).

### Dependencies

`deny.toml` lets `cargo deny check` (run by `fork.yml`) refuse a crate with a known
vulnerability or yanked, one under a licence not on its list, and a git source other than the
vello fork.
