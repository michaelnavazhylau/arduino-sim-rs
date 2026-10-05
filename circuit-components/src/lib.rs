// SPDX-License-Identifier: MIT

//! Electrical models only: no raylib, AVR, geometry or firmware dependencies.
//!
//! SI units everywhere. For both devices V = Vpositive - Vnegative and I flows
//! positive -> negative. For an LED these terminals are anode -> cathode.
//! The default red LED is illustrative, not a fitted model of a specific part.
//!
//! [`netlist`] turns a circuit description into a solver topology, so a host can
//! add a part as data instead of hand-writing node allocations.

pub mod netlist;

pub use netlist::{
    isource, led, resistor, switch, vsource, CompiledCircuit, Netlist, NetlistError, Parameters,
    Part, PartFactory, PartRegistry, PlacedPart,
};

use analog_solver::{Device, Linearization, ModelError};

fn positive_finite(value: f64, name: &'static str) -> Result<f64, ModelError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(ModelError::InvalidParameter(name))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Resistor {
    ohms: f64,
    conductance: f64,
}
impl Resistor {
    /// Wires are a shared solver node; zero resistance is rejected.
    pub fn new(ohms: f64) -> Result<Self, ModelError> {
        positive_finite(ohms, "resistance")?;
        let conductance = positive_finite(ohms.recip(), "resistor conductance")?;
        Ok(Self { ohms, conductance })
    }
    pub fn ohms(self) -> f64 {
        self.ohms
    }
}
impl Device for Resistor {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError> {
        let current = voltage * self.conductance;
        if !current.is_finite() {
            return Err(ModelError::OutOfRange("resistor voltage/current"));
        }
        Ok(Linearization {
            current,
            conductance: self.conductance,
        })
    }
}

/// I = Is * (exp(V / (n * Vt)) - 1) + V / Rshunt.
///
/// The explicit shunt represents small reverse leakage and anchors an
/// undriven circuit to ground. It is a model parameter, not solver-added gmin.
/// No avalanche breakdown, capacitance, temperature evolution, damage, or
/// internal series resistance is included; add series resistors as branches.
#[derive(Clone, Copy, Debug)]
pub struct LedParameters {
    pub saturation_current: f64,
    pub ideality_factor: f64,
    pub thermal_voltage: f64,
    pub shunt_resistance: f64,
    /// Optical brightness normalization, not an electrical current clamp.
    pub nominal_current: f64,
}
impl Default for LedParameters {
    fn default() -> Self {
        Self {
            saturation_current: 1e-20,
            ideality_factor: 2.0,
            thermal_voltage: 0.02585,
            shunt_resistance: 1e12,
            nominal_current: 0.020,
        }
    }
}

impl LedParameters {
    /// Illustrative indicator presets for the three common colours.
    ///
    /// The saturation current sets the forward voltage, and it is the dominant
    /// difference between LED colours: it spans nine orders of magnitude between
    /// red and blue because the band gap does. Everything else is held fixed, so
    /// the three presets differ in exactly one physical parameter.
    ///
    /// These are **illustrative curves, not fitted data** for any specific part.
    /// Real parts vary by colour bin, and the green and blue figures below are
    /// closer to modern InGaN parts than to older GaP green.
    ///
    /// `nominal_current` is the 10 mA indicator operating point these presets are
    /// normalised against, so a 330 ohm resistor on a 5 V pin lands them all in a
    /// visible range rather than at the bottom of the brightness mapping.
    pub fn red() -> Self {
        Self {
            saturation_current: 1e-20,
            nominal_current: 0.010,
            ..Self::default()
        }
    }

    /// Illustrative green indicator; see [`LedParameters::red`].
    pub fn green() -> Self {
        Self {
            saturation_current: 1.4e-24,
            nominal_current: 0.010,
            ..Self::default()
        }
    }

