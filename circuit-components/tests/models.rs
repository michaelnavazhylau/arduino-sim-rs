// SPDX-License-Identifier: MIT
use analog_solver::*;
use circuit_components::*;

#[test]
fn resistor_obeys_signed_ohms_law_and_absorbs_power_in_both_directions() {
    let r = Resistor::new(220.0).unwrap();
    assert_eq!(r.ohms(), 220.0);
    for voltage in [-5.0, 0.0, 5.0] {
        let iv = r.evaluate(voltage).unwrap();
        assert!((iv.current - voltage / 220.0).abs() < 1e-14);
        assert!((iv.conductance - 1.0 / 220.0).abs() < 1e-14);
        assert!(voltage * iv.current >= 0.0);
    }
}

#[test]
fn invalid_and_nonfinite_parameters_are_rejected() {
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e-320] {
        assert!(Resistor::new(value).is_err());
    }
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        for params in [
            LedParameters {
                saturation_current: value,
                ..LedParameters::default()
            },
            LedParameters {
                ideality_factor: value,
                ..LedParameters::default()
            },
            LedParameters {
                thermal_voltage: value,
                ..LedParameters::default()
            },
            LedParameters {
                shunt_resistance: value,
                ..LedParameters::default()
            },
            LedParameters {
                nominal_current: value,
                ..LedParameters::default()
            },
        ] {
            assert!(Led::new(params).is_err());
        }
    }
}

#[test]
fn diode_directionality_forward_drop_and_optical_current_mapping() {
    let led = Led::red();
    let scale = led.parameters().ideality_factor * led.parameters().thermal_voltage;
    let forward_voltage = scale * (0.010 / led.parameters().saturation_current + 1.0).ln();
    assert!((forward_voltage - 2.143).abs() < 0.001);
    assert!((led.evaluate(forward_voltage).unwrap().current - 0.010).abs() < 1e-10);
    assert!(led.evaluate(-5.0).unwrap().current.abs() < 6e-12);
    assert_eq!(led.evaluate(0.0).unwrap().current, 0.0);
    assert_eq!(led.brightness(-0.010), 0.0);
    assert_eq!(led.brightness(0.010), 0.5);
    assert_eq!(led.brightness(1.0), 1.0);
    // Brightness clips, but the electrical model does not silently clamp current.
    assert!(led.evaluate(2.3).unwrap().current > 0.020);
}

#[test]
fn analytic_jacobian_matches_central_difference() {
    for model in [&Resistor::new(220.0).unwrap() as &dyn Device, &Led::red()] {
        for voltage in [-5.0, 0.0, 1.7, 2.2] {
            let iv = model.evaluate(voltage).unwrap();
            let h = 1e-6;
            let derivative = (model.evaluate(voltage + h).unwrap().current
                - model.evaluate(voltage - h).unwrap().current)
                / (2.0 * h);
            assert!((derivative - iv.conductance).abs() <= iv.conductance.abs() * 1e-6 + 1e-18);
        }
    }
}

#[test]
fn exponential_overflow_is_an_error_not_a_fake_current_cap() {
    assert!(matches!(
        Led::red().evaluate(100.0),
        Err(ModelError::OutOfRange(_))
    ));
    assert!(Resistor::new(1.0).unwrap().evaluate(f64::NAN).is_err());
}

fn series(volts: f64, ohms: f64, reverse: bool) -> (Solution, Node, BranchId, BranchId, BranchId) {
    let mut c = Circuit::new();
    let supply = c.node();
    let anode = c.node();
    let source = c.voltage_source(supply, Node::GROUND, volts).unwrap();
    let resistor = c
        .device(supply, anode, Resistor::new(ohms).unwrap())
        .unwrap();
    let (p, n) = if reverse {
        (Node::GROUND, anode)
    } else {
        (anode, Node::GROUND)
    };
    let diode = c.device(p, n, Led::red()).unwrap();
    let options = SolveOptions {
        current_tolerance: 1e-15,
        ..SolveOptions::default()
    };
    (c.solve(options).unwrap(), anode, source, resistor, diode)
}

#[test]
fn nonlinear_operating_point_matches_independent_bisection_and_conserves_power() {
    for volts in [0.0, 1.0, 3.3, 5.0, 12.0] {
        let (s, anode, source, resistor, diode) = series(volts, 220.0, false);
        let mut lo = 0.0;
        let mut hi = volts;
        // Independent scalar root, NOT a second use of MNA/Newton.
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            let current = Led::red().evaluate(mid).unwrap().current;
            if current > (volts - mid) / 220.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        assert!((s.voltage(anode).unwrap() - (lo + hi) / 2.0).abs() < 1e-7);
        let r = s.branch(resistor).unwrap();
        let d = s.branch(diode).unwrap();
        let v = s.branch(source).unwrap();
        assert!((r.current - d.current).abs() < 1e-9);
        assert!((r.voltage + d.voltage - volts).abs() < 1e-8);
        assert!((r.power() + d.power() + v.power()).abs() < 1e-8);
        assert!(s.max_kcl_residual < 1e-9);
    }
}

