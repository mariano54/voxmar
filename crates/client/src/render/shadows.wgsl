// Soft sun shadows from Bevy's cascaded shadow maps, sampled our own way.
//
// - Percentage-closer soft shadows: a blocker search finds how far the shadow's caster
//   is, and the filter widens with that distance, so contact shadows are crisp and a
//   canopy's shadow on the forest floor has a soft penumbra, as under a sun of real size.
// - Every filter tap is a hardware 2×2 comparison, and taps lie on a fixed golden-angle
//   spiral, a few texels wide at most, so the result depends only on where a point is: as
//   the camera moves, nothing about a shadow changes (no screen noise, which would swim
//   and, at the edge of a bright sunfleck, flicker; it only resolves under temporal
//   anti-aliasing, which we do not have).
// - Receiver plane depth bias: each tap compares against the receiving face's own depth
//   at that tap, so a wide filter over a face lit at a grazing angle never shades the face
//   itself (no acne, no flicker where it would flip). Voxel faces are flat and their
//   normals exact, which makes this exact.
// - Cascades blend over their overlap (a dither would hop between them as the camera
//   moves), and shadows fade out over the end of the last one instead of stopping at a
//   line.

#define_import_path voxmar::shadows

#import bevy_pbr::mesh_view_bindings::{view, lights, directional_shadow_textures, directional_shadow_textures_comparison_sampler}
#import voxmar::atmosphere::Atmosphere

// A turn by the golden angle (2.39996323 radians), as its cosine and sine.
const GOLDEN_TURN = vec2(-0.73736888, 0.67549029);
const BLOCKER_TAPS: u32 = 8u;
// The widest blocker search and penumbra, in shadow texels: wide enough for soft shade
// under a canopy, narrow enough that the fixed spiral's taps blur into each other.
const MAX_SEARCH: f32 = 16.0;
const MAX_RADIUS: f32 = 5.0;
// Shadows fade out over this last fraction of the shadow distance.
const FADE: f32 = 0.15;

// The points of a golden-angle spiral filling the unit disc: point `k` of `n` lies
// sqrt((k + 0.5) / n) out, `k` golden angles round. Its direction is turned from the last
// point's, which spares each tap a sine and a cosine.
struct Spiral {
    n: f32,
    k: f32,
    direction: vec2<f32>,
};

fn spiral(n: u32) -> Spiral {
    return Spiral(f32(n), 0.0, vec2(1.0, 0.0));
}

fn next_point(s: ptr<function, Spiral>) -> vec2<f32> {
    let point = sqrt(((*s).k + 0.5) / (*s).n) * (*s).direction;
    let d = (*s).direction;
    (*s).direction = vec2(d.x * GOLDEN_TURN.x - d.y * GOLDEN_TURN.y, d.x * GOLDEN_TURN.y + d.y * GOLDEN_TURN.x);
    (*s).k += 1.0;
    return point;
}

