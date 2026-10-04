// SPDX-License-Identifier: MIT

//! Electrical models only: no raylib, AVR, geometry or firmware dependencies.
//!
//! SI units everywhere. For both devices V = Vpositive - Vnegative and I flows
//! positive -> negative. For an LED these terminals are anode -> cathode.
//! The default red LED is illustrative, not a fitted model of a specific part.

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
