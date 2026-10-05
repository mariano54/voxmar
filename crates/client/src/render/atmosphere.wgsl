// The air between the eye and what it sees: clear-air haze that thins with height, ground
// fog (valley fog, dawn mist, sea fog) filling the land below a height and thinning above
// it, the weather's fog, and the low cloud, a layer at a height in the world. Haze, fog and
// ground fog beyond the volumetric pass's reach are integrated analytically along the view
// ray here, so they cost a few exponentials per pixel and never band or shimmer; nearer,
// ground fog takes its uneven, drifting shape in the volumetric pass (volumetric.wgsl),
// which also draws the clouds. The terrain, the sky and that pass share these functions and
// one uniform (render/atmosphere.rs).

#define_import_path voxmar::atmosphere

#import voxmar::clouds::{cloud_body, cloud_detail, cloud_threshold, value_noise3}

struct Atmosphere {
    // Shadows. x: filter taps (0: no shadows, 1: one bilinear tap), y: penumbra width per
    // block between caster and receiver, z: normal offset in shadow texels, w: widest
    // penumbra, in blocks.
    shadow: vec4<f32>,
    // Surfaces. x: how steeply sky light falls off into shade (an exponent), y: ambient
    // occlusion strength, z: how brightly leaves and plants glow when the sun is behind
    // them, w: how brightly glowing blocks glow by themselves.
    surface: vec4<f32>,
    // Clear-air haze. rgb: its colour, lit by the sky (the horizon's colour), w: how much
    // light it takes out per block at its base.
    haze: vec4<f32>,
    // x: the height haze is densest at, y: the height over which it thins by e, z and w:
    // where the distant terrain's edge starts and finishes fading into the sky.
    haze_shape: vec4<f32>,
    // Sunlight scattered by the air. rgb: its colour around the sun (as the sky's glow), w:
    // how far the volumetric light pass reaches, in blocks (0 when it is off, and the haze
    // then scatters all of it).
    glow: vec4<f32>,
    // Fog. rgb: its colour, lit by the sky and the sun, w: how much light the weather's fog
    // (mist, whiteout, rain, dust) takes out per block at its base.
    fog: vec4<f32>,
    // x: the height the weather's fog is thick below (around the eye, whose weather it
    // is), y: the height over which it thins by e above that, z: sea level, below which
    // the volumetric pass draws no ground fog (water and caves lie there).
    fog_shape: vec4<f32>,
    // Ground fog. x: light taken out per block inside it, y: the height of its top, z: the
    // height over which it thins by e above that, w: how far the volumetric pass draws it
    // (it is analytic beyond), 0 for nowhere.
    mist: vec4<f32>,
    // Volumetric light. x: strength, y: ray steps, z: sunlight scattered evenly in every
    // direction, besides the glow around the sun, w: how much more of the clear air
    // scatters it than dims (shafts show in clear weather too).
    rays: vec4<f32>,
    // The low cloud (clouds.wgsl). xy: the noise coordinates over the eye, z: how much of
    // the sky it covers, w: how heaped it is.
    clouds: vec4<f32>,
    // x, y: the heights of its base and top, z: seconds it has been forming for, w: how
    // stormy it is (dark bases).
    cloud_shape: vec4<f32>,
    // x: how dark its shadows on the land are, y: a lightning flash in it, z: steps through
    // it in the volumetric pass (0: no clouds drawn), w: how far it is drawn, in blocks.
    cloud_look: vec4<f32>,
    // The cloud map (render/cloud_map.rs). xy: its corner from the eye (x, z), z: the
    // blocks it spans, w: 1 while it is drawn.
    cloud_map: vec4<f32>,
    // The cloud shade map (render/cloud_map.rs), likewise.
    cloud_shade: vec4<f32>,
    // The ground. x: how wet rain has left it, y: frost on it, z: snow lying around, w:
    // how much sunlight it bounces onto walls and undersides.
    ground: vec4<f32>,
};

