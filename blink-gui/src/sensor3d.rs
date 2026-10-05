// SPDX-License-Identifier: MIT

//! A to-scale 3D rendition of the ultrasonic ranging rig.
//!
//! **One unit is 10 mm, the same as the board view.** Everything is modelled at
//! its real size: the 68.6 x 53.4 mm Uno, a 45 x 20 mm HC-SR04 module, its
//! 2.54 mm header pitch, 1.5 mm jumper wires, and the target at its true
//! distance — up to 4 m, which is 400 units and therefore 58 board lengths. That
//! is the point of the view: the far end of the range is genuinely far away.
//!
//! The consequence is honest but worth stating: at 2-4 m the board and the wires
//! are only a few pixels across, because they really are that small compared to
//! the distance being measured. The camera re-frames on the target, and the
//! overlay carries the numbers at every range.
//!
//! The Uno is drawn by [`crate::board3d::Board3D::draw_board`], so the same
//! geometry serves both views and the rig is wired to the illustrated header
//! pins rather than to guessed coordinates.

use crate::board3d::{self, Board3D, Header};
use crate::shading;
use crate::wires3d::wire;
use raylib::prelude::*;

/// Model units per metre. One unit is 10 mm, matching the board view.
const UNITS_PER_METRE: f32 = 100.0;

/// Where the Uno sits relative to the module.
///
/// The board's front edge ends at `-7.0 + 2.67 = -4.33` and the module's back
/// edge at `-1.0`, leaving a 33 mm run for the jumpers.
pub const BOARD_OFFSET: Vector3 = Vector3 {
    x: 0.0,
    y: 0.0,
    z: -7.0,
};

/// Module PCB: 45 x 20 mm board, 1.6 mm thick.
const MODULE_W: f32 = 4.5;
const MODULE_D: f32 = 2.0;
const MODULE_H: f32 = 0.16;
/// Z of the module's emitting face.
const FACE_Z: f32 = 1.0;
/// 40 kHz transducer cans: 16 mm across, 11.5 mm long.
const TRANSDUCER_R: f32 = 0.8;
const TRANSDUCER_L: f32 = 1.15;
const TRANSDUCER_X: f32 = 1.15;
/// Height of the transducer axes above the ground plane.
const AXIS_Y: f32 = MODULE_H + TRANSDUCER_R;
/// Four-pin header on the module's back edge, 2.54 mm pitch.
const MODULE_HEADER_Z: f32 = -0.85;
const MODULE_HEADER_PITCH: f32 = 0.254;
const MODULE_HEADER_PINS: usize = 4;
const MODULE_HEADER_BLOCK_Y: f32 = MODULE_H + 0.125;
const MODULE_HEADER_PIN_H: f32 = 0.30;
/// Y of a module header pin's tip, where a jumper lands.
const MODULE_PIN_Y: f32 = MODULE_H + 0.25 + MODULE_HEADER_PIN_H;

/// Jumper wire radius: 1.5 mm across.
const WIRE_R: f32 = 0.075;

/// Target cube: a 30 mm object.
const CUBE_SIZE: f32 = 3.0;
/// Ruler ticks every 100 mm, out to 4 m.
const TICK_STEP_M: f32 = 0.1;
const TICKS: usize = 40;
/// Half-width of the measuring track: a 40 mm lane.
const TRACK_HALF_W: f32 = 2.0;
/// Beam guide radius.
const BEAM_R: f32 = 0.15;

/// HC-SR04 modules typically ship on a blue board, which also keeps them
/// legible against the Uno's teal PCB in the same scene.
const PCB_COLOR: Color = Color::new(24, 62, 122, 255);
const TRANSDUCER: Color = Color::new(176, 182, 196, 255);
const PLUNGER: Color = Color::new(58, 62, 72, 255);
const RETAINER: Color = Color::new(122, 128, 140, 255);
const RAIL: Color = Color::new(58, 64, 82, 255);
const TICK_MINOR: Color = Color::new(76, 84, 104, 255);
const TICK_HALF: Color = Color::new(104, 114, 140, 255);
const TICK_MAJOR: Color = Color::new(138, 148, 178, 255);
/// Detected target.
const TARGET: Color = Color::new(222, 172, 96, 255);
/// Detected and inside the firmware's proximity threshold.
const TARGET_NEAR: Color = Color::new(255, 124, 92, 255);
/// The module reported no echo: drawn dim and undersized, never moved to a
/// guessed position.
const TARGET_LOST: Color = Color::new(74, 78, 92, 255);
const BEAM_IDLE: Color = Color::new(64, 132, 196, 150);
const BEAM_PULSE: Color = Color::new(128, 236, 255, 255);
const VCC_WIRE: Color = Color::new(214, 62, 52, 255);
const GND_WIRE: Color = Color::new(58, 62, 70, 255);
const TRIG_WIRE: Color = Color::new(240, 176, 46, 255);
const ECHO_WIRE: Color = Color::new(72, 196, 216, 255);

