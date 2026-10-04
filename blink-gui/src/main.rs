// SPDX-License-Identifier: MIT

//! A raylib front-end for the native AVR simulator.
//!
//! The simulator runs a real `arduino-cli`-compiled blink sketch for an
//! ATmega328P; raylib only observes it. Two views share one simulator state:
//!
//! * **Schematic** — the flat presentation of pin 13.
//! * **Board** — a procedural 3D rendition of the Uno, with the pin 13 LED and
//!   its halo driven by the simulated `PORTB5`.
//!
//! Controls: `space` pause, `r` reset, `p` reverse the external LED,
//! `←`/`→` simulated clock rate, `v` or
//! `tab` switch view, left-drag to orbit and the wheel to zoom in the 3D view.
//!
//! Use `--release`. The simulator is roughly 10x slower in a debug build and the
//! blink will visibly lag.

mod analog;
mod board3d;
mod camera;
mod circuit3d;
mod hud;
mod schematic;
mod shading;
mod sim;

use board3d::Board3D;
use camera::OrbitCamera;
use raylib::prelude::*;
use sim::{Sim, CYCLES_PER_FRAME_1X};

/// Simulated clock-rate multipliers reachable with Left/Right.
const SPEEDS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
/// Index of 1.0x, which is real time at the target frame rate.
const DEFAULT_SPEED: usize = 2;
const WINDOW_W: i32 = 900;
const WINDOW_H: i32 = 600;

/// Which presentation is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Schematic,
    Board,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Schematic => "schematic view",
            Self::Board => "3D board view",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Schematic => Self::Board,
            Self::Board => Self::Schematic,
        }
    }
}

fn main() {
    let (mut rl, thread) = raylib::init()
        .size(WINDOW_W, WINDOW_H)
        .title("Arduino blink - native Rust AVR simulator")
        .build();
    rl.set_target_fps(60);

    let mut sim = Sim::boot();
    if std::env::var("BLINK_REVERSED").as_deref() == Ok("1") {
        sim.toggle_led_polarity();
    }
    let board = Board3D::new(&mut rl, &thread);
    // Frame the PCB and the external circuit extending beyond its front edge.
    let mut camera = OrbitCamera::new(Vector3::new(0.0, 0.55, 1.0), 16.5);

    // `BLINK_VIEW=3d` selects the 3D view at start-up, for the smoke test.
    let mut view = match std::env::var("BLINK_VIEW").as_deref() {
        Ok("3d") | Ok("board") => View::Board,
        _ => View::Schematic,
    };
    let mut paused = false;
    let mut speed = DEFAULT_SPEED;

    // Smoke-test hook: `BLINK_FRAMES=300` exits after N frames instead of
    // waiting for a click, which is how this demo is verified without a human
    // watching the window.
    let frame_limit: u64 = std::env::var("BLINK_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX);
    let mut frame: u64 = 0;

    while !rl.window_should_close() && frame < frame_limit {
        if rl.is_key_pressed(KeyboardKey::KEY_SPACE) {
            paused = !paused;
        }
        if rl.is_key_pressed(KeyboardKey::KEY_R) {
            sim = Sim::boot_with_polarity(sim.analog.reversed());
        }
        if rl.is_key_pressed(KeyboardKey::KEY_P) {
            sim.toggle_led_polarity();
        }
        if rl.is_key_pressed(KeyboardKey::KEY_V) || rl.is_key_pressed(KeyboardKey::KEY_TAB) {
            view = view.toggled();
        }
        if rl.is_key_pressed(KeyboardKey::KEY_RIGHT) || rl.is_key_pressed(KeyboardKey::KEY_UP) {
            speed = (speed + 1).min(SPEEDS.len() - 1);
        }
        if rl.is_key_pressed(KeyboardKey::KEY_LEFT) || rl.is_key_pressed(KeyboardKey::KEY_DOWN) {
            speed = speed.saturating_sub(1);
        }

        if !paused {
            sim.advance((CYCLES_PER_FRAME_1X * SPEEDS[speed]) as u64);
        }
        if view == View::Board {
            camera.update(&rl);
        }

        let (width, height) = (rl.get_screen_width(), rl.get_screen_height());
        let mut d = rl.begin_drawing(&thread);
        d.clear_background(Color::new(18, 19, 26, 255));

        match view {
            View::Schematic => schematic::draw(
                &mut d,
                width,
                height,
                sim.analog.brightness(),
                sim.analog.reversed(),
            ),
            View::Board => {
                // `nobuild` hides raylib-rs's camera module, so build the raw
                // struct ourselves; see `camera.rs` for the details.
                let raw = camera.raw();
                d.draw_mode3D(raw, |mut guard| {
                    board.draw(
                        &mut guard,
                        sim.led_on,
                        sim.analog.brightness(),
                        sim.analog.reversed(),
                    )
                });
            }
        }
        hud::draw(
            &mut d,
            &sim,
            paused,
            SPEEDS[speed],
            view.label(),
            width,
            height,
        );

        frame += 1;
    }

    println!(
        "executed {} instructions ({} cycles), {} LED toggles, {:.0} ms simulated",
        sim.instructions(),
        sim.cycles(),
        sim.toggles(),
        sim.millis()
    );
    match &sim.analog.reading {
        Ok(reading) => println!(
            "DC {:?}, reversed={}: Vpin={:.6} V, Vled(A-K)={:+.6} V, Iled={:+.6e} A, brightness={:.6}, {} solves",
            sim.analog.state(), sim.analog.reversed(), reading.pin_voltage,
            reading.led.voltage, reading.led.current, reading.brightness, sim.analog.solves,
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
