//! Builds each view's index buffer on the GPU from the quads of the chunks it sees, so
//! terrain takes a draw per page and stream instead of one per chunk, and pages need
//! store no indices. Culling on the CPU decides what is visible and where in the view's
//! buffer each chunk's indices go; a compute pass writes them, splitting each quad along
//! its darker diagonal, as the mesher does, from the ambient occlusion of its corners.
//! That costs the GPU a small fraction of a millisecond and saves the CPU a draw call
//! (and the driver's work behind it) for each of the thousands of chunks in view.

use std::num::NonZeroU64;
use std::ops::Range;

use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferBinding,
    BufferDescriptor, BufferUsages, CachedComputePipelineId, CommandEncoderDescriptor, ComputePassDescriptor,
    ComputePipelineDescriptor, PipelineCache, ShaderStages,
};
use bevy::render::renderer::{PendingCommandBuffers, RenderDevice, RenderQueue};
use rustc_hash::FxHashMap;

use super::{TerrainDraws, TerrainGpu};

pub(super) const COMPACT_SHADER: &str = "embedded://voxmar/render/terrain/compact.wgsl";
/// The most workgroups (one per copy) a dispatch may have.
const MAX_GROUPS: u32 = 65_535;
/// The most quads one copy covers. One workgroup does each copy, so long runs are split
/// to spread over the GPU's cores; this is four passes of compact.wgsl's 64 threads.
const COPY_LIMIT: u32 = 256;

/// A run of quads whose indices to write, as compact.wgsl reads it.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct Copy {
    /// The run's first vertex in its page, four to a quad.
    first_vertex: u32,
    destination: u32,
    quads: u32,
    /// 6, or 12 for quads drawn from both sides.
    indices_per_quad: u32,
}

/// Copies that read from one page, one dispatch.
#[derive(PartialEq, Eq)]
struct Batch {
    stream: u8,
    page: u32,
    copies: Range<u32>,
}

/// A view's copies for this frame, and the buffers they go through.
#[derive(Default)]
pub(super) struct Compaction {
    copies: Vec<Copy>,
    batches: Vec<Batch>,
    /// Indices gathered so far.
    len: u32,
    buffers: Option<ViewBuffers>,
    /// Whether this frame's copies were dispatched, so the gathered indices can be drawn.
    pub ready: bool,
    /// The copies and source pages last dispatched. Uploads to other pages cannot change
    /// these indices; a write to any source page or a world clear still invalidates them.
    last: LastDispatch,
}

#[derive(Default)]
struct LastDispatch {
    copies: Vec<Copy>,
    batches: Vec<Batch>,
    pages: Vec<Option<u64>>,
    generation: u32,
    /// Retained only to count reuse that the old whole-terrain check would reject.
    revision: u64,
}

impl Compaction {
    pub fn clear(&mut self) {
        if self.ready {
            self.last.copies.clear();
            self.last.batches.clear();
            std::mem::swap(&mut self.copies, &mut self.last.copies);
            std::mem::swap(&mut self.batches, &mut self.last.batches);
        }
        self.copies.clear();
        self.batches.clear();
        self.len = 0;
        self.ready = false;
    }

    /// Gathers the quads of ranges of one page's vertices, in order, and says where their
    /// indices ended up.
    pub fn gather(
        &mut self,
        stream: usize,
        page: u32,
        ranges: impl IntoIterator<Item = Range<u32>>,
        indices_per_quad: u32,
    ) -> Range<u32> {
        let start = self.len;
        let first = self.copies.len() as u32;
        for range in ranges {
            let (first_quad, end) = (range.start / 4, range.end / 4);
            for quad in (first_quad..end).step_by(COPY_LIMIT as usize) {
                let quads = (end - quad).min(COPY_LIMIT);
                self.copies.push(Copy { first_vertex: quad * 4, destination: self.len, quads, indices_per_quad });
                self.len += quads * indices_per_quad;
            }
        }
        let copies = first..self.copies.len() as u32;
        if !copies.is_empty() {
            self.batches.push(Batch { stream: stream as u8, page, copies });
        }
        start..self.len
    }

