// SPDX-License-Identifier: MIT

//! A half-size solderless breadboard, shared by the scenes that use one.
//!
//! Same scale as every other view: **one unit is 10 mm**. An 83 x 55 mm
//! breadboard with 8.5 mm of plastic, 2.54 mm contact pitch and its two power
//! rails along the long edges.
//!
//! Contacts are drawn as strips rather than as several hundred individual holes.
//! At this scale a strip reads the same and costs five draw calls per half
//! instead of sixty, and the holes are not what makes a breadboard legible.

use raylib::prelude::*;

/// Breadboard width: 83 mm.
pub const WIDTH: f32 = 8.3;
/// Breadboard depth: 55 mm.
pub const DEPTH: f32 = 5.5;
/// Plastic thickness: 8.5 mm.
pub const HEIGHT: f32 = 0.85;
/// Top surface everything plugs into.
pub const DECK: f32 = HEIGHT;
/// Power rails run along the long edges.
pub const RAIL_Z: f32 = 2.30;
/// Insulation-displacement centre channel.
const CHANNEL_Z: f32 = 0.0;
/// Contact pitch: 2.54 mm.
const PITCH: f32 = 0.254;

const PLASTIC: Color = Color::new(206, 206, 202, 255);
const PLASTIC_EDGE: Color = Color::new(176, 176, 172, 255);
const CHANNEL: Color = Color::new(150, 150, 148, 255);
const RAIL_RED: Color = Color::new(150, 62, 58, 255);
const RAIL_BLUE: Color = Color::new(58, 76, 138, 255);
const CONTACT: Color = Color::new(150, 150, 156, 255);

/// One box of the breadboard.
struct Slab {
    center: Vector3,
    size: Vector3,
    color: Color,
}

impl Slab {
    fn cube(center: Vector3, size: Vector3, color: Color) -> Self {
        Self {
            center,
            size,
            color,
        }
    }
}

/// The breadboard's static geometry.
pub struct Breadboard {
    parts: Vec<Slab>,
}

impl Default for Breadboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadboard {
    /// A breadboard centred on the origin, sitting on the ground plane.
    pub fn new() -> Self {
        Self { parts: slabs() }
    }

    /// Draw every slab using the caller's unit cube.
    pub fn draw<D: RaylibDraw3D>(&self, d: &mut D, cube: &Model) {
        let spin = Vector3::new(0.0, 1.0, 0.0);
        for part in &self.parts {
            d.draw_model_ex(cube, part.center, spin, 0.0, part.size, part.color);
        }
    }
}

fn slabs() -> Vec<Slab> {
    let mut parts = vec![
        Slab::cube(
            Vector3::new(0.0, HEIGHT / 2.0, 0.0),
            Vector3::new(WIDTH, HEIGHT, DEPTH),
            PLASTIC,
        ),
        // Centre channel, where a DIP package would straddle.
        Slab::cube(
            Vector3::new(0.0, HEIGHT + 0.01, CHANNEL_Z),
            Vector3::new(WIDTH - 0.6, 0.04, 0.30),
            CHANNEL,
        ),
    ];

    // Contact strips, five per half, standing in for the hole rows.
    for half in [-1.0_f32, 1.0] {
        for row in 0..5 {
            let z = half * (0.45 + row as f32 * PITCH);
            if z.abs() > RAIL_Z - 0.35 {
                continue;
            }
            parts.push(Slab::cube(
                Vector3::new(0.0, HEIGHT + 0.01, z),
                Vector3::new(WIDTH - 0.6, 0.03, 0.06),
                CONTACT,
            ));
        }
    }

    // Power rails: the conventional red and blue lines along the long edges.
    for (z, colour) in [(RAIL_Z, RAIL_RED), (-RAIL_Z, RAIL_BLUE)] {
        parts.push(Slab::cube(
            Vector3::new(0.0, HEIGHT + 0.012, z),
            Vector3::new(WIDTH - 0.6, 0.025, 0.42),
            colour,
        ));
        parts.push(Slab::cube(
            Vector3::new(0.0, HEIGHT + 0.006, z),
            Vector3::new(WIDTH - 0.6, 0.02, 0.06),
            PLASTIC_EDGE,
        ));
    }
    parts
}
