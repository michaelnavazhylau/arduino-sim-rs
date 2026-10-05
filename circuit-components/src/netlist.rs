// SPDX-License-Identifier: MIT

//! Declarative netlists: circuits as data, compiled into a solver topology.
//!
//! A [`Netlist`] names nets, places parts with parameters and connects *named*
//! terminals. [`Netlist::compile`] validates the whole description and produces
//! a [`CompiledCircuit`] that maps results back by reference designator, so host
//! code asks for `branch("D1")` instead of remembering positional branch indices.
//!
//! Parts come from a [`PartRegistry`]. Adding a component is a new factory plus
//! data, not an edit to every circuit that might use it — which is what makes a
//! netlist a description rather than a program.
//!
//! This module is purely electrical. Board wiring, MCU pin drivers and
//! sensor-specific parts belong to the host, and are registered here rather than
//! hard-coded.

use crate::{Led, LedParameters, Resistor};
use analog_solver::{BranchId, Circuit, Node, Solution, SolveError, SolveOptions};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// Why a netlist could not be built or compiled.
#[derive(Clone, Debug, PartialEq)]
pub enum NetlistError {
    /// A connection names a net the netlist does not declare.
    UnknownNet(String),
    /// The same net name was declared twice.
    DuplicateNet(String),
    /// A part id is not in the registry.
    UnknownPart(String),
    /// The same reference designator was placed twice.
    DuplicateReference(String),
    /// A part instance left a terminal unconnected.
    MissingTerminal { reference: String, terminal: String },
    /// A connection names a terminal the part does not have.
    UnknownTerminal { reference: String, terminal: String },
    /// A terminal was connected to more than one net.
    DuplicateTerminal { reference: String, terminal: String },
    /// A part's parameters were rejected by its model.
    InvalidParameter { reference: String },
}

impl fmt::Display for NetlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNet(net) => write!(f, "netlist: undeclared net {net:?}"),
            Self::DuplicateNet(net) => write!(f, "netlist: duplicate net {net:?}"),
            Self::UnknownPart(part) => write!(f, "netlist: unknown part {part:?}"),
            Self::DuplicateReference(reference) => {
                write!(f, "netlist: duplicate reference {reference:?}")
            }
            Self::MissingTerminal {
                reference,
                terminal,
            } => write!(
                f,
                "netlist: {reference} left terminal {terminal:?} unconnected"
            ),
            Self::UnknownTerminal {
                reference,
                terminal,
            } => write!(f, "netlist: {reference} has no terminal {terminal:?}"),
            Self::DuplicateTerminal {
                reference,
                terminal,
            } => write!(
                f,
                "netlist: {reference} connects terminal {terminal:?} to more than one net"
            ),
            Self::InvalidParameter { reference } => {
                write!(f, "netlist: {reference} has invalid parameters")
            }
        }
    }
}

impl Error for NetlistError {}

/// A named parameter set for a part instance.
///
/// Values are SI and unit-agnostic: the part factory interprets them, so
/// `ohms` is a plain number here rather than a type the netlist must know about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parameters {
    values: BTreeMap<String, f64>,
}

impl Parameters {
    /// No parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace one parameter.
    pub fn set(&mut self, name: &str, value: f64) {
        self.values.insert(name.to_string(), value);
    }

    /// Builder form of [`Parameters::set`].
    pub fn with(mut self, name: &str, value: f64) -> Self {
        self.set(name, value);
        self
    }

    /// The value of `name`, if present.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }

    /// The value of `name`, or `default` when absent.
    pub fn get_or(&self, name: &str, default: f64) -> f64 {
        self.get(name).unwrap_or(default)
    }

    /// True when no parameter was supplied.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Parameter names and values, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, f64)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
    }
}

impl<const N: usize> From<[(&str, f64); N]> for Parameters {
    fn from(entries: [(&str, f64); N]) -> Self {
        let mut parameters = Self::new();
        for (name, value) in entries {
            parameters.set(name, value);
        }
        parameters
    }
}

