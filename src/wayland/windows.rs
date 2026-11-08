use std::collections::HashSet;

use wayland_client::{
    Proxy,
    protocol::{wl_output::WlOutput, wl_surface::WlSurface},
};

use crate::graphics::WindowGpu;
use crate::input::{KeyHandlers, Pointer};
use crate::{changes, frame};

use super::{
    WaylandState,
    layer::{self, Settings},
    surface::{Content, OpenWindow, Role, View},
};

impl WaylandState {
    pub fn open(&mut self, view: View, output: Option<WlOutput>) {
        let surface = self.compositor.create_surface(&self.qh);

        /*
         * later views can change the settings, each redraw compares them with the
         * last ones; a window that starts hidden never gets a configure, so it never
         * draws, and what its first view read is all that can wake it to show
         */
        let (content, reads) = frame::run_view(|| view.run(), 0, 0, self.scale_factor);

        // normal windows open in open_normal
        let Content::Layer(window) = content else {
            unreachable!("only layer views open as layer windows");
        };

        let settings = Settings::from(&window);

        let layer_surface = layer::create(
            &self.layer_shell,
            surface,
            output.as_ref(),
            &self.qh,
            &settings,
            self.scale_factor,
        );

        let role = Role::Layer {
            surface: layer_surface,
            settings,
        };

        self.add(view, output, role);

        if let Some(window) = self.windows.last_mut() {
            window.reads = reads;
        }
    }

    // everything a window needs besides its surface is the same for layers and lock screens
    pub fn add(&mut self, view: View, output: Option<WlOutput>, role: Role) {
        // the gpu draws straight into the surface, so it gets libwayland's own pointers
        let display = self.connection.backend().display_ptr().cast();
        let surface = role.wl_surface().id().as_ptr().cast();

        let gpu = WindowGpu::new(display, surface);

        let fractional = self.make_fractional(role.wl_surface());

        let on_output = output.as_ref().and_then(|output| self.output_name(output));

        let window = OpenWindow {
            view,

            output,
            on_output,
            move_to: None,

            input_region: None,
            blur_region: None,
            blur_shapes: None,

            width: 0,
            height: 0,

            scale: 1.0,

            scale_factor: self.scale_factor,

            frame_requested: false,
            last_frame: None,
            reads: HashSet::new(),

            pointer: Pointer::default(),
            keys: KeyHandlers::default(),
            on_close: None,

            gpu,

            fractional,
            blur_effect: None,
            blur_shapes_object: None,
            vibrant_layers: None,

            role,

            compositor: self.compositor.wl_compositor().clone(),
            background_effect: self.background_effect.clone(),
            blur_shapes_manager: self.blur_shapes.clone(),
            vibrancy_kit: self.vibrancy_kit.clone(),

            qh: self.qh.clone(),
        };

        self.windows.push(window);
    }

    // wayland events name the surface they are about, this finds its window
    pub fn window(&mut self, surface: &WlSurface) -> Option<&mut OpenWindow> {
        self.windows
            .iter_mut()
            .find(|window| window.role.wl_surface() == surface)
    }

    // a service or handler may have changed what any of the windows shows
    pub fn request_frames(&mut self) {
        for window in &mut self.windows {
            window.request_frame();
        }
    }

    // input only changes the window it arrived in, besides the services it wrote
    pub fn request_frame_on(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window(surface) {
            window.request_frame();
        }
    }

    /*
     * the services some open window's last frame read: a polling service keeps
     * polling while one is open, and stops when none is (services::store)
     */
    pub fn publish_watched(&self) {
        let read: std::collections::HashSet<changes::Source> = self
            .windows
            .iter()
            .flat_map(|window| window.reads.iter().copied())
            .collect();

        let watched = read
            .iter()
            .filter_map(|source| match source {
                changes::Source::Service(id) => Some(*id),
                _ => None,
            })
            .collect();

        crate::services::store::set_watched(watched);
        changes::set_read_by_windows(read);
    }

    // only windows that read a service that changed draw again
    pub fn request_changed_frames(&mut self) {
        let Some(changed) = changes::take() else {
            self.request_frames();

            return;
        };

        for window in &mut self.windows {
            if window.reads.is_disjoint(&changed) {
                continue;
            }

            window.request_frame();
        }
    }

    // the popups opened straight over this normal or layer window
    fn popups_over(&self, surface: &WlSurface) -> Vec<&'static str> {
        let Some(closing) = self
            .windows
            .iter()
            .find(|window| window.role.wl_surface() == surface)
        else {
            return Vec::new();
        };

        let mut popups = Vec::new();

        for window in &self.windows {
            let Role::Popup {
                namespace,
                parent,
                window: parent_window,
                ..
            } = &window.role
            else {
                continue;
            };

            let over = match (&closing.role, &closing.view) {
                (Role::Normal { .. }, View::Normal(key, _)) => parent_window.as_ref() == Some(key),
                (Role::Layer { settings, .. }, _) => {
                    parent_window.is_none() && *parent == Some(settings.namespace)
                }
                _ => false,
            };

            if over {
                popups.push(*namespace);
            }
        }

        popups
    }

    // opens again the app windows the compositor closed, letting it choose the monitor
    pub fn output_name(&self, output: &WlOutput) -> Option<String> {
        self.output.info(output)?.name
    }

    /*
     * a window made once whose view asked for another monitor is made again
     * there, popups over it first: a layer surface keeps the monitor it was
     * made on
     */
    pub fn move_windows(&mut self) {
        while let Some(index) = self
            .windows
            .iter()
            .position(|window| window.move_to.is_some())
        {
            let name = self.windows[index].move_to.take().unwrap_or_default();

            let Some(output) = self
                .output
                .outputs()
                .find(|output| self.output_name(output).as_deref() == Some(name.as_str()))
            else {
                // no such monitor now; it stays, and asks again with its next frame
                continue;
            };

            let surface = self.windows[index].role.wl_surface().clone();

            for namespace in self.popups_over(&surface) {
                self.close_popup_cascade(namespace);
            }

            let Some(index) = self
                .windows
                .iter()
                .position(|window| window.role.wl_surface() == &surface)
            else {
                continue;
            };

            // the old surface goes before the new one is made
            let view = {
                let window = self.windows.remove(index);

                window.view
            };

            self.open(view, Some(output));
        }
    }

    pub fn reopen_closed(&mut self) {
        for view in std::mem::take(&mut self.closed_views) {
            self.open(View::Plain(view), None);
        }
    }

    pub fn close(&mut self, surface: &WlSurface) {
        // xdg-shell wants a window's popups gone before the window itself
        for namespace in self.popups_over(surface) {
            self.close_popup_cascade(namespace);
        }

        self.windows
            .retain(|window| window.role.wl_surface() != surface);

        // windows made per monitor, or closed with their monitor, come back when one is plugged in
        if self.windows.is_empty()
            && self.per_monitor.is_empty()
            && self.closed_views.is_empty()
            && !self.keep_alive
        {
            self.running = false;
        }
    }
}