/// Which generated unit mesh a part is drawn with.
#[derive(Clone, Copy)]
enum Unit {
    Cube,
    Cylinder,
}

/// One transformed instance of a shared unit mesh.
struct Part {
    unit: Unit,
    center: Vector3,
    axis: Vector3,
    angle: f32,
    /// Full extents for a cube; `(radius, height, radius)` for a cylinder.
    scale: Vector3,
    color: Color,
}

impl Part {
    fn cube(center: Vector3, size: Vector3, color: Color) -> Self {
        Self {
            unit: Unit::Cube,
            center,
            axis: Vector3::new(0.0, 1.0, 0.0),
            angle: 0.0,
            scale: size,
            color,
        }
    }

    /// A cylinder whose axis points along `+Z`, so it faces the target.
    ///
    /// Generated cylinders begin at `y = 0` and extend along `+Y`; rotating a
    /// quarter turn about `X` sends `+Y` to `+Z`, so `center` is the base disc.
    fn facing(center: Vector3, radius: f32, length: f32, color: Color) -> Self {
        Self {
            unit: Unit::Cylinder,
            center,
            axis: Vector3::new(1.0, 0.0, 0.0),
            angle: 90.0,
            scale: Vector3::new(radius, length, radius),
            color,
        }
    }
}

/// What the renderer needs to know about the current measurement.
#[derive(Clone, Copy, Debug)]
pub struct TargetView {
    /// True distance of the reflector, in metres; positions the cube.
    pub distance_m: f64,
    /// The firmware's own measurement, or `None` when it reported no echo.
    pub measured_m: Option<f64>,
    /// True while the sensor is holding ECHO high.
    pub echo_high: bool,
    /// True when the firmware lit its proximity indicator.
    pub proximity: bool,
}

/// Position of a module header pin's tip, in world coordinates.
///
/// Pin order on the module is `VCC`, `TRIG`, `ECHO`, `GND`.
fn module_pin(index: usize) -> Vector3 {
    let span = (MODULE_HEADER_PINS - 1) as f32 * MODULE_HEADER_PITCH;
    Vector3::new(
        -span / 2.0 + index as f32 * MODULE_HEADER_PITCH,
        MODULE_PIN_Y,
        MODULE_HEADER_Z,
    )
}

/// Camera target and distance that frame the whole rig for `distance_m`.
///
/// Because the scene is to scale it spans three orders of magnitude, so one
/// fixed framing cannot show both a 5 cm and a 4 m target. The view re-frames
/// when the target moves, which does reset the wheel zoom.
pub fn frame(distance_m: f64) -> (Vector3, f32) {
    let target_z = FACE_Z + distance_m as f32 * UNITS_PER_METRE;
    let back = BOARD_OFFSET.z - board3d::BOARD_DEPTH / 2.0;
    let center_z = (back + target_z) * 0.5;
    // 40 degrees vertical FOV: half-height at distance D is 0.364 * D.
    let distance = ((target_z - back) * 0.5 / 0.364 * 1.2).clamp(9.0, 780.0);
    (Vector3::new(0.0, 0.6, center_z), distance)
}

/// The ranging rig: module geometry, shader and shared unit meshes.
pub struct Sensor3D {
    /// Owned so the materials' shader references outlive the models.
    _shader: Shader,
    cube: Model,
    cylinder: Model,
    sphere: Model,
    emissive: Model,
    parts: Vec<Part>,
}

impl Sensor3D {
    pub fn new(rl: &mut RaylibHandle, thread: &RaylibThread) -> Self {
        let shader = shading::lit(rl, thread);

        let mut cube = mesh_model(rl, thread, Mesh::gen_mesh_cube(thread, 1.0, 1.0, 1.0));
        let mut cylinder = mesh_model(rl, thread, Mesh::gen_mesh_cylinder(thread, 1.0, 1.0, 32));
        shading::apply(&mut cube, &shader);
        shading::apply(&mut cylinder, &shader);

        // These stay on raylib's unlit default shader so they read as light
        // sources rather than as lit plastic.
        let sphere = mesh_model(rl, thread, Mesh::gen_mesh_sphere(thread, 1.0, 12, 12));
        let emissive = mesh_model(rl, thread, Mesh::gen_mesh_cube(thread, 1.0, 1.0, 1.0));

        Self {
            _shader: shader,
            cube,
            cylinder,
            sphere,
            emissive,
            parts: module_parts(),
        }
    }

