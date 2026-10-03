// Sky dome: horizon-to-zenith gradient, drifting clouds, sun, moon and stars.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view

struct SkyParams {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: direction towards the sun, w: daylight 0..1
    sun: vec4<f32>,
    // x: seconds of game time, y: cloud cover 0..1
    clouds: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyParams;

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2(1.0, 0.0));
    let c = hash2(i + vec2(0.0, 1.0));
    let d = hash2(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        value += value_noise(q) * amplitude;
        q = q * 2.03 + vec2(17.1, 9.3);
        amplitude *= 0.5;
    }
    return value;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    let up = max(dir.y, 0.0);
    var color = mix(sky.horizon.rgb, sky.zenith.rgb, pow(up, 0.55));
    if dir.y < 0.0 {
        color = sky.horizon.rgb * mix(1.0, 0.7, min(-dir.y * 4.0, 1.0));
    }

    let daylight = sky.sun.w;
    let night = 1.0 - daylight;
    let sun_dot = dot(dir, sky.sun.xyz);

    let cell = floor(dir * 300.0);
    let star = step(0.9985, hash(cell)) * smoothstep(0.0, 0.25, dir.y) * night * night;
    color += vec3(star * (0.6 + 0.4 * hash(cell + 1.0)));

    let glow = pow(max(sun_dot, 0.0), 48.0) * 0.35 + pow(max(sun_dot, 0.0), 6.0) * 0.1;
    color += vec3(1.0, 0.8, 0.55) * glow * clamp(daylight * 2.0, 0.0, 1.0);
    color = mix(color, vec3(8.0, 7.2, 5.6), smoothstep(0.99955, 0.9997, sun_dot));
    color = mix(color, vec3(1.5, 1.55, 1.7), smoothstep(0.99965, 0.9998, -sun_dot) * night);

    // Clouds on a plane far above, fading out towards the horizon.
    if dir.y > 0.01 {
        let wind = vec2(sky.clouds.x * 0.004, sky.clouds.x * 0.0015);
        let p = dir.xz / (dir.y + 0.08) * 1.6 + wind;
        let density = smoothstep(0.62 - sky.clouds.y * 0.25, 0.92, fbm(p * 1.3));
        let shading = fbm(p * 1.3 + sky.sun.xz * 0.08);
        let lit = mix(vec3(1.0, 0.98, 0.95), vec3(1.0, 0.7, 0.5), 1.0 - smoothstep(0.0, 0.35, sky.sun.y));
        let cloud = mix(sky.horizon.rgb * 1.1, lit, 0.55 + 0.45 * (1.0 - shading)) * mix(0.08, 1.0, daylight)
            + vec3(1.0, 0.85, 0.6) * glow * 0.6;
        color = mix(color, cloud, density * smoothstep(0.01, 0.25, dir.y) * 0.9);
    }
    return vec4(color, 1.0);
}
