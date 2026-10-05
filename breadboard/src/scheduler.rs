// SPDX-License-Identifier: MIT

//! Deterministic simulated-time event queue.
//!
//! Events are ordered by due cycle and then by the order they were scheduled.
//! That tie-break is what makes a run reproducible: two events due in the same
//! cycle always dispatch in a fixed, recorded order rather than in whatever
//! order a hash map or a sort happened to produce.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

/// Index of a registered [`Stimulus`](crate::Stimulus) within a host.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StimulusId(pub usize);

/// One event addressed to a stimulus, due at `due_cycle`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScheduledEvent {
    /// AVR cycle at which the event is due.
    pub due_cycle: u64,
    /// Stimulus the event is addressed to.
    pub stimulus: StimulusId,
    /// Stimulus-defined payload; the scheduler never interprets it.
    pub payload: u64,
}

/// Queue entry. Ordering uses only `due_cycle` and `sequence`, so payloads and
/// stimulus indices cannot perturb dispatch order.
#[derive(Clone, Copy, Debug)]
struct Entry {
    due_cycle: u64,
    sequence: u64,
    stimulus: StimulusId,
    payload: u64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.due_cycle == other.due_cycle && self.sequence == other.sequence
    }
}

impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.due_cycle
            .cmp(&other.due_cycle)
            .then_with(|| self.sequence.cmp(&other.sequence))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A simulated-time event queue in AVR cycles.
#[derive(Default)]
pub struct Scheduler {
    queue: BinaryHeap<Reverse<Entry>>,
    sequence: u64,
    scheduled: u64,
}

impl Scheduler {
    /// An empty scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedule `payload` for `stimulus` at `due_cycle`.
    ///
    /// A `due_cycle` already in the past is not rejected: it is due immediately,
    /// which keeps a model's arithmetic independent of host overshoot.
    pub fn schedule(&mut self, due_cycle: u64, stimulus: StimulusId, payload: u64) {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.scheduled = self.scheduled.wrapping_add(1);
        self.queue.push(Reverse(Entry {
            due_cycle,
            sequence,
            stimulus,
            payload,
        }));
    }

    /// Schedule every event in `events`.
    pub fn extend(&mut self, events: impl IntoIterator<Item = ScheduledEvent>) {
        for event in events {
            self.schedule(event.due_cycle, event.stimulus, event.payload);
        }
    }

    /// Remove and return the next event due at or before `now`.
    pub fn pop_due(&mut self, now: u64) -> Option<ScheduledEvent> {
        let due = self
            .queue
            .peek()
            .is_some_and(|Reverse(entry)| entry.due_cycle <= now);
        if !due {
            return None;
        }
        let Reverse(entry) = self.queue.pop()?;
        Some(ScheduledEvent {
            due_cycle: entry.due_cycle,
            stimulus: entry.stimulus,
            payload: entry.payload,
        })
    }

    /// Cycle of the earliest pending event, if any.
    pub fn earliest(&self) -> Option<u64> {
        self.queue.peek().map(|Reverse(entry)| entry.due_cycle)
    }

    /// Number of pending events.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True when no event is pending.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Total events scheduled since construction, including dispatched ones.
    pub fn scheduled(&self) -> u64 {
        self.scheduled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: StimulusId = StimulusId(0);
    const B: StimulusId = StimulusId(1);

    #[test]
    fn events_dispatch_in_due_cycle_order() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(30, A, 1);
        scheduler.schedule(10, B, 2);
        scheduler.schedule(20, A, 3);
        assert_eq!(scheduler.earliest(), Some(10));
        assert_eq!(scheduler.pop_due(9), None);
        assert_eq!(scheduler.pop_due(10).unwrap().payload, 2);
        assert_eq!(scheduler.pop_due(100).unwrap().payload, 3);
        assert_eq!(scheduler.pop_due(100).unwrap().payload, 1);
        assert!(scheduler.is_empty());
    }

    #[test]
    fn same_cycle_events_keep_their_scheduling_order() {
        let mut scheduler = Scheduler::new();
        for payload in 0..5 {
            scheduler.schedule(7, A, payload);
        }
        assert_eq!(scheduler.len(), 5);
        let order: Vec<u64> = std::iter::from_fn(|| scheduler.pop_due(7))
            .map(|event| event.payload)
            .collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
        assert_eq!(scheduler.scheduled(), 5);
    }

    #[test]
    fn a_past_due_cycle_is_due_immediately_with_its_scheduled_time_preserved() {
        let mut scheduler = Scheduler::new();
        scheduler.schedule(0, A, 9);
        let event = scheduler.pop_due(500).unwrap();
        assert_eq!(event.due_cycle, 0);
        assert_eq!(event.stimulus, A);
    }
}