// Mean density between heights `y0` and `y0 + dy` of air that is densest below `base`
// and thins by e every `height` above it.
fn layer_mean(y0: f32, dy: f32, base: f32, height: f32) -> f32 {
    let start = max(y0 - base, 0.0) / height;
    let end = max(y0 + dy - base, 0.0) / height;
    let rise = end - start;
    return select((exp(-start) - exp(-end)) / rise, exp(-0.5 * (start + end)), abs(rise) < 1e-3);
}

// Light taken out by clear-air haze between heights `y0` and `y0 + dy` over `distance`
// blocks. Density falls off exponentially above the haze base, so valleys and lowlands
// fade sooner than peaks.
fn haze_depth(a: Atmosphere, y0: f32, dy: f32, distance: f32) -> f32 {
    return a.haze.w * distance * layer_mean(y0, dy, a.haze_shape.x, a.haze_shape.y);
}

// Light taken out by the weather's fog likewise: it fills the air around and below the
// eye and thins above, so looking up through mist shows a pale sky, not a white one.
fn weather_fog_depth(a: Atmosphere, y0: f32, dy: f32, distance: f32) -> f32 {
    if a.fog.w <= 0.0 {
        return 0.0;
    }
    return a.fog.w * distance * layer_mean(y0, dy, a.fog_shape.x, a.fog_shape.y);
}

fn weather_fog_density(a: Atmosphere, y: f32) -> f32 {
    return a.fog.w * exp(-max(y - a.fog_shape.x, 0.0) / a.fog_shape.y);
}

// Light taken out by ground fog between heights `y0` and `y0 + dy` over `distance` blocks:
// it is densest below its top and thins by e every `mist.z` blocks above, so it lies in
// valleys and hollows but has no hard top and reaches thinly up the slopes.
fn mist_depth(a: Atmosphere, y0: f32, dy: f32, distance: f32) -> f32 {
    if a.mist.x <= 0.0 {
        return 0.0;
    }
    return a.mist.x * distance * layer_mean(y0, dy, a.mist.y, a.mist.z);
}

// Ground fog's light taken out along `direction` from `eye` between `near` and `far`.
fn mist_between(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>, near: f32, far: f32) -> f32 {
    let length = max(far - near, 0.0);
    return mist_depth(a, eye.y + direction.y * near, direction.y * length, length);
}

// All the light taken out along `offset` from `eye`: haze, ground fog (beyond the
// volumetric pass's reach) and the weather's fog.
fn optical_depth(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>) -> vec3<f32> {
    let distance = length(offset);
    let direction = offset / max(distance, 1e-4);
    let mist = mist_between(a, eye, direction, min(a.mist.w, distance), distance);
    return vec3(haze_depth(a, eye.y, offset.y, distance), mist, weather_fog_depth(a, eye.y, offset.y, distance));
}

// How strongly the air glows looking `cos_sun` away from the sun: the sky's halo, which
// distant haze shares so terrain fades into the sky around the sun exactly.
fn sun_glow(cos_sun: f32) -> f32 {
    let c = max(cos_sun, 0.0);
    return pow(c, 48.0) * 0.35 + pow(c, 6.0) * 0.1;
}

// The colour of distant haze looking along `direction`: the horizon, glowing around the
// sun as the sky does.
fn haze_color(a: Atmosphere, direction: vec3<f32>, to_sun: vec3<f32>) -> vec3<f32> {
    return a.haze.rgb + a.glow.rgb * sun_glow(dot(direction, to_sun));
}

struct Aerial {
    // How much of what lies behind shows through.
    transmittance: f32,
    // Light the air adds on the way to the eye.
    light: vec3<f32>,
};