    /// The copies gathered so far, in order: where each run of quads starts, where its
    /// indices go, how many quads and indices each.
    #[cfg(test)]
    pub fn gathered(&self) -> impl Iterator<Item = [u32; 4]> + '_ {
        self.copies.iter().map(|copy| [copy.first_vertex, copy.destination, copy.quads, copy.indices_per_quad])
    }

    fn unchanged(&self, generation: u32, page_revision: impl Fn(&Batch) -> Option<u64>) -> bool {
        self.last.generation == generation
            && self.last.batches == self.batches
            && self.last.pages.len() == self.batches.len()
            && self.batches.iter().zip(&self.last.pages).all(|(batch, previous)| {
                page_revision(batch).is_some_and(|current| Some(current) == *previous)
            })
            && self.last.copies == self.copies
    }

    fn remember_pages(&mut self, generation: u32, revision: u64, page_revision: impl Fn(&Batch) -> Option<u64>) {
        self.last.pages.clear();
        self.last.pages.extend(self.batches.iter().map(page_revision));
        self.last.generation = generation;
        self.last.revision = revision;
    }

    /// The buffer the gathered indices are in.
    pub fn indices(&self) -> Option<&Buffer> {
        self.buffers.as_ref().map(|buffers| &buffers.indices)
    }
}

/// A view's buffers: the gathered indices, the copies, and each dispatch's first copy.
struct ViewBuffers {
    indices: Buffer,
    copies: Buffer,
    params: Buffer,
    /// Bind groups by stream and page; dropped when a buffer is replaced.
    bind_groups: FxHashMap<(u8, u32), BindGroup>,
    /// The terrain's generation the bind groups were made for (pages go on a clear).
    generation: u32,
}

#[derive(Resource)]
pub(super) struct CompactPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

pub(super) fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = BindGroupLayoutDescriptor::new(
        "terrain_compact_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(true, NonZeroU64::new(16)),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("terrain_compact".into()),
        layout: vec![layout.clone()],
        shader: asset_server.load(COMPACT_SHADER),
        zero_initialize_workgroup_memory: false,
        ..default()
    });
    commands.insert_resource(CompactPipeline { layout, pipeline });
}

