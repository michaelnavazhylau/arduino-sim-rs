// SPDX-License-Identifier: MIT

//! A procedural 3D rendition of an Arduino Uno.
//!
//! The board is assembled from three unit meshes raylib generates — a cube, a
//! cylinder and a sphere — instanced with `draw_model_ex` transforms. There is
//! no STL, OBJ or GLB anywhere: raylib has **no STL loader at all** (its
//! `LoadModel` handles only .obj, .iqm, .gltf/.glb, .vox and .m3d), and more
//! importantly a procedural board needs no binary asset, no conversion step and
//! no model licence.
//!
//! Model scale is 1.0 = 10 mm, so a 68.6 x 53.4 mm Uno PCB is 6.86 x 5.34 and
//! header pitch is 2.54 mm = 0.254. Component placement is approximate but
//! recognisable. The onboard LED remains a digital indicator; the external LED
//! uses the solved forward current and independently reversible polarity.

use crate::{circuit3d, shading};
use raylib::prelude::*;

/// PCB thickness: 1.6 mm.
const PCB_H: f32 = 0.16;
/// PCB width: 68.6 mm.
const PCB_W: f32 = 6.86;
/// PCB depth: 53.4 mm.
const PCB_D: f32 = 5.34;
/// Lift the board off the y = 0 grid plane to avoid z-fighting with it.
const BOARD_LIFT: f32 = 0.06;
/// Top surface of the PCB; every component is placed relative to this.
const DECK: f32 = BOARD_LIFT + PCB_H;
/// Header pin pitch: 2.54 mm.
const PITCH: f32 = 0.254;

/// Board dimensions in model units, so another view can place and frame it.
pub const BOARD_WIDTH: f32 = PCB_W;
pub const BOARD_DEPTH: f32 = PCB_D;

/// Pin headers, as `(centre x, centre z, pin count)`.
///
/// A real Uno has **fourteen** digital pins in two blocks on one edge. Only the
/// `D8`..`D13` block was modelled at first, which is why every earlier demo had
/// to use a pin from that half; `D0`..`D7` sits inboard on the same edge, as it
/// does on the board.
const DIGITAL_LOW_HEADER: (f32, f32, usize) = (-1.00, 2.30, 8);
const DIGITAL_HEADER: (f32, f32, usize) = (1.18, 2.30, 10);
const ANALOG_HEADER: (f32, f32, usize) = (1.62, -2.30, 6);
const POWER_HEADER: (f32, f32, usize) = (-1.38, -2.30, 8);
const ISP_HEADER: (f32, f32, usize) = (2.95, 0.72, 6);

/// A pin header on the board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Header {
    /// `D0`..`D7`.
    DigitalLow,
    /// `D8`..`D13`, `GND`, `AREF`, `SDA`, `SCL`.
    Digital,
    /// `A0`..`A5`.
    Analog,
    /// Power: `NC`, `IOREF`, `RESET`, `3V3`, `5V`, `GND`, `GND`, `VIN`.
    Power,
    /// The 2x3 ICSP header.
    Isp,
}

impl Header {
    /// `(centre x, centre z, pin count)` of the header body.
    fn layout(self) -> (f32, f32, usize) {
        match self {
            Self::DigitalLow => DIGITAL_LOW_HEADER,
            Self::Digital => DIGITAL_HEADER,
            Self::Analog => ANALOG_HEADER,
            Self::Power => POWER_HEADER,
            Self::Isp => ISP_HEADER,
        }
    }
}

/// Position of a header pin's tip, in board-local coordinates.
///
/// Exposed so another view can wire to the illustrated header rather than
/// guessing where the pins are; add the board's own offset to get world space.
pub fn header_pin(header: Header, index: usize) -> Vector3 {
    let (cx, cz, pins) = header.layout();
    assert!(
        index < pins,
        "header pin {index} is out of range for {pins} pins"
    );
    let span = (pins - 1) as f32 * PITCH;
    Vector3::new(cx - span / 2.0 + index as f32 * PITCH, DECK + 0.37, cz)
}

/// Body colours.
const PCB_COLOR: Color = Color::new(17, 82, 98, 255);
const PLASTIC: Color = Color::new(30, 30, 35, 255);
const SILVER: Color = Color::new(168, 174, 184, 255);
const GOLD: Color = Color::new(212, 176, 92, 255);

