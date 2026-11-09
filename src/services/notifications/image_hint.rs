#![warn(clippy::undocumented_unsafe_blocks)]
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::BufWriter;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};

use png::{BitDepth, ColorType, Encoder};

use crate::Value;
use crate::graphics::image;

// the spec renamed this hint twice, senders still use all three names
const DATA_HINTS: [&str; 3] = ["image-data", "image_data", "icon_data"];

const PATH_HINTS: [&str; 2] = ["image-path", "image_path"];

const FOLDER: &str = "amane-notifications";

// each saved image gets a new name, so a replaced notification never shows a cached old one
static NEXT_NAME: AtomicU32 = AtomicU32::new(0);

/*
 * raw pixels are written to a png, so they are drawn like any image
 * file; a sender's own image path comes back as it was sent
 */
pub fn read(hints: &Value) -> String {
    for name in DATA_HINTS {
        if let Some(path) = save(hints.get(name)) {
            return path.to_string_lossy().into_owned();
        }
    }

    for name in PATH_HINTS {
        let path = hints.get(name).text();

        if !path.is_empty() {
            return String::from(path);
        }
    }

    String::new()
}

// only images saved here are removed, never a file the sender owns
pub fn remove(image: &str) {
    let path = Path::new(image);

    if folder().is_some_and(|folder| path.parent() == Some(folder.as_path())) {
        let _ = fs::remove_file(path);
    }
}

/*
 * in the runtime directory, which only this user can enter; none without one, rather than
 * /tmp, where another user could put a link where an image is about to be written
 */
fn folder() -> Option<PathBuf> {
    let runtime_directory = env::var_os("XDG_RUNTIME_DIR").filter(|path| !path.is_empty())?;

    let runtime_directory = PathBuf::from(runtime_directory);

    runtime_directory
        .is_absolute()
        .then(|| runtime_directory.join(FOLDER))
}

/*
 * the pixels' layout as the hint gives it, checked against the data it carries before
 * anything is allocated: a few bytes may claim a 100000 by 100000 image
 */
fn layout(
    width: f64,
    height: f64,
    rowstride: f64,
    bits: f64,
    channels: f64,
    length: usize,
) -> Option<(u32, u32, usize, usize)> {
    let whole = |value: f64| {
        (value.fract() == 0.0 && value >= 0.0 && value <= f64::from(u32::MAX))
            .then_some(value as u32)
    };

    let width = whole(width)?;
    let height = whole(height)?;
    let rowstride = whole(rowstride)? as usize;
    let channels = whole(channels)? as usize;

    // the spec only allows 8 bit rgb or rgba
    if bits != 8.0 || !(channels == 3 || channels == 4) || !image::fits(width, height) {
        return None;
    }

    let row = (width as usize).checked_mul(channels)?;

    if rowstride < row {
        return None;
    }

    // the last row may stop at its last pixel
    let needed = rowstride
        .checked_mul(height as usize - 1)?
        .checked_add(row)?;

    (length >= needed).then_some((width, height, rowstride, channels))
}

// the hint is (width, height, rowstride, has_alpha, bits_per_sample, channels, data)
fn save(hint: &Value) -> Option<PathBuf> {
    let [width, height, rowstride, _has_alpha, bits, channels, data] = hint.list() else {
        return None;
    };

    let data = data.list();

    let (width, height, rowstride, channels) = layout(
        width.number(),
        height.number(),
        rowstride.number(),
        bits.number(),
        channels.number(),
        data.len(),
    )?;
    let (width, height) = (width as usize, height as usize);

    let mut pixels = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        for x in 0..width {
            let start = y * rowstride + x * channels;

            let pixel = data.get(start..start + channels)?;

            for channel in pixel {
                pixels.push(channel.number() as u8);
            }

            if channels == 3 {
                pixels.push(255);
            }
        }
    }

    let folder = folder()?;

    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .ok()?;

    let number = NEXT_NAME.fetch_add(1, Ordering::Relaxed);

    let path = folder.join(format!("{}-{number}.png", process::id()));

    // a new file only, never through a link left where it's written
    let _ = fs::remove_file(&path);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    let file = BufWriter::new(file);

    let mut encoder = Encoder::new(file, width as u32, height as u32);

    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);

    encoder
        .write_header()
        .ok()?
        .write_image_data(&pixels)
        .ok()?;

    Some(path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::graphics::image;

    #[test]
    fn a_hint_must_carry_the_pixels_it_claims() {
        // fine: 2 by 1 rgb, rows of 8 bytes, the last row ending at its last pixel
        assert_eq!(layout(2.0, 1.0, 8.0, 8.0, 3.0, 6), Some((2, 1, 8, 3)));

        // a few bytes claiming a 100000 by 100000 image
        assert_eq!(layout(100_000.0, 100_000.0, 400_000.0, 8.0, 4.0, 16), None);

        // fewer bytes than the rows need, rows shorter than a row of pixels
        assert_eq!(layout(2.0, 2.0, 8.0, 8.0, 3.0, 10), None);
        assert_eq!(layout(4.0, 1.0, 8.0, 8.0, 3.0, 64), None);

        // not whole numbers, negative, other bit depths or channel counts
        assert_eq!(layout(1.5, 1.0, 8.0, 8.0, 3.0, 64), None);
        assert_eq!(layout(-1.0, 1.0, 8.0, 8.0, 3.0, 64), None);
        assert_eq!(layout(1.0, 1.0, 8.0, 16.0, 3.0, 64), None);
        assert_eq!(layout(1.0, 1.0, 8.0, 8.0, 2.0, 64), None);
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
    fn a_layout_taken_always_fits_its_data() {
        let mut noise = Noise(0xD1B5_4A32_D192_ED03);
        let pick = |noise: &mut Noise| match noise.below(4) {
            0 => noise.below(8) as f64,
            1 => noise.below(5000) as f64,
            2 => f64::from(u32::MAX) - noise.below(4) as f64,
            _ => noise.below(1 << 40) as f64 * if noise.below(2) == 0 { 1.0 } else { -1.0 },
        };
        for _ in 0..200_000 {
            let (width, height, rowstride) = (pick(&mut noise), pick(&mut noise), pick(&mut noise));
            let bits = [8.0, 16.0, 7.5][noise.below(3) as usize];
            let channels = noise.below(6) as f64;
            let length = noise.below(1 << 26) as usize;
            if let Some((w, h, stride, channels)) =
                layout(width, height, rowstride, bits, channels, length)
            {
                // what save() reads, last pixel included, is inside the data
                let last = stride * (h as usize - 1) + (w as usize - 1) * channels + channels;
                assert!(last <= length);
                assert!(image::fits(w, h));
            }
        }
    }

    #[test]
    fn saves_image_data_as_png() {
        let number = |value: f64| Value::Number(value);

        // two rgb pixels in a row padded to 8 bytes
        let bytes = [255.0, 0.0, 0.0, 0.0, 255.0, 0.0, 9.0, 9.0];

        let hint = Value::List(vec![
            number(2.0),
            number(1.0),
            number(8.0),
            Value::Bool(false),
            number(8.0),
            number(3.0),
            Value::List(bytes.map(number).to_vec()),
        ]);

        let hints = Value::Map(BTreeMap::from([(String::from("image-data"), hint)]));

        let path = read(&hints);

        let bitmap = image::read(Path::new(&path)).expect("failed to read saved image");

        remove(&path);

        assert_eq!(*bitmap.pixels, [255, 0, 0, 255, 0, 255, 0, 255]);
        assert!(!Path::new(&path).exists());
    }
}
