// SPDX-License-Identifier: MIT

//! Simulator side of the three-LED indicator demo.
//!
//! Three indicator LEDs, one 330 Ω resistor each, on `D9`, `D10` and `D11`. The
//! circuit is a **SPICE deck** and the pins are coupled through `breadboard`'s
//! `AnalogCoupling`, so the current behind each LED is *solved* rather than
//! inferred from a GPIO boolean. Nothing here knows about rendering, so the
//! electrical result is testable without a display.
//!
//! The interesting result is that **equal resistors give unequal currents**. The
//! forward voltages of red, green and blue differ by about a volt, so the same
//! 330 Ω part is a different operating point on each colour — which is exactly
//! why real designs often pick a different resistor per colour rather than
//! reusing one value.

use breadboard::{AnalogCoupling, BreadboardHost, CouplingOutcome, LedPreset, Pin, Scene, Wiring};
use std::fmt::Write as _;

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
    /// Net between the series resistor and the LED anode.
    pub anode_net: &'static str,
    /// Arduino pin index.
    pub pin: u8,
    /// The model the deck is built from, so brightness uses the same mapping
    /// rather than a second copy of the normalization.
    preset: LedPreset,
}

/// The three channels, in the order the firmware sequences them.
pub fn channels() -> [Channel; 3] {
    let preset = |name, resistor, led, pin_net, anode_net, pin, preset| Channel {
        name,
        resistor,
        led,
        pin_net,
        anode_net,
        pin,
        preset,
    };
    [
        preset("red", "R1", "D1", "p_red", "a_red", 9, LedPreset::red()),
        preset(
            "green",
            "R2",
            "D2",
            "p_green",
            "a_green",
            10,
            LedPreset::green(),
        ),
        preset(
            "blue",
            "R3",
            "D3",
            "p_blue",
            "a_blue",
            11,
            LedPreset::blue(),
        ),
    ]
}

/// The three channels as one SPICE deck, without the MCU drivers.
fn deck() -> String {
    let mut deck = String::from("three indicator LEDs\n");
    for channel in channels() {
        let _ = writeln!(
            deck,
            "{} {} {} {SERIES_OHMS}",
            channel.resistor, channel.pin_net, channel.anode_net
        );
        deck.push_str(&channel.preset.cards(channel.led, channel.anode_net, "0"));
        deck.push('\n');
    }
    deck
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
        let wiring = Wiring::new()
            .bind(Pin::digital(9), "p_red")
            .bind(Pin::digital(10), "p_green")
            .bind(Pin::digital(11), "p_blue");

        let mut host = BreadboardHost::from_hex(FIRMWARE, Scene::empty(), Vec::new())
            .expect("leds firmware boots");
        let coupling = AnalogCoupling::new(deck(), wiring).expect("led deck and wiring are valid");
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
        let pin_voltage = reading.net_voltage(channel.pin_net).unwrap_or(0.0);
        let anode_voltage = reading.net_voltage(channel.anode_net).unwrap_or(0.0);
        // The series resistor is ideal, so its own Ohm's law *is* the LED current.
        let current = (pin_voltage - anode_voltage) / SERIES_OHMS;
        ChannelReading {
            voltage: anode_voltage,
            current,
            pin_voltage,
            brightness: channel.preset.brightness(current) as f32,
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
    use breadboard::spice::MODEL_THERMAL_VOLTS;

    /// Run the firmware to `millis` of simulated time.
    fn at(millis: f64) -> LedsSim {
        let mut sim = LedsSim::boot();
        sim.advance_micros(millis * 1000.0);
        sim
    }

    /// Independent forward voltage at `current`, by bisection on the diode
    /// equation the preset describes, so the test does not restate the
    /// simulator's arithmetic.
    fn forward_voltage(preset: &LedPreset, current: f64) -> f64 {
        let scale = preset.ideality_factor * MODEL_THERMAL_VOLTS;
        let diode = |voltage: f64| {
            preset.saturation_current * ((voltage / scale).exp() - 1.0)
                + voltage / preset.shunt_ohms
        };
        let (mut lo, mut hi) = (0.0_f64, 5.0_f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if diode(mid) < current {
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
    fn each_operating_point_satisfies_ohms_law_and_the_diode_curve() {
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
                (reading.current - reading.predicted_current()).abs() < 1e-9,
                "{}: solved {:.6} mA vs Ohm's law {:.6} mA",
                channel.name,
                reading.current * 1e3,
                reading.predicted_current() * 1e3
            );
            // And the branch voltage really is the model's Vf at that current.
            let expected = forward_voltage(&channel.preset, reading.current);
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
