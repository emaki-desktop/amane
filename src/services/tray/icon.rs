/*
 * an item's icon as a file the bar can show: a theme icon by name (from
 * the item's own theme folder first), a path, or the pixels it sends,
 * written once as a png into the cache
 */

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::Value;
use crate::services::apps;

/*
 * the icon named `name` or else the best of `pixmaps` for `size` pixels;
 * none when it has neither
 */
pub fn find(name: &str, pixmaps: &Value, theme_path: &str, size: u32) -> Option<PathBuf> {
    if !name.is_empty() {
        let named = in_folder(name, theme_path).or_else(|| apps::find_icon(name));

        if named.is_some() {
            return named;
        }
    }

    let (width, height, argb) = best(pixmaps, size)?;

    write(width, height, &argb)
}

// an icon an item keeps in a folder of its own, like Steam's or an AppImage's
fn in_folder(name: &str, folder: &str) -> Option<PathBuf> {
    // a name, never a path: one with a / in it would reach out of the folder
    if folder.is_empty() || name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }

    find_in(Path::new(folder), name, 4)
}

// how many entries of each folder are looked in for an item's icon
const FOLDERS_LOOKED_IN: usize = 64;

fn find_in(folder: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    for extension in ["svg", "png"] {
        let file = folder.join(format!("{name}.{extension}"));

        if file.is_file() {
            return Some(file);
        }
    }

    if depth == 0 {
        return None;
    }

    // the biggest sizes sort last by name rarely, so any match will do
    // an item names its folder: one like / mustn't have the whole disk walked
    let mut folders: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .take(FOLDERS_LOOKED_IN)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();

    folders.sort();
    folders.reverse();

    folders
        .iter()
        .find_map(|folder| find_in(folder, name, depth - 1))
}

/*
 * the pixmap nearest `size` from the list of (width, height, argb bytes):
 * the smallest at least that big, else the biggest
 */
fn best(pixmaps: &Value, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let mut found: Vec<(u32, u32, &[Value])> = pixmaps
        .list()
        .iter()
        .filter_map(|pixmap| {
            let [width, height, bytes] = pixmap.list() else {
                return None;
            };

            let (width, height) = (width.number() as u32, height.number() as u32);
            let bytes = bytes.list();

            // checked, so a sender's sizes can't wrap round to the bytes it sent
            let expected = (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4));
            (width > 0
                && height > 0
                && crate::graphics::image::fits(width, height)
                && expected == Some(bytes.len()))
            .then_some((width, height, bytes))
        })
        .collect();

    found.sort_by_key(|(width, ..)| *width);

    let chosen = found
        .iter()
        .find(|(width, ..)| *width >= size)
        .or_else(|| found.last())?;

    let argb = chosen.2.iter().map(|byte| byte.number() as u8).collect();

    Some((chosen.0, chosen.1, argb))
}

// argb in network byte order, as the protocol sends it, to rgba
fn to_rgba(argb: &[u8]) -> Vec<u8> {
    argb.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[a, r, g, b]| [r, g, b, a])
        .collect()
}

// named by what it holds, so a changed icon is a new file the bar loads afresh
fn write(width: u32, height: u32, argb: &[u8]) -> Option<PathBuf> {
    let rgba = to_rgba(argb);

    let folder = super::menu::cache()?;
    let file = folder.join(format!("{:016x}-{width}.png", fnv(&rgba)));

    if file.is_file() {
        return Some(file);
    }

    fs::create_dir_all(&folder).ok()?;

    let partial = file.with_extension("part");

    {
        let out = BufWriter::new(fs::File::create(&partial).ok()?);
        let mut encoder = png::Encoder::new(out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }

    fs::rename(&partial, &file).ok()?;

    Some(file)
}

pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixmap(size: u32) -> Value {
        let bytes = vec![Value::Number(255.0); (size * size * 4) as usize];

        Value::List(vec![
            Value::Number(f64::from(size)),
            Value::Number(f64::from(size)),
            Value::List(bytes),
        ])
    }

    #[test]
    fn picks_the_smallest_pixmap_big_enough() {
        let pixmaps = Value::List(vec![pixmap(16), pixmap(64), pixmap(32)]);

        assert_eq!(best(&pixmaps, 30).map(|found| found.0), Some(32));
        assert_eq!(best(&pixmaps, 100).map(|found| found.0), Some(64));
        assert_eq!(best(&Value::Nothing, 30), None);
    }

    #[test]
    fn a_pixmap_of_the_wrong_length_is_left_out() {
        let broken = Value::List(vec![
            Value::Number(4.0),
            Value::Number(4.0),
            Value::List(vec![Value::Number(1.0)]),
        ]);

        assert_eq!(best(&Value::List(vec![broken]), 4), None);
    }

    #[test]
    fn argb_turns_into_rgba() {
        assert_eq!(to_rgba(&[10, 20, 30, 40]), [20, 30, 40, 10]);
    }
}
