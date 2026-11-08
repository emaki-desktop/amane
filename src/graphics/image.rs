mod shrink;
mod soften;
mod svg;

use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use png::{ColorType, Decoder, Transformations};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::changes;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];

// rgba pixels with plain, not premultiplied, alpha
pub struct Bitmap {
    width: u32,
    height: u32,

    // shared with the gpu, which draws from the same pixels instead of a copy
    pub pixels: Arc<Vec<u8>>,
}

// the same pixels, not equal ones: comparing frames shouldn't read every pixel
impl PartialEq for Bitmap {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && Arc::ptr_eq(&self.pixels, &other.pixels)
    }
}

impl Bitmap {
    // one clear pixel, for a texture slot that has to hold something
    pub fn empty() -> Self {
        Self {
            width: 1,
            height: 1,
            pixels: Arc::new(vec![0; 4]),
        }
    }

    // pixels made here, like a color letter's: plain rgba, width × height × 4 bytes
    pub(crate) fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(pixels.len(), (width * height * 4) as usize);

        Self {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/*
 * what a window waiting on an image counts as having read, so a finished
 * decode only draws the windows still missing a picture, not every window
 */
struct Decoded;

// a file, the size it is shrunk to cover (none for its own size), and how far it is blurred
type Key = (PathBuf, Option<(u32, u32)>, u32);

// an image asked for: none while it is still decoding, or when it could not be read
struct Entry {
    image: Option<Arc<Bitmap>>,
    // when a frame last asked for it, for letting the longest unused go first
    used: Instant,
}

// every image asked for
static LOADED: LazyLock<Mutex<HashMap<Key, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// each file's own size in pixels, once decoded, for laying it out at its shape
static SIZES: LazyLock<Mutex<HashMap<PathBuf, (u32, u32)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/*
 * the decoded copies kept, in bytes, past which the longest unused go: AMANE_IMAGE_CACHE_MB,
 * 64 by default. One a frame asked for within KEPT_WHILE_SHOWN stays whatever the total
 */
static CAP: LazyLock<usize> = LazyLock::new(|| {
    std::env::var("AMANE_IMAGE_CACHE_MB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(64)
        * 1024
        * 1024
});

const KEPT_WHILE_SHOWN: Duration = Duration::from_secs(2);

/*
 * the copy `loaded` and `size` decode to say a file is ready: big enough to draw an icon or a
 * thumbnail from at once, far smaller than most photos
 */
const PROBE: (u32, u32) = (256, 256);

/*
 * decoding a large image takes long enough to drop frames, so it happens
 * on one of the decoding threads; until it is done this gives none and the
 * image is left out, then the window is woken to draw it
 */
pub fn load(path: &Path, cover: Option<(u32, u32)>, blur: u32) -> Option<Arc<Bitmap>> {
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);

    let key = (path.to_path_buf(), cover, blur);

    if let Some(entry) = loaded.get_mut(&key) {
        entry.used = Instant::now();
        if entry.image.is_none() {
            changes::note_read(TypeId::of::<Decoded>());
        }

        return entry.image.clone();
    }

    loaded.insert(
        key.clone(),
        Entry {
            image: None,
            used: Instant::now(),
        },
    );
    drop(loaded);

    queue(key);

    changes::note_read(TypeId::of::<Decoded>());

    None
}

/*
 * `path` to draw over `pixels` (the area it fills, in real pixels): decoded only big enough
 * for that, at least PROBE and rounded up to a power of two, so a photo or a 512-pixel icon
 * drawn small keeps a small copy, on the cpu and on the gpu. While that copy decodes,
 * another copy of the same file stands in, so a size that changes (an animation, a window
 * resized) never shows a gap. AMANE_IMAGE_FULL=1 decodes every file at its own size
 */
pub fn load_drawn(path: &Path, pixels: (f32, f32), blur: u32) -> Option<Arc<Bitmap>> {
    static FULL: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("AMANE_IMAGE_FULL").is_some_and(|v| v == "1"));
    if *FULL {
        return load(path, None, blur);
    }

    let bucket = |v: f32| (v.max(1.0).ceil() as u32).next_power_of_two();
    let want = (bucket(pixels.0).max(PROBE.0), bucket(pixels.1).max(PROBE.1));
    if let Some(image) = load(path, Some(want), blur) {
        return Some(image);
    }

    // the largest copy of the file already decoded, with the same blur
    let loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    loaded
        .iter()
        .filter(|((p, _, b), _)| p == path && *b == blur)
        .filter_map(|(_, entry)| entry.image.clone())
        .max_by_key(|image| image.width * image.height)
}

/// Whether `path` is decoded, at least as the small copy drawing starts from; starts that if
/// nothing asked for it yet. A file that can't be read never becomes ready.
pub fn ready(path: &Path) -> bool {
    load(path, Some(PROBE), 0).is_some()
}

/// The file's own size in pixels once it's decoded (starting that if need be).
pub fn size(path: &Path) -> Option<(u32, u32)> {
    if let Some(size) = SIZES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
    {
        return Some(*size);
    }
    load(path, Some(PROBE), 0)?;
    SIZES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .copied()
}

// waiting decodes, served by up to DECODERS threads that park when there are none
static QUEUE: Mutex<(VecDeque<Key>, usize)> = Mutex::new((VecDeque::new(), 0));
static WORK: Condvar = Condvar::new();
const DECODERS: usize = 2;

fn queue(key: Key) {
    let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
    queue.0.push_back(key);
    if queue.1 < DECODERS {
        queue.1 += 1;
        let spawned = thread::Builder::new()
            .name(String::from("amane-decode"))
            .spawn(decoder);
        if spawned.is_err() {
            queue.1 -= 1;
        }
    }
    drop(queue);
    WORK.notify_one();
}

fn decoder() {
    loop {
        let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        let key = loop {
            if let Some(key) = queue.0.pop_front() {
                break key;
            }
            queue = WORK.wait(queue).unwrap_or_else(PoisonError::into_inner);
        };
        drop(queue);
        decode(key);
    }
}

// the copies kept now come to more than the cap: the longest unused go, not one shown now
fn evict(loaded: &mut HashMap<Key, Entry>) {
    let bytes = |entry: &Entry| {
        entry
            .image
            .as_ref()
            .map_or(0, |image| image.width as usize * image.height as usize * 4)
    };
    let mut total: usize = loaded.values().map(bytes).sum();
    if total <= *CAP {
        return;
    }
    let mut old: Vec<(Instant, Key, usize)> = loaded
        .iter()
        .filter(|(_, entry)| entry.image.is_some() && entry.used.elapsed() > KEPT_WHILE_SHOWN)
        .map(|(key, entry)| (entry.used, key.clone(), bytes(entry)))
        .collect();
    old.sort_by_key(|(used, ..)| *used);
    for (_, key, size) in old {
        if total <= *CAP {
            break;
        }
        loaded.remove(&key);
        total -= size;
    }
    crate::allocator::trim_soon();
}

fn decode(key: Key) {
    let (path, cover, blur) = &key;

    // a broken file stays empty instead of taking the shell down
    let Some(image) = read(path) else {
        return;
    };

    SIZES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.clone(), (image.width, image.height));

