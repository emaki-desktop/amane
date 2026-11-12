#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::ffi::c_void;
use std::rc::Rc;

use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::subcompositor::SubcompositorState;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_compositor::WlCompositor, wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};

use crate::graphics::Command;
use crate::graphics::Gpu;

use super::WaylandState;

pub mod protocol {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]

    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/niri-vibrancy-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/niri-vibrancy-v1.xml");
}

use protocol::niri_vibrancy_manager_v1::{self, NiriVibrancyManagerV1};
use protocol::niri_vibrancy_v1::{self, Mode, NiriVibrancyV1};

/*
 * what a window needs to make its vibrant layers, none of it when the
 * compositor lacks niri-vibrancy or subsurfaces
 */
#[derive(Clone)]
pub struct VibrancyKit {
    pub manager: NiriVibrancyManagerV1,
    pub subcompositor: Rc<SubcompositorState>,
    pub compositor: WlCompositor,
    pub viewporter: Option<WpViewporter>,
    pub connection: Connection,
}

/*
 * a vibrant window's two layers over it: light grey content the compositor
 * adds to what's behind (plus-lighter), dark grey content it multiplies with
 * it (plus-darker); made the first time a vibrant window draws
 */
pub struct VibrantLayers {
    pub lighter: VibrantLayer,
    pub darker: VibrantLayer,
}

/*
 * one subsurface the size of its window, synced to it, so what it shows
 * changes in the same commit as the window; it takes no input
 */
pub struct VibrantLayer {
    // first to go, it draws into the surface
    gpu: Option<Gpu>,

    vibrancy: NiriVibrancyV1,
    viewport: Option<WpViewport>,
    subsurface: WlSubsurface,
    surface: WlSurface,

    // whether the last frame drawn showed anything, so an empty layer is drawn empty once
    showing: bool,
}

impl VibrantLayers {
    pub fn new(kit: &VibrancyKit, parent: &WlSurface, qh: &QueueHandle<WaylandState>) -> Self {
        Self {
            lighter: VibrantLayer::new(kit, parent, qh, Mode::PlusLighter),
            darker: VibrantLayer::new(kit, parent, qh, Mode::PlusDarker),
        }
    }
}

impl VibrantLayer {
    fn new(
        kit: &VibrancyKit,
        parent: &WlSurface,
        qh: &QueueHandle<WaylandState>,
        mode: Mode,
    ) -> Self {
        let (subsurface, surface) = kit.subcompositor.create_subsurface(parent.clone(), qh);

        // synced, so its frames show with the window's, over it
        subsurface.set_sync();
        subsurface.place_above(parent);

        // the window under it takes every click
        let none = kit.compositor.create_region(qh, ());
        surface.set_input_region(Some(&none));
        none.destroy();

        let vibrancy = kit.manager.get_vibrancy(&surface, qh, ());
        vibrancy.set_mode(mode);

        let viewport = kit
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, qh, ()));

        // the gpu draws straight into the surface, so it gets libwayland's own pointers
        let display: *mut c_void = kit.connection.backend().display_ptr().cast();
        let gpu = Gpu::new(display, surface.id().as_ptr().cast());

        Self {
            gpu: Some(gpu),
            vibrancy,
            viewport,
            subsurface,
            surface,
            showing: false,
        }
    }

    /*
     * draws the layer's commands at the window's size, the buffer at the
     * window's scale; a layer that would show what it shows already is left
     * as it is (synced, it keeps its buffer through the window's commit),
     * and one with nothing on it is unmapped and holds nothing; true when it
     * drew or unmapped, which shows with the window's next commit
     */
    pub fn draw(
        &mut self,
        commands: Vec<Command>,
        size: (u32, u32),
        buffer: (u32, u32),
        whole_scale: Option<i32>,
        moving: bool,
    ) -> bool {
        let showing = !commands.is_empty();

        // an empty layer that showed nothing has nothing on it to clear
        if !showing && !self.showing {
            return false;
        }

        /*
         * a layer with nothing on it now gives its buffer back rather than
         * drawing an empty one: unmapped, it shows nothing, and its gpu memory
         * (swapchain, canvas, spares) goes with it until it shows again
         */
        if !showing {
            self.surface.attach(None, 0, 0);
            self.surface.commit();

            if let Some(gpu) = &mut self.gpu {
                gpu.forget_shown();
                gpu.release();
            }

            self.showing = false;

            return true;
        }

        if !moving
            && self
                .gpu
                .as_ref()
                .is_some_and(|gpu| gpu.shows(&commands, buffer.0, buffer.1))
        {
            return false;
        }

        match (&self.viewport, whole_scale) {
            (Some(viewport), None) => viewport.set_destination(size.0 as i32, size.1 as i32),
            (_, Some(scale)) => self.surface.set_buffer_scale(scale),
            (None, None) => {}
        }

        let Some(gpu) = &mut self.gpu else {
            return false;
        };

        if gpu.draw(commands, buffer.0, buffer.1) {
            self.showing = showing;

            return true;
        }

        false
    }
}

// each has to go before the surface it belongs to, and the gpu before the surface too
impl Drop for VibrantLayer {
    fn drop(&mut self) {
        drop(self.gpu.take());

        self.vibrancy.destroy();

        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }

        self.subsurface.destroy();
        self.surface.destroy();
    }
}

// the manager and the objects never send events, but wayland-client still needs somewhere to send them
impl Dispatch<NiriVibrancyManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &NiriVibrancyManagerV1,
        _: niri_vibrancy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<NiriVibrancyV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &NiriVibrancyV1,
        _: niri_vibrancy_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
