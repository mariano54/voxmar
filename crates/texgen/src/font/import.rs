//! Glyphs read from someone else's pixel font, for the scripts our own fonts don't draw.
//!
//! Japanese needs kana and thousands of kanji, and good Japanese pixel fonts already exist
//! as TrueType files whose outlines are squares on a pixel grid. [`import`] fills each
//! glyph's outline at the centre of every pixel, which recovers the original drawing
//! exactly, and moves it onto our grid so it sits on the same baseline as our letters.
//! Merged under a font drawn as text ([`PixelFont::fill_from`]), it supplies only what
//! that font lacks, and everything comes out as one font with one set of metrics. The
//! file it reads ships unmodified; the game imports it when it starts.

use super::*;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// Characters whose ink sets where a font's glyphs sit: the first it has. Kanji and kana
/// fill the box a font draws them in, so centring them centres everything.
const REFERENCE: [char; 6] = ['漢', '国', '語', 'あ', 'ア', 'H'];

/// The row a font's reference glyph is centred on: the middle of a capital letter.
const CENTRE: f32 = (CAP_HEIGHT - 1) as f32 / 2.0;

/// Reads every glyph of a pixel-outline TrueType or OpenType font drawn `pixels_per_em`
/// pixels to the em, each of its pixels `scale` of ours (1 for the same size; ¾ draws a
/// 12-pixel font about as tall as a 9-pixel one, with all its detail, crisp wherever a
/// screen has pixels to spare). `scale` must make a whole number of font units per pixel.
/// Characters outside the Basic Multilingual Plane and empty glyphs other than spaces are
/// left out.
pub fn import(data: &[u8], pixels_per_em: u32, scale: f32) -> Result<PixelFont, Error> {
    let units_per_pixel = (UNITS_PER_PIXEL as f32 * scale).round() as i32;
    if !(1..=UNITS_PER_PIXEL).contains(&units_per_pixel) || (units_per_pixel as f32 - UNITS_PER_PIXEL as f32 * scale).abs() > 1e-3 {
        return Err(Error(format!("font: a scale of {scale} is not a whole number of font units")));
    }
    let font = FontRef::new(data).map_err(|e| Error(format!("font: cannot read: {e}")))?;
    let units_per_em = font.metrics(Size::unscaled(), LocationRef::default()).units_per_em;
    if units_per_em == 0 || pixels_per_em == 0 {
        return Err(Error("font: no size".into()));
    }
    let unit = units_per_em as f32 / pixels_per_em as f32;
    let outlines = font.outline_glyphs();
    let advances = font.glyph_metrics(Size::unscaled(), LocationRef::default());

    // Each glyph's ink as pixels, x right from its origin and y up from the baseline.
    struct Drawn {
        advance: i32,
        ink: Vec<(i32, i32)>,
    }
    let mut drawn: BTreeMap<char, Drawn> = BTreeMap::new();
    let mut pen = Edges::default();
    for (code, glyph) in font.charmap().mappings() {
        let Some(c) = char::from_u32(code).filter(|c| (*c as u32) <= 0xFFFF && !c.is_control()) else { continue };
        let Some(outline) = outlines.get(glyph) else { continue };
        pen.clear();
        if outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).is_err() {
            continue;
        }
        let ink = pen.fill(unit);
        let advance = (advances.advance_width(glyph).unwrap_or(0.0) / unit).round() as i32;
        if ink.is_empty() && !c.is_whitespace() {
            continue;
        }
        drawn.insert(c, Drawn { advance, ink });
    }

    // Centre the reference glyph on a capital letter's middle, keeping it inside the
    // lines our fonts reserve where it fits.
    let reference = REFERENCE.iter().find_map(|c| drawn.get(c).filter(|d| !d.ink.is_empty()));
    // On a finer grid, our rows are 1 / scale of its rows.
    let ours = |rows: f32| (rows / scale).floor() as i32;
    let shift = reference.map_or(0, |d| {
        let bottom = d.ink.iter().map(|p| p.1).min().unwrap_or(0);
        let top = d.ink.iter().map(|p| p.1).max().unwrap_or(0);
        let shift = (CENTRE / scale).round() as i32 - (bottom + top).div_euclid(2);
        shift.min(ours(ASCENT as f32) - 1 - top).max(-ours(DESCENT as f32) - bottom)
    });

    let mut out = PixelFont::default();
    for (c, d) in drawn {
        let left = d.ink.iter().map(|p| p.0).min().unwrap_or(0).min(0);
        let right = d.ink.iter().map(|p| p.0 + 1 - left).max().unwrap_or(0);
        let width = (d.advance - SPACING).max(right).max(1);
        let row = |y: i32| CAP_HEIGHT - 1 - (y + shift);
        let top = d.ink.iter().map(|p| row(p.1)).min().unwrap_or(0);
        let bottom = d.ink.iter().map(|p| row(p.1)).max().unwrap_or(-1);
        let mut rows = vec![vec![false; width as usize]; (bottom - top + 1) as usize];
        for &(x, y) in &d.ink {
            rows[(row(y) - top) as usize][(x - left) as usize] = true;
        }
        out.glyphs.insert(c, Bitmap { width, top, rows });
        if units_per_pixel != UNITS_PER_PIXEL {
            out.units.insert(c, units_per_pixel);
        }
    }
    Ok(out)
}

