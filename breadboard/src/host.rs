// SPDX-License-Identifier: MIT

//! Headless host: steps the AVR and couples external components to it.
//!
//! The host is the single owner of simulated time. Each pass:
//!
//! 1. samples the `PINx` pad level of every observed pin,
//! 2. delivers any transitions to the owning stimulus,
//! 3. dispatches scheduled events that are now due,
//! 4. resolves and applies external drive to pads the AVR is not driving,
//! 5. executes one AVR instruction plus its peripheral tick.
//!
//! Nothing here knows about rendering, and no component can read the wall
//! clock, so a fast host, a slow host and a paused-then-resumed host all produce
//! the same waveform.

use crate::analog::{AnalogCoupling, CouplingError};
use crate::bus::{self, Bridge, BridgeBackend, BusError, I2cSlave, SpiSlave};
use crate::pin::{Drive, Pin, Port};
use crate::scene::Scene;
use crate::scheduler::{Scheduler, StimulusId};
use crate::stimulus::{InputChange, Stimulus, StimulusCtx};
use crate::time::{cycles_to_micros, micros_to_cycles};
use avr8rs::board::{parse_hex, Board, PinState};
use avr8rs::runtime::Handle;
use avr8rs::{native_backend, Backend, Runtime, Value};
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::error::Error;
use std::fmt;
use std::rc::Rc;

/// Upper bound on events dispatched for one cycle, so a model that keeps
/// rescheduling at the current cycle fails instead of hanging.
const MAX_EVENTS_PER_STEP: usize = 1024;

/// Why a host step could not proceed.
#[derive(Clone, Debug, PartialEq)]
pub enum HostError {
    /// Two drivers assert opposite strong levels on one pin.
    PinConflict { pin: Pin, a: Drive, b: Drive },
    /// A stimulus kept scheduling work at the current cycle.
    EventStorm { cycle: u64 },
    /// The coupled analog circuit could not be set up or updated.
    Coupling(CouplingError),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PinConflict { pin, a, b } => {
                write!(f, "pin {pin} conflict: {a:?} against {b:?}")
            }
            Self::EventStorm { cycle } => {
                write!(f, "event storm at cycle {cycle}: dispatch limit exceeded")
            }
            Self::Coupling(error) => write!(f, "{error}"),
        }
    }
}

impl Error for HostError {}

/// One observed pin belonging to one stimulus.
struct Observed {
    pin: Pin,
    stimulus: StimulusId,
    /// `None` until the first sample, so power-on is not reported as an edge.
    last: Option<bool>,
}

/// A headless Uno with external components attached.
pub struct BreadboardHost {
    backend: Rc<dyn Backend>,
    bridge: Rc<RefCell<Bridge>>,
    runtime: Runtime,
    board: Board,
    scene: Scene,
    scheduler: Scheduler,
    stimuli: Vec<Box<dyn Stimulus>>,
    observed: Vec<Observed>,
    pending_inputs: Vec<(StimulusId, InputChange)>,
    analog: Option<AnalogCoupling>,
    instructions: u64,
}

impl BreadboardHost {
    /// Boot `flash` and attach `stimuli`.
    ///
    /// `flash` is a byte image of at least the program; an image shorter than
    /// flash is fine, as unprogrammed words read as `0xff`. Use
    /// [`BreadboardHost::from_hex`] for an Intel HEX image.
    pub fn new(
        flash: Vec<u8>,
        scene: Scene,
        stimuli: Vec<Box<dyn Stimulus>>,
    ) -> Result<Self, HostError> {
        let backend: Rc<dyn Backend> = native_backend();
        let bridge = Rc::new(RefCell::new(Bridge::new()));
        let backend: Rc<dyn Backend> = Rc::new(BridgeBackend::new(backend, bridge.clone()));
        let mut runtime = Runtime::new(backend.clone());
        let board = Board::uno(backend.as_ref(), &mut runtime, flash);
        let mut observed = Vec::new();
        for (index, stimulus) in stimuli.iter().enumerate() {
            let id = StimulusId(index);
            for &pin in stimulus.observes() {
                observed.push(Observed {
                    pin,
                    stimulus: id,
                    last: None,
                });
            }
        }
        let mut host = Self {
            backend,
            bridge,
            runtime,
            board,
            scene,
            scheduler: Scheduler::new(),
            stimuli,
            observed,
            pending_inputs: Vec::new(),
            analog: None,
            instructions: 0,
        };
        // Resolve the idle drive now so the first instruction already sees the
        // pads a wired-up breadboard would present.
        host.apply_drives()?;
        Ok(host)
    }

    /// Boot an Intel HEX image and attach `stimuli`.
    pub fn from_hex(
        hex: &str,
        scene: Scene,
        stimuli: Vec<Box<dyn Stimulus>>,
    ) -> Result<Self, HostError> {
        Self::new(parse_hex(hex), scene, stimuli)
    }

