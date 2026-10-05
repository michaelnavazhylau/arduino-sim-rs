// SPDX-License-Identifier: MIT

//! Simulator side of the three-LED indicator demo.
//!
//! Three indicator LEDs, one 330 Ω resistor each, on `D9`, `D10` and `D11`. The
//! circuit is a **netlist** and the pins are coupled through `breadboard`'s
//! `AnalogCoupling`, so the current behind each LED is *solved* rather than
//! inferred from a GPIO boolean. Nothing here knows about rendering, so the
//! electrical result is testable without a display.
//!
//! The interesting result is that **equal resistors give unequal currents**. The
//! forward voltages of red, green and blue differ by about a volt, so the same
//! 330 Ω part is a different operating point on each colour — which is exactly
//! why real designs often pick a different resistor per colour rather than
//! reusing one value.

use breadboard::netlist::{resistor, Netlist, Parameters, PlacedPart};
use breadboard::{AnalogCoupling, BreadboardHost, CouplingOutcome, Pin, Scene, Wiring};
use circuit_components::{Led, LedParameters};

/// Firmware built from `sketches/leds/leds.ino` for `arduino:avr:uno`.
const FIRMWARE: &str = include_str!("../firmware/leds.hex");

/// The one resistor value every channel uses.
pub const SERIES_OHMS: f64 = 330.0;

/// How long one firmware cycle takes, in milliseconds.
pub const CYCLE_MS: f64 = 3_100.0;

/// One LED channel: a resistor, an LED, and the pin that drives them.
pub struct Channel {
    /// Colour name, as shown on screen.
    pub name: &'static str,
    /// Netlist reference designator of the series resistor.
    pub resistor: &'static str,
    /// Netlist reference designator of the LED.
    pub led: &'static str,
    /// Net the pin drives, whose voltage is the driver's output.
    pub pin_net: &'static str,
    /// Arduino pin index.
    pub pin: u8,
    /// The model, so brightness uses the library's own mapping rather than a
    /// second copy of the normalisation.
    model: Led,
}

/// The three channels, in the order the firmware sequences them.
pub fn channels() -> [Channel; 3] {
    let preset = |name, resistor, led, pin_net, pin, parameters| Channel {
        name,
        resistor,
        led,
        pin_net,
        pin,
        model: Led::new(parameters).expect("documented LED presets are valid"),
    };
    [
        preset("red", "R1", "D1", "p_red", 9, LedParameters::red()),
        preset("green", "R2", "D2", "p_green", 10, LedParameters::green()),
        preset("blue", "R3", "D3", "p_blue", 11, LedParameters::blue()),
    ]
}

fn coloured_led(reference: &str, part: &str, anode_net: &str) -> PlacedPart {
    PlacedPart::new(reference, part, Parameters::new())
        .terminal("a", anode_net)
        .terminal("k", "gnd")
}

/// What one channel is doing at the current operating point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelReading {
    /// Signed forward voltage, anode -> cathode, in volts.
    pub voltage: f64,
    /// Signed forward current, anode -> cathode, in amperes.
    pub current: f64,
    /// Voltage the driver presents to the series resistor, in volts.
    pub pin_voltage: f64,
    /// Rendering brightness, from the LED model's own current mapping.
    pub brightness: f32,
}

impl ChannelReading {
    /// True when the LED is carrying meaningful forward current.
    pub fn is_lit(&self) -> bool {
        self.current > 1e-3
    }

    /// Voltage across the series resistor: driver output minus forward voltage.
    #[cfg(test)]
    fn resistor_voltage(&self) -> f64 {
        self.pin_voltage - self.voltage
    }

    /// Current the resistor's own Ohm's law predicts, for a consistency check.
    #[cfg(test)]
    fn predicted_current(&self) -> f64 {
        self.resistor_voltage() / SERIES_OHMS
    }
}

/// The three-LED demo's board and circuit.
pub struct LedsSim {
    host: BreadboardHost,
    channels: [Channel; 3],
}