#[test]
fn reversing_led_or_supply_blocks_current_and_increasing_resistance_reduces_it() {
    let (s, _, _, r, d) = series(5.0, 220.0, true);
    assert!((s.branch(d).unwrap().voltage + 5.0).abs() < 1e-8);
    assert!(s.branch(r).unwrap().current.abs() < 6e-12);
    assert!(s.branch(d).unwrap().current < 0.0);
    assert_eq!(Led::red().brightness(s.branch(d).unwrap().current), 0.0);
    let (s, _, _, _, d) = series(-5.0, 220.0, false);
    assert!(s.branch(d).unwrap().current.abs() < 6e-12);
    let (s, _, _, _, d) = series(5.0, 220.0, false);
    let forward = s.branch(d).unwrap().current;
    assert!((0.012..0.014).contains(&forward));
    let (s, _, _, _, d) = series(5.0, 1000.0, false);
    assert!(s.branch(d).unwrap().current < forward / 3.0);
}

#[test]
fn antiparallel_diodes_choose_the_forward_oriented_branch() {
    for voltage in [-5.0, 5.0] {
        let mut c = Circuit::new();
        let supply = c.node();
        let middle = c.node();
        c.voltage_source(supply, Node::GROUND, voltage).unwrap();
        c.device(supply, middle, Resistor::new(220.0).unwrap())
            .unwrap();
        let forward = c.device(middle, Node::GROUND, Led::red()).unwrap();
        let backward = c.device(Node::GROUND, middle, Led::red()).unwrap();
        let s = c.solve(SolveOptions::default()).unwrap();
        let (on, off) = if voltage > 0.0 {
            (forward, backward)
        } else {
            (backward, forward)
        };
        assert!(s.branch(on).unwrap().current > 0.01);
        assert!(s.branch(off).unwrap().current.abs() < 6e-12);
    }
}

#[test]
fn led_colour_presets_have_distinct_forward_voltages() {
    // Forward voltage at the 10 mA operating point these presets are normalised
    // to, found by bisection on the analytic model rather than restating it.
    fn forward_voltage(parameters: LedParameters) -> f64 {
        let led = Led::new(parameters).unwrap();
        let (mut lo, mut hi) = (0.0_f64, 5.0_f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if led.evaluate(mid).unwrap().current < 0.010 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    let red = forward_voltage(LedParameters::red());
    let green = forward_voltage(LedParameters::green());
    let blue = forward_voltage(LedParameters::blue());
    assert!(
        red < green && green < blue,
        "red {red}, green {green}, blue {blue}"
    );
    assert!((1.9..2.3).contains(&red), "red {red}");
    assert!((2.4..2.8).contains(&green), "green {green}");
    assert!((2.8..3.2).contains(&blue), "blue {blue}");

    // The presets differ in exactly one physical parameter, so the ideality
    // factor and thermal voltage stay identical across colours.
    for (preset, expected) in [
        (LedParameters::red(), 1e-20),
        (LedParameters::green(), 1.4e-24),
        (LedParameters::blue(), 6.1e-28),
    ] {
        assert_eq!(preset.saturation_current, expected);
        assert_eq!(
            preset.ideality_factor,
            LedParameters::default().ideality_factor
        );
    }
}

#[test]
fn a_switch_is_a_finite_resistance_in_both_states() {
    let open = Switch::button();
    let closed = Switch::button().with_closed(true);
    assert!(!open.is_closed() && closed.is_closed());

    // Open: insulation resistance, so nanoamps at 5 V.
    let iv = open.evaluate(5.0).unwrap();
    assert!((iv.current - 5.0 / 100e6).abs() < 1e-12, "{}", iv.current);
    // Closed: contact resistance, so the same voltage drives 100 A if unlimited.
    let iv = closed.evaluate(5.0).unwrap();
    assert!((iv.current - 5.0 / 0.05).abs() < 1e-6, "{}", iv.current);
    assert!((iv.conductance - 20.0).abs() < 1e-9);

    // Ohmic and symmetric: a switch does not rectify.
    assert!(open.evaluate(-5.0).unwrap().current < 0.0);
    assert!(closed.evaluate(-5.0).unwrap().current < 0.0);
    assert!(open.evaluate(0.0).unwrap().current == 0.0);
}

#[test]
fn a_switch_rejects_physically_contradictory_parameters() {
    for parameters in [
        SwitchParameters {
            closed_ohms: 100.0,
            open_ohms: 10.0,
        },
        SwitchParameters {
            closed_ohms: 0.0,
            open_ohms: 1e6,
        },
        SwitchParameters {
            closed_ohms: -1.0,
            open_ohms: 1e6,
        },
        SwitchParameters {
            closed_ohms: f64::NAN,
            open_ohms: 1e6,
        },
        SwitchParameters {
            closed_ohms: 0.05,
            open_ohms: f64::INFINITY,
        },
    ] {
        assert!(Switch::new(parameters).is_err(), "{parameters:?}");
    }
}

#[test]
fn toggling_a_switch_changes_conductance_not_parameters() {
    let mut switch = Switch::button();
    let before = switch.parameters();
    let open_conductance = switch.evaluate(1.0).unwrap().conductance;
    switch.set_closed(true);
    let closed_conductance = switch.evaluate(1.0).unwrap().conductance;
    assert_eq!(switch.parameters(), before);
    // Nine decades of conductance between the two states.
    assert!(closed_conductance / open_conductance > 1e9);
}
