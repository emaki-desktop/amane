# Input method and secure fill (fork addition)

An app can be the seat's input method: it then hears what kind of text field has the keyboard in
*any* app, shows a popup by that field, and can put text into it through the compositor's checked
fill. The shell's password fill is built on this. Upstream amane only has the other side, an app's
own `TextInput` talking to an input method ([text-input](text-input.md#input-methods)).

## Usage

```rust,ignore
App::new()
    .window(bar)
    .input_method(fill_popup)   // this app becomes the input method
    .run();

fn fill_popup() -> Popup {
    let field = FocusedField::read();

    Popup::new("fill")
        .size(260, 120)
        .anchor_rect(0, 0, 1, 1)          // not used: the compositor places it by the field
        .visible(field.active() && field.is_password() && !field.own())
        .child(rows())
}

// on a click: fill the field that was focused when the popup showed
FocusedField::fill(Zeroizing::new(secret), generation, |result| { /* Ok(()) or Err(FillError) */ });
```

- **`App::input_method(view)`** binds `zwp_input_method_v2` (and `niri-secure-fill-v1` where the
  compositor has it), only for the app that calls it. Ordinary amane apps never take the seat's
  one input-method slot.
- **The popup** is a `zwp_input_popup_surface_v2`. It shows while a field has the keyboard and
  the view says `.visible(true)`. The compositor places it below the field, or above it near the
  bottom of the screen, and `FocusedField::field_rect` says where the field is relative to it. It
  takes the pointer, not the keyboard, and the app keeps its keys. There's no keyboard grab and no
  virtual keyboard.
- **`FocusedField`**, a `Service`:

  | Method | What it says |
  |---|---|
  | `available()` | this app is the input method |
  | `can_fill()` | the compositor has secure fill too |
  | `active()` | some field has the keyboard |
  | `purpose()`, `hints()` | what the app said the field is |
  | `is_password()` | Password purpose, or hidden sensitive text |
  | `empty()` | whether the field holds text, when the app said; never for amane's password inputs |
  | `generation()` | a new number for every new field |
  | `owner()` | a `FieldOwner`, below |
  | `own()` | the field is in this app's own windows |

  A `FieldOwner` comes from the compositor and holds:
  - `pid`: from the app's socket;
  - `exe`: read by the compositor from `/proc/<pid>/exe`;
  - `app_id`: what the app calls itself, so only a hint;
  - `window_id`: the IPC window id;
  - `sandboxed`, `takes_text`, `in_window`, `in_layer`.
- **`FocusedField::fill(text, generation, done)`** can be called from any thread. It sends the
  text to the field of `generation` through `niri-secure-fill-v1`, and `done` gets `Ok(())` or a
  `FillError`:

  | `FillError` | Why |
  |---|---|
  | `Changed` | another field or app has the keyboard |
  | `NoTextInput` | the field takes no text input |
  | `Locked` | the session is locked |
  | `Expired` | the target is too old |
  | `Used` | the target was filled already |
  | `TooSoon` | fills come too quickly |
  | `InvalidText` | the text is empty, too long or has control characters |
  | `Unsupported` | no secure fill, or not the input method |

  The text is wiped once sent.

Example: `examples/input_method.rs`. Run it beside the shell, then focus a field in another app.

## Design / internals

- **Never `commit_string`.** An input method's `commit_string` goes to whichever text input is
  focused when the request arrives, so a secret could reach an app that took the focus a moment
  before. amane never commits through the input method. `fill` goes through the niri fork's
  `niri-secure-fill-v1`. For each new field (each `generation`), the event loop takes a target,
  the compositor's snapshot of that field and its owner. A fill goes to that target, and the
  compositor commits it only if the same text input of the same surface still has the keyboard
  ([niri's doc](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/input/secure-fill.md)). A
  target is filled once; it then waits for its answer and a fresh target is taken for the same
  field.
- **What is kept.** `surrounding_text` is reduced to whether the field is empty, and the text
  is wiped as soon as it arrives. Content type, change cause and activation are kept;
  nothing typed is.
- **One per seat.** When another input method holds the seat, the compositor sends
  `unavailable`. amane logs it once, `FocusedField::available()` turns false, and nothing else
  happens.
- **Own fields.** amane's own `TextInput`s also talk to the seat's input method. While one of
  this app's surfaces has the keyboard, `own()` is true so the popup can stay away from them.

Code:
- `src/wayland/input_method_v2.rs`: the input method and the popup role (`Role::InputPopup`);
- `src/wayland/secure_fill.rs`: targets and fills;
- `src/services/focused_field.rs`: the service.
