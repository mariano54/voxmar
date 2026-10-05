// Lightning bolts (bolts.rs): a bright core across each ribbon with a softer glow around
// it, blue-white and far brighter than anything lit, for bloom to spread.

#import bevy_pbr::forward_io::VertexOutput

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let across = in.uv.x;
    let core = exp(-across * across * 6.0);
    let fall = 1.0 - across * across;
    let glow = fall * fall * 0.3;
    // Premultiplied blending with no alpha adds the light to what lies behind.
    return vec4(vec3(0.75, 0.8, 1.0) * 8.0 * in.uv.y * (core + glow), 0.0);
}
