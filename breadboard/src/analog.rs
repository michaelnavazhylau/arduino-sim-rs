// SPDX-License-Identifier: MIT

//! Analog coupling between a netlist and the AVR's pins and ADC.
//!
//! A digital pin is not an ideal switch, and an analog input is not a number a
//! host invents. This module closes the loop in both directions:
//!
//! * **MCU -> circuit.** Each bound pin contributes a Thevenin driver — a
//!   source and a series resistance — reflecting its current drive mode. A pin
//!   in high impedance contributes *nothing*, because an undriven pin really is
//!   undriven; approximating it with a huge resistor would make a floating input
//!   look like a measurement.
//! * **Circuit -> MCU.** Solved node voltages feed the ADC mux channels, and a
//!   bound input pin's pad voltage is resolved through the AVR's input
//!   thresholds and written back to `PINx`.
//!
//! Topology is reused: a `Low`/`High` change only rewrites a source value, and
//! only a change of *driver shape* (driven, pulled up, or absent) recompiles.

use crate::pin::{Pin, Port};
use analog_solver::{BranchId, BranchPoint, Circuit, Node, Solution, SolveError, SolveOptions};
use avr_port_tests::board::{Board, PinState};
use avr_port_tests::runtime::Handle;
use avr_port_tests::{Backend, Runtime, Value};
use circuit_components::netlist::{
    CompiledCircuit, Netlist, NetlistError, Parameters, Part, PartFactory, PartRegistry, PlacedPart,
};
use circuit_components::Resistor;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Number of single-ended ADC channels the ATmega328P mux exposes here.
pub const ADC_CHANNELS: usize = 8;
/// `DIDR0`: each bit disables the digital input buffer of the matching ADC pin.
const DIDR0: usize = 0x7e;

/// Part type for a Thevenin-equivalent MCU pin driver.
///
/// Terminals are `out` (the pin's net) and `return` (normally ground). The
/// principal branch is the source, so a host that only changes the driven level
/// can rewrite it in place with `Circuit::set_source`.
pub struct McuDriverFactory;

impl PartFactory for McuDriverFactory {
    fn id(&self) -> &'static str {
        "mcu.driver"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["out", "return"]
    }

    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError> {
        let volts = parameters.get_or("volts", f64::NAN);
        let ohms = parameters.get_or("ohms", f64::NAN);
        if !volts.is_finite() || !ohms.is_finite() || ohms <= 0.0 {
            return Err(NetlistError::InvalidParameter {
                reference: reference.to_string(),
            });
        }
        Ok(Box::new(McuDriverPart { volts, ohms }))
    }
}

struct McuDriverPart {
    volts: f64,
    ohms: f64,
}

impl Part for McuDriverPart {
    fn id(&self) -> &'static str {
        "mcu.driver"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["out", "return"]
    }

    fn stamp(
        &self,
        terminals: &[Node],
        circuit: &mut Circuit,
    ) -> Result<Vec<BranchId>, SolveError> {
        let mid = circuit.node();
        let source = circuit.voltage_source(mid, terminals[1], self.volts)?;
        let device = Resistor::new(self.ohms)
            .map_err(|_| SolveError::InvalidValue("MCU driver resistance"))?;
        let series = circuit.device(mid, terminals[0], device)?;
        Ok(vec![source, series])
    }

    fn summary(&self) -> String {
        format!("{:.3} V through {:.1} ohm", self.volts, self.ohms)
    }
}

/// Which net each Uno pin is wired to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wiring {
    bindings: Vec<(Pin, String)>,
}

impl Wiring {
    /// No pins bound.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `pin` to `net`.
    pub fn bind(mut self, pin: Pin, net: &str) -> Self {
        self.bindings.push((pin, net.to_string()));
        self
    }

    /// Pin/net pairs in binding order.
    pub fn bindings(&self) -> &[(Pin, String)] {
        &self.bindings
    }
}

/// Topology class of a pin driver, which decides whether a recompile is needed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DriverShape {
    /// High impedance: the pin contributes no branch.
    Absent,
    /// A source behind the output resistance.
    Driven,
    /// A source behind the pull-up resistance.
    PullUp,
}

impl DriverShape {
    fn of(state: PinState) -> Self {
        match state {
            PinState::Low | PinState::High => Self::Driven,
            PinState::InputPullUp => Self::PullUp,
            PinState::Input => Self::Absent,
        }
    }
}

/// Why a coupling could not be set up or updated.
#[derive(Clone, Debug, PartialEq)]
pub enum CouplingError {
    /// The netlist itself was rejected.
    Netlist(NetlistError),
    /// A bound net is not declared by the netlist.
    UnknownNet { pin: Pin, net: String },
    /// The same pin was bound twice.
    DuplicateBinding(Pin),
    /// A topology update or solve failed outright.
    Solve(SolveError),
}