/// Copies every view's visible chunk indices into place, before the frame is drawn.
pub(super) fn dispatch(
    mut views: Query<&mut TerrainDraws>,
    gpu: Res<TerrainGpu>,
    compact: Res<CompactPipeline>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let mut profile = crate::render::work::measure(crate::render::work::Part::Compact);
    let Some(pipeline) = cache.get_compute_pipeline(compact.pipeline) else { return };
    let layout = cache.get_bind_group_layout(&compact.layout);
    let stride = device.limits().min_uniform_buffer_offset_alignment as u64;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("terrain_compact") });
    let mut dispatched = false;
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("terrain_compact"), ..default() });
        pass.set_pipeline(pipeline);
        for mut draws in &mut views {
            if let Some(profile) = &mut profile { profile.counts.views += 1; }
            let compaction = &mut draws.compaction;
            if compaction.batches.is_empty() {
                continue;
            }
            let page_revision = |batch: &Batch| {
                gpu.streams.get(batch.stream as usize).and_then(|pool| pool.pages.get(batch.page as usize)).map(|page| page.revision)
            };
            let unchanged = compaction.buffers.is_some() && compaction.unchanged(gpu.generation, page_revision);
            if unchanged {
                if let Some(profile) = &mut profile {
                    profile.counts.reused += 1;
                    profile.counts.reused_after_unrelated_changes += u64::from(compaction.last.revision != gpu.revision);
                }
                compaction.last.revision = gpu.revision;
                compaction.ready = true;
                continue;
            }
            compaction.remember_pages(gpu.generation, gpu.revision, page_revision);
            if let Some(profile) = &mut profile {
                profile.counts.rebuilt += 1;
                profile.counts.copies += compaction.copies.len() as u64;
                profile.counts.batches += compaction.batches.len() as u64;
                profile.counts.indices += compaction.len as u64;
            }
            // Each dispatch reads its first copy from `params`, at a uniform offset; long
            // batches are split to stay within the dispatch size limit.
            let mut params = Vec::new();
            let mut dispatches = Vec::new();
            for batch in &compaction.batches {
                let mut first = batch.copies.start;
                while first < batch.copies.end {
                    let groups = (batch.copies.end - first).min(MAX_GROUPS);
                    let mut param = vec![0u8; stride as usize];
                    param[..4].copy_from_slice(&first.to_le_bytes());
                    params.extend(param);
                    dispatches.push((batch.stream, batch.page, groups));
                    first += groups;
                }
            }
            let buffers = ensure_buffers(&mut compaction.buffers, &device, compaction.len, compaction.copies.len(), params.len());
            if buffers.generation != gpu.generation {
                buffers.bind_groups.clear();
                buffers.generation = gpu.generation;
            }
            let written_bytes = (compaction.copies.len() * size_of::<Copy>() + params.len()) as u64;
            if let Some(profile) = &mut profile { profile.counts.payload_bytes += written_bytes; }
            {
                let mut writes = crate::render::work::measure(crate::render::work::Part::CompactWrite);
                if let Some(writes) = &mut writes { writes.counts.payload_bytes = written_bytes; writes.counts.dispatches = 2; }
                queue.write_buffer(&buffers.copies, 0, bytemuck::cast_slice(&compaction.copies));
                queue.write_buffer(&buffers.params, 0, &params);
            }
            for (i, (stream, page, groups)) in dispatches.into_iter().enumerate() {
                let Some(source) = gpu.streams.get(stream as usize).and_then(|pool| pool.pages.get(page as usize)) else {
                    continue;
                };
                let bind_group = buffers.bind_groups.entry((stream, page)).or_insert_with(|| {
                    device.create_bind_group(
                        "terrain_compact",
                        &layout,
                        &BindGroupEntries::sequential((
                            BufferBinding { buffer: &buffers.params, offset: 0, size: NonZeroU64::new(16) },
                            buffers.copies.as_entire_binding(),
                            source.vertex_buffer.as_entire_binding(),
                            buffers.indices.as_entire_binding(),
                        )),
                    )
                });
                pass.set_bind_group(0, &*bind_group, &[(i as u64 * stride) as u32]);
                pass.dispatch_workgroups(groups, 1, 1);
                if let Some(profile) = &mut profile { profile.counts.dispatches += 1; }
                dispatched = true;
            }
            compaction.ready = true;
        }
    }
    // Submitted with the frame's own command buffers, ahead of them, rather than on its
    // own: every submission costs time on Metal (see docs/rendering.md), and most frames
    // have nothing to copy at all.
    if dispatched {
        pending.push([encoder.finish()]);
    }
}

