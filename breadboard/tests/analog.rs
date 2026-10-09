// SPDX-License-Identifier: MIT

//! Analog coupling tests: a SPICE deck must drive and be driven by real AVR pins.
//!
//! Every test boots an assembled AVR program and configures pins through the
//! same register writes a sketch performs, so the loop under test is the real
//! one: DDR/PORT -> pin mode -> MCU driver in the deck -> `.op` solve -> ADC mux
//! channels and `PINx` back to the AVR.

use avr_port_tests::sim::assembler::assemble;
use breadboard::{
    AnalogCoupling, AnalogReading, BreadboardHost, CouplingError, CouplingOutcome, HostError,
    LedPreset, Pin, Scene, Wiring,
};

const DDRB: usize = 0x24;
const PORTB: usize = 0x25;
const PINB: usize = 0x23;
const DDRC: usize = 0x27;
const ADMUX: usize = 0x7c;
const ADCSRA: usize = 0x7a;
const ADCL: usize = 0x78;
const ADCH: usize = 0x79;

/// `D9` = `PORTB1`.
const D9_BIT: u8 = 1;

/// Series resistor between the pin and the LED.
const SERIES_OHMS: f64 = 220.0;

fn idle_flash() -> Vec<u8> {
    let result = assemble("loop: JMP loop");
    assert!(result.errors.is_empty(), "assembler: {:?}", result.errors);
    let mut flash = vec![0xffu8; 0x8000];
    flash[..result.bytes.len()].copy_from_slice(&result.bytes);
    flash
}

fn host() -> BreadboardHost {
    BreadboardHost::new(idle_flash(), Scene::empty(), Vec::new()).expect("host boots")
}

/// `D9` -> 220 ohm -> LED -> ground, as a breadboard would be wired.
fn led_deck() -> String {
    format!(
        "led\nR1 d9 led_a {SERIES_OHMS}\n{}\n",
        LedPreset::red().cards("D1", "led_a", "0")
    )
}

fn attach_led(host: &mut BreadboardHost) {
    let coupling = AnalogCoupling::new(led_deck(), Wiring::new().bind(Pin::digital(9), "d9"))
        .expect("coupling is valid");
    host.attach_analog(coupling).expect("attaches");
}

fn drive_d9(host: &mut BreadboardHost, high: bool) {
    host.write_data(DDRB, 1 << D9_BIT);
    host.write_data(PORTB, if high { 1 << D9_BIT } else { 0 });
    host.advance_micros(10.0).expect("settles");
}

fn reading(host: &BreadboardHost) -> &AnalogReading {
    host.analog().expect("coupling attached").reading()
}

/// LED current, from Ohm's law across the ideal series resistor.
///
/// ngspice does not expose `@d1[id]`, so the branch current is derived from the
/// solved node voltages rather than read from the diode.
fn led_current(host: &BreadboardHost) -> f64 {
    let reading = reading(host);
    let pin = reading.net_voltage("d9").expect("d9");
    let anode = reading.net_voltage("led_a").expect("led_a");
    (pin - anode) / SERIES_OHMS
}

#[test]
fn a_high_output_drives_the_led_through_its_thevenin_resistance() {
    let mut host = host();
    attach_led(&mut host);
    drive_d9(&mut host, true);

    assert_eq!(reading(&host).outcome(), CouplingOutcome::Solved);
    assert!((reading(&host).net_voltage("d9").unwrap() - 4.709_244).abs() < 1e-4);
    assert!((reading(&host).net_voltage("led_a").unwrap() - 2.150_594).abs() < 1e-4);
    assert!((led_current(&host) - 1.163_023e-2).abs() < 1e-6);
    // A source delivering power reports a negative current in SPICE.
    assert!((reading(&host).source_current("V_MCU_D9").unwrap() + led_current(&host)).abs() < 1e-9);
    // The pin is driven, so the circuit does not get to decide its logic level.
    assert_eq!(reading(&host).digital_level(Pin::digital(9)), Some(true));
}

