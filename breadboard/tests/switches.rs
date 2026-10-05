// SPDX-License-Identifier: MIT

//! Switch tests: two wirings of the same button must read **opposite** ways.
//!
//! A pull-up reads high when the button is open and low when it is closed; a
//! pull-down reads the other way round. Both circuits are built as netlists and
//! driven through `AnalogCoupling`, so what is under test is the electrical
//! result rather than a boolean the host asserted.
//!
//! Recommended values are checked against independently computed voltage
//! dividers rather than against the solver's own answer, because the whole point
//! of a switch model with finite resistance in *both* states is that an open
//! contact is not a perfect break and a closed one is not a perfect short.

use avr_port_tests::sim::assembler::assemble;
use breadboard::netlist::{resistor, switch, vsource, Netlist};
use breadboard::{AnalogCoupling, BreadboardHost, Pin, Scene, Wiring};

const DDRB: usize = 0x24;
const PORTB: usize = 0x25;
const PINB: usize = 0x23;

/// `D8` is `PORTB` bit 0, so the button sits on the first digital pin.
const BUTTON_BIT: u8 = 0;

/// The button's Arduino pin.
fn button() -> Pin {
    Pin::digital(8)
}

/// The AVR's illustrative internal pull-up, matching `AnalogCoupling`.
const PULLUP_OHMS: f64 = 30_000.0;
/// The illustrative pull-down in the second wiring.
const PULLDOWN_OHMS: f64 = 10_000.0;
/// Datasheet-style tactile switch resistances, matching the component defaults.
const CONTACT_OHMS: f64 = 0.05;
const INSULATION_OHMS: f64 = 100e6;

fn idle_flash() -> Vec<u8> {
    let result = assemble("loop: JMP loop");
    assert!(result.errors.is_empty(), "assembler: {:?}", result.errors);
    let mut flash = vec![0xffu8; 0x8000];
    flash[..result.bytes.len()].copy_from_slice(&result.bytes);
    flash
}

fn host_with(netlist: Netlist) -> BreadboardHost {
    let mut host =
        BreadboardHost::new(idle_flash(), Scene::empty(), Vec::new()).expect("host boots");
    let mut coupling = AnalogCoupling::new(netlist, Wiring::new().bind(button(), "btn"))
        .expect("coupling is valid");
    coupling.bind_switch("SW1").expect("SW1 is a switch");
    host.attach_analog(coupling).expect("pin is free");
    host
}

/// Button to ground, read with the AVR's **internal pull-up**.
fn pull_up_host() -> BreadboardHost {
    let netlist = Netlist::new()
        .nets(["btn"])
        .part(switch("SW1", "btn", "gnd"));
    let mut host = host_with(netlist);
    // Input with the internal pull-up: DDRB bit 0 clear, PORTB bit 0 set.
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 1 << BUTTON_BIT);
    host.advance_micros(10.0).expect("settles");
    host
}

/// Button to the 5 V rail, with an **external pull-down** to ground.
fn pull_down_host() -> BreadboardHost {
    let netlist = Netlist::new()
        .nets(["btn", "vcc"])
        .part(vsource("M5V", 5.0, "vcc", "gnd"))
        .part(switch("SW1", "btn", "vcc"))
        .part(resistor("R1", PULLDOWN_OHMS, "btn", "gnd"));
    let mut host = host_with(netlist);
    // Plain input: the external resistor does the work, not the AVR.
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 0);
    host.advance_micros(10.0).expect("settles");
    host
}

/// Throw the switch and let the topology recompile.
fn press(host: &mut BreadboardHost, closed: bool) {
    assert!(
        host.analog_mut()
            .expect("coupling attached")
            .set_switch("SW1", closed),
        "SW1 was never bound"
    );
    host.advance_micros(10.0).expect("settles");
}

fn pin_voltage(host: &BreadboardHost) -> f64 {
    host.analog()
        .expect("coupling attached")
        .reading()
        .net_voltage("btn")
        .expect("btn is a declared net")
}

fn level(host: &BreadboardHost) -> Option<bool> {
    host.analog()
        .expect("coupling attached")
        .reading()
        .digital_level(button())
}

fn pad(host: &BreadboardHost) -> bool {
    host.board().read_bit(host.backend(), PINB, BUTTON_BIT)
}

/// A divider of `5 V` through `r_top`, loaded by `r_bottom` to ground.
fn divider(r_top: f64, r_bottom: f64) -> f64 {
    5.0 * r_bottom / (r_top + r_bottom)
}

