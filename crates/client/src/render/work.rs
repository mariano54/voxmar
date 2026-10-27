//! Optional wall/CPU measurements of streaming's render work, kept with slow frames.
//!
//! Chrome traces can themselves fill gigabytes while flying. `VOXMAR_PROFILE_FRAMES`
//! instead measures streaming and buffer operations in one fixed-size render-frame record.
//! The existing three-frame slow-frame ring owns the records; only a slow frame writes
//! JSON. Thread CPU excludes time asleep or waiting for a core, but is not GPU time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(super) enum Part { Wait, Receive, Compact, PageAllocate, NewPage, ArenaGrow, CompactWrite, ArenaWrite, ArenaSubmit, CompactBuffers, PageOwners }
const PARTS: usize = 11;

/// Only the fields named by the corresponding part's JSON are meaningful.
#[derive(Clone, Copy, Default)]
pub(super) struct Counts {
    pub submitted: u64,
    pub done_before: u64,
    pub done_after: u64,
    pub polls: u64,
    pub pending_before: u64,
    pub pending_after: u64,
    pub received: u64,
    pub scanned: u64,
    pub applied: u64,
    pub deferred: u64,
    pub behind: u64,
    pub near_past_budget: u64,
    pub finishing_past_budget: u64,
    pub quads: u64,
    pub blades: u64,
    pub payload_bytes: u64,
    pub column_groups: u64,
    pub views: u64,
    pub reused: u64,
    pub reused_after_unrelated_changes: u64,
    pub rebuilt: u64,
    pub copies: u64,
    pub batches: u64,
    pub indices: u64,
    pub buffers_grown: u64,
    pub dispatches: u64,
    pub original_writes: u64,
    pub original_bytes: u64,
    pub unique_dirty_words: u64,
}

#[derive(Clone, Copy)]
struct Measurement { wall_ms: f64, cpu_ms: Option<f64>, calls: u64, counts: Counts }

#[derive(Clone, Copy, Default)]
pub(super) struct Profile {
    frame: u64,
    finished_utc: f64,
    begun_monotonic: Option<f64>,
    finished_monotonic: Option<f64>,
    parts: [Option<Measurement>; PARTS],
}

impl Profile {
    fn record(&mut self, frame: u64, part: Part, sample: Measurement) {
        if self.frame != frame { return; }
        if let Some(previous) = &mut self.parts[part as usize] {
            previous.wall_ms += sample.wall_ms;
            previous.cpu_ms = previous.cpu_ms.zip(sample.cpu_ms).map(|(a, b)| a + b);
            previous.calls += sample.calls;
            macro_rules! add { ($($field:ident),*) => { $(previous.counts.$field += sample.counts.$field;)* }; }
            add!(submitted,done_before,done_after,polls,pending_before,pending_after,received,scanned,applied,deferred,
                behind,near_past_budget,finishing_past_budget,quads,blades,payload_bytes,column_groups,views,reused,reused_after_unrelated_changes,
                rebuilt,copies,batches,indices,buffers_grown,dispatches,original_writes,original_bytes,unique_dirty_words);
        } else { self.parts[part as usize] = Some(sample); }
    }

    pub(super) fn json(&self) -> Value {
        let mut parts = serde_json::Map::new();
        for (i, name) in ["wait", "receive", "compact", "page_allocate", "new_page", "arena_grow", "compact_write", "arena_write", "arena_submit", "compact_buffers", "page_owners"].into_iter().enumerate() {
            let value = self.parts[i].map_or(Value::Null, |sample| {
                let c = sample.counts;
                let counts = match i {
                    0 => json!({"submitted":c.submitted,"done_before":c.done_before,"done_after":c.done_after,
                        "outstanding_before":c.submitted.saturating_sub(c.done_before),
                        "outstanding_after":c.submitted.saturating_sub(c.done_after),"polls":c.polls}),
                    1 => json!({"pending_before":c.pending_before,"pending_after":c.pending_after,"received":c.received,
                        "scanned":c.scanned,"applied":c.applied,"deferred":c.deferred,"behind":c.behind,
                        "near_past_budget":c.near_past_budget,"finishing_past_budget":c.finishing_past_budget,
                        "quads":c.quads,"blades":c.blades,"input_bytes":c.payload_bytes,"column_groups_begun":c.column_groups}),
                    2 => json!({"views":c.views,"reused":c.reused,"reused_after_unrelated_changes":c.reused_after_unrelated_changes,"rebuilt":c.rebuilt,"copies":c.copies,"batches":c.batches,
                        "indices":c.indices,"dispatches":c.dispatches,"written_bytes":c.payload_bytes}),
                    3 => json!({"written_bytes":c.payload_bytes,"pages_added":c.buffers_grown,"writes":c.dispatches}),
                    4 | 9 => json!({"buffer_bytes_created":c.payload_bytes,"buffers_created":c.buffers_grown}),
                    5 => json!({"buffer_bytes_created":c.payload_bytes,"copied_bytes":c.copies,"buffers_created":c.buffers_grown,"submissions":c.dispatches}),
                    8 => json!({"copied_bytes":c.payload_bytes,"submissions":c.dispatches}),
                    10 => json!({"written_bytes":c.payload_bytes,"writes":c.dispatches,"original_writes":c.original_writes,
                        "original_bytes":c.original_bytes,"unique_dirty_words":c.unique_dirty_words}),
                    _ => json!({"written_bytes":c.payload_bytes,"writes":c.dispatches}),
                };
                json!({"wall_ms":sample.wall_ms,"thread_cpu_ms":sample.cpu_ms,"calls":sample.calls,"counts":counts})
            });
            parts.insert(name.to_string(), value);
        }
        json!({"render_local_frame":self.frame,"finished_utc":self.finished_utc,
            "begun_monotonic":self.begun_monotonic,"finished_monotonic":self.finished_monotonic,"parts":parts})
    }
}