/// One part instance placed in a netlist.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedPart {
    /// Reference designator, e.g. `R1` or `D1`.
    pub reference: String,
    /// Registry id of the part type, e.g. `resistor`.
    pub part: String,
    /// Instance parameters.
    pub parameters: Parameters,
    connections: Vec<(String, String)>,
}

impl PlacedPart {
    /// A part with no connections yet.
    pub fn new(reference: &str, part: &str, parameters: Parameters) -> Self {
        Self {
            reference: reference.to_string(),
            part: part.to_string(),
            parameters,
            connections: Vec::new(),
        }
    }

    /// Connect one named terminal to a net.
    pub fn terminal(mut self, terminal: &str, net: &str) -> Self {
        self.connections
            .push((terminal.to_string(), net.to_string()));
        self
    }

    /// Terminal-to-net connections in the order they were declared.
    pub fn connections(&self) -> &[(String, String)] {
        &self.connections
    }
}

/// A resistor placed between two nets.
pub fn resistor(reference: &str, ohms: f64, p: &str, n: &str) -> PlacedPart {
    PlacedPart::new(reference, "resistor", Parameters::from([("ohms", ohms)]))
        .terminal("p", p)
        .terminal("n", n)
}

/// A two-terminal diode part (an LED by default) placed anode -> cathode.
pub fn led(reference: &str, a: &str, k: &str) -> PlacedPart {
    PlacedPart::new(reference, "led", Parameters::new())
        .terminal("a", a)
        .terminal("k", k)
}

/// An ideal voltage source constrained to `volts` from `p` to `n`.
pub fn vsource(reference: &str, volts: f64, p: &str, n: &str) -> PlacedPart {
    PlacedPart::new(reference, "vsource", Parameters::from([("volts", volts)]))
        .terminal("p", p)
        .terminal("n", n)
}

/// An ideal current source delivering `amps` from `p` to `n`.
pub fn isource(reference: &str, amps: f64, p: &str, n: &str) -> PlacedPart {
    PlacedPart::new(reference, "isource", Parameters::from([("amps", amps)]))
        .terminal("p", p)
        .terminal("n", n)
}

/// A circuit description: named nets plus placed parts.
#[derive(Clone, Debug, PartialEq)]
pub struct Netlist {
    ground: String,
    nets: Vec<String>,
    parts: Vec<PlacedPart>,
}

impl Default for Netlist {
    fn default() -> Self {
        Self::new()
    }
}

impl Netlist {
    /// The default ground net name.
    pub const DEFAULT_GROUND: &'static str = "gnd";

    /// An empty netlist whose ground net is [`Netlist::DEFAULT_GROUND`].
    pub fn new() -> Self {
        Self {
            ground: Self::DEFAULT_GROUND.to_string(),
            nets: Vec::new(),
            parts: Vec::new(),
        }
    }

    /// Use `name` as the ground net.
    ///
    /// The ground net is always available and is never allocated a solver node:
    /// it *is* [`Node::GROUND`], so a netlist cannot accidentally float its own
    /// reference.
    pub fn ground(mut self, name: &str) -> Self {
        self.ground = name.to_string();
        self
    }

    /// Declare a net.
    pub fn net(mut self, name: &str) -> Self {
        self.nets.push(name.to_string());
        self
    }

