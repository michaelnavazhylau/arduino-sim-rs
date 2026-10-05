// SPDX-License-Identifier: MIT

//! Three indicator LEDs on a breadboard, wired to the Uno.
//!
//! Same scale as every other view: **one unit is 10 mm**. A 5 mm LED, its two
//! 2.54 mm-spaced legs, a 6.3 mm 330 Ω axial resistor and an 83 x 55 mm
//! half-size breadboard are all at their real size, so the three columns sit
//! 24 mm apart because that is what fits.
//!
//! The Uno is drawn by [`crate::board3d::Board3D::draw_board`] and the jumpers
//! land on real header pins via [`crate::board3d::header_pin`], so this view
//! shares the board geometry rather than modelling it a second time.
//!
//! Brightness per column comes from the *solved* forward current, not from the
//! pin state: an LED on a pin that the firmware has driven high still looks dark
//! if the circuit is open.

use crate::board3d::{self, Board3D, Header};
use crate::components3d::{led_body, resistor, LedPalette};
use crate::shading;
use crate::wires3d::{tube, wire};
use raylib::prelude::*;

/// Where the Uno sits relative to the breadboard.
pub const BOARD_OFFSET: Vector3 = Vector3 {
    x: 0.0,
    y: 0.0,
    z: -7.0,
};

/// Half-size breadboard: 83 x 55 mm, 8.5 mm of plastic.
const BREADBOARD_W: f32 = 8.3;
const BREADBOARD_D: f32 = 5.5;
const BREADBOARD_H: f32 = 0.85;
/// Top surface everything plugs into.
const DECK: f32 = BREADBOARD_H;
/// Power rails run along the long edges.
const RAIL_Z: f32 = 2.30;
/// Insulation-displacement centre channel.
const CHANNEL_Z: f32 = 0.0;

/// The three component columns.
const COLUMN_X: [f32; 3] = [-2.4, 0.0, 2.4];
/// Axial 330 Ω resistor: 6.3 mm body, so 9.8 mm lead to lead.
const RESISTOR_BODY_R: f32 = 0.115;
const RESISTOR_SPAN: f32 = 0.98;
const RESISTOR_Z: f32 = -1.50;
/// The LED stands in front of its resistor, legs showing.
const LED_Z: f32 = -0.30;
const LED_LEG_OFFSET: f32 = 0.127;
const LED_FOOT_Y: f32 = DECK + 0.16;

const BAND_ORANGE: Color = Color::new(238, 118, 32, 255);
const BAND_BROWN: Color = Color::new(112, 58, 26, 255);
const BAND_GOLD: Color = Color::new(218, 177, 72, 255);
/// Orange, orange, brown, gold = 33 * 10 Ω, ±5%.
const BANDS_330: [Color; 4] = [BAND_ORANGE, BAND_ORANGE, BAND_BROWN, BAND_GOLD];

const PLASTIC: Color = Color::new(206, 206, 202, 255);
const PLASTIC_EDGE: Color = Color::new(176, 176, 172, 255);
const CHANNEL: Color = Color::new(150, 150, 148, 255);
const RAIL_RED: Color = Color::new(150, 62, 58, 255);
const RAIL_BLUE: Color = Color::new(58, 76, 138, 255);
/// Metal of a breadboard contact, shown as a thin slot strip.
const CONTACT: Color = Color::new(150, 150, 156, 255);

const JUMPER_RED: Color = Color::new(214, 62, 52, 255);
const JUMPER_GREEN: Color = Color::new(72, 190, 96, 255);
const JUMPER_BLUE: Color = Color::new(66, 120, 226, 255);
const JUMPER_BLACK: Color = Color::new(48, 50, 56, 255);

/// The palettes the three columns draw with, in firmware order.
const PALETTES: [LedPalette; 3] = [LedPalette::RED, LedPalette::GREEN, LedPalette::BLUE];
/// Jumper colour per column, so the wiring reads at a glance.
const JUMPERS: [Color; 3] = [JUMPER_RED, JUMPER_GREEN, JUMPER_BLUE];

