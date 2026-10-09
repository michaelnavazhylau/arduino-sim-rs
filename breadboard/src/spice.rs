// SPDX-License-Identifier: MIT

//! DC operating points solved by [`ngspice_rs`], the in-repo Rust port of ngspice.
//!
//! This is the one place the host touches a circuit simulator. A circuit is
//! *text*: a SPICE deck is parsed, its operating point is solved, and node
//! voltages and voltage-source currents are read back. Nothing here knows about
//! the AVR, so the electrical layer can be tested without a board.
//!
//! Only `.op` is used. The host is still **DC only**: there is no transient
//! integration, so no RC charging, no PWM averaging and no sample-and-hold. A
//! deck that relies on a capacitor for its DC path therefore has an undetermined
//! operating point, which [`solve_op`] reports rather than papering over.

use ngspice_rs::analysis::{runner, AnalysisContext, AnalysisRequest};
use ngspice_rs::devices::Circuit;
use ngspice_rs::netlist::ast::Netlist;
use ngspice_rs::netlist::source::parse_deck_text;
use ngspice_rs::netlist::Parser;
use ngspice_rs::primitives::{AnalysisKind, NodeTable, SpiceError};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

/// ngspice's `CHARGE / CONSTboltz` quotient, in volts per kelvin.
///
/// Recomputed here because ngspice-rs keeps it crate-private. It is the
/// constant [`MODEL_TEMPERATURE_C`] is derived from, so the two stay consistent:
/// changing this value changes the temperature that reproduces the project's
/// illustrative LED thermal voltage.
const K_OVER_Q: f64 = 1.38064852e-23 / 1.6021766208e-19;

/// The illustrative LED thermal voltage the decks in this repository are built
/// around, in volts.
///
/// The retired in-tree LED model hard-coded this value; a SPICE diode derives
/// `Vt` from temperature instead, so [`MODEL_TEMPERATURE_C`] is chosen to
/// reproduce it rather than the diode parameters being re-fitted.
pub const MODEL_THERMAL_VOLTS: f64 = 0.02585;

/// Circuit temperature, in Celsius, that reproduces the illustrative LED
/// thermal voltage of 25.85 mV.
///
/// With this setting a red LED driven from a 5 V pad through 25 + 220 ohm solves
/// to 4.709244 V at the pad and 2.150594 V across the diode at 11.63023 mA, which
/// is exactly what the retired model produced.
pub const MODEL_TEMPERATURE_C: f64 = MODEL_THERMAL_VOLTS / K_OVER_Q - 273.15;

/// File name reported in ngspice-rs diagnostics.
const DECK_NAME: &str = "analog.cir";

/// Ground, in the spelling a deck may use for it.
const GROUND: &str = "0";

/// A solved DC operating point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Solution {
    net_voltages: BTreeMap<String, f64>,
    source_currents: BTreeMap<String, f64>,
}

impl Solution {
    /// Operating-point voltage of `net`, in volts.
    ///
    /// Ground, written `0` or its ngspice alias `gnd`, is the reference and
    /// always reads 0 V even though it has no row of its own. `None` means the
    /// deck has no such net.
    pub fn net_voltage(&self, net: &str) -> Option<f64> {
        let canonical = NodeTable::canonical_name(net, true);
        if canonical == GROUND {
            return Some(0.0);
        }
        self.net_voltages.get(&canonical).copied()
    }

    /// Operating-point current through the voltage source named `reference`, in
    /// amperes, using ngspice's sign convention: positive current flows into the
    /// source's first (positive) node and out of its second, so a source that
    /// *delivers* power reports a **negative** current.
    pub fn source_current(&self, reference: &str) -> Option<f64> {
        self.source_currents
            .get(&reference.to_ascii_lowercase())
            .copied()
    }

    /// Every solved net voltage, keyed by canonical node name.
    pub fn net_voltages(&self) -> &BTreeMap<String, f64> {
        &self.net_voltages
    }

    /// Every voltage-source current, keyed by lowercase instance name.
    pub fn source_currents(&self) -> &BTreeMap<String, f64> {
        &self.source_currents
    }
}

/// Why a DC operating point could not be established.
#[derive(Clone, Debug, PartialEq)]
pub enum OpError {
    /// The deck was rejected, or the analysis did not converge.
    Spice(SpiceError),
    /// Nets with no DC path to ground.
    ///
    /// ngspice would still return a number here by regularising the node with
    /// `gmin`, which turns an undriven net into a plausible-looking voltage.
    /// That would make a floating input read as a definite logic level, so the
    /// operating point is reported as undetermined instead of fabricated.
    FloatingNets(Vec<String>),
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spice(error) => write!(f, "{error}"),
            Self::FloatingNets(nets) => {
                write!(f, "no DC path to ground for {}", nets.join(", "))
            }
        }
    }
}

