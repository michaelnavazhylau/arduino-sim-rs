// SPDX-License-Identifier: MIT

//! HC-SR04 ultrasonic distance sensor.
//!
//! The HC-SR04 is a digital, time-coded sensor. A TRIG pulse of at least 10 us
//! asks for a measurement; the module emits a burst and then drives ECHO high
//! for the acoustic round-trip time. Holding ECHO low forever would be wrong,
//! and so would returning a distance without the pulse: firmware measures the
//! pulse, not a number.
//!
//! Timing values are illustrative module behaviour, not a fitted datasheet
//! characterization. There is no beam angle, no multi-path reflection, no
//! cross-talk between two sensors, and no acoustic dead zone beyond
//! [`UltrasonicParameters::min_range_m`]. Out-of-range and undetectable targets
//! both read as *no echo*: ECHO stays low, rather than inventing a timeout pulse
//! whose width varies between module revisions.

use crate::pin::{Drive, Pin};
use crate::scene::Reflector;
use crate::stimulus::{InputChange, Stimulus, StimulusCtx};
use crate::time::{micros_to_cycles, speed_of_sound_m_per_s};
use avr_port_tests::board::UNO_CLOCK_HZ;
use std::any::Any;
use std::error::Error;
use std::fmt;

const EVENT_ECHO_START: u64 = 1;
const EVENT_ECHO_END: u64 = 2;

/// Pack an event kind and generation into one payload.
///
/// The generation makes a stale event harmless: a new TRIG pulse supersedes an
/// in-flight measurement, and the old ECHO callbacks then find a mismatched
/// generation and return without touching the pin. That is cheaper and more
/// deterministic than removing events from the queue.
const fn pack(kind: u64, generation: u32) -> u64 {
    (kind << 32) | generation as u64
}

const fn kind_of(payload: u64) -> u64 {
    payload >> 32
}

const fn generation_of(payload: u64) -> u32 {
    payload as u32
}

/// Timing and range settings for an [`HcSr04`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UltrasonicParameters {
    /// Trigger input, driven by the MCU.
    pub trig: Pin,
    /// Echo output, driven by the sensor.
    pub echo: Pin,
    /// Minimum TRIG high time that starts a measurement (10 us by datasheet).
    pub min_trigger_cycles: u64,
    /// Delay from the TRIG falling edge to the ECHO rising edge, covering the
    /// module's burst and internal processing.
    pub response_delay_cycles: u64,
    /// Closest distance the module reports.
    pub min_range_m: f64,
    /// Furthest distance the module reports.
    pub max_range_m: f64,
    /// Air temperature used for the speed of sound.
    pub temperature_celsius: f64,
    /// Reflectors below this reflectivity return no detectable echo.
    pub min_reflectivity: f64,
}

impl Default for UltrasonicParameters {
    fn default() -> Self {
        Self {
            trig: Pin::digital(9),
            echo: Pin::digital(10),
            min_trigger_cycles: micros_to_cycles(10.0),
            response_delay_cycles: micros_to_cycles(460.0),
            min_range_m: 0.02,
            max_range_m: 4.0,
            temperature_celsius: 20.0,
            min_reflectivity: 0.05,
        }
    }
}

/// A completed measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// Distance to the reflector that produced the echo.
    pub distance_m: f64,
    /// ECHO high time the model produced, in AVR cycles.
    pub echo_cycles: u64,
    /// Cycle at which the ECHO falling edge was due.
    pub completed_cycle: u64,
}

/// Why an [`HcSr04`] could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UltrasonicError {
    /// TRIG and ECHO must be different pins.
    SamePin,
    /// Ranges must be finite, positive and ordered.
    InvalidRange,
    /// Temperature must be finite and physically plausible.
    InvalidTemperature,
    /// Reflectivity threshold must be within `[0, 1]`.
    InvalidReflectivity,
}

impl fmt::Display for UltrasonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SamePin => write!(f, "HC-SR04: TRIG and ECHO must be different pins"),
            Self::InvalidRange => {
                write!(f, "HC-SR04: range must be finite, positive and ordered")
            }
            Self::InvalidTemperature => {
                write!(f, "HC-SR04: temperature must be finite and plausible")
            }
            Self::InvalidReflectivity => {
                write!(f, "HC-SR04: reflectivity threshold must be within [0, 1]")
            }
        }
    }
}