    /// Declare several nets.
    pub fn nets<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for name in names {
            self.nets.push(name.as_ref().to_string());
        }
        self
    }

    /// Place a part.
    pub fn part(mut self, part: PlacedPart) -> Self {
        self.parts.push(part);
        self
    }

    /// Declared net names, excluding the ground net.
    pub fn net_names(&self) -> &[String] {
        &self.nets
    }

    /// Placed parts.
    pub fn parts(&self) -> &[PlacedPart] {
        &self.parts
    }

    /// The ground net name.
    pub fn ground_net(&self) -> &str {
        &self.ground
    }

    /// Validate the description and build a solver topology.
    pub fn compile(&self, registry: &PartRegistry) -> Result<CompiledCircuit, NetlistError> {
        let mut circuit = Circuit::new();
        let mut net_nodes: BTreeMap<String, Node> = BTreeMap::new();
        let mut node_names: Vec<String> = vec![self.ground.clone()];
        net_nodes.insert(self.ground.clone(), Node::GROUND);

        for name in &self.nets {
            if net_nodes.contains_key(name) {
                return Err(NetlistError::DuplicateNet(name.clone()));
            }
            let node = circuit.node();
            node_names.push(name.clone());
            net_nodes.insert(name.clone(), node);
        }

        let mut branches = BTreeMap::new();
        let mut references = Vec::new();
        for placed in &self.parts {
            if branches.contains_key(&placed.reference) {
                return Err(NetlistError::DuplicateReference(placed.reference.clone()));
            }
            // Two nets on one terminal is ambiguous wiring, not a last-wins
            // override, so it is rejected before anything is stamped.
            for (index, (terminal, _)) in placed.connections.iter().enumerate() {
                if placed.connections[index + 1..]
                    .iter()
                    .any(|(other, _)| other == terminal)
                {
                    return Err(NetlistError::DuplicateTerminal {
                        reference: placed.reference.clone(),
                        terminal: terminal.clone(),
                    });
                }
            }
            let factory = registry
                .get(&placed.part)
                .ok_or_else(|| NetlistError::UnknownPart(placed.part.clone()))?;
            let part = factory.build(&placed.reference, &placed.parameters)?;

            let mut terminals = Vec::new();
            for terminal in part.terminals() {
                let net = placed
                    .connections
                    .iter()
                    .find(|(name, _)| name == terminal)
                    .map(|(_, net)| net)
                    .ok_or_else(|| NetlistError::MissingTerminal {
                        reference: placed.reference.clone(),
                        terminal: (*terminal).to_string(),
                    })?;
                let node = *net_nodes
                    .get(net)
                    .ok_or_else(|| NetlistError::UnknownNet(net.clone()))?;
                terminals.push(node);
            }
            for (terminal, _) in &placed.connections {
                if !part.terminals().contains(&terminal.as_str()) {
                    return Err(NetlistError::UnknownTerminal {
                        reference: placed.reference.clone(),
                        terminal: terminal.clone(),
                    });
                }
            }

            let stamped = part.stamp(&terminals, &mut circuit).map_err(|_| {
                NetlistError::InvalidParameter {
                    reference: placed.reference.clone(),
                }
            })?;
            // The first stamped branch is the part's principal branch: the one
            // whose voltage and current describe the part itself.
            if let Some(principal) = stamped.first() {
                branches.insert(placed.reference.clone(), *principal);
            }
            references.push(placed.reference.clone());
        }

        Ok(CompiledCircuit {
            circuit,
            net_nodes,
            node_names,
            branches,
            references,
        })
    }
}

/// A part type in the catalogue.
pub trait PartFactory {
    /// Registry id.
    fn id(&self) -> &'static str;

    /// Ordered terminal names.
    fn terminals(&self) -> &'static [&'static str];

    /// Validate parameters and build the part.
    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError>;
}

/// An instantiated part that can stamp itself into a circuit.
pub trait Part {
    /// Registry id of the part type.
    fn id(&self) -> &'static str;

    /// Ordered terminal names.
    fn terminals(&self) -> &'static [&'static str];

    /// Stamp into `circuit`, allocating any internal nodes it needs.
    ///
    /// `terminals` is in [`Part::terminals`] order. The returned branches are in
    /// a part-defined order whose first entry is the principal branch.
    fn stamp(&self, terminals: &[Node], circuit: &mut Circuit)
        -> Result<Vec<BranchId>, SolveError>;

    /// One-line parameter summary for diagnostics and UI.
    fn summary(&self) -> String;
}

/// The catalogue of part types a netlist may reference.
pub struct PartRegistry {
    factories: BTreeMap<String, Box<dyn PartFactory>>,
}

impl Default for PartRegistry {
    fn default() -> Self {
        Self::standard()
    }
}

