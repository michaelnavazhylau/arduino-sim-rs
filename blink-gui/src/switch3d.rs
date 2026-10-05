// SPDX-License-Identifier: MIT

//! A push button on a breadboard, wired to the Uno two different ways.
//!
//! Same scale as every other view: **one unit is 10 mm**. A 6 mm tactile switch,
//! its 2.54 mm contact pitch, a 330 Ω indicator resistor and the 10 kΩ pull-down
//! are all at their real size.
//!
//! The switch is drawn as the four-legged package it really is. Electrically it
//! is two contacts — legs `A`/`D` are one, `B`/`C` the other — which is why the
//! wires land on opposite sides of the body and the two legs on each side are
//! interchangeable.
//!
//! The Uno is drawn by [`crate::board3d::Board3D::draw_board`] and the jumpers
//! land on real header pins via [`crate::board3d::header_pin`].

use crate::board3d::{self, Board3D, Header};
use crate::breadboard3d::{Breadboard, DECK, DEPTH, RAIL_Z};
use crate::components3d::{led_body, resistor, LedPalette, LEAD};
use crate::shading;
use crate::wires3d::{tube, wire};
use raylib::prelude::*;

/// Where the Uno sits relative to the breadboard.
pub const BOARD_OFFSET: Vector3 = Vector3 {
    x: 0.0,
    y: 0.0,
    z: -7.0,
};

/// The switch: a 6 mm square package on four legs.
const BUTTON_X: f32 = -2.0;
const BUTTON_Z: f32 = -0.90;
const BODY_W: f32 = 0.62;
const BODY_H: f32 = 0.38;
/// Body bottom above the deck, standing on its legs.
const BODY_LIFT: f32 = 0.30;
/// Leg spacing: 4.5 mm between the two legs of a contact pair.
const LEG_OFFSET: f32 = 0.225;
const LEG_R: f32 = 0.045;
/// How far the stem sinks when the button is held.
const STEM_TRAVEL: f32 = 0.07;

/// The indicator resistor and LED, laid out like the three-LED view.
const LED_X: f32 = 0.9;
const LED_Z: f32 = -0.30;
const LED_LEG_OFFSET: f32 = 0.127;
const LED_FOOT_Y: f32 = DECK + 0.16;
const RESISTOR_BODY_R: f32 = 0.115;
const RESISTOR_SPAN: f32 = 0.98;
const RESISTOR_Z: f32 = -1.50;
/// The 10 kΩ pull-down, in the inverse wiring only.
const PULLDOWN_X: f32 = 2.4;

const BAND_ORANGE: Color = Color::new(238, 118, 32, 255);
const BAND_BROWN: Color = Color::new(112, 58, 26, 255);
const BAND_GOLD: Color = Color::new(218, 177, 72, 255);
/// Orange, orange, brown, gold = 33 * 10 Ω, ±5%.
const BANDS_330: [Color; 4] = [BAND_ORANGE, BAND_ORANGE, BAND_BROWN, BAND_GOLD];
/// Brown, black, orange, gold = 10 * 10^3 Ω, ±5%.
const BANDS_10K: [Color; 4] = [
    Color::new(112, 58, 26, 255),
    Color::new(30, 30, 32, 255),
    BAND_ORANGE,
    BAND_GOLD,
];

const BODY_PLASTIC: Color = Color::new(38, 38, 42, 255);
const STEM: Color = Color::new(214, 58, 52, 255);
const JUMPER_SIGNAL: Color = Color::new(236, 176, 48, 255);
const JUMPER_GROUND: Color = Color::new(48, 50, 56, 255);
const JUMPER_SUPPLY: Color = Color::new(214, 62, 52, 255);

/// What the renderer needs to know about the current state.
#[derive(Clone, Copy, Debug)]
pub struct SwitchView {
    /// True while the button is held down; the stem sinks.
    pub pressed: bool,
    /// Indicator brightness from the **solved** LED current.
    pub led_brightness: f32,
    /// The inverse wiring adds a 10 kΩ pull-down and a 5 V jumper.
    pub pull_down: bool,
}

/// Camera target and distance that frame the board and the breadboard together.
pub fn frame() -> (Vector3, f32) {
    let back = BOARD_OFFSET.z - board3d::BOARD_DEPTH / 2.0;
    (Vector3::new(0.0, 0.5, (back + DEPTH / 2.0) * 0.5), 24.0)
}

