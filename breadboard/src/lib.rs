// SPDX-License-Identifier: MIT

//! Headless host, simulated-time scheduler and external components.
//!
//! This crate is the adapter layer the [sim2real
//! roadmap](../../docs/sim2real-roadmap.md) calls for: it owns the MCU pin
//! drivers, the simulated-time event queue and the components wired to the
//! board, and it depends on the AVR core rather than the other way round.
//! `rust_port` stays dependency-free and `analog-solver` stays renderer-free.
//!
//! The central distinction from `circuit-components` is **time**. A resistor
//! or LED is a memoryless current/voltage law; a sensor is a stateful device
//! whose output depends on when things happened. Sensors implement
//! [`Stimulus`], not `analog_solver::Device`, so digital timing never has to be
//! squeezed into an electrical solve.
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
//! * Analog coupling is real but narrow. [`AnalogCoupling`] solves a netlist
//!   with a Thevenin driver per bound pin, feeds the ADC mux channels and writes
//!   resolved input levels back to `PINx`. It is **DC only**: there is no
//!   transient integration, so no RC charging, no PWM low-pass averaging and no
//!   sample-and-hold behaviour.
//! * [`HcSr04`] is the only modelled sensor. `Bridge` carries generic I2C and
//!   SPI devices, but the shipped ones are register-file models rather than
//!   fitted parts.
//! * Components are attached in code. The netlist is data, but there is no
//!   schematic-to-model binding and no netlist file format yet.

pub mod analog;
pub mod bus;
pub mod host;
pub mod pin;
pub mod scene;
pub mod scheduler;
pub mod stimulus;
pub mod time;
pub mod ultrasonic;

pub use analog::{
    AnalogCoupling, AnalogReading, CouplingError, CouplingOutcome, McuDriverFactory, Wiring,
    ADC_CHANNELS,
};
pub use bus::{
    Bridge, BridgeBackend, BusError, I2cSlave, RegisterMap, SpiRegisterMap, SpiSlave, SPCR, SPDR,
    SPSR, TWCR, TWDR, TWSR,
};
pub use host::{BreadboardHost, HostError};
pub use pin::{AnalogPin, DigitalPin, Drive, Pin, PinConflict, Port};
pub use scene::{Reflector, Scene, SceneError};
pub use scheduler::{ScheduledEvent, Scheduler, StimulusId};
pub use stimulus::{InputChange, Stimulus, StimulusCtx};
pub use time::{cycles_to_micros, micros_to_cycles, speed_of_sound_m_per_s};
pub use ultrasonic::{HcSr04, Measurement, UltrasonicError, UltrasonicParameters};

/// Uno clock rate in Hz, re-exported so timing arithmetic uses one constant.
pub use avr_port_tests::board::UNO_CLOCK_HZ;

/// The declarative netlist format and part catalogue used by [`AnalogCoupling`].
pub use circuit_components::netlist;
