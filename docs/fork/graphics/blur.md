# Blur (fork addition)

A layer window can tell the compositor where to blur behind it: as plain
rectangles (`blur_region`) or as rounded, fading shapes, each optionally drawn
with a liquid-glass material (`blur_shapes`). Its content can also blend with
the frosted backdrop (`vibrant`). The compositor side is in the niri fork's
[blur shapes](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/effects/blur-shapes.md),
[vibrancy](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/effects/vibrancy.md) and
[materials](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/effects/materials.md) docs.

## Usage

```rust,ignore
.blur_region(vec![InputArea { x: 0, y: 0, width: 200, height: 40 }])   // the compositor blurs here
.blur_shapes(vec![BlurShape::new(8.0, 8.0, 164.0, 74.0, 28.0)])        // or in these rounded shapes
```

`blur_region` asks the compositor to blur behind rectangles (`ext-background-effect`). `blur_shapes` asks for rounded rectangles, each of which can fade with `.alpha(..)` and name a compositor material with `.material(..)`. To follow a layer that moves, use `.scale_about(..)`, `.offset(..)` and `.fade(..)`. niri from the amane fork draws shapes exactly (`niri-blur-shapes-v1`). Elsewhere amane sends them as a region of strips that step in at the corners, and leaves out any shape less than half visible, since a region can't fade. `amane::blur_fades()` tells which of the two you got, and `amane::material_available(name)` whether the compositor offers a material. Leave both unset to let the compositor's own rules decide; an empty list blurs nowhere. See `examples/blur_shapes.rs`.

```rust,ignore
LayerWindow::new()
    .width(200.0)
    .height(90.0)
    .blur_shapes(vec![
        BlurShape::new(8.0, 8.0, 164.0, 74.0, 28.0)
            .alpha(open)              // fades with the card
            .material("glass")        // if the compositor offers it
            .tint(Color::rgba(255, 255, 255, 40)),
    ])
```

A **blur shape** is a rounded rectangle the compositor blurs behind, with its own opacity.

The rest of this page covers how it works.

## Blur region — `ext-background-effect-v1`

`LayerWindow::blur_region(Vec<InputArea>)` works like `input_region`:

- `None` (not set) leaves blur to the compositor's own rules; an empty list
  blurs nowhere.
- The protocol object is created the first time a view asks for a region, and
  dropped with the window.
- Only a changed region is sent.

(`src/wayland/background_effect.rs`, `src/wayland/update.rs`.)

## Blur shapes — `niri-blur-shapes-v1`

`LayerWindow::blur_shapes(Vec<BlurShape>)` sends rounded rectangles, each with
its own corner radius, alpha, material and tint. Shapes take the place of a blur
region. The protocol is private to the niri fork
([niri fork: blur shapes](https://github.com/emaki-desktop/niri/blob/emaki/docs/fork/effects/blur-shapes.md));
it is vendored at `protocols/niri-blur-shapes-v1.xml`.

| API | Meaning |
|---|---|
| `BlurShape::new(x, y, w, h, radius)` | a rounded rectangle, logical px |
| `.alpha(a)` | how much of the effect shows, 0–1 |
| `.material(name)` / `.tint(color)` | a compositor material and the color it lays on |
| `.scale_about(s, origin)`, `.offset(dx, dy)`, `.fade(opacity)` | follow a layer that is scaled, moved or faded |
| `.id(n)` | the program's own name for the shape, the same every frame, for `Backdrop` |
| `Backdrop::read().luminance(n)` | how bright niri says the backdrop behind shape `n` is, 0–1, if it said |
| `amane::blur_fades()` | whether the compositor draws shapes exactly (with fade) |
| `amane::material_available(name)` | whether the compositor announced that material |

**Fallback.** On a compositor without the protocol, shapes are turned into an
`ext-background-effect` region: strips that step in at the corners, rounded
inwards. A region can't fade, so a shape less than half visible is left out.

**Materials.** niri announces its material names (and again on config reload).
When they change, every window sends its shapes again so material ids stay
correct. An unknown name draws plain blur.

**Backdrop readings.** Version 2 of the protocol adds a `backdrop(index,
luminance)` event, which niri sends for shapes whose material has
`report-backdrop` on. amane binds up to version 2, remembers the ids of the
shapes it last sent in order (`ShapeIds`, the object's user data), and puts
each reading under its shape's id in the `Backdrop` service. Reading the
service marks the view, so a new reading redraws it, and a reading that didn't
change redraws nothing. A shape without an id gets no readings, and on a
compositor that doesn't send them the service stays empty.

**Arrives with its buffer.** A skipped frame doesn't send a new blur with the
bare commit that requests the next frame; the last shown blur is put back, so
blur never runs ahead of the pixels it sits behind (`389fc29`).

(`src/blur.rs`, `src/services/backdrop.rs`, `src/wayland/blur_shapes.rs`, `src/wayland/update.rs`;
example: `examples/blur_shapes.rs`.)

## Vibrancy: `niri-vibrancy-v1`

`LayerWindow::vibrant(true)`, `Popup::vibrant(true)` and `Window::vibrant(true)` ask for a
window's vibrant content to blend with what the compositor frosted behind it, instead of being
painted over it: labels and fills that stay legible on any backdrop the material shows.

A vibrant window's draw commands are split three ways (`src/graphics/vibrancy.rs`):

- glyphs, rectangles, borders, fills and strokes in translucent white go on a **lighter**
  layer;
- ones in translucent black go on a **darker** layer;
- everything else stays on the window: color, opaque or off-white greys (a material's tint,
  the window's background, a switch's knob), images, gradients, shadows, blurs, cuts and
  shaders.

Groups and clips are kept around whatever of theirs goes on each layer. Each layer is a synced
`wl_subsurface` over the window (`src/wayland/vibrancy.rs`), with an empty input region and its
own `Gpu`. niri draws the lighter layer plus-lighter and the darker one plus-darker, which it
subtracts: the darker layer is drawn with its colors inverted, so it holds how far each pixel
darkens (a black label at 50% is drawn white at 50%, and takes 0.5 away). The layers
are drawn before the window, so they show in the window's own commit. A layer with nothing on
it is drawn empty once, then left alone.

The split goes by color rather than by what the content is, so white text on an accent disc
lands on the lighter layer, where adding it to the blue keeps it white. On the darker layer it
would darken the disc instead.

Without the protocol (or with `AMANE_NO_VIBRANCY=1`) the window draws everything itself, as
before. A window that stops being vibrant drops its layers.

## Drag release hook

`Rectangle::on_release(|pos| ..)` runs once the left button is let go after a
drag that started on the rectangle, wherever the pointer is then. A drag also
ends this way when the pointer leaves the window. While a drag is running, the
dragged target keeps the cursor shape until the drop (`2e9f8e0`). Before, a
resize handle that doesn't follow the pointer flipped the cursor on every
motion, and each `set_shape` made niri redraw every output.

(`src/widgets/rectangle/targets.rs`, `src/input/pointer.rs`.)
