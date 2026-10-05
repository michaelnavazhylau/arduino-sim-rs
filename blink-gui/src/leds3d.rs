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
use crate::breadboard3d::{Breadboard, DECK, DEPTH, RAIL_Z};
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
    (Vector3::new(0.0, 0.5, (back + DEPTH / 2.0) * 0.5), 24.0)
}

/// The three-LED rig: breadboard, shader and shared unit meshes.
pub struct Leds3D {
    /// Owned so the materials' shader references outlive the models.
    _shader: Shader,
    cube: Model,
    cylinder: Model,
    sphere: Model,
    breadboard: Breadboard,
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
            breadboard: Breadboard::new(),
        }
    }

    /// Draw the Uno, the breadboard, the three columns and the jumpers.
    pub fn draw<D: RaylibDraw3D>(&self, d: &mut D, board: &Board3D, view: &LedView) {
        board.draw_board(d, BOARD_OFFSET);
        self.breadboard.draw(d, &self.cube);
        self.draw_columns(d, view);
        self.draw_jumpers(d);
    }

    /// One resistor and one LED per column, plus the legs and links between them.
    fn draw_columns<D: RaylibDraw3D>(&self, d: &mut D, view: &LedView) {
        for (index, x) in COLUMN_X.iter().copied().enumerate() {
            let lead_y = DECK + RESISTOR_BODY_R;
            let near = Vector3::new(x, lead_y, RESISTOR_Z - RESISTOR_SPAN / 2.0);
            let far = Vector3::new(x, lead_y, RESISTOR_Z + RESISTOR_SPAN / 2.0);
            resistor(d, &self.cylinder, near, far, RESISTOR_BODY_R, BANDS_330);

            // The LED stands in front of the resistor with its legs showing.
            let anode = Vector3::new(x - LED_LEG_OFFSET, DECK, LED_Z);
            let cathode = Vector3::new(x + LED_LEG_OFFSET, DECK, LED_Z);
            for leg in [anode, cathode] {
                tube(
                    d,
                    &self.cylinder,
                    leg,
                    Vector3::new(leg.x, LED_FOOT_Y + 0.06, leg.z),
                    0.026,
                    crate::components3d::LEAD,
                );
            }
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
            // LED cathode down the lane to the ground rail. The lane sits outboard
            // of the resistor, so the wire crosses nothing on the way.
            let rail = Vector3::new(x + 0.55, lead_y, -RAIL_Z);
            wire(
                d,
                &self.cylinder,
                &self.sphere,
                &[cathode, Vector3::new(x + 0.55, lead_y, -0.55), rail],
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

        // Ground comes off the same header and lands on the blue rail, which
        // leaves the red rail free for a supply, as the colours advertise.
        let ground_pin = pin(6);
        let rail = Vector3::new(3.6, DECK + RESISTOR_BODY_R, -RAIL_Z);
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                ground_pin,
                Vector3::new(ground_pin.x, 1.9, ground_pin.z),
                Vector3::new(rail.x, 1.9, ground_pin.z),
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
