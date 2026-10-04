//! Contact sheets for reviewing a pack: each texture large, tiled (to judge seams and
//! repetition), small (to judge it at a distance) and optionally beside a reference.

use crate::Image;

/// One texture on the sheet.
pub struct Entry {
    pub name: String,
    pub image: Image,
    /// The same name from a reference pack, drawn alongside.
    pub reference: Option<Image>,
}

const GAP: u32 = 6;
const LABEL: u32 = 16;
const BACKGROUND: [u8; 4] = [46, 48, 54, 255];

/// `size` is the side of each box in pixels.
pub fn contact_sheet(entries: &[Entry], columns: u32, size: u32) -> Image {
    let size = size.max(48);
    let with_reference = entries.iter().any(|e| e.reference.is_some());
    let boxes = if with_reference { 4 } else { 3 };
    let cell_w = boxes * (size + GAP) + GAP;
    let cell_h = size + LABEL + 2 * GAP;
    let columns = columns.clamp(1, entries.len().max(1) as u32);
    let rows = (entries.len() as u32).div_ceil(columns).max(1);
    let mut sheet = Image::filled(columns * cell_w, rows * cell_h, BACKGROUND);
    for (k, entry) in entries.iter().enumerate() {
        let (cx, cy) = ((k as u32 % columns) * cell_w, (k as u32 / columns) * cell_h);
        let mut x = cx + GAP;
        let y = cy + GAP;
        let mut place = |sheet: &mut Image, image: &Image| {
            checker(sheet, x, y, size, size);
            let fitted = fit(image, size, size);
            sheet.draw(&fitted, (x + (size - fitted.width) / 2) as i64, (y + (size - fitted.height) / 2) as i64, 1);
            x += size + GAP;
        };
        place(&mut sheet, &entry.image);
        place(&mut sheet, &tiled(&entry.image, 3));
        // At a distance: one pixel per texel, as blocks a few dozen metres away appear.
        let mut far = Image::new(size, size);
        let small = tiled(&entry.image, 3);
        let small = if small.width <= size && small.height <= size { small } else { fit(&small, size, size) };
        far.draw(&small, ((size - small.width) / 2) as i64, ((size - small.height) / 2) as i64, 1);
        place(&mut sheet, &far);
        if let Some(reference) = &entry.reference {
            place(&mut sheet, reference);
        }
        let label: String = entry.name.strip_prefix("block/").unwrap_or(&entry.name).chars().take((cell_w / 8) as usize).collect();
        text(&mut sheet, &label, cx + GAP, cy + GAP + size + 4, 2, [235, 235, 235, 255]);
    }
    sheet
}

/// The image repeated `n`×`n` times.
pub fn tiled(image: &Image, n: u32) -> Image {
    let mut out = Image::new(image.width * n, image.height * n);
    for ty in 0..n {
        for tx in 0..n {
            out.draw(image, (tx * image.width) as i64, (ty * image.height) as i64, 1);
        }
    }
    out
}

/// Nearest-neighbour scaled to fit `w`×`h` (upscaled by whole factors when it is small).
pub fn fit(image: &Image, w: u32, h: u32) -> Image {
    let scale = (w / image.width).min(h / image.height);
    if scale >= 1 {
        let mut out = Image::new(image.width * scale, image.height * scale);
        out.draw(image, 0, 0, scale);
        return out;
    }
    let factor = (image.width as f32 / w as f32).max(image.height as f32 / h as f32);
    let (ow, oh) = (((image.width as f32 / factor) as u32).max(1), ((image.height as f32 / factor) as u32).max(1));
    let mut out = Image::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            out.set(x, y, image.get(((x as f32 * factor) as u32).min(image.width - 1), ((y as f32 * factor) as u32).min(image.height - 1)));
        }
    }
    out
}

fn checker(sheet: &mut Image, x0: u32, y0: u32, w: u32, h: u32) {
    for y in 0..h {
        for x in 0..w {
            let light = ((x / 8) + (y / 8)) % 2 == 0;
            sheet.set(x0 + x, y0 + y, if light { [92, 94, 100, 255] } else { [76, 78, 84, 255] });
        }
    }
}

