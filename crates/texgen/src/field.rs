//! Scalar fields over a texture: the patterns [`Term`]s describe, evaluated per texel.
//!
//! Everything wraps around the texture's edges, so textures tile without seams, and is a
//! pure function of the seed, so a recipe always renders the same.

use crate::recipe::{
    Bevel, Boards, Border, Bricks, Cells, Circle, Fringe, Gradient, Noise, Rings, Ridge, Speckle, Stripes, Term,
};

/// Width and height of the texture being rendered.
#[derive(Clone, Copy)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub fn len(self) -> usize {
        (self.w * self.h) as usize
    }

    pub fn index(self, x: i32, y: i32) -> usize {
        (y.rem_euclid(self.h as i32) as u32 * self.w + x.rem_euclid(self.w as i32) as u32) as usize
    }
}

/// A well-mixed 32-bit hash of a seed and two coordinates.
pub fn hash(seed: u32, a: i32, b: i32) -> u32 {
    let mut h = seed ^ 0x9e37_79b9;
    for v in [a as u32, b as u32] {
        h ^= v.wrapping_mul(0x85eb_ca6b);
        h = h.rotate_left(13).wrapping_mul(0xc2b2_ae35);
        h ^= h >> 16;
    }
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

/// A hash in 0..1.
pub fn unit(seed: u32, a: i32, b: i32) -> f32 {
    (hash(seed, a, b) >> 8) as f32 / (1u32 << 24) as f32
}

/// Seed for a string, such as a texture name.
pub fn seed_of(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// Sum of `terms` per texel, each seeded from `seed` and its position in the list.
pub fn sum(terms: &[Term], size: Size, seed: u32) -> Vec<f32> {
    let mut field = vec![0.0; size.len()];
    for (i, term) in terms.iter().enumerate() {
        add(term, size, hash(seed, i as i32, 0x7e57), &mut field);
    }
    field
}

fn add(term: &Term, size: Size, seed: u32, field: &mut [f32]) {
    match term {
        Term::Flat(v) => field.iter_mut().for_each(|f| *f += v),
        Term::Noise(n) => noise(n, size, seed, field),
        Term::Ridge(r) => ridge(r, size, seed, field),
        Term::Speckle(s) => speckle(s, size, seed, field),
        Term::Cells(c) => cells(c, size, seed, field),
        Term::Bricks(b) => bricks(b, size, seed, field),
        Term::Boards(b) => boards(b, size, seed, field),
        Term::Rings(r) => rings(r, size, seed, field),
        Term::Stripes(s) => stripes(s, size, seed, field),
        Term::Border(b) => border(b, size, field),
        Term::Bevel(b) => bevel(b, size, field),
        Term::Fringe(f) => fringe(f, size, seed, field),
        Term::Gradient(g) => gradient(g, size, field),
        Term::Circle(c) => circle(c, size, seed, field),
        Term::Dither(amount) => {
            const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];
            for y in 0..size.h {
                for x in 0..size.w {
                    field[(y * size.w + x) as usize] += amount * ((BAYER[(y % 4) as usize][(x % 4) as usize] + 0.5) / 16.0 - 0.5);
                }
            }
        }
    }
}

/// Tileable value noise in 0..1 at texel centre `x`, `y`, on a lattice of `cells`.
fn value_noise(x: f32, y: f32, size: Size, cells: (u32, u32), seed: u32) -> f32 {
    let (cx, cy) = (cells.0.max(1) as i32, cells.1.max(1) as i32);
    let u = (x + 0.5) / size.w as f32 * cx as f32;
    let v = (y + 0.5) / size.h as f32 * cy as f32;
    let (i, j) = (u.floor() as i32, v.floor() as i32);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (fu, fv) = (smooth(u - i as f32), smooth(v - j as f32));
    let corner = |di: i32, dj: i32| unit(seed, (i + di).rem_euclid(cx), (j + dj).rem_euclid(cy));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * fu;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * fu;
    top + (bottom - top) * fv
}

/// Octaves of value noise, normalised to 0..1.
fn fractal(x: f32, y: f32, size: Size, cells: (u32, u32), octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut total, mut weight, mut amplitude) = (0.0, 0.0, 1.0);
    for octave in 0..octaves.max(1) {
        let scale = 1 << octave;
        total += amplitude * value_noise(x, y, size, (cells.0 * scale, cells.1 * scale), hash(seed, octave as i32, 1));
        weight += amplitude;
        amplitude *= gain;
    }
    total / weight
}

