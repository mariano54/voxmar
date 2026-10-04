// Volumetric light: sunlight scattered towards the eye by the air in front of each pixel,
// where the sun reaches it. Marches the view ray through the cascaded shadow maps out to
// the atmosphere's reach, so shafts fall through gaps in canopies and past ridges, mist and
// fog glow where the sun lights them, and the haze around the sun brightens. The forward
// fog leaves this stretch's sunlight to it (see atmosphere.wgsl) and scatters the rest.
//
// Two passes: `march` at half resolution (a quarter of the pixels; the light is soft),
// keeping each ray's length beside its light, and `composite`, which adds it to the HDR
// image before bloom, blending the nearest half-resolution rays that ended about as far
// away as the pixel's own surface, so shafts never bleed across the edge of a trunk or a
// ridge. Steps are spaced more closely near the eye and start at a fixed noise, so the
// march has fine grain rather than bands, which the composite's blend smooths away.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::mesh_view_bindings::{view, lights, directional_shadow_textures, directional_shadow_textures_comparison_sampler}
#import bevy_pbr::utils::interleaved_gradient_noise
#import voxmar::atmosphere::{Atmosphere, cloud_light, mist_density, sun_glow, weather_fog_density}

#ifdef MULTISAMPLED
@group(2) @binding(0) var depth_texture: texture_depth_multisampled_2d;
#else
@group(2) @binding(0) var depth_texture: texture_depth_2d;
#endif
// `march`: the atmosphere. `composite`: the half-resolution light and ray lengths.
#ifdef COMPOSITE
@group(2) @binding(1) var march_texture: texture_2d<f32>;
#else
@group(2) @binding(1) var<uniform> atmosphere: Atmosphere;
#endif

// Rays to the sky stop here (far beyond the reach).
const SKY: f32 = 65504.0;
// Light (linear) below which a ray is not worth marching: a faint veil at most.
const FAINT: f32 = 0.015;

// The direction from the eye to what full-resolution `pixel` shows, and in w its distance.
fn surface(pixel: vec2<i32>) -> vec4<f32> {
    let depth = textureLoad(depth_texture, pixel, 0);
    let uv = (vec2<f32>(pixel) + 0.5) / view.viewport.zw;
    let ndc = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = view.world_from_clip * vec4(ndc, max(depth, 1e-7), 1.0);
    let offset = far.xyz / far.w - view.world_position;
    return vec4(normalize(offset), select(length(offset), SKY, depth <= 0.0));
}

#ifndef COMPOSITE

// Whether the sun reaches `world`, from the cascade covering `depth` (distance along the
// view): one bilinear comparison, which the many steps blur together.
fn sunlit(world: vec3<f32>, depth: f32) -> f32 {
    let light = &lights.directional_lights[0];
    let count = (*light).num_cascades;
    var index = 0u;
    while index < count && depth >= (*light).cascades[index].far_bound {
        index++;
    }
    if index >= count {
        return 1.0;
    }
    let clip = (*light).cascades[index].clip_from_world * vec4(world, 1.0);
    if any(abs(clip.xy) > vec2(1.0)) || clip.z < 0.0 || clip.z > 1.0 {
        return 1.0;
    }
    let uv = clip.xy * vec2(0.5, -0.5) + 0.5;
    let layer = i32((*light).depth_texture_base_index + index);
    return textureSampleCompareLevel(directional_shadow_textures, directional_shadow_textures_comparison_sampler, uv, layer, clip.z);
}

@fragment
fn march(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let a = atmosphere;
    let light = &lights.directional_lights[0];
    // The full-resolution pixel this half-resolution one stands for.
    let ray = surface(vec2<i32>(in.position.xy) * 2);
    if a.rays.x <= 0.0 || lights.n_directional_lights == 0u || ((*light).flags & 1u) == 0u {
        return vec4(0.0, 0.0, 0.0, ray.w);
    }
    let direction = ray.xyz;
    let reach = min(ray.w, a.glow.w);
    let to_sun = (*light).direction_to_light;
    let sun = (*light).color.rgb;
    let phase = sun_glow(dot(direction, to_sun)) + a.rays.z;
    // Skip rays that would add only a faint veil even if all of their air were lit at its
    // densest: in clear air by day, the view away from the sun.
    // It fades in above that, so no edge shows where rays start being marched.
    let densest = a.haze.w * a.rays.w + a.mist.x + a.fog.w;
    let most = phase * densest * reach * a.rays.x * max(sun.r, max(sun.g, sun.b));
    if most < FAINT {
        return vec4(0.0, 0.0, 0.0, ray.w);
    }
    let steps = max(u32(a.rays.y), 1u);
    let jitter = interleaved_gradient_noise(in.position.xy, 0u);
    let forward = (view.view_from_world * vec4(direction, 0.0)).xyz;
    // Cloud shade changes slowly along the ray: take it at both ends.
    let eye = view.world_position;
    let cloud_near = cloud_light(a, eye, eye, to_sun);
    let cloud_far = cloud_light(a, eye, eye + direction * reach, to_sun);
    var scattered = 0.0;
    var through = 1.0;
    var previous = 0.0;
    for (var i = 1u; i <= steps; i++) {
        // Steps grow with the square of their index: fine near the eye, coarse far off.
        let t = f32(i) / f32(steps);
        let end = reach * t * t;
        let at = mix(previous, end, jitter);
        let span = end - previous;
        previous = end;
        let p = eye + direction * at;
        let haze = a.haze.w * exp(-max(p.y - a.haze_shape.x, 0.0) / a.haze_shape.y);
        let mist = a.mist.x * mist_density(a, p.y);
        let fog = weather_fog_density(a, p.y);
        let scatter = haze * a.rays.w + mist + fog;
        let extinction = haze + mist + fog;
        let lit = sunlit(p, -forward.z * at) * mix(cloud_near, cloud_far, at / max(reach, 1e-3));
        scattered += scatter * through * lit * span;
        through *= exp(-extinction * span);
    }
    return vec4(sun * phase * scattered * a.rays.x * smoothstep(FAINT, 2.0 * FAINT, most), ray.w);
}

#else

@fragment
fn composite(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let distance = surface(pixel).w;
    // Where this pixel's centre falls among the half-resolution rays' centres.
    let at = in.position.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(at));
    let f = fract(at);
    let last = vec2<i32>(textureDimensions(march_texture)) - 1;
    var light = vec3(0.0);
    var total = 0.0;
    // A tent two rays wide over the 4×4 nearest rays smooths the march's grain away.
    for (var k = 0; k < 16; k++) {
        let o = vec2(k & 3, k >> 2u) - 1;
        let ray = textureLoad(march_texture, clamp(base + o, vec2(0), last), 0);
        let d = vec2<f32>(o) - f;
        let tent = max(2.0 - abs(d.x), 0.0) * max(2.0 - abs(d.y), 0.0);
        // Rays that ended much nearer or farther than this pixel's surface saw other things.
        let alike = exp(-abs(ray.a - distance) / (0.05 * min(distance, ray.a) + 0.25));
        let weight = tent * (alike + 1e-4);
        light += ray.rgb * weight;
        total += weight;
    }
    return vec4(light / max(total, 1e-6), 0.0);
}

#endif