// How much fog hides the view looking along `direction` from `eye` to the far edge of the
// distant terrain: the weather's fog and ground fog, but not the clear-air haze, which the
// horizon's colour already is.
fn edge_fog(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>) -> f32 {
    let reach = a.haze_shape.w;
    let mist = mist_between(a, eye, direction, min(a.mist.w, reach), reach);
    return 1.0 - exp(-(mist + weather_fog_depth(a, eye.y, direction.y * reach, reach)));
}

// What lies beyond everything looking along `direction`: the horizon's haze glowing
// around the sun, behind whatever fog fills the view. Distant terrain fades into it and
// the sky shows it, so they meet seamlessly.
fn far_color(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>, to_sun: vec3<f32>) -> vec3<f32> {
    return mix(haze_color(a, direction, to_sun), a.fog.rgb, edge_fog(a, eye, direction));
}

// The air between `eye` and what lies `offset` from it, for what the volumetric pass
// draws over: sky light scattered by each medium takes its colour; ground fog within the
// pass's reach, and sunlight scattered within its reach, are left to that pass, which
// shapes the one and knows where shadows fall for the other. The far edge of the distant
// terrain fades completely into what lies beyond.
fn aerial(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>, to_sun: vec3<f32>) -> Aerial {
    let distance = max(length(offset), 1e-4);
    let direction = offset / distance;
    var out = air_along(a, eye, offset, to_sun);
    let edge = smoothstep(a.haze_shape.z, a.haze_shape.w, distance);
    if edge > 0.0 {
        out.transmittance *= 1.0 - edge;
        out.light = mix(out.light, far_color(a, eye, direction, to_sun), edge);
    }
    return out;
}

// `aerial` without the far edge: the air in front of what lies beyond the terrain (the
// clouds).
fn air_along(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>, to_sun: vec3<f32>) -> Aerial {
    let distance = max(length(offset), 1e-4);
    let direction = offset / distance;
    let depth = optical_depth(a, eye, offset);
    let total = depth.x + depth.y + depth.z;
    let through = exp(-total);
    let sky_lit = (a.haze.rgb * depth.x + a.fog.rgb * (depth.y + depth.z)) / max(total, 1e-6);
    // Scattered sunlight from beyond the volumetric pass's reach.
    var sunlit = 1.0 - through;
    if a.glow.w > 0.0 {
        // Within the pass's reach it lights all of the air itself; beyond, only the air past it.
        sunlit = 0.0;
        if distance > a.glow.w {
            let near = optical_depth(a, eye, direction * a.glow.w);
            sunlit = exp(-(near.x + near.y + near.z)) - through;
        }
    }
    let glow = a.glow.rgb * sun_glow(dot(direction, to_sun));
    var out: Aerial;
    out.transmittance = through;
    out.light = sky_lit * (1.0 - through) + glow * sunlit;
    return out;
}

// Blocks per unit of the low cloud's noise coordinates (render::CLOUD_UNIT), and the
// scale of its body and of the billows at its edges in those units.
const CLOUD_UNIT: f32 = 256.0;
const LAYER_SCALE: f32 = 1.1;
const CLOUD_DETAIL: f32 = 7.0;
// Size of the 3D puffs of the low cloud near the eye, in blocks.
const BILLOW_SIZE: f32 = 22.0;

// The low cloud's 2D body (clouds.wgsl) at `offset` (x, z) from the eye, before the
// layer's profile shapes it.
fn cloud_body_at(a: Atmosphere, offset: vec2<f32>) -> f32 {
    return cloud_body((a.clouds.xy + offset / CLOUD_UNIT) * LAYER_SCALE, a.cloud_shape.z);
}

#ifdef CLOUD_MAP
@group(2) @binding(8) var cloud_map: texture_2d<f32>;
@group(2) @binding(9) var cloud_map_sampler: sampler;
@group(2) @binding(10) var cloud_shade_map: texture_2d<f32>;
#endif

