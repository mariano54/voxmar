// Cracks over the block being mined (see cracks.rs). The pipeline blends the output as
// source × destination × 2, so returning half of a factor multiplies the block behind by it.

#import bevy_pbr::forward_io::VertexOutput

// x: stage, y: strength 0..1, z: how much of the cracks' light edges show 0..1
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var stages: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var stages_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(stages, stages_sampler, in.uv, i32(params.x));
    // Minecraft multiplies its gamma-encoded image by twice the texel; on our linear image
    // the same change is that factor raised to 2.2.
    let factor = pow(2.0 * texel.rgb, vec3(2.2));
    // Dark lines darken the block under them, whatever its light. Their light edges (over
    // mid grey) are light the crack catches, so they show as much as the face is lit.
    let lit = min(factor, vec3(1.0)) + max(factor - vec3(1.0), vec3(0.0)) * params.z;
    let shown = select(0.0, params.y, texel.a > 0.1);
    return vec4(mix(vec3(1.0), lit, shown) * 0.5, 1.0);
}
