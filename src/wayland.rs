mod activation;
mod background_effect;
mod blur_shapes;
pub(crate) mod clipboard;
mod compositor;
pub(crate) mod dropped;
mod idle;
mod idle_gpu;
mod input_method_v2;
mod keyboard;
mod layer;
mod lock;
mod normal;
mod output;
mod pointer;
mod popup;
mod redraw;
mod scale;
mod seat;
mod secure_fill;
mod socket;
mod surface;
mod text_input;
mod update;
mod vibrancy;
mod wake;
mod windows;

use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::ExtIdleNotificationV1, ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use smithay_client_toolkit::reexports::protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    viewporter::client::wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::{
    compositor::CompositorState,
    data_device_manager::{
        DataDeviceManagerState, data_device::DataDevice, data_source::DragSource,
    },
    primary_selection::{PrimarySelectionManagerState, device::PrimarySelectionDevice},
    delegate_dispatch2, delegate_registry,
    output::OutputState,
    reexports::{
        calloop::{EventLoop, LoopHandle},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{SeatState, pointer::ThemedPointer},
    session_lock::{SessionLock, SessionLockState},
    subcompositor::SubcompositorState,
    activation::ActivationState,
    shell::{wlr_layer::LayerShell, xdg::XdgShell},
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard::WlKeyboard, wl_surface::WlSurface},
};

use crate::ipc::IpcHandlers;
use crate::popup::Popup;
use crate::scale::ScaleFactor;
use crate::window::WindowView;
use crate::{Cursor, LayerWindow, Monitor};

use surface::{OpenWindow, View};

