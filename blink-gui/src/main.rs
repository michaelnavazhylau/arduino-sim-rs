// SPDX-License-Identifier: MIT

//! A raylib front-end for the native AVR simulator.
//!
//! Two demos share one window, one camera and one clock budget:
//!
//! * **Blink** — the simulator runs a real `arduino-cli`-compiled blink sketch
//!   for an ATmega328P; raylib only observes it. Two presentations: the flat
//!   schematic of pin 13, and a procedural 3D rendition of the Uno whose
//!   external LED is lit by the *solved* forward current, not `led_on`.
//! * **Ultrasonic ranging** — the simulator runs a real HC-SR04 sketch that
//!   pulses TRIG on D9, times ECHO on D10 with `pulseIn`, and publishes the
//!   measurement over I2C because the simulated board has no console. The 3D
//!   view draws the module and a target cube; the number on screen is what the
//!   **firmware measured**.
//!
//! Controls: `space` pause, `r` reset, `p` reverse the external LED,
//! `←`/`→` simulated clock rate, `v` or `tab` cycle views, `-`/`=` move the
//! ultrasonic target, left-drag to orbit and the wheel to zoom.
//!
//! Use `--release`. The simulator is roughly 10x slower in a debug build and
//! both demos will visibly lag.

mod analog;
mod board3d;
mod breadboard3d;
mod camera;
mod circuit3d;
mod components3d;
mod hud;
mod leds3d;
mod leds_sim;
mod schematic;
mod sensor3d;
mod sensor_sim;
mod shading;
mod sim;
mod switch3d;
mod switch_sim;
mod wires3d;

use board3d::Board3D;
use camera::OrbitCamera;
use leds3d::{LedView, Leds3D};
use leds_sim::LedsSim;
use raylib::prelude::*;
use sensor3d::{Sensor3D, TargetView};
use sensor_sim::SensorSim;
use sim::{Sim, CYCLES_PER_FRAME_1X};
use switch3d::{Switch3D, SwitchView};
use switch_sim::{SwitchSim, Wiring as SwitchWiring};

/// Simulated clock-rate multipliers reachable with Left/Right.
const SPEEDS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
/// Index of 1.0x, which is real time at the target frame rate.
const DEFAULT_SPEED: usize = 2;
const WINDOW_W: i32 = 900;
const WINDOW_H: i32 = 600;

/// Simulated microseconds per rendered frame at 1.0x: 60 fps.
const MICROS_PER_FRAME_1X: f64 = 1_000_000.0 / 60.0;

/// How far one key press moves the ultrasonic target, in metres.
const TARGET_STEP_M: f64 = 0.1;

/// Which presentation is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Schematic,
    Board,
    Sensor,
    Leds,
    SwitchPullUp,
    SwitchPullDown,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Schematic => "schematic view",
            Self::Board => "3D board view",
            Self::Sensor => "ultrasonic ranging",
            Self::Leds => "three indicator LEDs",
            Self::SwitchPullUp => "switch input: simple",
            Self::SwitchPullDown => "switch input: inverse",
        }
    }

    /// Cycle to the next presentation.
    fn toggled(self) -> Self {
        match self {
            Self::Schematic => Self::Board,
            Self::Board => Self::Sensor,
            Self::Sensor => Self::Leds,
            Self::Leds => Self::SwitchPullUp,
            Self::SwitchPullUp => Self::SwitchPullDown,
            Self::SwitchPullDown => Self::Schematic,
        }
    }

    /// The ultrasonic presentation, which has its own sim and controls.
    fn is_sensor(self) -> bool {
        self == Self::Sensor
    }

    /// The three-LED presentation.
    fn is_leds(self) -> bool {
        self == Self::Leds
    }

    /// Either switch-input presentation.
    fn is_switch(self) -> bool {
        matches!(self, Self::SwitchPullUp | Self::SwitchPullDown)
    }

    /// Which wiring the switch presentations show.
    fn switch_wiring(self) -> Option<SwitchWiring> {
        match self {
            Self::SwitchPullUp => Some(SwitchWiring::PullUp),
            Self::SwitchPullDown => Some(SwitchWiring::PullDown),
            _ => None,
        }
    }
}