/// The switch rig: breadboard, shader and shared unit meshes.
pub struct Switch3D {
    /// Owned so the materials' shader references outlive the models.
    _shader: Shader,
    cube: Model,
    cylinder: Model,
    sphere: Model,
    breadboard: Breadboard,
}

impl Switch3D {
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

    /// Draw the Uno, the breadboard, the button, the LED chain and the jumpers.
    pub fn draw<D: RaylibDraw3D>(&self, d: &mut D, board: &Board3D, view: &SwitchView) {
        board.draw_board(d, BOARD_OFFSET);
        self.breadboard.draw(d, &self.cube);
        self.draw_button(d, view);
        self.draw_led_chain(d, view);
        self.draw_jumpers(d, view);
    }

    /// The tactile switch, with its stem sunk while held.
    fn draw_button<D: RaylibDraw3D>(&self, d: &mut D, view: &SwitchView) {
        let spin = Vector3::new(0.0, 1.0, 0.0);
        let body_base = DECK + BODY_LIFT;
        let body_top = body_base + BODY_H;

        // Four legs, because the package has four and they are what you see.
        for dx in [-LEG_OFFSET, LEG_OFFSET] {
            for dz in [-LEG_OFFSET, LEG_OFFSET] {
                let foot = Vector3::new(BUTTON_X + dx, DECK, BUTTON_Z + dz);
                tube(
                    d,
                    &self.cylinder,
                    foot,
                    Vector3::new(foot.x, body_base, foot.z),
                    LEG_R,
                    LEAD,
                );
            }
        }

        d.draw_model_ex(
            &self.cube,
            Vector3::new(BUTTON_X, body_base + BODY_H / 2.0, BUTTON_Z),
            spin,
            0.0,
            Vector3::new(BODY_W, BODY_H, BODY_W),
            BODY_PLASTIC,
        );

        // The stem: the one part that moves, so travel is visible.
        let sink = if view.pressed { STEM_TRAVEL } else { 0.0 };
        tube(
            d,
            &self.cylinder,
            Vector3::new(BUTTON_X, body_top, BUTTON_Z),
            Vector3::new(BUTTON_X, body_top + 0.09 - sink, BUTTON_Z),
            0.20,
            STEM,
        );
    }