    // only the small copy is kept, the full size one is dropped here
    let image = match cover {
        Some((width, height)) => shrink::to_cover(image, *width, *height),
        None => image,
    };

    // blurred after shrinking, where there are far fewer pixels to average
    let image = soften::soften(image, *blur);

    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    // asked for and since let go of: kept anyway, it was just made
    loaded.insert(
        key,
        Entry {
            image: Some(Arc::new(image)),
            used: Instant::now(),
        },
    );
    evict(&mut loaded);
    drop(loaded);

    changes::mark(TypeId::of::<Decoded>());

    // the decoder's buffers were freed on this thread's pool
    crate::allocator::trim_soon();
}

/*
 * frees a full size image no window shows anymore; small copies stay,
 * they cost little and decoding them again is slow
 */
pub fn forget(image: &Arc<Bitmap>) {
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);

    let before = loaded.len();
    loaded.retain(|(_, cover, _), kept| {
        let same = kept
            .image
            .as_ref()
            .is_some_and(|kept| Arc::ptr_eq(kept, image));

        cover.is_some() || !same
    });

    if loaded.len() != before {
        crate::allocator::trim_soon();
    }
}

// none for a file that can't be read or decoded, like one still being written
pub fn read(path: &Path) -> Option<Bitmap> {
    let bytes = std::fs::read(path).ok()?;

    // the file's first bytes say its format, whatever its name ends in
    if bytes.starts_with(&PNG_SIGNATURE) {
        return decode_png(&bytes);
    }

    if bytes.starts_with(&JPEG_SIGNATURE) {
        return decode_jpeg(&bytes);
    }

    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return svg::rasterize(&bytes);
    }

    // only png, jpeg and svg are supported
    None
}

fn decode_png(bytes: &[u8]) -> Option<Bitmap> {
    let mut decoder = Decoder::new(Cursor::new(bytes));

    // palettes, tiny bit depths and 16 bit samples all turn into plain 8 bit channels
    decoder.set_transformations(Transformations::normalize_to_color8());

    let mut reader = decoder.read_info().ok()?;

    let size = reader.output_buffer_size()?;

    let mut pixels = vec![0; size];

    let frame = reader.next_frame(&mut pixels).ok()?;

    pixels.truncate(frame.buffer_size());

    let bitmap = Bitmap {
        width: frame.width,
        height: frame.height,
        pixels: Arc::new(expand(&pixels, frame.color_type)),
    };

    Some(bitmap)
}

// fills in the channels a png left out, so every pixel is red, green, blue, alpha
fn expand(pixels: &[u8], color_type: ColorType) -> Vec<u8> {
    match color_type {
        ColorType::Rgba => pixels.to_vec(),

        ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),

        ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),

        ColorType::Grayscale => pixels
            .iter()
            .flat_map(|&gray| [gray, gray, gray, 255])
            .collect(),

        ColorType::Indexed => panic!("failed to decode png: palette was not expanded"),
    }
}

fn decode_jpeg(bytes: &[u8]) -> Option<Bitmap> {
    // a bitmap has four channels, jpeg only stores three
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);

    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);

    let pixels = decoder.decode().ok()?;

    let info = decoder.info()?;

    let bitmap = Bitmap {
        width: u32::from(info.width),
        height: u32::from(info.height),
        pixels: Arc::new(pixels),
    };

    Some(bitmap)
}