/// Makes sure a view's buffers hold this frame's indices, copies and parameters, growing
/// them by half again when they don't.
fn ensure_buffers<'a>(
    buffers: &'a mut Option<ViewBuffers>,
    device: &RenderDevice,
    indices: u32,
    copies: usize,
    params: usize,
) -> &'a mut ViewBuffers {
    let mut profile = crate::render::work::measure(crate::render::work::Part::CompactBuffers);
    let fits = |buffer: &Buffer, bytes: u64| buffer.size() >= bytes;
    let (indices, copies, params) = (indices as u64 * 4, (copies * size_of::<Copy>()) as u64, params as u64);
    let all_fit = buffers
        .as_ref()
        .is_some_and(|current| fits(&current.indices, indices) && fits(&current.copies, copies) && fits(&current.params, params));
    if all_fit {
        return buffers.as_mut().expect("checked above");
    }
    let mut buffer = |label: &str, bytes: u64, usage: BufferUsages| {
        let size = (bytes + bytes / 2).max(64 << 10).next_multiple_of(256);
        if let Some(profile) = &mut profile { profile.counts.buffers_grown += 1; profile.counts.payload_bytes += size; }
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let old = buffers.take();
    let keep = |old: Option<&Buffer>, bytes: u64| old.filter(|b| fits(b, bytes)).cloned();
    *buffers = Some(ViewBuffers {
        indices: keep(old.as_ref().map(|b| &b.indices), indices)
            .unwrap_or_else(|| buffer("terrain_view_indices", indices, BufferUsages::INDEX | BufferUsages::STORAGE)),
        copies: keep(old.as_ref().map(|b| &b.copies), copies)
            .unwrap_or_else(|| buffer("terrain_view_copies", copies, BufferUsages::STORAGE | BufferUsages::COPY_DST)),
        params: keep(old.as_ref().map(|b| &b.params), params)
            .unwrap_or_else(|| buffer("terrain_view_params", params, BufferUsages::UNIFORM | BufferUsages::COPY_DST)),
        bind_groups: FxHashMap::default(),
        generation: old.map_or(0, |b| b.generation),
    });
    buffers.as_mut().expect("just made")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gather(compaction: &mut Compaction) {
        compaction.gather(0, 1, [0..16], 6);
        compaction.gather(6, 2, [32..64], 6);
    }

    fn dispatched(compaction: &mut Compaction, generation: u32, revisions: &FxHashMap<(u8, u32), u64>) {
        compaction.remember_pages(generation, 10, |batch| revisions.get(&(batch.stream, batch.page)).copied());
        compaction.ready = true;
        compaction.clear();
        gather(compaction);
    }

    #[test]
    fn unrelated_page_writes_reuse_but_each_source_page_write_invalidates() {
        let mut compaction = Compaction::default();
        gather(&mut compaction);
        let mut revisions = FxHashMap::from_iter([((0, 1), 3), ((6, 2), 7), ((0, 9), 11)]);
        dispatched(&mut compaction, 4, &revisions);
        let unchanged = |compaction: &Compaction, revisions: &FxHashMap<(u8, u32), u64>| {
            compaction.unchanged(4, |batch| revisions.get(&(batch.stream, batch.page)).copied())
        };
        assert!(unchanged(&compaction, &revisions));
        revisions.insert((0, 9), 12);
        assert!(unchanged(&compaction, &revisions), "another page changes none of this view's indices");
        for key in [(0, 1), (6, 2)] {
            let previous = revisions[&key];
            revisions.insert(key, previous + 1);
            assert!(!unchanged(&compaction, &revisions), "same-sized vertex replacement can change quad diagonals");
            revisions.insert(key, previous);
            assert!(unchanged(&compaction, &revisions));
        }
        // A further reuse swaps the vectors again and must preserve the source stamps.
        compaction.ready = true;
        compaction.clear();
        gather(&mut compaction);
        assert!(unchanged(&compaction, &revisions));
    }

    #[test]
    fn world_clear_missing_page_and_changed_copies_cannot_reuse_indices() {
        let mut compaction = Compaction::default();
        gather(&mut compaction);
        let revisions = FxHashMap::from_iter([((0, 1), 3), ((6, 2), 7)]);
        dispatched(&mut compaction, 4, &revisions);
        let revision = |batch: &Batch| revisions.get(&(batch.stream, batch.page)).copied();
        assert!(compaction.unchanged(4, revision));
        assert!(!compaction.unchanged(5, revision), "new world pages may repeat old revision numbers");
        assert!(!compaction.unchanged(4, |batch| if batch.stream == 6 { None } else { revision(batch) }));
        compaction.copies[0].first_vertex += 4;
        assert!(!compaction.unchanged(4, revision));
        compaction.copies[0].first_vertex -= 4;
        compaction.batches[0].stream = 1;
        assert!(!compaction.unchanged(4, revision));
    }
}
