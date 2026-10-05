// SPDX-License-Identifier: MIT

//! Simulator side of the two switch-input demos.
//!
//! The same button, the same LED and the same firmware *behaviour* in two
//! wirings that read opposite levels:
//!
//! | Wiring | Button | Pin mode | Released | Pressed |
//! | --- | --- | --- | --- | --- |
//! | simple | `D2` to GND | `INPUT_PULLUP` | HIGH | LOW |
//! | inverse | `D2` to 5 V, 10 k to GND | `INPUT` | LOW | HIGH |
//!
//! Both circuits are netlists driven through `breadboard`'s `AnalogCoupling`, so
//! the pin level is a solved node voltage rather than an asserted boolean, and
//! the switch is thrown through [`AnalogCoupling::set_switch`].
//!
//! Nothing here knows about rendering, so the electrical result is testable
//! without a display.

use breadboard::netlist::{resistor, switch, vsource, Netlist, Parameters, PlacedPart};
use breadboard::{
    AnalogCoupling, BreadboardHost, CouplingOutcome, Pin, Scene, Wiring as PinWiring,
};
use circuit_components::{Led, LedParameters};

const PULL_UP_FIRMWARE: &str = include_str!("../firmware/switch_pullup.hex");
const PULL_DOWN_FIRMWARE: &str = include_str!("../firmware/switch_pulldown.hex");

/// The button's Arduino pin; `D2` is `PORTD` bit 2.
pub const BUTTON_PIN: u8 = 2;
/// The indicator LED's pin.
pub const LED_PIN: u8 = 9;
/// Series resistor for the LED.
pub const SERIES_OHMS: f64 = 330.0;
/// The external pull-down in the inverse wiring.
pub const PULLDOWN_OHMS: f64 = 10_000.0;
/// The 5 V rail the inverse wiring switches to.
pub const SUPPLY_VOLTS: f64 = 5.0;
/// Netlist reference of the button, so the host can throw it.
pub const SWITCH_REFERENCE: &str = "SW1";
/// Netlist reference of the indicator LED.
pub const LED_REFERENCE: &str = "D1";

/// Which way the button is wired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wiring {
    /// Button to ground, read with the AVR's internal pull-up. Idles HIGH.
    PullUp,
    /// Button to 5 V with an external pull-down. Idles LOW.
    PullDown,
}

impl Wiring {
    /// Short label for the overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::PullUp => "simple: D2 to GND, internal pull-up",
            Self::PullDown => "inverse: D2 to 5V, external 10k pull-down",
        }
    }

    /// What the sketch does differently, in one line.
    pub fn logic(self) -> &'static str {
        match self {
            Self::PullUp => "pressed reads LOW, so the sketch inverts it",
            Self::PullDown => "pressed reads HIGH, so no inversion is needed",
        }
    }

    /// The level the pin reads while the button is held down.
    pub fn pressed_level(self) -> bool {
        match self {
            Self::PullUp => false,
            Self::PullDown => true,
        }
    }

    /// Whether the AVR's internal pull-up is what holds the pin up.
    pub fn uses_internal_pullup(self) -> bool {
        self == Self::PullUp
    }

    fn firmware(self) -> &'static str {
        match self {
            Self::PullUp => PULL_UP_FIRMWARE,
            Self::PullDown => PULL_DOWN_FIRMWARE,
        }
    }
}

fn red_led(reference: &str, anode_net: &str) -> PlacedPart {
    PlacedPart::new(reference, "led.red", Parameters::new())
        .terminal("a", anode_net)
        .terminal("k", "gnd")
}

/// The two switch wires, as a netlist.
fn netlist(wiring: Wiring) -> Netlist {
    match wiring {
        Wiring::PullUp => Netlist::new()
            .nets(["btn", "d9", "led_a"])
            .part(switch(SWITCH_REFERENCE, "btn", "gnd"))
            .part(resistor("R1", SERIES_OHMS, "d9", "led_a"))
            .part(red_led(LED_REFERENCE, "led_a")),
        Wiring::PullDown => Netlist::new()
            .nets(["btn", "d9", "led_a", "vcc"])
            // The board's 5 V rail, modelled as an ideal source: the regulator's
            // output impedance and current limit are not represented.
            .part(vsource("M5V", SUPPLY_VOLTS, "vcc", "gnd"))
            .part(switch(SWITCH_REFERENCE, "btn", "vcc"))
            .part(resistor("R2", PULLDOWN_OHMS, "btn", "gnd"))
            .part(resistor("R1", SERIES_OHMS, "d9", "led_a"))
            .part(red_led(LED_REFERENCE, "led_a")),
    }
}

