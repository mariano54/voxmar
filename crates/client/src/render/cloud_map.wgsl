// The cloud maps (render/cloud_map.rs): the low cloud's 2D body at each texel around the
// eye, and from it its density through the middle of its layer, where shadows look it up.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import voxmar::atmosphere::{Atmosphere, cloud_body_at, cloud_middle_of}

@group(0) @binding(0) var<uniform> atmosphere: Atmosphere;
// For the shade map, the body map.
@group(0) @binding(1) var body_map: texture_2d<f32>;
@group(0) @binding(2) var body_sampler: sampler;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // The texel's centre, from the eye.
    let offset = atmosphere.cloud_map.xy + in.uv * atmosphere.cloud_map.z;
    return vec4(cloud_body_at(atmosphere, offset), 0.0, 0.0, 1.0);
}

@fragment
fn shade(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let offset = atmosphere.cloud_shade.xy + in.uv * atmosphere.cloud_shade.z;
    // The shade map lies well within the body map.
    let uv = (offset - atmosphere.cloud_map.xy) / atmosphere.cloud_map.z;
    let body = textureSampleLevel(body_map, body_sampler, uv, 0.0).r;
    return vec4(cloud_middle_of(atmosphere, body, offset), 0.0, 0.0, 1.0);
}
