// SPDX-License-Identifier: MIT

//! Simulator side of the ultrasonic demo: a real sketch, an HC-SR04, and the
//! firmware's own measurement read back from an I2C host sink.
//!
//! Nothing here knows about rendering. The firmware in
//! `firmware/ultrasonic.hex` pulses TRIG on D9, times ECHO on D10 with
//! `pulseIn`, and publishes the result over I2C because the simulated board has
//! no console attached. The host reads that sink, so the number on screen is
//! what the *firmware measured* — not what the sensor model happened to emit.
//!
//! This means the whole chain is exercised end to end and, crucially, testable
//! without a display: `cargo test` boots the firmware and asserts on real
//! measurements.

use breadboard::{BreadboardHost, HcSr04, RegisterMap, Scene, Stimulus};

/// Firmware built from `sketches/ultrasonic/ultrasonic.ino` for `arduino:avr:uno`.
const FIRMWARE: &str = include_str!("../firmware/ultrasonic.hex");

/// I2C address the sketch publishes its measurements to.
pub const HOST_SINK_ADDRESS: u8 = 0x68;

/// Closest and furthest reflector the demo allows, in metres.
pub const MIN_DISTANCE_M: f64 = 0.05;
/// Furthest reflector; beyond this the module reports no echo.
pub const MAX_DISTANCE_M: f64 = 4.0;
/// Distance the demo starts at.
pub const DEFAULT_DISTANCE_M: f64 = 0.6;

/// A measurement the firmware published, as read from the host sink.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Telemetry {
    /// 0 = ok, 1 = no echo inside the timeout, 2 = out of the module's range.
    pub status: u8,
    /// Wrapping counter the firmware increments once per measurement. A gap
    /// means the host sampled slower than the firmware measured.
    pub sequence: u8,
    /// Distance the **firmware** computed from its own `pulseIn` timing.
    pub distance_mm: u16,
    /// ECHO pulse width the firmware timed, in microseconds.
    pub echo_micros: u16,
}

impl Telemetry {
    /// The firmware's distance in metres, or `None` when it reported no echo.
    pub fn distance_m(&self) -> Option<f64> {
        (self.status == 0).then(|| f64::from(self.distance_mm) / 1000.0)
    }
}

/// The ultrasonic demo's board, sensor and reflector.
pub struct SensorSim {
    host: BreadboardHost,
    distance_m: f64,
    telemetry: Telemetry,
    last_sequence: Option<u8>,
    measurements: u64,
    missed: u64,
}

impl SensorSim {
    /// Boot the firmware and attach the sensor and the telemetry sink.
    pub fn boot() -> Self {
        Self::boot_at(DEFAULT_DISTANCE_M)
    }

    /// Boot with the reflector at `distance_m`, clamped to the demo's range.
    pub fn boot_at(distance_m: f64) -> Self {
        let distance_m = distance_m.clamp(MIN_DISTANCE_M, MAX_DISTANCE_M);
        let scene = Scene::wall(distance_m).expect("clamped distance is valid");
        let mut host = BreadboardHost::from_hex(
            FIRMWARE,
            scene,
            vec![Box::new(HcSr04::with_defaults()) as Box<dyn Stimulus>],
        )
        .expect("sensor firmware boots");
        // Attach before the first instruction: the sketch's `setup()` runs on
        // the first step and would otherwise transmit into nothing.
        host.attach_i2c(vec![Box::new(
            RegisterMap::new(HOST_SINK_ADDRESS, 8).expect("valid host sink"),
        )]);

        let mut sim = Self {
            host,
            distance_m,
            telemetry: Telemetry {
                status: 1,
                sequence: 0,
                distance_mm: 0,
                echo_micros: 0,
            },
            last_sequence: None,
            measurements: 0,
            missed: 0,
        };
        sim.poll_telemetry();
        sim
    }

    /// Advance simulated time and pick up any newly published measurement.
    pub fn advance_micros(&mut self, micros: f64) {
        self.host
            .advance_micros(micros)
            .expect("sensor demo stays solvable");
        self.poll_telemetry();
    }

    /// Move the reflector.
    ///
    /// The next measurement reflects the new position; the previous one stands
    /// until the firmware publishes again, which is exactly how a real module
    /// behaves.
    pub fn set_distance_m(&mut self, distance_m: f64) {
        self.distance_m = distance_m.clamp(MIN_DISTANCE_M, MAX_DISTANCE_M);
        self.host
            .set_scene(Scene::wall(self.distance_m).expect("clamped distance is valid"));
    }

    /// True distance of the reflector, in metres.
    pub fn distance_m(&self) -> f64 {
        self.distance_m
    }

    /// The most recent measurement the firmware published.
    pub fn telemetry(&self) -> Telemetry {
        self.telemetry
    }

    /// Measurements the host has observed.
    pub fn measurements(&self) -> u64 {
        self.measurements
    }

    /// Publications the host skipped because it sampled too slowly.
    pub fn missed_measurements(&self) -> u64 {
        self.missed
    }

    /// Simulated milliseconds elapsed.
    pub fn millis(&self) -> f64 {
        self.host.micros() / 1000.0
    }

    /// AVR cycles elapsed.
    pub fn cycles(&self) -> u64 {
        self.host.now()
    }

    /// AVR instructions retired.
    pub fn instructions(&self) -> u64 {
        self.host.instructions()
    }

