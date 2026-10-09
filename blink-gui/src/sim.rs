// SPDX-License-Identifier: MIT

//! Simulator side of the demo: backend, dispatch runtime and wired-up board.
//!
//! Nothing here knows about rendering. GPIO is observed at the cycle-poll
//! boundary; the external LED is lit by solved current, not `led_on`.

use crate::analog::BlinkCircuit;
use avr_port_tests::board::{parse_hex, Board, PinState, LED_BUILTIN_PIN, UNO_CLOCK_HZ};
use avr_port_tests::{native_backend, Backend, Runtime};
use std::rc::Rc;

/// Firmware built from `sketches/blink/blink.ino` for `arduino:avr:uno`.
const FIRMWARE: &str = include_str!("../firmware/blink.hex");

/// AVR cycles executed per rendered frame at 1.0x: 16 MHz / 60 fps.
pub const CYCLES_PER_FRAME_1X: f64 = UNO_CLOCK_HZ / 60.0;

/// Instructions between simulated-cycle checks. Bounds how far [`Sim::advance`]
/// can overshoot while keeping the per-step cost near zero.
const CYCLE_POLL_STEPS: u64 = 32;

/// The simulator side of the demo.
pub struct Sim {
    backend: Rc<dyn Backend>,
    runtime: Runtime,
    board: Board,
    instructions: u64,
    toggles: u64,
    /// Onboard digital indicator (not part of the external circuit deck).
    pub led_on: bool,
    pub analog: BlinkCircuit,
}

impl Sim {
    /// Cold-boot the simulated Uno and let its startup code run.
    pub fn boot() -> Self {
        Self::boot_with_polarity(false)
    }

    pub fn boot_with_polarity(reversed: bool) -> Self {
        let backend: Rc<dyn Backend> = native_backend();
        let mut runtime = Runtime::new(backend.clone());
        let board = Board::uno(backend.as_ref(), &mut runtime, parse_hex(FIRMWARE));
        let led_on = board.led_builtin(backend.as_ref(), &mut runtime).is_high();
        Self {
            backend,
            runtime,
            board,
            instructions: 0,
            toggles: 0,
            led_on,
            analog: BlinkCircuit::new(reversed),
        }
    }

    /// Rewire only the external LED, including while AVR execution is paused.
    pub fn toggle_led_polarity(&mut self) {
        self.update_circuit(self.analog.state(), !self.analog.reversed());
    }

    fn update_circuit(&mut self, state: PinState, reversed: bool) {
        if self.analog.state() == state && self.analog.reversed() == reversed {
            return;
        }
        self.analog.update(state, reversed);
        if matches!(state, PinState::Input | PinState::InputPullUp) {
            if let Ok(reading) = &self.analog.reading {
                if let Some(high) = reading.digital_input {
                    self.board.set_input(
                        self.backend.as_ref(),
                        &mut self.runtime,
                        self.board.portb,
                        LED_BUILTIN_PIN,
                        high,
                    );
                }
                // The indeterminate threshold band retains the previous digital
                // sample; it must not silently be rounded to a valid high/low.
            }
        }
    }

    /// Execute one instruction and refill the callback budget when due.
    fn tick(&mut self) {
        // Timer interrupts exhaust the Runtime's per-case callback budget;
        // refill it rather than rebuilding the Runtime every frame.
        if self.instructions > 0
            && self
                .instructions
                .is_multiple_of(Board::BUDGET_REFILL_CYCLES)
        {
            self.runtime.reset_budget();
        }
        self.board.step(self.backend.as_ref(), &mut self.runtime);
        self.instructions += 1;
    }

    /// Simulated AVR cycles executed so far.
    pub fn cycles(&self) -> u64 {
        self.board.cycles(self.backend.as_ref()) as u64
    }

    /// Execute at least `cycles` simulated AVR cycles, then resample the LED.
    ///
    /// Budgeting in cycles rather than instructions is what makes 1.0x real
    /// time: this firmware retires roughly 0.77 instructions per cycle, so
    /// stepping a fixed instruction count would run about 1.3x too fast.
    pub fn advance(&mut self, cycles: u64) {
        let target = self.cycles() + cycles;
        while self.cycles() < target {
            for _ in 0..CYCLE_POLL_STEPS {
                self.tick();
            }
            let state = self
                .board
                .led_builtin(self.backend.as_ref(), &mut self.runtime);
            let on = state.is_high();
            if on != self.led_on {
                self.toggles += 1;
            }
            self.led_on = on;
            self.update_circuit(state, self.analog.reversed());
        }
    }

    /// Simulated elapsed milliseconds, on `millis()`'s time base.
    pub fn millis(&self) -> f64 {
        self.board.millis(self.backend.as_ref())
    }

    /// Instructions retired so far.
    pub fn instructions(&self) -> u64 {
        self.instructions
    }

    /// LED transitions observed so far.
    pub fn toggles(&self) -> u64 {
        self.toggles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avr_port_tests::Value;

    #[test]
    fn compiled_blink_drives_current_and_reverse_polarity_is_independent_of_gpio() {
        let mut sim = Sim::boot();
        assert_eq!(sim.analog.state(), PinState::Input);
        assert!(sim.analog.brightness() < 1e-9, "gmin leakage only");
        sim.advance(2_000_000);
        assert!(sim.led_on);
        assert_eq!(sim.analog.state(), PinState::High);
        assert!(sim.analog.reading.as_ref().unwrap().led.current > 0.011);
        let cycles = sim.cycles();
        sim.toggle_led_polarity();
        assert_eq!(sim.cycles(), cycles); // Rewire while paused.
        assert!(sim.led_on); // Onboard indicator still high.
        assert!(sim.analog.brightness() < 1e-9, "gmin leakage only");
        assert!(sim.analog.reading.as_ref().unwrap().led.voltage < -4.99);
        sim.toggle_led_polarity();
        assert!(sim.analog.brightness() > 0.5);
        sim.advance(8_000_000);
        assert!(!sim.led_on);
        assert_eq!(sim.analog.state(), PinState::Low);
        assert!(sim.analog.brightness() < 1e-9, "gmin leakage only");
        sim.advance(8_000_000);
        assert!(sim.led_on);
        assert!(sim.analog.brightness() > 0.5);
    }

    #[test]
    fn analog_thresholds_feed_an_input_back_to_the_avr_pin_register() {
        let mut sim = Sim::boot_with_polarity(true);
        // PORTB pull-up enabled with DDRB still input, using the real write hook.
        sim.backend.call(
            &mut sim.runtime,
            Some(sim.board.cpu_handle),
            "writeData",
            vec![
                Value::Number(0x25 as f64),
                Value::Number((1 << LED_BUILTIN_PIN) as f64),
            ],
        );
        sim.update_circuit(PinState::InputPullUp, true);
        assert_eq!(
            sim.analog.reading.as_ref().unwrap().digital_input,
            Some(true)
        );
        assert!(sim
            .board
            .read_bit(sim.backend.as_ref(), 0x23, LED_BUILTIN_PIN));
        sim.backend.call(
            &mut sim.runtime,
            Some(sim.board.cpu_handle),
            "writeData",
            vec![Value::Number(0x25 as f64), Value::Number(0.0)],
        );
        sim.update_circuit(PinState::Input, true);
        assert!(!sim
            .board
            .read_bit(sim.backend.as_ref(), 0x23, LED_BUILTIN_PIN));
    }
}
