// SPDX-License-Identifier: MIT
use analog_solver::*;

#[test]
fn exact_solution_on_final_allowed_newton_step_is_accepted() {
    let mut c = Circuit::new();
    let a = c.node();
    c.voltage_source(a, Node::GROUND, 5.0).unwrap();
    let options = SolveOptions {
        max_iterations: 1,
        max_voltage_step: 10.0,
        relative_tolerance: 0.0,
        ..SolveOptions::default()
    };
    let s = c.solve(options).unwrap();
    assert_eq!(s.iterations, 1);
    assert_eq!(s.voltage(a), Some(5.0));
}

#[test]
fn invalid_relative_tolerance_cannot_hide_source_constraint_failure() {
    let mut c = Circuit::new();
    let a = c.node();
    c.voltage_source(a, Node::GROUND, 5.0).unwrap();
    for value in [-1.0, 1.0, 1e308, f64::NAN] {
        let options = SolveOptions {
            relative_tolerance: value,
            ..SolveOptions::default()
        };
        assert!(matches!(c.solve(options), Err(SolveError::InvalidValue(_))));
    }
}

struct InvalidModel;
impl Device for InvalidModel {
    fn evaluate(&self, _: f64) -> Result<Linearization, ModelError> {
        Err(ModelError::InvalidParameter("test parameter"))
    }
}
#[test]
fn model_errors_include_the_responsible_branch() {
    let mut c = Circuit::new();
    let a = c.node();
    let id = c.device(a, Node::GROUND, InvalidModel).unwrap();
    assert_eq!(
        c.solve(SolveOptions::default()).unwrap_err(),
        SolveError::Model {
            branch: id,
            error: ModelError::InvalidParameter("test parameter")
        }
    );
}

// Intentionally inconsistent derivative: residual backtracking must fail,
// rather than returning the previous iterate as a "converged" circuit.
struct WrongDerivative;
impl Device for WrongDerivative {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError> {
        Ok(Linearization {
            current: voltage,
            conductance: -1.0,
        })
    }
}
#[test]
fn backtracking_exhaustion_is_explicit_nonconvergence() {
    let mut c = Circuit::new();
    let a = c.node();
    c.current_source(Node::GROUND, a, 1.0).unwrap();
    c.device(a, Node::GROUND, WrongDerivative).unwrap();
    assert!(matches!(
        c.solve(SolveOptions::default()),
        Err(SolveError::NoConvergence { .. })
    ));
}
