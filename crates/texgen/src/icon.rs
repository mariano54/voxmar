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

/// One textured quad of a block's shape, for [`isometric_quads`]: its corners in blocks
/// from the cell's lower corner (counter-clockwise seen from outside), their texture
/// coordinates (0..1 across `texture`), and which cube face it is lit as (0 top, 1 left or
/// +z, 2 right or +x; any other faces away from the viewer).
pub struct IconQuad<'a> {
    pub corners: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    pub texture: &'a Image,
    pub light: f32,
}

/// The brightness of a quad facing `normal` in an icon: as [`isometric`] lights a cube's
/// top, left (+z) and right (+x) faces, by whichever its normal is nearest.
pub fn icon_light(normal: [f32; 3]) -> f32 {
    let [x, y, z] = normal;
    if y >= x.abs().max(z.abs()) {
        LIGHT[0]
    } else if z >= x.abs() {
        LIGHT[1]
    } else {
        LIGHT[2]
    }
}

/// A block's shape seen as [`isometric`] sees a cube, `size`×`size`: its quads drawn
/// nearest last, so a barrel or a stair shows its own outline instead of a cube's.
pub fn isometric_quads(quads: &[IconQuad], size: u32) -> Image {
    let s = size as f32;
    let m = s / 32.0;
    let (w, half) = (s - 2.0 * m, (s - 2.0 * m) / 2.0);
    // The cube's top back corner, and where a step along x, z and down y goes on screen:
    // the projection `isometric` draws, which looks along (-1, -1, -1).
    let back = (s / 2.0, m);
    let (ax, az, down) = ((w / 2.0, half / 2.0), (-w / 2.0, half / 2.0), half);
    let project = |p: [f32; 3]| (back.0 + p[0] * ax.0 + p[2] * az.0, back.1 + p[0] * ax.1 + p[2] * az.1 + (1.0 - p[1]) * down);
    let mut icon = Image::new(size, size);
    let mut depth = vec![f32::NEG_INFINITY; (size * size) as usize];
    for quad in quads {
        let screen = quad.corners.map(project);
        // Seen from behind (turned away from the viewer): wound clockwise on screen.
        let area = (screen[1].0 - screen[0].0) * (screen[2].1 - screen[0].1) - (screen[2].0 - screen[0].0) * (screen[1].1 - screen[0].1);
        if area >= 0.0 {
            continue;
        }
        for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
            let (pa, pb, pc) = (screen[a], screen[b], screen[c]);
            let x0 = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as u32;
            let x1 = (pa.0.max(pb.0).max(pc.0).ceil() as u32).min(size);
            let y0 = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as u32;
            let y1 = (pa.1.max(pb.1).max(pc.1).ceil() as u32).min(size);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (x as f32 + 0.5, y as f32 + 0.5);
                    let Some((u, v)) = solve(sub(p, pa), sub(pb, pa), sub(pc, pa)) else { continue };
                    let t = 1.0 - u - v;
                    if u < 0.0 || v < 0.0 || t < 0.0 {
                        continue;
                    }
                    let blend = |i: usize| quad.uvs[a][i] * t + quad.uvs[b][i] * u + quad.uvs[c][i] * v;
                    let corner = |i: usize| quad.corners[a][i] * t + quad.corners[b][i] * u + quad.corners[c][i] * v;
                    // Nearer the viewer the larger x + y + z.
                    let d = corner(0) + corner(1) + corner(2);
                    let slot = (y * size + x) as usize;
                    if d <= depth[slot] {
                        continue;
                    }
                    let texture = quad.texture;
                    let tx = ((blend(0) * texture.width as f32) as u32).min(texture.width - 1);
                    let ty = ((blend(1) * texture.height as f32) as u32).min(texture.height - 1);
                    let texel = texture.get(tx, ty);
                    if texel[3] < 128 {
                        continue;
                    }
                    depth[slot] = d;
                    let shade = |c: u8| (c as f32 * quad.light).round() as u8;
                    icon.set(x, y, [shade(texel[0]), shade(texel[1]), shade(texel[2]), 255]);
                }
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

    /// A whole cube drawn from its six faces as quads looks as `isometric` draws it, and a
    /// slab, half as tall, leaves the icon's top empty.
    #[test]
    fn shapes_are_drawn_as_cubes_are() {
        let face = Image::filled(16, 16, [200, 100, 50, 255]);
        let quad = |corners: [[f32; 3]; 4], normal: [f32; 3]| IconQuad { corners, uvs: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]], texture: &face, light: icon_light(normal) };
        let cube = |top: f32| {
            vec![
                quad([[0.0, top, 1.0], [1.0, top, 1.0], [1.0, top, 0.0], [0.0, top, 0.0]], [0.0, 1.0, 0.0]),
                quad([[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, top, 1.0], [0.0, top, 1.0]], [0.0, 0.0, 1.0]),
                quad([[1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, top, 0.0], [1.0, top, 1.0]], [1.0, 0.0, 0.0]),
                // Turned away from the viewer: never drawn.
                quad([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]], [0.0, -1.0, 0.0]),
            ]
        };
        let (whole, drawn) = (isometric(&face, &face, &face, 32), isometric_quads(&cube(1.0), 32));
        let differ = (0..32).flat_map(|y| (0..32).map(move |x| (x, y))).filter(|&(x, y)| whole.get(x, y) != drawn.get(x, y)).count();
        assert!(differ <= 8, "{differ} pixels differ from the cube icon");
        let slab = isometric_quads(&cube(0.5), 32);
        assert_eq!(slab.get(16, 7)[3], 0);
        assert_eq!(slab.get(16, 20), [200, 100, 50, 255]);
        assert_eq!(slab.get(8, 24), [160, 80, 40, 255]);
    }
}
