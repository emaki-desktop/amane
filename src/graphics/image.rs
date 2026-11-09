mod shrink;
mod soften;
mod svg;

use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use png::{ColorType, Decoder, Limits, Transformations};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::changes;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];

/*
 * images come from other programs too (a notification's icon, a tray item's, an album's art
 * fetched from the web), so a file is read only up to a size, and an image only decoded up to
 * a size, checked from its header before anything is allocated; an 8K wallpaper still fits
 */
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

// a side and a whole image, in pixels: 72 MP is 288 MB of RGBA
pub(crate) const MAX_SIDE: u32 = 16384;
pub(crate) const MAX_PIXELS: u64 = 72_000_000;

// svg is text, and an icon's is a few kB; a bigger one is mostly a way to make the parser work
const MAX_SVG_BYTES: u64 = 4 * 1024 * 1024;

/// Whether an image this big may be decoded.
pub(crate) fn fits(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

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
    let bytes = read_file(path)?;

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
        if bytes.len() as u64 > MAX_SVG_BYTES {
            return None;
        }

        return svg::rasterize(&bytes);
    }

    // only png, jpeg and svg are supported
    None
}

/*
 * a regular file's bytes, up to MAX_FILE_BYTES: never a device like /dev/zero, which never
 * ends, or a fifo, which would hold a decoder thread for good (opened without blocking, so
 * even opening one can't)
 */
fn read_file(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .ok()?;

    let metadata = file.metadata().ok()?;

    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);

    // a file still growing stops at the limit all the same
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;

    (bytes.len() as u64 <= MAX_FILE_BYTES).then_some(bytes)
}

fn decode_png(bytes: &[u8]) -> Option<Bitmap> {
    // the decoder's own allocations, beside the frame below, which its limit doesn't count
    let limits = Limits {
        bytes: MAX_FILE_BYTES as usize,
    };

    let mut decoder = Decoder::new_with_limits(Cursor::new(bytes), limits);

    // palettes, tiny bit depths and 16 bit samples all turn into plain 8 bit channels
    decoder.set_transformations(Transformations::normalize_to_color8());

    let mut reader = decoder.read_info().ok()?;

    // the header says how big: a few bytes may claim 100000 by 100000
    let info = reader.info();
    if !fits(info.width, info.height) {
        return None;
    }

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
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_SIDE as usize)
        .set_max_height(MAX_SIDE as usize);

    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);

    // the header first, so a huge image is refused before its pixels are allocated
    decoder.decode_headers().ok()?;

    let (width, height) = decoder.dimensions()?;
    if !fits(u32::try_from(width).ok()?, u32::try_from(height).ok()?) {
        return None;
    }

    let pixels = decoder.decode().ok()?;

    let info = decoder.info()?;

    let bitmap = Bitmap {
        width: u32::from(info.width),
        height: u32::from(info.height),
        pixels: Arc::new(pixels),
    };

    Some(bitmap)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use super::*;

    // a file of its own for this test, removed when dropped
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let folder =
                std::env::temp_dir().join(format!("amane-image-test-{}", std::process::id()));
            std::fs::create_dir_all(&folder).expect("failed to make the test folder");
            Self(folder.join(name))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn devices_are_never_read() {
        assert!(read(Path::new("/dev/zero")).is_none());
        assert!(read(Path::new("/dev/urandom")).is_none());
    }

    #[test]
    fn a_fifo_neither_blocks_nor_reads() {
        let fifo = Scratch::new("fifo.png");
        let name = std::ffi::CString::new(fifo.0.as_os_str().as_encoded_bytes()).expect("a path");
        // SAFETY: a valid C string and a mode
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

        // would hang forever if opening or reading blocked
        assert!(read(&fifo.0).is_none());
    }

    #[test]
    fn a_png_header_claiming_a_huge_image_is_refused_before_allocating() {
        // a valid header for 100000 by 100000 rgba, and no pixels
        let mut png = PNG_SIGNATURE.to_vec();
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(100_000u32.to_be_bytes());
        ihdr.extend(100_000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc32(&ihdr).to_be_bytes());

        assert!(decode_png(&png).is_none());
    }

    #[test]
    fn an_svg_cant_pull_in_another_file() {
        // a real png, which an svg's <image> names
        let png = Scratch::new("pixel.png");
        let mut encoder = png::Encoder::new(
            std::fs::File::create(&png.0).expect("failed to make the png"),
            1,
            1,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&[255, 0, 0, 255]))
            .expect("failed to write the png");
        let href = png.0.to_string_lossy();

        // usvg's own resolver reads it, whatever file it is
        let default = resvg::usvg::Options::default();
        assert!((default.image_href_resolver.resolve_string)(&href, &default).is_some());

        // ours never opens it
        let ours = svg::options();
        assert!((ours.image_href_resolver.resolve_string)(&href, &ours).is_none());

        // and the svg naming it still draws, without it
        let svg = Scratch::new("outside.svg");
        let text = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="{href}"/></svg>"#
        );
        std::fs::File::create(&svg.0)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .expect("failed to write the svg");
        assert!(read(&svg.0).is_some());
    }

    // a small seeded generator, so a failure is the same every run
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn any_png_header_decodes_or_is_refused_quickly() {
        let mut noise = Noise(0xA076_1D64_78BD_642F);
        for _ in 0..5_000 {
            let width = [1, 7, 4096, 16385, 100_000, u32::MAX][noise.below(6) as usize];
            let height = [1, 9, 4096, 16385, 100_000, u32::MAX][noise.below(6) as usize];
            let depth = [1u8, 2, 4, 8, 16, 3][noise.below(6) as usize];
            let color = [0u8, 2, 3, 4, 6, 5][noise.below(6) as usize];
            let mut png = PNG_SIGNATURE.to_vec();
            let mut ihdr = b"IHDR".to_vec();
            ihdr.extend(width.to_be_bytes());
            ihdr.extend(height.to_be_bytes());
            ihdr.extend([depth, color, 0, 0, 0]);
            png.extend(13u32.to_be_bytes());
            png.extend(&ihdr);
            png.extend(crc32(&ihdr).to_be_bytes());
            // and some noise after, as the rest of a file
            png.extend((0..noise.below(64)).map(|_| noise.next() as u8));

            if let Some(bitmap) = decode_png(&png) {
                assert!(fits(bitmap.width, bitmap.height));
            }
        }
    }

    #[test]
    fn sizes_that_fit() {
        assert!(fits(7680, 4320));
        assert!(!fits(0, 10));
        assert!(!fits(MAX_SIDE + 1, 1));
        assert!(!fits(MAX_SIDE, MAX_SIDE));
    }

    // png's chunk checksum, for the hand-made header above
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