    /// The 330 Ω resistor and the indicator LED it feeds.
    fn draw_led_chain<D: RaylibDraw3D>(&self, d: &mut D, view: &SwitchView) {
        let lead_y = DECK + RESISTOR_BODY_R;
        let near = Vector3::new(LED_X, lead_y, RESISTOR_Z - RESISTOR_SPAN / 2.0);
        let far = Vector3::new(LED_X, lead_y, RESISTOR_Z + RESISTOR_SPAN / 2.0);
        resistor(d, &self.cylinder, near, far, RESISTOR_BODY_R, BANDS_330);

        let anode = Vector3::new(LED_X - LED_LEG_OFFSET, DECK, LED_Z);
        let cathode = Vector3::new(LED_X + LED_LEG_OFFSET, DECK, LED_Z);
        for leg in [anode, cathode] {
            tube(
                d,
                &self.cylinder,
                leg,
                Vector3::new(leg.x, LED_FOOT_Y + 0.06, leg.z),
                0.026,
                LEAD,
            );
        }
        led_body(
            d,
            &self.cylinder,
            &self.sphere,
            Vector3::new(LED_X, LED_FOOT_Y, LED_Z),
            1.0,
            LedPalette::RED,
            view.led_brightness,
        );

        // Resistor output to the LED anode, then the cathode to the blue rail.
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[far, Vector3::new(LED_X, lead_y, LED_Z), anode],
            0.045,
            JUMPER_SIGNAL,
        );
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                cathode,
                Vector3::new(LED_X + 0.55, lead_y, -0.60),
                Vector3::new(LED_X + 0.55, lead_y, -RAIL_Z),
            ],
            0.045,
            JUMPER_GROUND,
        );
    }

    fn draw_jumpers<D: RaylibDraw3D>(&self, d: &mut D, view: &SwitchView) {
        let lead_y = DECK + RESISTOR_BODY_R;
        // `A` is the pair of legs on the left of the body, `B` the pair on the
        // right; either leg of a pair is the same electrical point.
        let contact_a = Vector3::new(BUTTON_X - LEG_OFFSET, lead_y, BUTTON_Z + LEG_OFFSET);
        let contact_b = Vector3::new(BUTTON_X + LEG_OFFSET, lead_y, BUTTON_Z - LEG_OFFSET);

        // D2 comes from the inboard digital header, since that is where D0-D7
        // actually is on an Uno.
        let d2 = board3d::header_pin(Header::DigitalLow, 2) + BOARD_OFFSET;
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                d2,
                Vector3::new(d2.x, 1.4, d2.z),
                Vector3::new(contact_a.x, 1.4, contact_a.z),
                Vector3::new(contact_a.x, lead_y, contact_a.z),
            ],
            0.06,
            JUMPER_SIGNAL,
        );

        // D9 feeds the indicator resistor.
        let d9 = board3d::header_pin(Header::Digital, 1) + BOARD_OFFSET;
        let resistor_in = Vector3::new(LED_X, lead_y, RESISTOR_Z - RESISTOR_SPAN / 2.0);
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                d9,
                Vector3::new(d9.x, 1.8, d9.z),
                Vector3::new(resistor_in.x, 1.8, resistor_in.z),
                resistor_in,
            ],
            0.06,
            JUMPER_SIGNAL,
        );

        if view.pull_down {
            self.draw_pull_down(d, contact_a, contact_b);
        } else {
            // The simple wiring: the button's far contact goes straight to the
            // blue ground rail, and the AVR's internal pull-up does the rest.
            let rail = Vector3::new(contact_b.x, lead_y, -RAIL_Z);
            wire(
                d,
                &self.cylinder,
                &self.sphere,
                &[contact_b, rail],
                0.045,
                JUMPER_GROUND,
            );
        }
    }

    /// The inverse wiring: 10 kΩ from the signal to ground, and the button
    /// switching the signal up to the 5 V rail instead of down to ground.
    fn draw_pull_down<D: RaylibDraw3D>(&self, d: &mut D, contact_a: Vector3, contact_b: Vector3) {
        let lead_y = DECK + RESISTOR_BODY_R;
        let near = Vector3::new(PULLDOWN_X, lead_y, RESISTOR_Z - RESISTOR_SPAN / 2.0);
        let far = Vector3::new(PULLDOWN_X, lead_y, RESISTOR_Z + RESISTOR_SPAN / 2.0);
        resistor(d, &self.cylinder, near, far, RESISTOR_BODY_R, BANDS_10K);

        // Signal lane: the button's contact A to the pull-down's near lead,
        // routed behind the components so it crosses nothing.
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                contact_a,
                Vector3::new(contact_a.x, lead_y, -2.20),
                Vector3::new(near.x, lead_y, -2.20),
                near,
            ],
            0.045,
            JUMPER_SIGNAL,
        );
        // Pull-down to the blue ground rail.
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                far,
                Vector3::new(far.x, lead_y, -0.60),
                Vector3::new(far.x, lead_y, -RAIL_Z),
            ],
            0.045,
            JUMPER_GROUND,
        );
        // The button's other contact up to the red 5 V rail.
        let rail = Vector3::new(contact_b.x, lead_y, RAIL_Z);
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[contact_b, Vector3::new(contact_b.x, lead_y, 0.80), rail],
            0.045,
            JUMPER_SUPPLY,
        );
        // And 5 V onto that rail from the board's power header, over the top so
        // it reaches the near edge without crossing the board.
        let five_volts = board3d::header_pin(Header::Power, 4) + BOARD_OFFSET;
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                five_volts,
                Vector3::new(five_volts.x, 2.4, five_volts.z),
                Vector3::new(3.0, 2.4, five_volts.z),
                Vector3::new(3.0, 2.4, RAIL_Z),
                Vector3::new(3.0, lead_y, RAIL_Z),
            ],
            0.06,
            JUMPER_SUPPLY,
        );
    }
}

/// Load a generated mesh into a [`Model`].
fn mesh_model(rl: &mut RaylibHandle, thread: &RaylibThread, mesh: Mesh) -> Model {
    rl.load_model_from_mesh(thread, unsafe { mesh.make_weak() })
        .expect("load model from generated mesh")
}