/// Draws `text` in a 3×5 pixel font, `scale` pixels per font pixel.
pub fn text(sheet: &mut Image, text: &str, x: u32, y: u32, scale: u32, colour: [u8; 4]) {
    for (i, ch) in text.chars().enumerate() {
        let Some(rows) = glyph(ch) else { continue };
        for (row, bits) in rows.iter().enumerate() {
            for (column, bit) in bits.chars().enumerate() {
                if bit != '#' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = x + (i as u32 * 4 + column as u32) * scale + dx;
                        let py = y + row as u32 * scale + dy;
                        if px < sheet.width && py < sheet.height {
                            sheet.set(px, py, colour);
                        }
                    }
                }
            }
        }
    }
}

fn glyph(ch: char) -> Option<[&'static str; 5]> {
    Some(match ch.to_ascii_lowercase() {
        'a' => [".#.", "#.#", "###", "#.#", "#.#"],
        'b' => ["##.", "#.#", "##.", "#.#", "##."],
        'c' => [".##", "#..", "#..", "#..", ".##"],
        'd' => ["##.", "#.#", "#.#", "#.#", "##."],
        'e' => ["###", "#..", "##.", "#..", "###"],
        'f' => ["###", "#..", "##.", "#..", "#.."],
        'g' => [".##", "#..", "#.#", "#.#", ".##"],
        'h' => ["#.#", "#.#", "###", "#.#", "#.#"],
        'i' => ["###", ".#.", ".#.", ".#.", "###"],
        'j' => ["..#", "..#", "..#", "#.#", ".#."],
        'k' => ["#.#", "#.#", "##.", "#.#", "#.#"],
        'l' => ["#..", "#..", "#..", "#..", "###"],
        'm' => ["#.#", "###", "###", "#.#", "#.#"],
        'n' => ["##.", "#.#", "#.#", "#.#", "#.#"],
        'o' => [".#.", "#.#", "#.#", "#.#", ".#."],
        'p' => ["##.", "#.#", "##.", "#..", "#.."],
        'q' => [".#.", "#.#", "#.#", "##.", ".##"],
        'r' => ["##.", "#.#", "##.", "#.#", "#.#"],
        's' => [".##", "#..", ".#.", "..#", "##."],
        't' => ["###", ".#.", ".#.", ".#.", ".#."],
        'u' => ["#.#", "#.#", "#.#", "#.#", "###"],
        'v' => ["#.#", "#.#", "#.#", "#.#", ".#."],
        'w' => ["#.#", "#.#", "###", "###", "#.#"],
        'x' => ["#.#", "#.#", ".#.", "#.#", "#.#"],
        'y' => ["#.#", "#.#", ".#.", ".#.", ".#."],
        'z' => ["###", "..#", ".#.", "#..", "###"],
        '0' => ["###", "#.#", "#.#", "#.#", "###"],
        '1' => [".#.", "##.", ".#.", ".#.", "###"],
        '2' => ["##.", "..#", ".#.", "#..", "###"],
        '3' => ["##.", "..#", ".#.", "..#", "##."],
        '4' => ["#.#", "#.#", "###", "..#", "..#"],
        '5' => ["###", "#..", "##.", "..#", "##."],
        '6' => [".##", "#..", "###", "#.#", "###"],
        '7' => ["###", "..#", ".#.", ".#.", ".#."],
        '8' => ["###", "#.#", "###", "#.#", "###"],
        '9' => ["###", "#.#", "###", "..#", "##."],
        '_' => ["...", "...", "...", "...", "###"],
        '/' => ["..#", "..#", ".#.", "#..", "#.."],
        ':' => ["...", ".#.", "...", ".#.", "..."],
        '-' => ["...", "...", "###", "...", "..."],
        '.' => ["...", "...", "...", "...", ".#."],
        _ => return None,
    })
}
