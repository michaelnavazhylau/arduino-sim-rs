// SPDX-License-Identifier: MIT

//! Analog coupling between a SPICE deck and the AVR's pins and ADC.
//!
//! A digital pin is not an ideal switch, and an analog input is not a number a
//! host invents. This module closes the loop in both directions:
//!
//! * **MCU -> circuit.** Each bound pin contributes a Thevenin driver — a
//!   voltage source and a series resistance — reflecting its current drive mode.
//!   A pin in high impedance contributes *nothing*, because an undriven pin
//!   really is undriven; approximating it with a huge resistor would make a
//!   floating input look like a measurement.
//! * **Circuit -> MCU.** Solved node voltages feed the ADC mux channels, and a
//!   bound input pin's pad voltage is resolved through the AVR's input
//!   thresholds and written back to `PINx`.
//!
//! The circuit is a **SPICE deck**, solved by [`crate::spice`]. Drivers and
//! switches are card text the host appends to whatever deck it was given, so the
//! host never has to know the deck's topology and the deck never has to know
//! which pins are attached.
//!
//! Solving is cached: an unchanged drive mode and switch state reuse the last
//! operating point rather than re-running the simulator.

use crate::pin::{Pin, Port};
use crate::spice::{deck_index, solve_op, DeckIndex, OpError, Solution};
use avr_sim::board::{Board, PinState};
use avr_sim::runtime::Handle;
use avr_sim::{Backend, Runtime, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

/// Number of single-ended ADC channels the ATmega328P mux exposes here.
pub const ADC_CHANNELS: usize = 8;
/// `DIDR0`: each bit disables the digital input buffer of the matching ADC pin.
const DIDR0: usize = 0x7e;

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

/// Topology class of a pin driver.
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
    /// The deck was rejected, or the operating point failed.
    Spice(OpError),
    /// A pin is bound to a net the deck does not connect.
    UnknownNet { pin: Pin, net: String },
    /// A switch was bound to a net the deck does not connect.
    UnknownSwitchNet { reference: String, net: String },
    /// The same pin was bound twice.
    DuplicateBinding(Pin),
    /// A switch reference was bound twice, or collides with a deck element.
    DuplicateSwitch(String),
    /// A driver or switch resistance is not a positive finite number.
    InvalidParameter(&'static str),
}

impl fmt::Display for CouplingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spice(error) => write!(f, "analog coupling: {error}"),
            Self::UnknownNet { pin, net } => {
                write!(
                    f,
                    "analog coupling: pin {pin} is bound to undeclared net {net:?}"
                )
            }
            Self::UnknownSwitchNet { reference, net } => write!(
                f,
                "analog coupling: switch {reference:?} is bound to undeclared net {net:?}"
            ),
            Self::DuplicateBinding(pin) => {
                write!(f, "analog coupling: pin {pin} is bound more than once")
            }
            Self::DuplicateSwitch(reference) => {
                write!(
                    f,
                    "analog coupling: switch {reference:?} is bound more than once"
                )
            }
            Self::InvalidParameter(name) => write!(f, "analog coupling: invalid {name}"),
        }
    }
}

impl Error for CouplingError {}

/// Result of one coupling update.
#[derive(Clone, Debug, PartialEq)]
pub enum CouplingOutcome {
    /// The circuit solved and every reading is valid.
    Solved,
    /// The operating point is not determined — typically an undriven net with no
    /// DC path to ground. The reason is preserved rather than replaced by a
    /// plausible number.
    Indeterminate(OpError),
}

/// Voltages and levels read out of the last solve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnalogReading {
    outcome: Option<CouplingOutcome>,
    net_voltages: BTreeMap<String, f64>,
    source_currents: BTreeMap<String, f64>,
    adc_voltages: Vec<Option<f64>>,
    digital_levels: BTreeMap<Pin, Option<bool>>,
}

impl AnalogReading {
    /// Outcome of the solve this reading came from.
    pub fn outcome(&self) -> CouplingOutcome {
        self.outcome
            .clone()
            .unwrap_or(CouplingOutcome::Indeterminate(OpError::FloatingNets(
                Vec::new(),
            )))
    }

    /// Solved voltage of a net, when the solve succeeded.
    ///
    /// Ground, written `0` or `gnd`, always reads 0 V.
    pub fn net_voltage(&self, net: &str) -> Option<f64> {
        let canonical = canonical(net);
        if canonical == "0" {
            return Some(0.0);
        }
        self.net_voltages.get(&canonical).copied()
    }

