// Creature skins (creatures.rs): a low-poly body that bends at its joints, its colour read
// from a skin painted through the body's volume (engine creature::skin::paint_volume) at
// the point of the body at rest, snapped to the game's 32 texels a block so the surface
// shows the same pixels as the blocks. Faces are shaded flat, by their own normal, as the
// sun lights a block's sides; the material's colour carries the light where the creature
// stands and the red of a blow.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_bindings::material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    mesh_view_bindings::{view, lights},
}
#import voxmar::atmosphere::{Atmosphere, fogged}

struct SkinFrame {
    // xyz: the volume's corner in model space; w: texels a block.
    origin: vec4<f32>,
    // xyz: the volume's size in blocks.
    size: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var skin_texture: texture_3d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var skin_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> skin: SkinFrame;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var<storage, read> atmosphere: Atmosphere;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
#ifdef VERTEX_COLORS
    // The vertex colour carries the point of the body at rest.
    let rest = in.color.xyz;
#else
    let rest = vec3<f32>(0.0);
#endif
    let texels = skin.origin.w;
    let snapped = (floor(rest * texels) + 0.5) / texels;
    let texel = textureSampleLevel(skin_texture, skin_sampler, (snapped - skin.origin.xyz) / skin.size.xyz, 0.0);
    // Flat faces: the normal of the face itself, so the low-poly shape reads.
    let n = normalize(cross(dpdx(in.world_position.xyz), dpdy(in.world_position.xyz)));
    let shade = 0.5 + 0.5 * clamp(n.y * 0.5 + 0.5, 0.0, 1.0) + 0.06 * n.x - 0.04 * n.z;
    let lit = texel.rgb * material.base_color.rgb * shade;
    // Through the same air as the terrain: far creatures fade into it as hills do.
    let to_sun = lights.directional_lights[0].direction_to_light;
    let color = vec4<f32>(fogged(atmosphere, view.world_position, in.world_position.xyz, to_sun, lit), 1.0);
    pbr_input.material.base_color = color;
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, color);
    return out;
}
