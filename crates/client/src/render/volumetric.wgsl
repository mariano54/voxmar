// The air in front of each pixel, drawn over the lit image before bloom: the low cloud, a
// layer in the world that passes over, around or below the eye; ground fog nearby, uneven
// and drifting, thinning upwards (beyond, atmosphere.wgsl's smooth fog takes over); and
// sunlight scattered by haze, fog and ground fog where the sun reaches them, marched through
// the cascaded shadow maps, so shafts fall through gaps in canopies and past ridges.
//
// Two passes: `march` at half resolution (a quarter of the pixels; all of this is soft)
// writes the light the air adds and how much of what lies behind shows through, and
// `composite` blends it over the image, from the nearby half-resolution rays that ended
// about as far away as the pixel's own surface, so nothing bleeds across the edge of a
// trunk or a ridge. Steps start at a fixed noise, so the march has fine grain rather than
// bands, which the composite's blend smooths away.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::mesh_view_bindings::{view, lights, directional_shadow_textures, directional_shadow_textures_comparison_sampler}
#import bevy_pbr::utils::interleaved_gradient_noise
#import voxmar::atmosphere::{Atmosphere, air_along, cloud_density, cloud_light, sun_glow, weather_fog_density}
#import voxmar::clouds::{pcg, unit, value_noise, value_noise3}

#ifdef MULTISAMPLED
@group(2) @binding(0) var depth_texture: texture_depth_multisampled_2d;
#else
@group(2) @binding(0) var depth_texture: texture_depth_2d;
#endif
// `march`: the atmosphere. `composite`: the half-resolution light and transmittance.
#ifdef COMPOSITE
@group(2) @binding(1) var march_texture: texture_2d<f32>;
#else
@group(2) @binding(1) var<uniform> atmosphere: Atmosphere;
#endif

// Rays to the sky stop here (far beyond anything drawn).
const SKY: f32 = 65504.0;
const LUMINANCE = vec3(0.2126, 0.7152, 0.0722);

#ifndef COMPOSITE
// The direction from the eye to what full-resolution `pixel` shows, and in w its distance.
fn surface(pixel: vec2<i32>) -> vec4<f32> {
    let depth = textureLoad(depth_texture, pixel, 0);
    let uv = (vec2<f32>(pixel) + 0.5) / view.viewport.zw;
    let ndc = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = view.world_from_clip * vec4(ndc, max(depth, 1e-7), 1.0);
    let offset = far.xyz / far.w - view.world_position;
    return vec4(normalize(offset), select(length(offset), SKY, depth <= 0.0));
}

// Light below which sunlight scattered by clear air is not worth marching: a faint veil.
const FAINT: f32 = 0.015;
// Ground fog: how far its top swells and sinks, over what distances, the size of its billows
// (two octaves), how much slower than the clouds it drifts, and steps through it.
const MIST_SWELL: f32 = 6.0;
const MIST_SWELL_SCALE: f32 = 70.0;
const MIST_BILLOWS = vec2(18.0, 6.0);
const MIST_DRIFT: f32 = 0.3;
const MIST_STEPS: u32 = 14u;
// Cloud: light taken out per block at full density (about 20 blocks of visibility inside
// it, so its edges are crisp), the longest stretch of it marched (seen edge-on near the horizon; beyond, it is
// solid or fades into haze anyway), and where its billows fade out with distance.
const CLOUD_EXTINCTION: f32 = 0.15;
const CLOUD_SPAN: f32 = 1500.0;
const CLOUD_BILLOWS = vec2(400.0, 800.0);
// The longest step through cloud worth taking, in blocks, until the steps run out.
const CLOUD_STEP: f32 = 12.0;
// Where cloud towards the sun is looked up (blocks along the way), and how much of the
// light it takes out relative to the view (light passing through cloud is mostly
// scattered onwards, not lost).
const SUN_LOOKS = vec2(12.0, 48.0);
const SUN_SHADE: f32 = 0.5;

// What a stretch of air adds to the view, and how much of what lies behind it shows.
struct Air {
    light: vec3<f32>,
    through: f32,
};

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

// How dense ground fog is at `p`, relative to `mist.x`: its top swells and sinks across the
// land and its body billows, all drifting slowly with the wind, so from inside it thickens
// and thins as it passes and from above it lies as an uneven sea.
fn mist_at(a: Atmosphere, p: vec3<f32>, drift: vec2<f32>) -> f32 {
    if p.y < a.fog_shape.z {
        return 0.0;
    }
    let q = p + vec3(drift.x, 0.0, drift.y);
    let top = a.mist.y + (value_noise(q.xz / MIST_SWELL_SCALE) - 0.5) * 2.0 * MIST_SWELL;
    let profile = exp(-max(p.y - top, 0.0) / a.mist.z);
    if profile < 0.01 {
        return 0.0;
    }
    let billows = value_noise3(q / MIST_BILLOWS.x) * 0.7 + value_noise3(q / MIST_BILLOWS.y + 17.0) * 0.3;
    return profile * smoothstep(0.2, 0.75, billows) * 1.8;
}

