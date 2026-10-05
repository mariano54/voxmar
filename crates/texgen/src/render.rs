//! Paints a resolved recipe's layers onto a canvas.

use std::collections::BTreeMap;

use crate::colour::{self, Rgba};
use crate::field::{self, Size, hash, unit};
use crate::recipe::{Clusters, Cut, Grid, Layer, Paint, Shift, Stamp};
use crate::{Error, Image};

/// A recipe with its bases folded in: everything needed to paint it.
pub struct Resolved {
    pub size: (u32, u32),
    pub seed: u32,
    pub palettes: BTreeMap<String, Vec<Rgba>>,
    /// Layers with their seeds; `None` takes the seed from `seed` and the layer's index.
    pub layers: Vec<(Option<u32>, Layer)>,
    /// Textures that `Stamp` layers draw, already rendered.
    pub stamps: BTreeMap<String, Image>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Texel {
    Clear,
    Colour(Rgba),
    /// A shade of one of the recipe's palettes, by palette index, so later layers can
    /// shift it within its own ramp.
    Shade(u16, u8),
}

struct Canvas<'a> {
    size: Size,
    texels: Vec<Texel>,
    names: Vec<&'a str>,
    palettes: Vec<&'a [Rgba]>,
}

impl Canvas<'_> {
    fn palette(&self, name: &str) -> Result<u16, Error> {
        self.names.iter().position(|n| *n == name).map(|i| i as u16).ok_or_else(|| Error(format!("no palette `{name}`")))
    }

    /// Lightens (positive) or darkens a texel by whole shades.
    fn shift(&mut self, i: usize, steps: i32) {
        self.texels[i] = match self.texels[i] {
            Texel::Shade(p, s) => {
                let top = self.palettes[p as usize].len() as i32 - 1;
                Texel::Shade(p, (s as i32 + steps).clamp(0, top) as u8)
            }
            Texel::Colour(c) => {
                let f = 1.0 + 0.12 * steps as f32;
                Texel::Colour([0, 1, 2, 3].map(|k| if k == 3 { c[3] } else { (c[k] as f32 * f).clamp(0.0, 255.0) as u8 }))
            }
            Texel::Clear => Texel::Clear,
        }
    }

    fn image(&self) -> Image {
        let mut image = Image::new(self.size.w, self.size.h);
        for (i, texel) in self.texels.iter().enumerate() {
            let colour = match *texel {
                Texel::Clear => [0; 4],
                Texel::Colour(c) => c,
                Texel::Shade(p, s) => self.palettes[p as usize][s as usize],
            };
            image.pixels[i * 4..i * 4 + 4].copy_from_slice(&colour);
        }
        image
    }
}

pub fn render(recipe: &Resolved) -> Result<Image, Error> {
    let (w, h) = recipe.size;
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return Err(Error(format!("size {w}×{h} is out of range")));
    }
    let mut canvas = Canvas {
        size: Size { w, h },
        texels: vec![Texel::Clear; (w * h) as usize],
        names: recipe.palettes.keys().map(String::as_str).collect(),
        palettes: recipe.palettes.values().map(Vec::as_slice).collect(),
    };
    for (index, (layer_seed, layer)) in recipe.layers.iter().enumerate() {
        let seed = layer_seed.unwrap_or_else(|| hash(recipe.seed, index as i32, 0x1a7e));
        match layer {
            Layer::Paint(paint) => draw_paint(&mut canvas, paint, seed)?,
            Layer::Grid(grid) => draw_grid(&mut canvas, grid)?,
            Layer::Clusters(clusters) => draw_clusters(&mut canvas, clusters, seed)?,
            Layer::Shift(shift) => draw_shift(&mut canvas, shift, seed),
            Layer::Cut(cut) => draw_cut(&mut canvas, cut, seed),
            Layer::Stamp(stamp) => draw_stamp(&mut canvas, stamp, &recipe.stamps[&stamp.texture]),
            Layer::Corners(corners) => draw_corners(&mut canvas, corners)?,
        }
    }
    Ok(canvas.image())
}