impl Error for UltrasonicError {}

/// An HC-SR04 module attached to two pins.
#[derive(Debug)]
pub struct HcSr04 {
    parameters: UltrasonicParameters,
    inputs: [Pin; 1],
    outputs: [Pin; 1],
    trigger_high_since: Option<u64>,
    generation: u32,
    echo_high: bool,
    pending: Option<(f64, u64)>,
    last: Option<Measurement>,
    rejected_triggers: u64,
    no_echo_measurements: u64,
}

impl HcSr04 {
    /// Build a sensor, validating its parameters.
    pub fn new(parameters: UltrasonicParameters) -> Result<Self, UltrasonicError> {
        if parameters.trig == parameters.echo {
            return Err(UltrasonicError::SamePin);
        }
        if !parameters.min_range_m.is_finite()
            || !parameters.max_range_m.is_finite()
            || parameters.min_range_m <= 0.0
            || parameters.max_range_m <= parameters.min_range_m
        {
            return Err(UltrasonicError::InvalidRange);
        }
        if !parameters.temperature_celsius.is_finite()
            || !(-60.0..=125.0).contains(&parameters.temperature_celsius)
        {
            return Err(UltrasonicError::InvalidTemperature);
        }
        if !parameters.min_reflectivity.is_finite()
            || !(0.0..=1.0).contains(&parameters.min_reflectivity)
        {
            return Err(UltrasonicError::InvalidReflectivity);
        }
        Ok(Self {
            inputs: [parameters.trig],
            outputs: [parameters.echo],
            parameters,
            trigger_high_since: None,
            generation: 0,
            echo_high: false,
            pending: None,
            last: None,
            rejected_triggers: 0,
            no_echo_measurements: 0,
        })
    }

    /// An HC-SR04 with default pins `D9`/`D10`.
    pub fn with_defaults() -> Self {
        Self::new(UltrasonicParameters::default()).expect("valid default HC-SR04 parameters")
    }

    /// The configured parameters.
    pub fn parameters(&self) -> UltrasonicParameters {
        self.parameters
    }

    /// The most recent completed measurement, or `None` when the last
    /// measurement produced no echo.
    pub fn last_measurement(&self) -> Option<Measurement> {
        self.last
    }

    /// True while ECHO is driven high.
    pub fn echo_high(&self) -> bool {
        self.echo_high
    }

    /// TRIG pulses dropped for being shorter than the minimum width.
    pub fn rejected_triggers(&self) -> u64 {
        self.rejected_triggers
    }

    /// Measurements that produced no echo, including out-of-range targets.
    pub fn no_echo_measurements(&self) -> u64 {
        self.no_echo_measurements
    }

    /// Acoustic round-trip time for `distance_m`, in AVR cycles.
    ///
    /// This is the ECHO pulse width the model will produce, so a test can
    /// compare against an independent expectation instead of restating the
    /// implementation.
    pub fn round_trip_cycles(&self, distance_m: f64) -> u64 {
        let speed = speed_of_sound_m_per_s(self.parameters.temperature_celsius);
        let seconds = 2.0 * distance_m / speed;
        (seconds * UNO_CLOCK_HZ).round().max(0.0) as u64
    }

    fn begin_measurement(&mut self, ctx: &mut StimulusCtx<'_>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        ctx.schedule_after(
            self.parameters.response_delay_cycles,
            pack(EVENT_ECHO_START, generation),
        );
    }

    /// Answer the reflector, or leave ECHO low when nothing is detectable.
    fn start_echo(&mut self, reflector: Option<Reflector>, ctx: &mut StimulusCtx<'_>) {
        let Some(reflector) = reflector else {
            self.no_echo();
            return;
        };
        let distance_m = reflector.distance_m();
        if distance_m < self.parameters.min_range_m || distance_m > self.parameters.max_range_m {
            self.no_echo();
            return;
        }
        let cycles = self.round_trip_cycles(distance_m);
        self.echo_high = true;
        self.pending = Some((distance_m, cycles));
        let generation = self.generation;
        ctx.schedule_after(cycles, pack(EVENT_ECHO_END, generation));
    }

    fn no_echo(&mut self) {
        self.echo_high = false;
        self.pending = None;
        self.last = None;
        self.no_echo_measurements = self.no_echo_measurements.wrapping_add(1);
    }
}

