// SPDX-License-Identifier: MIT

//! End-to-end tests for the HC-SR04 through the real AVR board.
//!
//! These run a genuinely assembled AVR program and drive TRIG by writing the
//! same registers a sketch would, so the pipeline under test is the real one:
//! AVR port register -> `PINx` pad -> host observation -> sensor state machine
//! -> scheduled event -> external drive -> `PINx` back to the AVR.

use avr_sim::sim::assembler::assemble;
use breadboard::{
    BreadboardHost, Drive, HcSr04, HostError, Pin, Reflector, Scene, Stimulus,
    UltrasonicParameters, UNO_CLOCK_HZ,
};

/// `DDRB`, `PINB` and `PORTB` in data space.
const DDRB: usize = 0x24;
const PORTB: usize = 0x25;
const PINB: usize = 0x23;

/// `D9` is `PORTB1` (TRIG); `D10` is `PORTB2` (ECHO).
const TRIG_BIT: u8 = 1;
const ECHO_BIT: u8 = 2;

/// An ATmega328P image whose only job is to spin forever.
///
/// The host drives the test through registers, so the firmware must not touch
/// `PORTB` itself. An explicit halt loop is used rather than `0xff` filler so a
/// runaway program cannot masquerade as passing.
fn idle_flash() -> Vec<u8> {
    let result = assemble("loop: JMP loop");
    assert!(result.errors.is_empty(), "assembler: {:?}", result.errors);
    let mut flash = vec![0xffu8; 0x8000];
    flash[..result.bytes.len()].copy_from_slice(&result.bytes);
    flash
}

fn host(scene: Scene) -> BreadboardHost {
    BreadboardHost::new(
        idle_flash(),
        scene,
        vec![Box::new(HcSr04::with_defaults()) as Box<dyn Stimulus>],
    )
    .expect("host boots")
}

/// Independently recompute the ECHO pulse width from the physical constants,
/// rather than restating the model's own arithmetic.
fn expected_pulse_cycles(distance_m: f64, temperature_celsius: f64) -> u64 {
    let speed = 331.3 + 0.606 * temperature_celsius;
    (2.0 * distance_m / speed * UNO_CLOCK_HZ).round() as u64
}

/// Drive `D9` high for `high_micros`, then low to start a measurement.
fn pulse_trigger(host: &mut BreadboardHost, high_micros: f64) {
    host.write_data(DDRB, 1 << TRIG_BIT);
    host.write_data(PORTB, 1 << TRIG_BIT);
    host.advance_micros(high_micros).expect("trigger pulse");
    host.write_data(PORTB, 0);
}

fn echo_level(host: &BreadboardHost) -> bool {
    host.board().read_bit(host.backend(), PINB, ECHO_BIT)
}

#[test]
fn echo_pulse_width_reports_the_scene_distance() {
    let mut host = host(Scene::wall(0.5).expect("valid wall"));
    pulse_trigger(&mut host, 20.0);

    // Inside the module's response delay ECHO must still be idle.
    host.advance_micros(100.0).expect("pre-response");
    assert!(!echo_level(&host), "ECHO rose before the response delay");
    assert!(!host.find::<HcSr04>().unwrap().echo_high());

    // Past the response delay ECHO is high and the AVR can read it.
    host.advance_micros(400.0).expect("response");
    assert!(echo_level(&host), "ECHO did not rise on PINB");
    assert!(host.find::<HcSr04>().unwrap().echo_high());

    // After the round trip ECHO drops again and the measurement is recorded.
    host.advance_micros(3000.0).expect("echo");
    assert!(!echo_level(&host), "ECHO did not fall on PINB");
    let measurement = host
        .find::<HcSr04>()
        .unwrap()
        .last_measurement()
        .expect("measurement recorded");
    assert!(
        (measurement.distance_m - 0.5).abs() < 1e-12,
        "measured {}",
        measurement.distance_m
    );
    assert_eq!(
        measurement.echo_cycles,
        expected_pulse_cycles(0.5, 20.0),
        "pulse width disagrees with independent time-of-flight"
    );
}

#[test]
fn pulse_width_scales_with_distance_and_temperature() {
    let mut near = host(Scene::wall(0.3).unwrap());
    pulse_trigger(&mut near, 20.0);
    near.advance_micros(2500.0).unwrap();
    let near_cycles = near
        .find::<HcSr04>()
        .unwrap()
        .last_measurement()
        .unwrap()
        .echo_cycles;
    assert_eq!(near_cycles, expected_pulse_cycles(0.3, 20.0));

    let mut far = host(Scene::wall(0.6).unwrap());
    pulse_trigger(&mut far, 20.0);
    far.advance_micros(5000.0).unwrap();
    let far_cycles = far
        .find::<HcSr04>()
        .unwrap()
        .last_measurement()
        .unwrap()
        .echo_cycles;
    assert_eq!(far_cycles, expected_pulse_cycles(0.6, 20.0));
    assert!(
        far_cycles > near_cycles * 2 - 3 && far_cycles < near_cycles * 2 + 3,
        "doubling distance should double the pulse: {near_cycles} -> {far_cycles}"
    );

    // Warmer air is faster, so the same distance reads a shorter pulse.
    let warm_parameters = UltrasonicParameters {
        temperature_celsius: 40.0,
        ..UltrasonicParameters::default()
    };
    let mut warm = BreadboardHost::new(
        idle_flash(),
        Scene::wall(0.6).unwrap(),
        vec![Box::new(HcSr04::new(warm_parameters).unwrap()) as Box<dyn Stimulus>],
    )
    .unwrap();
    pulse_trigger(&mut warm, 20.0);
    warm.advance_micros(5000.0).unwrap();
    let warm_cycles = warm
        .find::<HcSr04>()
        .unwrap()
        .last_measurement()
        .unwrap()
        .echo_cycles;
    assert!(
        warm_cycles < far_cycles,
        "warmer air must shorten the pulse"
    );
    assert_eq!(warm_cycles, expected_pulse_cycles(0.6, 40.0));
}