    /// Illustrative blue indicator; see [`LedParameters::red`].
    pub fn blue() -> Self {
        Self {
            saturation_current: 6.1e-28,
            nominal_current: 0.010,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Led {
    parameters: LedParameters,
    scale: f64,
    shunt_conductance: f64,
}
impl Led {
    pub fn new(parameters: LedParameters) -> Result<Self, ModelError> {
        positive_finite(parameters.saturation_current, "LED saturation current")?;
        positive_finite(parameters.ideality_factor, "LED ideality factor")?;
        positive_finite(parameters.thermal_voltage, "LED thermal voltage")?;
        positive_finite(parameters.nominal_current, "LED nominal current")?;
        positive_finite(parameters.shunt_resistance, "LED shunt resistance")?;
        let scale = positive_finite(
            parameters.ideality_factor * parameters.thermal_voltage,
            "LED n*Vt",
        )?;
        positive_finite(
            parameters.saturation_current / scale,
            "LED junction conductance",
        )?;
        let shunt_conductance =
            positive_finite(parameters.shunt_resistance.recip(), "LED shunt conductance")?;
        let led = Self {
            parameters,
            scale,
            shunt_conductance,
        };
        if !(parameters.saturation_current / scale + shunt_conductance).is_finite() {
            return Err(ModelError::InvalidParameter("LED total conductance"));
        }
        Ok(led)
    }
    /// An illustrative red indicator LED (~2.14 V at 10 mA).
    pub fn red() -> Self {
        Self::new(LedParameters::default()).expect("valid default LED parameters")
    }
    pub fn parameters(self) -> LedParameters {
        self.parameters
    }
    /// Rendering hint proportional to *forward* current. Reverse leakage never
    /// lights the LED. This does not cap the solved current or prevent damage.
    pub fn brightness(self, current: f64) -> f64 {
        if !current.is_finite() {
            return 0.0;
        }
        (current / self.parameters.nominal_current).clamp(0.0, 1.0)
    }
}
impl Device for Led {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError> {
        if !voltage.is_finite() {
            return Err(ModelError::OutOfRange("LED voltage"));
        }
        let exponent = voltage / self.scale;
        // Log-domain evaluation avoids overflowing exp(V/nVt) before multiplying
        // by a tiny Is. expm1 near zero preserves the signed leakage accurately.
        let exponential_current = (self.parameters.saturation_current.ln() + exponent).exp();
        let junction_current = if exponent.abs() < 0.5 {
            self.parameters.saturation_current * exponent.exp_m1()
        } else {
            exponential_current - self.parameters.saturation_current
        };
        let current = junction_current + voltage * self.shunt_conductance;
        let conductance = exponential_current / self.scale + self.shunt_conductance;
        if !current.is_finite() || !conductance.is_finite() {
            return Err(ModelError::OutOfRange("LED exponential current"));
        }
        Ok(Linearization {
            current,
            conductance,
        })
    }
}

/// A mechanical switch: two contacts, either touching or apart.
///
/// A switch is **not a wire**, and modelling it as one would be wrong in both
/// directions. A closed contact really does have resistance, and an open one
/// really does insulate rather than perfectly disconnect, so both states are
/// modelled as finite resistances with datasheet-style values for a tactile
/// switch: a few tens of milliohms closed, and hundreds of megohms open.
///
/// That keeps the topology fixed, so toggling a switch never has to merge or
/// split solver nodes. The two states differ only in conductance, which the host
/// turns into a recompiled circuit because `Circuit` has no mutable device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchParameters {
    /// Contact resistance while closed, in ohms.
    pub closed_ohms: f64,
    /// Insulation resistance while open, in ohms.
    pub open_ohms: f64,
}

impl Default for SwitchParameters {
    fn default() -> Self {
        Self {
            // Tactile switches are typically specified at <= 100 mohm contact
            // resistance and >= 100 Mohm insulation resistance.
            closed_ohms: 0.05,
            open_ohms: 100e6,
        }
    }
}

/// A two-state switch, memoryless in the electrical sense.
#[derive(Clone, Copy, Debug)]
pub struct Switch {
    parameters: SwitchParameters,
    closed: bool,
    closed_conductance: f64,
    open_conductance: f64,
}

impl Switch {
    /// An open switch with the given contact and insulation resistances.
    pub fn new(parameters: SwitchParameters) -> Result<Self, ModelError> {
        positive_finite(parameters.closed_ohms, "switch contact resistance")?;
        positive_finite(parameters.open_ohms, "switch insulation resistance")?;
        if parameters.open_ohms <= parameters.closed_ohms {
            return Err(ModelError::InvalidParameter(
                "switch insulation must exceed contact resistance",
            ));
        }
        let closed_conductance =
            positive_finite(parameters.closed_ohms.recip(), "switch contact conductance")?;
        let open_conductance = positive_finite(
            parameters.open_ohms.recip(),
            "switch insulation conductance",
        )?;
        Ok(Self {
            parameters,
            closed: false,
            closed_conductance,
            open_conductance,
        })
    }

    /// An open tactile switch: 50 mohm closed, 100 Mohm open.
    pub fn button() -> Self {
        Self::new(SwitchParameters::default()).expect("valid default switch parameters")
    }

    /// Set the contact state; a chainable convenience for construction.
    pub fn with_closed(mut self, closed: bool) -> Self {
        self.closed = closed;
        self
    }

    /// Open or close the contact.
    pub fn set_closed(&mut self, closed: bool) {
        self.closed = closed;
    }

    /// Whether the contacts are touching.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The contact and insulation resistances.
    pub fn parameters(&self) -> SwitchParameters {
        self.parameters
    }
}

impl Device for Switch {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError> {
        let conductance = if self.closed {
            self.closed_conductance
        } else {
            self.open_conductance
        };
        let current = voltage * conductance;
        if !current.is_finite() {
            return Err(ModelError::OutOfRange("switch voltage/current"));
        }
        Ok(Linearization {
            current,
            conductance,
        })
    }
}