    /// Operating-point current through the voltage source named `reference`, in
    /// amperes, using ngspice's sign convention: positive current flows into the
    /// source's first node, so a source *delivering* power reports a negative
    /// current.
    pub fn source_current(&self, reference: &str) -> Option<f64> {
        self.source_currents
            .get(&reference.to_ascii_lowercase())
            .copied()
    }

    /// Solved voltage presented to an ADC channel.
    pub fn adc_voltage(&self, channel: usize) -> Option<f64> {
        self.adc_voltages.get(channel).copied().flatten()
    }

    /// Resolved digital level of a bound pin, or `None` in the threshold band.
    pub fn digital_level(&self, pin: Pin) -> Option<bool> {
        self.digital_levels.get(&pin).copied().flatten()
    }

    /// Every solved net voltage, keyed by canonical node name.
    pub fn net_voltages(&self) -> &BTreeMap<String, f64> {
        &self.net_voltages
    }
}

/// A switch the host can throw.
///
/// A mechanical switch is not a pin drive and not a sensor: it changes the
/// electrical network itself. The host declares its two nets, and the coupling
/// emits one resistor whose value is the contact or insulation resistance.
#[derive(Clone, Debug)]
struct SwitchBinding {
    reference: String,
    net_a: String,
    net_b: String,
    /// State the host has asked for.
    closed: bool,
}

/// An analog circuit, given as a SPICE deck, coupled to specific Uno pins.
pub struct AnalogCoupling {
    deck: String,
    /// Nets and elements the supplied deck declares, without the drivers and
    /// switches this module appends.
    base: DeckIndex,
    wiring: Wiring,
    switches: Vec<SwitchBinding>,
    supply_volts: f64,
    output_ohms: f64,
    pullup_ohms: f64,
    closed_ohms: f64,
    open_ohms: f64,
    /// Pin modes the last solve used.
    modes: Vec<PinState>,
    /// Switch states the last solve used.
    applied_switches: Vec<bool>,
    reading: AnalogReading,
    solves: u64,
    solved: bool,
    /// Whether the bindings have been checked. Validation is deferred until the
    /// first solve because a switch net may be supplied by a binding added after
    /// construction, and it must not run on the per-instruction hot path.
    validated: bool,
}

impl fmt::Debug for AnalogCoupling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalogCoupling")
            .field("deck_lines", &self.deck.lines().count())
            .field("bindings", &self.wiring.bindings())
            .field("switches", &self.switches)
            .field("modes", &self.modes)
            .field("solves", &self.solves)
            .field("outcome", &self.reading.outcome())
            .finish_non_exhaustive()
    }
}

