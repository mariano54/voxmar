// The cloud map (render/cloud_map.rs): the low cloud's 2D body at each texel, around the eye.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import voxmar::atmosphere::{Atmosphere, cloud_body_at}

@group(0) @binding(0) var<uniform> atmosphere: Atmosphere;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // The texel's centre, from the eye.
    let offset = atmosphere.cloud_map.xy + in.uv * atmosphere.cloud_map.z;
    return vec4(cloud_body_at(atmosphere, offset), 0.0, 0.0, 1.0);
}