    /// True while the sensor is holding ECHO high.
    pub fn echo_high(&self) -> bool {
        self.host.find::<HcSr04>().is_some_and(HcSr04::echo_high)
    }

    /// The sensor model's own last measurement, for comparison on screen.
    pub fn model_measurement_m(&self) -> Option<f64> {
        self.host
            .find::<HcSr04>()
            .and_then(HcSr04::last_measurement)
            .map(|measurement| measurement.distance_m)
    }

    /// True when the firmware has the proximity indicator lit (`D13`).
    pub fn proximity_led(&mut self) -> bool {
        self.host.led_builtin().is_high()
    }

    /// Read the telemetry sink and record a new publication, if any.
    fn poll_telemetry(&mut self) {
        let bridge = self.host.bridge();
        let Some(device) = bridge.i2c_device_as::<RegisterMap>(HOST_SINK_ADDRESS) else {
            return;
        };
        let sequence = device.register(1);
        if self.last_sequence == Some(sequence) {
            return;
        }
        if let Some(previous) = self.last_sequence {
            if sequence != previous.wrapping_add(1) {
                self.missed += 1;
            }
        }
        self.last_sequence = Some(sequence);
        self.measurements += 1;
        self.telemetry = Telemetry {
            status: device.register(0),
            sequence,
            distance_mm: u16::from_be_bytes([device.register(2), device.register(3)]),
            echo_micros: u16::from_be_bytes([device.register(4), device.register(5)]),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for a real `pulseIn` measurement: `micros()` has 4 us
    /// resolution on a 16 MHz ATmega328P, and the polling loop adds a little
    /// more, so a few millimetres of quantisation are expected rather than a bug.
    const TOLERANCE_M: f64 = 0.03;

    fn measure_at(distance_m: f64) -> SensorSim {
        let mut sim = SensorSim::boot_at(distance_m);
        sim.advance_micros(300_000.0);
        sim
    }

    #[test]
    fn the_firmware_measures_the_reflector_it_can_see() {
        for distance_m in [0.3, 0.6, 1.5] {
            let sim = measure_at(distance_m);
            let measured = sim
                .telemetry()
                .distance_m()
                .unwrap_or_else(|| panic!("no echo at {distance_m} m"));
            assert!(
                (measured - distance_m).abs() < TOLERANCE_M,
                "firmware measured {measured} m for a {distance_m} m reflector"
            );
            assert!(sim.measurements() > 0);
        }
    }

    #[test]
    fn the_published_pulse_width_agrees_with_the_firmwares_own_arithmetic() {
        let sim = measure_at(0.6);
        let telemetry = sim.telemetry();
        // The sketch computes mm = us * 343 / 2000, so its two published fields
        // must be consistent with each other, not just with the host's model.
        let recomputed = u32::from(telemetry.echo_micros) * 343 / 2000;
        assert_eq!(
            u32::from(telemetry.distance_mm),
            recomputed,
            "published distance and echo time disagree"
        );
    }

    #[test]
    fn moving_the_reflector_moves_the_measurement() {
        let mut sim = SensorSim::boot_at(0.4);
        sim.advance_micros(300_000.0);
        let near = sim.telemetry().distance_m().expect("echo");

        sim.set_distance_m(1.8);
        sim.advance_micros(300_000.0);
        let far = sim.telemetry().distance_m().expect("echo");

        assert!((near - 0.4).abs() < TOLERANCE_M, "near measured {near}");
        assert!((far - 1.8).abs() < TOLERANCE_M, "far measured {far}");
        assert!(
            sim.telemetry().sequence != 0,
            "the firmware counter should have advanced"
        );
    }

    #[test]
    fn something_out_of_range_reads_as_no_echo_rather_than_a_number() {
        let mut sim = SensorSim::boot_at(0.5);
        sim.host.set_scene(Scene::empty());
        sim.advance_micros(400_000.0);

        let telemetry = sim.telemetry();
        assert_eq!(telemetry.status, 1, "expected a timeout status");
        assert_eq!(telemetry.distance_m(), None);
    }

    #[test]
    fn the_firmware_lights_the_proximity_indicator_only_when_close() {
        // The sketch lights D13 below 200 mm.
        let mut near = SensorSim::boot_at(0.1);
        near.advance_micros(300_000.0);
        assert!(near.proximity_led(), "D13 should be lit at 100 mm");

        let mut far = SensorSim::boot_at(1.0);
        far.advance_micros(300_000.0);
        assert!(!far.proximity_led(), "D13 should be dark at 1000 mm");
    }

    #[test]
    fn the_model_and_the_firmware_agree_on_the_same_reflector() {
        let sim = measure_at(0.6);
        let model = sim.model_measurement_m().expect("model measured");
        let firmware = sim.telemetry().distance_m().expect("firmware measured");
        assert!(
            (model - firmware).abs() < TOLERANCE_M,
            "model {model} m vs firmware {firmware} m"
        );
    }

    #[test]
    fn the_host_observes_every_publication_it_samples_fast_enough_for() {
        // A measurement cycle is roughly 12 us trigger + echo + 60 ms delay, so
        // 30 ms of stepping per poll keeps up with the firmware.
        let mut sim = SensorSim::boot_at(0.5);
        for _ in 0..20 {
            sim.advance_micros(30_000.0);
        }
        assert!(sim.measurements() >= 8, "observed {}", sim.measurements());
        assert_eq!(sim.missed_measurements(), 0);
    }
}
