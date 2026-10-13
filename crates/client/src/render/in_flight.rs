//! At most two frames in flight on the GPU when drawing to a texture (headless captures).
//!
//! A window's swapchain holds the CPU back when the GPU falls behind: acquiring the next
//! image waits for one of the two or three it has. A capture drawing offscreen has no
//! swapchain, so in a scene limited by the GPU the CPU ran frames ahead until the driver
//! stopped it, for 5-6 ms in one submission every twenty-odd frames: hitches the game in a
//! window never has, which set the benchmark's p99 frame (the herd of creatures: 6.4 ms
//! with, 4.5 ms without). Each frame here waits, as acquiring a swapchain image would,
//! until the frame two before it has finished on the GPU.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bevy::prelude::*;
use bevy::render::render_resource::PollType;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

pub struct InFlightPlugin;

impl Plugin for InFlightPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<InFlight>()
                .add_systems(Render, (wait.in_set(RenderSystems::ExtractCommands), note.in_set(RenderSystems::Cleanup)));
        }
    }
}

/// Frames submitted, and frames the GPU has finished.
#[derive(Resource, Default)]
struct InFlight {
    submitted: u64,
    done: Arc<AtomicU64>,
}

/// Frames in flight on the GPU at most, counting the one about to be prepared.
const FRAMES: u64 = 2;

fn note(queue: Res<RenderQueue>, mut frames: ResMut<InFlight>) {
    frames.submitted += 1;
    let done = frames.done.clone();
    queue.on_submitted_work_done(move || {
        done.fetch_add(1, Ordering::Release);
    });
}

fn wait(device: Res<RenderDevice>, frames: Res<InFlight>) {
    let mut profile = super::work::measure(super::work::Part::Wait);
    if let Some(profile) = &mut profile {
        profile.counts.submitted = frames.submitted;
        profile.counts.done_before = frames.done.load(Ordering::Acquire);
    }
    while frames.submitted.saturating_sub(frames.done.load(Ordering::Acquire)) >= FRAMES {
        if let Some(profile) = &mut profile { profile.counts.polls += 1; }
        let _ = device.poll(PollType::Poll);
        std::thread::sleep(Duration::from_micros(50));
    }
    if let Some(profile) = &mut profile { profile.counts.done_after = frames.done.load(Ordering::Acquire); }
}