    /// Execute one AVR instruction and dispatch any work it makes due.
    pub fn step(&mut self) -> Result<(), HostError> {
        let now = self.now();
        self.sample_inputs(now);
        self.dispatch_inputs(now);
        self.dispatch_events(now)?;
        self.apply_drives()?;
        self.update_analog()?;
        self.board.step(self.backend.as_ref(), &mut self.runtime);
        self.instructions += 1;
        if self.instructions > 0
            && self
                .instructions
                .is_multiple_of(Board::BUDGET_REFILL_CYCLES)
        {
            self.runtime.reset_budget();
        }
        Ok(())
    }

    /// Execute at least `cycles` simulated AVR cycles.
    ///
    /// The instruction being executed can carry the cycle count past the target,
    /// so the elapsed value may exceed `cycles` by up to one instruction.
    pub fn advance_cycles(&mut self, cycles: u64) -> Result<(), HostError> {
        let target = self.now().saturating_add(cycles);
        while self.now() < target {
            self.step()?;
        }
        Ok(())
    }

    /// Execute `micros` simulated microseconds, rounded to whole cycles.
    pub fn advance_micros(&mut self, micros: f64) -> Result<(), HostError> {
        self.advance_cycles(micros_to_cycles(micros))
    }

    /// Simulated AVR cycles elapsed.
    pub fn now(&self) -> u64 {
        self.board.cycles(self.backend.as_ref()) as u64
    }

    /// Simulated microseconds elapsed.
    pub fn micros(&self) -> f64 {
        cycles_to_micros(self.now())
    }

    /// Write one data-space byte through the CPU's register hooks.
    ///
    /// Writes go through the same hooks firmware uses, so GPIO, timers and
    /// interrupt flags react as they would to an `out` instruction. This exists
    /// so a test can set a pin up without compiling a sketch; it is not a
    /// component interface. An invalid address panics in the backend.
    pub fn write_data(&mut self, address: usize, value: u8) {
        self.backend.call(
            &mut self.runtime,
            Some(self.board.cpu_handle),
            "writeData",
            vec![Value::Number(address as f64), Value::Number(value as f64)],
        );
    }

    /// The physical world components measure.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// State of the onboard LED: Arduino digital pin 13 (`PORTB5`).
    pub fn led_builtin(&mut self) -> avr8rs::board::PinState {
        self.board
            .led_builtin(self.backend.as_ref(), &mut self.runtime)
    }

    /// Attach an analog circuit coupled to specific pins.
    ///
    /// A pin driven by a [`Stimulus`] must not also be analog-bound: the two
    /// would fight over the same pad, and the analog layer would win silently.
    pub fn attach_analog(&mut self, coupling: AnalogCoupling) -> Result<(), HostError> {
        for (pin, _) in coupling.wiring().bindings() {
            let driven = self
                .stimuli
                .iter()
                .any(|stimulus| stimulus.drives().contains(pin));
            if driven {
                return Err(HostError::Coupling(CouplingError::DuplicateBinding(*pin)));
            }
        }
        self.analog = Some(coupling);
        self.update_analog()
    }

    /// The coupled analog circuit, when one is attached.
    pub fn analog(&self) -> Option<&AnalogCoupling> {
        self.analog.as_ref()
    }

    /// Mutable access to the coupling, for throwing a switch or moving a part.
    pub fn analog_mut(&mut self) -> Option<&mut AnalogCoupling> {
        self.analog.as_mut()
    }

    /// Attach `slaves` to the I2C bus and route `AVRTWI` to them.
    pub fn attach_i2c(&mut self, slaves: Vec<Box<dyn I2cSlave>>) {
        self.backend.set(
            &mut self.runtime,
            self.board.twi,
            "eventHandler",
            bus::twi_handler(),
        );
        self.bridge.borrow_mut().attach_i2c(self.board.twi, slaves);
    }

    /// Attach the single SPI device and route `AVRSPI` transfers to it.
    pub fn attach_spi(&mut self, slaves: Vec<Box<dyn SpiSlave>>) -> Result<(), BusError> {
        // Validate before installing the handler, so a rejected bus leaves the
        // peripheral with its original, inert callback.
        self.bridge.borrow_mut().attach_spi(slaves)?;
        self.backend.set(
            &mut self.runtime,
            self.board.spi,
            "onTransfer",
            bus::spi_transfer_handler(),
        );
        Ok(())
    }