fn noise(n: &Noise, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, n.seed as i32, 2);
    for y in 0..size.h {
        for x in 0..size.w {
            let v = fractal(x as f32, y as f32, size, n.cells, n.octaves, n.gain, seed);
            field[(y * size.w + x) as usize] += (v - 0.5) * n.amount;
        }
    }
}

fn ridge(r: &Ridge, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, r.seed as i32, 3);
    for y in 0..size.h {
        for x in 0..size.w {
            let v = fractal(x as f32, y as f32, size, r.cells, r.octaves, 0.5, seed);
            let distance = (v - 0.5).abs() * 2.0;
            if distance < r.width {
                field[(y * size.w + x) as usize] += r.amount * (1.0 - distance / r.width.max(1e-6));
            }
        }
    }
}

fn speckle(s: &Speckle, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, s.seed as i32, 4);
    for y in 0..size.h {
        for x in 0..size.w {
            if unit(seed, x as i32, y as i32) < s.chance {
                field[(y * size.w + x) as usize] += s.amount;
            }
        }
    }
}

/// Shortest wrapped offset from `a` to `b` along an axis of length `len`.
fn wrapped(a: f32, b: f32, len: f32) -> f32 {
    let d = (b - a).rem_euclid(len);
    if d > len / 2.0 { d - len } else { d }
}

fn cells(c: &Cells, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, c.seed as i32, 5);
    let (gx, gy) = (c.count.0.max(1), c.count.1.max(1));
    let (sw, sh) = (size.w as f32 / gx as f32, size.h as f32 / gy as f32);
    let centres: Vec<(f32, f32)> = (0..gy)
        .flat_map(|j| (0..gx).map(move |i| (i, j)))
        .map(|(i, j)| {
            let jx = (unit(seed, i as i32, j as i32 * 2) - 0.5) * c.jitter;
            let jy = (unit(seed, i as i32, j as i32 * 2 + 1) - 0.5) * c.jitter;
            ((i as f32 + 0.5 + jx) * sw, (j as f32 + 0.5 + jy) * sh)
        })
        .collect();
    let (w, h) = (size.w as f32, size.h as f32);
    // The cell every texel belongs to, and its offset from that cell's centre.
    let mut owner = vec![0usize; size.len()];
    let mut offset = vec![(0.0f32, 0.0f32); size.len()];
    for y in 0..size.h {
        for x in 0..size.w {
            let (mut px, mut py) = (x as f32 + 0.5, y as f32 + 0.5);
            if c.warp != 0.0 {
                px += (fractal(x as f32, y as f32, size, (4, 4), 2, 0.5, hash(seed, 1, 7)) - 0.5) * 2.0 * c.warp;
                py += (fractal(x as f32, y as f32, size, (4, 4), 2, 0.5, hash(seed, 2, 7)) - 0.5) * 2.0 * c.warp;
            }
            let mut best = (f32::MAX, 0, (0.0, 0.0));
            for (k, &(cx, cy)) in centres.iter().enumerate() {
                let (dx, dy) = (wrapped(cx, px, w), wrapped(cy, py, h));
                let d = dx * dx + dy * dy;
                if d < best.0 {
                    best = (d, k, (dx, dy));
                }
            }
            let i = (y * size.w + x) as usize;
            owner[i] = best.1;
            offset[i] = best.2;
        }
    }
    let radius = (sw + sh) / 4.0;
    let width = c.width.round().max(1.0) as i32;
    for y in 0..size.h as i32 {
        for x in 0..size.w as i32 {
            let i = size.index(x, y);
            let own = owner[i];
            let differs = |dx: i32, dy: i32| owner[size.index(x + dx, y + dy)] != own;
            let mut value = (unit(seed, own as i32, 99) - 0.5) * c.value;
            // Boundaries run along the right and bottom of each cell, one crisp line.
            if (1..=width).any(|d| differs(d, 0) || differs(0, d)) {
                value += c.edge;
            } else {
                let (dx, dy) = offset[i];
                value += c.bevel * 0.5 * (-(dx + dy) / (2.0 * radius)).clamp(-1.0, 1.0);
                value -= c.round * (dx.hypot(dy) / radius).min(1.5);
                if differs(-1, 0) || differs(0, -1) {
                    value += c.bevel;
                }
            }
            field[i] += value;
        }
    }
}

