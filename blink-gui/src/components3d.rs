// SPDX-License-Identifier: MIT

//! Shared drawings for the discrete parts both 3D scenes use.
//!
//! An axial resistor and a through-hole indicator LED appear in the blink
//! circuit **and** in the three-LED bank, at different sizes and in different
//! colours. The geometry therefore lives here, parameterised, rather than being
//! duplicated with one copy quietly drifting from the other.
//!
//! Configurable by design: `resistor` derives its leads and its four colour
//! bands from the extent and the body radius, so one call covers a 2.3 mm
//! indicator part or a 6.3 mm one, and `led_body` takes a per-colour palette.

use crate::wires3d::{ball, tube};
use raylib::prelude::*;

/// Metal of a component lead.
pub const LEAD: Color = Color::new(192, 196, 206, 255);
/// Axial resistor body: tan phenolic.
pub const RESISTOR_BODY: Color = Color::new(208, 184, 126, 255);

/// Linear interpolation between two points.
///
/// Written out component-wise rather than relying on operator overloads, so the
/// segment maths is explicit about what it is doing.
pub fn lerp(from: Vector3, to: Vector3, t: f32) -> Vector3 {
    Vector3::new(
        from.x + (to.x - from.x) * t,
        from.y + (to.y - from.y) * t,
        from.z + (to.z - from.z) * t,
    )
}

/// The colours an indicator LED takes as it goes from dark to fully lit.
///
/// The lit colour is deliberately brighter than the die colour, because a
/// saturated LED emits rather than merely reflecting: rendering it as flat
/// plastic would make "on" and "off" differ only in shade.
#[derive(Clone, Copy, Debug)]
pub struct LedPalette {
    /// Body colour with no current.
    pub off: Color,
    /// Body colour at full brightness.
    pub on: Color,
    /// Halo colour; its alpha scales with brightness.
    pub glow: Color,
}

impl LedPalette {
    /// Illustrative red indicator.
    pub const RED: Self = Self {
        off: Color::new(94, 22, 18, 255),
        on: Color::new(255, 92, 48, 255),
        glow: Color::new(255, 72, 35, 255),
    };
    /// Illustrative green indicator.
    pub const GREEN: Self = Self {
        off: Color::new(20, 58, 28, 255),
        on: Color::new(96, 248, 140, 255),
        glow: Color::new(64, 240, 120, 255),
    };
    /// Illustrative blue indicator.
    pub const BLUE: Self = Self {
        off: Color::new(24, 40, 84, 255),
        on: Color::new(110, 176, 255, 255),
        glow: Color::new(84, 150, 255, 255),
    };

    /// Body colour at `brightness`, clamped to `0..=1`.
    pub fn body(self, brightness: f32) -> Color {
        let t = brightness.clamp(0.0, 1.0);
        let channel = |low: u8, high: u8| (low as f32 + (high as f32 - low as f32) * t) as u8;
        Color::new(
            channel(self.off.r, self.on.r),
            channel(self.off.g, self.on.g),
            channel(self.off.b, self.on.b),
            255,
        )
    }

    /// Halo colour at `brightness`; fully transparent when dark.
    pub fn halo(self, brightness: f32) -> Color {
        let t = brightness.clamp(0.0, 1.0);
        Color::new(self.glow.r, self.glow.g, self.glow.b, (36.0 * t) as u8)
    }
}

/// Draw a through-hole LED body standing on `foot`, scaled by `scale`.
///
/// Draws the flange, the cylindrical body, the dome and the halo. The caller
/// owns the leads and the polarity markers, because those depend on the layout
/// rather than on the part.
pub fn led_body<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    sphere: &Model,
    foot: Vector3,
    scale: f32,
    palette: LedPalette,
    brightness: f32,
) {
    let up = |dy: f32| Vector3::new(foot.x, foot.y + dy * scale, foot.z);
    let colour = palette.body(brightness);
    // Flange, then the narrower body, then the dome.
    tube(d, cylinder, up(0.0), up(0.07), 0.29 * scale, colour);
    tube(d, cylinder, up(0.07), up(0.35), 0.25 * scale, colour);
    ball(d, sphere, up(0.35), 0.25 * scale, colour);
    if brightness > 0.001 {
        ball(d, sphere, up(0.28), 0.48 * scale, palette.halo(brightness));
    }
}

/// Draw an axial resistor between lead tips `from` and `to`.
///
/// The body occupies the middle 64% of the extent, the leads take the rest, and
/// the four colour bands are placed at the fractions a real part uses. Bands are
/// `[first, second, multiplier, tolerance]`.
pub fn resistor<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    from: Vector3,
    to: Vector3,
    radius: f32,
    bands: [Color; 4],
) {
    let body_start = lerp(from, to, 0.1786);
    let body_end = lerp(from, to, 0.8214);
    let lead = radius * 0.179;
    tube(d, cylinder, from, body_start, lead, LEAD);
    tube(d, cylinder, body_end, to, lead, LEAD);
    tube(d, cylinder, body_start, body_end, radius, RESISTOR_BODY);
    for (fraction, colour) in [
        (0.178, bands[0]),
        (0.344, bands[1]),
        (0.511, bands[2]),
        (0.811, bands[3]),
    ] {
        tube(
            d,
            cylinder,
            lerp(body_start, body_end, fraction),
            lerp(body_start, body_end, fraction + 0.0722),
            radius * 1.036,
            colour,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resistor_body_spans_the_middle_of_the_extent() {
        let from = Vector3::new(-0.25, 0.52, 4.50);
        let to = Vector3::new(1.15, 0.52, 4.50);
        let start = lerp(from, to, 0.1786);
        let end = lerp(from, to, 0.8214);
        assert!((start.x - 0.0).abs() < 1e-4, "body start {}", start.x);
        assert!((end.x - 0.90).abs() < 1e-4, "body end {}", end.x);
    }

    #[test]
    fn an_unlit_led_is_its_off_colour_and_a_lit_one_is_its_on_colour() {
        assert_eq!(LedPalette::RED.body(0.0), LedPalette::RED.off);
        assert_eq!(LedPalette::RED.body(1.0), LedPalette::RED.on);
        assert_eq!(LedPalette::BLUE.body(0.0), LedPalette::BLUE.off);
        assert_eq!(LedPalette::BLUE.body(1.0), LedPalette::BLUE.on);
        // Clamped, not extrapolated.
        assert_eq!(LedPalette::GREEN.body(-1.0), LedPalette::GREEN.off);
        assert_eq!(LedPalette::GREEN.body(2.0), LedPalette::GREEN.on);
    }

    #[test]
    fn the_halo_fades_to_transparent_when_dark() {
        assert_eq!(LedPalette::RED.halo(0.0).a, 0);
        assert!(LedPalette::RED.halo(1.0).a > 0);
        assert!(LedPalette::RED.halo(0.5).a < LedPalette::RED.halo(1.0).a);
    }
}
