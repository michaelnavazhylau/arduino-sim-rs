// SPDX-License-Identifier: MIT

//! Fixed D13 -> 220 ohm -> LED -> GND demonstration circuit.
//!
//! Electrical topology and operating points live in `analog.rs`; this module
//! only renders them. External LED brightness comes from forward current, and
//! terminal positions swap when its polarity is reversed.

use raylib::prelude::*;

// The ten-position digital header is displayed as D8..D13, GND, AREF, SDA, SCL.
const PIN_Y: f32 = 0.59;
const PIN_Z: f32 = 2.30;
const HEADER_START: f32 = 1.18 - 9.0 * 0.254 / 2.0;
const RED: Color = Color::new(220, 46, 38, 255);
const BLACK: Color = Color::new(65, 69, 78, 255);
const SIGNAL: Color = Color::new(235, 154, 40, 255);
const METAL: Color = Color::new(192, 196, 206, 255);
// Red, red, brown, gold = 22 * 10 ohm, +/- 5%.
const BANDS: [Color; 4] = [
    RED,
    RED,
    Color::new(112, 58, 26, 255),
    Color::new(218, 177, 72, 255),
];

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
        BLACK,
    );

    // Axial resistor: metal leads, tan body, and four colour bands.
    let body_start = Vector3::new(0.0, 0.52, 4.50);
    let body_end = Vector3::new(0.90, 0.52, 4.50);
    tube(d, cylinder, resistor_in, body_start, 0.025, METAL);
    tube(d, cylinder, body_end, resistor_out, 0.025, METAL);
    tube(
        d,
        cylinder,
        body_start,
        body_end,
        0.14,
        Color::new(208, 184, 126, 255),
    );
    for (x, colour) in [0.16, 0.31, 0.46, 0.73].into_iter().zip(BANDS) {
        tube(
            d,
            cylinder,
            Vector3::new(x, 0.52, 4.50),
            Vector3::new(x + 0.065, 0.52, 4.50),
            0.145,
            colour,
        );
    }

    // Through-hole LED: two distinct leads, flange, cylindrical body and dome.
    tube(
        d,
        cylinder,
        anode,
        Vector3::new(anode.x, 1.12, anode.z),
        0.026,
        METAL,
    );
    tube(
        d,
        cylinder,
        cathode,
        Vector3::new(cathode.x, 1.04, cathode.z),
        0.026,
        METAL,
    );
    // Green terminal marker = anode; dark marker = cathode. They swap with P.
    ball(d, sphere, anode, 0.065, Color::new(90, 205, 115, 255));
    ball(d, sphere, cathode, 0.065, BLACK);
    let colour = led_colour(brightness);
    tube(
        d,
        cylinder,
        Vector3::new(2.45, 1.00, 4.50),
        Vector3::new(2.45, 1.07, 4.50),
        0.29,
        colour,
    );
    tube(
        d,
        cylinder,
        Vector3::new(2.45, 1.07, 4.50),
        Vector3::new(2.45, 1.35, 4.50),
        0.25,
        colour,
    );
    ball(d, sphere, Vector3::new(2.45, 1.35, 4.50), 0.25, colour);
    if brightness > 0.001 {
        ball(
            d,
            sphere,
            Vector3::new(2.45, 1.28, 4.50),
            0.48,
            Color::new(255, 72, 35, (36.0 * brightness.clamp(0.0, 1.0)) as u8),
        );
    }
}

pub fn led_colour(brightness: f32) -> Color {
    let t = brightness.clamp(0.0, 1.0);
    Color::new(
        (94.0 + 161.0 * t) as u8,
        (22.0 + 70.0 * t) as u8,
        (18.0 + 30.0 * t) as u8,
        255,
    )
}

fn wire<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    sphere: &Model,
    points: &[Vector3],
    colour: Color,
) {
    for pair in points.windows(2) {
        tube(d, cylinder, pair[0], pair[1], 0.045, colour);
    }
    for point in points {
        ball(d, sphere, *point, 0.045, colour);
    }
}

fn ball<D: RaylibDraw3D>(d: &mut D, sphere: &Model, position: Vector3, radius: f32, colour: Color) {
    d.draw_model(sphere, position, radius, colour);
}

/// Rotate a +Y unit cylinder onto a segment. Generated raylib cylinders run
/// from y=0 to y=1 (not -0.5 to +0.5), so their draw position is the start.
fn segment_transform(start: Vector3, end: Vector3) -> Option<(Vector3, f32, f32)> {
    let v = Vector3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    let length = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if length < 0.00001 {
        return None;
    }
    // Cross product of +Y with the segment direction.
    let mut axis = Vector3::new(v.z, 0.0, -v.x);
    let axis_length = (axis.x * axis.x + axis.z * axis.z).sqrt();
    if axis_length < 0.00001 {
        axis = Vector3::new(1.0, 0.0, 0.0);
    } else {
        axis.x /= axis_length;
        axis.z /= axis_length;
    }
    let angle = (v.y / length).clamp(-1.0, 1.0).acos().to_degrees();
    Some((axis, angle, length))
}

fn tube<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    start: Vector3,
    end: Vector3,
    radius: f32,
    colour: Color,
) {
    if let Some((axis, angle, length)) = segment_transform(start, end) {
        d.draw_model_ex(
            cylinder,
            start,
            axis,
            angle,
            Vector3::new(radius, length, radius),
            colour,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cylinder_rotation_reaches_segment_endpoint() {
        let start = Vector3::new(0.3, 0.4, 0.5);
        for direction in [
            Vector3::new(0.0, 2.0, 0.0),
            Vector3::new(0.0, -2.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, -2.0),
            Vector3::new(-1.0, 0.7, 2.0),
        ] {
            let end = Vector3::new(
                start.x + direction.x,
                start.y + direction.y,
                start.z + direction.z,
            );
            let (axis, angle, length) = segment_transform(start, end).unwrap();
            let (sin, cos) = angle.to_radians().sin_cos();
            // Rodrigues rotation of (0,length,0), with axis.y == 0.
            let rotated = Vector3::new(-axis.z * length * sin, length * cos, axis.x * length * sin);
            assert!((rotated.x - direction.x).abs() < 0.0001);
            assert!((rotated.y - direction.y).abs() < 0.0001);
            assert!((rotated.z - direction.z).abs() < 0.0001);
        }
    }

    #[test]
    fn zero_length_wire_segments_are_skipped() {
        assert!(segment_transform(Vector3::zero(), Vector3::zero()).is_none());
    }

    #[test]
    fn external_led_colour_follows_current_brightness() {
        assert!(led_colour(1.0).r > led_colour(0.5).r);
        assert!(led_colour(0.5).r > led_colour(0.0).r);
        assert_eq!(led_colour(1.0).a, 255);
        assert_eq!(led_colour(0.0).a, 255);
        assert_eq!(led_colour(-1.0).r, led_colour(0.0).r);
    }
}
