//! Colours: parsing `#rrggbb` strings and building hue-shifted shade ramps.
//!
//! Ramps are built in OKLCh, a perceptual space, so equal lightness steps look equal and
//! hue shifts do not swing saturation around. Following pixel-art practice, shadows turn
//! towards blue and lose a little lightness faster than saturation, and highlights turn
//! towards yellow and pale out.

use crate::Error;
use crate::recipe::Ramp;

pub type Rgba = [u8; 4];

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn parse(text: &str) -> Result<Rgba, Error> {
    let bad = || Error(format!("`{text}` is not a colour (#rrggbb or #rrggbbaa)"));
    let hex = text.strip_prefix('#').ok_or_else(bad)?;
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1).ok_or_else(bad)?, 16).map_err(|_| bad());
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).ok_or_else(bad)?, 16).map_err(|_| bad());
    match hex.len() {
        3 => Ok([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
        6 => Ok([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Ok([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => Err(bad()),
    }
}

/// The shades of a ramp, darkest first.
pub fn ramp(ramp: &Ramp) -> Result<Vec<Rgba>, Error> {
    let key = parse(&ramp.ramp)?;
    let shades = ramp.shades.clamp(2, 16);
    let [l, a, b] = to_oklab(key);
    let chroma = a.hypot(b);
    let hue = b.atan2(a).to_degrees();
    Ok((0..shades)
        .map(|i| {
            // -0.5 for the darkest shade, 0.5 for the lightest; the key colour sits at the
            // middle (between the two middle shades when the count is even).
            let t = i as f32 / (shades - 1) as f32 - 0.5;
            let l = (l + t * ramp.light).clamp(0.0, 1.0);
            let c = (chroma * (1.0 + t * ramp.chroma)).max(0.0);
            let target = if t < 0.0 { COOL } else { WARM };
            let h = turn_towards(hue, target, t.abs() * ramp.hue).to_radians();
            let mut rgb = from_oklab([l, c * h.cos(), c * h.sin()]);
            // Out of gamut: halve the chroma until it fits.
            let mut c = c;
            while rgb.is_none() && c > 0.001 {
                c *= 0.8;
                rgb = from_oklab([l, c * h.cos(), c * h.sin()]);
            }
            let [r, g, b] = rgb.unwrap_or_else(|| from_oklab([l, 0.0, 0.0]).unwrap_or([0, 0, 0]));
            [r, g, b, key[3]]
        })
        .collect())
}

/// OKLCh hues (degrees) that shadows and highlights turn towards.
const COOL: f32 = 264.0;
const WARM: f32 = 95.0;

/// Moves `hue` towards `target` by at most `degrees`, the short way round.
fn turn_towards(hue: f32, target: f32, degrees: f32) -> f32 {
    let delta = (target - hue + 540.0).rem_euclid(360.0) - 180.0;
    hue + delta.clamp(-degrees, degrees)
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// `None` when the colour is outside sRGB.
fn linear_to_srgb(c: f32) -> Option<u8> {
    if !(-0.001..=1.001).contains(&c) {
        return None;
    }
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    Some((s * 255.0).round() as u8)
}

pub fn to_oklab([r, g, b, _]: Rgba) -> [f32; 3] {
    let (r, g, b) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn from_oklab([l, a, b]: [f32; 3]) -> Option<[u8; 3]> {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    Some([
        linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s)?,
        linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s)?,
        linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colours() {
        assert_eq!(parse("#ff8000").unwrap(), [255, 128, 0, 255]);
        assert_eq!(parse("#f80").unwrap(), [255, 136, 0, 255]);
        assert_eq!(parse("#10203040").unwrap(), [16, 32, 48, 64]);
        assert!(parse("ff8000").is_err());
        assert!(parse("#ff80").is_err());
    }

    #[test]
    fn oklab_round_trips() {
        for colour in [[0, 0, 0, 255], [255, 255, 255, 255], [200, 30, 90, 255], [12, 140, 220, 255]] {
            let [r, g, b] = from_oklab(to_oklab(colour)).unwrap();
            assert!((r as i32 - colour[0] as i32).abs() <= 1, "{colour:?}");
            assert!((g as i32 - colour[1] as i32).abs() <= 1, "{colour:?}");
            assert!((b as i32 - colour[2] as i32).abs() <= 1, "{colour:?}");
        }
    }

    #[test]
    fn ramps_run_dark_to_light_around_the_key() {
        let shades = ramp(&Ramp { ramp: "#808080".into(), ..Ramp::default() }).unwrap();
        assert_eq!(shades.len(), 5);
        let lightness: Vec<f32> = shades.iter().map(|&s| to_oklab(s)[0]).collect();
        assert!(lightness.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(shades[2], [128, 128, 128, 255]);
    }
}