#[test]
fn a_low_output_blocks_the_led_without_inventing_a_negative_drop() {
    let mut host = host();
    attach_led(&mut host);
    drive_d9(&mut host, false);

    assert!(reading(&host).net_voltage("d9").unwrap().abs() < 1e-9);
    // Reverse leakage only: no fabricated forward current.
    assert!(led_current(&host).abs() < 1e-9);
}

#[test]
fn a_reverse_biased_led_blocks_current_but_the_pad_still_rises() {
    // Same parts, diode reversed. The pad is pulled to the rail because the
    // diode blocks, which is exactly what a real open circuit does.
    let deck = format!(
        "led reversed\nR1 d9 led_a {SERIES_OHMS}\n{}\n",
        LedPreset::red().cards("D1", "0", "led_a")
    );
    let mut host = host();
    host.attach_analog(
        AnalogCoupling::new(deck, Wiring::new().bind(Pin::digital(9), "d9")).unwrap(),
    )
    .unwrap();
    drive_d9(&mut host, true);

    assert!((reading(&host).net_voltage("d9").unwrap() - 5.0).abs() < 1e-6);
    assert!(led_current(&host).abs() < 1e-9);
}

#[test]
fn a_high_impedance_input_takes_its_level_from_the_circuit() {
    // 5 V through 1k to the pin, 9k from the pin to ground: 4.5 V, a definite high.
    let deck = "divider high\nV1 vcc 0 5\nR1 vcc d9 1000\nR2 d9 0 9000\n";
    let mut host = host();
    host.attach_analog(
        AnalogCoupling::new(deck, Wiring::new().bind(Pin::digital(9), "d9")).unwrap(),
    )
    .unwrap();
    // Input, no pull-up: the MCU contributes no driver at all.
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 0);
    host.advance_micros(10.0).unwrap();

    assert!((reading(&host).net_voltage("d9").unwrap() - 4.5).abs() < 1e-6);
    assert_eq!(reading(&host).digital_level(Pin::digital(9)), Some(true));
    // The resolved level reached the AVR's own input register.
    assert!(host.board().read_bit(host.backend(), PINB, D9_BIT));
}

#[test]
fn a_divider_in_the_threshold_band_leaves_the_input_indeterminate() {
    // 5 V through 1k and 1k to ground: 2.5 V sits between 0.3 Vcc and 0.6 Vcc.
    let deck = "divider mid\nV1 vcc 0 5\nR1 vcc d9 1000\nR2 d9 0 1000\n";
    let mut host = host();
    host.attach_analog(
        AnalogCoupling::new(deck, Wiring::new().bind(Pin::digital(9), "d9")).unwrap(),
    )
    .unwrap();
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 0);
    host.advance_micros(10.0).unwrap();

    assert!((reading(&host).net_voltage("d9").unwrap() - 2.5).abs() < 1e-6);
    assert_eq!(reading(&host).digital_level(Pin::digital(9)), None);
    // Nothing was written back, so the pad keeps its previous level.
    assert!(!host.board().read_bit(host.backend(), PINB, D9_BIT));
}

#[test]
fn a_pull_up_is_loaded_by_the_circuit_instead_of_forced_high() {
    let mut host = host();
    attach_led(&mut host);
    // Input with the internal pull-up: 5 V behind 30k into the LED path.
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 1 << D9_BIT);
    host.advance_micros(10.0).unwrap();

    let current = led_current(&host);
    assert!(
        (9e-5..1.2e-4).contains(&current),
        "pull-up current {current}"
    );
    // The loaded pull-up sits inside the threshold band, so the level is
    // genuinely ambiguous rather than rounded to a logic high.
    assert_eq!(reading(&host).digital_level(Pin::digital(9)), None);
}