// The air between the eye and `distance` along `direction`, near enough to march: ground
// fog, which dims and lights the view, and sunlight scattered by haze, fog and ground fog
// where the shadow maps say the sun reaches.
fn near_air(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>, distance: f32, jitter: f32) -> Air {
    var out = Air(vec3(0.0), 1.0);
    let light = &lights.directional_lights[0];
    let shadows = lights.n_directional_lights > 0u && ((*light).flags & 1u) != 0u;
    let shafts = a.rays.x > 0.0 && shadows;
    let reach = min(distance, max(select(0.0, a.glow.w, shafts), a.mist.w));
    if reach <= 0.0 {
        return out;
    }
    let to_sun = (*light).direction_to_light;
    let sun = (*light).color.rgb;
    let phase = sun_glow(dot(direction, to_sun)) + a.rays.z;
    var strength = 0.0;
    if shafts {
        // Skip sunlight that would add only a faint veil even if all of the air were lit at
        // its densest (in clear air by day, the view away from the sun); it fades in above
        // that, so no edge shows where it starts being marched.
        let densest = a.haze.w * a.rays.w + a.mist.x + a.fog.w;
        let most = phase * densest * min(reach, a.glow.w) * a.rays.x * max(sun.r, max(sun.g, sun.b));
        strength = a.rays.x * smoothstep(FAINT, 2.0 * FAINT, most);
    }
    if strength <= 0.0 && a.mist.w <= 0.0 {
        return out;
    }
    let steps = select(u32(max(a.rays.y, 1.0)), max(u32(a.rays.y), MIST_STEPS), a.mist.w > 0.0);
    let forward = (view.view_from_world * vec4(direction, 0.0)).xyz;
    // Cloud shade changes slowly along the ray: take it at both ends.
    let cloud_near = cloud_light(a, eye, eye, to_sun);
    let cloud_far = cloud_light(a, eye, eye + direction * reach, to_sun);
    // How far the wind has carried the clouds (their noise coordinates are over the eye).
    let drift = (a.clouds.xy * 256.0 - eye.xz) * MIST_DRIFT;
    var sun_scattered = 0.0;
    var sky_scattered = 0.0;
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
        var mist = 0.0;
        if a.mist.w > 0.0 && at < a.mist.w {
            mist = a.mist.x * mist_at(a, p, drift);
        }
        var lit = 1.0;
        var scatter = mist;
        if strength > 0.0 && at < a.glow.w {
            let haze = a.haze.w * exp(-max(p.y - a.haze_shape.x, 0.0) / a.haze_shape.y);
            scatter += (haze * a.rays.w + weather_fog_density(a, p.y)) * strength;
            lit = sunlit(p, -forward.z * at) * mix(cloud_near, cloud_far, at / max(reach, 1e-3));
        } else if mist > 0.0 && shadows {
            // Ground fog glows towards the sun only where the sun reaches it, with or
            // without light shafts.
            lit = sunlit(p, -forward.z * at);
        }
        sun_scattered += scatter * through * lit * span;
        sky_scattered += mist * through * span;
        through *= exp(-mist * span);
    }
    out.light = sun * phase * sun_scattered + a.fog.rgb * sky_scattered;
    out.through = through;
    return out;
}

