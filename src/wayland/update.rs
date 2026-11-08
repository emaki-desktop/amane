use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::wl_region::{self, WlRegion},
};

use crate::scale::ScaleFactor;
use crate::{BlurShape, InputArea, LayerWindow};

use super::{
    WaylandState,
    background_effect::BlurEffect,
    blur_shapes::{BlurShapes, ShapeIds},
    layer::{self, Settings},
    surface::{Content, OpenWindow, Role},
};

// the protocol object for a window's surface, none when the compositor lacks the protocol
fn make_blur_effect(window: &OpenWindow) -> Option<BlurEffect> {
    let manager = window.background_effect.as_ref()?;

    let surface = manager.get_background_effect(window.role.wl_surface(), &window.qh, ());

    Some(BlurEffect { surface })
}

impl OpenWindow {
    /*
     * settings changed by input or services go to the compositor before anything is drawn;
     * true when they wait for the frame's commit, so position, content and blur move together
     */
    pub fn update_surface(&mut self, content: &Content) -> bool {
        if let Content::Normal(window) = content {
            self.update_normal(window);

            return false;
        }

        let Content::Layer(window) = content else {
            return false;
        };

        // a window made once can be asked onto another monitor, and is made again there
        if matches!(self.view, super::surface::View::Plain(_))
            && window.output.is_some()
            && window.output != self.on_output
        {
            self.move_to = window.output.clone();
        }

        self.update_input_region(window);
        self.update_blur_region(window);

        // the compositor sizes lock screens itself, and never lets them hide
        let Role::Layer {
            surface: layer_surface,
            settings: current,
        } = &mut self.role
        else {
            return false;
        };

        let settings = Settings::from(window);

        if settings == *current {
            return false;
        }

        let was_visible = current.visible;

        *current = settings;

        // the rest waits until the window shows again, which sends every setting
        if !settings.visible {
            if was_visible {
                self.hide();
            }

            return false;
        }

        layer::apply(layer_surface, &settings, self.scale_factor);

        // a window that draws now sends them with its frame
        if self.width != 0 && self.height != 0 {
            return true;
        }

        /*
         * a commit without a buffer also shows a hidden window again,
         * the compositor answers with a configure and drawing starts there
         */
        self.role.commit();

        false
    }

    // only a changed region is sent, it takes effect with the next commit like the rest
    pub fn update_input_region(&mut self, window: &LayerWindow) {
        if window.input_region == self.input_region {
            return;
        }

        self.input_region = window.input_region.clone();

        let surface = self.role.wl_surface();

        // no region set means the whole window takes the pointer again
        let Some(areas) = &self.input_region else {
            surface.set_input_region(None);

            return;
        };

        let region = self.compositor.create_region(&self.qh, ());

        for area in areas {
            let area = scaled_region(*area, self.scale_factor);
            region.add(area.x, area.y, area.width, area.height);
        }

        surface.set_input_region(Some(&region));

        // the surface keeps its own copy, so the region can go right away
        region.destroy();
    }

    /*
     * shapes go to the compositor as they are where it has them, and as a
     * region of strips where it doesn't; a plain region goes as before
     */
    pub fn update_blur_region(&mut self, window: &LayerWindow) {
        let factor = self.scale_factor;

        if let Some(shapes) = &window.blur_shapes {
            if self.blur_shapes_manager.is_some() {
                self.send_blur_shapes(shapes);

                return;
            }

            self.send_blur_region(Some(crate::blur::strips(shapes, factor.get())));

            return;
        }

        // the region applies again once the shapes are gone
        if self.blur_shapes_object.take().is_some() {
            self.blur_shapes = None;
        }

        let region = window.blur_region.as_ref().map(|areas| {
            areas
                .iter()
                .map(|area| scaled_region(*area, factor))
                .collect()
        });

        self.send_blur_region(region);
    }

    // what was last sent of the blur region and shapes
    pub fn blur_sent(&self) -> BlurSent {
        BlurSent {
            region: self.blur_region.clone(),
            shapes: self.blur_shapes.as_ref().map(|(shapes, _)| shapes.clone()),
        }
    }

    // sends `sent` again where it differs from what went out since, so the next commit keeps it
    pub fn restore_blur(&mut self, sent: BlurSent) {
        if sent.region != self.blur_region {
            self.send_blur_region(sent.region);
        }

        let current = self.blur_shapes.as_ref().map(|(shapes, _)| shapes);

        if sent.shapes.as_ref() == current {
            return;
        }

        match sent.shapes {
            Some(shapes) => self.send_blur_shapes(&shapes),

            // there were no shapes, so the region applies again
            None => {
                self.blur_shapes_object = None;
                self.blur_shapes = None;
            }
        }
    }