impl PartRegistry {
    /// A registry with no part types.
    pub fn empty() -> Self {
        Self {
            factories: BTreeMap::new(),
        }
    }

    /// A registry with the built-in passive parts and ideal sources.
    pub fn standard() -> Self {
        let mut registry = Self::empty();
        registry.register(Box::new(ResistorFactory));
        registry.register(Box::new(VoltageSourceFactory));
        registry.register(Box::new(CurrentSourceFactory));
        registry.register(Box::new(LedFactory::default()));
        registry.register(Box::new(LedFactory::red()));
        registry
    }

    /// Add a part type, returning any factory it replaced.
    pub fn register(&mut self, factory: Box<dyn PartFactory>) -> Option<Box<dyn PartFactory>> {
        self.factories.insert(factory.id().to_string(), factory)
    }

    /// Look up a part type.
    pub fn get(&self, id: &str) -> Option<&dyn PartFactory> {
        self.factories.get(id).map(|factory| factory.as_ref())
    }

    /// Registered part ids, ordered by name.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }
}

/// A validated circuit with its topology and lookup maps.
pub struct CompiledCircuit {
    circuit: Circuit,
    net_nodes: BTreeMap<String, Node>,
    node_names: Vec<String>,
    branches: BTreeMap<String, BranchId>,
    references: Vec<String>,
}

impl CompiledCircuit {
    /// Solve the present topology.
    pub fn solve(&self, options: SolveOptions) -> Result<Solution, SolveError> {
        self.circuit.solve(options)
    }

    /// The solver node a net maps to.
    pub fn node(&self, net: &str) -> Option<Node> {
        self.net_nodes.get(net).copied()
    }

    /// The net a solver node came from, when it came from the netlist.
    pub fn net_name(&self, node: Node) -> Option<&str> {
        self.node_names.get(node.0).map(String::as_str)
    }

    /// The principal branch of a placed part.
    pub fn branch(&self, reference: &str) -> Option<BranchId> {
        self.branches.get(reference).copied()
    }

    /// Reference designators in placement order.
    pub fn references(&self) -> &[String] {
        &self.references
    }

    /// The compiled topology, for inspection and source-value updates.
    pub fn circuit(&self) -> &Circuit {
        &self.circuit
    }

    /// Mutable topology access, for updating an existing source's value.
    ///
    /// Take the [`BranchId`] from [`CompiledCircuit::branch`] first: the
    /// immutable and mutable borrows cannot overlap in one expression.
    pub fn circuit_mut(&mut self) -> &mut Circuit {
        &mut self.circuit
    }
}

/// Ideal source part types, kept here so a netlist can describe a biased circuit
/// without the host inventing topology. A host that needs finite source
/// impedance adds a series resistor, exactly as it would on a breadboard.
struct VoltageSourceFactory;

impl PartFactory for VoltageSourceFactory {
    fn id(&self) -> &'static str {
        "vsource"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError> {
        let volts = parameters.get_or("volts", f64::NAN);
        if !volts.is_finite() {
            return Err(NetlistError::InvalidParameter {
                reference: reference.to_string(),
            });
        }
        Ok(Box::new(VoltageSourcePart { volts }))
    }
}

struct VoltageSourcePart {
    volts: f64,
}

impl Part for VoltageSourcePart {
    fn id(&self) -> &'static str {
        "vsource"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn stamp(
        &self,
        terminals: &[Node],
        circuit: &mut Circuit,
    ) -> Result<Vec<BranchId>, SolveError> {
        Ok(vec![circuit.voltage_source(
            terminals[0],
            terminals[1],
            self.volts,
        )?])
    }

    fn summary(&self) -> String {
        format!("{:.6} V", self.volts)
    }
}

struct CurrentSourceFactory;

impl PartFactory for CurrentSourceFactory {
    fn id(&self) -> &'static str {
        "isource"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError> {
        let amps = parameters.get_or("amps", f64::NAN);
        if !amps.is_finite() {
            return Err(NetlistError::InvalidParameter {
                reference: reference.to_string(),
            });
        }
        Ok(Box::new(CurrentSourcePart { amps }))
    }
}

