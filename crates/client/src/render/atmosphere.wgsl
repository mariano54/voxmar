// The air between the eye and what it sees: clear-air haze that thins with height, a layer
// of ground fog (valley fog, dawn mist, sea fog) lying under a height, and the weather's
// fog, each dimming what lies behind it and scattering sky light and sunlight towards the
// eye. Everything is integrated analytically along the view ray, so it costs a few
// exponentials per pixel and never bands or shimmers. The terrain and the sky share these
// functions and one uniform (render/atmosphere.rs).

#define_import_path voxmar::atmosphere

struct Atmosphere {
    // Clear-air haze. rgb: its colour, lit by the sky (the horizon's colour), w: how much
    // light it takes out per block at its base.
    haze: vec4<f32>,
    // x: the height haze is densest at, y: the height over which it thins by e, z and w:
    // where the distant terrain's edge starts and finishes fading into the sky.
    haze_shape: vec4<f32>,
    // Sunlight scattered by the air. rgb: its colour around the sun (as the sky's glow), w:
    // unused.
    glow: vec4<f32>,
    // Fog. rgb: its colour, lit by the sky and the sun, w: how much light the weather's fog
    // (mist, whiteout, rain, dust) takes out per block at its base.
    fog: vec4<f32>,
    // x: the height the weather's fog is thick below (around the eye, whose weather it
    // is), y: the height over which it thins by e above that.
    fog_shape: vec4<f32>,
    // Ground fog. x: light taken out per block inside it, y: the height of its top, z: how
    // gradually it thins above that, w: unused.
    mist: vec4<f32>,
    // The ground. w: how much sunlight it bounces onto walls and undersides; x, y, z
    // unused.
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

// ∫ of the ground fog's density profile 1 / (1 + e^((y - top) / soft)): full below the
// top, thinning smoothly above it.
fn mist_profile_integral(a: Atmosphere, y: f32) -> f32 {
    let x = (y - a.mist.y) / a.mist.z;
    let softplus = max(x, 0.0) + log(1.0 + exp(-abs(x)));
    return y - a.mist.z * softplus;
}

fn mist_density(a: Atmosphere, y: f32) -> f32 {
    return 1.0 / (1.0 + exp(clamp((y - a.mist.y) / a.mist.z, -30.0, 30.0)));
}

// Light taken out by ground fog between heights `y0` and `y0 + dy` over `distance` blocks.
fn mist_depth(a: Atmosphere, y0: f32, dy: f32, distance: f32) -> f32 {
    if a.mist.x <= 0.0 {
        return 0.0;
    }
    if abs(dy) < 0.01 {
        return a.mist.x * distance * mist_density(a, y0 + 0.5 * dy);
    }
    let mean = (mist_profile_integral(a, y0 + dy) - mist_profile_integral(a, y0)) / dy;
    return a.mist.x * distance * max(mean, 0.0);
}

// All the light taken out along `offset` from a point at height `y0`.
fn optical_depth(a: Atmosphere, y0: f32, offset: vec3<f32>) -> vec3<f32> {
    let distance = length(offset);
    return vec3(haze_depth(a, y0, offset.y, distance), mist_depth(a, y0, offset.y, distance), weather_fog_depth(a, y0, offset.y, distance));
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
    let dy = direction.y * reach;
    return 1.0 - exp(-(mist_depth(a, eye.y, dy, reach) + weather_fog_depth(a, eye.y, dy, reach)));
}

// What lies beyond everything looking along `direction`: the horizon's haze glowing
// around the sun, behind whatever fog fills the view. Distant terrain fades into it and
// the sky shows it, so they meet seamlessly.
fn far_color(a: Atmosphere, eye: vec3<f32>, direction: vec3<f32>, to_sun: vec3<f32>) -> vec3<f32> {
    return mix(haze_color(a, direction, to_sun), a.fog.rgb, edge_fog(a, eye, direction));
}

// The air between `eye` and what lies `offset` from it. Sky light scattered by each
// medium takes its colour, sunlight scattered by it glows around the sun, and the far
// edge of the distant terrain fades completely into what lies beyond.
fn aerial(a: Atmosphere, eye: vec3<f32>, offset: vec3<f32>, to_sun: vec3<f32>) -> Aerial {
    let distance = max(length(offset), 1e-4);
    let direction = offset / distance;
    let depth = optical_depth(a, eye.y, offset);
    let total = depth.x + depth.y + depth.z;
    let through = exp(-total);
    let sky_lit = (a.haze.rgb * depth.x + a.fog.rgb * (depth.y + depth.z)) / max(total, 1e-6);
    let sunlit = 1.0 - through;
    let glow = a.glow.rgb * sun_glow(dot(direction, to_sun));
    let edge = smoothstep(a.haze_shape.z, a.haze_shape.w, distance);
    var out: Aerial;
    out.transmittance = through * (1.0 - edge);
    out.light = (sky_lit * (1.0 - through) + glow * sunlit) * (1.0 - edge);
    if edge > 0.0 {
        out.light += far_color(a, eye, direction, to_sun) * edge;
    }
    return out;
}