#[test]
fn the_solved_node_voltage_reaches_the_adc_and_the_avr_reads_it() {
    // A 10k/10k divider from 5 V puts exactly 2.5 V on A0.
    let deck = "adc divider\nV1 vcc 0 5\nR1 vcc a0 10000\nR2 a0 0 10000\n";
    let mut host = host();
    host.attach_analog(
        AnalogCoupling::new(deck, Wiring::new().bind(Pin::analog(0), "a0")).unwrap(),
    )
    .unwrap();
    host.write_data(DDRC, 0);
    host.advance_micros(10.0).unwrap();

    assert!((reading(&host).adc_voltage(0).unwrap() - 2.5).abs() < 1e-6);

    // Drive a real conversion: AVCC-referenced, channel 0, prescaler 2.
    host.write_data(ADMUX, 0x40);
    host.write_data(ADCSRA, 0xc0);
    host.advance_micros(200.0).unwrap();

    let low = host.board().read_data(host.backend(), ADCL);
    let high = host.board().read_data(host.backend(), ADCH);
    let counts = u16::from(high) << 8 | u16::from(low);
    // 2.5 V of a 5 V reference is half scale on a 10-bit converter.
    assert!(
        (511..=513).contains(&counts),
        "ADC read {counts}, expected about 512"
    );
}

#[test]
fn an_unbound_net_is_reported_rather_than_silently_ignored() {
    let coupling =
        AnalogCoupling::new(led_deck(), Wiring::new().bind(Pin::digital(9), "not_a_net"))
            .expect("the deck itself is valid");
    let mut host = host();
    let error = host
        .attach_analog(coupling)
        .expect_err("a net no device connects must be reported");
    assert_eq!(
        error,
        HostError::Coupling(CouplingError::UnknownNet {
            pin: Pin::digital(9),
            net: "not_a_net".into()
        })
    );
}

#[test]
fn binding_the_same_pin_twice_is_rejected() {
    let error = AnalogCoupling::new(
        led_deck(),
        Wiring::new()
            .bind(Pin::digital(9), "d9")
            .bind(Pin::digital(9), "led_a"),
    )
    .unwrap_err();
    assert_eq!(error, CouplingError::DuplicateBinding(Pin::digital(9)));
}

#[test]
fn a_malformed_deck_is_rejected_at_construction() {
    let error = AnalogCoupling::new("broken\nR1 d9\n", Wiring::new().bind(Pin::digital(9), "d9"))
        .unwrap_err();
    assert!(matches!(error, CouplingError::Spice(_)), "{error:?}");
}

#[test]
fn an_unchanged_drive_mode_reuses_the_solved_operating_point() {
    let mut host = host();
    attach_led(&mut host);
    drive_d9(&mut host, true);
    let solves = host.analog().unwrap().solves();

    // Repeated updates with no electrical change must not re-solve.
    host.advance_micros(50.0).unwrap();
    host.advance_micros(50.0).unwrap();
    assert_eq!(host.analog().unwrap().solves(), solves);

    // A Low/High change rewrites the driver source and re-solves.
    drive_d9(&mut host, false);
    assert_eq!(host.analog().unwrap().solves(), solves + 1);
    drive_d9(&mut host, true);
    assert_eq!(host.analog().unwrap().solves(), solves + 2);

    // Changing shape (driven -> high impedance) re-solves too.
    host.write_data(DDRB, 0);
    host.advance_micros(10.0).unwrap();
    assert_eq!(host.analog().unwrap().solves(), solves + 3);
}

#[test]
fn an_undriven_net_is_reported_as_indeterminate_not_as_a_number() {
    // A resistor to a floating net with no source anywhere has no unique
    // solution; that must surface as an error outcome, not a fabricated 0 V.
    let deck = "floating\nR1 d9 floating 1000\n";
    let mut host = host();
    host.attach_analog(
        AnalogCoupling::new(deck, Wiring::new().bind(Pin::digital(9), "d9")).unwrap(),
    )
    .unwrap();
    host.write_data(DDRB, 0);
    host.write_data(PORTB, 0);
    host.advance_micros(10.0).unwrap();

    let outcome = reading(&host).outcome();
    assert!(
        matches!(outcome, CouplingOutcome::Indeterminate(_)),
        "expected an indeterminate solve, got {outcome:?}"
    );
    assert!(reading(&host).net_voltage("d9").is_none());
}