impl Stimulus for HcSr04 {
    fn name(&self) -> &'static str {
        "HC-SR04"
    }

    fn observes(&self) -> &[Pin] {
        &self.inputs
    }

    fn drives(&self) -> &[Pin] {
        &self.outputs
    }

    fn drive(&self, pin: Pin) -> Drive {
        if pin != self.parameters.echo {
            return Drive::HighZ;
        }
        // ECHO is actively driven low when idle, not left floating.
        if self.echo_high {
            Drive::High
        } else {
            Drive::Low
        }
    }

    fn on_input(&mut self, change: InputChange, ctx: &mut StimulusCtx<'_>) {
        if change.pin != self.parameters.trig {
            return;
        }
        if change.high {
            self.trigger_high_since = Some(change.observed_cycle);
            return;
        }
        let Some(rising) = self.trigger_high_since.take() else {
            return;
        };
        let width = change.observed_cycle.saturating_sub(rising);
        if width < self.parameters.min_trigger_cycles {
            self.rejected_triggers = self.rejected_triggers.wrapping_add(1);
            return;
        }
        self.begin_measurement(ctx);
    }

    fn on_event(&mut self, payload: u64, ctx: &mut StimulusCtx<'_>) {
        if generation_of(payload) != self.generation {
            return; // Superseded by a newer trigger pulse.
        }
        match kind_of(payload) {
            EVENT_ECHO_START => {
                let reflector = ctx.scene().nearest_echo(self.parameters.min_reflectivity);
                self.start_echo(reflector, ctx);
            }
            EVENT_ECHO_END => {
                self.echo_high = false;
                if let Some((distance_m, echo_cycles)) = self.pending.take() {
                    self.last = Some(Measurement {
                        distance_m,
                        echo_cycles,
                        completed_cycle: ctx.now,
                    });
                }
            }
            _ => {}
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters() -> UltrasonicParameters {
        UltrasonicParameters::default()
    }

    #[test]
    fn invalid_parameters_are_rejected_rather_than_clamped() {
        assert_eq!(
            HcSr04::new(UltrasonicParameters {
                echo: Pin::digital(9),
                ..parameters()
            })
            .unwrap_err(),
            UltrasonicError::SamePin
        );
        assert_eq!(
            HcSr04::new(UltrasonicParameters {
                min_range_m: 0.0,
                ..parameters()
            })
            .unwrap_err(),
            UltrasonicError::InvalidRange
        );
        assert_eq!(
            HcSr04::new(UltrasonicParameters {
                max_range_m: 0.01,
                ..parameters()
            })
            .unwrap_err(),
            UltrasonicError::InvalidRange
        );
        assert_eq!(
            HcSr04::new(UltrasonicParameters {
                temperature_celsius: f64::NAN,
                ..parameters()
            })
            .unwrap_err(),
            UltrasonicError::InvalidTemperature
        );
        assert_eq!(
            HcSr04::new(UltrasonicParameters {
                min_reflectivity: 1.5,
                ..parameters()
            })
            .unwrap_err(),
            UltrasonicError::InvalidReflectivity
        );
    }

    #[test]
    fn round_trip_time_grows_with_distance_and_falls_with_temperature() {
        let sensor = HcSr04::with_defaults();
        assert!(sensor.round_trip_cycles(0.4) > sensor.round_trip_cycles(0.2));
        // Twice the distance is twice the flight time, within rounding.
        let near = sensor.round_trip_cycles(0.2);
        let far = sensor.round_trip_cycles(0.4);
        assert!(far >= near * 2 - 2 && far <= near * 2 + 2);

        let warm = HcSr04::new(UltrasonicParameters {
            temperature_celsius: 40.0,
            ..parameters()
        })
        .unwrap();
        assert!(warm.round_trip_cycles(0.5) < sensor.round_trip_cycles(0.5));
    }

    #[test]
    fn an_idle_sensor_drives_echo_low_and_leaves_trigger_floating() {
        let sensor = HcSr04::with_defaults();
        assert_eq!(sensor.drive(Pin::digital(10)), Drive::Low);
        assert_eq!(sensor.drive(Pin::digital(9)), Drive::HighZ);
        assert_eq!(sensor.observes(), &[Pin::digital(9)][..]);
        assert_eq!(sensor.drives(), &[Pin::digital(10)][..]);
        assert_eq!(sensor.name(), "HC-SR04");
    }
}