/// A camera framed for one presentation.
///
/// The scenes differ by an order of magnitude in extent — the ranging rig is
/// hundreds of units long while the board is about seven — so they cannot share
/// a framing.
fn camera_for(view: View) -> OrbitCamera {
    match view {
        View::Schematic | View::Board => OrbitCamera::new(Vector3::new(0.0, 0.55, 1.0), 16.5),
        View::Sensor => OrbitCamera::new(Vector3::new(0.0, 1.2, 11.0), 30.0),
        View::Leds => {
            let (target, distance) = leds3d::frame();
            let mut camera = OrbitCamera::new(target, distance);
            // Steeper than the board view, so the breadboard layout reads.
            camera.set_orientation(34.0, 34.0);
            camera
        }
        View::SwitchPullUp | View::SwitchPullDown => {
            let (target, distance) = switch3d::frame();
            let mut camera = OrbitCamera::new(target, distance);
            camera.set_orientation(34.0, 34.0);
            camera
        }
    }
}

fn main() {
    let (mut rl, thread) = raylib::init()
        .size(WINDOW_W, WINDOW_H)
        .title("Arduino AVR simulator - blink, ranging and indicator LEDs")
        .build();
    rl.set_target_fps(60);

    let mut sim = Sim::boot();
    if std::env::var("BLINK_REVERSED").as_deref() == Ok("1") {
        sim.toggle_led_polarity();
    }
    let board = Board3D::new(&mut rl, &thread);
    let sensor3d = Sensor3D::new(&mut rl, &thread);
    let leds3d = Leds3D::new(&mut rl, &thread);
    let mut leds = LedsSim::boot();
    let switch3d = Switch3D::new(&mut rl, &thread);
    let mut switch_up = SwitchSim::boot(SwitchWiring::PullUp);
    let mut switch_down = SwitchSim::boot(SwitchWiring::PullDown);
    // `SWITCH_PRESSED=1` starts with the button held, so the pressed state can be
    // captured without a human holding a key down.
    let start_pressed = std::env::var("SWITCH_PRESSED").as_deref() == Ok("1");
    if start_pressed {
        switch_up.set_pressed(true);
        switch_down.set_pressed(true);
    }

    // `SENSOR_DISTANCE_M` sets the reflector's starting position.
    let mut sensor = match std::env::var("SENSOR_DISTANCE_M")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
    {
        Some(distance) => SensorSim::boot_at(distance),
        None => SensorSim::boot(),
    };

    // `BLINK_VIEW=3d`, `=sensor` or `=leds` selects the starting presentation,
    // for the smoke test; anything else starts on the flat schematic.
    let mut view = match std::env::var("BLINK_VIEW").as_deref() {
        Ok("3d") | Ok("board") => View::Board,
        Ok("sensor") | Ok("ultrasonic") => View::Sensor,
        Ok("leds") | Ok("indicator") => View::Leds,
        Ok("switch") | Ok("pullup") => View::SwitchPullUp,
        Ok("pulldown") => View::SwitchPullDown,
        _ => View::Schematic,
    };
    let mut camera = camera_for(view);
    // Distance the ranging camera is currently framed for; `None` forces a
    // re-frame on the first frame the sensor view is on screen. It is not a
    // NaN sentinel: every comparison against NaN is false, so the re-frame
    // would silently never run.
    let mut framed_distance: Option<f64> = None;
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
            if view.is_sensor() {
                // Reboot the firmware, keeping the reflector where it is.
                sensor = SensorSim::boot_at(sensor.distance_m());
            } else if view.is_leds() {
                leds = LedsSim::boot();
            } else if view.is_switch() {
                match view.switch_wiring() {
                    Some(SwitchWiring::PullUp) => switch_up = SwitchSim::boot(SwitchWiring::PullUp),
                    _ => switch_down = SwitchSim::boot(SwitchWiring::PullDown),
                }
            } else {
                sim = Sim::boot_with_polarity(sim.analog.reversed());
            }
        }
        if rl.is_key_pressed(KeyboardKey::KEY_P)
            && !view.is_sensor()
            && !view.is_leds()
            && !view.is_switch()
        {
            sim.toggle_led_polarity();
        }
        if rl.is_key_pressed(KeyboardKey::KEY_V) || rl.is_key_pressed(KeyboardKey::KEY_TAB) {
            view = view.toggled();
            framed_distance = None;
            camera = camera_for(view);
        }
        if rl.is_key_pressed(KeyboardKey::KEY_RIGHT) || rl.is_key_pressed(KeyboardKey::KEY_UP) {
            speed = (speed + 1).min(SPEEDS.len() - 1);
        }
        if rl.is_key_pressed(KeyboardKey::KEY_LEFT) || rl.is_key_pressed(KeyboardKey::KEY_DOWN) {
            speed = speed.saturating_sub(1);
        }
        if view.is_sensor() {
            // Both `-`/`=` and the bracket pair move the cube; the keyboard
            // layouts differ enough that one binding is not enough.
            let nearer = rl.is_key_pressed(KeyboardKey::KEY_MINUS)
                || rl.is_key_pressed(KeyboardKey::KEY_LEFT_BRACKET);
            let further = rl.is_key_pressed(KeyboardKey::KEY_EQUAL)
                || rl.is_key_pressed(KeyboardKey::KEY_RIGHT_BRACKET);
            if nearer {
                sensor.set_distance_m(sensor.distance_m() - TARGET_STEP_M);
            }
            if further {
                sensor.set_distance_m(sensor.distance_m() + TARGET_STEP_M);
            }
            // Re-frame whenever the target moves, so both a 5 cm and a 4 m
            // reflector stay on screen. This does reset the wheel zoom.
            if framed_distance != Some(sensor.distance_m()) {
                let entering = framed_distance.is_none();
                framed_distance = Some(sensor.distance_m());
                let (target, distance) = sensor3d::frame(sensor.distance_m());
                camera.look_at(target, distance);
                // Only set the aspect on entry, so an orbit survives moving the
                // target. The yaw places the camera behind the module, which
                // puts the sensor in the foreground and the target receding.
                if entering {
                    camera.set_orientation(200.0, 20.0);
                }
            }
        }

        if !paused {
            if view.is_sensor() {
                sensor.advance_micros(MICROS_PER_FRAME_1X * SPEEDS[speed]);
            } else if view.is_leds() {
                leds.advance_micros(MICROS_PER_FRAME_1X * SPEEDS[speed]);
            } else if view.is_switch() {
                let micros = MICROS_PER_FRAME_1X * SPEEDS[speed];
                match view.switch_wiring() {
                    Some(SwitchWiring::PullUp) => switch_up.advance_micros(micros),
                    _ => switch_down.advance_micros(micros),
                }
            } else {
                sim.advance((CYCLES_PER_FRAME_1X * SPEEDS[speed]) as u64);
            }
        }
        // A momentary button wants a held key, not a toggle: the switch is only
        // closed while the key is down, exactly like a finger on the part.
        if view.is_switch() {
            let held = rl.is_key_down(KeyboardKey::KEY_B) || start_pressed;
            match view.switch_wiring() {
                Some(SwitchWiring::PullUp) => switch_up.set_pressed(held),
                _ => switch_down.set_pressed(held),
            }
        }

        if view != View::Schematic {
            camera.update(&rl);
        }

        let (width, height) = (rl.get_screen_width(), rl.get_screen_height());
        // Read the firmware's proximity indicator once per frame: it drives both
        // the target's tint and the overlay, and it costs a pin-state query.
        let proximity = sensor.proximity_led();
        {
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
                View::Sensor => {
                    let target = TargetView {
                        distance_m: sensor.distance_m(),
                        measured_m: sensor.telemetry().distance_m(),
                        echo_high: sensor.echo_high(),
                        proximity,
                    };
                    let raw = camera.raw();
                    d.draw_mode3D(raw, |mut guard| sensor3d.draw(&mut guard, &board, &target));
                }
                View::Leds => {
                    let readings = leds.readings();
                    let view_state = LedView {
                        brightness: [
                            readings[0].brightness,
                            readings[1].brightness,
                            readings[2].brightness,
                        ],
                    };
                    let raw = camera.raw();
                    d.draw_mode3D(raw, |mut guard| {
                        leds3d.draw(&mut guard, &board, &view_state)
                    });
                }
                View::SwitchPullUp | View::SwitchPullDown => {
                    let sim = if view.is_switch() && view == View::SwitchPullDown {
                        &switch_down
                    } else {
                        &switch_up
                    };
                    let view_state = SwitchView {
                        pressed: sim.pressed(),
                        led_brightness: sim.led_brightness(),
                        pull_down: sim.wiring() == SwitchWiring::PullDown,
                    };
                    let raw = camera.raw();
                    d.draw_mode3D(raw, |mut guard| {
                        switch3d.draw(&mut guard, &board, &view_state)
                    });
                }
            }

            if view.is_sensor() {
                hud::draw_sensor(
                    &mut d,
                    &sensor,
                    proximity,
                    paused,
                    SPEEDS[speed],
                    width,
                    height,
                );
            } else if view.is_leds() {
                hud::draw_leds(&mut d, &leds, paused, SPEEDS[speed], width, height);
            } else if view.is_switch() {
                let sim = if view == View::SwitchPullDown {
                    &switch_down
                } else {
                    &switch_up
                };
                hud::draw_switch(&mut d, sim, paused, SPEEDS[speed], width, height);
            } else {
                hud::draw(
                    &mut d,
                    &sim,
                    paused,
                    SPEEDS[speed],
                    view.label(),
                    width,
                    height,
                );
            }
        }

        frame += 1;

        // `BLINK_SCREENSHOT=shot.png` captures a rendered frame, which is how
        // these views are reviewed, documented and regenerated without a human
        // at the window.
        //
        // Three raylib behaviours shape this hook. It must run *after* the draw
        // handle closes, because raylib batches 2D draws and only flushes them
        // in `EndDrawing` — a capture taken inside the handle loses the whole
        // overlay. The capture reflects the frame presented immediately
        // *before* the current one, because the read happens across a buffer
        // swap, so a blinking subject can land in either phase and regenerating
        // an image means choosing the frame count with that in mind. And the
        // value is a *base name* in the working directory: raylib drops any
        // directory part, so `docs/shot.png` writes `./shot.png`.
        if frame == frame_limit {
            if let Ok(path) = std::env::var("BLINK_SCREENSHOT") {
                rl.take_screenshot(&thread, &path);
                println!("screenshot written: {path}");
            }
        }
    }

    if view.is_sensor() {
        print_sensor_summary(&sensor);
    } else if view.is_leds() {
        print_leds_summary(&leds);
    } else if view == View::SwitchPullDown {
        print_switch_summary(&switch_down);
    } else if view == View::SwitchPullUp {
        print_switch_summary(&switch_up);
    } else {
        print_blink_summary(&sim);
    }
}