// a popup's namespace and what runs when the compositor dismisses it
type DismissHandler = (&'static str, std::rc::Rc<dyn Fn()>);

struct WaylandState {
    // every window has its own layer surface and gpu, and is dropped before the connection
    windows: Vec<OpenWindow>,

    // popup slots: their views are polled each tick to open/close surfaces
    popup_views: Vec<popup::PopupSlot>,

    // on_dismiss callbacks keyed by popup namespace
    popup_dismiss_handlers: Vec<DismissHandler>,

    // popups the compositor dismissed; kept closed until the app hides+shows
    // them, so we never reopen with a stale grab serial every tick
    dismissed: Vec<&'static str>,

    // the serial last used to grab each popup, so the same serial is not
    // reused for a grab (which the compositor would reject)
    last_grab_serial: std::collections::HashMap<&'static str, u32>,

    // last pointer button serial, needed for xdg_popup.grab()
    last_input_serial: Option<u32>,

    // each of these gets a window on every monitor, including ones plugged in later
    per_monitor: Vec<fn(&Monitor) -> LayerWindow>,

    /*
     * app windows the compositor closed, as it does when their monitor goes
     * away; they open again on the next monitor, or at once when another is
     * still there
     */
    closed_views: Vec<fn() -> LayerWindow>,

    scale_factor: ScaleFactor,

    // keys do not say which window they are for, so the window that has focus is kept
    keyboard_focus: Option<WlSurface>,

    // what normal windows are known by when they don't set their own
    app_id: String,

    // the normal windows App was given, opened again when a closed app is launched again
    normal_views: Vec<(String, WindowView)>,

    // lets a window take the focus with a token, none when the compositor lacks xdg-activation
    activation: Option<ActivationState>,

    // a token to focus the next window that shows with, like the one amane was started with
    pending_token: Option<String>,

    // set by App::keep_alive, so closing the last window doesn't end the app
    keep_alive: bool,

    registry: RegistryState,
    output: OutputState,
    seat: SeatState,

    // kept so new windows can be made while the shell runs
    compositor: CompositorState,
    layer_shell: LayerShell,
    xdg_shell: XdgShell,
    shm: Shm,

    // both are needed for fractional scales like 1.25, none when the compositor lacks one
    fractional_scale: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,

    // lets a window say where the compositor blurs behind it, none when it lacks ext-background-effect
    background_effect: Option<ExtBackgroundEffectManagerV1>,

    // rounded, fading blur shapes in place of that region, none when the compositor lacks niri-blur-shapes
    blur_shapes: Option<blur_shapes::protocol::niri_blur_shapes_manager_v1::NiriBlurShapesManagerV1>,

    // what windows make their vibrant layers with, none when the compositor lacks niri-vibrancy
    vibrancy_kit: Option<vibrancy::VibrancyKit>,

    // tells Idle when the user went away, none when the compositor lacks ext-idle-notify
    idle_notifier: Option<ExtIdleNotifierV1>,
    idle_notifications: Vec<ExtIdleNotificationV1>,

    // says when input comes while the screen is off (screen_off), empty while it's on
    screen_off_watch: Vec<ExtIdleNotificationV1>,

    /*
     * the lock screen's view, what asks the compositor for a lock (kept, so
     * the session can be locked again), and the lock while the session is locked
     */
    lock_view: Option<fn(&Monitor) -> LayerWindow>,
    lock_state: Option<SessionLockState>,
    session_lock: Option<SessionLock>,
    connection: Connection,

    // kept so they can be released when the mouse or keyboard is unplugged
    pointer_device: Option<ThemedPointer<()>>,
    keyboard_device: Option<WlKeyboard>,

    // repeats held keys on a timer, set once the event loop exists
    loop_handle: Option<LoopHandle<'static, WaylandState>>,

    // files dragged in from other programs, none when the compositor lacks it
    data_device_manager: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,

    // the surface files are being dragged over, and the files, read as they came in
    files_held: Option<(WlSurface, Vec<std::path::PathBuf>)>,

    // file lists and drops still being read from the program that offered them
    drags: dropped::Drags,

    // the middle-click selection, none when the compositor lacks it
    primary_manager: Option<PrimarySelectionManagerState>,
    primary_device: Option<PrimarySelectionDevice>,

    // what amane put on the clipboards, sent to whoever pastes it
    copied: clipboard::Copied,

    // types what a keyboard can't, none when the compositor lacks text-input-v3
    text_input_manager: Option<smithay_client_toolkit::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    input_method: Option<text_input::InputMethod>,

    /*
     * this app as the seat's input method, only after App::input_method: the popup's view,
     * the manager to make it with once the seat has a keyboard, and the input method then
     */
    input_popup_view: Option<crate::popup::PopupView>,
    ime_manager: Option<wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    ime: Option<input_method_v2::ImeServer>,

    // fills checked by the compositor (niri-secure-fill), the target for the focused field, and
    // the targets filled that wait for their answer
    secure_fill: Option<secure_fill::protocol::niri_secure_fill_manager_v1::NiriSecureFillManagerV1>,
    fill_target: Option<secure_fill::Target>,
    spent_fill_targets: Vec<secure_fill::Target>,

    // what is being dragged out to another program
    dragging_out: Option<(DragSource, crate::DragData)>,

    // a drag out has to name the press that started it
    last_press_serial: Option<u32>,

    // the press before, so quick presses count as a double or triple click
    last_press: Option<pointer::LastPress>,

    // what the pointer was last set to on one of the windows, none after it leaves
    cursor_shown: Option<Cursor>,

    qh: QueueHandle<WaylandState>,

    running: bool,
}

// the state is dropped before the event loop, which holds the connection the gpu draws through
pub struct WaylandApp {
    state: WaylandState,

    event_loop: EventLoop<'static, WaylandState>,
}

impl WaylandApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        views: Vec<fn() -> LayerWindow>,
        normal_views: Vec<(String, WindowView)>,
        popup_views: Vec<fn() -> Popup>,
        per_monitor: Vec<fn(&Monitor) -> LayerWindow>,
        lock_view: Option<fn(&Monitor) -> LayerWindow>,
        input_method: Option<fn() -> Popup>,
        handlers: Option<IpcHandlers>,
        scale_factor: ScaleFactor,
        app_id: String,
        keep_alive: bool,
    ) -> Self {
        let connection = connect();

        let (globals, event_queue) =
            registry_queue_init(&connection).expect("failed to discover Wayland globals");

        let qh = event_queue.handle();

        let compositor =
            CompositorState::bind(&globals, &qh).expect("failed to bind wl_compositor");

        let layer_shell = LayerShell::bind(&globals, &qh).expect("failed to bind wlr-layer-shell");

        let xdg_shell = XdgShell::bind(&globals, &qh).expect("failed to bind xdg-shell");

        let shm = Shm::bind(&globals, &qh).expect("failed to bind wl_shm");

        let fractional_scale = globals.bind(&qh, 1..=1, ()).ok();
        let viewporter = globals.bind(&qh, 1..=1, ()).ok();

        let background_effect = globals.bind(&qh, 1..=1, ()).ok();

        // AMANE_NO_BLUR_SHAPES=1 tries the blur region a compositor without shapes would get
        let blur_shapes = if std::env::var_os("AMANE_NO_BLUR_SHAPES").is_some() {
            None
        } else {
            globals
                .bind(&qh, 1..=2, blur_shapes::Materials::default())
                .ok()
        };
        crate::blur::set_shapes_available(blur_shapes.is_some());

        // AMANE_NO_VIBRANCY=1 draws vibrant windows as a compositor without it would get them
        let vibrancy_manager = if std::env::var_os("AMANE_NO_VIBRANCY").is_some() {
            None
        } else {
            globals.bind(&qh, 1..=1, ()).ok()
        };
        let subcompositor =
            SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh).ok();
        let vibrancy_kit = match (vibrancy_manager, subcompositor) {
            (Some(manager), Some(subcompositor)) => Some(vibrancy::VibrancyKit {
                manager,
                subcompositor: std::rc::Rc::new(subcompositor),
                compositor: compositor.wl_compositor().clone(),
                viewporter: viewporter.clone(),
                connection: connection.clone(),
            }),
            _ => None,
        };

        let idle_notifier = globals.bind(&qh, 1..=1, ()).ok();

        let data_device_manager = DataDeviceManagerState::bind(&globals, &qh).ok();

        let primary_manager = PrimarySelectionManagerState::bind(&globals, &qh).ok();

        let text_input_manager = globals.bind(&qh, 1..=1, ()).ok();

        // the input method and checked fills, only for an app that asked to be the input method
        let (ime_manager, secure_fill) = match input_method {
            Some(_) => (
                globals.bind(&qh, 1..=1, ()).ok(),
                globals.bind(&qh, 1..=1, ()).ok(),
            ),
            None => (None, None),
        };

        // windows per monitor are opened once the compositor describes each monitor
        let mut state = WaylandState {
            windows: Vec::new(),

            popup_views: popup_views
                .into_iter()
                .map(popup::PopupSlot::registered)
                .collect(),
            popup_dismiss_handlers: Vec::new(),
            dismissed: Vec::new(),
            last_grab_serial: std::collections::HashMap::new(),
            last_input_serial: None,

            per_monitor,
            closed_views: Vec::new(),

            scale_factor,

            keyboard_focus: None,

            app_id,
            keep_alive,

            normal_views: normal_views.clone(),
            activation: ActivationState::bind(&globals, &qh).ok(),
            pending_token: std::env::var("XDG_ACTIVATION_TOKEN").ok(),

            registry: RegistryState::new(&globals),
            output: OutputState::new(&globals, &qh),
            seat: SeatState::new(&globals, &qh),

            compositor,
            layer_shell,
            xdg_shell,
            shm,

            fractional_scale,
            viewporter,

            background_effect,
            blur_shapes,
            vibrancy_kit,

            idle_notifier,
            idle_notifications: Vec::new(),
            screen_off_watch: Vec::new(),

            lock_view,
            lock_state: None,
            session_lock: None,
            connection: connection.clone(),

            pointer_device: None,
            cursor_shown: None,
            keyboard_device: None,
            loop_handle: None,

            data_device_manager,
            data_device: None,
            files_held: None,
            drags: dropped::Drags::default(),

            primary_manager,
            primary_device: None,
            copied: clipboard::Copied::default(),
            text_input_manager,
            input_method: None,
            input_popup_view: input_method
                .map(|view| std::sync::Arc::new(view) as crate::popup::PopupView),
            ime_manager,
            ime: None,
            secure_fill,
            fill_target: None,
            spent_fill_targets: Vec::new(),
            dragging_out: None,
            last_press_serial: None,
            last_press: None,

            qh,

            running: true,
        };

        for view in views {
            state.open(View::Plain(view), None);
        }

        for (name, view) in normal_views {
            state.open_normal(name, view);
        }

        if lock_view.is_some() {
            state.lock_state = Some(SessionLockState::new(&globals, &state.qh));
        }

        // a Lock::start before run() has no event loop to wake yet
        state.start_lock_if_asked();

        // the same for Idle::set_levels, and the seats there at start-up never pass new_seat
        state.update_idle_if_asked();

        let event_loop = EventLoop::try_new().expect("failed to create event loop");

        WaylandSource::new(connection, event_queue)
            .insert(event_loop.handle())
            .expect("failed to insert Wayland source");

        if let Some(handlers) = handlers {
            socket::insert(&event_loop.handle(), handlers, &state.app_id);
        }

        wake::insert(&event_loop.handle());

        crate::timer::insert(&event_loop.handle());

        idle_gpu::insert(&event_loop.handle());

        state.loop_handle = Some(event_loop.handle());

        Self { state, event_loop }
    }

    pub fn run(&mut self) {
        while self.state.running {
            self.event_loop
                .dispatch(None, &mut self.state)
                .expect("failed to dispatch events");

            // windows asked onto another monitor are made again there
            self.state.move_windows();

            // Reconcile popup slots (open/close) after each dispatch tick.
            self.state.reconcile_popups();

            self.state.handle_fill_requests();
            self.state.reconcile_input_popup();

            self.state.sync_input_method();

            // what the windows drew from, so services nothing shows can stop polling
            self.state.publish_watched();
        }
    }
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

// the cursor theme fallback draws its icons into shared memory
impl ShmHandler for WaylandState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

fn connect() -> Connection {
    Connection::connect_to_env().expect("failed to connect to Wayland")
}

delegate_registry!(WaylandState);

delegate_dispatch2!(WaylandState);
