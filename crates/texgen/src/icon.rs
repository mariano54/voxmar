//! Inventory icons drawn from block faces.

use crate::Image;

/// Brightness of the top, left and right faces, lit from above and the left.
const LIGHT: [f32; 3] = [1.0, 0.8, 0.62];

/// A cube seen from above at the usual inventory angle, `size`×`size`: `top` on the
/// top face, `left` and `right` on the two side faces facing the viewer. Transparent
/// texels stay transparent.
pub fn isometric(top: &Image, left: &Image, right: &Image, size: u32) -> Image {
    let s = size as f32;
    // Corners: the top face is a rhombus half as tall as wide, the sides drop half the
    // size below it, leaving a one-pixel margin.
    let m = s / 32.0;
    let (w, half) = (s - 2.0 * m, (s - 2.0 * m) / 2.0);
    let back = (s / 2.0, m);
    let right_corner = (m + w, m + half / 2.0);
    let front = (s / 2.0, m + half);
    let left_corner = (m, m + half / 2.0);
    let drop = half;
    // Each face: origin, then where the texture's x and y axes go.
    let faces = [
        (top, back, sub(right_corner, back), sub(left_corner, back), LIGHT[0]),
        (left, left_corner, sub(front, left_corner), (0.0, drop), LIGHT[1]),
        (right, front, sub(right_corner, front), (0.0, drop), LIGHT[2]),
    ];
    let mut icon = Image::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            for &(texture, origin, ax, ay, light) in &faces {
                let Some((u, v)) = solve(sub(p, origin), ax, ay) else { continue };
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let texel = texture.get((u * texture.width as f32) as u32, (v * texture.height as f32) as u32);
                if texel[3] < 128 {
                    continue;
                }
                let shade = |c: u8| (c as f32 * light).round() as u8;
                icon.set(x, y, [shade(texel[0]), shade(texel[1]), shade(texel[2]), 255]);
                break;
            }
        }
    }
    icon
}

fn sub(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 - b.0, a.1 - b.1)
}

/// Solves `p = u * a + v * b`.
fn solve(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> Option<(f32, f32)> {
    let det = a.0 * b.1 - a.1 * b.0;
    if det.abs() < 1e-6 {
        return None;
    }
    Some(((p.0 * b.1 - p.1 * b.0) / det, (a.0 * p.1 - a.1 * p.0) / det))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_faces_are_shaded() {
        let face = Image::filled(16, 16, [200, 100, 50, 255]);
        let icon = isometric(&face, &face, &face, 32);
        assert_eq!(icon.get(16, 7), [200, 100, 50, 255]);
        assert_eq!(icon.get(8, 20), [160, 80, 40, 255]);
        assert_eq!(icon.get(24, 20)[0], 124);
        assert_eq!(icon.get(0, 0)[3], 0);
        assert_eq!(icon.get(31, 31)[3], 0);
    }
}
