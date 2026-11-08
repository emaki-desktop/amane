#![warn(clippy::unwrap_used, clippy::dbg_macro, clippy::todo, missing_docs)]

use std::collections::HashSet;
use std::mem;
use std::sync::{Arc, PoisonError};

use smithay_client_toolkit::reexports::protocols::xdg::shell::client::{
    xdg_positioner, xdg_surface,
};
use smithay_client_toolkit::shell::{
    WaylandSurface,
    xdg::{
        XdgPositioner, XdgSurface,
        popup::{Popup as SctkPopup, PopupConfigure, PopupHandler},
    },
};
use wayland_client::Proxy;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};

use crate::Popup;
use crate::graphics::WindowGpu;
use crate::input::{KeyHandlers, Pointer};
use crate::popup::{Anchor, ConstraintAdjustment, Gravity, OPENING, PopupView};

use super::{
    WaylandState,
    surface::{OpenWindow, Role, View},
};

// a popup view the event loop polls, from App::popup or from open_popup
pub struct PopupSlot {
    view: PopupView,

    // from open_popup: the slot goes once its popup has closed
    transient: bool,

    // seen open at least once, so a transient slot knows its popup has come and gone
    opened: bool,
}

impl PopupSlot {
    pub fn registered(view: fn() -> Popup) -> Self {
        Self {
            view: Arc::new(view),
            transient: false,
            opened: false,
        }
    }
}

impl WaylandState {
    // the popups open_popup asked for; one with the namespace of an open popup replaces it
    pub fn handle_popup_requests(&mut self) {
        let mut queue = OPENING.lock().unwrap_or_else(PoisonError::into_inner);

        let opening = mem::take(&mut *queue);

        drop(queue);

        for view in opening {
            let namespace = view().namespace;

            self.popup_views
                .retain(|slot| !(slot.transient && (slot.view)().namespace == namespace));

            self.close_popup_cascade(namespace);

            self.dismissed.retain(|dismissed| *dismissed != namespace);

            self.popup_views.push(PopupSlot {
                view,
                transient: true,
                opened: false,
            });
        }
    }