/// Which of the shared unit meshes a part is drawn with.
#[derive(Clone, Copy)]
enum Unit {
    Cube,
    Cylinder,
}

/// One transformed instance of a shared unit mesh.
struct Part {
    unit: Unit,
    center: Vector3,
    /// Full extents for a cube; `(radius, height, radius)` for a cylinder.
    scale: Vector3,
    color: Color,
}

impl Part {
    fn cube(center: Vector3, size: Vector3, color: Color) -> Self {
        Self {
            unit: Unit::Cube,
            center,
            scale: size,
            color,
        }
    }

    fn cylinder(center: Vector3, radius: f32, height: f32, color: Color) -> Self {
        Self {
            unit: Unit::Cylinder,
            center,
            scale: Vector3::new(radius, height, radius),
            color,
        }
    }
}

/// The assembled board, its shader and the two LEDs drawn emissively.
pub struct Board3D {
    /// Owned so the materials' shader references outlive the models.
    _shader: Shader,
    cube: Model,
    cylinder: Model,
    /// Unlit unit cube, for LEDs.
    emissive: Model,
    /// Unlit unit sphere, for the LED halo.
    halo: Model,
    parts: Vec<Part>,
    led13_center: Vector3,
    led13_size: Vector3,
    on_led_center: Vector3,
    on_led_size: Vector3,
}

impl Board3D {
    pub fn new(rl: &mut RaylibHandle, thread: &RaylibThread) -> Self {
        let shader = shading::lit(rl, thread);

        let mut cube = mesh_model(rl, thread, Mesh::gen_mesh_cube(thread, 1.0, 1.0, 1.0));
        let mut cylinder = mesh_model(rl, thread, Mesh::gen_mesh_cylinder(thread, 1.0, 1.0, 24));
        shading::apply(&mut cube, &shader);
        shading::apply(&mut cylinder, &shader);

        // These two stay on raylib's default (unlit) shader so they read as
        // light sources rather than as lit plastic.
        let emissive = mesh_model(rl, thread, Mesh::gen_mesh_cube(thread, 1.0, 1.0, 1.0));
        let halo = mesh_model(rl, thread, Mesh::gen_mesh_sphere(thread, 1.0, 12, 12));

        Self {
            _shader: shader,
            cube,
            cylinder,
            emissive,
            halo,
            parts: board_parts(),
            // The "L" LED sits inboard of the digital header, near pin 13.
            led13_center: Vector3::new(2.28, DECK + 0.08, 1.36),
            led13_size: Vector3::new(0.30, 0.16, 0.26),
            // The always-on power LED, bottom-left near the power header.
            on_led_center: Vector3::new(-2.34, DECK + 0.06, -2.08),
            on_led_size: Vector3::new(0.20, 0.12, 0.18),
        }
    }

    /// Draw only the static board geometry, translated by `offset`.
    ///
    /// The ranging view reuses this to put the same Uno in its own scene, so the
    /// board is never modelled twice and both views stay at one scale.
    pub fn draw_board<D: RaylibDraw3D>(&self, d: &mut D, offset: Vector3) {
        let spin = Vector3::new(0.0, 1.0, 0.0);
        for part in &self.parts {
            let model = match part.unit {
                Unit::Cube => &self.cube,
                Unit::Cylinder => &self.cylinder,
            };
            d.draw_model_ex(
                model,
                part.center + offset,
                spin,
                0.0,
                part.scale,
                part.color,
            );
        }
    }

    /// Onboard indicator follows GPIO; the external LED follows solved current.
    pub fn draw<D: RaylibDraw3D>(
        &self,
        d: &mut D,
        led_on: bool,
        external_brightness: f32,
        reversed: bool,
    ) {
        let spin = Vector3::new(0.0, 1.0, 0.0);

        // Reference grid at y = 0; the board floats just above it.
        d.draw_grid(14, 1.0);

        self.draw_board(d, Vector3::zero());

        // D13 -> 220 ohm resistor -> external LED -> GND.
        circuit3d::draw(d, &self.cylinder, &self.halo, external_brightness, reversed);

        // Power LED: steady green.
        d.draw_model_ex(
            &self.emissive,
            self.on_led_center,
            spin,
            0.0,
            self.on_led_size,
            Color::new(96, 236, 140, 255),
        );

        // Pin 13 LED plus a soft halo when lit, driven by the simulator.
        let (body, glow) = if led_on {
            (Color::new(255, 176, 138, 255), Color::new(255, 92, 48, 54))
        } else {
            (Color::new(92, 36, 30, 255), Color::new(0, 0, 0, 0))
        };
        d.draw_model_ex(
            &self.emissive,
            self.led13_center,
            spin,
            0.0,
            self.led13_size,
            body,
        );
        if led_on {
            d.draw_model_ex(
                &self.halo,
                self.led13_center,
                spin,
                0.0,
                Vector3::new(0.62, 0.5, 0.62),
                glow,
            );
        }
    }
}

