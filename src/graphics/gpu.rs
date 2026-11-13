mod atlas;
mod blur;
mod bounds;
mod clip;
mod convert;
mod cut;
mod dispatch;
mod effect;
mod encoder;
mod flush;
mod forget;
mod glyph;
mod gradient;
mod group;
mod image;
mod paint;
mod painted;
mod pass;
mod picture;
mod pool;
mod present;
mod quads;
mod shade;
mod shader;
mod shadow;
mod shape;
mod shared;
mod target;
mod texture;
mod wait;
mod window_gpu;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use vello::Scene;
use vello::peniko::ImageData;
use vello::wgpu::{
    BlendState, CommandEncoder, Device, PollType, Queue, Sampler, Surface, SurfaceConfiguration,
    Texture, TextureFormat,
};

use crate::graphics::image::Bitmap;
use crate::graphics::renderer::Command;

use pass::Pass;
use shared::Shared;
pub(crate) use shared::{VELLO_IDLE, on_vello_run, release_idle_scratch};
pub(crate) use window_gpu::WindowGpu;

// AMANE_KEEP_HIDDEN=1 keeps a hidden window's gpu memory, as before the release on hide
static KEEP_HIDDEN: LazyLock<bool> = LazyLock::new(|| env::var_os("AMANE_KEEP_HIDDEN").is_some());

// Outlives the backend so cached GPU resources are released after every window.
pub(crate) struct Session;

impl Drop for Session {
    fn drop(&mut self) {
        shared::clear();
    }
}

/*
 * the only part of amane that knows how drawing is done,
 * vello draws the shapes and wgpu runs everything else on the gpu
 */
pub struct Gpu {
    device: Device,
    queue: Queue,

    // libwayland's display and surface the window draws into, to make its gpu surface again
    display: *mut c_void,
    handle: *mut c_void,

    surface: Surface<'static>,
    config: SurfaceConfiguration,

    // one renderer for every window, see shared.rs
    vello: Rc<RefCell<vello::Renderer>>,

    // images already turned into vello's form, keyed by where the loaded image lives
    images: HashMap<usize, ImageData>,

    // images this window keeps copies of, and the ones it drew this frame
    shown: HashMap<usize, Arc<Bitmap>>,
    drawn: HashSet<usize>,

    // letters already drawn into pictures, none for letters without pixels like a space
    glyphs: HashMap<glyph::GlyphKey, Option<glyph::Glyph>>,

    quads: quads::Quads,

    // the shared count of atlas drops, and the one this window last sent its images again after
    atlas_drops: Rc<Cell<u64>>,
    atlas_seen: u64,

    // custom shaders by the path they were read from, compiled again once their file is saved
    shaders: HashMap<PathBuf, HashMap<shader::Kind, shader::Loaded>>,

    // the shader files' generation of the frame on screen, and the shader clock's changes
    shaders_seen: u64,
    clock_seen: u64,

    // what effect passes read their inputs with, and read in place of an input they don't have
    input_sampler: Sampler,
    blank: Texture,

    // image inputs on the gpu, kept with their pixels so the key stays theirs, and the ones read this frame
    input_images: HashMap<usize, (Arc<Bitmap>, Texture)>,
    input_images_read: HashSet<usize>,

    // finished window-sized textures waiting to be reused: given back this frame, and left from the last
    spare: Vec<Texture>,
    stale: Vec<Texture>,

    // last frame's vello results in the order they were painted, and how far this frame got
    painted: Vec<Option<painted::Painted>>,
    painting: usize,

    // how far the scene being gathered reaches, given up with it when it's painted
    reach: bounds::Reach,

    // a run's scene moved to the corner of its own small texture, kept to reuse its memory
    moved: Scene,

    // the commands and buffer size of the frame on screen, none before one shows
    showing: Option<(Vec<Command>, u32, u32)>,

    // how many times this frame went through vello, for the frame log
    runs: u32,

    // shared with every window: when vello last drew, for letting its scratch go when idle
    vello_used: Rc<Cell<Instant>>,
    vello_released: Rc<Cell<bool>>,

