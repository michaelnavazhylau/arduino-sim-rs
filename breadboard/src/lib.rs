// SPDX-License-Identifier: MIT

//! Headless host, simulated-time scheduler and external components.
//!
//! This crate is the adapter layer the [sim2real
//! roadmap](../../docs/sim2real-roadmap.md) calls for: it owns the MCU pin
//! drivers, the simulated-time event queue and the components wired to the
//! board, and it depends on the AVR core rather than the other way round.
//! `rust_port` stays dependency-free and the electrical layer stays
//! renderer-free.
//!
//! The central distinction in the electrical layer is **time**. A resistor or
//! LED is a memoryless current/voltage law and belongs in a SPICE deck; a sensor
//! is a stateful device whose output depends on when things happened. Sensors
//! implement [`Stimulus`], not an electrical device, so digital timing never has
//! to be squeezed into an operating-point solve.
//!
//! ```no_run
//! use breadboard::{BreadboardHost, HcSr04, Scene, Stimulus};
//!
//! let host = BreadboardHost::new(
//!     vec![0x00; 0x8000],
//!     Scene::wall(0.5).unwrap(),
//!     vec![Box::new(HcSr04::with_defaults()) as Box<dyn Stimulus>],
//! );
//! assert!(host.is_ok());
//! ```
//!
//! Current scope and honest limits:
//!
//! * Analog coupling is real but narrow. [`AnalogCoupling`] takes a **SPICE
//!   deck**, appends a Thevenin driver per bound pin, solves it with
//!   [`ngspice_rs`] through [`spice`], feeds the ADC mux channels and writes
//!   resolved input levels back to `PINx`. It is **DC only**: there is no
//!   transient integration, so no RC charging, no PWM low-pass averaging and no
//!   sample-and-hold behaviour.
//! * [`HcSr04`] is the only modelled sensor. `Bridge` carries generic I2C and
//!   SPI devices, but the shipped ones are register-file models rather than
//!   fitted parts.
//! * Circuits are attached in code as deck text. There is no schematic-to-model
//!   binding and no netlist file format.

pub mod analog;
pub mod bus;
pub mod host;
pub mod led;
pub mod pin;
pub mod scene;
pub mod scheduler;
pub mod spice;
pub mod stimulus;
pub mod time;
pub mod ultrasonic;

pub use analog::{
    AnalogCoupling, AnalogReading, CouplingError, CouplingOutcome, Wiring, ADC_CHANNELS,
};
pub use bus::{
    Bridge, BridgeBackend, BusError, I2cSlave, RegisterMap, SpiRegisterMap, SpiSlave, SPCR, SPDR,
    SPSR, TWCR, TWDR, TWSR,
};
pub use host::{BreadboardHost, HostError};
pub use led::LedPreset;
pub use pin::{AnalogPin, DigitalPin, Drive, Pin, PinConflict, Port};
pub use scene::{Reflector, Scene, SceneError};
pub use scheduler::{ScheduledEvent, Scheduler, StimulusId};
pub use spice::{deck_index, solve_op, DeckIndex, OpError, Solution};
pub use stimulus::{InputChange, Stimulus, StimulusCtx};
pub use time::{cycles_to_micros, micros_to_cycles, speed_of_sound_m_per_s};
pub use ultrasonic::{HcSr04, Measurement, UltrasonicError, UltrasonicParameters};

/// Uno clock rate in Hz, re-exported so timing arithmetic uses one constant.
pub use avr_sim::board::UNO_CLOCK_HZ;