impl LedsSim {
    /// Boot the firmware and wire the three channels to `D9`, `D10` and `D11`.
    pub fn boot() -> Self {
        let netlist = Netlist::new()
            .nets(["p_red", "p_green", "p_blue", "a_red", "a_green", "a_blue"])
            .part(resistor("R1", SERIES_OHMS, "p_red", "a_red"))
            .part(coloured_led("D1", "led.red", "a_red"))
            .part(resistor("R2", SERIES_OHMS, "p_green", "a_green"))
            .part(coloured_led("D2", "led.green", "a_green"))
            .part(resistor("R3", SERIES_OHMS, "p_blue", "a_blue"))
            .part(coloured_led("D3", "led.blue", "a_blue"));
        let wiring = Wiring::new()
            .bind(Pin::digital(9), "p_red")
            .bind(Pin::digital(10), "p_green")
            .bind(Pin::digital(11), "p_blue");

        let mut host = BreadboardHost::from_hex(FIRMWARE, Scene::empty(), Vec::new())
            .expect("leds firmware boots");
        let coupling = AnalogCoupling::new(netlist, wiring).expect("led netlist is valid");
        host.attach_analog(coupling).expect("pins are free");

        Self {
            host,
            channels: channels(),
        }
    }

    /// Advance simulated time.
    pub fn advance_micros(&mut self, micros: f64) {
        self.host
            .advance_micros(micros)
            .expect("three-LED demo stays solvable");
    }

    /// The three channels, in firmware order.
    pub fn channels(&self) -> &[Channel; 3] {
        &self.channels
    }

    /// The current operating point of one channel.
    pub fn reading(&self, index: usize) -> ChannelReading {
        let channel = &self.channels[index];
        let reading = self
            .host
            .analog()
            .expect("analog coupling is attached")
            .reading();
        let point = reading.branch(channel.led);
        let voltage = point.map_or(0.0, |point| point.voltage);
        let current = point.map_or(0.0, |point| point.current);
        ChannelReading {
            voltage,
            current,
            pin_voltage: reading.net_voltage(channel.pin_net).unwrap_or(0.0),
            brightness: channel.model.brightness(current) as f32,
        }
    }

    /// Every channel's operating point, in firmware order.
    pub fn readings(&self) -> [ChannelReading; 3] {
        [self.reading(0), self.reading(1), self.reading(2)]
    }

    /// Whether the last solve succeeded, or why it did not.
    pub fn outcome(&self) -> CouplingOutcome {
        self.host
            .analog()
            .expect("analog coupling is attached")
            .reading()
            .outcome()
    }

