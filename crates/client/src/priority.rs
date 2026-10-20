//! Which of the game's threads a busy machine serves first.
//!
//! On a Mac crowded past its cores (Mariano's often runs at a load of 30-70 beside the game),
//! most slow frames he recorded were the main thread waiting on the render thread or on
//! Bevy's system threads (`outside_ms`), and the worst had PostUpdate's parts each stalled
//! for 100-190 ms at once: frame threads descheduled for other work. So the threads a frame
//! waits for (main, render, Bevy's systems) ask for the user-interactive class, which the
//! scheduler serves before default work (builds, scripts) and keeps on the performance cores,
//! and every thread a frame never waits for (worldgen, lighting, meshing, Bevy's io and async
//! pools, file reads) the utility class, below them. Both are requests any process may make;
//! nothing needs elevated rights.
//!
//! On Linux, where raising a thread needs privileges, frame threads stay as they are and
//! background ones are niced by 5. `VOXMAR_THREAD_PRIORITY=off` gives the threads the
//! classes they had before this split, for timing one build both ways: the streaming workers
//! lowered as they always were, every other thread left as it starts. (Leaving the workers at
//! the default too halved flight's frame rate on a busy Mac: that compares against no build.)

use std::sync::OnceLock;

use bevy::prelude::*;
use bevy::render::{Render, RenderApp};

/// Whether threads other than the streaming workers are given a class (not with
/// `VOXMAR_THREAD_PRIORITY=off`).
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("VOXMAR_THREAD_PRIORITY").map_or(true, |v| v != "off"))
}

/// Raises the calling thread to the class of threads a frame waits for.
pub fn frame_thread() {
    if !enabled() {
        return;
    }
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the scheduling class of the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// Puts the calling thread at the default class, between the two: work a frame does not
/// wait for but that should not queue behind streaming either. Said outright, since on
/// macOS whether a new thread takes its creator's class is not promised.
pub fn default_thread() {
    if !enabled() {
        return;
    }
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the scheduling class of the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_DEFAULT, 0);
    }
}

/// Lowers the calling thread below the game's frame threads.
pub fn background_thread() {
    if enabled() {
        lower();
    }
}

/// Lowers a streaming worker (the shared pool's) below the frame threads, with
/// `VOXMAR_THREAD_PRIORITY=off` too: they were lowered before the frame threads had a class.
pub fn streaming_worker() {
    lower();
}

fn lower() {
    #[cfg(target_os = "macos")]
    // SAFETY: only changes the scheduling class of the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux `nice` affects only the calling thread.
    unsafe {
        libc::nice(5);
    }
}

/// Spawns a thread that does work no frame waits for, at background priority.
pub fn spawn_background<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            background_thread();
            f()
        })
        .expect("a background thread starts")
}

/// Raises the render thread, which Bevy starts itself, the first time it runs the render
/// schedule: an exclusive system runs on the thread running the schedule.
pub struct PriorityPlugin;

impl Plugin for PriorityPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.add_systems(Render, |_: &mut World, mut raised: Local<bool>| {
            if !*raised {
                *raised = true;
                frame_thread();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn background_threads_run_below_the_others() {
        // SAFETY: getpriority only reads; on Linux, for 0, the calling thread's nice value.
        let nice = || unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        let (before, after) = std::thread::spawn(move || {
            let before = nice();
            super::background_thread();
            (before, nice())
        })
        .join()
        .unwrap();
        assert!(after > before || before == 19, "nice {before} -> {after}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn frame_and_background_threads_take_their_classes() {
        let class = || {
            let (mut class, mut relative) = (libc::qos_class_t::QOS_CLASS_UNSPECIFIED, 0);
            // SAFETY: writes the calling thread's class and relative priority into two locals.
            unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) };
            class
        };
        let (frame, background) = std::thread::spawn(move || {
            super::frame_thread();
            let frame = class();
            super::background_thread();
            (frame, class())
        })
        .join()
        .unwrap();
        assert!(matches!(frame, libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE));
        assert!(matches!(background, libc::qos_class_t::QOS_CLASS_UTILITY));
    }
}