/// What the renderer needs to know about the current operating point.
#[derive(Clone, Copy, Debug)]
pub struct LedView {
    /// Brightness of each column, `0..=1`, in firmware order. The halo and the
    /// body colour both follow it, so "dark" needs no separate flag.
    pub brightness: [f32; 3],
}

/// Camera target and distance that frame the board and the breadboard together.
pub fn frame() -> (Vector3, f32) {
    let back = BOARD_OFFSET.z - board3d::BOARD_DEPTH / 2.0;
    (
        Vector3::new(0.0, 0.5, (back + BREADBOARD_D / 2.0) * 0.5),
        24.0,
    )
}

/// The three-LED rig: breadboard geometry, shader and shared unit meshes.
pub struct Leds3D {
    /// Owned so the materials' shader references outlive the models.
    _shader: Shader,
    cube: Model,
    cylinder: Model,
    sphere: Model,
    parts: Vec<Part>,
}

/// One transformed instance of a shared unit mesh.
///
/// Every static part in this scene is a cube: the breadboard is the only
/// geometry here, and the LEDs and resistors come from `components3d`.
struct Part {
    center: Vector3,
    scale: Vector3,
    color: Color,
}

impl Part {
    fn cube(center: Vector3, size: Vector3, color: Color) -> Self {
        Self {
            center,
            scale: size,
            color,
        }
    }
}

impl Leds3D {
    pub fn new(rl: &mut RaylibHandle, thread: &RaylibThread) -> Self {
        let shader = shading::lit(rl, thread);
        let mut cube = mesh_model(rl, thread, Mesh::gen_mesh_cube(thread, 1.0, 1.0, 1.0));
        let mut cylinder = mesh_model(rl, thread, Mesh::gen_mesh_cylinder(thread, 1.0, 1.0, 24));
        shading::apply(&mut cube, &shader);
        shading::apply(&mut cylinder, &shader);
        let sphere = mesh_model(rl, thread, Mesh::gen_mesh_sphere(thread, 1.0, 12, 12));

        Self {
            _shader: shader,
            cube,
            cylinder,
            sphere,
            parts: breadboard_parts(),
        }
    }

    /// Draw the Uno, the breadboard, the three columns and the jumpers.
    pub fn draw<D: RaylibDraw3D>(&self, d: &mut D, board: &Board3D, view: &LedView) {
        board.draw_board(d, BOARD_OFFSET);

        let spin = Vector3::new(0.0, 1.0, 0.0);
        for part in &self.parts {
            d.draw_model_ex(&self.cube, part.center, spin, 0.0, part.scale, part.color);
        }

        self.draw_columns(d, view);
        self.draw_jumpers(d);
    }

    /// One resistor and one LED per column, plus the legs between them.
    fn draw_columns<D: RaylibDraw3D>(&self, d: &mut D, view: &LedView) {
        for (index, x) in COLUMN_X.iter().copied().enumerate() {
            let lead_y = DECK + RESISTOR_BODY_R;
            let near = Vector3::new(x, lead_y, RESISTOR_Z - RESISTOR_SPAN / 2.0);
            let far = Vector3::new(x, lead_y, RESISTOR_Z + RESISTOR_SPAN / 2.0);
            resistor(d, &self.cylinder, near, far, RESISTOR_BODY_R, BANDS_330);

            // The LED stands in front of the resistor with its legs showing.
            let anode = Vector3::new(x - LED_LEG_OFFSET, DECK, LED_Z);
            let cathode = Vector3::new(x + LED_LEG_OFFSET, DECK, LED_Z);
            tube(
                d,
                &self.cylinder,
                anode,
                Vector3::new(anode.x, LED_FOOT_Y + 0.06, anode.z),
                0.026,
                crate::components3d::LEAD,
            );
            tube(
                d,
                &self.cylinder,
                cathode,
                Vector3::new(cathode.x, LED_FOOT_Y + 0.06, cathode.z),
                0.026,
                crate::components3d::LEAD,
            );
            led_body(
                d,
                &self.cylinder,
                &self.sphere,
                Vector3::new(x, LED_FOOT_Y, LED_Z),
                1.0,
                PALETTES[index],
                view.brightness[index],
            );

            // Resistor output to the LED anode.
            wire(
                d,
                &self.cylinder,
                &self.sphere,
                &[far, Vector3::new(x, lead_y, LED_Z), anode],
                0.045,
                JUMPERS[index],
            );
            // LED cathode down the lane to the ground rail.
            let rail = Vector3::new(x, lead_y, RAIL_Z);
            wire(
                d,
                &self.cylinder,
                &self.sphere,
                &[cathode, Vector3::new(x, lead_y, 1.30), rail],
                0.045,
                JUMPER_BLACK,
            );
        }
    }

