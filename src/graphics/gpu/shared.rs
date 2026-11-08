use std::cell::{Cell, OnceCell, RefCell};
use std::env;
use std::ffi::c_void;
use std::num::NonZeroUsize;
use std::process;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use vello::wgpu::{
    Adapter, Device, DeviceDescriptor, DeviceLostReason, Instance, InstanceDescriptor, MemoryHints,
    PollType, Queue, RequestAdapterOptions, Surface,
};
use vello::{AaSupport, RendererOptions};

use super::target::Display;
use super::wait::wait;

/*
 * one gpu device and one vello for every window: separate devices are
 * separate gpu contexts, which the driver switches between whenever two
 * windows draw, and each compiled every shader again on its first frame
 */
#[derive(Clone)]
pub struct Shared {
    pub adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
    pub vello: Rc<RefCell<vello::Renderer>>,

    // how many times vello dropped its image atlas, which every window's images were in
    pub atlas_drops: Rc<Cell<u64>>,

    // when vello last drew, and whether its scratch was let go since; see release_idle_scratch
    pub vello_used: Rc<Cell<Instant>>,
    pub vello_released: Rc<Cell<bool>>,
}

// every window draws on the main thread, so thread locals are enough
thread_local! {
    static INSTANCE: OnceCell<Instance> = const { OnceCell::new() };

    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/*
 * vello keeps the scratch buffers of its last run (29 MB with the fork's
 * sizes, plus the scene uploads) for the next, which on a desktop may be
 * minutes away. AMANE_VELLO_IDLE is the seconds without a run after which
 * they're let go, 30 by default; 0 keeps them for life, as upstream does
 */
pub static VELLO_IDLE: LazyLock<Option<Duration>> = LazyLock::new(|| {
    let seconds = env::var("AMANE_VELLO_IDLE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30u64);

    (seconds > 0).then(|| Duration::from_secs(seconds))
});

// drops vello's scratch if nothing drew through it for the idle time; the timer calls this
pub fn release_idle_scratch() {
    let Some(idle) = *VELLO_IDLE else {
        return;
    };

    SHARED.with_borrow(|shared| {
        let Some(shared) = shared else {
            return;
        };

        if shared.vello_released.get() || shared.vello_used.get().elapsed() < idle {
            return;
        }

        shared.vello.borrow_mut().release_scratch();

        // wgpu frees what was dropped only when the device is polled, and an idle desktop may not draw for a while
        shared
            .device
            .poll(PollType::wait_indefinitely())
            .expect("failed to wait for the gpu");

        shared.vello_released.set(true);

        #[cfg(feature = "vello-stats")]
        if crate::timing::enabled() {
            eprintln!("released vello's scratch after {:?} idle", idle);
        }
    });
}

/*
 * the surfaces of every window come from the same instance, made with the
 * first window's display: the gl backend picks how it presents when the
 * instance is made, and without the display it can only draw offscreen
 */
pub fn instance(display: *mut c_void) -> Instance {
    INSTANCE.with(|instance| instance.get_or_init(|| open_instance(display)).clone())
}

fn open_instance(display: *mut c_void) -> Instance {
    let display = Box::new(Display::new(display));

    let descriptor = InstanceDescriptor::new_with_display_handle_from_env(display);

    Instance::new(descriptor)
}

// opened by the first window, with a gpu that can draw to its surface; later windows reuse it
pub fn get(instance: &Instance, surface: &Surface) -> Shared {
    SHARED.with_borrow_mut(|shared| {
        shared
            .get_or_insert_with(|| open(instance, surface))
            .clone()
    })
}

pub fn clear() {
    // wgpu's destruction uses thread locals, so finish before thread-local teardown begins.
    drop(SHARED.with_borrow_mut(Option::take));
}

/*
 * how long a process whose windows are all hidden keeps its gpus, the device and vello, so
 * a window shown again soon finds them: AMANE_DEVICE_IDLE seconds, 60 by default, 0 for life
 */
pub static DEVICE_IDLE: LazyLock<Option<Duration>> = LazyLock::new(|| {
    let seconds = env::var("AMANE_DEVICE_IDLE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60u64);

    (seconds > 0).then(|| Duration::from_secs(seconds))
});

// lets go of the device and vello once no gpu holds them; window_gpu.rs calls this
pub fn release_device() {
    if let Some(shared) = SHARED.with_borrow_mut(Option::take) {
        // what was dropped is freed when the device is polled
        let _ = shared.device.poll(PollType::wait_indefinitely());
        drop(shared);
    }
}

// vello's log on stderr, for its image atlas size, when amane is built to count what vello takes
#[cfg(feature = "vello-stats")]
fn hear_vello() {
    struct Stderr;

    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.target().starts_with("vello")
        }

        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("{}: {}", record.target(), record.args());
            }
        }

        fn flush(&self) {}
    }

    if crate::timing::enabled() && log::set_logger(&Stderr).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

fn open(instance: &Instance, surface: &Surface) -> Shared {
    #[cfg(feature = "vello-stats")]
    hear_vello();

    let adapter_options = RequestAdapterOptions {
        compatible_surface: Some(surface),
        ..RequestAdapterOptions::default()
    };

    let adapter = wait(instance.request_adapter(&adapter_options))
        .expect("failed to find a gpu that can draw to the window");

    // a software driver draws on the cpu and keeps every texture in ram, so say which one was picked
    let info = adapter.get_info();

    eprintln!(
        "amane: drawing with {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );

    // shells keep their textures alive, so favor smaller allocation blocks over spare capacity
    let descriptor = DeviceDescriptor {
        memory_hints: MemoryHints::MemoryUsage,
        ..DeviceDescriptor::default()
    };

    let (device, queue) = wait(adapter.request_device(&descriptor)).expect("failed to open gpu");

    // one bad command only loses its frame, the default handler would panic and end the shell
    device.on_uncaptured_error(Arc::new(|error| eprintln!("amane: gpu error: {error}")));

    /*
     * a driver reset or a gpu taken away loses every texture and pipeline; amane
     * cannot build them again yet, so it exits with a message instead of freezing
     */
    device.set_device_lost_callback(|reason, message| {
        // the device is also reported lost when it is dropped on the way out
        if reason == DeviceLostReason::Destroyed {
            return;
        }

        eprintln!("amane: lost the gpu ({message}), start the shell again");

        process::exit(1);
    });

    // area anti-aliasing matches how edges looked with the cpu renderer; one thread
    // compiles the shaders, instead of one per cpu for a start that's a second either way
    let vello_options = RendererOptions {
        antialiasing_support: AaSupport::area_only(),
        num_init_threads: NonZeroUsize::new(1),
        ..RendererOptions::default()
    };

    /*
     * AMANE_VELLO_SCRATCH=upstream gives vello's scratch buffers upstream's sizes instead of the
     * vello fork's shell-sized ones, for a scene that comes out blank because it overflowed
     */
    if env::var_os("AMANE_VELLO_SCRATCH").is_some_and(|value| value == "upstream") {
        vello::low_level::vello_encoding::use_upstream_scratch(true);
    }

    let vello = vello::Renderer::new(&device, vello_options).expect("failed to start vello");

    Shared {
        adapter,
        device,
        queue,
        vello: Rc::new(RefCell::new(vello)),
        atlas_drops: Rc::new(Cell::new(0)),
        vello_used: Rc::new(Cell::new(Instant::now())),
        vello_released: Rc::new(Cell::new(false)),
    }
}