    /// Draw the Uno, the module, the wires, the measuring track and the target.
    pub fn draw<D: RaylibDraw3D>(&self, d: &mut D, board: &Board3D, view: &TargetView) {
        board.draw_board(d, BOARD_OFFSET);

        for part in &self.parts {
            let model = match part.unit {
                Unit::Cube => &self.cube,
                Unit::Cylinder => &self.cylinder,
            };
            d.draw_model_ex(
                model,
                part.center,
                part.axis,
                part.angle,
                part.scale,
                part.color,
            );
        }

        self.draw_wires(d);
        self.draw_track(d);

        let face = Vector3::new(0.0, AXIS_Y, FACE_Z);
        let target_z = FACE_Z + view.distance_m as f32 * UNITS_PER_METRE;

        // The beam is a rod rather than a hairline, so it survives at the far
        // end of the range where a single-pixel line would vanish. At 4 m it is
        // still a 400-unit thread, which is the honest picture.
        let beam = if view.echo_high {
            BEAM_PULSE
        } else {
            BEAM_IDLE
        };
        let span = (target_z - FACE_Z).max(0.05);
        d.draw_model_ex(
            &self.cube,
            Vector3::new(0.0, AXIS_Y, FACE_Z + span / 2.0),
            Vector3::new(0.0, 1.0, 0.0),
            0.0,
            Vector3::new(BEAM_R, BEAM_R, span),
            beam,
        );
        // A ping marker at the emitting face, brightest during the echo.
        d.draw_model_ex(
            &self.emissive,
            face,
            Vector3::new(0.0, 1.0, 0.0),
            0.0,
            Vector3::new(0.5, 0.5, 0.5),
            if view.echo_high {
                BEAM_PULSE
            } else {
                Color::new(30, 60, 86, 255)
            },
        );

        // The cube: amber when the firmware detected it, red when it is also
        // inside the proximity threshold, and dim when nothing came back. A lost
        // target is never moved to a guessed place.
        let (color, size) = match view.measured_m {
            None => (TARGET_LOST, CUBE_SIZE * 0.8),
            Some(_) if view.proximity => (TARGET_NEAR, CUBE_SIZE),
            Some(_) => (TARGET, CUBE_SIZE),
        };
        d.draw_model_ex(
            &self.cube,
            Vector3::new(0.0, size / 2.0, target_z),
            Vector3::new(0.0, 1.0, 0.0),
            0.0,
            Vector3::new(size, size, size),
            color,
        );

        // A small indicator on the module mirroring the firmware's D13.
        let indicator = if view.proximity {
            Color::new(120, 240, 150, 255)
        } else {
            Color::new(40, 66, 52, 255)
        };
        d.draw_model_ex(
            &self.emissive,
            Vector3::new(-MODULE_W / 2.0 + 0.4, MODULE_H + 0.12, 0.5),
            Vector3::new(0.0, 1.0, 0.0),
            0.0,
            Vector3::new(0.35, 0.18, 0.35),
            indicator,
        );
    }

    /// Four jumpers from the illustrated Arduino headers to the module.
    ///
    /// `TRIG`, `ECHO` and `GND` come off the digital header, which faces the
    /// module. `VCC` has to come from the power header on the far edge, so it is
    /// routed around the board's left side rather than across it.
    fn draw_wires<D: RaylibDraw3D>(&self, d: &mut D) {
        let pin = |header: Header, index: usize| board3d::header_pin(header, index) + BOARD_OFFSET;
        // Digital header order is D8..D13, GND, AREF, SDA, SCL.
        let trig = pin(Header::Digital, 1);
        let echo = pin(Header::Digital, 2);
        let gnd = pin(Header::Digital, 6);
        // Power header order is NC, IOREF, RESET, 3V3, 5V, GND, GND, VIN.
        let vcc = pin(Header::Power, 4);

        for (from, to, arch, colour) in [
            (trig, module_pin(1), 1.5, TRIG_WIRE),
            (echo, module_pin(2), 1.7, ECHO_WIRE),
            (gnd, module_pin(3), 1.9, GND_WIRE),
        ] {
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
                WIRE_R,
                colour,
            );
        }