static ENABLED: OnceLock<bool> = OnceLock::new();
static FRAME: AtomicU64 = AtomicU64::new(0);
static CURRENT: LazyLock<Mutex<Profile>> = LazyLock::new(|| Mutex::new(Profile::default()));
static ORIGIN: LazyLock<Instant> = LazyLock::new(Instant::now);

fn enabled() -> bool { *ENABLED.get_or_init(|| std::env::var_os("VOXMAR_PROFILE_FRAMES").is_some()) }

/// A shared process-local clock for joining main and render intervals without assuming
/// their frame counters are equal. The first pre-origin stamp is unavailable.
pub(crate) fn timestamp(stamp: Instant) -> Option<f64> {
    if !enabled() { return None; }
    stamp.checked_duration_since(*ORIGIN).map(|duration| duration.as_secs_f64())
}

pub(super) fn begin_frame(frame: u64) {
    if enabled() {
        let _ = *ORIGIN;
        if let Ok(mut current) = CURRENT.lock() { *current = Profile { frame, begun_monotonic: timestamp(Instant::now()), ..Profile::default() }; }
        FRAME.store(frame, Ordering::Relaxed);
    }
}

pub(super) fn finish_frame(frame: u64) -> Option<Profile> {
    if !enabled() { return None; }
    let mut current = CURRENT.lock().ok()?;
    if current.frame != frame { return None; }
    current.finished_utc = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
    current.finished_monotonic = timestamp(Instant::now());
    Some(*current)
}

/// Nanoseconds spent by this thread, unavailable on an unsupported/failed clock.
pub(crate) fn cpu_clock() -> Option<u128> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime writes one timespec at this valid mutable address.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut stamp) } != 0 { return None; }
        if stamp.tv_sec < 0 || !(0..1_000_000_000).contains(&stamp.tv_nsec) { return None; }
        Some(stamp.tv_sec as u128 * 1_000_000_000 + stamp.tv_nsec as u128)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { None }
}

fn cpu_ms(start: Option<u128>, end: Option<u128>) -> Option<f64> {
    Some(end?.checked_sub(start?)? as f64 / 1_000_000.0)
}

pub(super) struct Timer {
    part: Part,
    frame: u64,
    wall: Instant,
    cpu: Option<u128>,
    thread: std::thread::ThreadId,
    pub counts: Counts,
}

pub(super) fn measure(part: Part) -> Option<Timer> {
    enabled().then(|| Timer { part, frame: FRAME.load(Ordering::Relaxed), wall: Instant::now(),
        cpu: cpu_clock(), thread: std::thread::current().id(), counts: Counts::default() })
}

impl Drop for Timer {
    fn drop(&mut self) {
        let wall_ms = self.wall.elapsed().as_secs_f64() * 1000.0;
        let cpu_ms = (self.thread == std::thread::current().id()).then(|| cpu_ms(self.cpu, cpu_clock())).flatten();
        let sample = Measurement { wall_ms, cpu_ms, calls: 1, counts: self.counts };
        if let Ok(mut current) = CURRENT.lock() { current.record(self.frame, self.part, sample); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_or_reversed_cpu_clock_is_not_reported_as_zero_work() {
        assert_eq!(cpu_ms(None, Some(50)), None);
        assert_eq!(cpu_ms(Some(50), None), None);
        assert_eq!(cpu_ms(Some(50), Some(49)), None);
        assert_eq!(cpu_ms(Some(50), Some(1_000_050)), Some(1.0));
    }

    #[test]
    fn an_old_render_frame_cannot_supply_the_next_frames_measurement() {
        let sample = Measurement { wall_ms: 20.0, cpu_ms: Some(1.0), calls: 1, counts: Counts::default() };
        let mut profile = Profile { frame: 7, ..Profile::default() };
        profile.record(6, Part::Wait, sample);
        assert!(profile.parts[0].is_none());
        profile.record(7, Part::Receive, sample);
        assert_eq!(profile.json()["parts"]["receive"]["thread_cpu_ms"], 1.0);
        assert!(profile.json()["parts"]["wait"].is_null());
        profile = Profile { frame: 8, ..Profile::default() };
        assert!(profile.parts.iter().all(Option::is_none));
    }

    #[test]
    fn repeated_buffer_operations_accumulate_without_fabricating_cpu_time() {
        let mut profile = Profile { frame: 1, ..Profile::default() };
        let sample = Measurement { wall_ms: 2.0, cpu_ms: Some(1.0), calls: 1,
            counts: Counts { payload_bytes: 256, dispatches: 1, ..Counts::default() } };
        profile.record(1, Part::PageAllocate, sample);
        profile.record(1, Part::PageAllocate, Measurement { cpu_ms: None, ..sample });
        let value = profile.json();
        let entry = &value["parts"]["page_allocate"];
        assert_eq!(entry["wall_ms"], 4.0);
        assert_eq!(entry["calls"], 2);
        assert_eq!(entry["counts"]["written_bytes"], 512);
        assert!(entry["thread_cpu_ms"].is_null());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn the_native_thread_clock_counts_actual_work() {
        let first = cpu_clock().expect("supported native thread CPU clock");
        for i in 0..1000 { std::hint::black_box(i * i); }
        let last = cpu_clock().expect("supported native thread CPU clock");
        assert!(cpu_ms(Some(first), Some(last)).unwrap() > 0.0);
    }
}