impl AnalogCoupling {
    /// Couple the SPICE `deck` to the pins named in `wiring`.
    ///
    /// The deck's first line is its title, as in ngspice. Ground is `0` or its
    /// alias `gnd`. The coupling appends the MCU drivers, the bound switches,
    /// `.op` and `.end`, so the deck describes only the circuit.
    ///
    /// Net names a pin is bound to may be supplied by the deck or by a switch
    /// bound later with [`AnalogCoupling::bind_switch`], so the binding check
    /// runs at [`AnalogCoupling::validate`] time rather than here.
    ///
    /// # Errors
    ///
    /// [`CouplingError::Spice`] when the deck does not parse, and
    /// [`CouplingError::DuplicateBinding`] for a repeated pin.
    pub fn new(deck: impl Into<String>, wiring: Wiring) -> Result<Self, CouplingError> {
        let deck = deck.into();
        let base = deck_index(&deck).map_err(CouplingError::Spice)?;
        let mut seen = Vec::new();
        for (pin, _) in wiring.bindings() {
            if seen.contains(pin) {
                return Err(CouplingError::DuplicateBinding(*pin));
            }
            seen.push(*pin);
        }
        let count = wiring.bindings().len();
        Ok(Self {
            deck,
            base,
            wiring,
            switches: Vec::new(),
            supply_volts: 5.0,
            output_ohms: 25.0,
            pullup_ohms: 30_000.0,
            closed_ohms: 0.05,
            open_ohms: 100e6,
            // A reset AVR presents every pin as a floating input.
            modes: vec![PinState::Input; count],
            applied_switches: Vec::new(),
            reading: AnalogReading {
                adc_voltages: vec![None; ADC_CHANNELS],
                ..AnalogReading::default()
            },
            solves: 0,
            solved: false,
            validated: false,
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

    /// Switch contact and insulation resistances, in ohms.
    ///
    /// Tactile switches are typically specified at <= 100 mohm closed and
    /// \>= 100 Mohm open. Neither state is ideal, which is the point: an open
    /// contact is not a perfect break and a closed one is not a perfect short.
    pub fn with_switch(mut self, closed_ohms: f64, open_ohms: f64) -> Self {
        self.closed_ohms = closed_ohms;
        self.open_ohms = open_ohms;
        self
    }

    /// The SPICE deck, without the drivers and switches this module appends.
    pub fn deck(&self) -> &str {
        &self.deck
    }

    /// The pin bindings.
    pub fn wiring(&self) -> &Wiring {
        &self.wiring
    }

    /// Bind a switch between `net_a` and `net_b` for the host to throw.
    ///
    /// The switch is not written in the deck: the coupling emits it as a single
    /// resistor, so throwing it is a value change rather than a topology edit.
    ///
    /// # Errors
    ///
    /// [`CouplingError::DuplicateSwitch`] when the reference is already bound or
    /// would collide with a deck element of the same name.
    pub fn bind_switch(
        &mut self,
        reference: &str,
        net_a: &str,
        net_b: &str,
    ) -> Result<(), CouplingError> {
        if self.switches.iter().any(|s| s.reference == reference) {
            return Err(CouplingError::DuplicateSwitch(reference.to_string()));
        }
        if self
            .base
            .elements()
            .contains(&element_name(reference).to_ascii_lowercase())
        {
            return Err(CouplingError::DuplicateSwitch(reference.to_string()));
        }
        self.switches.push(SwitchBinding {
            reference: reference.to_string(),
            net_a: net_a.to_string(),
            net_b: net_b.to_string(),
            closed: false,
        });
        self.validated = false;
        Ok(())
    }

    /// Open or close a bound switch, returning whether it was bound at all.
    pub fn set_switch(&mut self, reference: &str, closed: bool) -> bool {
        match self.switches.iter_mut().find(|s| s.reference == reference) {
            Some(switch) => {
                switch.closed = closed;
                true
            }
            None => false,
        }
    }

    /// The requested state of a bound switch.
    pub fn switch(&self, reference: &str) -> Option<bool> {
        self.switches
            .iter()
            .find(|s| s.reference == reference)
            .map(|s| s.closed)
    }

    /// Reference designators of the bound switches, in binding order.
    pub fn switches(&self) -> impl Iterator<Item = &str> {
        self.switches.iter().map(|s| s.reference.as_str())
    }

    /// Validate driver resistances and every bound net name.
    ///
    /// A bound net must be connected by the deck, supplied by a bound switch, or
    /// be ground; a switch net must be connected by the deck, be bound to a pin,
    /// or be ground. [`AnalogCoupling::update`] runs this before every solve, so
    /// call it directly only to fail fast.
    ///
    /// # Errors
    ///
    /// [`CouplingError::InvalidParameter`] for a non-positive or non-finite
    /// resistance, [`CouplingError::UnknownNet`] for a pin bound to a net nothing
    /// supplies, and [`CouplingError::UnknownSwitchNet`] for a switch whose nets
    /// nothing connects.
    pub fn validate(&self) -> Result<(), CouplingError> {
        for (value, name) in [
            (self.output_ohms, "output resistance"),
            (self.pullup_ohms, "pull-up resistance"),
            (self.closed_ohms, "switch contact resistance"),
            (self.open_ohms, "switch insulation resistance"),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(CouplingError::InvalidParameter(name));
            }
        }
        if !self.supply_volts.is_finite() {
            return Err(CouplingError::InvalidParameter("supply voltage"));
        }
        let switch_nets: BTreeSet<String> = self
            .switches
            .iter()
            .flat_map(|s| [canonical(&s.net_a), canonical(&s.net_b)])
            .collect();
        let wiring_nets: BTreeSet<String> = self
            .wiring
            .bindings()
            .iter()
            .map(|(_, net)| canonical(net))
            .collect();
        for (pin, net) in self.wiring.bindings() {
            if !self.base.connects(net) && !switch_nets.contains(&canonical(net)) {
                return Err(CouplingError::UnknownNet {
                    pin: *pin,
                    net: net.clone(),
                });
            }
        }
        for switch in &self.switches {
            for net in [&switch.net_a, &switch.net_b] {
                if !self.base.connects(net) && !wiring_nets.contains(&canonical(net)) {
                    return Err(CouplingError::UnknownSwitchNet {
                        reference: switch.reference.clone(),
                        net: net.clone(),
                    });
                }
            }
        }
        Ok(())
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
    /// Re-solves only when a driver's shape or level, or a switch state, has
    /// changed since the last solve.
    ///
    /// # Errors
    ///
    /// Any [`CouplingError`] from [`AnalogCoupling::validate`].
    pub fn update(
        &mut self,
        backend: &dyn Backend,
        runtime: &mut Runtime,
        board: &Board,
    ) -> Result<CouplingOutcome, CouplingError> {
        if !self.validated {
            self.validate()?;
            self.validated = true;
        }
        let modes: Vec<PinState> = self
            .wiring
            .bindings()
            .iter()
            .map(|(pin, _)| {
                let (port, bit) = pin.port_bit();
                board.pin_state(backend, runtime, port_handle(board, port), bit)
            })
            .collect();
        let switches_changed = self
            .switches
            .iter()
            .map(|switch| switch.closed)
            .ne(self.applied_switches.iter().copied());

        if self.solved && modes == self.modes && !switches_changed {
            // Nothing electrical changed, so nothing is pushed back to the AVR.
            return Ok(self.reading.outcome());
        }
        self.modes = modes;
        self.applied_switches = self.switches.iter().map(|switch| switch.closed).collect();
        let deck = self.assemble_deck();
        self.solves += 1;
        self.solved = true;
        self.reading = match solve_op(&deck) {
            Ok(solution) => self.read(&solution),
            Err(error) => AnalogReading {
                outcome: Some(CouplingOutcome::Indeterminate(error)),
                adc_voltages: vec![None; ADC_CHANNELS],
                ..AnalogReading::default()
            },
        };
        self.apply(backend, runtime, board);
        Ok(self.reading.outcome())
    }

    /// The supplied deck plus a driver per driven pin, the bound switches, and
    /// the analysis cards.
    fn assemble_deck(&self) -> String {
        let mut deck = String::with_capacity(self.deck.len() + 256);
        for line in self.deck.lines() {
            let lowered = line.trim().to_ascii_lowercase();
            if lowered == ".op" || lowered == ".end" {
                continue;
            }
            deck.push_str(line);
            deck.push('\n');
        }
        for (index, (pin, net)) in self.wiring.bindings().iter().enumerate() {
            let (volts, ohms) = match DriverShape::of(self.modes[index]) {
                DriverShape::Absent => continue,
                DriverShape::Driven => (
                    driven_volts(self.modes[index], self.supply_volts),
                    self.output_ohms,
                ),
                DriverShape::PullUp => (self.supply_volts, self.pullup_ohms),
            };
            let _ = writeln!(
                deck,
                "{} {} 0 {volts}\n{} {} {net} {ohms}",
                driver_source(*pin),
                driver_node(*pin),
                driver_resistor(*pin),
                driver_node(*pin),
            );
        }
        for switch in &self.switches {
            let ohms = if switch.closed {
                self.closed_ohms
            } else {
                self.open_ohms
            };
            let _ = writeln!(
                deck,
                "{} {} {} {ohms}",
                element_name(&switch.reference),
                switch.net_a,
                switch.net_b,
            );
        }
        deck.push_str(".op\n.end\n");
        deck
    }

    fn read(&self, solution: &Solution) -> AnalogReading {
        let mut net_voltages = BTreeMap::new();
        for (net, voltage) in solution.net_voltages() {
            net_voltages.insert(net.clone(), *voltage);
        }
        let source_currents = solution.source_currents().clone();
        let mut adc_voltages = vec![None; ADC_CHANNELS];
        let mut digital_levels = BTreeMap::new();
        for (pin, net) in self.wiring.bindings() {
            let Some(voltage) = solution.net_voltage(net) else {
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
            source_currents,
            adc_voltages,
            digital_levels,
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

/// Canonical node name: lowercased, with `gnd` aliased to ground.
fn canonical(net: &str) -> String {
    ngspice_rs::primitives::NodeTable::canonical_name(net, true)
}

/// Element name of the resistor a bound switch becomes.
fn element_name(reference: &str) -> String {
    format!("R{reference}")
}

/// Reference designator of the voltage source a bound pin contributes.
fn driver_source(pin: Pin) -> String {
    format!("V_MCU_{pin}")
}

/// Reference designator of the series resistor a bound pin contributes.
fn driver_resistor(pin: Pin) -> String {
    format!("R_MCU_{pin}")
}

/// Internal node between a driver's source and its series resistor.
fn driver_node(pin: Pin) -> String {
    format!("N_MCU_{pin}")
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