/// Where a mask lets a layer draw: everywhere when it has no terms.
fn mask(terms: &[crate::recipe::Term], above: f32, size: Size, seed: u32) -> Vec<bool> {
    if terms.is_empty() {
        return vec![true; size.len()];
    }
    field::sum(terms, size, hash(seed, 1, 1)).into_iter().map(|v| v > above).collect()
}

fn draw_paint(canvas: &mut Canvas, paint: &Paint, seed: u32) -> Result<(), Error> {
    let size = canvas.size;
    let palette = canvas.palette(&paint.palette)?;
    let count = canvas.palettes[palette as usize].len();
    let shade = field::sum(&paint.shade, size, seed);
    let painted = mask(&paint.mask, paint.above, size, seed);
    let mut shades: Vec<u8> = shade
        .iter()
        .map(|&v| ((paint.base + v).clamp(0.0, 0.9999) * count as f32) as u8)
        .collect();
    if paint.clean {
        let before = shades.clone();
        for y in 0..size.h as i32 {
            for x in 0..size.w as i32 {
                let i = size.index(x, y);
                let around = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dy)| size.index(x + dx, y + dy));
                if !painted[i] || around.iter().any(|&n| !painted[n]) {
                    continue;
                }
                let first = before[around[0]];
                if first != before[i] && around.iter().all(|&n| before[n] == first) {
                    shades[i] = first;
                }
            }
        }
    }
    for i in 0..size.len() {
        if painted[i] {
            let gap = paint.cut.is_some_and(|cut| paint.base + shade[i] < cut);
            canvas.texels[i] = if gap { Texel::Clear } else { Texel::Shade(palette, shades[i]) };
        }
    }
    if paint.shadow != 0 {
        rim(canvas, &painted, paint.shadow, 1);
    }
    if paint.light != 0 {
        rim(canvas, &painted, paint.light, -1);
    }
    Ok(())
}

/// Shifts the texels next to a region (but outside it) by `steps`: those right of and
/// below it for `side` 1, left of and above it for -1.
fn rim(canvas: &mut Canvas, region: &[bool], steps: i32, side: i32) {
    let size = canvas.size;
    for y in 0..size.h as i32 {
        for x in 0..size.w as i32 {
            let i = size.index(x, y);
            if !region[i] && (region[size.index(x - side, y)] || region[size.index(x, y - side)]) {
                canvas.shift(i, steps);
            }
        }
    }
}

enum Ink {
    Colour(Rgba),
    Shade(u16, u8),
    Shift(i32),
    Clear,
}

fn ink(canvas: &Canvas, text: &str) -> Result<Ink, Error> {
    if text == "clear" {
        return Ok(Ink::Clear);
    }
    if text.starts_with('#') {
        return Ok(Ink::Colour(colour::parse(text)?));
    }
    if let Some(steps) = text.strip_prefix('+').or(text.strip_prefix('-').map(|_| text)) {
        return steps.parse().map(Ink::Shift).map_err(|_| Error(format!("`{text}` is not a shade shift")));
    }
    let (name, shade) = text.rsplit_once(':').ok_or_else(|| Error(format!("`{text}` is not a colour, palette:shade, shift or `clear`")))?;
    let palette = canvas.palette(name)?;
    let count = canvas.palettes[palette as usize].len();
    let shade: usize = shade.parse().map_err(|_| Error(format!("`{text}`: `{shade}` is not a shade number")))?;
    if shade >= count {
        return Err(Error(format!("`{text}`: palette `{name}` has only {count} shades")));
    }
    Ok(Ink::Shade(palette, shade as u8))
}

