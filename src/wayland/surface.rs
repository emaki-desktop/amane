use std::collections::HashSet;
use std::time::Instant;

use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
use smithay_client_toolkit::{
    session_lock::SessionLockSurface,
    shell::{
        WaylandSurface, wlr_layer::LayerSurface, xdg::popup::Popup as SctkPopup,
        xdg::window::Window as XdgWindow,
    },
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_compositor::WlCompositor, wl_output::WlOutput, wl_surface::WlSurface},
};

use crate::changes::Source;
use crate::graphics::WindowGpu;
use crate::input::{KeyHandlers, Pointer};
use crate::popup::PopupView;
use crate::scale::ScaleFactor;
use crate::window::{CloseHandler, WindowView};
use crate::{BlurShape, InputArea, LayerWindow, Monitor, Popup, Widget, Window, WindowState};

use super::{
    WaylandState,
    background_effect::BlurEffect,
    blur_shapes::{BlurShapes, protocol::niri_blur_shapes_manager_v1::NiriBlurShapesManagerV1},
    layer::{self, Settings},
    normal::{Dialog, NormalSettings},
    scale::Fractional,
    vibrancy::{VibrancyKit, VibrantLayers},
};

// one surface on screen, with everything it needs to draw and take input
pub struct OpenWindow {
    pub view: View,

    // only windows made per monitor are tied to one, the rest let the compositor choose
    pub output: Option<WlOutput>,

    // the monitor it shows on by name, once known: its own, or the one the compositor chose
    pub on_output: Option<String>,

    // the monitor its view asked to show on instead, so it's made again there
    pub move_to: Option<String>,

    // the input region last sent, so only a real change is sent again
    pub input_region: Option<Vec<InputArea>>,

    // the blur region last sent (in surface coordinates), the same way
    pub blur_region: Option<Vec<InputArea>>,

    // the blur shapes last sent, with the materials they were sent against
    pub blur_shapes: Option<(Vec<BlurShape>, u64)>,

    pub width: u32,
    pub height: u32,

    pub scale: f32,

    pub scale_factor: ScaleFactor,

    pub frame_requested: bool,

    // when the last frame started drawing, for AMANE_FRAMES
    pub last_frame: Option<Instant>,

    // the services the last view read, a change to any of them draws the window again
    pub reads: HashSet<Source>,

    // both come from the last drawn view, so input matches what is on screen
    pub pointer: Pointer,
    pub keys: KeyHandlers,

    // a normal window's, from the last drawn view, asked before the user closes it
    pub on_close: Option<CloseHandler>,

    // a vibrant window's layers over it, made the first time it draws; they go before its surface
    pub vibrant_layers: Option<VibrantLayers>,

    // the gpu draws into the surface, so it has to go first when both are dropped
    pub gpu: WindowGpu,

    // none at whole scales; dropped before the role, since it belongs to its surface
    pub fractional: Option<Fractional>,

    // made once a view asks for a blur region; dropped before the role too
    pub blur_effect: Option<BlurEffect>,

    // made once a view asks for blur shapes where there are any; dropped before the role too
    pub blur_shapes_object: Option<BlurShapes>,

    pub role: Role,

    // kept to make input regions, which come from the compositor
    pub compositor: WlCompositor,

    // kept to ask for a blur region, none when the compositor lacks the protocol
    pub background_effect: Option<ExtBackgroundEffectManagerV1>,

    // kept to ask for blur shapes, none when the compositor lacks them
    pub blur_shapes_manager: Option<NiriBlurShapesManagerV1>,

    // kept to make the vibrant layers, none when the compositor lacks niri-vibrancy
    pub vibrancy_kit: Option<VibrancyKit>,

    pub qh: QueueHandle<WaylandState>,
}

// what a window is to the compositor: a layer like a bar, one screen of the session lock, or a normal window
pub enum Role {
    Layer {
        surface: LayerSurface,

        // what the compositor was last told, so only a real change is sent again
        settings: Settings,
    },

    Lock(SessionLockSurface),

    Normal {
        // over its parent, only held so it is destroyed before the window it belongs to
        #[allow(dead_code)]
        dialog: Option<Dialog>,

        window: XdgWindow,

        // the size it last had, kept for as long as the compositor leaves the size to it
        size: (u32, u32),

        // what the compositor was last told, so only a real change is sent again
        settings: NormalSettings,

        // what the compositor last said, handed to the view as window_state()
        state: WindowState,
    },

