// Sky dome: horizon-to-zenith gradient, sun, moon and stars.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view

struct SkyParams {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    // xyz: direction towards the sun, w: daylight 0..1
    sun: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyParams;

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(12.9898, 78.233, 37.719))) * 43758.5453);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    let up = max(dir.y, 0.0);
    var color = mix(sky.horizon.rgb, sky.zenith.rgb, pow(up, 0.6));
    if dir.y < 0.0 {
        color = sky.horizon.rgb * mix(1.0, 0.75, min(-dir.y * 4.0, 1.0));
    }

    let daylight = sky.sun.w;
    let sun_dot = dot(dir, sky.sun.xyz);
    let sun_disc = smoothstep(0.99955, 0.9997, sun_dot);
    let glow = pow(max(sun_dot, 0.0), 48.0) * 0.35 + pow(max(sun_dot, 0.0), 6.0) * 0.08;
    color += vec3(1.0, 0.85, 0.6) * glow * clamp(daylight * 2.0, 0.0, 1.0);
    color = mix(color, vec3(6.0, 5.4, 4.2), sun_disc);

    let night = 1.0 - daylight;
    let moon = smoothstep(0.99965, 0.9998, -sun_dot);
    color = mix(color, vec3(1.4, 1.45, 1.6), moon * night);

    let cell = floor(dir * 300.0);
    let star = step(0.9985, hash(cell)) * smoothstep(0.0, 0.25, dir.y) * night * night;
    color += vec3(star * (0.6 + 0.4 * hash(cell + 1.0)));
    return vec4(color, 1.0);
}