/// Load a generated mesh into a [`Model`].
///
/// `make_weak` transfers ownership of the raw mesh to the model, which unloads
/// it on drop — the safe path here because the `Mesh` is a temporary.
fn mesh_model(rl: &mut RaylibHandle, thread: &RaylibThread, mesh: Mesh) -> Model {
    rl.load_model_from_mesh(thread, unsafe { mesh.make_weak() })
        .expect("load model from generated mesh")
}

/// Everything on the board that does not change state.
fn board_parts() -> Vec<Part> {
    let mut parts = vec![
        // PCB.
        Part::cube(
            Vector3::new(0.0, BOARD_LIFT + PCB_H / 2.0, 0.0),
            Vector3::new(PCB_W, PCB_H, PCB_D),
            PCB_COLOR,
        ),
        // USB-B receptacle, rear-left edge.
        Part::cube(
            Vector3::new(-2.62, DECK + 0.72, -1.30),
            Vector3::new(1.62, 1.44, 1.56),
            SILVER,
        ),
        // Barrel power jack, front-left edge.
        Part::cube(
            Vector3::new(-2.62, DECK + 0.50, 1.52),
            Vector3::new(0.94, 1.00, 1.42),
            PLASTIC,
        ),
        // ATmega328P in DIP-28: 34.7 mm long.
        Part::cube(
            Vector3::new(0.52, DECK + 0.28, 0.30),
            Vector3::new(3.47, 0.56, 0.86),
            Color::new(26, 26, 30, 255),
        ),
        // 16 MHz crystal.
        Part::cube(
            Vector3::new(-1.32, DECK + 0.20, 0.30),
            Vector3::new(1.15, 0.40, 0.50),
            SILVER,
        ),
    ];

    // Electrolytic capacitors.
    for z in [1.02, -0.58] {
        parts.push(Part::cylinder(
            // Generated cylinders start at y=0, so position their base on the PCB.
            Vector3::new(-1.98, DECK, z),
            0.40,
            0.92,
            Color::new(38, 58, 108, 255),
        ));
    }

    // Voltage regulator.
    parts.push(Part::cube(
        Vector3::new(-0.98, DECK + 0.45, 2.02),
        Vector3::new(0.95, 0.90, 0.42),
        PLASTIC,
    ));

    // Reset button.
    parts.push(Part::cube(
        Vector3::new(-0.18, DECK + 0.18, 2.20),
        Vector3::new(0.48, 0.36, 0.48),
        Color::new(62, 64, 72, 255),
    ));

    // Current-limiting resistor for the pin 13 LED.
    parts.push(Part::cube(
        Vector3::new(1.86, DECK + 0.09, 1.36),
        Vector3::new(0.36, 0.18, 0.22),
        Color::new(186, 172, 122, 255),
    ));

    // Headers: D0-D7 (8), D8-D13 (10), analog (6), power (8), ICSP (6).
    for header in [
        Header::DigitalLow,
        Header::Digital,
        Header::Analog,
        Header::Power,
        Header::Isp,
    ] {
        let (cx, cz, pins) = header.layout();
        push_header(&mut parts, cx, cz, pins);
    }

    parts
}

/// A black plastic header body with gold pins standing on top of it.
fn push_header(parts: &mut Vec<Part>, cx: f32, cz: f32, pins: usize) {
    let span = (pins - 1) as f32 * PITCH;
    parts.push(Part::cube(
        Vector3::new(cx, DECK + 0.13, cz),
        Vector3::new(span + 0.28, 0.26, 0.58),
        PLASTIC,
    ));
    for index in 0..pins {
        let x = cx - span / 2.0 + index as f32 * PITCH;
        parts.push(Part::cube(
            Vector3::new(x, DECK + 0.29, cz),
            Vector3::new(0.10, 0.16, 0.10),
            GOLD,
        ));
    }
}