#[test]
fn a_trigger_shorter_than_the_minimum_is_ignored() {
    let mut host = host(Scene::wall(0.5).unwrap());
    pulse_trigger(&mut host, 5.0);
    host.advance_micros(2000.0).unwrap();

    let sensor = host.find::<HcSr04>().unwrap();
    assert_eq!(sensor.rejected_triggers(), 1);
    assert_eq!(sensor.no_echo_measurements(), 0);
    assert!(sensor.last_measurement().is_none());
    assert!(!echo_level(&host), "a rejected trigger must not drive ECHO");
}

#[test]
fn an_empty_or_out_of_range_scene_reads_as_no_echo() {
    for scene in [
        Scene::empty(),
        Scene::wall(5.0).unwrap(), // Beyond the 4 m maximum range.
    ] {
        let mut host = host(scene);
        pulse_trigger(&mut host, 20.0);
        host.advance_micros(1000.0).unwrap();

        let sensor = host.find::<HcSr04>().unwrap();
        assert_eq!(sensor.no_echo_measurements(), 1);
        assert!(sensor.last_measurement().is_none());
        assert!(!echo_level(&host), "no echo must leave ECHO low");
    }
}

#[test]
fn a_dim_reflector_is_not_detected() {
    let dim = Scene::empty().with_reflector(Reflector::new(0.5, 0.01).unwrap());
    let mut host = host(dim);
    pulse_trigger(&mut host, 20.0);
    host.advance_micros(1000.0).unwrap();
    assert_eq!(host.find::<HcSr04>().unwrap().no_echo_measurements(), 1);
}

#[test]
fn opposing_drivers_on_the_echo_line_are_reported_as_a_short() {
    let mut host = host(Scene::wall(0.5).unwrap());
    // The AVR drives both lines and holds ECHO low, as a mis-wired sketch would.
    host.write_data(DDRB, (1 << TRIG_BIT) | (1 << ECHO_BIT));
    host.write_data(PORTB, 1 << TRIG_BIT);
    host.advance_micros(20.0).unwrap();
    host.write_data(PORTB, 0);

    // ECHO is low from both sides until the sensor answers, then they short.
    let error = host.advance_micros(500.0).unwrap_err();
    match error {
        HostError::PinConflict { pin, a, b } => {
            assert_eq!(pin, Pin::digital(10));
            assert_eq!((a, b), (Drive::Low, Drive::High));
        }
        other => panic!("expected a pin conflict, got {other:?}"),
    }
}

#[test]
fn a_second_trigger_supersedes_the_measurement_already_in_flight() {
    let mut host = host(Scene::wall(0.5).unwrap());
    pulse_trigger(&mut host, 20.0);
    host.advance_micros(100.0).unwrap();
    // Retrigger well inside the first response delay, so the first ECHO rise is
    // scheduled but not yet due and must be discarded by its generation.
    pulse_trigger(&mut host, 20.0);
    host.advance_micros(5000.0).unwrap();

    let sensor = host.find::<HcSr04>().unwrap();
    assert_eq!(sensor.no_echo_measurements(), 0);
    let measurement = sensor.last_measurement().expect("measurement recorded");
    assert!((measurement.distance_m - 0.5).abs() < 1e-12);
    assert_eq!(measurement.echo_cycles, expected_pulse_cycles(0.5, 20.0));
    // Two ECHO rises were scheduled but only the live one scheduled a fall.
    assert_eq!(host.scheduler().scheduled(), 3);
}

#[test]
fn replaying_the_same_wiring_produces_the_same_timeline() {
    let run = || {
        let mut host = host(Scene::wall(0.5).unwrap());
        pulse_trigger(&mut host, 20.0);
        host.advance_micros(5000.0).unwrap();
        let sensor = host.find::<HcSr04>().unwrap();
        (
            sensor.last_measurement().unwrap(),
            host.scheduler().scheduled(),
            host.scheduler().len(),
        )
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "identical wiring must replay identically");
    // Both events of the measurement (ECHO rise and fall) were dispatched.
    assert_eq!(first.1, 2);
    assert_eq!(first.2, 0);
}