// The low cloud along `direction` from `eye`, up to `distance`: lit by the sun from above
// (bright tops, grey bases, silver edges towards the sun) and by the sky, darker in storms,
// and seen through the haze between it and the eye.
fn cloud_layer(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>, distance: f32, jitter: f32) -> Air {
    var out = Air(vec3(0.0), 1.0);
    let least = u32(a.cloud_look.z);
    if least == 0u {
        return out;
    }
    let base = a.cloud_shape.x;
    let top = a.cloud_shape.y;
    // The stretch of the ray inside the layer.
    var start = 0.0;
    var end = min(distance, a.cloud_look.w);
    if abs(direction.y) > 1e-4 {
        let to_base = (base - eye.y) / direction.y;
        let to_top = (top - eye.y) / direction.y;
        start = max(start, min(to_base, to_top));
        end = min(end, max(to_base, to_top));
    } else if eye.y < base || eye.y > top {
        return out;
    }
    if end <= start {
        return out;
    }
    let light = &lights.directional_lights[0];
    let to_sun = (*light).direction_to_light;
    let sun = (*light).color.rgb;
    // Clouds take sunlight whiter than the land does.
    let sunlight = mix(sun, vec3(dot(sun, LUMINANCE)), 0.5) * 1.2;
    let sky = lights.ambient_color.rgb;
    // Cloud scatters light forwards: edges towards the sun glow silver.
    let phase = 0.9 + 1.6 * pow(max(dot(direction, to_sun), 0.0), 6.0);
    let storm = a.cloud_shape.w;
    let flash = vec3(0.75, 0.8, 1.0) * a.cloud_look.y;
    let span = min(end - start, CLOUD_SPAN);
    // Long crossings (the layer seen at a slant) take more steps, so no step is much longer
    // than the billows.
    let steps = clamp(u32(span / CLOUD_STEP), least, 2u * least);
    // From inside the layer, steps grow with the square of their index, fine near the eye
    // where the cloud passes by; from outside, they are even.
    let power = select(1.0, 2.0, start <= 0.0);
    var color = vec3(0.0);
    var through = 1.0;
    var weighted = 0.0;
    var previous = start;
    for (var i = 1u; i <= steps; i++) {
        let next = start + span * pow(f32(i) / f32(steps), power);
        let at = mix(previous, next, jitter);
        let step = next - previous;
        previous = next;
        let p = eye + direction * at;
        // Far off it thins into the sky rather than ending at a line.
        let far = 1.0 - smoothstep(0.5 * a.cloud_look.w, a.cloud_look.w, at);
        let billows = 1.0 - smoothstep(CLOUD_BILLOWS.x, CLOUD_BILLOWS.y, at);
        let density = cloud_density(a, p, eye, billows) * far;
        if density > 0.0 {
            let alpha = 1.0 - exp(-density * CLOUD_EXTINCTION * step);
            let h = saturate((p.y - base) / (top - base));
            // How much cloud lies towards the sun, from two looks along the way: sunlit
            // faces and tops are bright, the far side and the cores shaded, and near the eye
            // each billow shades the next.
            let shade = cloud_density(a, p + to_sun * SUN_LOOKS.x, eye, billows) * SUN_LOOKS.x
                + cloud_density(a, p + to_sun * SUN_LOOKS.y, eye, 0.0) * (SUN_LOOKS.y - SUN_LOOKS.x);
            // Light scattered many times inside reaches deep into it, more softly.
            let depth = shade * CLOUD_EXTINCTION * SUN_SHADE;
            let sunlit = (0.7 * exp(-depth) + 0.3 * exp(-0.25 * depth)) * (1.0 - 0.6 * storm * (1.0 - h));
            let lit = sunlight * sunlit * phase;
            // Sky light, less of it low down, where the cloud above hides the sky.
            let ambient = sky * mix(0.65, 1.0, h) * (1.0 - 0.5 * storm);
            color += through * alpha * (lit + ambient + flash);
            weighted += through * alpha * at;
            through *= 1.0 - alpha;
            if through < 0.02 {
                break;
            }
        }
    }
    if through > 0.999 {
        return out;
    }
    // The haze between the eye and the cloud, where most of it lies.
    let air = air_along(a, eye, direction * (weighted / (1.0 - through)), to_sun);
    out.light = color * air.transmittance + air.light * (1.0 - through);
    out.through = through;
    return out;
}

@fragment
fn march(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let a = atmosphere;
    // The full-resolution pixel this half-resolution one stands for.
    let ray = surface(vec2<i32>(in.position.xy) * 2);
    let eye = view.world_position;
    // Where each ray's steps start. Near the eye, interleaved gradient noise, which the
    // composite's blend smooths best; through the cloud, whose steps are long and whose
    // layers line up, white noise, as the gradient's diagonal pattern shows there as streaks.
    let pixel = vec2<u32>(in.position.xy);
    let near = near_air(a, eye, ray.xyz, ray.w, interleaved_gradient_noise(in.position.xy, 0u));
    // The cloud lies behind the air near the eye (or, around the eye inside it, mingles with
    // it, which this treats alike).
    let cloud = cloud_layer(a, eye, ray.xyz, ray.w, unit(pcg(pixel.x + pcg(pixel.y ^ 0x5bd1e995u))));
    return vec4(near.light + near.through * cloud.light, near.through * cloud.through);
}

#else

// How far in front of the eye what full-resolution `pixel` shows lies (the view depth of
// Bevy's infinite reverse-z projection), as cheap as a load and a divide.
fn depth_at(pixel: vec2<i32>) -> f32 {
    let depth = textureLoad(depth_texture, pixel, 0);
    return select(view.clip_from_view[3][2] / depth, SKY, depth <= 0.0);
}

@fragment
fn composite(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let distance = depth_at(pixel);
    // Where this pixel's centre falls among the half-resolution rays' centres.
    let at = in.position.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(at));
    let f = fract(at);
    let last = vec2<i32>(textureDimensions(march_texture)) - 1;
    let full = vec2<i32>(view.viewport.zw) - 1;
    var air = vec4(0.0);
    var total = 0.0;
    // A tent two rays wide over the 4×4 nearest rays smooths the march's grain away.
    for (var k = 0; k < 16; k++) {
        let o = vec2(k & 3, k >> 2u) - 1;
        let ray = clamp(base + o, vec2(0), last);
        let d = vec2<f32>(o) - f;
        let tent = max(2.0 - abs(d.x), 0.0) * max(2.0 - abs(d.y), 0.0);
        // Rays that ended much nearer or farther than this pixel's surface saw other things.
        let ended = depth_at(min(ray * 2, full));
        let alike = exp(-abs(ended - distance) / (0.05 * min(distance, ended) + 0.25));
        let weight = tent * (alike + 1e-4);
        air += textureLoad(march_texture, ray, 0) * weight;
        total += weight;
    }
    // Blended as light + image × transmittance.
    return air / max(total, 1e-6);
}

#endif