#[test]
fn a_pull_up_button_reads_high_open_and_low_closed() {
    let mut host = pull_up_host();

    // Open: the 30k pull-up against 100 Mohm of insulation, so the pin sits just
    // under the rail rather than exactly at it.
    let expected = divider(PULLUP_OHMS, INSULATION_OHMS);
    assert!(
        (pin_voltage(&host) - expected).abs() < 1e-6,
        "open pin at {} V, predicted {expected} V",
        pin_voltage(&host)
    );
    assert_eq!(level(&host), Some(true), "an open pull-up must read high");
    assert!(pad(&host), "the AVR must see the high pad");

    // Closed: the 50 mohm contact against the 30k pull-up.
    press(&mut host, true);
    let expected = divider(PULLUP_OHMS, CONTACT_OHMS);
    assert!(
        pin_voltage(&host) < 1e-4,
        "closed pin at {} V, predicted {expected} V",
        pin_voltage(&host)
    );
    assert_eq!(level(&host), Some(false), "a closed pull-up must read low");
    assert!(!pad(&host), "the AVR must see the low pad");
}

#[test]
fn a_pull_down_button_reads_low_open_and_high_closed() {
    let mut host = pull_down_host();

    // Open: the 100 Mohm insulation is the only path to 5 V, against 10k down.
    let expected = divider(INSULATION_OHMS, PULLDOWN_OHMS);
    assert!(
        (pin_voltage(&host) - expected).abs() < 1e-9,
        "open pin at {} V, predicted {expected} V",
        pin_voltage(&host)
    );
    assert_eq!(level(&host), Some(false), "an open pull-down must read low");
    assert!(!pad(&host), "the AVR must see the low pad");

    // Closed: the 50 mohm contact to 5 V, against 10k down.
    press(&mut host, true);
    let expected = divider(CONTACT_OHMS, PULLDOWN_OHMS);
    assert!(
        (pin_voltage(&host) - expected).abs() < 1e-4,
        "closed pin at {} V, predicted {expected} V",
        pin_voltage(&host)
    );
    assert_eq!(
        level(&host),
        Some(true),
        "a closed pull-down must read high"
    );
    assert!(pad(&host), "the AVR must see the high pad");
}

#[test]
fn the_two_wirings_are_genuinely_inverted() {
    // Same button, same AVR input mode in the sense that matters, opposite
    // reading. This is the whole reason the demo shows both.
    let mut up = pull_up_host();
    let mut down = pull_down_host();
    assert_eq!(
        (level(&up), level(&down)),
        (Some(true), Some(false)),
        "released states must differ"
    );
    press(&mut up, true);
    press(&mut down, true);
    assert_eq!(
        (level(&up), level(&down)),
        (Some(false), Some(true)),
        "pressed states must differ"
    );
}

#[test]
fn a_closed_pull_up_draws_about_167_microamps_and_the_pull_down_half_a_milliamp() {
    // Sanity on the current the two wirings actually draw, because that is what
    // differs between them in practice: a pull-up only conducts while pressed.
    let mut up = pull_up_host();
    press(&mut up, true);
    let up_current = pin_voltage(&up) / CONTACT_OHMS;
    assert!(
        (up_current - 5.0 / PULLUP_OHMS).abs() < 1e-9,
        "pull-up draws {up_current} A"
    );

    let mut down = pull_down_host();
    press(&mut down, true);
    let down_current = pin_voltage(&down) / PULLDOWN_OHMS;
    assert!(
        (down_current - 5.0 / PULLDOWN_OHMS).abs() < 1e-6,
        "pull-down draws {down_current} A"
    );
    // 167 uA against 500 uA: the pull-down is the thirstier wiring.
    assert!(down_current > up_current * 2.0);
}

#[test]
fn throwing_a_switch_recompiles_the_topology_exactly_once() {
    let mut host = pull_up_host();
    let before = host.analog().expect("attached").solves();

    // Re-requesting the state it already has must not re-solve.
    press(&mut host, false);
    assert_eq!(host.analog().expect("attached").solves(), before);

    press(&mut host, true);
    assert_eq!(host.analog().expect("attached").solves(), before + 1);

    // Idle frames after the toggle are cached again.
    host.advance_micros(50.0).expect("settles");
    assert_eq!(host.analog().expect("attached").solves(), before + 1);
}

#[test]
fn binding_something_that_is_not_a_switch_is_reported() {
    let netlist = Netlist::new()
        .nets(["btn"])
        .part(resistor("R1", 10_000.0, "btn", "gnd"));
    let mut coupling =
        AnalogCoupling::new(netlist, Wiring::new().bind(button(), "btn")).expect("valid");
    assert!(matches!(
        coupling.bind_switch("R1"),
        Err(breadboard::CouplingError::NotASwitch(reference)) if reference == "R1"
    ));
    assert!(matches!(
        coupling.bind_switch("SW9"),
        Err(breadboard::CouplingError::NotASwitch(reference)) if reference == "SW9"
    ));
}