impl Error for OpError {}

/// Solve the DC operating point of a SPICE `deck`.
///
/// The deck is a complete ngspice deck: its first line is the title, and
/// `.op`/`.end` may be present or absent. Node names are matched
/// case-insensitively, and `gnd` aliases ground exactly as ngspice does.
///
/// # Errors
///
/// [`OpError::Spice`] when the deck is rejected or the operating point does not
/// converge, and [`OpError::FloatingNets`] when some net has no DC path to
/// ground.
pub fn solve_op(deck: &str) -> Result<Solution, OpError> {
    let text = parse_deck_text(Path::new(DECK_NAME), deck);
    let netlist = Parser::new().parse_deck(&text).map_err(OpError::Spice)?;
    if let Some(floating) = floating_nets(&netlist) {
        if !floating.is_empty() {
            return Err(OpError::FloatingNets(floating));
        }
    }
    let mut circuit = Circuit::from_netlist(&netlist).map_err(OpError::Spice)?;
    let request = AnalysisRequest::new(AnalysisKind::OperatingPoint);
    let context = AnalysisContext {
        temperature: MODEL_TEMPERATURE_C,
        nominal_temperature: MODEL_TEMPERATURE_C,
        ..AnalysisContext::default()
    };
    let plot = runner(request.kind)
        .map_err(OpError::Spice)?
        .run(&mut circuit, &request, &context)
        .map_err(OpError::Spice)?;

    let mut solution = Solution::default();
    for variable in &plot.variables {
        let Some(value) = plot.value(&variable.name, 0) else {
            continue;
        };
        if let Some(net) = call_argument(&variable.name, 'v') {
            solution.net_voltages.insert(net, value.re);
        } else if let Some(reference) = call_argument(&variable.name, 'i') {
            solution.source_currents.insert(reference, value.re);
        }
    }
    Ok(solution)
}

/// `v(out)` -> `Some("out")`, `i(v1)` -> `Some("v1")`, anything else `None`.
fn call_argument(name: &str, kind: char) -> Option<String> {
    let mut characters = name.chars();
    if characters.next()? != kind {
        return None;
    }
    let rest = characters.as_str();
    rest.strip_prefix('(')?
        .strip_suffix(')')
        .map(str::to_string)
}

/// Nets with no DC path to ground.
///
/// The check is topological: two-terminal devices that constrain the DC
/// operating point tie their terminals into one conducting group, and anything
/// reachable from ground is determined. A current source injects charge without
/// referencing a node, and a capacitor is an open circuit at DC, so neither is
/// a conducting edge.
///
/// `None` means the deck uses a device this classification does not cover, in
/// which case the check stays silent rather than reporting a false positive.
fn floating_nets(netlist: &Netlist) -> Option<Vec<String>> {
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut classified = true;
    for device in &netlist.devices {
        let terminals: Vec<String> = device.nodes.iter().map(|node| canonical(node)).collect();
        for terminal in &terminals {
            nodes.insert(terminal.clone());
        }
        match device.designator {
            // Resistor, diode, inductor, switch, voltage source, controlled
            // source: each constrains the DC operating point across its
            // terminals.
            'r' | 'v' | 'l' | 'd' | 's' | 'w' | 'e' | 'f' | 'g' | 'h' => {
                if let [first, second, ..] = terminals.as_slice() {
                    adjacency
                        .entry(first.clone())
                        .or_default()
                        .insert(second.clone());
                    adjacency
                        .entry(second.clone())
                        .or_default()
                        .insert(first.clone());
                }
            }
            // A current source does not reference its node to anything, and a
            // capacitor blocks DC.
            'i' | 'c' => {}
            // Behavioural sources, subcircuits, transistors: conduction
            // semantics this check does not classify.
            _ => classified = false,
        }
    }
    if !classified {
        return None;
    }
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    reachable.insert(GROUND.to_string());
    let mut pending = vec![GROUND.to_string()];
    while let Some(node) = pending.pop() {
        for neighbour in adjacency.get(&node).into_iter().flatten() {
            if reachable.insert(neighbour.clone()) {
                pending.push(neighbour.clone());
            }
        }
    }
    nodes.remove(GROUND);
    Some(nodes.difference(&reachable).cloned().collect())
}

