//! One MSAA resolve a frame, at the end of the main passes.
//!
//! Under MSAA every pass Bevy draws into the camera's image with
//! `ViewTarget::get_color_attachment` also resolves the multisampled image into the main one
//! when it ends: reading all four samples of every pixel and writing the result, a full
//! screen of memory traffic each time. Bevy's opaque pass must (the water reads the scene it
//! resolves), but the water and the volumetric composite draw into the samples without
//! resolving ([`unresolved_color_attachment`]), and the image is resolved once after them:
//! by Bevy's transparent pass when it draws anything, otherwise by [`resolve`]. At 2268×1473
//! on an NVIDIA L4 that took 0.3-0.5 ms off frames of 5-6 ms.

use bevy::core_pipeline::core_3d::{Transparent3d, main_transparent_pass_3d};
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::render_phase::ViewSortedRenderPhases;
use bevy::render::render_resource::{LoadOp, Operations, RenderPassColorAttachment, RenderPassDescriptor, StoreOp};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};

pub struct ResolvePlugin;

impl Plugin for ResolvePlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Core3d, resolve.after(main_transparent_pass_3d).in_set(Core3dSystems::MainPass));
        }
    }
}

/// The camera's image to draw into, without resolving it at the end of the pass: its samples
/// under MSAA (resolved later by Bevy's transparent pass or [`resolve`]), else the image.
pub fn unresolved_color_attachment(target: &ViewTarget) -> RenderPassColorAttachment<'_> {
    match target.sampled_main_texture_view() {
        Some(samples) => RenderPassColorAttachment {
            view: samples,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        },
        None => target.get_color_attachment(),
    }
}

/// Resolves the multisampled image into the main one when Bevy's transparent pass, which
/// would have, drew nothing.
fn resolve(view: ViewQuery<(&ViewTarget, &ExtractedView)>, phases: Res<ViewSortedRenderPhases<Transparent3d>>, mut ctx: RenderContext) {
    let (target, extracted) = view.into_inner();
    if target.sampled_main_texture_view().is_none()
        || phases.get(&extracted.retained_view_entity).is_some_and(|phase| !phase.items.is_empty())
    {
        return;
    }
    // A pass that draws nothing still resolves its attachment when it ends.
    ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("msaa_resolve"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}