fn draw_grid(canvas: &mut Canvas, grid: &Grid) -> Result<(), Error> {
    // Only the characters the rows use: a book's shared legend may name palettes that a
    // recipe without those characters does not have.
    let legend: BTreeMap<char, Ink> = grid
        .legend
        .iter()
        .filter(|(c, _)| grid.rows.iter().any(|row| row.contains(**c)))
        .map(|(&c, text)| Ok((c, ink(canvas, text)?)))
        .collect::<Result<_, Error>>()?;
    let origins = if grid.at.is_empty() { vec![(0, 0)] } else { grid.at.clone() };
    let size = canvas.size;
    for (ox, oy) in origins {
        for (row, line) in grid.rows.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            for (column, ch) in chars.iter().enumerate() {
                let column = if grid.mirror { chars.len() - 1 - column } else { column };
                let (mut x, mut y) = (ox + column as i32, oy + row as i32);
                if grid.wrap {
                    (x, y) = (x.rem_euclid(size.w as i32), y.rem_euclid(size.h as i32));
                }
                if x < 0 || y < 0 || x >= size.w as i32 || y >= size.h as i32 {
                    continue;
                }
                let i = size.index(x, y);
                match legend.get(ch) {
                    None => {}
                    Some(Ink::Colour(c)) => canvas.texels[i] = Texel::Colour(*c),
                    Some(Ink::Shade(p, s)) => canvas.texels[i] = Texel::Shade(*p, *s),
                    Some(Ink::Shift(steps)) => canvas.shift(i, *steps),
                    Some(Ink::Clear) => canvas.texels[i] = Texel::Clear,
                }
            }
        }
    }
    Ok(())
}

fn draw_clusters(canvas: &mut Canvas, clusters: &Clusters, seed: u32) -> Result<(), Error> {
    let size = canvas.size;
    let palette = canvas.palette(&clusters.palette)?;
    let count = canvas.palettes[palette as usize].len() as i32;
    let middle = clusters.shade.map_or(count / 2, |s| (s as i32).min(count - 1));
    let seed = hash(seed, clusters.seed as i32, 0xc1);
    let (w, h) = (size.w as i32, size.h as i32);
    let margin = clusters.margin as i32;
    let inside = |x: i32, y: i32| margin == 0 || (x >= margin && y >= margin && x < w - margin && y < h - margin);
    let mut centres: Vec<(i32, i32)> = Vec::new();
    let mut members = vec![false; size.len()];
    for k in 0..clusters.count as i32 {
        // Find a spot far enough from the others; give up on this blob after a few tries.
        let spot = (0..24).map(|attempt| {
            let x = margin + (unit(seed, k, attempt * 2) * (w - 2 * margin).max(1) as f32) as i32;
            let y = margin + (unit(seed, k, attempt * 2 + 1) * (h - 2 * margin).max(1) as f32) as i32;
            (x, y)
        });
        let far = |&(x, y): &(i32, i32)| {
            centres.iter().all(|&(cx, cy)| {
                let dx = (x - cx).rem_euclid(w).min((cx - x).rem_euclid(w));
                let dy = (y - cy).rem_euclid(h).min((cy - y).rem_euclid(h));
                ((dx * dx + dy * dy) as f32).sqrt() >= clusters.spacing
            })
        };
        let Some(centre) = spot.clone().find(far) else { continue };
        centres.push(centre);
        let (low, high) = (clusters.size.0.max(1), clusters.size.1.max(clusters.size.0.max(1)));
        let target = low + (unit(seed, k, 100) * (high - low + 1) as f32) as u32;
        // Grow compact blobs: each step adds the neighbouring texel touching the most of
        // the blob, with some randomness and `spread` favouring one direction.
        let mut blob = vec![centre];
        let wrap = |(x, y): (i32, i32)| (x.rem_euclid(w), y.rem_euclid(h));
        for step in 0..target as i32 {
            if blob.len() >= target as usize {
                break;
            }
            let mut best: Option<((i32, i32), f32)> = None;
            for &(bx, by) in &blob {
                for (dx, dy, weight) in [(1, 0, clusters.spread.0), (-1, 0, clusters.spread.0), (0, 1, clusters.spread.1), (0, -1, clusters.spread.1)] {
                    let next = wrap((bx + dx, by + dy));
                    if blob.contains(&next) || !inside(next.0, next.1) {
                        continue;
                    }
                    let touching = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().filter(|(ax, ay)| blob.contains(&wrap((next.0 + ax, next.1 + ay)))).count();
                    let score = touching as f32 + weight * 1.3 * unit(seed, k * 64 + step, next.0 * 31 + next.1);
                    if best.is_none_or(|(_, b)| score > b) {
                        best = Some((next, score));
                    }
                }
            }
            match best {
                Some((next, _)) => blob.push(next),
                None => break,
            }
        }
        let has = |x: i32, y: i32| blob.contains(&(x.rem_euclid(w), y.rem_euclid(h)));
        for &(x, y) in &blob {
            let light = !has(x, y - 1) as i32 + !has(x - 1, y) as i32 - !has(x, y + 1) as i32 - !has(x + 1, y) as i32;
            // Lit edges brighten (the corner most of all), shadowed corners darken, and
            // lone texels keep the middle shade so small blobs still read.
            let step = match light {
                2.. => 2,
                1 => 1,
                ..=-2 => -1,
                _ => 0,
            };
            let shade = (middle + step).clamp(0, count - 1);
            let i = size.index(x, y);
            canvas.texels[i] = Texel::Shade(palette, shade as u8);
            members[i] = true;
        }
    }
    if clusters.shadow != 0 {
        rim(canvas, &members, clusters.shadow, 1);
    }
    Ok(())
}