/// Canonical node name: lowercased, with `gnd` aliased to ground.
fn canonical(node: &str) -> String {
    NodeTable::canonical_name(node, true)
}

/// The nets and element names a deck declares.
///
/// A host that appends its own cards needs to know which names are already
/// taken and which nets a binding may legitimately refer to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckIndex {
    nets: BTreeSet<String>,
    elements: BTreeSet<String>,
}

impl DeckIndex {
    /// Canonical net names the deck's devices connect to.
    pub fn nets(&self) -> &BTreeSet<String> {
        &self.nets
    }

    /// Lowercase element names the deck declares.
    pub fn elements(&self) -> &BTreeSet<String> {
        &self.elements
    }

    /// Whether `net` is ground or a net the deck connects.
    pub fn connects(&self, net: &str) -> bool {
        let canonical = canonical(net);
        canonical == GROUND || self.nets.contains(&canonical)
    }
}

/// Index a deck's nets and element names.
///
/// # Errors
///
/// [`OpError::Spice`] when the deck does not parse.
pub fn deck_index(deck: &str) -> Result<DeckIndex, OpError> {
    let text = parse_deck_text(Path::new(DECK_NAME), deck);
    let netlist = Parser::new().parse_deck(&text).map_err(OpError::Spice)?;
    let mut index = DeckIndex::default();
    for device in &netlist.devices {
        index.elements.insert(device.name.to_ascii_lowercase());
        for node in &device.nodes {
            index.nets.insert(canonical(node));
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LED_DECK: &str = "\
blink
V_MCU_D9 ndrv_d9 0 5
R_MCU_D9 ndrv_d9 d9 25
R1 d9 led_a 220
D1 led_a 0 DLED
Rsh_D1 led_a 0 1e12
.model DLED D(IS=1e-20 N=2)
.op
.end
";

    #[test]
    fn a_driven_led_solves_to_the_retired_models_operating_point() {
        let solution = solve_op(LED_DECK).expect("solves");
        assert!((solution.net_voltage("d9").unwrap() - 4.709_244).abs() < 1e-6);
        assert!((solution.net_voltage("led_a").unwrap() - 2.150_594).abs() < 1e-6);
        // The resistor is ideal, so its current is exactly the LED current.
        let current =
            (solution.net_voltage("d9").unwrap() - solution.net_voltage("led_a").unwrap()) / 220.0;
        assert!((current - 1.163_023e-2).abs() < 1e-8);
        // A source delivering power reports a negative current.
        assert!((solution.source_current("V_MCU_D9").unwrap() + current).abs() < 1e-9);
    }

    #[test]
    fn ground_reads_zero_under_both_spellings() {
        let solution = solve_op(LED_DECK).expect("solves");
        assert_eq!(solution.net_voltage("0"), Some(0.0));
        assert_eq!(solution.net_voltage("gnd"), Some(0.0));
        assert_eq!(solution.net_voltage("GND"), Some(0.0));
        assert_eq!(solution.net_voltage("no_such_net"), None);
    }

    #[test]
    fn a_reverse_biased_led_blocks_current() {
        let reversed = LED_DECK.replace("D1 led_a 0 DLED", "D1 0 led_a DLED");
        let solution = solve_op(&reversed).expect("solves");
        assert!((solution.net_voltage("d9").unwrap() - 5.0).abs() < 1e-6);
        let current =
            (solution.net_voltage("d9").unwrap() - solution.net_voltage("led_a").unwrap()) / 220.0;
        assert!(current.abs() < 1e-9, "reverse leakage {current} A");
    }

    #[test]
    fn an_undriven_net_with_no_path_to_ground_is_not_given_a_number() {
        let error = solve_op("floating\nR1 d9 floating 1k\n.op\n.end\n").unwrap_err();
        match error {
            OpError::FloatingNets(nets) => {
                assert!(nets.contains(&"d9".to_string()), "{nets:?}");
                assert!(nets.contains(&"floating".to_string()), "{nets:?}");
            }
            other => panic!("expected floating nets, got {other:?}"),
        }
    }

    #[test]
    fn a_net_referenced_only_by_a_current_source_is_undetermined() {
        let error = solve_op("injected\nI1 0 n 1m\n.op\n.end\n").unwrap_err();
        assert!(matches!(error, OpError::FloatingNets(_)), "{error:?}");
    }

    #[test]
    fn a_deck_with_an_unsupported_device_reports_a_spice_error() {
        let error = solve_op("bad\nR1 a b 1k\nFrobnicate a b\n.op\n.end\n").unwrap_err();
        assert!(matches!(error, OpError::Spice(_)), "{error:?}");
    }
}