    // the input method's popup by the focused field in another app, placed by the compositor
    InputPopup(super::input_method_v2::InputPopupSurface),

    Popup {
        popup: SctkPopup,

        // the slot's namespace, matching the Popup builder's namespace
        namespace: &'static str,

        // the parent surface's namespace, so descendants can be torn down
        // child-first (xdg-shell requires reverse-creation-order destroy)
        parent: Option<&'static str>,

        // the normal window it opened over, so closing that window closes it first
        window: Option<String>,

        // a grabbing popup, like a menu, takes the keys while it is open
        grab: bool,
    },
}

// what a window runs on every redraw to find out what it shows
pub enum View {
    Plain(fn() -> LayerWindow),

    // the monitor is replaced when the compositor reports a change to it
    Monitor(fn(&Monitor) -> LayerWindow, Monitor),

    // the name tells it apart from other normal windows
    Normal(String, WindowView),

    // an xdg-popup; the view returns a Popup with .visible(bool) each frame
    Popup(PopupView),
}

pub type Parts = (Option<Box<dyn Widget>>, KeyHandlers, Option<CloseHandler>);

// what a view gave back
pub enum Content {
    Layer(LayerWindow),

    Normal(Window),

    Popup(Popup),
}

impl OpenWindow {
    // a size of 0 leaves the choice to the window, which then keeps the size it asked for
    pub fn resize(&mut self, width: u32, height: u32) {
        let (asked_width, asked_height) = self.asked_size();

        self.width = match width {
            0 => asked_width,
            width => width,
        };

        self.height = match height {
            0 => asked_height,
            height => height,
        };

        self.redraw();
    }

    fn asked_size(&self) -> (u32, u32) {
        match &self.role {
            Role::Layer { settings, .. } => {
                let width = layer::to_pixels(settings.width, self.scale_factor);
                let height = layer::to_pixels(settings.height, self.scale_factor);

                (width, height)
            }

            Role::Normal { size, .. } => *size,

            // the compositor always gives a lock screen its monitor's size
            Role::Lock(_) => (0, 0),

            // popups are configured with explicit sizes by the compositor
            Role::Popup { .. } => (0, 0),

            // the view gives its size, the compositor only places it
            Role::InputPopup(_) => (0, 0),
        }
    }
}

impl Role {
    pub fn wl_surface(&self) -> &WlSurface {
        match self {
            Role::Layer { surface, .. } => surface.wl_surface(),
            Role::Lock(lock_surface) => lock_surface.wl_surface(),
            Role::Normal { window, .. } => window.wl_surface(),
            Role::Popup { popup, .. } => popup.wl_surface(),
            Role::InputPopup(popup) => &popup.surface,
        }
    }

    pub fn commit(&self) {
        self.wl_surface().commit();
    }
}

impl View {
    pub fn run(&self) -> Content {
        match self {
            View::Plain(view) => Content::Layer(view()),
            View::Monitor(view, monitor) => Content::Layer(view(monitor)),
            View::Normal(key, view) => {
                crate::window::set_current(key);

                Content::Normal(view())
            }
            View::Popup(view) => Content::Popup(view()),
        }
    }

    pub fn shows(&self, name: &str) -> bool {
        let View::Normal(shown, _) = self else {
            return false;
        };

        shown == name
    }
}

impl Content {
    // the name AMANE_FRAMES logs the window under
    pub fn name(&self) -> String {
        match self {
            Content::Layer(window) => String::from(window.namespace),
            Content::Normal(_) => format!("normal {}", crate::window::current_window()),
            Content::Popup(popup) => String::from(popup.namespace),
        }
    }

    // whether its grey content is blended with what's behind it
    pub fn vibrant(&self) -> bool {
        match self {
            Content::Layer(window) => window.vibrant,
            Content::Normal(window) => window.vibrant,
            Content::Popup(popup) => popup.vibrant,
        }
    }

    // whether the last frame stays up, see LayerWindow::frozen
    pub fn frozen(&self) -> bool {
        match self {
            Content::Layer(window) => window.frozen,
            Content::Normal(_) | Content::Popup(_) => false,
        }
    }

    // the widgets and the handlers, drawn and used the same way by every kind of window
    pub fn into_parts(self) -> Parts {
        match self {
            Content::Layer(window) => (window.root, window.keys, None),
            Content::Normal(window) => (window.root, window.keys, window.on_close),
            Content::Popup(popup) => (popup.root, popup.keys, None),
        }
    }
}