/// One switch demo's board and circuit.
pub struct SwitchSim {
    host: BreadboardHost,
    wiring: Wiring,
    model: Led,
}

impl SwitchSim {
    /// Boot the firmware for `wiring` and wire the button and LED.
    pub fn boot(wiring: Wiring) -> Self {
        let pins = PinWiring::new()
            .bind(Pin::digital(BUTTON_PIN), "btn")
            .bind(Pin::digital(LED_PIN), "d9");
        let mut host = BreadboardHost::from_hex(wiring.firmware(), Scene::empty(), Vec::new())
            .expect("switch firmware boots");
        let mut coupling =
            AnalogCoupling::new(netlist(wiring), pins).expect("switch netlist is valid");
        coupling
            .bind_switch(SWITCH_REFERENCE)
            .expect("SW1 is a switch");
        host.attach_analog(coupling).expect("pins are free");

        Self {
            host,
            wiring,
            model: Led::new(LedParameters::red()).expect("documented preset is valid"),
        }
    }

    /// Which wiring this instance is simulating.
    pub fn wiring(&self) -> Wiring {
        self.wiring
    }

    /// Press or release the button. The topology recompiles on the next advance.
    pub fn set_pressed(&mut self, pressed: bool) {
        assert!(
            self.host
                .analog_mut()
                .expect("coupling attached")
                .set_switch(SWITCH_REFERENCE, pressed),
            "SW1 was bound at boot"
        );
    }

    /// Whether the button is currently held down.
    pub fn pressed(&self) -> bool {
        self.host
            .analog()
            .expect("coupling attached")
            .switch(SWITCH_REFERENCE)
            .expect("SW1 was bound at boot")
    }

    /// Advance simulated time.
    pub fn advance_micros(&mut self, micros: f64) {
        self.host
            .advance_micros(micros)
            .expect("switch demo solves");
    }

    /// Advance enough for the 10 ms polling loop to react to a button change.
    #[cfg(test)]
    pub fn advance_until_settled(&mut self) {
        self.advance_micros(30_000.0);
    }

    /// Solved voltage on the button net.
    pub fn button_voltage(&self) -> f64 {
        self.reading().net_voltage("btn").unwrap_or(0.0)
    }

    /// Thresholded level the AVR reads on its button pin.
    pub fn digital_level(&self) -> Option<bool> {
        self.reading().digital_level(Pin::digital(BUTTON_PIN))
    }

    /// Solved forward current through the indicator LED.
    pub fn led_current(&self) -> f64 {
        self.reading()
            .branch(LED_REFERENCE)
            .map_or(0.0, |point| point.current)
    }

    /// Solved forward voltage across the indicator LED.
    pub fn led_voltage(&self) -> f64 {
        self.reading()
            .branch(LED_REFERENCE)
            .map_or(0.0, |point| point.voltage)
    }

    /// Rendering brightness from the LED model's own current mapping.
    pub fn led_brightness(&self) -> f32 {
        self.model.brightness(self.led_current()) as f32
    }

    /// Whether the LED is carrying meaningful forward current.
    pub fn led_lit(&self) -> bool {
        self.led_current() > 1e-3
    }

    /// Whether the last solve succeeded, or why it did not.
    pub fn outcome(&self) -> CouplingOutcome {
        self.reading().outcome()
    }

