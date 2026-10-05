// SPDX-License-Identifier: MIT

//! Netlist compilation: a circuit described as data must solve to the same
//! operating point as the same circuit hand-wired in Rust.

use analog_solver::{Node, SolveOptions};
use circuit_components::netlist::{
    isource, led, resistor, switch, vsource, Netlist, NetlistError, Parameters, Part, PartFactory,
    PartRegistry, PlacedPart,
};
use circuit_components::Led;

/// The reference circuit from the analog library: a 5 V Thevenin source with
/// 25 ohm output resistance driving 220 ohm and a red LED, written as
/// **explicit branches** rather than as a netlist.
fn hand_wired_led_demo() -> (f64, f64, f64) {
    use circuit_components::Resistor;
    // Identical topology to `circuit-components/examples/blink.rs`.
    let mut circuit = analog_solver::Circuit::new();
    let rail = circuit.node();
    let pin = circuit.node();
    let signal = circuit.node();
    circuit.voltage_source(rail, Node::GROUND, 5.0).unwrap();
    circuit
        .device(rail, pin, Resistor::new(25.0).unwrap())
        .unwrap();
    let series = circuit
        .device(pin, signal, Resistor::new(220.0).unwrap())
        .unwrap();
    let diode = circuit.device(signal, Node::GROUND, Led::red()).unwrap();
    let solution = circuit
        .solve(SolveOptions {
            current_tolerance: 1e-15,
            ..SolveOptions::default()
        })
        .unwrap();
    (
        solution.voltage(pin).unwrap(),
        solution.branch(diode).unwrap().voltage,
        solution.branch(series).unwrap().current,
    )
}

fn data_driven_led_demo() -> Netlist {
    Netlist::new()
        .nets(["vcc", "d9", "led_a"])
        .part(vsource("M1", 5.0, "vcc", "gnd"))
        .part(resistor("M1R", 25.0, "vcc", "d9"))
        .part(resistor("R1", 220.0, "d9", "led_a"))
        .part(led("D1", "led_a", "gnd"))
}

#[test]
fn a_data_driven_circuit_solves_to_the_hand_wired_operating_point() {
    let registry = PartRegistry::standard();
    let compiled = data_driven_led_demo().compile(&registry).expect("compiles");
    let solution = compiled
        .solve(SolveOptions {
            current_tolerance: 1e-15,
            ..SolveOptions::default()
        })
        .expect("solves");

    let (pin_voltage, led_voltage, led_current) = hand_wired_led_demo();

    let pin = compiled.node("d9").unwrap();
    let diode = compiled.branch("D1").unwrap();
    let series = compiled.branch("R1").unwrap();
    assert!((solution.voltage(pin).unwrap() - pin_voltage).abs() < 1e-12);
    assert!((solution.branch(diode).unwrap().voltage - led_voltage).abs() < 1e-12);
    assert!((solution.branch(series).unwrap().current - led_current).abs() < 1e-12);
    // The published expectation, so this test also fails on a topology mistake.
    assert!((pin_voltage - 4.709_244).abs() < 1e-4);
    assert!((led_current - 1.163_023e-2).abs() < 1e-6);
}

#[test]
fn results_are_reachable_by_name_not_by_allocation_order() {
    let registry = PartRegistry::standard();
    let compiled = data_driven_led_demo().compile(&registry).unwrap();

    assert_eq!(compiled.node("gnd"), Some(Node::GROUND));
    assert!(compiled.node("led_a").is_some());
    assert!(compiled.node("nope").is_none());
    assert!(compiled.branch("D1").is_some());
    assert!(compiled.branch("R1").is_some());
    assert!(compiled.branch("M1R").is_some());
    assert!(compiled.branch("nope").is_none());
    assert_eq!(
        compiled.references(),
        ["M1", "M1R", "R1", "D1"],
        "placement order is preserved"
    );

    // Every netlist node maps back to its net name, and ground does not get one.
    let net = compiled.node("led_a").unwrap();
    assert_eq!(compiled.net_name(net), Some("led_a"));
    assert_eq!(compiled.net_name(Node::GROUND), Some("gnd"));
}