    /// Called every event-loop tick to reconcile popup slots: open the ones
    /// whose latest view said `.visible(true)` and no live surface exists,
    /// close the ones whose latest view said `.visible(false)`.
    pub fn reconcile_popups(&mut self) {
        if self.popup_views.is_empty() {
            return;
        }

        // Make a stable snapshot of the view fns before iterating (so a view
        // closure that mutates runtime doesn't invalidate indices).
        let views: Vec<PopupView> = self
            .popup_views
            .iter()
            .map(|slot| slot.view.clone())
            .collect();

        for view_fn in views {
            let popup = view_fn();
            let is_live = self.windows.iter().any(|w| {
                matches!(&w.role, Role::Popup { namespace, .. } if *namespace == popup.namespace)
            });

            let Some(slot) = self
                .popup_views
                .iter_mut()
                .find(|slot| Arc::ptr_eq(&slot.view, &view_fn))
            else {
                continue;
            };

            slot.opened |= is_live;

            // a transient popup that has closed, or that its view no longer wants, is done
            if slot.transient && !is_live && (slot.opened || !popup.visible) {
                self.popup_views
                    .retain(|slot| !Arc::ptr_eq(&slot.view, &view_fn));

                self.dismissed.retain(|ns| *ns != popup.namespace);

                continue;
            }

            match (popup.visible, is_live) {
                (true, false) => {
                    // A popup the compositor just dismissed stays closed until
                    // the app genuinely hides and re-shows it, so we don't
                    // reopen it (and re-grab with a stale serial) every tick.
                    if self.dismissed.contains(&popup.namespace) {
                        continue;
                    }
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!("popup open: ns={}", popup.namespace);
                    }
                    self.open_popup(view_fn, popup);
                }
                (false, true) => {
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!("popup close: ns={}", popup.namespace);
                    }
                    self.close_popup_cascade(popup.namespace);
                }
                (false, false) => {
                    // The view agrees the popup is down: re-arm opening so a
                    // later show after a compositor dismissal works again.
                    self.dismissed.retain(|ns| *ns != popup.namespace);
                }
                _ => {}
            }
        }
    }

    /// Close a popup and every popup descended from it, child-first.
    ///
    /// xdg-shell requires nested popups be destroyed in reverse creation
    /// order (deepest child before its parent). `reconcile_popups` and
    /// `done()` can trigger a parent's teardown while a child is still live
    /// (e.g. outside-click dismiss with a submenu open), so we gather the
    /// whole descendant subtree and drop it deepest-first.
    pub fn close_popup_cascade(&mut self, namespace: &'static str) {
        // BFS from `namespace` over the parent links; `order` ends up
        // breadth-first (shallowest first), so we drop it in reverse.
        let mut order: Vec<&'static str> = vec![namespace];
        let mut i = 0;
        while i < order.len() {
            let current = order[i];
            for w in &self.windows {
                if let Role::Popup {
                    namespace: ns,
                    parent: Some(p),
                    ..
                } = &w.role
                    && *p == current
                    && !order.contains(ns)
                {
                    order.push(*ns);
                }
            }
            i += 1;
        }

        // Drop deepest-first: each removal drops exactly one SctkPopup, so
        // the xdg_popup objects are destroyed in reverse creation order.
        for ns in order.into_iter().rev() {
            let surface = self.windows.iter().find_map(|w| match &w.role {
                Role::Popup {
                    namespace, popup, ..
                } if *namespace == ns => Some(popup.wl_surface().clone()),
                _ => None,
            });

            if let Some(surface) = surface {
                self.close(&surface);
            }
            self.popup_dismiss_handlers.retain(|(n, _)| *n != ns);
            self.last_grab_serial.remove(ns);
        }
    }

    fn open_popup(&mut self, view_fn: PopupView, popup: Popup) {
        // Find a parent xdg_surface. First try the requested normal window, then
        // the requested namespace; if None, fall back to any live Role::Layer window.
        let parent_surface =
            self.find_parent_xdg_surface(popup.parent, popup.parent_window.as_deref());

        let Some(parent) = parent_surface else {
            if std::env::var_os("AMANE_DEBUG").is_some() {
                eprintln!(
                    "popup open skipped: parent not up (requested={:?})",
                    popup.parent
                );
            }
            return;
        };

        let positioner = match XdgPositioner::new(&self.xdg_shell) {
            Ok(p) => p,
            Err(e) => {
                if std::env::var_os("AMANE_DEBUG").is_some() {
                    eprintln!("popup open failed: ns={} positioner: {e}", popup.namespace);
                }
                return;
            }
        };

        // A zero dimension is an xdg_positioner protocol error, so clamp.
        let w = popup.size.0.max(1);
        let h = popup.size.1.max(1);
        positioner.set_size(w as i32, h as i32);
        positioner.set_anchor_rect(
            popup.anchor_rect.x,
            popup.anchor_rect.y,
            popup.anchor_rect.width.max(1),
            popup.anchor_rect.height.max(1),
        );
        positioner.set_anchor(to_sctk_anchor(popup.anchor));
        positioner.set_gravity(to_sctk_gravity(popup.gravity));
        positioner.set_constraint_adjustment(to_sctk_constraint(popup.constraint_adjustment));
        positioner.set_offset(popup.offset.0, popup.offset.1);

        let surface = self.compositor.create_surface(&self.qh);

        // ParentSurface tells how we parent: either None (reparent via
        // layer_surface.get_popup) or Some(xdg_surface) (nested popup).
        let (parent_arg, layer_parent) = match parent {
            ParentSurface::Layer(layer_surface) => (None, Some(layer_surface)),
            ParentSurface::Popup(parent_xdg) | ParentSurface::Normal(parent_xdg) => {
                (Some(parent_xdg), None)
            }
        };

        let sctk_popup = match SctkPopup::from_surface(
            parent_arg.as_ref(),
            &positioner,
            &self.qh,
            surface,
            &self.xdg_shell,
        ) {
            Ok(p) => p,
            Err(e) => {
                if std::env::var_os("AMANE_DEBUG").is_some() {
                    eprintln!(
                        "popup open failed: ns={} from_surface: {e}",
                        popup.namespace
                    );
                }
                return;
            }
        };

        // niri's background-effect / corner-radius mask only applies when the
        // popup has a Wayland geometry set to the visible rect (excluding any
        // client-drawn shadow). We draw no shadow, so geometry == full size.
        // Must be before the initial commit.
        sctk_popup
            .xdg_surface()
            .set_window_geometry(0, 0, w as i32, h as i32);

        // For a layer-parented popup, SCTK requires we call get_popup BEFORE
        // the initial commit.
        if let Some(ls) = layer_parent {
            use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
            // scoped lookup to find the LayerSurface object
            if let Some(found) = self.windows.iter().find_map(|w| match &w.role {
                Role::Layer { surface, .. } if surface.wl_surface() == &ls => Some(surface.clone()),
                _ => None,
            }) {
                LayerSurface::get_popup(&found, sctk_popup.xdg_popup());
            }
        }

        // Grab, if requested. The grab needs a recent input serial; without
        // one, the compositor would reject the grab (and some compositors kill
        // the connection). We skip + warn instead.
        if popup.grab {
            match (self.last_input_serial, self.seat.seats().next()) {
                (Some(serial), Some(seat))
                    if self.last_grab_serial.get(popup.namespace) != Some(&serial) =>
                {
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!("popup grab: ns={} serial={}", popup.namespace, serial);
                    }
                    self.last_grab_serial.insert(popup.namespace, serial);
                    sctk_popup.xdg_popup().grab(&seat, serial);
                }
                (Some(serial), Some(_)) => {
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!(
                            "popup grab skipped: ns={} (serial {serial} already used)",
                            popup.namespace
                        );
                    }
                }
                (None, _) => {
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!(
                            "popup grab skipped: ns={} (no input serial yet)",
                            popup.namespace
                        );
                    }
                }
                (_, None) => {
                    if std::env::var_os("AMANE_DEBUG").is_some() {
                        eprintln!("popup grab skipped: ns={} (no seat)", popup.namespace);
                    }
                }
            }
        }

        sctk_popup.wl_surface().commit();

        // Store the on_dismiss callback keyed by namespace so popup_done can
        // find it later.
        self.popup_dismiss_handlers
            .retain(|(ns, _)| *ns != popup.namespace);
        if let Some(cb) = popup.on_dismiss.clone() {
            self.popup_dismiss_handlers.push((popup.namespace, cb));
        }

        // Create the OpenWindow backing it.
        let display = self.connection.backend().display_ptr().cast();
        let surface_ptr = sctk_popup.wl_surface().id().as_ptr().cast();
        let gpu = WindowGpu::new(display, surface_ptr);
        let fractional = self.make_fractional(sctk_popup.wl_surface());

        let namespace = popup.namespace;

        let role = Role::Popup {
            popup: sctk_popup,
            namespace,
            parent: popup.parent,
            window: popup.parent_window,
            grab: popup.grab,
        };

        let window = OpenWindow {
            view: View::Popup(view_fn),

            output: None,
            on_output: None,
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

    fn find_parent_xdg_surface(
        &self,
        requested: Option<&'static str>,
        window: Option<&str>,
    ) -> Option<ParentSurface> {
        if let Some(key) = window {
            return self.windows.iter().find_map(|w| match &w.role {
                Role::Normal { window, .. } if w.view.shows(key) => {
                    Some(ParentSurface::Normal(window.xdg_surface().clone()))
                }
                _ => None,
            });
        }

        if let Some(ns) = requested {
            for w in &self.windows {
                match &w.role {
                    Role::Popup {
                        namespace, popup, ..
                    } if *namespace == ns => {
                        return Some(ParentSurface::Popup(popup.xdg_surface().clone()));
                    }
                    Role::Layer { surface, settings } if settings.namespace == ns => {
                        // a view made per monitor has one surface on each; the
                        // popup hangs under the one the pointer is over
                        let here = crate::pointer_monitor();
                        let same = self.windows.iter().find_map(|other| match &other.role {
                            Role::Layer { surface, settings }
                                if settings.namespace == ns
                                    && here.is_some()
                                    && other.on_output == here =>
                            {
                                Some(surface.wl_surface().clone())
                            }
                            _ => None,
                        });

                        return Some(ParentSurface::Layer(
                            same.unwrap_or_else(|| surface.wl_surface().clone()),
                        ));
                    }
                    _ => {}
                }
            }
            return None;
        }

        // No namespace given: fall back to the first live Layer window.
        for w in &self.windows {
            if let Role::Layer { surface, .. } = &w.role {
                return Some(ParentSurface::Layer(surface.wl_surface().clone()));
            }
        }

        None
    }
}

