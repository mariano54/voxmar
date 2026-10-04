// Half-resolution depth for water's reflection march (water.rs): the nearest of each 2×2
// pixels of the scene's depth (sample 0 under MSAA), so thin things still stand in the way
// of the rays, in a single-sample texture a quarter the size, which the march reads far
// faster than the full multisampled depth.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

#ifdef MULTISAMPLED
@group(0) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(0) var depth: texture_depth_2d;
#endif

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let last = vec2<i32>(textureDimensions(depth)) - 1;
    let p = vec2<i32>(in.position.xy) * 2;
    let a = textureLoad(depth, min(p, last), 0);
    let b = textureLoad(depth, min(p + vec2(1, 0), last), 0);
    let c = textureLoad(depth, min(p + vec2(0, 1), last), 0);
    let d = textureLoad(depth, min(p + vec2(1, 1), last), 0);
    // Reverse-Z: the nearest is the largest.
    return vec4(max(max(a, b), max(c, d)), 0.0, 0.0, 1.0);
}
