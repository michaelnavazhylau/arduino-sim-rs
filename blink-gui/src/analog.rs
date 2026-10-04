// SPDX-License-Identifier: MIT

//! Electrical coupling for the fixed blink netlist. Rebuild/solve only when
//! GPIO drive mode or LED polarity changes, never inside rendering code.
//! All AVR-specific knowledge is here, not in either electrical library.

use analog_solver::{BranchPoint, Circuit, Node, SolveError, SolveOptions};
use avr_port_tests::board::PinState;
use circuit_components::{Led, Resistor};

pub const SUPPLY_VOLTS: f64 = 5.0;
pub const SERIES_OHMS: f64 = 220.0;
/// Illustrative Thevenin driver model, not an ATmega pad characterization.
pub const OUTPUT_OHMS: f64 = 25.0;
pub const PULLUP_OHMS: f64 = 30_000.0;

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub pin_voltage: f64,
    pub resistor: BranchPoint,
    /// Voltage/current oriented anode -> cathode, including when reversed.
    pub led: BranchPoint,
    pub brightness: f64,
    pub driver_current: f64,
    pub driver_power: f64,
    pub source_power: f64,
    pub max_kcl_residual: f64,
    /// AVR input thresholds: <=0.3 Vcc low, >=0.6 Vcc high, otherwise undefined.
    pub digital_input: Option<bool>,
}

pub struct BlinkCircuit {
    state: PinState,
    reversed: bool,
    pub reading: Result<Reading, SolveError>,
    pub solves: u64,
}
impl BlinkCircuit {
    pub fn new(reversed: bool) -> Self {
        Self {
            state: PinState::Input,
            reversed,
            reading: solve(PinState::Input, reversed),
            solves: 1,
        }
    }
    pub fn update(&mut self, state: PinState, reversed: bool) {
        if self.state == state && self.reversed == reversed {
            return;
        }
        self.state = state;
        self.reversed = reversed;
        self.reading = solve(state, reversed);
        self.solves += 1;
    }
    pub fn reversed(&self) -> bool {
        self.reversed
    }
    pub fn state(&self) -> PinState {
        self.state
    }
    pub fn brightness(&self) -> f32 {
        self.reading
            .as_ref()
            .map_or(0.0, |reading| reading.brightness as f32)
    }
}

fn solve(state: PinState, reversed: bool) -> Result<Reading, SolveError> {
    let mut c = Circuit::new();
    let pin = c.node();
    let signal = c.node();
    let driver = match state {
        PinState::Input => None, // Disconnected source, NOT a large fake resistor.
        state => {
            let rail = c.node();
            let volts = if state == PinState::Low {
                0.0
            } else {
                SUPPLY_VOLTS
            };
            let ohms = if state == PinState::InputPullUp {
                PULLUP_OHMS
            } else {
                OUTPUT_OHMS
            };
            let source = c.voltage_source(rail, Node::GROUND, volts)?;
            let resistance = c.device(
                rail,
                pin,
                Resistor::new(ohms).expect("valid driver resistance"),
            )?;
            Some((source, resistance))
        }
    };
    let resistor = c.device(
        pin,
        signal,
        Resistor::new(SERIES_OHMS).expect("valid series resistance"),
    )?;
    let (anode, cathode) = if reversed {
        (Node::GROUND, signal)
    } else {
        (signal, Node::GROUND)
    };
    let led = Led::red();
    let diode = c.device(anode, cathode, led)?;
    // Tight absolute tolerance resolves the pA reverse leakage, not just mA loads.
    let options = SolveOptions {
        current_tolerance: 1e-15,
        ..SolveOptions::default()
    };
    let s = c.solve(options)?;
    // All IDs below were allocated above in this exact circuit.
    let pin_voltage = s.voltage(pin).unwrap();
    let led_point = s.branch(diode).unwrap();
    let (driver_current, driver_power, source_power) =
        driver.map_or((0.0, 0.0, 0.0), |(source, resistance)| {
            let r = s.branch(resistance).unwrap();
            (r.current, r.power(), s.branch(source).unwrap().power())
        });
    Ok(Reading {
        pin_voltage,
        resistor: s.branch(resistor).unwrap(),
        led: led_point,
        brightness: led.brightness(led_point.current),
        driver_current,
        driver_power,
        source_power,
        max_kcl_residual: s.max_kcl_residual,
        digital_input: if pin_voltage <= 0.3 * SUPPLY_VOLTS {
            Some(false)
        } else if pin_voltage >= 0.6 * SUPPLY_VOLTS {
            Some(true)
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_driver_load_sets_real_pin_voltage_and_preserves_power_balance() {
        let r = solve(PinState::High, false).unwrap();
        assert!((0.011..0.012).contains(&r.led.current));
        assert!((2.14..2.16).contains(&r.led.voltage));
        assert!((4.70..4.72).contains(&r.pin_voltage));
        assert!((r.pin_voltage - (SUPPLY_VOLTS - OUTPUT_OHMS * r.driver_current)).abs() < 1e-8);
        assert!((r.resistor.current - r.led.current).abs() < 1e-9);
        assert!(
            (r.source_power + r.driver_power + r.resistor.power() + r.led.power()).abs() < 1e-8
        );
        assert!(r.max_kcl_residual < 1e-9);
    }
    #[test]
    fn reverse_led_blocks_current_despite_a_high_avr_output() {
        let r = solve(PinState::High, true).unwrap();
        assert!((r.pin_voltage - 5.0).abs() < 1e-8);
        assert!((r.led.voltage + 5.0).abs() < 1e-8);
        assert!(r.led.current < 0.0 && r.led.current > -6e-12);
        assert_eq!(r.brightness, 0.0);
    }
    #[test]
    fn low_high_impedance_and_pullup_have_distinct_electrical_behavior() {
        for reversed in [false, true] {
            for state in [PinState::Low, PinState::Input] {
                let r = solve(state, reversed).unwrap();
                assert!(r.led.current.abs() < 1e-15);
                assert_eq!(r.brightness, 0.0);
                assert_eq!(r.digital_input, Some(false));
            }
        }
        let r = solve(PinState::InputPullUp, false).unwrap();
        assert!((0.00009..0.00012).contains(&r.led.current));
        assert!(r.brightness > 0.0 && r.brightness < 0.01);
        assert_eq!(r.digital_input, None); // LED-loaded input is between thresholds.
        let r = solve(PinState::InputPullUp, true).unwrap();
        assert_eq!(r.digital_input, Some(true));
        assert_eq!(r.brightness, 0.0);
    }
    #[test]
    fn unchanged_drive_state_is_cached_but_reversal_is_resolved() {
        let mut c = BlinkCircuit::new(false);
        c.update(PinState::Input, false);
        assert_eq!(c.solves, 1);
        c.update(PinState::High, false);
        assert_eq!(c.solves, 2);
        c.update(PinState::High, true);
        assert_eq!(c.solves, 3);
        assert_eq!(c.brightness(), 0.0);
    }
}