impl PixelFont {
    /// Adds `other`'s glyphs for the characters this font doesn't draw.
    pub fn fill_from(&mut self, other: &PixelFont) {
        for (c, glyph) in &other.glyphs {
            if !self.glyphs.contains_key(c) {
                self.glyphs.insert(*c, glyph.clone());
                if let Some(&units) = other.units.get(c) {
                    self.units.insert(*c, units);
                }
            }
        }
    }
}

/// An outline as straight edges, curves flattened, in font units.
#[derive(Default)]
struct Edges {
    edges: Vec<[(f32, f32); 2]>,
    start: (f32, f32),
    at: (f32, f32),
}

impl Edges {
    fn clear(&mut self) {
        self.edges.clear();
    }

    fn edge(&mut self, to: (f32, f32)) {
        if to != self.at {
            self.edges.push([self.at, to]);
        }
        self.at = to;
    }

    /// The pixels whose centres the outline encloses (non-zero winding), scanning each
    /// pixel row's centre line across the edges.
    fn fill(&self, unit: f32) -> Vec<(i32, i32)> {
        let mut pixels = Vec::new();
        if self.edges.is_empty() {
            return pixels;
        }
        let low = self.edges.iter().flatten().map(|p| p.1).fold(f32::MAX, f32::min);
        let high = self.edges.iter().flatten().map(|p| p.1).fold(f32::MIN, f32::max);
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        for y in (low / unit).floor() as i32..(high / unit).ceil() as i32 {
            let line = (y as f32 + 0.5) * unit;
            crossings.clear();
            for &[(x0, y0), (x1, y1)] in &self.edges {
                if (y0 <= line) != (y1 <= line) {
                    let x = x0 + (line - y0) / (y1 - y0) * (x1 - x0);
                    crossings.push((x, if y1 > y0 { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                if winding != 0 {
                    // Pixels whose centres lie between the two crossings.
                    let first = (pair[0].0 / unit - 0.5).ceil() as i32;
                    let last = (pair[1].0 / unit - 0.5).ceil() as i32;
                    pixels.extend((first..last).map(|x| (x, y)));
                }
            }
        }
        pixels.sort_unstable();
        pixels.dedup();
        pixels
    }
}

impl OutlinePen for Edges {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.at = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.edge((x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let from = self.at;
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            self.edge((u * u * from.0 + 2.0 * u * t * cx + t * t * x, u * u * from.1 + 2.0 * u * t * cy + t * t * y));
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let from = self.at;
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.edge((a * from.0 + b * cx0 + c * cx1 + d * x, a * from.1 + b * cy0 + c * cy1 + d * y));
        }
    }

    fn close(&mut self) {
        self.edge(self.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square outline from (x0, y0) to (x1, y1) in pixels, drawn either way round.
    fn square(edges: &mut Edges, unit: f32, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
        edges.move_to(x0 * unit, y0 * unit);
        edges.line_to(x1 * unit, y0 * unit);
        edges.line_to(x1 * unit, y1 * unit);
        edges.line_to(x0 * unit, y1 * unit);
        edges.close();
    }

    #[test]
    fn fills_exactly_the_pixels_an_outline_covers() {
        let mut edges = Edges::default();
        square(&mut edges, 100.0, (0.0, 0.0), (3.0, 2.0));
        assert_eq!(edges.fill(100.0), [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]);

        // A ring: an outer square with a hole drawn the other way round.
        let mut edges = Edges::default();
        square(&mut edges, 64.0, (0.0, -1.0), (3.0, 2.0));
        square(&mut edges, 64.0, (1.0, 1.0), (2.0, 0.0));
        let ring = edges.fill(64.0);
        assert_eq!(ring.len(), 8);
        assert!(!ring.contains(&(1, 0)));
        assert!(ring.contains(&(0, -1)));
    }
}