// `cloud_body_at`, from the cloud map where it reaches and the pipeline binds it.
fn cloud_field(a: Atmosphere, offset: vec2<f32>) -> f32 {
#ifdef CLOUD_MAP
    let uv = (offset - a.cloud_map.xy) / a.cloud_map.z;
    if a.cloud_map.w > 0.0 && all(uv > vec2(0.0)) && all(uv < vec2(1.0)) {
        return textureSampleLevel(cloud_map, cloud_map_sampler, uv, 0.0).r;
    }
#endif
    return cloud_body_at(a, offset);
}

// The low cloud's 2D shape at `offset` (x, z) from the eye and height `h` through its
// layer (0 at the base, 1 at the top), before billows: above 0 there is cloud. Its 2D
// noise sets where cloud is and how thick; within the layer flat cloud fills it from a
// little above the base nearly to the top, while heaped cloud rises in domes over a flat
// base, its thickest cores tallest.
fn cloud_core(a: Atmosphere, offset: vec2<f32>, h: f32) -> f32 {
    return cloud_core_of(a, cloud_field(a, offset), offset, h);
}

// `cloud_core`, for the cloud's 2D body there (`cloud_field`) already looked up.
fn cloud_core_of(a: Atmosphere, body: f32, offset: vec2<f32>, h: f32) -> f32 {
    let cover = a.clouds.z;
    let threshold = cloud_threshold(cover);
    var core = (body - threshold) / (1.0 - threshold);
    // Nothing the detail or billows add could reach cloud here.
    if core < -0.3 {
        return core;
    }
    let heaped = a.clouds.w;
    // 0 a little above the base, 1 at the top and the very bottom.
    let rise = select((h - 0.12) / 0.88, (0.12 - h) / 0.12, h < 0.12);
    core -= mix(0.8 * pow(rise, 6.0), rise * rise, heaped) * smoothstep(1.0, 0.85, cover);
    let at = a.clouds.xy + offset / CLOUD_UNIT;
    let detail = cloud_detail(at * CLOUD_DETAIL, a.cloud_shape.z) - 0.5;
    return core + (detail - 0.1) * mix(0.3, 0.4, heaped) * (1.0 - saturate(core * 3.0));
}

// How sharply the low cloud's edge goes from clear to dense: `cloud_core` times this,
// clamped to 0..1, is its density.
fn cloud_edge(a: Atmosphere) -> f32 {
    return mix(5.0, 9.0, a.clouds.w);
}

// How dense the low cloud is at `p`, 0..1, for the eye at `eye` (see `cloud_core`).
// `billows` (0..1, for samples near enough to show them) lets 3D puffs eat into its edges
// and tops, so heaped cloud is lumpy rather than smooth.
fn cloud_density(a: Atmosphere, p: vec3<f32>, eye: vec3<f32>, billows: f32) -> f32 {
    let h = (p.y - a.cloud_shape.x) / (a.cloud_shape.y - a.cloud_shape.x);
    if a.clouds.z < 0.005 || h <= 0.0 || h >= 1.0 {
        return 0.0;
    }
    var core = cloud_core(a, p.xz - eye.xz, h);
    if billows > 0.0 && core > -0.2 {
        // Puffs a few dozen blocks across, carried with the cloud and slowly turning over.
        let at = a.clouds.xy + (p.xz - eye.xz) / CLOUD_UNIT;
        let q = vec3(at.x * CLOUD_UNIT, p.y + a.cloud_shape.z * 0.05, at.y * CLOUD_UNIT) / BILLOW_SIZE;
        let puff = value_noise3(q) * 0.65 + value_noise3(q * 2.3 + 11.0) * 0.35;
        // They eat into edges more than cores, and into tops more than bases.
        let bite = mix(0.15, 0.35, a.clouds.w) * mix(0.6, 1.2, h) * (1.0 - 0.6 * saturate(core * 2.0));
        core -= (1.0 - puff) * bite * billows;
    }
    return saturate(core * cloud_edge(a));
}

