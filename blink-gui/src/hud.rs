// SPDX-License-Identifier: MIT

//! The overlay shared by both views: identity, live stats and the control hint.
//!
//! Everything here is drawn in screen space over whichever view is active, and
//! sits on a translucent panel so it stays legible against the 3D grid.

use crate::sensor_sim::SensorSim;
use crate::{analog::SERIES_OHMS, sim::Sim};
use raylib::prelude::*;

const TITLE: Color = Color::RAYWHITE;
const SUBTLE: Color = Color::new(122, 129, 152, 255);
const VALUE: Color = Color::new(178, 185, 208, 255);
const PAUSED: Color = Color::new(255, 202, 92, 255);
const PANEL: Color = Color::new(12, 13, 19, 190);
const HINT: Color = Color::new(96, 102, 124, 255);

/// Draw the title, the stat panel and the control hint.
pub fn draw(
    d: &mut RaylibDrawHandle<'_>,
    sim: &Sim,
    paused: bool,
    speed: f64,
    view_label: &str,
    width: i32,
    height: i32,
) {
    let h = height as f32;

    d.draw_text("ATmega328P blink", 20, 16, 22, TITLE);
    d.draw_text(
        "firmware: arduino-cli / arduino:avr:uno",
        20,
        44,
        14,
        SUBTLE,
    );

    let terminals = if sim.analog.reversed() {
        "signal -> K / A -> GND"
    } else {
        "signal -> A / K -> GND"
    };
    d.draw_text(
        &format!(
            "DC: D13 {:?} -> {SERIES_OHMS:.0} ohm -> LED | {terminals}",
            sim.analog.state()
        ),
        20,
        72,
        13,
        VALUE,
    );
    match &sim.analog.reading {
        Ok(r) => {
            d.draw_text(
                &format!(
                    "Vpin {:.3} V    Vled(A-K) {:+.3} V    Iled {:+.3e} A",
                    r.pin_voltage, r.led.voltage, r.led.current
                ),
                20,
                92,
                13,
                VALUE,
            );
            d.draw_text(
                &format!(
                    "P_R {:.2} mW   P_LED {:.2} mW   Idriver {:+.3e} A",
                    r.resistor.power() * 1000.0,
                    r.led.power() * 1000.0,
                    r.driver_current
                ),
                20,
                112,
                13,
                SUBTLE,
            );
            d.draw_text(
                &format!(
                    "Psrc {:.2} mW   Pdriver {:.2} mW   KCL residual {:.1e} A",
                    r.source_power * 1000.0,
                    r.driver_power * 1000.0,
                    r.max_kcl_residual
                ),
                20,
                132,
                13,
                SUBTLE,
            );
            if r.digital_input.is_none() {
                d.draw_text(
                    "Pad voltage is between valid digital input thresholds",
                    20,
                    152,
                    13,
                    PAUSED,
                );
            }
        }
        Err(error) => d.draw_text(&format!("ANALOG ERROR: {error}"), 20, 92, 13, Color::RED),
    }

    // Live stats, bottom-left, on a panel.
    let panel = Rectangle::new(18.0, h - 128.0, 300.0, 108.0);
    d.draw_rectangle_rounded(panel, 0.16, 8, PANEL);
    let x = 32;
    let mut y = (h - 112.0) as i32;
    for (label, value) in [
        ("sim time", format!("{:.0} ms", sim.millis())),
        ("cycles", format!("{}", sim.cycles())),
        ("instructions", format!("{}", sim.instructions())),
        ("led toggles", format!("{}", sim.toggles())),
    ] {
        d.draw_text(label, x, y, 15, SUBTLE);
        d.draw_text(&value, x + 226 - text_width(d, &value, 15), y, 15, VALUE);
        y += 23;
    }

    // Clock rate, bottom-right.
    let rate = format!("{speed:.2}x");
    let rate_color = if paused { PAUSED } else { VALUE };
    d.draw_text(
        &rate,
        width - 24 - text_width(d, &rate, 20),
        height - 46,
        20,
        rate_color,
    );
    if paused {
        d.draw_text(
            "PAUSED",
            width - 24 - text_width(d, "PAUSED", 14),
            height - 70,
            14,
            PAUSED,
        );
    }

    // ASCII only: raylib's built-in font is CP437, so arrows and other symbols
    // outside that range render as '?'.
    let hint = format!(
        "{view_label} - space pause  r reset  p reverse LED  < > speed  v view  drag/scroll orbit"
    );
    d.draw_text(
        &hint,
        (width - text_width(d, &hint, 13)) / 2,
        height - 26,
        13,
        HINT,
    );
}