enum ParentSurface {
    Layer(WlSurface),
    Popup(xdg_surface::XdgSurface),
    Normal(xdg_surface::XdgSurface),
}

impl PopupHandler for WaylandState {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        popup: &SctkPopup,
        config: PopupConfigure,
    ) {
        if std::env::var_os("AMANE_DEBUG").is_some() {
            eprintln!(
                "popup configure: pos=({},{}) size={}x{} kind={:?}",
                config.position.0, config.position.1, config.width, config.height, config.kind
            );
        }

        let Some(window) = self.window(popup.wl_surface()) else {
            return;
        };

        let w = config.width.max(0) as u32;
        let h = config.height.max(0) as u32;
        window.resize(w, h);
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &SctkPopup) {
        let surface = popup.wl_surface().clone();

        // Fire on_dismiss callback.
        let namespace = self.windows.iter().find_map(|w| match &w.role {
            Role::Popup {
                namespace,
                popup: p,
                ..
            } if p.wl_surface() == &surface => Some(*namespace),
            _ => None,
        });

        if std::env::var_os("AMANE_DEBUG").is_some() {
            eprintln!("popup done: ns={:?}", namespace);
        }

        if let Some(ns) = namespace {
            // Keep this popup from being reopened on the next tick with a
            // stale input serial; cleared when the app genuinely hides it.
            if !self.dismissed.contains(&ns) {
                self.dismissed.push(ns);
            }

            if let Some((_, cb)) = self
                .popup_dismiss_handlers
                .iter()
                .find(|(n, _)| *n == ns)
                .cloned()
            {
                cb();
            }

            // Tear down this popup and any still-live descendants child-first.
            self.close_popup_cascade(ns);
        } else {
            self.close(&surface);
        }
    }
}