// The low cloud through the middle of its layer at `offset` (x, z) from the eye, where
// shadows and reflections look it up: its density unclamped (0 and below is clear, 1 and
// above dense), for the shade map to keep its edges sharp between texels.
fn cloud_middle_at(a: Atmosphere, offset: vec2<f32>) -> f32 {
    return cloud_middle_of(a, cloud_field(a, offset), offset);
}

// `cloud_middle_at`, for the cloud's 2D body there already looked up.
fn cloud_middle_of(a: Atmosphere, body: f32, offset: vec2<f32>) -> f32 {
    if a.clouds.z < 0.005 {
        return 0.0;
    }
    return cloud_core_of(a, body, offset, 0.5) * cloud_edge(a);
}

// `cloud_middle_at`, from the shade map where it reaches and the pipeline binds it.
fn cloud_middle(a: Atmosphere, offset: vec2<f32>) -> f32 {
#ifdef CLOUD_MAP
    let uv = (offset - a.cloud_shade.xy) / a.cloud_shade.z;
    if a.cloud_shade.w > 0.0 && all(uv > vec2(0.0)) && all(uv < vec2(1.0)) {
        return textureSampleLevel(cloud_shade_map, cloud_map_sampler, uv, 0.0).r;
    }
#endif
    return cloud_middle_at(a, offset);
}

// Fraction of sunlight the low cloud lets through to `world` (with the eye at `eye`), from
// the cloud where the ray towards the sun `to_sun` crosses the middle of the layer.
fn cloud_light(a: Atmosphere, eye: vec3<f32>, world: vec3<f32>, to_sun: vec3<f32>) -> f32 {
    let middle = 0.5 * (a.cloud_shape.x + a.cloud_shape.y);
    if a.cloud_look.x <= 0.0 || a.clouds.z < 0.005 || to_sun.y < 0.02 || world.y >= middle {
        return 1.0;
    }
    let crossing = world.xz + to_sun.xz * ((middle - world.y) / to_sun.y);
    return 1.0 - a.cloud_look.x * smoothstep(0.0, 0.5, cloud_middle(a, crossing - eye.xz));
}

// How opaque the low cloud looks along `direction` (upwards) from `origin`, from one look
// where the ray crosses the middle of the layer (x), and how far away that is (y).
fn cloud_across(a: Atmosphere, eye: vec3<f32>, origin: vec3<f32>, direction: vec3<f32>) -> vec2<f32> {
    let middle = 0.5 * (a.cloud_shape.x + a.cloud_shape.y);
    if a.cloud_look.z <= 0.0 || direction.y <= 0.01 || origin.y >= middle {
        return vec2(0.0);
    }
    let along = (middle - origin.y) / direction.y;
    if along > a.cloud_look.w {
        return vec2(0.0);
    }
    let density = cloud_middle(a, origin.xz + direction.xz * along - eye.xz);
    return vec2(smoothstep(0.0, 0.35, density) * smoothstep(0.01, 0.08, direction.y), along);
}

// The low cloud along `direction` (upwards) from `origin` (see `cloud_across`), for the sky
// in reflections: its base, lit by `sun` from above and by `sky`, seen through the air on
// the way, in premultiplied colour and opacity.
fn cloud_seen(a: Atmosphere, eye: vec3<f32>, origin: vec3<f32>, direction: vec3<f32>, to_sun: vec3<f32>, sun: vec3<f32>, sky: vec3<f32>) -> vec4<f32> {
    let across = cloud_across(a, eye, origin, direction);
    let alpha = across.x;
    if alpha <= 0.0 {
        return vec4(0.0);
    }
    let white = vec3(dot(sun, vec3(0.2126, 0.7152, 0.0722)));
    let lit = (mix(sun, white, 0.5) * 0.4 + sky * 0.7) * (1.0 - 0.5 * a.cloud_shape.w);
    let air = air_along(a, origin, direction * across.y, to_sun);
    return vec4((lit * air.transmittance + air.light) * alpha, alpha);
}
