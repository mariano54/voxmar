// The air between the eye and what it sees: clear-air haze that thins with height, ground
// fog (valley fog, dawn mist, sea fog) filling the land below a height and thinning above
// it, the weather's fog, and the low cloud, a layer at a height in the world. Haze, fog and
// ground fog beyond the volumetric pass's reach are integrated analytically along the view
// ray here, so they cost a few exponentials per pixel and never band or shimmer; nearer,
// ground fog takes its uneven, drifting shape in the volumetric pass (volumetric.wgsl),
// which also draws the clouds. The terrain, the sky and that pass share these functions and
// one uniform (render/atmosphere.rs).

#define_import_path voxmar::atmosphere

#import voxmar::clouds::{cloud_body, cloud_detail, cloud_threshold, value_noise, value_noise3}
#import voxmar::water::{absorption, scattering}

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
    // Under water (see `under_water`). rgb: the colour of the water the eye is in, w: the
    // height of its surface.
    water: vec4<f32>,
    // rgb: sun and sky light falling on that surface, w: 1 while the eye is under water.
    water_light: vec4<f32>,
    // x, y: how strongly water absorbs and scatters light (the pack's).
    water_scales: vec4<f32>,
    // Dusk and dawn (see `dusk_tint`). xy: the way to the sun across the land, z: how
    // strongly its side of the sky is warmer than the other.
    dusk: vec4<f32>,
    // Lightning inside the low cloud: xyz where (world space), w how bright, 0..1.
    cloud_flash: vec4<f32>,
    // A rainbow (see `rainbow`): xyz the direction to the sun, w how strong (above 0.75 a
    // double one), 0 for none.
    bow: vec4<f32>,
    // A light held in the player's hand (a firebrand): xyz where it is, w its light level
    // over 15 (0: none held). It lights what is near as a block giving that light would,
    // one level less a block, without waiting for the light to spread (`held_light`).
    held: vec4<f32>,
};

// At full dusk or dawn, how much more red, green and blue the side of the sky towards the
// sun has, and the side away from it less: a warm, bright glow under the low sun, the
// cool, dimmer sky opposite under the earth's shadow.
const DUSK_CONTRAST = vec3(0.45, 0.12, -0.25);

// The tint the sky's light takes looking along `direction` at dusk and dawn, as a factor
// (1 at other times): its horizontal part says how far it looks towards the sun, so the
// zenith, and a top face, take neither side. A shorter `direction` takes less of it.
fn dusk_tint(a: Atmosphere, direction: vec3<f32>) -> vec3<f32> {
    return max(vec3(1.0) + DUSK_CONTRAST * (dot(direction.xz, a.dusk.xy) * a.dusk.z), vec3(0.0));
}

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
    return a.haze.rgb * dusk_tint(a, direction) + a.glow.rgb * sun_glow(dot(direction, to_sun));
}

