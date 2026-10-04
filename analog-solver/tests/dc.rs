// SPDX-License-Identifier: MIT
use analog_solver::*;

struct Conductance(f64);
impl Device for Conductance {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError> {
        Ok(Linearization {
            current: voltage * self.0,
            conductance: self.0,
        })
    }
}
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn resistor_divider_and_source_current_obey_kcl_kvl_and_power() {
    let mut c = Circuit::new();
    let supply = c.node();
    let middle = c.node();
    let source = c.voltage_source(supply, Node::GROUND, 5.0).unwrap();
    let top = c.device(supply, middle, Conductance(1.0 / 1000.0)).unwrap();
    let bottom = c
        .device(middle, Node::GROUND, Conductance(1.0 / 1000.0))
        .unwrap();
    let s = c.solve(SolveOptions::default()).unwrap();
    close(s.voltage(Node::GROUND).unwrap(), 0.0);
    close(s.voltage(supply).unwrap(), 5.0);
    close(s.voltage(middle).unwrap(), 2.5);
    close(s.branch(top).unwrap().current, 0.0025);
    close(s.branch(bottom).unwrap().current, 0.0025);
    close(s.branch(source).unwrap().current, -0.0025);
    close(
        s.branch(source).unwrap().power()
            + s.branch(top).unwrap().power()
            + s.branch(bottom).unwrap().power(),
        0.0,
    );
    assert!(s.max_kcl_residual < 1e-12);
    assert!(s.max_voltage_residual < 1e-9);
}

#[test]
fn branch_orientation_and_negative_source_polarity() {
    let mut c = Circuit::new();
    let node = c.node();
    let source = c.voltage_source(Node::GROUND, node, 2.0).unwrap();
    let r = c.device(node, Node::GROUND, Conductance(0.01)).unwrap();
    let s = c.solve(SolveOptions::default()).unwrap();
    close(s.voltage(node).unwrap(), -2.0);
    close(s.branch(r).unwrap().current, -0.02);
    close(s.branch(source).unwrap().current, -0.02);
}

#[test]
fn current_source_injects_into_resistor_and_can_be_updated() {
    let mut c = Circuit::new();
    let node = c.node();
    let i = c.current_source(Node::GROUND, node, 0.001).unwrap();
    c.device(node, Node::GROUND, Conductance(0.001)).unwrap();
    close(
        c.solve(SolveOptions::default())
            .unwrap()
            .voltage(node)
            .unwrap(),
        1.0,
    );
    c.set_source(i, -0.002).unwrap();
    close(
        c.solve(SolveOptions::default())
            .unwrap()
            .voltage(node)
            .unwrap(),
        -2.0,
    );
}

#[test]
fn multiple_voltage_sources_and_nonground_source_terminals() {
    let mut c = Circuit::new();
    let a = c.node();
    let b = c.node();
    c.voltage_source(a, Node::GROUND, 3.0).unwrap();
    c.voltage_source(b, a, 2.0).unwrap();
    c.device(b, Node::GROUND, Conductance(0.001)).unwrap();
    close(
        c.solve(SolveOptions::default())
            .unwrap()
            .voltage(b)
            .unwrap(),
        5.0,
    );
}

#[test]
fn floating_nodes_are_errors_even_with_initial_zero_residual() {
    let mut c = Circuit::new();
    c.node();
    assert_eq!(
        c.solve(SolveOptions::default()).unwrap_err(),
        SolveError::Singular
    );
    let mut c = Circuit::new();
    let a = c.node();
    let b = c.node();
    c.device(a, b, Conductance(0.001)).unwrap();
    assert_eq!(
        c.solve(SolveOptions::default()).unwrap_err(),
        SolveError::Singular
    );
}

#[test]
fn conflicting_and_redundant_ideal_sources_are_errors() {
    for value in [1.0, 2.0] {
        let mut c = Circuit::new();
        let a = c.node();
        c.voltage_source(a, Node::GROUND, 1.0).unwrap();
        c.voltage_source(a, Node::GROUND, value).unwrap();
        assert_eq!(
            c.solve(SolveOptions::default()).unwrap_err(),
            SolveError::Singular
        );
    }
}

#[test]
fn invalid_nodes_sources_options_and_models_fail_explicitly() {
    let mut c = Circuit::new();
    assert!(matches!(
        c.current_source(Node(1), Node::GROUND, 1.0),
        Err(SolveError::InvalidNode(_))
    ));
    let a = c.node();
    assert!(c.voltage_source(a, Node::GROUND, f64::NAN).is_err());
    let r = c.device(a, Node::GROUND, Conductance(f64::NAN)).unwrap();
    assert!(c.set_source(r, 1.0).is_err());
    assert_eq!(
        c.solve(SolveOptions::default()).unwrap_err(),
        SolveError::NonFinite
    );
    let opts = SolveOptions {
        max_voltage_step: 0.0,
        ..SolveOptions::default()
    };
    assert!(matches!(c.solve(opts), Err(SolveError::InvalidValue(_))));
}

#[test]
fn insufficient_iteration_budget_does_not_return_a_partial_solution() {
    let mut c = Circuit::new();
    let a = c.node();
    c.voltage_source(a, Node::GROUND, 5.0).unwrap();
    let opts = SolveOptions {
        max_iterations: 1,
        ..SolveOptions::default()
    };
    assert!(matches!(
        c.solve(opts),
        Err(SolveError::NoConvergence { .. })
    ));
}

#[test]
fn ground_only_circuit_and_large_resistance() {
    let c = Circuit::new();
    close(
        c.solve(SolveOptions::default())
            .unwrap()
            .voltage(Node::GROUND)
            .unwrap(),
        0.0,
    );
    let mut c = Circuit::new();
    let node = c.node();
    c.current_source(Node::GROUND, node, 1e-12).unwrap();
    c.device(node, Node::GROUND, Conductance(1e-12)).unwrap();
    let opts = SolveOptions {
        current_tolerance: 1e-18,
        ..SolveOptions::default()
    };
    close(c.solve(opts).unwrap().voltage(node).unwrap(), 1.0);
}
