//! Scheduling by need: a deterministic event queue for things that happen once at a known
//! tick (an action completing, a furnace finishing), and update tiers for processes that
//! run continuously at different rates. Idle things cost nothing: they are simply not in
//! the queue.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::FxHashSet;

pub type Tick = u64;

/// How often a process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Every tick: player movement, nearby physics, combat.
    High,
    /// Every 4 ticks: active machines, nearby creatures.
    Medium,
    /// Every 20 ticks (once a second): ecology, regeneration, fields near players.
    Low,
    /// Every 200 ticks: distant regions, populations, economies.
    VeryLow,
}

impl Tier {
    pub const fn period(self) -> Tick {
        match self {
            Tier::High => 1,
            Tier::Medium => 4,
            Tier::Low => 20,
            Tier::VeryLow => 200,
        }
    }

    pub const fn due(self, tick: Tick) -> bool {
        tick.is_multiple_of(self.period())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(u64);

/// Events ordered by tick, then by scheduling order, so the same inputs always fire in the
/// same order regardless of hashing or threads.
pub struct Scheduler<E> {
    queue: BinaryHeap<Reverse<(Tick, EventId)>>,
    events: rustc_hash::FxHashMap<EventId, E>,
    cancelled: FxHashSet<EventId>,
    next: u64,
}

impl<E> Default for Scheduler<E> {
    fn default() -> Self {
        Self { queue: BinaryHeap::new(), events: Default::default(), cancelled: Default::default(), next: 0 }
    }
}

impl<E> Scheduler<E> {
    pub fn schedule(&mut self, at: Tick, event: E) -> EventId {
        let id = EventId(self.next);
        self.next += 1;
        self.queue.push(Reverse((at, id)));
        self.events.insert(id, event);
        id
    }

    /// Cancels a pending event, returning it if it had not fired yet.
    pub fn cancel(&mut self, id: EventId) -> Option<E> {
        let event = self.events.remove(&id)?;
        self.cancelled.insert(id);
        Some(event)
    }

    /// Removes and returns the next event due at or before `now`.
    pub fn pop_due(&mut self, now: Tick) -> Option<(Tick, EventId, E)> {
        while let Some(&Reverse((at, id))) = self.queue.peek() {
            if at > now {
                return None;
            }
            self.queue.pop();
            if self.cancelled.remove(&id) {
                continue;
            }
            if let Some(event) = self.events.remove(&id) {
                return Some((at, id, event));
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_fire_in_tick_then_schedule_order() {
        let mut scheduler = Scheduler::default();
        scheduler.schedule(5, "b");
        let cancelled = scheduler.schedule(3, "x");
        scheduler.schedule(3, "a");
        scheduler.schedule(5, "c");
        assert_eq!(scheduler.cancel(cancelled), Some("x"));
        assert!(scheduler.pop_due(2).is_none());
        let fired: Vec<_> = std::iter::from_fn(|| scheduler.pop_due(10).map(|(t, _, e)| (t, e))).collect();
        assert_eq!(fired, [(3, "a"), (5, "b"), (5, "c")]);
        assert!(scheduler.is_empty());
    }
}
