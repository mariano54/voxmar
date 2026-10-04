//! Gathers each view's visible chunk meshes into one index buffer on the GPU, so terrain
//! takes a draw per page and stream instead of one per chunk. Culling on the CPU decides
//! what is visible and where in the view's buffer each chunk's indices go; a compute pass
//! copies them. The copy costs the GPU a small fraction of a millisecond and saves the
//! CPU a draw call (and the driver's work behind it) for each of the thousands of chunks
//! in view.

use std::num::NonZeroU64;
use std::ops::Range;

use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferBinding,
    BufferDescriptor, BufferUsages, CachedComputePipelineId, CommandEncoderDescriptor, ComputePassDescriptor,
    ComputePipelineDescriptor, PipelineCache, ShaderStages,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use rustc_hash::FxHashMap;

use super::{TerrainDraws, TerrainGpu};

pub(super) const COMPACT_SHADER: &str = "embedded://voxmar/render/terrain/compact.wgsl";
/// The most workgroups (one per copy) a dispatch may have.
const MAX_GROUPS: u32 = 65_535;
/// The most indices one copy moves. One workgroup does each copy, so long runs are split
/// to spread over the GPU's cores; this is eight passes of compact.wgsl's 256 threads.
const COPY_LIMIT: u32 = 2048;

/// One range of indices to copy, as compact.wgsl reads it.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct Copy {
    source: u32,
    destination: u32,
    count: u32,
    padding: u32,
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
    /// The copies last dispatched, and the terrain's revision then: when nothing changed,
    /// last frame's gathered indices are still right and the copy is skipped.
    last: (Vec<Copy>, Vec<Batch>, u64),
}

impl Compaction {
    pub fn clear(&mut self) {
        if self.ready {
            self.last.0.clear();
            self.last.1.clear();
            std::mem::swap(&mut self.copies, &mut self.last.0);
            std::mem::swap(&mut self.batches, &mut self.last.1);
        }
        self.copies.clear();
        self.batches.clear();
        self.len = 0;
        self.ready = false;
    }

    /// Gathers ranges of one page's indices, in order, and says where they ended up.
    pub fn gather(&mut self, stream: usize, page: u32, ranges: impl IntoIterator<Item = Range<u32>>) -> Range<u32> {
        let start = self.len;
        let first = self.copies.len() as u32;
        for range in ranges {
            let end = range.end;
            for source in range.step_by(COPY_LIMIT as usize) {
                let count = (end - source).min(COPY_LIMIT);
                self.copies.push(Copy { source, destination: self.len, count, padding: 0 });
                self.len += count;
            }
        }
        let copies = first..self.copies.len() as u32;
        if !copies.is_empty() {
            self.batches.push(Batch { stream: stream as u8, page, copies });
        }
        start..self.len
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
) {
    let Some(pipeline) = cache.get_compute_pipeline(compact.pipeline) else { return };
    let layout = cache.get_bind_group_layout(&compact.layout);
    let stride = device.limits().min_uniform_buffer_offset_alignment as u64;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("terrain_compact") });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("terrain_compact"), ..default() });
        pass.set_pipeline(pipeline);
        for mut draws in &mut views {
            let compaction = &mut draws.compaction;
            if compaction.batches.is_empty() {
                continue;
            }
            let unchanged = compaction.buffers.is_some()
                && compaction.last.2 == gpu.revision
                && compaction.last.0 == compaction.copies
                && compaction.last.1 == compaction.batches;
            if unchanged {
                compaction.ready = true;
                continue;
            }
            compaction.last.2 = gpu.revision;
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
            queue.write_buffer(&buffers.copies, 0, bytemuck::cast_slice(&compaction.copies));
            queue.write_buffer(&buffers.params, 0, &params);
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
                            source.index_buffer.as_entire_binding(),
                            buffers.indices.as_entire_binding(),
                        )),
                    )
                });
                pass.set_bind_group(0, &*bind_group, &[(i as u64 * stride) as u32]);
                pass.dispatch_workgroups(groups, 1, 1);
            }
            compaction.ready = true;
        }
    }
    queue.submit([encoder.finish()]);
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
    let fits = |buffer: &Buffer, bytes: u64| buffer.size() >= bytes;
    let (indices, copies, params) = (indices as u64 * 4, (copies * size_of::<Copy>()) as u64, params as u64);
    let all_fit = buffers
        .as_ref()
        .is_some_and(|current| fits(&current.indices, indices) && fits(&current.copies, copies) && fits(&current.params, params));
    if all_fit {
        return buffers.as_mut().expect("checked above");
    }
    let buffer = |label: &str, bytes: u64, usage: BufferUsages| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: (bytes + bytes / 2).max(64 << 10).next_multiple_of(256),
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
