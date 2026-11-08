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
- **BeginAuthentication** parks polkitd's call, picks the user to authenticate as (the one
  running amane if polkitd lists them, else the first `unix-user`; `/etc/passwd` gives the name),
  and publishes an `AuthRequest`. **CancelAuthentication** (the program gave up, or timed out)
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
- A wrong password starts the helper again with the same cookie, up to three tries, with
  `error()` an `AuthenticationFailed` [`ServiceError`](errors.md) in between, which the shell
  words; the helper's own error lines come as `Failed` with its text.

## The request

| `AuthRequest` | Gives |
|---|---|
| `id()` | what `answer`/`cancel` take |
| `action_id()`, `message()`, `icon()` | polkit's action (`org.freedesktop.systemd1.manage-units`) and its own sentence |
| `user()` | who the password proves |
| `prompt()` | `AuthPrompt { text(), echo() }` once the helper asks; `None` before, and while an answer is checked |
| `info()`, `error()` | lines the helper showed; what went wrong with the last try, as a `ServiceError` |
| `tries()`, `busy()` | wrong answers so far; an answer is with the helper |

`Polkit::answer(id, text)` sends what was typed (kept until the helper asks, if it hasn't
yet); `Polkit::cancel(id)` refuses. Both run off the caller's thread. `is_registered()` says
whether polkitd took the registration. Only one request is open at a time; a second from
polkitd is refused with `…Error.Failed`.

## In the shell

`emaki` calls `Polkit::serve_agent()` at start and draws the request as its alert
(`src/surfaces/alert/polkit.rs`): the action's message, the helper's lines, a password field,
**Cancel** and **Authenticate**. Return answers, Escape cancels.