impl fmt::Display for CouplingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Netlist(error) => write!(f, "analog coupling: {error}"),
            Self::UnknownNet { pin, net } => {
                write!(
                    f,
                    "analog coupling: pin {pin} is bound to undeclared net {net:?}"
                )
            }
            Self::DuplicateBinding(pin) => {
                write!(f, "analog coupling: pin {pin} is bound more than once")
            }
            Self::Solve(error) => write!(f, "analog coupling: {error}"),
        }
    }
}

impl Error for CouplingError {}

/// Result of one coupling update.
#[derive(Clone, Debug, PartialEq)]
pub enum CouplingOutcome {
    /// The circuit solved and every reading is valid.
    Solved,
    /// The topology has no unique solution, typically an undriven input net.
    /// The error is preserved rather than replaced by a plausible number.
    Indeterminate(SolveError),
}

/// Voltages and levels read out of the last solve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnalogReading {
    outcome: Option<CouplingOutcome>,
    net_voltages: BTreeMap<String, f64>,
    branch_points: BTreeMap<String, BranchPoint>,
    adc_voltages: Vec<Option<f64>>,
    digital_levels: BTreeMap<Pin, Option<bool>>,
    max_kcl_residual: f64,
}

impl AnalogReading {
    /// Outcome of the solve this reading came from.
    pub fn outcome(&self) -> CouplingOutcome {
        self.outcome
            .clone()
            .unwrap_or(CouplingOutcome::Indeterminate(SolveError::InvalidValue(
                "no solve has run",
            )))
    }

    /// Solved voltage of a net, when the solve succeeded.
    pub fn net_voltage(&self, net: &str) -> Option<f64> {
        self.net_voltages.get(net).copied()
    }

    /// Solved voltage and current of a part's principal branch.
    pub fn branch(&self, reference: &str) -> Option<BranchPoint> {
        self.branch_points.get(reference).copied()
    }

    /// Solved voltage presented to an ADC channel.
    pub fn adc_voltage(&self, channel: usize) -> Option<f64> {
        self.adc_voltages.get(channel).copied().flatten()
    }

    /// Resolved digital level of a bound pin, or `None` in the threshold band.
    pub fn digital_level(&self, pin: Pin) -> Option<bool> {
        self.digital_levels.get(&pin).copied().flatten()
    }

    /// Every solved net voltage.
    pub fn net_voltages(&self) -> &BTreeMap<String, f64> {
        &self.net_voltages
    }

    /// Maximum KCL imbalance at the accepted operating point, in amperes.
    pub fn max_kcl_residual(&self) -> f64 {
        self.max_kcl_residual
    }
}

/// An analog circuit coupled to specific Uno pins.
pub struct AnalogCoupling {
    netlist: Netlist,
    registry: PartRegistry,
    wiring: Wiring,
    supply_volts: f64,
    output_ohms: f64,
    pullup_ohms: f64,
    options: SolveOptions,
    shapes: Vec<DriverShape>,
    modes: Vec<PinState>,
    compiled: Option<CompiledCircuit>,
    reading: AnalogReading,
    solves: u64,
}

impl fmt::Debug for AnalogCoupling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The registry holds trait objects and the compiled topology holds a
        // solver, so the summary reports what a reader needs to identify a
        // coupling rather than dumping internals.
        f.debug_struct("AnalogCoupling")
            .field("nets", &self.netlist.net_names())
            .field("bindings", &self.wiring.bindings())
            .field("modes", &self.modes)
            .field("solves", &self.solves)
            .field("outcome", &self.reading.outcome())
            .finish_non_exhaustive()
    }
}

impl AnalogCoupling {
    /// Couple `netlist` to the pins named in `wiring`.
    pub fn new(netlist: Netlist, wiring: Wiring) -> Result<Self, CouplingError> {
        let mut registry = PartRegistry::standard();
        registry.register(Box::new(McuDriverFactory));
        let mut seen = Vec::new();
        for (pin, net) in wiring.bindings() {
            if seen.contains(pin) {
                return Err(CouplingError::DuplicateBinding(*pin));
            }
            seen.push(*pin);
            let known =
                net == netlist.ground_net() || netlist.net_names().iter().any(|name| name == net);
            if !known {
                return Err(CouplingError::UnknownNet {
                    pin: *pin,
                    net: net.clone(),
                });
            }
        }
        let count = wiring.bindings().len();
        Ok(Self {
            netlist,
            registry,
            wiring,
            supply_volts: 5.0,
            output_ohms: 25.0,
            pullup_ohms: 30_000.0,
            options: SolveOptions {
                current_tolerance: 1e-15,
                ..SolveOptions::default()
            },
            shapes: vec![DriverShape::Absent; count],
            // A reset AVR presents every pin as a floating input.
            modes: vec![PinState::Input; count],
            compiled: None,
            reading: AnalogReading {
                adc_voltages: vec![None; ADC_CHANNELS],
                ..AnalogReading::default()
            },
            solves: 0,
        })
    }

