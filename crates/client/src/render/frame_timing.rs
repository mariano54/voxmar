//! How long the render thread spent on each part of its recent frames, for the slow-frame
//! record (`slow_frames.rs`): most slow frames while playing are time the main thread spent
//! waiting for the render thread, which the main world's own stages cannot explain.
//!
//! Marks between the render schedule's sets split a frame into: taking in what was
//! extracted, preparing assets and meshes and specialising pipelines (`assets`); preparing
//! views, which includes waiting for the window's next drawable, where a GPU that has
//! fallen behind shows (`views`); queueing and sorting draws (`queue`); preparing buffers
//! and bind groups (`prepare`); running the render graph, which encodes, submits and
//! presents, and builds any pipeline still queued (`render`); and cleaning up (`cleanup`).
//! A mark is a system ordered between two sets, so each part also holds whatever of its
//! neighbours ran in parallel; at the sizes of a hitch that does not matter.
//!
//! The main world's PostUpdate is split the same way, around its heaviest sets: visibility
//! (frustum culling every entity for every view), UI layout and transform propagation.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use serde_json::{Value, json};

pub struct FrameTimingPlugin;

/// The parts of a render frame, between consecutive marks.
const PARTS: [&str; 6] = ["assets", "views", "queue", "prepare", "render", "cleanup"];
/// Render frames kept: a slow main frame waited on the one or two before it.
const KEPT: usize = 3;

/// Milliseconds of each part of the last few render frames, newest last.
static FRAMES: LazyLock<Mutex<VecDeque<FrameRecord>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

struct FrameRecord {
    parts: [f32; PARTS.len()],
    work: Option<super::work::Profile>,
}

/// When each mark of the render frame under way was passed.
#[derive(Resource, Default)]
struct Marks([Option<Instant>; PARTS.len() + 1], u64);

impl Plugin for FrameTimingPlugin {
    fn build(&self, app: &mut App) {
        use bevy::camera::visibility::VisibilitySystems;
        use bevy::transform::TransformSystems;
        use bevy::ui::UiSystems;
        app.init_resource::<PostUpdateSpans>().add_systems(
            PostUpdate,
            (
                begin::<0>.before(VisibilitySystems::CheckVisibility),
                end::<0>.after(VisibilitySystems::CheckVisibility),
                begin::<1>.before(UiSystems::Layout),
                end::<1>.after(UiSystems::Layout),
                begin::<2>.before(TransformSystems::Propagate),
                end::<2>.after(TransformSystems::Propagate),
            ),
        );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        use RenderSystems as S;
        render_app.init_resource::<Marks>().add_systems(
            Render,
            (
                mark::<0>.before(S::ExtractCommands),
                mark::<1>.after(S::Specialize).after(S::PrepareAssets).before(S::PrepareViews),
                mark::<2>.after(S::PrepareViews).before(S::Queue),
                mark::<3>.after(S::PhaseSort).before(S::Prepare),
                mark::<4>.after(S::Prepare).before(S::Render),
                mark::<5>.after(S::Render).before(S::Cleanup),
                finish.after(S::PostCleanup),
            ),
        );
    }
}

fn mark<const I: usize>(mut marks: ResMut<Marks>) {
    if I == 0 {
        marks.1 += 1;
        super::work::begin_frame(marks.1);
    }
    marks.0[I] = Some(Instant::now());
}

fn finish(mut marks: ResMut<Marks>) {
    let end = Instant::now();
    let mut parts = [0.0; PARTS.len()];
    for (i, part) in parts.iter_mut().enumerate() {
        let next = if i + 1 < PARTS.len() { marks.0[i + 1] } else { Some(end) };
        if let (Some(a), Some(b)) = (marks.0[i], next) {
            *part = b.saturating_duration_since(a).as_secs_f32() * 1000.0;
        }
    }
    marks.0 = default();
    let Ok(mut frames) = FRAMES.lock() else { return };
    if frames.len() == KEPT {
        frames.pop_front();
    }
    frames.push_back(FrameRecord { parts, work: super::work::finish_frame(marks.1) });
}

/// The parts of the last few render frames, oldest first, for a slow frame's record.
pub fn recent() -> Value {
    let Ok(frames) = FRAMES.lock() else { return Value::Null };
    frames.iter().map(|frame| {
        let mut values = PARTS.iter().zip(&frame.parts).map(|(name, ms)| (name.to_string(), json!(*ms))).collect::<serde_json::Map<_, _>>();
        if let Some(work) = frame.work { values.insert("work".to_string(), work.json()); }
        values
    }).collect()
}

/// Spans of the main world's PostUpdate, each from just before a set to just after it.
const SPANS: [&str; 3] = ["visibility", "ui_layout", "transforms"];

/// When each PostUpdate span of this frame began and ended.
#[derive(Resource, Default)]
pub struct PostUpdateSpans([[Option<Instant>; 2]; SPANS.len()]);

impl PostUpdateSpans {
    /// Milliseconds of each span this frame, for a slow frame's record.
    pub fn json(&self) -> Value {
        SPANS
            .iter()
            .zip(&self.0)
            .map(|(name, [a, b])| {
                let ms = match (a, b) {
                    (Some(a), Some(b)) => b.saturating_duration_since(*a).as_secs_f32() * 1000.0,
                    _ => 0.0,
                };
                (name.to_string(), json!(ms))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }
}

fn begin<const I: usize>(mut spans: ResMut<PostUpdateSpans>) {
    spans.0[I] = [Some(Instant::now()), None];
}

fn end<const I: usize>(mut spans: ResMut<PostUpdateSpans>) {
    spans.0[I][1] = Some(Instant::now());
}