// Fraction of the sun reaching `world` from one cascade, or -1 outside it, soft-edged or
// not (one comparison of the four texels around).
fn cascade_shadow(a: Atmosphere, index: u32, world: vec3<f32>, normal: vec3<f32>, soft: bool) -> f32 {
    let light = &lights.directional_lights[0];
    let cascade = &(*light).cascades[index];
    let m = (*cascade).clip_from_world;
    let texel = (*cascade).texel_size;
    // The light's frame: right, up and towards the light, as rows of the projection.
    let right = normalize(vec3(m[0].x, m[1].x, m[2].x));
    let up = normalize(vec3(m[0].y, m[1].y, m[2].y));
    let toward = (*light).direction_to_light;
    let n = vec3(dot(normal, right), dot(normal, up), dot(normal, toward));
    let facing = max(abs(n.z), 0.2);
    let n_z = select(facing, -facing, n.z < 0.0);
    // Off the surface along its normal by about a texel, more where the light grazes it.
    let lifted = world + normal * texel * a.shadow.z * (1.0 + 1.5 * (1.0 - facing));
    let clip = m * vec4(lifted, 1.0);
    if any(abs(clip.xy) > vec2(1.0)) || clip.z < 0.0 || clip.z > 1.0 {
        return -1.0;
    }
    let size = vec2<f32>(textureDimensions(directional_shadow_textures).xy);
    let uv = clip.xy * vec2(0.5, -0.5) + 0.5;
    let layer = i32((*light).depth_texture_base_index + index);
    // Depth (reverse Z: nearer the light is greater) per block towards the light, and how
    // the receiving face's depth changes per texel across the map.
    let depth_per_block = length(vec3(m[0].z, m[1].z, m[2].z));
    let slope = -vec2(n.x, -n.y) / n_z * texel * depth_per_block;
    let taps = u32(a.shadow.x);
    if taps <= 1u || !soft {
        return textureSampleCompareLevel(directional_shadow_textures, directional_shadow_textures_comparison_sampler, uv, layer, clip.z);
    }

    // Blocker search over the widest penumbra.
    let search = clamp(a.shadow.w / texel, 1.5, MAX_SEARCH);
    var blockers = 0.0;
    var blocker_depth = 0.0;
    var around = spiral(BLOCKER_TAPS);
    for (var k = 0u; k < BLOCKER_TAPS; k++) {
        let offset = next_point(&around) * search;
        let at = clamp(vec2<i32>(floor(uv * size + offset)), vec2(0), vec2<i32>(size) - 1);
        let stored = textureLoad(directional_shadow_textures, at, layer, 0);
        // A blocker is nearer the light than the face itself is at that texel.
        let receiver = clip.z + dot(slope, offset) + depth_per_block * texel;
        if stored > receiver {
            blockers += 1.0;
            blocker_depth += stored - receiver;
        }
    }
    if blockers == 0.0 {
        // Nothing wide around; a single tap keeps thin shadows the search stepped over.
        return textureSampleCompareLevel(directional_shadow_textures, directional_shadow_textures_comparison_sampler, uv, layer, clip.z);
    }
    let distance = blocker_depth / blockers / depth_per_block;
    // Fewer taps blur less widely, so they still blur into each other.
    let widest = min(MAX_RADIUS * sqrt(f32(taps) / 16.0), search);
    let radius = clamp(distance * a.shadow.y / texel, 1.0, widest);

    // Taps as dense as the widest penumbra's: a narrower one (far cascades, contact
    // shadows, most of a sunny scene) needs fewer, down to 4 for one hardly wider than a
    // texel. The taps' spacing, so how they blur into each other, stays the same.
    let count = clamp(u32(ceil(f32(taps) * (radius / widest) * (radius / widest))), min(taps, 4u), taps);
    var lit = 0.0;
    var taps_around = spiral(count);
    for (var k = 0u; k < count; k++) {
        let offset = next_point(&taps_around) * radius;
        let receiver = clip.z + dot(slope, offset);
        lit += textureSampleCompareLevel(
            directional_shadow_textures,
            directional_shadow_textures_comparison_sampler,
            uv + offset / size,
            layer,
            receiver,
        );
    }
    return lit / f32(count);
}

// Fraction of direct sun reaching `world`, on a face with `normal` (exact for voxel faces),
// `soft`-edged or not. 1 beyond the shadow maps.
fn soft_shadow(a: Atmosphere, world: vec3<f32>, normal: vec3<f32>, soft: bool) -> f32 {
    let light = &lights.directional_lights[0];
    if a.shadow.x <= 0.0 || lights.n_directional_lights == 0u || ((*light).flags & 1u) == 0u {
        return 1.0;
    }
    let count = (*light).num_cascades;
    let depth = -(view.view_from_world * vec4(world, 1.0)).z;
    var index = 0u;
    while index < count && depth >= (*light).cascades[index].far_bound {
        index++;
    }
    if index >= count {
        return 1.0;
    }
    var shadow = cascade_shadow(a, index, world, normal, soft);
    // Blend into the next cascade over the overlap between them.
    let far = (*light).cascades[index].far_bound;
    let overlap = (1.0 - (*light).cascades_overlap_proportion) * far;
    if index + 1u < count && depth > overlap {
        let next = cascade_shadow(a, index + 1u, world, normal, soft);
        if shadow < 0.0 {
            shadow = next;
        } else if next >= 0.0 {
            shadow = mix(shadow, next, (depth - overlap) / (far - overlap));
        }
    }
    if shadow < 0.0 {
        return 1.0;
    }
    let end = (*light).cascades[count - 1u].far_bound;
    return mix(shadow, 1.0, smoothstep(end * (1.0 - FADE), end, depth));
}
