use std::time::Instant;

use smithay_client_toolkit::compositor::FrameCallbackData;

use crate::WindowState;
use crate::frame;
use crate::graphics::{Command, Renderer, vibrancy};
use crate::timing::{self, Timing};

use super::surface::{OpenWindow, Role};
use super::vibrancy::VibrantLayers;

impl OpenWindow {
    // one frame: run the view, send its settings, draw it and show it
    pub fn redraw(&mut self) {
        let started = Instant::now();

        // layer windows and popups get the empty state
        let state = match &self.role {
            Role::Normal { state, .. } => *state,
            _ => WindowState::default(),
        };

        crate::window::set_state(state);

        let (content, reads) = frame::run_view(
            || self.view.run(),
            self.width,
            self.height,
            self.scale_factor,
        );

        // a later change to one of these services draws this window again
        self.reads = reads;

        // a frozen window that shows keeps its last frame, with what was sent with it
        if content.frozen() && self.width != 0 && self.height != 0 {
            return;
        }

        let viewed = Instant::now();

        let name = &content.name();

        // what the compositor blurs behind the last frame shown, put back if this one is skipped
        let shown_blur = self.blur_sent();
        let shown_input = self.input_region.clone();

        let unsent = self.update_surface(&content);

        /*
         * whether this frame sent the compositor anything that only takes effect with a commit:
         * a layer window's settings, input region or blur; a normal window or popup is always
         * taken to have (what it sends isn't tracked)
         */
        let sent = unsent
            || !matches!(self.role, Role::Layer { .. })
            || self.input_region != shown_input
            || self.blur_sent() != shown_blur;

        // 0 until the compositor configures the window, and again while it is hidden
        if self.width == 0 || self.height == 0 {
            return;
        }

        let vibrant = content.vibrant();

        let (root, keys, on_close) = content.into_parts();

        // a window without a child has nothing to show, which is a mistake but not a crash
        let Some(root) = root else {
            eprintln!("amane: {name} has no child to draw, see child()");

            if unsent {
                self.role.commit();
            }

            return;
        };

        let frame = frame::build(
            root.as_ref(),
            self.width,
            self.height,
            self.scale,
            self.scale_factor,
        );

        // some widgets read services while drawing
        self.reads.extend(frame.reads);

        // the handlers are rebuilt with the view, so each frame replaces the last frame's
        self.pointer.set_targets(frame.targets);

        self.keys = keys;
        self.on_close = on_close;

        /*
         * an animation that has not arrived yet needs the next frame too; asking
         * in the same commit as this frame gets the answer on the next refresh,
         * a hidden window never gets here so it waits until it shows again
         */
        if frame.moving {
            self.ask_for_frame();
        }

        let drawn = Instant::now();

        let presented = self.present(frame.renderer, vibrant, frame.moving, sent);

        let timing = Timing {
            started,
            viewed,
            drawn,
            presented: Instant::now(),
        };

        timing::record(name, self.last_frame, &timing);
        timing::log(
            name,
            self.width,
            self.height,
            self.last_frame,
            &timing,
            self.gpu.vello_runs(),
        );

        self.last_frame = Some(started);

        if !presented {
            timing::skipped(name);

            /*
             * the blur this frame asked for goes with its buffer: the next commit
             * would otherwise move the blur under the last frame's content
             */
            self.restore_blur(shown_blur);

            // a skipped frame commits nothing, so the request and settings go out on their own
            if frame.moving || unsent {
                self.role.commit();
            }
        }
    }

    pub fn request_frame(&mut self) {
        /*
         * a hidden or unconfigured window gets no frame callbacks,
         * so the view runs right away to see if the window should show
         */
        if self.width == 0 {
            self.redraw();

            return;
        }

        // changes that land before the next frame all draw together in it
        if self.frame_requested {
            return;
        }

        self.ask_for_frame();

        self.role.commit();
    }

    // the compositor answers once, when the next frame is due
    fn ask_for_frame(&mut self) {
        if self.frame_requested {
            return;
        }

        self.frame_requested = true;

        let surface = self.role.wl_surface();

        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
    }

    /*
     * false when the gpu skipped the frame, so nothing was committed; the window
     * is measured in logical pixels, the buffer in real ones, and the scale goes
     * out with the commit that presenting the frame makes; a frame the same
     * as the one on screen commits only for what was sent with it (`sent`) or
     * a vibrant layer that drew: an empty commit still wakes the compositor
     */
    fn present(&mut self, renderer: Renderer, vibrant: bool, moving: bool, sent: bool) -> bool {
        let (buffer_width, buffer_height) = match &self.fractional {
            // a buffer at the exact scale, shown back at the window's size
            Some(fractional) => {
                let width = self.width as i32;
                let height = self.height as i32;

                fractional.viewport.set_destination(width, height);

                (
                    to_real_pixels(self.width, self.scale),
                    to_real_pixels(self.height, self.scale),
                )
            }

            None => {
                self.role.wl_surface().set_buffer_scale(self.scale as i32);

                (
                    self.width * self.scale as u32,
                    self.height * self.scale as u32,
                )
            }
        };

        let commands = renderer.finish();

        let (commands, layers_drew) =
            self.draw_vibrant_layers(commands, vibrant, moving, (buffer_width, buffer_height));

        if !moving && self.gpu.shows(&commands, buffer_width, buffer_height) {
            // the layers are synced subsurfaces: what they drew shows with this commit
            if sent || layers_drew {
                self.role.commit();
            }

            return true;
        }

        self.gpu.draw(commands, buffer_width, buffer_height)
    }

    /*
     * a vibrant window's light and dark grey content goes on its layers, drawn
     * first: they're synced, so they show with the window's own commit; what's
     * left is the window's, and whether a layer changed (so that commit is due).
     * Without niri-vibrancy everything stays on the window
     */
    fn draw_vibrant_layers(
        &mut self,
        commands: Vec<Command>,
        vibrant: bool,
        moving: bool,
        buffer: (u32, u32),
    ) -> (Vec<Command>, bool) {
        let Some(kit) = &self.vibrancy_kit else {
            return (commands, false);
        };

        // a window that stops being vibrant drops its layers, and shows all on itself again
        if !vibrant {
            let had = self.vibrant_layers.take().is_some();

            return (commands, had);
        }

        let parent = self.role.wl_surface().clone();
        let layers = self
            .vibrant_layers
            .get_or_insert_with(|| VibrantLayers::new(kit, &parent, &self.qh));

        let split = vibrancy::split(commands);

        let size = (self.width, self.height);
        let whole_scale = self.fractional.is_none().then_some(self.scale as i32);

        let lighter = layers
            .lighter
            .draw(split.lighter, size, buffer, whole_scale, moving);
        let darker = layers
            .darker
            .draw(split.darker, size, buffer, whole_scale, moving);

        (split.window, lighter || darker)
    }
}

// a logical size in real pixels at a fractional scale
fn to_real_pixels(size: u32, scale: f32) -> u32 {
    let pixels = size as f32 * scale;

    pixels.round() as u32
}