fn draw_shift(canvas: &mut Canvas, shift: &Shift, seed: u32) {
    let size = canvas.size;
    let by = field::sum(&shift.by, size, seed);
    let allowed = mask(&shift.mask, shift.above, size, seed);
    for i in 0..size.len() {
        let steps = by[i].round() as i32;
        if allowed[i] && steps != 0 {
            canvas.shift(i, steps);
        }
    }
}

fn draw_stamp(canvas: &mut Canvas, stamp: &Stamp, image: &Image) {
    let size = canvas.size;
    let mut stamped = vec![false; size.len()];
    let origins = if stamp.at.is_empty() { vec![(0, 0)] } else { stamp.at.clone() };
    for (ox, oy) in origins {
        for sy in 0..image.height as i32 {
            for sx in 0..image.width as i32 {
                let (x, y) = (ox + sx, oy + sy);
                if x < 0 || y < 0 || x >= size.w as i32 || y >= size.h as i32 {
                    continue;
                }
                let source = if stamp.mirror { image.width as i32 - 1 - sx } else { sx };
                let colour = image.get(source as u32, sy as u32);
                if colour[3] > 0 {
                    canvas.texels[size.index(x, y)] = Texel::Colour(colour);
                    stamped[size.index(x, y)] = true;
                }
            }
        }
    }
    if stamp.shadow != 0 {
        rim(canvas, &stamped, stamp.shadow, 1);
    }
}

fn draw_cut(canvas: &mut Canvas, cut: &Cut, seed: u32) {
    let size = canvas.size;
    let values = field::sum(&cut.mask, size, hash(seed, 1, 1));
    for (i, v) in values.into_iter().enumerate() {
        let dark = match (cut.darker_than, canvas.texels[i]) {
            (None, _) => true,
            (Some(limit), Texel::Shade(_, shade)) => shade < limit,
            _ => false,
        };
        if dark && (cut.above.is_some_and(|a| v > a) || cut.below.is_some_and(|b| v < b)) {
            canvas.texels[i] = Texel::Clear;
        }
    }
}

fn draw_corners(canvas: &mut Canvas, corners: &[String; 4]) -> Result<(), Error> {
    let size = canvas.size;
    let colours: Vec<Rgba> = corners.iter().map(|c| colour::parse(c)).collect::<Result<_, _>>()?;
    let lab: Vec<[f32; 3]> = colours.iter().map(|&c| colour::to_oklab(c)).collect();
    for y in 0..size.h {
        for x in 0..size.w {
            let u = x as f32 / (size.w - 1).max(1) as f32;
            let v = y as f32 / (size.h - 1).max(1) as f32;
            let mix = |k: usize| {
                let top = lab[0][k] + (lab[1][k] - lab[0][k]) * u;
                let bottom = lab[2][k] + (lab[3][k] - lab[2][k]) * u;
                top + (bottom - top) * v
            };
            let rgb = colour::from_oklab([mix(0), mix(1), mix(2)]).unwrap_or([0, 0, 0]);
            let alpha = colours[0][3];
            canvas.texels[(y * size.w + x) as usize] = Texel::Colour([rgb[0], rgb[1], rgb[2], alpha]);
        }
    }
    Ok(())
}