    /// Logic supply rail the drivers pull to.
    pub fn with_supply(mut self, volts: f64) -> Self {
        self.supply_volts = volts;
        self
    }

    /// Driver impedances, illustrative rather than measured pad characteristics.
    pub fn with_driver(mut self, output_ohms: f64, pullup_ohms: f64) -> Self {
        self.output_ohms = output_ohms;
        self.pullup_ohms = pullup_ohms;
        self
    }

    /// Solver settings used for every solve.
    pub fn with_solve_options(mut self, options: SolveOptions) -> Self {
        self.options = options;
        self
    }

    /// Register an extra part type, so a host can extend the catalogue.
    pub fn register_part(&mut self, factory: Box<dyn PartFactory>) {
        self.registry.register(factory);
    }

    /// The electrical netlist, without the MCU drivers this module adds.
    pub fn netlist(&self) -> &Netlist {
        &self.netlist
    }

    /// The pin bindings.
    pub fn wiring(&self) -> &Wiring {
        &self.wiring
    }

    /// The most recent reading.
    pub fn reading(&self) -> &AnalogReading {
        &self.reading
    }

    /// Solves performed so far; a cache hit does not count.
    pub fn solves(&self) -> u64 {
        self.solves
    }

    /// Bring the circuit up to date with the AVR's pin modes and apply readings.
    ///
    /// Recompiles only when a driver changes shape, so a pin toggling between
    /// `Low` and `High` reuses the existing topology.
    pub fn update(
        &mut self,
        backend: &dyn Backend,
        runtime: &mut Runtime,
        board: &Board,
    ) -> Result<CouplingOutcome, CouplingError> {
        let modes: Vec<PinState> = self
            .wiring
            .bindings()
            .iter()
            .map(|(pin, _)| {
                let (port, bit) = pin.port_bit();
                board.pin_state(backend, runtime, port_handle(board, port), bit)
            })
            .collect();
        let shapes: Vec<DriverShape> = modes.iter().map(|mode| DriverShape::of(*mode)).collect();

        if self.compiled.is_none() || shapes != self.shapes {
            self.rebuild(&modes)?;
            self.shapes = shapes;
        } else if modes != self.modes {
            self.repoint(&modes)?;
        } else {
            // Nothing electrical changed, so nothing is pushed back to the AVR.
            self.modes = modes;
            return Ok(self.reading.outcome());
        }
        self.modes = modes;
        self.apply(backend, runtime, board);
        Ok(self.reading.outcome())
    }

    /// Rewrite driver source values after a `Low`/`High` change.
    fn repoint(&mut self, modes: &[PinState]) -> Result<(), CouplingError> {
        let Some(compiled) = self.compiled.as_mut() else {
            return Ok(());
        };
        for (index, (pin, _)) in self.wiring.bindings().iter().enumerate() {
            if DriverShape::of(modes[index]) != DriverShape::Driven {
                continue;
            }
            let reference = driver_reference(*pin);
            let Some(branch) = compiled.branch(&reference) else {
                continue;
            };
            let volts = driven_volts(modes[index], self.supply_volts);
            compiled
                .circuit_mut()
                .set_source(branch, volts)
                .map_err(CouplingError::Solve)?;
        }
        self.solve()
    }

    /// Rebuild the topology with a driver per driven pin and solve it.
    fn rebuild(&mut self, modes: &[PinState]) -> Result<(), CouplingError> {
        let ground = self.netlist.ground_net().to_string();
        let mut netlist = self.netlist.clone();
        for (index, (pin, net)) in self.wiring.bindings().iter().enumerate() {
            let mode = modes[index];
            if DriverShape::of(mode) == DriverShape::Absent {
                continue;
            }
            let (volts, ohms) = match DriverShape::of(mode) {
                DriverShape::Driven => (driven_volts(mode, self.supply_volts), self.output_ohms),
                _ => (self.supply_volts, self.pullup_ohms),
            };
            netlist = netlist.part(
                PlacedPart::new(
                    &driver_reference(*pin),
                    "mcu.driver",
                    Parameters::from([("volts", volts), ("ohms", ohms)]),
                )
                .terminal("out", net)
                .terminal("return", &ground),
            );
        }
        let compiled = netlist
            .compile(&self.registry)
            .map_err(CouplingError::Netlist)?;
        self.compiled = Some(compiled);
        self.solve()
    }