    // only changed shapes are sent, or the same ones once the materials change
    fn send_blur_shapes(&mut self, shapes: &[BlurShape]) {
        let materials = crate::blur::materials_changed();

        if self
            .blur_shapes
            .as_ref()
            .is_some_and(|(sent, at)| sent == shapes && *at == materials)
        {
            return;
        }

        let Some(manager) = &self.blur_shapes_manager else {
            return;
        };

        let object = self.blur_shapes_object.get_or_insert_with(|| BlurShapes {
            shapes: manager.get_blur_shapes(self.role.wl_surface(), &self.qh, ShapeIds::default()),
        });

        let factor = f64::from(self.scale_factor.get());

        object.shapes.reset();

        for shape in shapes {
            let radius = f64::from(shape.radius) * factor;

            object.shapes.add_rounded_rect(
                f64::from(shape.x) * factor,
                f64::from(shape.y) * factor,
                f64::from(shape.width) * factor,
                f64::from(shape.height) * factor,
                radius,
                radius,
                radius,
                radius,
                f64::from(shape.alpha),
                shape
                    .material
                    .and_then(crate::blur::material_id)
                    .unwrap_or(0),
                shape.tint.map_or(0, crate::blur::premultiplied),
            );
        }

        if let Some(ids) = object.shapes.data::<ShapeIds>() {
            *ids.0.lock().unwrap_or_else(|e| e.into_inner()) =
                shapes.iter().map(BlurShape::shape_id).collect();
        }

        self.blur_shapes = Some((shapes.to_vec(), materials));
    }

    // only a changed region is sent, it takes effect with the next commit like the rest
    fn send_blur_region(&mut self, region: Option<Vec<InputArea>>) {
        if region == self.blur_region {
            return;
        }

        // a window that never asks keeps the compositor's rules, without the protocol object
        if self.blur_effect.is_none() {
            if region.is_none() {
                return;
            }

            self.blur_effect = make_blur_effect(self);
        }

        self.blur_region = region;

        let Some(effect) = &self.blur_effect else {
            return;
        };

        // no region set gives the choice back to the compositor's rules
        let Some(areas) = &self.blur_region else {
            effect.surface.set_blur_region(None);

            return;
        };

        let region = self.compositor.create_region(&self.qh, ());

        for area in areas {
            region.add(area.x, area.y, area.width, area.height);
        }

        effect.surface.set_blur_region(Some(&region));

        // the surface keeps its own copy, so the region can go right away
        region.destroy();
    }

    fn hide(&mut self) {
        // taking the buffer away unmaps the window and gives back its reserved space
        self.role.wl_surface().attach(None, 0, 0);

        self.role.commit();

        // showing again needs a buffer, even for the same frame
        self.gpu.forget_shown();

        // a hidden window holds nothing on the gpu; showing again makes it all afresh
        self.gpu.release();
        self.vibrant_layers = None;

        // the freed canvas and decode buffers go back to the system, not just to the pools
        crate::allocator::trim_soon();

        // nothing is drawn until showing the window brings a new configure
        self.width = 0;
        self.height = 0;

        // a frame callback asked for before hiding may never come
        self.frame_requested = false;
    }
}

// the blur a window last sent, to put back when a frame doesn't reach the screen
#[derive(Clone, PartialEq)]
pub struct BlurSent {
    region: Option<Vec<InputArea>>,
    shapes: Option<Vec<BlurShape>>,
}

// round outward so a fractional scale does not leave the edge of a clickable area out
fn scaled_region(area: InputArea, scale_factor: ScaleFactor) -> InputArea {
    let factor = f64::from(scale_factor.get());
    let x = (f64::from(area.x) * factor).floor() as i32;
    let y = (f64::from(area.y) * factor).floor() as i32;
    let right = ((f64::from(area.x) + f64::from(area.width)) * factor).ceil() as i32;
    let bottom = ((f64::from(area.y) + f64::from(area.height)) * factor).ceil() as i32;

    InputArea {
        x,
        y,
        width: if area.width > 0 {
            right.saturating_sub(x)
        } else {
            0
        },
        height: if area.height > 0 {
            bottom.saturating_sub(y)
        } else {
            0
        },
    }
}

// a region never sends events, but wayland-client still needs somewhere to send them
impl Dispatch<WlRegion, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_input_regions_cover_their_edges_and_preserve_empty_areas() {
        let area = InputArea {
            x: 1,
            y: -1,
            width: 3,
            height: 2,
        };

        assert_eq!(scaled_region(area, ScaleFactor::default()), area);
        assert_eq!(
            scaled_region(area, ScaleFactor::new(1.5)),
            InputArea {
                x: 1,
                y: -2,
                width: 5,
                height: 4
            },
        );

        let empty = InputArea {
            width: 0,
            height: 0,
            ..area
        };
        let scaled = scaled_region(empty, ScaleFactor::new(1.5));
        assert_eq!((scaled.width, scaled.height), (0, 0));
    }
}