        // VCC: up, out past the board's left edge, forward, then in to the pin.
        let lane = -board3d::BOARD_WIDTH / 2.0 - 1.3;
        let arch = 2.3;
        let vcc_module = module_pin(0);
        wire(
            d,
            &self.cylinder,
            &self.sphere,
            &[
                vcc,
                Vector3::new(vcc.x, arch, vcc.z),
                Vector3::new(lane, arch, vcc.z),
                Vector3::new(lane, arch, vcc_module.z),
                Vector3::new(vcc_module.x, arch, vcc_module.z),
                vcc_module,
            ],
            WIRE_R,
            VCC_WIRE,
        );
    }

    /// Two rails and a 100 mm ruler: a measuring track reads as a scale far
    /// better than a ground grid does across a 400-unit scene.
    fn draw_track<D: RaylibDraw3D>(&self, d: &mut D) {
        let end = FACE_Z + TICKS as f32 * TICK_STEP_M * UNITS_PER_METRE;
        let length = end - FACE_Z;
        for side in [-1.0_f32, 1.0] {
            d.draw_model_ex(
                &self.cube,
                Vector3::new(side * TRACK_HALF_W, 0.06, FACE_Z + length / 2.0),
                Vector3::new(0.0, 1.0, 0.0),
                0.0,
                Vector3::new(0.12, 0.12, length),
                RAIL,
            );
        }
        for index in 1..=TICKS {
            let z = FACE_Z + index as f32 * TICK_STEP_M * UNITS_PER_METRE;
            // Every tenth tick is a metre, every fifth a half metre.
            let (height, colour) = if index % 10 == 0 {
                (1.6, TICK_MAJOR)
            } else if index % 5 == 0 {
                (0.8, TICK_HALF)
            } else {
                (0.4, TICK_MINOR)
            };
            d.draw_model_ex(
                &self.cube,
                Vector3::new(0.0, height / 2.0, z),
                Vector3::new(0.0, 1.0, 0.0),
                0.0,
                Vector3::new(TRACK_HALF_W * 2.0, height, 0.06),
                colour,
            );
        }
    }
}

/// Load a generated mesh into a [`Model`].
fn mesh_model(rl: &mut RaylibHandle, thread: &RaylibThread, mesh: Mesh) -> Model {
    rl.load_model_from_mesh(thread, unsafe { mesh.make_weak() })
        .expect("load model from generated mesh")
}

/// The HC-SR04 module at true size: PCB, two transducer cans and a 4-pin header.
fn module_parts() -> Vec<Part> {
    let mut parts = vec![
        // PCB, its front face flush with the emitting plane.
        Part::cube(
            Vector3::new(0.0, MODULE_H / 2.0, FACE_Z - MODULE_D / 2.0),
            Vector3::new(MODULE_W, MODULE_H, MODULE_D),
            PCB_COLOR,
        ),
        // 40 kHz transmit and receive transducers. Each is a metal can, a
        // retaining ring and the recessed diaphragm behind it.
        Part::facing(
            Vector3::new(-TRANSDUCER_X, AXIS_Y, FACE_Z - TRANSDUCER_L),
            TRANSDUCER_R,
            TRANSDUCER_L,
            TRANSDUCER,
        ),
        Part::facing(
            Vector3::new(TRANSDUCER_X, AXIS_Y, FACE_Z - TRANSDUCER_L),
            TRANSDUCER_R,
            TRANSDUCER_L,
            TRANSDUCER,
        ),
        Part::facing(
            Vector3::new(-TRANSDUCER_X, AXIS_Y, FACE_Z - TRANSDUCER_L + 0.06),
            TRANSDUCER_R * 0.72,
            0.1,
            PLUNGER,
        ),
        Part::facing(
            Vector3::new(TRANSDUCER_X, AXIS_Y, FACE_Z - TRANSDUCER_L + 0.06),
            TRANSDUCER_R * 0.72,
            0.1,
            PLUNGER,
        ),
        // Header body on the back edge.
        Part::cube(
            Vector3::new(0.0, MODULE_HEADER_BLOCK_Y, MODULE_HEADER_Z),
            Vector3::new(
                (MODULE_HEADER_PINS - 1) as f32 * MODULE_HEADER_PITCH + 0.3,
                0.25,
                0.25,
            ),
            RETAINER,
        ),
    ];

    // Four gold pins standing on the header body: VCC, TRIG, ECHO, GND.
    for index in 0..MODULE_HEADER_PINS {
        let pin = module_pin(index);
        parts.push(Part::cube(
            Vector3::new(pin.x, MODULE_H + 0.25 + MODULE_HEADER_PIN_H / 2.0, pin.z),
            Vector3::new(0.10, MODULE_HEADER_PIN_H, 0.10),
            Color::new(206, 170, 90, 255),
        ));
    }

    // A couple of decoupling capacitors, because a bare board reads as a prop.
    for x in [0.0, 0.55] {
        parts.push(Part::cube(
            Vector3::new(x, MODULE_H + 0.18, -0.35),
            Vector3::new(0.22, 0.36, 0.18),
            Color::new(60, 64, 74, 255),
        ));
    }
    parts
}