struct CurrentSourcePart {
    amps: f64,
}

impl Part for CurrentSourcePart {
    fn id(&self) -> &'static str {
        "isource"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn stamp(
        &self,
        terminals: &[Node],
        circuit: &mut Circuit,
    ) -> Result<Vec<BranchId>, SolveError> {
        Ok(vec![circuit.current_source(
            terminals[0],
            terminals[1],
            self.amps,
        )?])
    }

    fn summary(&self) -> String {
        format!("{:.6} A", self.amps)
    }
}

struct ResistorFactory;

impl PartFactory for ResistorFactory {
    fn id(&self) -> &'static str {
        "resistor"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError> {
        let ohms = parameters.get_or("ohms", f64::NAN);
        let device = Resistor::new(ohms).map_err(|_| NetlistError::InvalidParameter {
            reference: reference.to_string(),
        })?;
        Ok(Box::new(ResistorPart { device }))
    }
}

struct ResistorPart {
    device: Resistor,
}

impl Part for ResistorPart {
    fn id(&self) -> &'static str {
        "resistor"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["p", "n"]
    }

    fn stamp(
        &self,
        terminals: &[Node],
        circuit: &mut Circuit,
    ) -> Result<Vec<BranchId>, SolveError> {
        Ok(vec![circuit.device(
            terminals[0],
            terminals[1],
            self.device,
        )?])
    }

    fn summary(&self) -> String {
        format!("{:.6} ohm", self.device.ohms())
    }
}

/// LED part type. `LedFactory::default()` registers `led`; `LedFactory::red()`
/// registers the illustrative `led.red` preset. Both accept overrides, so a
/// calibrated part is data rather than a new type.
struct LedFactory {
    id: &'static str,
    base: LedParameters,
}

impl LedFactory {
    fn red() -> Self {
        Self {
            id: "led.red",
            base: LedParameters::default(),
        }
    }
}

impl Default for LedFactory {
    fn default() -> Self {
        Self {
            id: "led",
            base: LedParameters::default(),
        }
    }
}

impl PartFactory for LedFactory {
    fn id(&self) -> &'static str {
        self.id
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["a", "k"]
    }

    fn build(
        &self,
        reference: &str,
        parameters: &Parameters,
    ) -> Result<Box<dyn Part>, NetlistError> {
        let base = self.base;
        // Every accepted name must be a real override, so a typo cannot silently
        // fall back to the default model.
        for (name, _) in parameters.iter() {
            let known = matches!(
                name,
                "saturation_current"
                    | "ideality_factor"
                    | "thermal_voltage"
                    | "shunt_resistance"
                    | "nominal_current"
            );
            if !known {
                return Err(NetlistError::InvalidParameter {
                    reference: reference.to_string(),
                });
            }
        }
        let values = LedParameters {
            saturation_current: parameters.get_or("saturation_current", base.saturation_current),
            ideality_factor: parameters.get_or("ideality_factor", base.ideality_factor),
            thermal_voltage: parameters.get_or("thermal_voltage", base.thermal_voltage),
            shunt_resistance: parameters.get_or("shunt_resistance", base.shunt_resistance),
            nominal_current: parameters.get_or("nominal_current", base.nominal_current),
        };
        let device = Led::new(values).map_err(|_| NetlistError::InvalidParameter {
            reference: reference.to_string(),
        })?;
        Ok(Box::new(LedPart { device }))
    }
}

struct LedPart {
    device: Led,
}

impl Part for LedPart {
    fn id(&self) -> &'static str {
        "led"
    }

    fn terminals(&self) -> &'static [&'static str] {
        &["a", "k"]
    }

    fn stamp(
        &self,
        terminals: &[Node],
        circuit: &mut Circuit,
    ) -> Result<Vec<BranchId>, SolveError> {
        Ok(vec![circuit.device(
            terminals[0],
            terminals[1],
            self.device,
        )?])
    }

    fn summary(&self) -> String {
        format!("Is={:.1e} A", self.device.parameters().saturation_current)
    }
}