    /// Solves performed so far; a cached frame does not count.
    pub fn solves(&self) -> u64 {
        self.host.analog().expect("coupling attached").solves()
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

    fn reading(&self) -> &breadboard::AnalogReading {
        self.host.analog().expect("coupling attached").reading()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent divider prediction: 5 V through `r_top` into `r_bottom`.
    fn divider(r_top: f64, r_bottom: f64) -> f64 {
        SUPPLY_VOLTS * r_bottom / (r_top + r_bottom)
    }

    fn released(wiring: Wiring) -> SwitchSim {
        let mut sim = SwitchSim::boot(wiring);
        sim.advance_until_settled();
        sim
    }

    fn pressed(wiring: Wiring) -> SwitchSim {
        let mut sim = SwitchSim::boot(wiring);
        sim.set_pressed(true);
        sim.advance_until_settled();
        sim
    }

    #[test]
    fn the_two_wirings_idle_at_opposite_levels() {
        let up = released(Wiring::PullUp);
        let down = released(Wiring::PullDown);
        assert_eq!(up.digital_level(), Some(true), "pull-up idles high");
        assert_eq!(down.digital_level(), Some(false), "pull-down idles low");
        // And the idle levels are the ones a 5 V logic family expects.
        assert!(up.button_voltage() > 4.5, "{}", up.button_voltage());
        assert!(down.button_voltage() < 0.5, "{}", down.button_voltage());
    }

    #[test]
    fn the_two_wirings_press_to_opposite_levels() {
        let up = pressed(Wiring::PullUp);
        let down = pressed(Wiring::PullDown);
        assert_eq!(up.digital_level(), Some(false), "pull-up reads low pressed");
        assert_eq!(
            down.digital_level(),
            Some(true),
            "pull-down reads high pressed"
        );
    }

    #[test]
    fn the_released_pull_up_voltage_is_the_insulation_loaded_divider() {
        // The open contact is 100 Mohm, not an ideal break, so the 30k pull-up
        // is very slightly loaded. 5 * 1e8/(1e8 + 3e4) = 4.9985 V.
        let sim = released(Wiring::PullUp);
        let predicted = divider(30_000.0, 100e6);
        assert!(
            (sim.button_voltage() - predicted).abs() < 1e-6,
            "{} vs {predicted}",
            sim.button_voltage()
        );
        assert!(
            sim.button_voltage() < SUPPLY_VOLTS,
            "an open switch cannot reach the rail exactly"
        );
    }

    #[test]
    fn the_released_pull_down_voltage_is_the_insulation_loaded_divider() {
        // Here the 100 Mohm insulation is the only path up, against 10k down:
        // 5 * 1e4/(1e8 + 1e4) = 0.5 mV.
        let sim = released(Wiring::PullDown);
        let predicted = divider(100e6, PULLDOWN_OHMS);
        assert!(
            (sim.button_voltage() - predicted).abs() < 1e-9,
            "{} vs {predicted}",
            sim.button_voltage()
        );
        assert!(sim.button_voltage() > 0.0, "a closed path leaks a little");
    }

    #[test]
    fn the_firmware_mirrors_the_button_to_the_led_in_both_wirings() {
        for wiring in [Wiring::PullUp, Wiring::PullDown] {
            let idle = released(wiring);
            assert!(!idle.led_lit(), "{wiring:?}: LED lit while released");
            let held = pressed(wiring);
            assert!(held.led_lit(), "{wiring:?}: LED dark while pressed");
            assert!(held.led_brightness() > 0.5, "{wiring:?}: dim");
            // A red LED behind 330 ohm on a 5 V pin sits near 8 mA.
            assert!(
                (7e-3..9e-3).contains(&held.led_current()),
                "{wiring:?}: {:.3} mA",
                held.led_current() * 1e3
            );
        }
    }

    #[test]
    fn the_pull_down_wiring_draws_more_current_while_held() {
        // 5 V across 10k is 500 uA; the pull-up path is 5 V across 30k, 167 uA.
        let up = pressed(Wiring::PullUp);
        let down = pressed(Wiring::PullDown);
        let up_current = up.button_voltage() / 0.05;
        let down_current = down.button_voltage() / PULLDOWN_OHMS;
        assert!(
            (up_current - SUPPLY_VOLTS / 30_000.0).abs() < 1e-9,
            "{up_current} A"
        );
        assert!(
            (down_current - SUPPLY_VOLTS / PULLDOWN_OHMS).abs() < 1e-6,
            "{down_current} A"
        );
        assert!(down_current > up_current * 2.0);
    }

    #[test]
    fn idle_frames_reuse_the_solved_topology() {
        let mut sim = SwitchSim::boot(Wiring::PullUp);
        sim.advance_until_settled();
        let settled = sim.solves();

        // Nothing electrical changes while the button is held still, however
        // many frames elapse.
        for _ in 0..5 {
            sim.advance_until_settled();
        }
        assert_eq!(sim.solves(), settled, "idle frames must be cached");

        // Pressing moves two things: the switch recompiles, and the firmware then
        // drives the LED to the other level, which is a source update rather than
        // a recompile because the driver's shape is unchanged. So the count rises
        // by a small bounded amount, not once per simulated frame.
        sim.set_pressed(true);
        sim.advance_until_settled();
        let after_press = sim.solves();
        assert!(after_press > settled, "the toggle must re-solve");
        assert!(
            after_press <= settled + 3,
            "{} solves for one button press",
            after_press - settled
        );

        for _ in 0..5 {
            sim.advance_until_settled();
        }
        assert_eq!(sim.solves(), after_press, "settled again, so cached again");
    }

    #[test]
    fn every_solve_reports_success() {
        for wiring in [Wiring::PullUp, Wiring::PullDown] {
            assert_eq!(released(wiring).outcome(), CouplingOutcome::Solved);
            assert_eq!(pressed(wiring).outcome(), CouplingOutcome::Solved);
        }
    }
}