fn print_switch_summary(sim: &SwitchSim) {
    println!(
        "switch ({:?}): {} instructions ({} cycles), {:.0} ms simulated",
        sim.wiring(),
        sim.instructions(),
        sim.cycles(),
        sim.millis()
    );
    println!(
        "  button {}  pin {:.4} V  reads {:?}  LED {:+.3} mA  brightness {:.3}",
        if sim.pressed() { "held" } else { "released" },
        sim.button_voltage(),
        sim.digital_level(),
        sim.led_current() * 1e3,
        sim.led_brightness(),
    );
}

fn print_leds_summary(leds: &LedsSim) {
    println!(
        "leds: {} instructions ({} cycles), {:.0} ms simulated, {:.1} firmware cycles",
        leds.instructions(),
        leds.cycles(),
        leds.millis(),
        leds.millis() / leds_sim::CYCLE_MS
    );
    for (channel, reading) in leds.channels().iter().zip(leds.readings()) {
        println!(
            "  {:<5} Vf {:+.4} V  If {:+.4} mA  Vpin {:+.4} V  brightness {:.3}",
            channel.name,
            reading.voltage,
            reading.current * 1e3,
            reading.pin_voltage,
            reading.brightness,
        );
    }
}

fn print_blink_summary(sim: &Sim) {
    println!(
        "blink: executed {} instructions ({} cycles), {} LED toggles, {:.0} ms simulated",
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

fn print_sensor_summary(sensor: &SensorSim) {
    let telemetry = sensor.telemetry();
    println!(
        "ultrasonic: {} instructions ({} cycles), {:.0} ms simulated",
        sensor.instructions(),
        sensor.cycles(),
        sensor.millis()
    );
    println!(
        "reflector {:.3} m | firmware measured {:?} | echo {:?} us, status {}, sequence {}, {} measurements ({} missed)",
        sensor.distance_m(),
        telemetry.distance_m(),
        telemetry.echo_micros,
        telemetry.status,
        telemetry.sequence,
        sensor.measurements(),
        sensor.missed_measurements(),
    );
    println!(
        "sensor model last measured {:?} m",
        sensor.model_measurement_m()
    );
}
