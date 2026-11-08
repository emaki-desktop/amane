# Settings Files

`Config<T>` keeps an app's settings in a JSON file in `~/.config`, and reads it like a Service. Every program that reads the same file sees a change as soon as it's saved, so a settings app can change the shell while both run.

```rust,ignore
use amane::{Config, Settings};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
struct Appearance {
    accent: String,
    dark: bool,
}

// ~/.config/emaki/appearance.json
impl Settings for Appearance {
    const APP: &'static str = "emaki";
    const FILE: &'static str = "appearance";
}

fn view() -> LayerWindow {
    let appearance = Config::<Appearance>::read();

    // ... draw with appearance.accent and appearance.dark
}

fn toggle_dark() {
    Config::<Appearance>::change(|appearance| appearance.dark = !appearance.dark);
}
```

- `Config::<T>::read()` gives the settings. A view that reads them redraws when they change, from this program or another.
- `Config::<T>::change(edit)` changes them and saves the file. The file is written next to the old one and moved over it, so nobody reads half a file. A change that changes nothing writes nothing.
- A missing file gives `T::default()`. `#[serde(default)]` fills in fields an older file doesn't have yet, so you can add settings later.
- A broken file gives the defaults too, with a message on stderr, and is left alone until the next `change`.
- The file is watched, so editing it by hand works as well.

`Settings` needs serde's `Serialize` and `Deserialize`. Add serde with its `derive` feature to your app, or use the copy Amane re-exports: `use amane::serde::{Deserialize, Serialize};` together with `#[serde(crate = "amane::serde")]` on the struct.

`XDG_CONFIG_HOME` moves `~/.config`, as it does for other programs.
