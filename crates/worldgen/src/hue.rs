//! Colour harmony: how well the colours of trees, plants and ground blocks sit together.
//!
//! Every candidate's colour is worked out once at load: its block's average colour, times
//! the grass or foliage colour where the game tints it. A biome has a palette, an anchor
//! colour for its canopy (its foliage colour, from its climate or set in data) and for its
//! flowers where data sets one; candidates are weighed by how near their colour lies to the
//! anchor, in Oklab, where distance follows what the eye sees. Hue and saturation count
//! fully and lightness little. Tinted leaves take the place's foliage colour, so any number
//! of green trees grow together; a crown of its own colour (a pink cherry, a blue
//! silverwood, a red maple) grows only where the biome's palette is built around a colour
//! like it, and never as a speck among other colours. Flowers and lone plants much brighter
//! than the grass are rarer accents. A mod's new tree joins any biome its colour and climate
//! suit. The ground keeps to its colours the same way: the blocks of a mixed material are
//! weighed against its main block, and patches unlike the ground's colour grow fewer and
//! smaller (`crate::surface`).

use voxmar_engine::block::{BlockDef, Tint};

use crate::content::{Axis, BiomeDef, WorldContent};

/// How far from its palette's anchor a canopy's colour may lie and still grow freely; at
/// three times this distance a tree is a rare accent.
pub(crate) const CANOPY_SPREAD: f32 = 0.07;
/// Likewise for a crown of its own colour that a biome without a canopy colour names,
/// against the biome's foliage: looser, as the foliage is no colour the biome chose.
pub(crate) const FOLIAGE_SPREAD: f32 = 0.1;
/// Likewise for flowers against a palette set in data.
pub(crate) const FLOWER_SPREAD: f32 = 0.08;
/// Likewise for the blocks mixed in a ground material, against its main block.
pub(crate) const GROUND_SPREAD: f32 = 0.09;
/// Likewise for carpeting plants against the grass: tight, so the carpet reads as one
/// surface and straw-coloured tufts keep to straw-coloured grass.
pub(crate) const CARPET_SPREAD: f32 = 0.05;
/// Below this harmony with its palette, a crown of its own colour does not grow at all.
pub(crate) const CLASH: f32 = 0.2;
/// Flowers and lone plants unlike the grass's colour grow this much rarer at the most,
/// judged over a wide spread so only the brightest contrasts count fully.
pub(crate) const ACCENT_RARITY: f32 = 0.75;
pub(crate) const CONTRAST_SPREAD: f32 = 0.2;