    // the frame's jobs not mailed yet; none between frames and right after mailing
    pending: RefCell<Option<CommandEncoder>>,

    composite: Pass,
    present: Pass,
    box_blur: Pass,
    erase: Pass,
}

impl Gpu {
    // the pointers are libwayland's display and surface, which have to outlive the gpu
    pub fn new(display: *mut c_void, handle: *mut c_void) -> Self {
        let instance = shared::instance(display);

        let surface = target::create(&instance, display, handle);

        let Shared {
            adapter,
            device,
            queue,
            vello,
            atlas_drops,
            vello_used,
            vello_released,
        } = shared::get(&instance, &surface);

        let config = target::configure(&surface, &adapter);

        // the canvas and the surface hold premultiplied colors
        let over = Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING);

        let composite = Pass::new(&device, "composite", TextureFormat::Rgba8Unorm, over);
        let present = Pass::new(&device, "composite", config.format, None);
        let box_blur = Pass::new(&device, "box_blur", TextureFormat::Rgba8Unorm, None);
        let erase = cut::erase(&device);

        let quads = quads::Quads::new(&device, &queue);

        let input_sampler = effect::sampler(&device);
        let blank = effect::blank(&device, &queue);

        Self {
            device,
            queue,

            display,
            handle,

            surface,
            config,

            vello,

            images: HashMap::new(),
            shown: HashMap::new(),
            drawn: HashSet::new(),
            glyphs: HashMap::new(),
            quads,
            atlas_drops,
            atlas_seen: 0,
            vello_used,
            vello_released,

            shaders: HashMap::new(),
            shaders_seen: crate::graphics::shader_files::generation(),
            clock_seen: crate::graphics::shader_files::clock_changes(),
            input_sampler,
            blank,
            input_images: HashMap::new(),
            input_images_read: HashSet::new(),
            spare: Vec::new(),
            stale: Vec::new(),

            painted: Vec::new(),
            painting: 0,

            reach: bounds::Reach::Nothing,
            moved: Scene::new(),

            showing: None,

            runs: 0,

            pending: RefCell::new(None),

            composite,
            present,
            box_blur,
            erase,
        }
    }

    // how many vello runs the last frame took; a frame the quads drew alone took none
    pub fn vello_runs(&self) -> u32 {
        self.runs
    }

    /*
     * lets go of what a hidden window holds on the gpu: its swapchain (a
     * surface made again is unconfigured, and the next draw configures it),
     * its canvas and spares, kept results, images, letters and shader inputs;
     * the pipelines stay, they're small and showing again would make the same
     */
    pub fn release(&mut self) {
        if *KEEP_HIDDEN {
            return;
        }

        let instance = shared::instance(self.display);

        self.surface = target::create(&instance, self.display, self.handle);

        self.config.width = 0;
        self.config.height = 0;

        self.spare = Vec::new();
        self.stale = Vec::new();
        self.painted = Vec::new();
        self.painting = 0;

        self.reach = bounds::Reach::Nothing;
        self.moved = Scene::new();

        self.showing = None;

        // nothing was drawn, so every image is unshown and goes the usual way
        self.drawn.clear();
        self.forget_unshown();

        self.images = HashMap::new();
        self.glyphs = HashMap::new();

        self.input_images = HashMap::new();
        self.input_images_read = HashSet::new();

        self.quads.release();

        /*
         * wgpu only frees what was dropped when the device is polled, and a
         * desktop that hid a window may draw nothing for a long while; waiting
         * for the gpu here, once, gives the window's memory back now
         */
        let _ = self.device.poll(PollType::wait_indefinitely());

        #[cfg(feature = "vello-stats")]
        if crate::timing::enabled()
            && let Some(report) = shared::instance(self.display).generate_report()
        {
            let hub = report.hub_report();

            eprintln!(
                "released: wgpu buffers {}+{} textures {}+{}",
                hub.buffers.num_kept_from_user,
                hub.buffers.num_released_from_user,
                hub.textures.num_kept_from_user,
                hub.textures.num_released_from_user,
            );
        }
    }
}
