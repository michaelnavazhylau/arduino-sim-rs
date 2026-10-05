// SPDX-License-Identifier: MIT

//! Fixed D13 -> 220 ohm -> LED -> GND demonstration circuit.
//!
//! Electrical topology and operating points live in `analog.rs`; this module
//! only renders them. External LED brightness comes from forward current, and
//! terminal positions swap when its polarity is reversed.

use crate::components3d::{led_body, resistor, LedPalette, LEAD};
use crate::wires3d::{ball, tube, wire};
use raylib::prelude::*;

// The ten-position digital header is displayed as D8..D13, GND, AREF, SDA, SCL.
const PIN_Y: f32 = 0.59;
const PIN_Z: f32 = 2.30;
const HEADER_START: f32 = 1.18 - 9.0 * 0.254 / 2.0;
const RED: Color = Color::new(220, 46, 38, 255);
const BLACK: Color = Color::new(65, 69, 78, 255);
const SIGNAL: Color = Color::new(235, 154, 40, 255);
// Red, red, brown, gold = 22 * 10 ohm, +/- 5%.
const BANDS: [Color; 4] = [
    RED,
    RED,
    Color::new(112, 58, 26, 255),
    Color::new(218, 177, 72, 255),
];
/// The part's own colours; the LED body is shared with the three-LED bank.
const LED: LedPalette = LedPalette::RED;

/// Draw the external circuit using the board's existing unit meshes.
pub fn draw<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    sphere: &Model,
    brightness: f32,
    reversed: bool,
) {
    let d13 = Vector3::new(HEADER_START + 5.0 * 0.254, PIN_Y, PIN_Z);
    let ground = Vector3::new(HEADER_START + 6.0 * 0.254, PIN_Y, PIN_Z);
    let resistor_in = Vector3::new(-0.25, 0.52, 4.50);
    let resistor_out = Vector3::new(1.15, 0.52, 4.50);
    let signal_terminal = Vector3::new(2.25, 0.52, 4.50);
    let ground_terminal = Vector3::new(2.65, 0.52, 4.50);
    let (anode, cathode) = if reversed {
        (ground_terminal, signal_terminal)
    } else {
        (signal_terminal, ground_terminal)
    };

    // Jumper wires: insulated tube segments with rounded joins. The conductor
    // reaches the actual illustrated header-pin tip, not a point above it.
    wire(
        d,
        cylinder,
        sphere,
        &[
            d13,
            Vector3::new(d13.x, 1.10, PIN_Z),
            Vector3::new(-0.55, 1.10, 3.60),
            Vector3::new(-0.55, 0.52, 4.50),
            resistor_in,
        ],
        0.045,
        RED,
    );
    wire(
        d,
        cylinder,
        sphere,
        &[
            resistor_out,
            Vector3::new(1.65, 0.52, 4.50),
            signal_terminal,
        ],
        0.045,
        SIGNAL,
    );
    wire(
        d,
        cylinder,
        sphere,
        &[
            ground_terminal,
            Vector3::new(3.35, 0.52, 4.50),
            Vector3::new(3.35, 0.90, 3.35),
            Vector3::new(ground.x, 0.90, PIN_Z),
            ground,
        ],
        0.045,
        BLACK,
    );

    // Axial resistor: metal leads, tan body, and four colour bands.
    resistor(d, cylinder, resistor_in, resistor_out, 0.14, BANDS);

    // Through-hole LED: leads and polarity markers live here, because they
    // depend on the layout; the body is the shared part.
    tube(
        d,
        cylinder,
        anode,
        Vector3::new(anode.x, 1.12, anode.z),
        0.026,
        LEAD,
    );
    tube(
        d,
        cylinder,
        cathode,
        Vector3::new(cathode.x, 1.04, cathode.z),
        0.026,
        LEAD,
    );
    // Green terminal marker = anode; dark marker = cathode. They swap with P.
    ball(d, sphere, anode, 0.065, Color::new(90, 205, 115, 255));
    ball(d, sphere, cathode, 0.065, BLACK);
    led_body(
        d,
        cylinder,
        sphere,
        Vector3::new(2.45, 1.00, 4.50),
        1.0,
        LED,
        brightness,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_displayed_header_places_d13_five_pitches_from_the_start() {
        let d13 = HEADER_START + 5.0 * 0.254;
        // D13 is the sixth pin of D8..D13, so it must sit right of centre.
        assert!(d13 > 0.0 && d13 < PIN_Z);
        assert!((d13 - 1.307).abs() < 1e-3, "D13 at {d13}");
    }
}