struct Aerial {
    // How much of what lies behind shows through, per channel.
    transmittance: vec3<f32>,
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
// `color` at `world` seen from `eye` through the air, `to_sun` the direction towards the sun
// (or moon): the terrain's `apply_fog` for a material, which reads the atmosphere from the
// shared buffer (atmosphere.rs, `SharedAtmosphere`) rather than the terrain's bind group.
fn fogged(a: Atmosphere, eye: vec3<f32>, world: vec3<f32>, to_sun: vec3<f32>, color: vec3<f32>) -> vec3<f32> {
    let air = aerial(a, eye, world - eye, to_sun);
    return color * air.transmittance + air.light;
}

fn aerial(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>, to_sun: vec3<f32>) -> Aerial {
    if a.water_light.w > 0.0 {
        return under_water(a, eye, offset);
    }
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
    let sky_lit = (a.haze.rgb * dusk_tint(a, direction) * depth.x + a.fog.rgb * (depth.y + depth.z)) / max(total, 1e-6);
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
    out.transmittance = vec3(through);
    out.light = sky_lit * (1.0 - through) + glow * sunlit;
    return out;
}

// How much brighter water seen from inside it glows than light scattered once in it would
// make it: light scattered again and again, which a single scattering leaves out.
const UNDER_WATER_SCATTERING: f32 = 3.0;

// Light the water the eye is in scatters towards the eye along `distance` blocks of a ray
// that heads `direction_y` upwards from `depth` blocks below its surface. Light comes down
// from the surface, fading as the water takes it, so the ray, rising or sinking into
// brighter or darker water, gathers (single scattering, integrated exactly)
//   light · e^(-k·depth) · s / (k·g) · (1 - e^(-k·distance·g)), with g = 1 - direction_y,
// k the water's extinction and s its scattering.
fn water_glow(a: Atmosphere, depth: f32, direction_y: f32, distance: f32) -> vec3<f32> {
    let scattered = scattering(a.water.rgb, a.water_scales.y);
    let extinction = absorption(a.water.rgb, a.water_scales.x) + scattered;
    let g = max(1.0 - direction_y, 1e-3);
    let gathered = scattered / (extinction * g) * (1.0 - exp(-extinction * distance * g));
    return a.water_light.rgb * exp(-extinction * depth) * gathered * UNDER_WATER_SCATTERING;
}

// Under water, the water between `eye` and what lies `offset` from it, which takes the
// air's place: the view closes in as the water absorbs red first and blue last (see
// voxmar::water), and fills with the light it scatters (`water_glow`). What lies below the
// surface was lit through the water above it too. What lies above it is seen through the
// water only as far as the surface, whose own shading (water_surface.wgsl) adds nothing
// more.
fn under_water(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>) -> Aerial {
    let extinction = absorption(a.water.rgb, a.water_scales.x) + scattering(a.water.rgb, a.water_scales.y);
    let surface = a.water.w;
    let distance = max(length(offset), 1e-4);
    var wet = distance;
    var down = surface - (eye.y + offset.y);
    if down < 0.0 {
        wet *= saturate((surface - eye.y) / max(offset.y, 1e-4));
        down = 0.0;
    }
    var out: Aerial;
    out.transmittance = exp(-extinction * (wet + down));
    out.light = water_glow(a, max(surface - eye.y, 0.0), offset.y / distance, wet);
    return out;
}

// A colour of the spectrum, red (0) through yellow and green to violet (1), soft as a
// rainbow's are (each colour spread over its neighbours).
fn spectrum(t: f32) -> vec3<f32> {
    let h = saturate(t) * 0.8;
    let rgb = saturate(abs(fract(vec3(h, h + 2.0 / 3.0, h + 1.0 / 3.0)) * 6.0 - 3.0) - 1.0);
    return mix(vec3(dot(rgb, vec3(0.3, 0.5, 0.2))), rgb, 0.75);
}

// A band from `inner` to `outer` degrees with edges a degree soft, 0..1.
fn band(angle: f32, inner: f32, outer: f32) -> f32 {
    return smoothstep(inner - 0.6, inner + 0.4, angle) * (1.0 - smoothstep(outer - 0.4, outer + 0.6, angle));
}

// A rainbow, in the rain between the eye and what lies beyond (the volumetric pass draws
// it over the clouds and the far land): opposite the sun, the primary bow red outside at
// 42° to violet at 40.5°, and with a double rainbow (strength over 0.75) the secondary bow
// from 50° to 53.5°, its colours reversed and fainter. Light to add along `dir`.
fn rainbow(a: Atmosphere, dir: vec3<f32>) -> vec3<f32> {
    let angle = degrees(acos(clamp(dot(dir, -a.bow.xyz), -1.0, 1.0)));
    if a.bow.w <= 0.0 || angle > 56.0 {
        return vec3(0.0);
    }
    var light = spectrum((42.0 - angle) / 1.5) * band(angle, 40.5, 42.0) * 0.45 * a.bow.w;
    if a.bow.w > 0.75 {
        let double = smoothstep(0.75, 0.9, a.bow.w);
        light += spectrum((angle - 50.0) / 3.5) * band(angle, 50.0, 53.5) * 0.14 * a.bow.w * double;
    }
    return light;
}

// Blocks per unit of the low cloud's noise coordinates (render::CLOUD_UNIT), and the
// scale of its body and of the billows at its edges in those units.
const CLOUD_UNIT: f32 = 256.0;
const LAYER_SCALE: f32 = 1.1;
const CLOUD_DETAIL: f32 = 7.0;
// Size of the 3D puffs of the low cloud near the eye, in blocks.
const BILLOW_SIZE: f32 = 22.0;
// The low cloud does not lie at one height: its clouds float higher or lower in drifts
// about CLOUD_UNIT / LIFT_SCALE blocks across, by up to LIFT_SHARE of the layer's depth
// either way (a little for flat layers, a lot for heaped cloud), so they meet the hills at
// different heights.
const LIFT_SCALE: f32 = 0.35;
const LIFT_SHARE: f32 = 0.6;

// How far the low cloud's layer is raised (or, below 0, sunk) at `offset` (x, z) from the
// eye, in blocks, within `cloud_lift_range` either way.
fn cloud_lift(a: Atmosphere, offset: vec2<f32>) -> f32 {
    let at = (a.clouds.xy + offset / CLOUD_UNIT) * LIFT_SCALE + vec2(31.7, -12.3);
    return (value_noise(at) - 0.5) * 2.0 * cloud_lift_range(a);
}

// The most the low cloud's layer is raised or sunk anywhere, in blocks: its clouds lie
// between its base less this and its top plus this.
fn cloud_lift_range(a: Atmosphere) -> f32 {
    return LIFT_SHARE * (a.cloud_shape.y - a.cloud_shape.x);
}

// Height through the low cloud's layer (0 at its base, 1 at its top) of `p`, where the
// layer is raised by `lift` (`cloud_lift`).
fn cloud_height(a: Atmosphere, p: vec3<f32>, lift: f32) -> f32 {
    return (p.y - lift - a.cloud_shape.x) / (a.cloud_shape.y - a.cloud_shape.x);
}

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

// Blocks over which the low cloud's underside thickens from clear to its full density.
const CLOUD_BASE_SOFTNESS: f32 = 8.0;

// How dense the low cloud is at `p`, 0..1, for the eye at `eye` (see `cloud_core`).
// `billows` (0..1, for samples near enough to show them) lets 3D puffs eat into its edges
// and tops, so heaped cloud is lumpy rather than smooth.
fn cloud_density(a: Atmosphere, p: vec3<f32>, eye: vec3<f32>, billows: f32) -> f32 {
    let range = cloud_lift_range(a);
    if a.clouds.z < 0.005 || p.y <= a.cloud_shape.x - range || p.y >= a.cloud_shape.y + range {
        return 0.0;
    }
    let h = cloud_height(a, p, cloud_lift(a, p.xz - eye.xz));
    if h <= 0.0 || h >= 1.0 {
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
    // The underside thickens over its last few blocks rather than at once, so flying up
    // into a cloud is a gradual whiteout, not a wall (Mariano's playtest, 2026-10-06).
    let soft_base = smoothstep(0.0, CLOUD_BASE_SOFTNESS / max(a.cloud_shape.y - a.cloud_shape.x, 1.0), h);
    return saturate(core * cloud_edge(a)) * soft_base;
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

// The share of a cloud's core over which its mirror image thickens, and how opaque that
// gets at most: waves smear what the sea mirrors.
const MIRRORED_EDGE: f32 = 0.6;
const MIRRORED_CLOUD: f32 = 0.7;

// The low cloud along `direction` (upwards) from `origin` (see `cloud_across`), for the sky
// in reflections: its base, lit by `sun` from above and by `sky`, seen through the air on
// the way, in premultiplied colour and opacity.
fn cloud_seen(a: Atmosphere, eye: vec3<f32>, origin: vec3<f32>, direction: vec3<f32>, to_sun: vec3<f32>, sun: vec3<f32>, sky: vec3<f32>) -> vec4<f32> {
    let across = cloud_across(a, eye, origin, direction);
    if across.x <= 0.0 {
        return vec4(0.0);
    }
    // A mirrored cloud is the whole layer's depth seen through its base, so its edge
    // thickens slowly rather than at the base's sharp outline (which mirrored as hard pale
    // stains across the sea), and it fades out well before the farthest cloud: far off,
    // where the mirrored ray grazes the layer, every gap closes and the sea filled with
    // flat white patches unlike any cloud overhead.
    // The core before `cloud_edge` sharpens it into the sky's outline, 0..1.
    let core = cloud_middle(a, origin.xz + direction.xz * across.y - eye.xz) / cloud_edge(a);
    let alpha = smoothstep(0.0, MIRRORED_EDGE, core) * smoothstep(0.01, 0.08, direction.y)
        * (1.0 - smoothstep(0.1, 0.5, across.y / max(a.cloud_look.w, 1.0))) * MIRRORED_CLOUD;
    if alpha <= 0.0 {
        return vec4(0.0);
    }
    // Lit as the sky draws a cloud's sunlit sides and underside together: brighter than the
    // shaded underside alone, which mirrored grey-brown against blue water.
    let white = vec3(dot(sun, vec3(0.2126, 0.7152, 0.0722)));
    let lit = (mix(sun, white, 0.5) * 0.8 + sky * 0.9) * (1.0 - 0.5 * a.cloud_shape.w);
    let air = air_along(a, origin, direction * across.y, to_sun);
    return vec4((lit * air.transmittance + air.light) * alpha, alpha);
}
