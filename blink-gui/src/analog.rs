// SPDX-License-Identifier: MIT

//! Electrical coupling for the fixed blink circuit, solved by ngspice-rs.
//!
//! The external LED is a SPICE deck whose driver depends on the AVR's D13 drive
//! mode: a source behind its output resistance when the pin is driven, a 30k
//! pull-up when it is high impedance with the pull-up enabled, and *nothing at
//! all* when it is truly floating, because approximating an undriven pin with a
//! huge resistor would make a floating input look like a measurement.
//!
//! The deck is solved by [`breadboard::solve_op`] and read back by net name, so
//! the LED is lit by *solved current*, not by `led_on`. Rebuild and solve happen
//! only when the drive mode or the LED polarity changes, never during rendering.

use avr8rs::board::PinState;
use breadboard::{solve_op, LedPreset, OpError};
use std::fmt::Write as _;

pub const SUPPLY_VOLTS: f64 = 5.0;
pub const SERIES_OHMS: f64 = 220.0;
/// Illustrative Thevenin driver model, not an ATmega pad characterization.
pub const OUTPUT_OHMS: f64 = 25.0;
pub const PULLUP_OHMS: f64 = 30_000.0;
/// Designator of the driver's voltage source, so its current can be read back.
pub const DRIVER_SOURCE: &str = "V_MCU_D9";
/// Forward current at full brightness for this demo's single LED.
///
/// This is a rendering normalization, not an electrical limit. It matches the
/// retired in-tree model's default, which is twice the three-LED demo's preset.
pub const NOMINAL_CURRENT: f64 = 0.020;

/// The diode curve and brightness mapping this demo's LED is drawn from.
fn preset() -> LedPreset {
    LedPreset::red().with_nominal_current(NOMINAL_CURRENT)
}

/// Voltage and current of one two-terminal branch, oriented terminal 1 -> 2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BranchPoint {
    pub voltage: f64,
    pub current: f64,
}

impl BranchPoint {
    /// Power absorbed by the branch, in watts.
    pub fn power(self) -> f64 {
        self.voltage * self.current
    }
}

/// One operating point of the external LED circuit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Series resistor, oriented pad -> signal.
    pub resistor: BranchPoint,
    /// LED, oriented anode -> cathode, including when the part is reversed.
    pub led: BranchPoint,
    /// Voltage at the AVR pad.
    pub pin_voltage: f64,
    /// Current the driver branch delivers, in amperes; zero when floating.
    pub driver_current: f64,
    /// Power the driver's series resistance absorbs, in watts.
    pub driver_power: f64,
    /// Power the driver's source absorbs, negative when it delivers.
    pub source_power: f64,
    /// Rendering brightness in `0..=1`, from solved forward current.
    pub brightness: f64,
    /// AVR input thresholds: <= 0.3 Vcc low, >= 0.6 Vcc high, otherwise undefined.
    pub digital_input: Option<bool>,
}

pub struct BlinkCircuit {
    state: PinState,
    reversed: bool,
    pub reading: Result<Reading, OpError>,
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

/// The deck for one drive mode and polarity.
fn deck(state: PinState, reversed: bool) -> String {
    let mut deck = String::from("blink\n");
    match state {
        PinState::Input => {}
        PinState::Low | PinState::High => {
            let volts = if state == PinState::High {
                SUPPLY_VOLTS
            } else {
                0.0
            };
            let _ = writeln!(deck, "{DRIVER_SOURCE} N_MCU_D9 0 {volts}");
            let _ = writeln!(deck, "R_MCU_D9 N_MCU_D9 d9 {OUTPUT_OHMS}");
        }
        PinState::InputPullUp => {
            let _ = writeln!(deck, "{DRIVER_SOURCE} N_MCU_D9 0 {SUPPLY_VOLTS}");
            let _ = writeln!(deck, "R_MCU_D9 N_MCU_D9 d9 {PULLUP_OHMS}");
        }
    }
    let _ = writeln!(deck, "R1 d9 led_a {SERIES_OHMS}");
    let (anode, cathode) = if reversed {
        ("0", "led_a")
    } else {
        ("led_a", "0")
    };
    deck.push_str(&preset().cards("D1", anode, cathode));
    deck.push_str("\n.op\n.end\n");
    deck
}

fn solve(state: PinState, reversed: bool) -> Result<Reading, OpError> {
    let solution = solve_op(&deck(state, reversed))?;
    let pin_voltage = solution.net_voltage("d9").unwrap_or(0.0);
    let signal_voltage = solution.net_voltage("led_a").unwrap_or(0.0);
    let resistor = BranchPoint {
        voltage: pin_voltage - signal_voltage,
        current: (pin_voltage - signal_voltage) / SERIES_OHMS,
    };
    // The diode's own anode -> cathode direction flips with the part, so the
    // signed branch values flip too.
    let led = if reversed {
        BranchPoint {
            voltage: -signal_voltage,
            current: -resistor.current,
        }
    } else {
        BranchPoint {
            voltage: signal_voltage,
            current: resistor.current,
        }
    };
    let (driver_current, driver_power, source_power) = match state {
        // A floating pin contributes no branch, so there is no driver to report.
        PinState::Input => (0.0, 0.0, 0.0),
        _ => {
            let ohms = if state == PinState::InputPullUp {
                PULLUP_OHMS
            } else {
                OUTPUT_OHMS
            };
            let delivered = -solution.source_current(DRIVER_SOURCE).unwrap_or(0.0);
            let absorbed = solution.source_current(DRIVER_SOURCE).unwrap_or(0.0);
            let volts = if state == PinState::Low {
                0.0
            } else {
                SUPPLY_VOLTS
            };
            (delivered, delivered * delivered * ohms, volts * absorbed)
        }
    };
    Ok(Reading {
        pin_voltage,
        resistor,
        led,
        driver_current,
        driver_power,
        source_power,
        brightness: preset().brightness(led.current),
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
    }

    #[test]
    fn reverse_led_blocks_current_despite_a_high_avr_output() {
        let r = solve(PinState::High, true).unwrap();
        assert!((r.pin_voltage - 5.0).abs() < 1e-8);
        assert!((r.led.voltage + 5.0).abs() < 1e-8);
        assert!(
            (-1e-9..0.0).contains(&r.led.current),
            "reverse leakage {} A",
            r.led.current
        );
        assert_eq!(r.brightness, 0.0);
    }

    #[test]
    fn low_high_impedance_and_pullup_have_distinct_electrical_behavior() {
        for reversed in [false, true] {
            for state in [PinState::Low, PinState::Input] {
                let r = solve(state, reversed).unwrap();
                assert!(r.led.current.abs() < 1e-12);
                assert!(r.brightness < 1e-9, "leakage brightness {}", r.brightness);
                assert_eq!(r.digital_input, Some(false));
            }
        }
        let r = solve(PinState::InputPullUp, false).unwrap();
        assert!((0.00009..0.00012).contains(&r.led.current));
        assert!(r.brightness > 0.0 && r.brightness < 0.01);
        assert_eq!(r.digital_input, None); // LED-loaded input is between thresholds.
        let r = solve(PinState::InputPullUp, true).unwrap();
        assert_eq!(r.digital_input, Some(true));
        assert!(r.brightness < 1e-9, "leakage brightness {}", r.brightness);
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
