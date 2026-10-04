// SPDX-License-Identifier: MIT
//! Headless analog circuit demonstration, independent of the AVR and GUI.
use analog_solver::{Circuit, Node, SolveOptions};
use circuit_components::{Led, Resistor};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reversed = std::env::args().any(|arg| arg == "--reverse");
    let mut c = Circuit::new();
    let rail = c.node();
    let pin = c.node();
    let signal = c.node();
    let source = c.voltage_source(rail, Node::GROUND, 5.0)?;
    let driver = c.device(rail, pin, Resistor::new(25.0)?)?;
    let resistor = c.device(pin, signal, Resistor::new(220.0)?)?;
    let (anode, cathode) = if reversed {
        (Node::GROUND, signal)
    } else {
        (signal, Node::GROUND)
    };
    let led = c.device(anode, cathode, Led::red())?;
    let options = SolveOptions {
        current_tolerance: 1e-15,
        ..SolveOptions::default()
    };
    let s = c.solve(options)?;
    let point = s.branch(led).unwrap();
    println!(
        "reversed={reversed}: Vpin={:.6} V, Vled(A-K)={:+.6} V, Iled={:+.6e} A, brightness={:.6}",
        s.voltage(pin).unwrap(),
        point.voltage,
        point.current,
        Led::red().brightness(point.current)
    );
    let power_balance: f64 = [source, driver, resistor, led]
        .iter()
        .map(|id| s.branch(*id).unwrap().power())
        .sum();
    println!(
        "KCL residual={:.3e} A, power balance={:+.3e} W, {} Newton steps",
        s.max_kcl_residual, power_balance, s.iterations
    );
    Ok(())
}
