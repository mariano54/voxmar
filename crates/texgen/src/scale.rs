//! Recipes redrawn at a finer resolution: the same texture with `k` times the texels
//! across, for packs drawn at 32×32 (or more) over books written at 16×16.
//!
//! A recipe is a description, not a grid of texels, so most of it is already independent
//! of resolution: noise, cells and ridges are laid out in lattice cells across the
//! texture, and per-texel terms (grain, speckle, dither) simply get finer, which is what a
//! higher-resolution texture is for. Everything measured in texels (brick and board sizes,
//! borders, circles, fringes, blob sizes, grid sprites and stamp offsets) is multiplied by
//! `k` here so shapes keep their proportions. One-texel touches a pixel artist adds (the
//! shadow and lit rim of a painted area or a blob) stay one texel wide: at the finer
//! resolution they read as the crisper edges hand-drawn high-resolution packs have.

use crate::recipe::{Clusters, Grid, Layer, Paint, Shift, Stamp, Term, Cut};

/// `layer` at `k` times the resolution.
pub fn layer(layer: &Layer, k: u32) -> Layer {
    let f = k as f32;
    match layer {
        Layer::Paint(p) => Layer::Paint(Paint { shade: terms(&p.shade, k), mask: terms(&p.mask, k), ..p.clone() }),
        Layer::Grid(g) => Layer::Grid(grid(g, k)),
        Layer::Clusters(c) => Layer::Clusters(Clusters {
            size: (c.size.0 * k * k, c.size.1 * k * k),
            spacing: c.spacing * f,
            margin: c.margin * k,
            ..c.clone()
        }),
        Layer::Shift(s) => Layer::Shift(Shift { by: terms(&s.by, k), mask: terms(&s.mask, k), ..s.clone() }),
        Layer::Cut(c) => Layer::Cut(Cut { mask: terms(&c.mask, k), ..c.clone() }),
        // The stamped texture is itself drawn `k` times finer (see `Library`), so only
        // offsets in texels grow; relative offsets are in sixteenths of it already.
        Layer::Stamp(s) if s.relative => layer.clone(),
        Layer::Stamp(s) => Layer::Stamp(Stamp { at: s.at.iter().map(|&(x, y)| (x * k as i32, y * k as i32)).collect(), ..s.clone() }),
        Layer::Corners(_) => layer.clone(),
    }
}

fn terms(terms: &[Term], k: u32) -> Vec<Term> {
    terms.iter().map(|t| term(t, k)).collect()
}

fn term(term: &Term, k: u32) -> Term {
    let f = k as f32;
    let mut t = term.clone();
    match &mut t {
        // Laid out across the texture, or one value per texel: unchanged.
        Term::Flat(_) | Term::Noise(_) | Term::Ridge(_) | Term::Speckle(_) | Term::Grain(..) | Term::Gradient(_) | Term::Dither(_) => {}
        Term::Cells(c) => {
            c.width *= f;
            c.warp *= f;
        }
        Term::Bricks(b) => {
            b.size = (b.size.0 * k, b.size.1 * k);
            b.offset = b.offset.map(|o| o * k);
        }
        Term::Boards(b) => b.height *= k,
        Term::Rings(r) => {
            r.centre = r.centre.map(|(x, y)| (x * f, y * f));
            r.spacing *= f;
            r.wobble *= f;
        }
        Term::Stripes(s) => {
            s.period *= f;
            s.warp *= f;
            s.phase *= f;
        }
        Term::Border(b) => b.width *= k,
        Term::Bevel(b) => b.width *= k,
        Term::Fringe(fr) => fr.depth = (fr.depth.0 * k, fr.depth.1 * k),
        Term::Circle(c) => {
            c.centre = c.centre.map(|(x, y)| (x * f, y * f));
            c.radius *= f;
            c.soft *= f;
            c.rough *= f;
        }
    }
    t
}

/// A text sprite with every character a `k`×`k` block.
fn grid(g: &Grid, k: u32) -> Grid {
    let rows = g
        .rows
        .iter()
        .flat_map(|row| {
            let wide: String = row.chars().flat_map(|c| std::iter::repeat_n(c, k as usize)).collect();
            std::iter::repeat_n(wide, k as usize)
        })
        .collect();
    Grid { rows, at: g.at.iter().map(|&(x, y)| (x * k as i32, y * k as i32)).collect(), ..g.clone() }
}