/// sRGB (0 ..= 255) to Oklab (Björn Ottosson's colour space, as in `voxmar_texgen`).
pub(crate) fn oklab(rgb: [f32; 3]) -> [f32; 3] {
    let linear = rgb.map(|c| {
        let c = (c / 255.0).clamp(0.0, 1.0);
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    let [r, g, b] = linear;
    let l = (0.412_221_5 * r + 0.536_332_5 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab to sRGB (0 ..= 255), clamped into gamut.
pub(crate) fn from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
    .map(|c| {
        let c = c.clamp(0.0, 1.0);
        255.0 * if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    })
}

/// The grass as the eye should meet it: every grass colour, the climate's and a biome's
/// own, a little less saturated, turned a few degrees towards yellow and a touch lighter,
/// so a meadow reads soft and warm rather than a saturated green. Being the same change
/// everywhere, it keeps every difference between places: forests stay deeper, dry land
/// straw, jungles lusher.
pub(crate) fn soften_grass(rgb: [f32; 3]) -> [f32; 3] {
    const CHROMA: f32 = 0.72;
    const WARM: f32 = 0.16; // radians towards yellow (Oklab hue about 1.9 rad)
    const LIGHT: f32 = 0.025;
    let [l, a, b] = oklab(rgb);
    let chroma = a.hypot(b) * CHROMA;
    let hue = b.atan2(a);
    // Turn towards yellow, but never past it: straw stays straw.
    let target = 1.75_f32;
    let hue = hue + (target - hue).clamp(-WARM, WARM);
    from_oklab([l + LIGHT, chroma * hue.cos(), chroma * hue.sin()])
}

/// How well colour `a` sits with `b`, 1 for the same hue and saturation, falling off as a
/// bell over `spread` (Oklab distance, lightness counted at a third).
pub(crate) fn harmony(a: [f32; 3], b: [f32; 3], spread: f32) -> f32 {
    let (a, b) = (oklab(a), oklab(b));
    let d2 = (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) + (0.33 * (a[0] - b[0])).powi(2);
    (-d2 / (spread * spread)).exp()
}

/// The hue angle of a colour, radians, for ordering by colour.
pub(crate) fn hue(rgb: [f32; 3]) -> f32 {
    let [_, a, b] = oklab(rgb);
    b.atan2(a)
}

/// The colour a block shows: its average colour, or the grass or foliage colour where the
/// game tints it.
pub(crate) fn shown(def: &BlockDef, grass: [f32; 3], foliage: [f32; 3]) -> [f32; 3] {
    match def.tint.drawn() {
        // A tinted texture is grey: the tint is its colour.
        Tint::Grass => grass,
        Tint::Foliage => foliage,
        Tint::None | Tint::Water => def.color.map(f32::from),
    }
}

/// Grass and foliage colours of a climate, from temperature and humidity as `Site::tint`
/// gives them (0 ..= 1): blue-green when cold, olive when hot and dry, deep green when hot
/// and wet.
pub(crate) fn climate_tints(t: f32, h: f32) -> [[f32; 3]; 2] {
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
    let grass = mix([128.0, 167.0, 140.0], mix([191.0, 183.0, 85.0], [71.0, 176.0, 26.0], h), t);
    let foliage = mix([97.0, 140.0, 110.0], mix([174.0, 164.0, 42.0], [48.0, 150.0, 18.0], h), t);
    [grass, foliage]
}

/// The middle of a biome's climate, °C and moisture, from the bands of its first climate
/// box (or of the biome it is a variant or ecotone of); `None` along an axis it leaves open.
pub(crate) fn centre(content: &WorldContent, biome: &BiomeDef) -> (Option<f32>, Option<f32>) {
    let parent = || {
        let ecotone = biome.ecotone.as_ref().map(|e| e.between.0.as_str());
        content.biomes.iter().find(|b| b.variants.iter().any(|(v, _)| *v == biome.name) || Some(b.name.as_str()) == ecotone)
    };
    let Some(climate) = biome.climate.first().or_else(|| parent()?.climate.first()) else { return (None, None) };
    let mid = |axis: Axis| {
        let (from, to) = climate.bands(axis)?;
        let bands = content.bands(axis);
        let index = |name: &str| bands.names.iter().position(|n| n == name);
        let (lo, hi) = (index(from)? as isize, index(to)? as isize);
        Some((crate::biome::band_edge(&bands.edges, lo - 1) + crate::biome::band_edge(&bands.edges, hi)) / 2.0)
    };
    (mid(Axis::Temperature), mid(Axis::Moisture))
}

/// A biome's grass and foliage colours: its own where it sets them, else its climate's at
/// the middle of its climate.
pub(crate) fn biome_tints(content: &WorldContent, biome: &BiomeDef) -> [[f32; 3]; 2] {
    let (t, m) = centre(content, biome);
    let climate = climate_tints(((t.unwrap_or(12.0) + 5.0) / 25.0).clamp(0.0, 1.0), ((m.unwrap_or(1.0) - 0.1) / 1.1).clamp(0.0, 1.0));
    let own = |set: Option<(u8, u8, u8)>, climate: [f32; 3]| set.map_or(climate, |(r, g, b)| [r as f32, g as f32, b as f32]);
    [soften_grass(own(biome.colors.grass, climate[0])), own(biome.colors.foliage, climate[1])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_softens_without_losing_its_differences() {
        let back = from_oklab(oklab([120.0, 160.0, 70.0]));
        assert!(back.iter().zip([120.0, 160.0, 70.0]).all(|(a, b)| (a - b).abs() < 1.0), "{back:?}");
        let saturation = |c: [f32; 3]| { let [_, a, b] = oklab(c); a.hypot(b) };
        let meadow = [96.0, 150.0, 64.0];
        let soft = soften_grass(meadow);
        assert!(saturation(soft) < saturation(meadow) * 0.8);
        // Warmer: more red against green than before.
        assert!(soft[0] / soft[1] > meadow[0] / meadow[1]);
        // Forest grass stays darker than meadow grass, and straw stays yellower than green.
        let forest = soften_grass([70.0, 120.0, 50.0]);
        assert!(forest[1] < soft[1]);
        let straw = soften_grass([190.0, 172.0, 90.0]);
        assert!(straw[0] / straw[1] > soft[0] / soft[1]);
    }

    #[test]
    fn greens_sit_together_and_strong_hues_stand_out() {
        let green = [70.0, 140.0, 40.0];
        assert!(harmony([60.0, 120.0, 45.0], green, CANOPY_SPREAD) > 0.6, "a darker green");
        for (name, other) in [("red maple", [190.0, 80.0, 40.0]), ("pink cherry", [235.0, 165.0, 195.0]), ("blue silverwood", [130.0, 175.0, 195.0])] {
            assert!(harmony(other, green, CANOPY_SPREAD) < 0.05, "{name}: {}", harmony(other, green, CANOPY_SPREAD));
        }
    }
}