fn to_sctk_anchor(a: Anchor) -> xdg_positioner::Anchor {
    match a {
        Anchor::None => xdg_positioner::Anchor::None,
        Anchor::Top => xdg_positioner::Anchor::Top,
        Anchor::Bottom => xdg_positioner::Anchor::Bottom,
        Anchor::Left => xdg_positioner::Anchor::Left,
        Anchor::Right => xdg_positioner::Anchor::Right,
        Anchor::TopLeft => xdg_positioner::Anchor::TopLeft,
        Anchor::TopRight => xdg_positioner::Anchor::TopRight,
        Anchor::BottomLeft => xdg_positioner::Anchor::BottomLeft,
        Anchor::BottomRight => xdg_positioner::Anchor::BottomRight,
    }
}

fn to_sctk_gravity(g: Gravity) -> xdg_positioner::Gravity {
    match g {
        Gravity::None => xdg_positioner::Gravity::None,
        Gravity::Top => xdg_positioner::Gravity::Top,
        Gravity::Bottom => xdg_positioner::Gravity::Bottom,
        Gravity::Left => xdg_positioner::Gravity::Left,
        Gravity::Right => xdg_positioner::Gravity::Right,
        Gravity::TopLeft => xdg_positioner::Gravity::TopLeft,
        Gravity::TopRight => xdg_positioner::Gravity::TopRight,
        Gravity::BottomLeft => xdg_positioner::Gravity::BottomLeft,
        Gravity::BottomRight => xdg_positioner::Gravity::BottomRight,
    }
}

fn to_sctk_constraint(c: ConstraintAdjustment) -> xdg_positioner::ConstraintAdjustment {
    let mut out = xdg_positioner::ConstraintAdjustment::None;
    if c.contains(ConstraintAdjustment::SLIDE_X) {
        out |= xdg_positioner::ConstraintAdjustment::SlideX;
    }
    if c.contains(ConstraintAdjustment::SLIDE_Y) {
        out |= xdg_positioner::ConstraintAdjustment::SlideY;
    }
    if c.contains(ConstraintAdjustment::FLIP_X) {
        out |= xdg_positioner::ConstraintAdjustment::FlipX;
    }
    if c.contains(ConstraintAdjustment::FLIP_Y) {
        out |= xdg_positioner::ConstraintAdjustment::FlipY;
    }
    if c.contains(ConstraintAdjustment::RESIZE_X) {
        out |= xdg_positioner::ConstraintAdjustment::ResizeX;
    }
    if c.contains(ConstraintAdjustment::RESIZE_Y) {
        out |= xdg_positioner::ConstraintAdjustment::ResizeY;
    }
    out
}