fn bricks(b: &Bricks, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, b.seed as i32, 6);
    let (bw, bh) = (b.size.0.max(2), b.size.1.max(2));
    let offset = b.offset.unwrap_or(bw / 2);
    let columns = size.w.div_ceil(bw);
    for y in 0..size.h {
        let course = y / bh;
        let shift = if course % 2 == 1 { offset } else { 0 };
        for x in 0..size.w {
            let xs = (x + shift) % size.w;
            let (column, lx, ly) = (xs / bw, xs % bw, y % bh);
            let i = (y * size.w + x) as usize;
            if lx == bw - 1 || ly == bh - 1 {
                field[i] += b.mortar;
                continue;
            }
            let mut value = (unit(seed, (column % columns) as i32, course as i32) - 0.5) * b.value;
            if lx == 0 || ly == 0 {
                value += b.bevel;
            } else if lx == bw - 2 || ly == bh - 2 {
                value -= b.bevel;
            }
            field[i] += value;
        }
    }
}

fn boards(b: &Boards, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, b.seed as i32, 7);
    let height = b.height.max(2);
    for y in 0..size.h {
        let board = (y / height) as i32;
        let ly = y % height;
        // One or two end joints per board, never too close together.
        let first = (unit(seed, board, 0) * size.w as f32) as u32;
        let mut joints = vec![first];
        if unit(seed, board, 1) < 0.4 {
            joints.push((first + size.w / 2 + (unit(seed, board, 2) * 3.0) as u32) % size.w);
        }
        joints.sort_unstable();
        for x in 0..size.w {
            let i = (y * size.w + x) as usize;
            if ly == height - 1 {
                field[i] += b.seam;
                continue;
            }
            if joints.contains(&x) {
                field[i] += b.joint;
                continue;
            }
            let segment = joints.iter().filter(|&&j| j < x).count() % joints.len();
            let mut value = (unit(seed, board, 10 + segment as i32) - 0.5) * b.value;
            if ly == 0 {
                value += b.bevel;
            }
            let near_joint = joints.iter().any(|&j| (x + size.w - j) % size.w == 2 || (j + size.w - x) % size.w == 2);
            if b.nails != 0.0 && near_joint && ly == height / 2 - 1 {
                value += b.nails;
            }
            field[i] += value;
        }
    }
}

fn rings(r: &Rings, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, r.seed as i32, 8);
    let (cx, cy) = r.centre.unwrap_or((size.w as f32 / 2.0, size.h as f32 / 2.0));
    for y in 0..size.h {
        for x in 0..size.w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let round = dx.hypot(dy);
            let square = dx.abs().max(dy.abs());
            let wobble = (fractal(x as f32, y as f32, size, (4, 4), 2, 0.5, seed) - 0.5) * 2.0 * r.wobble;
            let d = round + (square - round) * r.square + wobble;
            if (d / r.spacing.max(0.5)).fract() < r.width {
                field[(y * size.w + x) as usize] += r.amount;
            }
        }
    }
}