    /// Solves performed so far; a cached frame does not count.
    pub fn solves(&self) -> u64 {
        self.host
            .analog()
            .expect("analog coupling is attached")
            .solves()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use analog_solver::Device;

    /// Run the firmware to `millis` of simulated time.
    fn at(millis: f64) -> LedsSim {
        let mut sim = LedsSim::boot();
        sim.advance_micros(millis * 1000.0);
        sim
    }

    /// Independent forward voltage at `current`, by bisection on the model, so
    /// the test does not restate the solver's arithmetic.
    fn forward_voltage(model: &Led, current: f64) -> f64 {
        let (mut lo, mut hi) = (0.0_f64, 5.0_f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if model.evaluate(mid).expect("in range").current < current {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    /// A moment inside the firmware's "all three together" phase.
    const TOGETHER_MS: f64 = 2_000.0;

    #[test]
    fn equal_resistors_are_a_different_operating_point_on_each_colour() {
        let sim = at(TOGETHER_MS);
        let [red, green, blue] = sim.readings();
        assert!(red.is_lit() && green.is_lit() && blue.is_lit());

        // Forward voltage rises from red through blue, by roughly a volt.
        assert!(
            red.voltage < green.voltage && green.voltage < blue.voltage,
            "Vf: red {:.3}, green {:.3}, blue {:.3}",
            red.voltage,
            green.voltage,
            blue.voltage
        );
        assert!(
            blue.voltage - red.voltage > 0.5,
            "red and blue should differ substantially"
        );

        // The same resistor therefore passes less current on the higher-Vf part.
        assert!(
            red.current > green.current && green.current > blue.current,
            "I: red {:.2} mA, green {:.2} mA, blue {:.2} mA",
            red.current * 1e3,
            green.current * 1e3,
            blue.current * 1e3
        );
    }

    #[test]
    fn each_operating_point_satisfies_ohms_law_across_its_own_resistor() {
        let sim = at(TOGETHER_MS);
        for (channel, reading) in sim.channels().iter().zip(sim.readings()) {
            // The driver is a 5 V source behind 25 ohm, so the pin sits below
            // 5 V once it is loaded; the resistor sees the difference.
            assert!(
                reading.pin_voltage > reading.voltage,
                "{}: pin {:.3} V must exceed Vf {:.3} V",
                channel.name,
                reading.pin_voltage,
                reading.voltage
            );
            assert!(
                (reading.current - reading.predicted_current()).abs() < 1e-6,
                "{}: solved {:.6} mA vs Ohm's law {:.6} mA",
                channel.name,
                reading.current * 1e3,
                reading.predicted_current() * 1e3
            );
            // And the branch voltage really is the model's Vf at that current.
            let model = Led::new(match channel.name {
                "red" => LedParameters::red(),
                "green" => LedParameters::green(),
                _ => LedParameters::blue(),
            })
            .expect("preset is valid");
            let expected = forward_voltage(&model, reading.current);
            assert!(
                (reading.voltage - expected).abs() < 1e-6,
                "{}: solved Vf {:.6} V vs independent bisection {:.6} V",
                channel.name,
                reading.voltage,
                expected
            );
        }
    }

    #[test]
    fn the_firmware_lights_one_led_then_all_three_then_none() {
        // The sketch sequences red, green, blue for 350 ms each with a 150 ms
        // gap, then holds all three on for 1200 ms.
        let mut sim = LedsSim::boot();
        let mut lit = Vec::new();
        // The last sample is one full cycle plus 200 ms, so it must land back in
        // the first red phase: that is what pins `CYCLE_MS` to the firmware.
        for millis in [
            200.0,
            650.0,
            1_100.0,
            1_600.0,
            2_500.0,
            2_900.0,
            CYCLE_MS + 200.0,
        ] {
            while sim.millis() < millis {
                sim.advance_micros(2_000.0);
            }
            let names: Vec<&str> = sim
                .channels()
                .iter()
                .zip(sim.readings())
                .filter(|(_, reading)| reading.is_lit())
                .map(|(channel, _)| channel.name)
                .collect();
            lit.push(names);
        }
        assert_eq!(
            lit,
            vec![
                vec!["red"],
                vec!["green"],
                vec!["blue"],
                vec!["red", "green", "blue"],
                vec!["red", "green", "blue"],
                vec![],      // the trailing dark phase
                vec!["red"], // and the cycle repeats
            ]
        );
    }

    #[test]
    fn brightness_follows_the_solved_current_rather_than_the_pin_state() {
        let sim = at(TOGETHER_MS);
        let [red, green, blue] = sim.readings();
        // Brighter means more current, and all three are visibly on.
        assert!(red.brightness > green.brightness);
        assert!(green.brightness > blue.brightness);
        for reading in [red, green, blue] {
            assert!(reading.brightness > 0.3, "{reading:?}");
            assert!(reading.brightness <= 1.0);
        }
    }

    #[test]
    fn the_solve_reports_success_rather_than_a_silent_zero() {
        let sim = at(TOGETHER_MS);
        assert_eq!(sim.outcome(), CouplingOutcome::Solved);
        assert!(sim.solves() > 0);
    }
}
