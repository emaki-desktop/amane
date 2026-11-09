# Polkit agent (fork addition)

`Polkit` answers polkitd's authentication requests: when a program needs a privilege (`pkexec`,
`systemctl start`, a Flatpak install), polkitd asks the session's agent to authenticate the
user, and a shell that calls `Polkit::serve_agent()` once is that agent. It replaces a separate
agent process (polkit-kde-agent, hyprpolkitagent) with a prompt in the shell's own look.

The agent is behind the `polkit-agent` cargo feature, off by default, since only one program in
a session should be the agent. Without it, `Polkit`, `AuthRequest` and `AuthPrompt` still build,
so a UI compiles, but nothing serves or answers requests.

`Polkit::serve_agent_with(PolkitAgent { helper_socket })` takes the helper's socket for a system
that puts it elsewhere; `serve_agent()` uses `/run/polkit/agent-helper.socket`.

## How it works

- **Registered** on the system bus at `/org/amane/PolicyKit1/AuthenticationAgent` for the
  `unix-session` subject logind names as the user's display (`org.freedesktop.login1`
  `/user/self` `Display`, else `XDG_SESSION_ID`), with the process's locale. A shell running as a
  user service is outside the session's scope, so the session is asked for rather than read off
  the process. Registered again when `org.freedesktop.PolicyKit1` changes owner.
- **Only polkitd.** `BeginAuthentication` and `CancelAuthentication` are answered only when their
  sender is the bus name that owns `org.freedesktop.PolicyKit1` right now (`GetNameOwner`); any
  other caller gets `…Error.Failed`. The system bus's policy should see to that already; this
  holds where a policy is loose.
- **BeginAuthentication** parks polkitd's call, picks the user to authenticate as (the one
  running amane if polkitd lists them, else the first `unix-user`; the name comes from the
  system's user database through NSS, `getpwuid_r`, so LDAP, sssd and homed users are found
  too), reads what will run from polkit's `details` (below), and publishes an `AuthRequest`. **CancelAuthentication** (the program gave up, or timed out)
  answers it with `org.freedesktop.PolicyKit1.Error.Cancelled` and clears the request.
- **The helper.** Polkit 124+ runs its PAM helper as a socket-activated system service
  (`/run/polkit/agent-helper.socket`, world-writable) instead of a setuid program.
  `services/polkit/helper.rs` connects, writes `<user>\n<cookie>\n`, and relays lines:
  `PAM_PROMPT_ECHO_OFF <text>` / `PAM_PROMPT_ECHO_ON` (something to type), `PAM_TEXT_INFO`
  (a fingerprint reader's hint), `PAM_ERROR_MSG`, then `SUCCESS` or `FAILURE` (text is
  `g_strescape`d; `unescape()` undoes it). An answer is one line back. The helper tells polkitd
  the result itself (`AuthenticationAgentResponse2`, with the socket peer's uid), so the agent
  only replies to the parked call: empty on success, `…Error.Failed` after three failures.
  Closing the socket is how a cancel reaches the helper.
- **What will run.** `command()` is pkexec's `command_line` detail, arguments included, so a
  helper's target (the system Emaki Settings activates, say) is seen before the password is
  given. Without one it's the command line of the process asking (`polkit.subject-pid`, read from
  `/proc`). Control characters show as spaces, so a line break can't hide the rest or fake a
  second line, and it's cut at 512 characters.
- A wrong password starts the helper again with the same cookie, up to three tries, with
  `error()` an `AuthenticationFailed` [`ServiceError`](errors.md) in between, which the shell
  words; the helper's own error lines come as `Failed` with its text.

## The request

| `AuthRequest` | Gives |
|---|---|
| `id()` | what `answer`/`cancel` take |
| `action_id()`, `message()`, `icon()` | polkit's action (`org.freedesktop.systemd1.manage-units`) and its own sentence |
| `user()` | who the password proves |
| `command()` | what will run with the privilege: pkexec's command line, else the asking process's; `None` when polkit says neither |
| `prompt()` | `AuthPrompt { text(), echo() }` once the helper asks; `None` before, and while an answer is checked |
| `info()`, `error()` | lines the helper showed; what went wrong with the last try, as a `ServiceError` |
| `tries()`, `busy()` | wrong answers so far; an answer is with the helper |

`Polkit::answer(id, text)` sends what was typed (kept until the helper asks, if it hasn't
yet); `Polkit::cancel(id)` refuses. `text` is anything that turns into a `Zeroizing<String>`
(`TextInput::take_secret` gives one), and it's wiped once the helper has it, the copy kept
meanwhile too. An answer with a line break in it is refused with a `Failed` error ("A password
can't have a line break in it."): the helper reads one answer a line, so the rest would answer
its next question. Both run off the caller's thread. `is_registered()` says
whether polkitd took the registration. Only one request is open at a time; a second from
polkitd is refused with `…Error.Failed`.

## In the shell

`emaki` calls `Polkit::serve_agent()` at start and draws the request as its alert
(`src/surfaces/alert/polkit.rs`): the action's message, the helper's lines, the command line
(`command()`), a password field, **Cancel** and **Authenticate**. Return answers, Escape cancels.