fn stripes(s: &Stripes, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, s.seed as i32, 9);
    for y in 0..size.h {
        for x in 0..size.w {
            let (along, across) = if s.vertical { (x, y) } else { (y, x) };
            let wave = if s.warp != 0.0 {
                let n = if s.vertical {
                    value_noise(0.0, across as f32, size, (1, 4), seed)
                } else {
                    value_noise(across as f32, 0.0, size, (4, 1), seed)
                };
                (n - 0.5) * 2.0 * s.warp
            } else {
                0.0
            };
            let c = along as f32 + 0.5 + wave - s.phase + s.slope * across as f32;
            if (c / s.period.max(1.0)).rem_euclid(1.0) < s.width {
                field[(y * size.w + x) as usize] += s.amount;
            }
        }
    }
}

fn border(b: &Border, size: Size, field: &mut [f32]) {
    for y in 0..size.h {
        for x in 0..size.w {
            let near = |d: u32| d < b.width;
            let weights = [
                (near(y), b.sides.0),
                (near(size.w - 1 - x), b.sides.1),
                (near(size.h - 1 - y), b.sides.2),
                (near(x), b.sides.3),
            ];
            let weight = weights.iter().filter(|(n, _)| *n).map(|(_, w)| *w).fold(0.0f32, |a, w| if w.abs() > a.abs() { w } else { a });
            field[(y * size.w + x) as usize] += b.amount * weight;
        }
    }
}

fn bevel(b: &Bevel, size: Size, field: &mut [f32]) {
    for y in 0..size.h {
        for x in 0..size.w {
            let light = (y < b.width || x < b.width) as i32 as f32;
            let dark = (size.h - 1 - y < b.width || size.w - 1 - x < b.width) as i32 as f32;
            field[(y * size.w + x) as usize] += b.amount * (light - dark);
        }
    }
}

fn fringe(f: &Fringe, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, f.seed as i32, 10);
    let w = size.w as i32;
    let raw = |x: i32| unit(seed, x.rem_euclid(w), 0);
    for x in 0..w {
        let r = raw(x) * (1.0 - f.smooth) + (raw(x - 1) + raw(x + 1)) * 0.5 * f.smooth;
        let depth = f.depth.0 as f32 + r * (f.depth.1 as f32 - f.depth.0 as f32 + 1.0);
        let depth = (depth.floor() as u32).min(f.depth.1);
        for y in 0..size.h {
            let inside = if f.bottom { y >= size.h - depth } else { y < depth };
            if inside {
                field[(y * size.w + x as u32) as usize] += 1.0;
            }
        }
    }
}

fn gradient(g: &Gradient, size: Size, field: &mut [f32]) {
    for y in 0..size.h {
        for x in 0..size.w {
            let (p, len) = if g.across { (x, size.w) } else { (y, size.h) };
            let t = p as f32 / (len - 1).max(1) as f32;
            field[(y * size.w + x) as usize] += g.from + (g.to - g.from) * t;
        }
    }
}

fn circle(c: &Circle, size: Size, seed: u32, field: &mut [f32]) {
    let seed = hash(seed, c.seed as i32, 11);
    let (cx, cy) = c.centre.unwrap_or((size.w as f32 / 2.0, size.h as f32 / 2.0));
    for y in 0..size.h {
        for x in 0..size.w {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let rough = (fractal(x as f32, y as f32, size, (4, 4), 2, 0.5, seed) - 0.5) * 2.0 * c.rough;
            let r = c.radius + rough;
            let v = if c.soft > 0.0 { ((r - d) / c.soft + 0.5).clamp(0.0, 1.0) } else { (d < r) as i32 as f32 };
            field[(y * size.w + x) as usize] += v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_noise_tiles() {
        let size = Size { w: 16, h: 16 };
        for y in 0..16 {
            for x in 0..16 {
                let a = value_noise(x as f32, y as f32, size, (4, 2), 7);
                let b = value_noise(x as f32 + 16.0, y as f32 - 16.0, size, (4, 2), 7);
                assert!((a - b).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn hash_is_spread() {
        let mean = (0..1000).map(|i| unit(3, i, 0)).sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }
}