    /// Signal jumpers from the digital header, and one ground to the rail.
    fn draw_jumpers<D: RaylibDraw3D>(&self, d: &mut D) {
        let pin = |index: usize| board3d::header_pin(Header::Digital, index) + BOARD_OFFSET;
        // Digital header order is D8..D13, GND, AREF, SDA, SCL, so D9, D10 and
        // D11 are pins 1, 2 and 3.
        for (index, x) in COLUMN_X.iter().copied().enumerate() {
            let from = pin(index + 1);
            let to = Vector3::new(x, DECK + RESISTOR_BODY_R, RESISTOR_Z - RESISTOR_SPAN / 2.0);
            let arch = 1.5 + index as f32 * 0.25;
            wire(
                d,
                &self.cylinder,
                &self.sphere,
                &[
                    from,
                    Vector3::new(from.x, arch, from.z),
                    Vector3::new(to.x, arch, to.z),
                    to,
                ],
                0.06,
                JUMPERS[index],
            );
        }

        // Ground comes off the same header, so it stays clear of the signals.
        let ground_pin = pin(6);
        let rail = Vector3::new(3.6, DECK + RESISTOR_BODY_R, RAIL_Z);
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                ground_pin,
                Vector3::new(ground_pin.x, 2.3, ground_pin.z),
                Vector3::new(rail.x, 2.3, ground_pin.z),
                rail,
            ],
            0.06,
            JUMPER_BLACK,
        );
    }
}

/// Load a generated mesh into a [`Model`].
fn mesh_model(rl: &mut RaylibHandle, thread: &RaylibThread, mesh: Mesh) -> Model {
    rl.load_model_from_mesh(thread, unsafe { mesh.make_weak() })
        .expect("load model from generated mesh")
}

/// The breadboard: a slab, the centre channel, and the two power rails.
///
/// Holes are represented as contact strips rather than several hundred
/// individual cubes: at this scale a strip reads the same and costs three draw
/// calls instead of six hundred.
fn breadboard_parts() -> Vec<Part> {
    let mut parts = vec![
        Part::cube(
            Vector3::new(0.0, BREADBOARD_H / 2.0, 0.0),
            Vector3::new(BREADBOARD_W, BREADBOARD_H, BREADBOARD_D),
            PLASTIC,
        ),
        // Centre channel, where a DIP package would straddle.
        Part::cube(
            Vector3::new(0.0, BREADBOARD_H + 0.01, CHANNEL_Z),
            Vector3::new(BREADBOARD_W - 0.6, 0.04, 0.30),
            CHANNEL,
        ),
    ];

    // Contact strips, five per half, standing in for the hole rows.
    for half in [-1.0_f32, 1.0] {
        for row in 0..5 {
            let z = half * (0.45 + row as f32 * 0.254);
            if z.abs() > RAIL_Z - 0.35 {
                continue;
            }
            parts.push(Part::cube(
                Vector3::new(0.0, BREADBOARD_H + 0.01, z),
                Vector3::new(BREADBOARD_W - 0.6, 0.03, 0.06),
                CONTACT,
            ));
        }
    }

    // Power rails: the conventional red and blue lines along the long edges.
    for (z, colour) in [(RAIL_Z, RAIL_RED), (-RAIL_Z, RAIL_BLUE)] {
        parts.push(Part::cube(
            Vector3::new(0.0, BREADBOARD_H + 0.012, z),
            Vector3::new(BREADBOARD_W - 0.6, 0.025, 0.42),
            colour,
        ));
        parts.push(Part::cube(
            Vector3::new(0.0, BREADBOARD_H + 0.006, z),
            Vector3::new(BREADBOARD_W - 0.6, 0.02, 0.06),
            PLASTIC_EDGE,
        ));
    }
    parts
}