/// `RaylibDrawHandle` derefs to `RaylibHandle`, which is where the default-font
/// `measure_text` lives; this reaches it through the deref.
fn text_width(d: &RaylibDrawHandle<'_>, text: &str, size: i32) -> i32 {
    d.measure_text(text, size)
}

/// Draw the ultrasonic demo's overlay.
///
/// The two distances are kept side by side deliberately: the true one positions
/// the cube, the firmware's one is what a real board would report, and the gap
/// between them is the `pulseIn`/`micros()` quantisation rather than a bug to be
/// hidden.
pub fn draw_sensor(
    d: &mut RaylibDrawHandle<'_>,
    sim: &SensorSim,
    proximity: bool,
    paused: bool,
    speed: f64,
    width: i32,
    height: i32,
) {
    let h = height as f32;
    let telemetry = sim.telemetry();

    // The 3D scene has bright geometry behind the text, so the readout sits on
    // its own panel rather than competing with rail and grid lines.
    d.draw_rectangle_rounded(Rectangle::new(12.0, 8.0, 660.0, 172.0), 0.04, 6, PANEL);

    d.draw_text("HC-SR04 ultrasonic ranging", 20, 16, 22, TITLE);
    d.draw_text(
        "firmware: arduino-cli / arduino:avr:uno - firmware measures, I2C publishes",
        20,
        44,
        14,
        SUBTLE,
    );

    d.draw_text(
        &format!("reflector at {:.3} m (D9 TRIG, D10 ECHO)", sim.distance_m()),
        20,
        72,
        13,
        VALUE,
    );

    let status = match telemetry.status {
        0 => "echo",
        1 => "no echo",
        2 => "out of range",
        _ => "unknown",
    };
    let measured = match telemetry.distance_m() {
        Some(distance) => {
            let error_mm = (distance - sim.distance_m()) * 1000.0;
            format!("firmware measured {distance:.3} m ({error_mm:+.0} mm)")
        }
        None => format!("firmware measured nothing ({status})"),
    };
    d.draw_text(&measured, 20, 92, 13, VALUE);

    d.draw_text(
        &format!(
            "echo {:?} us   ECHO line {}",
            telemetry.echo_micros,
            if sim.echo_high() { "HIGH" } else { "low" }
        ),
        20,
        112,
        13,
        SUBTLE,
    );
    d.draw_text(
        &format!(
            "sensor model last measured {:?} m",
            sim.model_measurement_m()
        ),
        20,
        132,
        13,
        SUBTLE,
    );

    let near = if proximity { PAUSED } else { HINT };
    d.draw_text(
        if proximity {
            "D13 lit: target inside 200 mm"
        } else {
            "D13 dark: target beyond 200 mm"
        },
        20,
        152,
        13,
        near,
    );

    // Live stats. Unlike the blink overlay these sit on the right: the ranging
    // scene is to scale, so the Uno and its wiring occupy the left of the frame
    // and must not be covered by a panel.
    let panel = Rectangle::new(width as f32 - 318.0, h - 128.0, 300.0, 108.0);
    d.draw_rectangle_rounded(panel, 0.16, 8, PANEL);
    let x = width - 304;
    let mut y = (h - 112.0) as i32;
    for (label, value) in [
        ("sim time", format!("{:.0} ms", sim.millis())),
        ("cycles", format!("{}", sim.cycles())),
        ("instructions", format!("{}", sim.instructions())),
        (
            "measurements",
            format!(
                "{} ({} missed)",
                sim.measurements(),
                sim.missed_measurements()
            ),
        ),
    ] {
        d.draw_text(label, x, y, 15, SUBTLE);
        d.draw_text(&value, x + 226 - text_width(d, &value, 15), y, 15, VALUE);
        y += 23;
    }

    // Clock rate, top-right, clear of the 3D subject.
    let rate = format!("{speed:.2}x");
    let rate_color = if paused { PAUSED } else { VALUE };
    d.draw_text(
        &rate,
        width - 24 - text_width(d, &rate, 20),
        20,
        20,
        rate_color,
    );
    if paused {
        d.draw_text(
            "PAUSED",
            width - 24 - text_width(d, "PAUSED", 14),
            48,
            14,
            PAUSED,
        );
    }

    // ASCII only: raylib's built-in font is CP437, so arrows and other symbols
    // outside that range render as '?'.
    let hint = "ultrasonic ranging - space pause  r reboot  - = target distance  < > speed  v view  drag/scroll orbit";
    d.draw_text(
        hint,
        (width - text_width(d, hint, 13)) / 2,
        height - 26,
        13,
        HINT,
    );
    // The track is a real ruler: 100 mm ticks, taller at 500 mm and 1 m.
    d.draw_text(
        "track ticks every 0.1 m; taller at 0.5 m and 1 m (scene is to scale)",
        20,
        190,
        13,
        HINT,
    );
}