#[test]
fn a_source_value_can_be_updated_without_recompiling_the_topology() {
    let registry = PartRegistry::standard();
    let mut compiled = Netlist::new()
        .nets(["vcc", "a"])
        .part(vsource("V1", 5.0, "vcc", "gnd"))
        .part(resistor("R1", 1000.0, "vcc", "a"))
        .part(resistor("R2", 1000.0, "a", "gnd"))
        .compile(&registry)
        .unwrap();

    let node = compiled.node("a").unwrap();
    let divider = |c: &circuit_components::CompiledCircuit| {
        c.solve(SolveOptions::default())
            .unwrap()
            .voltage(node)
            .unwrap()
    };
    assert!((divider(&compiled) - 2.5).abs() < 1e-9);

    let source = compiled.branch("V1").unwrap();
    compiled.circuit_mut().set_source(source, 3.0).unwrap();
    assert!((divider(&compiled) - 1.5).abs() < 1e-9);
}

#[test]
fn a_custom_part_type_extends_the_catalogue_as_data() {
    /// A fixed 100 ohm resistor whose value is not a parameter.
    struct FixedFactory;
    struct Fixed;
    impl PartFactory for FixedFactory {
        fn id(&self) -> &'static str {
            "fixed100"
        }
        fn terminals(&self) -> &'static [&'static str] {
            &["p", "n"]
        }
        fn build(
            &self,
            _reference: &str,
            _parameters: &Parameters,
        ) -> Result<Box<dyn Part>, NetlistError> {
            Ok(Box::new(Fixed))
        }
    }
    impl Part for Fixed {
        fn id(&self) -> &'static str {
            "fixed100"
        }
        fn terminals(&self) -> &'static [&'static str] {
            &["p", "n"]
        }
        fn stamp(
            &self,
            terminals: &[Node],
            circuit: &mut analog_solver::Circuit,
        ) -> Result<Vec<analog_solver::BranchId>, analog_solver::SolveError> {
            Ok(vec![circuit.device(
                terminals[0],
                terminals[1],
                circuit_components::Resistor::new(100.0).unwrap(),
            )?])
        }
        fn summary(&self) -> String {
            "100 ohm".into()
        }
    }

    let mut registry = PartRegistry::standard();
    registry.register(Box::new(FixedFactory));
    let compiled = Netlist::new()
        .nets(["a"])
        .part(vsource("V1", 1.0, "a", "gnd"))
        .part(
            PlacedPart::new("R1", "fixed100", Parameters::new())
                .terminal("p", "a")
                .terminal("n", "gnd"),
        )
        .compile(&registry)
        .unwrap();
    let solution = compiled.solve(SolveOptions::default()).unwrap();
    let branch = compiled.branch("R1").unwrap();
    assert!((solution.branch(branch).unwrap().current - 0.01).abs() < 1e-9);
}

#[test]
fn malformed_netlists_are_rejected_with_the_offending_name() {
    let registry = PartRegistry::standard();
    // `CompiledCircuit` is intentionally neither `Debug` nor `PartialEq`, so the
    // rejection is compared as a `Result<(), _>` instead of as a whole circuit.
    let compile = |netlist: Netlist| netlist.compile(&registry).map(|_| ());

    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(resistor("R1", 100.0, "a", "typo"))
        ),
        Err(NetlistError::UnknownNet("typo".into()))
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a", "a"])
                .part(resistor("R1", 100.0, "a", "gnd"))
        ),
        Err(NetlistError::DuplicateNet("a".into()))
    );
    assert_eq!(
        compile(Netlist::new().nets(["a"]).part(PlacedPart::new(
            "X1",
            "no.such.part",
            Parameters::new()
        ))),
        Err(NetlistError::UnknownPart("no.such.part".into()))
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(resistor("R1", 100.0, "a", "gnd"))
                .part(resistor("R1", 200.0, "a", "gnd"))
        ),
        Err(NetlistError::DuplicateReference("R1".into()))
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(vsource("V1", 1.0, "a", "gnd"))
                // `led` needs both `a` and `k`; only the anode is wired.
                .part(PlacedPart::new("D1", "led", Parameters::new()).terminal("a", "a"))
        ),
        Err(NetlistError::MissingTerminal {
            reference: "D1".into(),
            terminal: "k".into()
        })
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(led("D1", "a", "gnd").terminal("a", "gnd"))
        ),
        Err(NetlistError::DuplicateTerminal {
            reference: "D1".into(),
            terminal: "a".into()
        })
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(vsource("V1", 1.0, "a", "gnd"))
                .part(resistor("R1", 100.0, "a", "gnd").terminal("wiper", "gnd"))
        ),
        Err(NetlistError::UnknownTerminal {
            reference: "R1".into(),
            terminal: "wiper".into()
        })
    );
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(resistor("R1", -100.0, "a", "gnd"))
        ),
        Err(NetlistError::InvalidParameter {
            reference: "R1".into()
        })
    );
    // A misspelled LED parameter must not silently fall back to the default.
    assert_eq!(
        compile(
            Netlist::new()
                .nets(["a"])
                .part(vsource("V1", 1.0, "a", "gnd"))
                .part(
                    PlacedPart::new("D1", "led", Parameters::from([("sat_current", 1e-20)]))
                        .terminal("a", "a")
                        .terminal("k", "gnd")
                )
        ),
        Err(NetlistError::InvalidParameter {
            reference: "D1".into()
        })
    );
}

