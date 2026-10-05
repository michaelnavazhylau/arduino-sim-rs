// SPDX-License-Identifier: MIT

//! The external-component contract.
//!
//! A [`Stimulus`] is anything wired to the MCU that has behaviour over time:
//! a sensor, a switch, a display, a second MCU. It is deliberately *not* an
//! [`analog_solver::Device`](https://docs.rs/analog-solver): a distance sensor
//! is not a two-terminal current/voltage law, and pretending it is would force
//! digital timing into the electrical solver.
//!
//! The host owns the timeline. A stimulus may only (a) observe the pins it
//! declares, (b) report the drive it wants on the pins it declares, and
//! (c) ask for a future callback through [`StimulusCtx`]. That keeps simulated
//! time in one place and keeps component models free of host state.

use crate::pin::{Drive, Pin};
use crate::scene::Scene;
use crate::scheduler::{ScheduledEvent, StimulusId};
use std::any::Any;

/// An observed input pin changed level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputChange {
    /// The pin that changed.
    pub pin: Pin,
    /// The new pad level.
    pub high: bool,
    /// Cycle at which the change was observed.
    ///
    /// Bounded by instruction granularity: a real AVR takes whole instructions,
    /// so the host cannot observe a transition between them. The correct AVR
    /// cycle count is reported, not the step count.
    pub observed_cycle: u64,
}

/// Simulated-time context handed to a stimulus during a dispatch.
///
/// `now` is the cycle the dispatch *represents*, which is the scheduled due
/// cycle for an event and the observed cycle for an input change. Using the
/// scheduled time rather than the host's current time keeps a model's arithmetic
/// independent of how far a step overshot, and therefore replayable.
pub struct StimulusCtx<'a> {
    /// Cycle this dispatch represents.
    pub now: u64,
    scene: &'a Scene,
    pending: &'a mut Vec<ScheduledEvent>,
    stimulus: StimulusId,
}

impl<'a> StimulusCtx<'a> {
    pub(crate) fn new(
        now: u64,
        scene: &'a Scene,
        pending: &'a mut Vec<ScheduledEvent>,
        stimulus: StimulusId,
    ) -> Self {
        Self {
            now,
            scene,
            pending,
            stimulus,
        }
    }

    /// The physical world this stimulus measures.
    pub fn scene(&self) -> &Scene {
        self.scene
    }

    /// Identifier of the stimulus being dispatched.
    pub fn stimulus(&self) -> StimulusId {
        self.stimulus
    }

    /// Ask to be called back at `due_cycle` with `payload`.
    ///
    /// A `due_cycle` at or before `now` is due on the next dispatch, so a model
    /// never needs to special-case a zero delay.
    pub fn schedule_at(&mut self, due_cycle: u64, payload: u64) {
        self.pending.push(ScheduledEvent {
            due_cycle,
            stimulus: self.stimulus,
            payload,
        });
    }

    /// Ask to be called back `delay_cycles` after this dispatch.
    pub fn schedule_after(&mut self, delay_cycles: u64, payload: u64) {
        self.schedule_at(self.now.saturating_add(delay_cycles), payload);
    }
}

/// A component wired to the MCU that has state and behaviour over time.
///
/// Implementations must be side-effect free with respect to the host: they may
/// only mutate themselves and use [`StimulusCtx`] to schedule. All timing is in
/// AVR cycles, so a faster host or a different frame rate cannot change results.
pub trait Stimulus {
    /// Stable label for diagnostics and tests.
    fn name(&self) -> &'static str;

    /// Pins observed as inputs. Only these produce [`Stimulus::on_input`].
    fn observes(&self) -> &[Pin] {
        &[]
    }

    /// Pins this stimulus may drive. [`Stimulus::drive`] is queried for each.
    fn drives(&self) -> &[Pin] {
        &[]
    }

    /// Drive currently asserted on `pin`.
    ///
    /// Return [`Drive::HighZ`] when not driving. The host resolves this against
    /// the AVR and every other stimulus before touching the pad, so a model does
    /// not need to know what else shares the net.
    fn drive(&self, _pin: Pin) -> Drive {
        Drive::HighZ
    }

    /// A previously scheduled event is due.
    fn on_event(&mut self, payload: u64, ctx: &mut StimulusCtx<'_>);

    /// One of the pins from [`Stimulus::observes`] changed level.
    fn on_input(&mut self, change: InputChange, ctx: &mut StimulusCtx<'_>);

    /// Downcast hook so a host can recover concrete stimulus state.
    fn as_any(&self) -> &dyn Any;
}
