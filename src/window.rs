use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use crate::changes;
use crate::input::KeyHandlers;
use crate::{Key, Widget};

// a regular desktop window with a title bar, like a settings app, next to the layer windows
pub struct Window {
    pub(crate) title: String,

    // read once, when the window opens; none takes App::app_id
    pub(crate) app_id: Option<String>,

    // the size it opens at, the user can resize it after that if it is resizable
    pub(crate) width: f32,
    pub(crate) height: f32,

    pub(crate) resizable: bool,

    pub(crate) min_size: Option<(f32, f32)>,
    pub(crate) max_size: Option<(f32, f32)>,

    // none leaves it to the user, a value is sent each time the view changes it
    pub(crate) maximized: Option<bool>,
    pub(crate) fullscreen: Option<bool>,

    // the key of the window this one belongs to, like a dialog over its app
    pub(crate) parent: Option<String>,
    pub(crate) modal: bool,

    pub(crate) root: Option<Box<dyn Widget>>,

    pub(crate) keys: KeyHandlers,

    pub(crate) on_close: Option<CloseHandler>,

    // its grey content blended with what's behind it, see vibrant
    pub(crate) vibrant: bool,
}

// asked when the user closes the window, false keeps it open
pub(crate) type CloseHandler = Rc<dyn Fn() -> bool>;

impl Window {
    pub fn new() -> Self {
        Self {
            title: String::from("amane"),
            app_id: None,

            width: 640.0,
            height: 480.0,

            resizable: true,

            min_size: None,
            max_size: None,

            maximized: None,
            fullscreen: None,

            parent: None,
            modal: false,

            root: None,

            keys: KeyHandlers::default(),

            on_close: None,
            vibrant: false,
        }
    }

    // sent again whenever it changes
    pub fn title(mut self, title: &str) -> Self {
        self.title = String::from(title);

        self
    }

    /*
     * what the compositor, the dock and window rules know the app by, like "emaki-settings";
     * read once, when the window opens, and App::app_id is used when it is not set
     */
    pub fn app_id(mut self, app_id: &str) -> Self {
        self.app_id = Some(String::from(app_id));

        self
    }

    // read once, when the window opens
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;

        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;

        self
    }

    // the smallest the user can make it; a compositor may still give a tiled window less
    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));

        self
    }

    pub fn max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = Some((width, height));

        self
    }

    /*
     * asks the compositor to maximize the window or not; it is asked again only when the
     * view changes its answer, so the user can still unmaximize it from the compositor
     */
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.maximized = Some(maximized);

        self
    }

    // the same as maximized, for fullscreen
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = Some(fullscreen);

        self
    }

    // shows it over the open window with this key, the way a dialog belongs to its app
    pub fn parent(mut self, key: &str) -> Self {
        self.parent = Some(String::from(key));

        self
    }

    // keeps the user out of the parent until this window closes, where the compositor can
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;

        self
    }

    /*
     * its light and dark grey content blended with what the compositor
     * frosted behind it rather than painted over it, on a compositor with
     * niri-vibrancy: labels that stay legible on any backdrop
     */
    pub fn vibrant(mut self, vibrant: bool) -> Self {
        self.vibrant = vibrant;

        self
    }

    pub fn child(mut self, child: impl Widget + 'static) -> Self {
        self.root = Some(Box::new(child));

        self
    }

    pub fn on_key(mut self, handler: impl Fn(Key) + 'static) -> Self {
        self.keys.press = Some(Rc::new(handler));

        self
    }

    pub fn on_key_release(mut self, handler: impl Fn(Key) + 'static) -> Self {
        self.keys.release = Some(Rc::new(handler));

        self
    }

    /*
     * runs first for keys pressed with ctrl, alt or super held, before a focused text
     * input; return true for a chord it used, like Ctrl+S, so the input doesn't see it
     */
    pub fn on_shortcut(mut self, handler: impl Fn(Key) -> bool + 'static) -> Self {
        self.keys.shortcut = Some(Rc::new(handler));

        self
    }

    /*
     * runs when the user closes the window from the compositor, like with a keybind;
     * returning false keeps it open, to ask about unsaved changes first. close_window
     * does not ask
     */
    pub fn on_close(mut self, handler: impl Fn() -> bool + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));

        self
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

// a view that can carry its own data, like the file a window shows
pub(crate) type WindowView = Arc<dyn Fn() -> Window + Send + Sync>;

// what handlers asked of normal windows, done in order by the event loop when it wakes
pub(crate) enum Request {
    Open(String, WindowView),
    Close(String),
    Minimize(String),
}

pub(crate) static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());

fn request(request: Request) {
    REQUESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);

    changes::mark_all();
}

/*
 * opens a normal window later on, like settings from a button or an ipc call;
 * the name tells windows apart, so asking for a name already open does nothing
 */
pub fn open_window(name: &str, view: fn() -> Window) {
    request(Request::Open(String::from(name), Arc::new(view)));
}

/*
 * the same, with a view that keeps its own data, so one view can show many windows:
 * open_window_with(format!("doc-{path}"), move || document(&path))
 */
pub fn open_window_with(key: impl Into<String>, view: impl Fn() -> Window + Send + Sync + 'static) {
    request(Request::Open(key.into(), Arc::new(view)));
}

// closes the window open with this name, like a close button inside it; a closed one is left alone
pub fn close_window(name: &str) {
    request(Request::Close(String::from(name)));
}

// asks the compositor to hide the window until the user brings it back, if it can
pub fn minimize_window(name: &str) {
    request(Request::Minimize(String::from(name)));
}

// what the compositor last said about a normal window
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    // the window the user is working in; dim or grey out the rest
    pub focused: bool,

    pub maximized: bool,
    pub fullscreen: bool,

    // placed edge to edge with other windows, like in a tiling compositor
    pub tiled: bool,

    // the user is dragging its edges right now
    pub resizing: bool,

    // the compositor draws its title bar; false means the window has none
    pub server_decorated: bool,
}

thread_local! {
    // the size of the window whose view is running, set by the backend before each run
    static SIZE: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };

    static STATE: Cell<WindowState> = const {
        Cell::new(WindowState {
            focused: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            resizing: false,
            server_decorated: false,
        })
    };

    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
}

/*
 * the window being drawn, in widget units, before App's scale factor; a compositor can give a
 * window another size than it asked for, like a tiling one does. 0 by 0
 * before the compositor has said
 */
pub fn window_size() -> (f32, f32) {
    SIZE.get()
}

pub(crate) fn set_size(width: f32, height: f32) {
    SIZE.set((width, height));
}

// the state of the normal window being drawn, all false for layer windows and popups
pub fn window_state() -> WindowState {
    STATE.get()
}

pub(crate) fn set_state(state: WindowState) {
    STATE.set(state);
}

/*
 * the name of the normal window being drawn, empty for other windows; read it in the
 * view and move it into a handler to close or minimize the window from inside it
 */
pub fn current_window() -> String {
    CURRENT.with_borrow(Clone::clone)
}

pub(crate) fn set_current(key: &str) {
    CURRENT.with_borrow_mut(|current| {
        current.clear();
        current.push_str(key);
    });
}