#[test]
fn a_current_source_biases_the_nets_it_connects() {
    let registry = PartRegistry::standard();
    let compiled = Netlist::new()
        .nets(["a"])
        .part(isource("I1", 1e-3, "gnd", "a"))
        .part(resistor("R1", 1000.0, "a", "gnd"))
        .compile(&registry)
        .unwrap();
    let solution = compiled.solve(SolveOptions::default()).unwrap();
    let node = compiled.node("a").unwrap();
    assert!((solution.voltage(node).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn the_led_preset_and_its_override_are_both_available_as_data() {
    let registry = PartRegistry::standard();
    // `led.red` is a preset id; supplying an override still wins.
    let compiled = Netlist::new()
        .nets(["a"])
        .part(vsource("V1", 2.0, "a", "gnd"))
        .part(
            PlacedPart::new(
                "D1",
                "led.red",
                Parameters::from([("saturation_current", 1e-18)]),
            )
            .terminal("a", "a")
            .terminal("k", "gnd"),
        )
        .compile(&registry)
        .unwrap();
    let solution = compiled.solve(SolveOptions::default()).unwrap();
    let diode = compiled.branch("D1").unwrap();
    // A larger Is conducts earlier, so the drop is below the illustrative 2.14 V.
    assert!(solution.branch(diode).unwrap().voltage < 2.14);
    assert!(solution.branch(diode).unwrap().voltage > 1.5);
}

#[test]
fn a_switch_selects_between_two_levels_without_changing_the_topology() {
    let registry = PartRegistry::standard();
    // A divider with a switch across the lower leg. Open, the node sits halfway;
    // closed, the 50 mohm contact swamps the 10k leg and the node collapses.
    let midpoint = |closed: bool| {
        let mut netlist = Netlist::new()
            .nets(["vcc", "mid"])
            .part(vsource("V1", 5.0, "vcc", "gnd"))
            .part(resistor("R1", 10_000.0, "vcc", "mid"))
            .part(resistor("R2", 10_000.0, "mid", "gnd"))
            .part(switch("SW1", "mid", "gnd"));
        netlist
            .set_parameter("SW1", "closed", if closed { 1.0 } else { 0.0 })
            .expect("SW1 is placed");
        let compiled = netlist.compile(&registry).expect("compiles");
        let node = compiled.node("mid").expect("declared net");
        compiled
            .solve(SolveOptions {
                current_tolerance: 1e-15,
                ..SolveOptions::default()
            })
            .expect("solves")
            .voltage(node)
            .expect("nonground node")
    };

    let open = midpoint(false);
    // An open switch is not a perfect disconnect: its 100 Mohm insulation sits
    // in parallel with the lower leg, so the divider is pulled down by about
    // 125 uV. Asserting the parallel-combination prediction checks that the
    // insulation resistance is really in the solve rather than being treated as
    // an ideal break.
    let loaded = 10_000.0 * 100e6 / (10_000.0 + 100e6);
    let predicted = 5.0 * loaded / (10_000.0 + loaded);
    assert!(
        (open - predicted).abs() < 1e-9,
        "open divider at {open} V, predicted {predicted} V"
    );
    assert!(open < 2.5, "the open switch can only pull the node down");

    let closed = midpoint(true);
    assert!(closed < 1e-3, "closed switch left {closed} V");
    assert!(closed > 0.0, "a closed contact cannot invert the sign");
}

#[test]
fn setting_a_parameter_on_an_absent_part_is_reported() {
    let mut netlist = Netlist::new()
        .nets(["a"])
        .part(resistor("R1", 100.0, "a", "gnd"));
    assert_eq!(
        netlist.set_parameter("R9", "closed", 1.0),
        Err(NetlistError::UnknownReference("R9".into()))
    );
    assert_eq!(netlist.parameter("R1", "ohms"), Some(100.0));
    assert_eq!(netlist.parameter("R1", "closed"), None);
}