    /// Solve the current topology and record the reading.
    fn solve(&mut self) -> Result<(), CouplingError> {
        let Some(compiled) = self.compiled.as_ref() else {
            return Ok(());
        };
        self.solves += 1;
        let ground = self.netlist.ground_net().to_string();
        match compiled.solve(self.options) {
            Ok(solution) => {
                self.reading = self.read(&solution, &ground);
                Ok(())
            }
            Err(error) => {
                // A floating net is a normal breadboard condition, not a broken
                // description, so the error is recorded rather than raised.
                self.reading = AnalogReading {
                    outcome: Some(CouplingOutcome::Indeterminate(error)),
                    adc_voltages: vec![None; ADC_CHANNELS],
                    ..AnalogReading::default()
                };
                Ok(())
            }
        }
    }

    fn read(&self, solution: &Solution, ground: &str) -> AnalogReading {
        let Some(compiled) = self.compiled.as_ref() else {
            return AnalogReading::default();
        };
        let mut net_voltages = BTreeMap::new();
        for net in
            std::iter::once(ground).chain(self.netlist.net_names().iter().map(String::as_str))
        {
            if let Some(node) = compiled.node(net) {
                if let Some(voltage) = solution.voltage(node) {
                    net_voltages.insert(net.to_string(), voltage);
                }
            }
        }
        let mut branch_points = BTreeMap::new();
        for reference in compiled.references() {
            if let Some(branch) = compiled.branch(reference) {
                if let Some(point) = solution.branch(branch) {
                    branch_points.insert(reference.clone(), point);
                }
            }
        }

        let mut adc_voltages = vec![None; ADC_CHANNELS];
        let mut digital_levels = BTreeMap::new();
        for (pin, net) in self.wiring.bindings() {
            let Some(&voltage) = net_voltages.get(net) else {
                continue;
            };
            if let Pin::Analog(channel) = pin {
                adc_voltages[channel.index() as usize] = Some(voltage);
            }
            digital_levels.insert(*pin, threshold(voltage, self.supply_volts));
        }
        AnalogReading {
            outcome: Some(CouplingOutcome::Solved),
            net_voltages,
            branch_points,
            adc_voltages,
            digital_levels,
            max_kcl_residual: solution.max_kcl_residual,
        }
    }

    /// Push readings back into the AVR: ADC mux channels and input pad levels.
    fn apply(&mut self, backend: &dyn Backend, runtime: &mut Runtime, board: &Board) {
        let channels: Vec<Value> = self
            .reading
            .adc_voltages
            .iter()
            .map(|voltage| Value::Number(voltage.unwrap_or(0.0)))
            .collect();
        backend.set(runtime, board.adc, "channelValues", Value::array(channels));

        for (index, (pin, _)) in self.wiring.bindings().iter().enumerate() {
            // Only a pin the AVR is actually driving keeps its port value: a
            // driven output must not be overwritten by the circuit it feeds.
            // A high-impedance *or pulled-up* pin has its pad level set by the
            // surrounding circuit, so that level is what the AVR reads back.
            if DriverShape::of(self.modes[index]) == DriverShape::Driven {
                continue;
            }
            if let Pin::Analog(channel) = pin {
                if board.read_bit(backend, DIDR0, channel.index()) {
                    continue;
                }
            }
            let Some(high) = self.reading.digital_levels.get(pin).copied().flatten() else {
                // The threshold band is indeterminate: retain the previous
                // sample rather than rounding it to a valid logic level.
                continue;
            };
            let (port, bit) = pin.port_bit();
            board.set_input(backend, runtime, port_handle(board, port), bit, high);
        }
    }
}

/// Reference designator of the driver a bound pin contributes.
fn driver_reference(pin: Pin) -> String {
    format!("MCU_{pin}")
}

fn driven_volts(state: PinState, supply_volts: f64) -> f64 {
    if state == PinState::High {
        supply_volts
    } else {
        0.0
    }
}

/// AVR input thresholds: <= 0.3 Vcc is low, >= 0.6 Vcc is high, else undefined.
fn threshold(voltage: f64, supply_volts: f64) -> Option<bool> {
    if voltage <= 0.3 * supply_volts {
        Some(false)
    } else if voltage >= 0.6 * supply_volts {
        Some(true)
    } else {
        None
    }
}

fn port_handle(board: &Board, port: Port) -> Handle {
    match port {
        Port::B => board.portb,
        Port::C => board.portc,
        Port::D => board.portd,
    }
}
