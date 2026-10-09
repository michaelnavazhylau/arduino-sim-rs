// SPDX-License-Identifier: MIT

//! Illustrative LED presets: SPICE diode parameters plus a brightness mapping.
//!
//! An LED is a diode with a visible operating point, so the electrical model is
//! a plain SPICE diode: `I = Is * (exp(V / (N * Vt)) - 1)`. As in the retired
//! in-tree model, a small explicit shunt resistor sits in parallel with the
//! junction, standing in for reverse leakage and anchoring the cathode path to
//! ground so an undriven LED node is not left floating.
//!
//! Colour is almost entirely a difference in **saturation current**, which spans
//! nine orders of magnitude between red and blue because the band gap does.
//! Everything else is held fixed, so the three presets differ in exactly one
//! physical parameter. They are illustrative curves, *not* fitted data for any
//! specific part.
//!
//! Brightness is a rendering normalization, not a clamp on the solved current:
//! it never limits conduction, it only decides how bright to draw the part.

/// Model parameters and rendering normalization for one LED colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedPreset {
    /// Colour name, as shown on screen.
    pub name: &'static str,
    /// Diode saturation current `IS`, in amperes.
    pub saturation_current: f64,
    /// Diode ideality factor `N`.
    pub ideality_factor: f64,
    /// Explicit junction shunt resistance, in ohms.
    pub shunt_ohms: f64,
    /// Forward current drawn at full brightness, in amperes.
    pub nominal_current: f64,
}

impl LedPreset {
    /// Illustrative red indicator: about 2.13 V at 8 mA.
    pub fn red() -> Self {
        Self {
            name: "red",
            saturation_current: 1e-20,
            ..Self::default()
        }
    }

    /// Illustrative green indicator; see [`LedPreset::red`].
    pub fn green() -> Self {
        Self {
            name: "green",
            saturation_current: 1.4e-24,
            ..Self::default()
        }
    }

    /// Illustrative blue indicator; see [`LedPreset::red`].
    pub fn blue() -> Self {
        Self {
            name: "blue",
            saturation_current: 6.1e-28,
            ..Self::default()
        }
    }

    /// The `DLED_<reference>` model name [`LedPreset::cards`] emits.
    pub fn model_name(reference: &str) -> String {
        format!("DLED_{reference}")
    }

    /// The same electrical curve with a different full-brightness current.
    ///
    /// `nominal_current` is a rendering normalization only, so two demos can
    /// share one diode curve and still be drawn at different scales.
    pub fn with_nominal_current(mut self, amperes: f64) -> Self {
        self.nominal_current = amperes;
        self
    }

    /// The `.model`, diode and shunt cards for one LED, as deck text.
    ///
    /// `reference` is the diode's designator (`D1`), `anode` and `cathode` are
    /// its nets. Keeping the three cards together is what guarantees every LED
    /// carries the explicit shunt its model relies on.
    pub fn cards(&self, reference: &str, anode: &str, cathode: &str) -> String {
        let model = Self::model_name(reference);
        // `{:e}` keeps a nine-decade span of saturation currents readable and is
        // unambiguous to the SPICE parser, unlike engineering suffixes such as
        // `M`, which ngspice reads as milli.
        format!(
            ".model {model} D(IS={:e} N={})\n{reference} {anode} {cathode} {model}\nRsh_{reference} {anode} {cathode} {:e}",
            self.saturation_current, self.ideality_factor, self.shunt_ohms
        )
    }

    /// Rendering brightness in `0..=1`, proportional to *forward* current.
    ///
    /// Reverse leakage never lights the LED. This is a display mapping only: it
    /// does not cap the solved current or model device damage.
    pub fn brightness(&self, current: f64) -> f64 {
        if !current.is_finite() {
            return 0.0;
        }
        (current / self.nominal_current).clamp(0.0, 1.0)
    }
}

impl Default for LedPreset {
    fn default() -> Self {
        Self {
            name: "led",
            saturation_current: 1e-20,
            ideality_factor: 2.0,
            shunt_ohms: 1e12,
            nominal_current: 0.010,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spice::solve_op;

    /// A 5 V pad behind 25 ohm into 330 ohm and the LED.
    fn lit(preset: &LedPreset) -> (f64, f64) {
        let deck = format!(
            "one led\nV_MCU_D9 ndrv 0 5\nR_MCU_D9 ndrv p 25\nR1 p a 330\n{}\n.op\n.end\n",
            preset.cards("D1", "a", "0")
        );
        let solution = solve_op(&deck).expect("solves");
        let pin = solution.net_voltage("p").expect("p");
        let anode = solution.net_voltage("a").expect("a");
        (anode, (pin - anode) / 330.0)
    }

    #[test]
    fn equal_series_resistors_give_unequal_operating_points_per_colour() {
        let (red_v, red_i) = lit(&LedPreset::red());
        let (green_v, green_i) = lit(&LedPreset::green());
        let (blue_v, blue_i) = lit(&LedPreset::blue());
        assert!(
            red_v < green_v && green_v < blue_v,
            "{red_v} {green_v} {blue_v}"
        );
        assert!(blue_v - red_v > 0.5, "{blue_v} - {red_v}");
        assert!(
            red_i > green_i && green_i > blue_i,
            "{red_i} {green_i} {blue_i}"
        );
        assert!(red_i.is_finite() && red_i > 1e-3);
    }

    #[test]
    fn brightness_saturates_at_the_nominal_current_and_ignores_reverse_leakage() {
        let preset = LedPreset::red();
        assert_eq!(preset.brightness(0.0), 0.0);
        assert_eq!(preset.brightness(-1e-6), 0.0);
        assert!((preset.brightness(0.005) - 0.5).abs() < 1e-12);
        assert_eq!(preset.brightness(1.0), 1.0);
        assert_eq!(preset.brightness(f64::NAN), 0.0);
    }

    #[test]
    fn every_led_card_carries_its_shunt() {
        let cards = LedPreset::red().cards("D1", "a", "0");
        assert!(cards.contains("D1 a 0 DLED_D1"));
        assert!(cards.contains(".model DLED_D1 D(IS=1e-20 N=2)"));
        assert!(cards.contains("Rsh_D1 a 0 1e12"));
    }
}