    /// Shared access to the attached bus devices.
    pub fn bridge(&self) -> Ref<'_, Bridge> {
        self.bridge.borrow()
    }

    /// Mutable access, for refreshing a device's registers from the host.
    pub fn bridge_mut(&mut self) -> RefMut<'_, Bridge> {
        self.bridge.borrow_mut()
    }

    fn update_analog(&mut self) -> Result<(), HostError> {
        let Some(coupling) = self.analog.as_mut() else {
            return Ok(());
        };
        coupling
            .update(self.backend.as_ref(), &mut self.runtime, &self.board)
            .map_err(HostError::Coupling)?;
        Ok(())
    }

    /// Replace the world, leaving the board and components running.
    pub fn set_scene(&mut self, scene: Scene) {
        self.scene = scene;
    }

    /// The simulated board.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The AVR backend, for scenario-style access.
    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    /// The dispatch runtime, for direct register or pin access in tests.
    pub fn runtime(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    /// Every attached component, in registration order.
    pub fn stimuli(&self) -> &[Box<dyn Stimulus>] {
        &self.stimuli
    }

    /// The event queue.
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    /// AVR instructions retired.
    pub fn instructions(&self) -> u64 {
        self.instructions
    }

    /// The first attached component of concrete type `T`.
    ///
    /// State lives in the component, so a test reads a measurement from it
    /// rather than from a separately maintained mirror.
    pub fn find<T: Any>(&self) -> Option<&T> {
        self.stimuli
            .iter()
            .find_map(|stimulus| stimulus.as_any().downcast_ref::<T>())
    }

    fn port_handle(&self, port: Port) -> Handle {
        match port {
            Port::B => self.board.portb,
            Port::C => self.board.portc,
            Port::D => self.board.portd,
        }
    }

    /// Read each observed pad and queue any transition.
    fn sample_inputs(&mut self, now: u64) {
        let backend = self.backend.as_ref();
        for entry in &mut self.observed {
            let (port, bit) = entry.pin.port_bit();
            let high = self.board.read_bit(backend, port.pin_register(), bit);
            if entry.last == Some(high) {
                continue;
            }
            entry.last = Some(high);
            self.pending_inputs.push((
                entry.stimulus,
                InputChange {
                    pin: entry.pin,
                    high,
                    observed_cycle: now,
                },
            ));
        }
    }

    /// Deliver queued transitions to their owning components.
    fn dispatch_inputs(&mut self, now: u64) {
        let changes = std::mem::take(&mut self.pending_inputs);
        for (id, change) in changes {
            let mut scheduled = Vec::new();
            {
                let stimulus = &mut self.stimuli[id.0];
                let mut ctx = StimulusCtx::new(now, &self.scene, &mut scheduled, id);
                stimulus.on_input(change, &mut ctx);
            }
            self.scheduler.extend(scheduled);
        }
    }

    /// Dispatch every event due at or before `now`.
    fn dispatch_events(&mut self, now: u64) -> Result<(), HostError> {
        let mut dispatched = 0usize;
        while let Some(event) = self.scheduler.pop_due(now) {
            dispatched += 1;
            if dispatched > MAX_EVENTS_PER_STEP {
                return Err(HostError::EventStorm { cycle: now });
            }
            let mut scheduled = Vec::new();
            {
                let stimulus = &mut self.stimuli[event.stimulus.0];
                let mut ctx =
                    StimulusCtx::new(event.due_cycle, &self.scene, &mut scheduled, event.stimulus);
                stimulus.on_event(event.payload, &mut ctx);
            }
            self.scheduler.extend(scheduled);
        }
        Ok(())
    }

    /// Resolve every external driver and apply it to the pads.
    fn apply_drives(&mut self) -> Result<(), HostError> {
        let mut resolved: Vec<(Pin, Drive)> = Vec::new();
        for stimulus in &self.stimuli {
            for &pin in stimulus.drives() {
                let drive = stimulus.drive(pin);
                if drive == Drive::HighZ {
                    continue;
                }
                match resolved.iter_mut().find(|(existing, _)| *existing == pin) {
                    Some((_, current)) => {
                        let combined =
                            current.combine(drive).map_err(|_| HostError::PinConflict {
                                pin,
                                a: *current,
                                b: drive,
                            })?;
                        *current = combined;
                    }
                    None => resolved.push((pin, drive)),
                }
            }
        }
        for (pin, external) in resolved {
            self.apply_pin(pin, external)?;
        }
        Ok(())
    }

    /// Resolve one external driver against the AVR and write the pad.
    fn apply_pin(&mut self, pin: Pin, external: Drive) -> Result<(), HostError> {
        let (port, bit) = pin.port_bit();
        let handle = self.port_handle(port);
        let state = self
            .board
            .pin_state(self.backend.as_ref(), &mut self.runtime, handle, bit);
        let avr = match state {
            PinState::Low => Drive::Low,
            PinState::High => Drive::High,
            PinState::Input => Drive::HighZ,
            PinState::InputPullUp => Drive::PullUp,
        };
        let settled = avr.combine(external).map_err(|_| HostError::PinConflict {
            pin,
            a: avr,
            b: external,
        })?;
        // A driven AVR output already reaches PINx through the port register, so
        // only a pad the AVR leaves as an input needs the external level.
        if avr.is_strong() {
            return Ok(());
        }
        if let Some(high) = settled.level() {
            self.board
                .set_input(self.backend.as_ref(), &mut self.runtime, handle, bit, high);
        }
        Ok(())
    }
}
