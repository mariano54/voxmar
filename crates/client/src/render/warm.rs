//! Pipelines compiled before they are needed, and a watch for any that are not.
//!
//! On Metal, Bevy compiles a pipeline on the render thread the first time something needs
//! it, holding that frame: tens of milliseconds with the system's shader cache warm, a few
//! hundred on a fresh build. Whatever shows only now and then (another player, a creature,
//! the first block broken) would stall the frame it first appears in. `warm` keeps a mesh
//! and material drawn from the start where nothing of it shows: shrunk to a speck at the
//! eye, a millionth of its size, and never culled, so every view (and, for what casts
//! shadows, every shadow map) draws it and so needs its pipeline, while its triangles cover
//! no pixel and no texel. (It was once a box around the eye showing only back faces, which
//! are culled; but the box showed as a huge flat salmon wall, another player's colour, in
//! dark caves, so nothing that relies on where the eye is can draw anything large.)
//! `PipelineWatch` lists the pipelines compiled while it watches; the gate watches from the
//! world having loaded until leaving it, and fails on any.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::{PipelineCache, PipelineDescriptor};
use bevy::render::{Render, RenderApp, RenderSystems};

pub struct WarmPlugin;

impl Plugin for WarmPlugin {
    fn build(&self, app: &mut App) {
        let watch = PipelineWatch::default();
        app.insert_resource(watch.clone()).add_systems(PostUpdate, follow_eye.before(TransformSystems::Propagate));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(watch).add_systems(Render, note_pipelines.in_set(RenderSystems::Cleanup));
        }
    }
}

/// How small the speck is, as a share of its mesh.
const SPECK: f32 = 1e-6;

/// Keeps an entity on the eye, at its scale.
#[derive(Component)]
struct FollowEye(f32);

/// Keeps `material` on `mesh` compiled from now on. `lifetime` goes on what is spawned
/// (say, `DespawnOnExit` of the screen it serves).
pub fn warm<M: Material>(commands: &mut Commands, mesh: Handle<Mesh>, material: Handle<M>, casts_shadows: bool, lifetime: impl Bundle) {
    let speck = commands.spawn((FollowEye(SPECK), Mesh3d(mesh), MeshMaterial3d(material), Transform::default(), NoFrustumCulling, lifetime)).id();
    if !casts_shadows {
        commands.entity(speck).insert(NotShadowCaster);
    }
}

/// As [`warm`], for a skinned mesh (creature bodies): its one joint is the speck that
/// follows the eye, since a skinned mesh is placed by its joints.
pub fn warm_skinned<M: Material>(commands: &mut Commands, mesh: Handle<Mesh>, material: Handle<M>, binds: Handle<bevy::mesh::skinning::SkinnedMeshInverseBindposes>, lifetime: impl Bundle + Clone) {
    let joint = commands.spawn((FollowEye(SPECK), Transform::default(), lifetime.clone())).id();
    let skinned = bevy::mesh::skinning::SkinnedMesh { inverse_bindposes: binds, joints: vec![joint] };
    commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), skinned, Transform::default(), NoFrustumCulling, lifetime));
}

/// Keeps `root`, something drawn as it is in play (another player's figure, joints and
/// all), at the eye as a speck from now on, so its pipelines are compiled now. Whatever of
/// it is culled by where it is needs `NoFrustumCulling` too.
pub fn at_eye(commands: &mut Commands, root: Entity) {
    commands.entity(root).insert(FollowEye(SPECK));
}

fn follow_eye(camera: Query<&GlobalTransform, With<Camera3d>>, mut followers: Query<(&mut Transform, &FollowEye), Without<Camera3d>>) {
    let Ok(eye) = camera.single() else { return };
    for (mut transform, follow) in &mut followers {
        *transform = Transform::from_translation(eye.translation()).with_scale(Vec3::splat(follow.0));
    }
}

/// The pipelines compiled while it watches, by label; shared by the main and render worlds.
#[derive(Resource, Clone, Default)]
pub struct PipelineWatch {
    watching: Arc<AtomicBool>,
    compiled: Arc<Mutex<Vec<String>>>,
}

impl PipelineWatch {
    pub fn start(&self) {
        self.compiled.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.watching.store(true, Ordering::Relaxed);
    }

    /// Stops watching, with what was compiled meanwhile.
    pub fn stop(&self) -> Vec<String> {
        self.watching.store(false, Ordering::Relaxed);
        std::mem::take(&mut *self.compiled.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A render pipeline's specialisation, in brief: its shader defs, vertex attributes,
/// topology and culling, depth format and samples.
fn specialisation(descriptor: &PipelineDescriptor) -> String {
    let PipelineDescriptor::RenderPipelineDescriptor(d) = descriptor else { return "compute".to_owned() };
    let mut defs: Vec<String> = d.vertex.shader_defs.iter().chain(d.fragment.iter().flat_map(|f| f.shader_defs.iter())).map(|def| format!("{def:?}")).collect();
    defs.sort();
    defs.dedup();
    let attributes: Vec<String> = d.vertex.buffers.iter().map(|b| b.attributes.iter().map(|a| format!("{}:{:?}", a.shader_location, a.format)).collect::<Vec<_>>().join(",")).collect();
    format!(
        "defs [{}]; vertex [{}]; {:?} cull {:?}; depth {:?}; samples {}",
        defs.join(" "),
        attributes.join(" | "),
        d.primitive.topology,
        d.primitive.cull_mode,
        d.depth_stencil.as_ref().map(|s| s.format),
        d.multisample.count
    )
}

fn note_pipelines(cache: Res<PipelineCache>, watch: Res<PipelineWatch>, mut seen: Local<usize>) {
    let count = cache.pipelines().count();
    if count > *seen && watch.watching.load(Ordering::Relaxed) {
        let mut compiled = watch.compiled.lock().unwrap_or_else(|e| e.into_inner());
        for pipeline in cache.pipelines().skip(*seen) {
            let label = match &pipeline.descriptor {
                PipelineDescriptor::RenderPipelineDescriptor(d) => d.label.clone(),
                PipelineDescriptor::ComputePipelineDescriptor(d) => d.label.clone(),
            };
            let label = label.map_or_else(|| "unnamed".to_owned(), |l| l.into_owned());
            // What it was specialised for, so a miss names the variant to warm (in the
            // gate's log, which shows `voxmar::gate` at info).
            info!(target: "voxmar::gate", "pipeline compiled while playing: {label}: {}", specialisation(&pipeline.descriptor));
            compiled.push(label);
        }
    }
    *seen = count;
}
