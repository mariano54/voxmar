//! GPU time of every frame, split at a few points (VOXMAR_GPU_FRAMES=FILE, Vulkan or DX12):
//! timestamps written before the terrain's compaction, at the start of the camera's passes,
//! after the shadow maps, after the main passes and at the end of post-processing, read back
//! a few frames later and appended to FILE as `frame,compaction,shadows,main,post,total` (ms),
//! with the main world's frame times in FILE.cpu as `frame,ms`. For finding which part of
//! the GPU's work makes a frame slow; nothing is recorded without the variable.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::diagnostic::FrameCount;
use bevy::pbr::{per_view_shadow_pass, shared_shadow_pass};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode, WgpuFeatures as Features};
use wgpu::{QuerySet, QuerySetDescriptor, QueryType};
use bevy::render::renderer::{PendingCommandBuffers, RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

pub struct GpuClockPlugin;

const MARKS: u32 = 5;
const RING: u32 = 8;
/// Query resolves land 256-byte aligned.
const STRIDE: u64 = 256;

impl Plugin for GpuClockPlugin {
    fn build(&self, app: &mut App) {
        let Some(path) = std::env::var_os("VOXMAR_GPU_FRAMES") else { return };
        let cpu = File::create(format!("{}.cpu", path.to_string_lossy())).expect("the GPU frames file can be written");
        let mut order = app.world_mut().resource_mut::<bevy::app::MainScheduleOrder>();
        order.insert_before(First, MainStart);
        order.insert_after(Last, MainEnd);
        app.insert_resource(CpuLog(Mutex::new(cpu), Mutex::new(None)))
            .add_systems(MainStart, |log: Res<CpuLog>| *log.1.lock().unwrap() = Some(std::time::Instant::now()))
            .add_systems(MainEnd, log_cpu);
        let gpu = File::create(&path).expect("the GPU frames file can be written");
        let render = File::create(format!("{}.render", path.to_string_lossy())).expect("the GPU frames file can be written");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(GpuLog(Mutex::new(gpu), Mutex::new(render)))
            .add_systems(RenderStartup, init)
            .insert_resource(RenderMarks::default())
            .add_systems(
                Render,
                (
                    mark_first.in_set(RenderSystems::PrepareAssets),
                    read_back.in_set(RenderSystems::Cleanup),
                    render_mark::<0>.in_set(RenderSystems::ExtractCommands),
                    render_mark::<1>.after(RenderSystems::Prepare).before(RenderSystems::Render),
                    render_mark::<2>.after(RenderSystems::Render).before(RenderSystems::Cleanup),
                    render_mark::<3>.in_set(RenderSystems::PostCleanup),
                ),
            )
            .add_systems(
                Core3d,
                (
                    mark::<1>.before(Core3dSystems::Prepass).before(per_view_shadow_pass::<false>).before(shared_shadow_pass::<false>),
                    mark::<2>
                        .after(per_view_shadow_pass::<true>)
                        .after(shared_shadow_pass::<true>)
                        .after(Core3dSystems::Prepass)
                        .before(Core3dSystems::MainPass),
                    mark::<3>.after(Core3dSystems::MainPass).before(Core3dSystems::EarlyPostProcess),
                    mark::<4>.after(Core3dSystems::PostProcess),
                ),
            );
    }
}

#[derive(Resource)]
struct CpuLog(Mutex<File>, Mutex<Option<std::time::Instant>>);

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct MainStart;
#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct MainEnd;

/// When the render world's schedule began, reached its graph, left it and ended.
#[derive(Resource, Default)]
struct RenderMarks([Option<std::time::Instant>; 4]);

fn render_mark<const N: usize>(marks: Option<ResMut<RenderMarks>>, frame: Option<Res<FrameCount>>, log: Option<Res<GpuLog>>) {
    let (Some(mut marks), Some(frame), Some(log)) = (marks, frame, log) else { return };
    marks.0[N] = Some(std::time::Instant::now());
    if N == 3
        && let [Some(a), Some(b), Some(c), Some(d)] = marks.0
    {
        let ms = |x: std::time::Instant, y: std::time::Instant| (y - x).as_secs_f64() * 1000.0;
        let _ = writeln!(log.1.lock().unwrap(), "{},{:.3},{:.3},{:.3}", frame.0, ms(a, b), ms(b, c), ms(c, d));
    }
}

#[derive(Resource)]
struct GpuLog(Mutex<File>, Mutex<File>);

struct Slot {
    readback: Buffer,
    mapped: Arc<AtomicBool>,
    requested: bool,
    frame: Option<u32>,
}

#[derive(Resource)]
struct GpuClock {
    set: QuerySet,
    resolve: Buffer,
    slots: Mutex<Vec<Slot>>,
    period_ns: f32,
}

fn log_cpu(log: Res<CpuLog>, frame: Res<FrameCount>, time: Res<Time<Real>>) {
    let busy = log.1.lock().unwrap().map_or(0.0, |start| start.elapsed().as_secs_f64() * 1000.0);
    let _ = writeln!(log.0.lock().unwrap(), "{},{:.3},{:.3}", frame.0, time.delta_secs() * 1000.0, busy);
}

fn init(mut commands: Commands, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    if !device.features().contains(Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
        warn!("VOXMAR_GPU_FRAMES: this device has no timestamp queries inside encoders");
        return;
    }
    let count = MARKS * RING;
    let set = device.wgpu_device().create_query_set(&QuerySetDescriptor { label: Some("gpu_clock"), ty: QueryType::Timestamp, count });
    let resolve = device.create_buffer(&BufferDescriptor {
        label: Some("gpu_clock_resolve"),
        size: RING as u64 * STRIDE,
        usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let slots = (0..RING)
        .map(|_| Slot {
            readback: device.create_buffer(&BufferDescriptor {
                label: Some("gpu_clock_readback"),
                size: MARKS as u64 * 8,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            mapped: Arc::new(AtomicBool::new(false)),
            requested: false,
            frame: None,
        })
        .collect();
    commands.insert_resource(GpuClock { set, resolve, slots: Mutex::new(slots), period_ns: queue.get_timestamp_period() });
}

/// Before the terrain's compaction, which is submitted ahead of the frame's passes.
fn mark_first(clock: Option<Res<GpuClock>>, frame: Res<FrameCount>, device: Res<RenderDevice>, mut pending: ResMut<PendingCommandBuffers>) {
    let Some(clock) = clock else { return };
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("gpu_clock") });
    encoder.write_timestamp(&clock.set, (frame.0 % RING) * MARKS);
    pending.push([encoder.finish()]);
}

fn mark<const N: u32>(_camera: ViewQuery<&ExtractedCamera>, clock: Option<Res<GpuClock>>, frame: Res<FrameCount>, mut ctx: RenderContext) {
    let Some(clock) = clock else { return };
    let slot = frame.0 % RING;
    let first = slot * MARKS;
    let encoder = ctx.command_encoder();
    encoder.write_timestamp(&clock.set, first + N);
    if N == MARKS - 1 {
        let mut slots = clock.slots.lock().unwrap();
        let entry = &mut slots[slot as usize];
        if entry.frame.is_some() {
            // Still being read back from RING frames ago: skip this frame.
            return;
        }
        encoder.resolve_query_set(&clock.set, first..first + MARKS, &clock.resolve, slot as u64 * STRIDE);
        encoder.copy_buffer_to_buffer(&clock.resolve, slot as u64 * STRIDE, &entry.readback, 0, MARKS as u64 * 8);
        entry.frame = Some(frame.0);
        entry.mapped.store(false, Ordering::Relaxed);
    }
}

fn read_back(clock: Option<Res<GpuClock>>, log: Res<GpuLog>) {
    let Some(clock) = clock else { return };
    let mut slots = clock.slots.lock().unwrap();
    for entry in slots.iter_mut() {
        let Some(frame) = entry.frame else { continue };
        if entry.mapped.load(Ordering::Acquire) {
            let times: Vec<u64> = {
                let data = entry.readback.slice(..).get_mapped_range();
                data.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
            };
            entry.readback.unmap();
            entry.frame = None;
            entry.requested = false;
            let ms = |a: usize, b: usize| times[b].wrapping_sub(times[a]) as f64 * clock.period_ns as f64 / 1e6;
            let _ = writeln!(
                log.0.lock().unwrap(),
                "{frame},{:.3},{:.3},{:.3},{:.3},{:.3}",
                ms(0, 1),
                ms(1, 2),
                ms(2, 3),
                ms(3, 4),
                ms(0, 4)
            );
        } else if !entry.requested {
            entry.requested = true;
            let mapped = entry.mapped.clone();
            entry.readback.slice(..).map_async(MapMode::Read, move |result| mapped.store(result.is_ok(), Ordering::Release));
        }
    }
}
