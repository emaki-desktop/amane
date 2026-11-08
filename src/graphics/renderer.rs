mod command;
mod effect;
mod shape;
mod text;

use super::{Area, Color, Corners, Transform, image, vibrancy};
use crate::style::Effect;

pub use command::Command;
pub use effect::VALUE_ROWS;
pub use text::Placed;

/*
 * collects what widgets draw as commands,
 * so no part of amane outside the gpu backend knows how drawing is done
 */
pub struct Renderer {
    commands: Vec<Command>,
    transform: Transform,
}

impl Renderer {
    pub fn new(scale: f32) -> Self {
        // widgets measure in logical pixels, the buffer in real ones
        let horizontal_scale = scale;
        let vertical_scale = scale;

        let transform = Transform::from_scale(horizontal_scale, vertical_scale);

        Self {
            commands: Vec::new(),
            transform,
        }
    }

    pub fn finish(self) -> Vec<Command> {
        self.commands
    }

    // how many real pixels a logical one covers here, as drawn (zoom and rotation included)
    pub(crate) fn pixel_scale(&self) -> f32 {
        let t = &self.transform;
        f32::max(t.sx.hypot(t.ky), t.kx.hypot(t.sy))
    }

    pub fn group(&self) -> Self {
        Self {
            commands: Vec::new(),
            transform: self.transform,
        }
    }

    pub fn blend(&mut self, group: Renderer, opacity: f32) {
        // nothing shows, so nothing is drawn
        if opacity <= 0.0 || group.commands.is_empty() {
            return;
        }

        let cuts = group
            .commands
            .iter()
            .any(|command| matches!(command, Command::Cut { .. }));

        // a plain group draws the same inline, and only a separate group costs the gpu extra
        if opacity == 1.0 && !cuts {
            self.commands.extend(group.commands);

            return;
        }

        // one shape can't overlap itself, so its colour takes the opacity and no group is needed
        let mut commands = group.commands;

        if let [only] = commands.as_mut_slice()
            && fade(only, opacity)
        {
            self.commands.append(&mut commands);

            return;
        }

        self.commands.push(Command::Group { commands, opacity });
    }

    // what the group drew only shows inside the rounded rectangle
    pub fn clip(&mut self, group: Renderer, area: Area, radius: Corners) {
        // an empty rectangle shows none of the group
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }

        self.commands.push(Command::Clip {
            area,
            radius,
            transform: self.transform,
            commands: group.commands,
        });
    }

    // what the group drew goes through the effect's passes, which cover the area and its margin
    pub fn effect(&mut self, group: Renderer, area: Area, effect: &Effect) {
        if effect.passes.is_empty() || area.width <= 0.0 || area.height <= 0.0 {
            self.commands.extend(group.commands);

            return;
        }

        // loaded here, so a window waiting on a decode is the one woken when it's done
        let images = effect
            .images()
            .map(|(path, blur)| image::load(path, None, blur))
            .collect();

        self.commands.push(Command::Effect {
            commands: group.commands,
            effect: effect.clone(),
            images,
            area,
            transform: self.transform,
        });
    }

    // everything drawn inside goes through the local transform before the renderer's own
    pub fn transformed(&mut self, local: Transform, draw: impl FnOnce(&mut Renderer)) {
        let outer = self.transform;

        self.transform = local.post_concat(outer);

        draw(self);

        self.transform = outer;
    }
}

/*
 * a single shape faded by its own colour, the way a group at `opacity` would
 * show it; false (and untouched) for one that can't be, or whose faded colour
 * would land on another vibrancy layer
 */
fn fade(command: &mut Command, opacity: f32) -> bool {
    if opacity > 1.0 {
        return false;
    }

    let (Command::Rectangle { color, .. }
    | Command::Border { color, .. }
    | Command::Glyph { color, .. }
    | Command::Fill { color, .. }
    | Command::Stroke { color, .. }
    | Command::Shadow { color, .. }) = command
    else {
        return false;
    };

    let alpha = (f32::from(color.alpha()) * opacity).round() as u8;
    let faded = Color::rgba(color.red(), color.green(), color.blue(), alpha);

    if !vibrancy::same_layer(*color, faded) {
        return false;
    }

    *color = faded;

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::ScaleFactor;
    use crate::{Color, Parent, Rectangle};

    #[test]
    fn frames_combine_global_and_output_scales_in_drawing() {
        let root = Rectangle::new()
            .width(Parent)
            .height(Parent)
            .fill(Color::BLUE);
        let frame = crate::frame::build(&root, 300, 150, 1.25, ScaleFactor::new(1.5));
        let commands = frame.renderer.finish();
        let Command::Rectangle {
            area, transform, ..
        } = commands[0]
        else {
            panic!("expected a rectangle");
        };

        assert_eq!(area, Area::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(transform.map(area.width, area.height), (375.0, 187.5));
    }

    fn drawn(root: &Rectangle) -> Vec<Command> {
        crate::frame::build(root, 100, 100, 1.0, ScaleFactor::new(1.0))
            .renderer
            .finish()
    }

    #[test]
    fn invisible_draws_nothing() {
        let root = Rectangle::new()
            .width(Parent)
            .height(Parent)
            .fill(Color::BLUE)
            .opacity(0.0);

        assert!(drawn(&root).is_empty());
    }

    #[test]
    fn one_faded_shape_takes_the_opacity_in_its_colour() {
        let root = Rectangle::new()
            .width(Parent)
            .height(Parent)
            .fill(Color::rgb(0, 0, 255))
            .opacity(0.5);

        let commands = drawn(&root);
        let [Command::Rectangle { color, .. }] = commands.as_slice() else {
            panic!("expected one rectangle, no group");
        };

        assert_eq!(color.alpha(), 128);
    }

    #[test]
    fn white_that_would_turn_vibrant_stays_a_group() {
        // opaque white is the window's, translucent white the lighter layer's
        let root = Rectangle::new()
            .width(Parent)
            .height(Parent)
            .fill(Color::rgb(255, 255, 255))
            .opacity(0.4);

        assert!(matches!(drawn(&root).as_slice(), [Command::Group { .. }]));
    }
}
