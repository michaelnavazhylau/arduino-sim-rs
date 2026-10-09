// SPDX-License-Identifier: MIT

//! The overlay shared by both views: identity, live stats and the control hint.
//!
//! Everything here is drawn in screen space over whichever view is active, and
//! sits on a translucent panel so it stays legible against the 3D grid.

use crate::components3d::LedPalette;
use crate::leds_sim::LedsSim;
use crate::sensor_sim::SensorSim;
use crate::switch_sim::{SwitchSim, Wiring};
use crate::{analog::SERIES_OHMS, sim::Sim};
use breadboard::CouplingOutcome;
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
                    "Psrc {:.2} mW   Pdriver {:.2} mW   solver ngspice-rs .op",
                    r.source_power * 1000.0,
                    r.driver_power * 1000.0
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

/// Draw the three-LED demo's overlay.
///
/// Each row is tinted with its LED's own palette so the text lines up with the
/// columns on screen, and every number is a solved quantity: the forward voltage
/// and current come from the deck's operating point, not from the pin state.
pub fn draw_leds(
    d: &mut RaylibDrawHandle<'_>,
    sim: &LedsSim,
    paused: bool,
    speed: f64,
    width: i32,
    height: i32,
) {
    let h = height as f32;
    let readings = sim.readings();

    d.draw_rectangle_rounded(Rectangle::new(12.0, 8.0, 660.0, 208.0), 0.04, 6, PANEL);
    d.draw_text("Three indicator LEDs", 20, 16, 22, TITLE);
    d.draw_text(
        "firmware: arduino-cli / arduino:avr:uno - one 330 ohm resistor per channel",
        20,
        44,
        14,
        SUBTLE,
    );

    let palettes = [LedPalette::RED, LedPalette::GREEN, LedPalette::BLUE];
    let mut y = 74;
    for (index, (channel, reading)) in sim.channels().iter().zip(readings).enumerate() {
        // Tinted with the LED's own lit colour, dimmed while it is dark.
        let tint = palettes[index].body(if reading.is_lit() { 1.0 } else { 0.4 });
        d.draw_text(channel.name, 20, y, 15, tint);
        d.draw_text(
            &format!(
                "{:<3} D{:<2}   Vf {:+.4} V   If {:+.4} mA   Vpin {:+.3} V   {:.0}%",
                channel.resistor,
                channel.pin,
                reading.voltage,
                reading.current * 1e3,
                reading.pin_voltage,
                reading.brightness * 100.0,
            ),
            96,
            y,
            15,
            VALUE,
        );
        y += 22;
    }

    d.draw_text(
        "equal resistors, unequal currents: forward voltage spans ~1 V red to blue",
        20,
        y + 8,
        13,
        SUBTLE,
    );
    match sim.outcome() {
        CouplingOutcome::Solved => {
            d.draw_text(
                &format!("ngspice-rs .op, {} solves", sim.solves()),
                20,
                y + 28,
                13,
                SUBTLE,
            );
        }
        CouplingOutcome::Indeterminate(error) => {
            d.draw_text(&format!("ANALOG: {error:?}"), 20, y + 28, 13, PAUSED)
        }
    }

    // Live stats, on the right, so the breadboard on the left stays clear.
    let panel = Rectangle::new(width as f32 - 318.0, h - 128.0, 300.0, 108.0);
    d.draw_rectangle_rounded(panel, 0.16, 8, PANEL);
    let x = width - 304;
    let mut y = (h - 112.0) as i32;
    // Which channels the firmware currently has selected, so the sequence is
    // visible as a state and not only as light.
    let selected: Vec<&str> = sim
        .channels()
        .iter()
        .zip(readings)
        .filter(|(_, reading)| reading.is_lit())
        .map(|(channel, _)| channel.name)
        .collect();
    let selected = if selected.is_empty() {
        "none".to_string()
    } else {
        selected.join("+")
    };
    for (label, value) in [
        ("sim time", format!("{:.0} ms", sim.millis())),
        ("cycles", format!("{}", sim.cycles())),
        ("instructions", format!("{}", sim.instructions())),
        ("selected", selected),
    ] {
        d.draw_text(label, x, y, 15, SUBTLE);
        d.draw_text(&value, x + 226 - text_width(d, &value, 15), y, 15, VALUE);
        y += 23;
    }

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
    let hint = "three indicator LEDs - space pause  r reboot  < > speed  v view  drag/scroll orbit";
    d.draw_text(
        hint,
        (width - text_width(d, hint, 13)) / 2,
        height - 26,
        13,
        HINT,
    );
}

/// Draw a switch-input demo's overlay.
///
/// The two wirings share this function because the interesting content is the
/// *comparison*: the same button and the same behaviour, read at opposite
/// levels. Every electrical number is solved rather than computed in the
/// overlay, so the pull-up load and the switch's insulation resistance both
/// show up in the voltages.
pub fn draw_switch(
    d: &mut RaylibDrawHandle<'_>,
    sim: &SwitchSim,
    paused: bool,
    speed: f64,
    width: i32,
    height: i32,
) {
    let h = height as f32;
    let wiring = sim.wiring();
    let pressed = sim.pressed();

    d.draw_rectangle_rounded(Rectangle::new(12.0, 8.0, 700.0, 210.0), 0.04, 6, PANEL);
    d.draw_text("Push button -> digital input -> LED", 20, 16, 22, TITLE);
    d.draw_text(wiring.label(), 20, 44, 14, SUBTLE);
    d.draw_text(wiring.logic(), 20, 64, 13, SUBTLE);

    // What the wiring asks of the MCU, and what it should read while held.
    d.draw_text(
        &format!(
            "pin mode {}   expected while held: {}",
            if wiring.uses_internal_pullup() {
                "INPUT_PULLUP"
            } else {
                "INPUT"
            },
            if wiring.pressed_level() {
                "HIGH"
            } else {
                "LOW"
            },
        ),
        20,
        84,
        13,
        SUBTLE,
    );

    d.draw_text(
        if pressed {
            "BUTTON HELD"
        } else {
            "BUTTON RELEASED"
        },
        20,
        108,
        16,
        if pressed { PAUSED } else { VALUE },
    );

    let level = match sim.digital_level() {
        Some(true) => "HIGH",
        Some(false) => "LOW",
        None => "indeterminate",
    };
    d.draw_text(
        &format!("pin D2   {:.4} V   reads {level}", sim.button_voltage()),
        20,
        132,
        15,
        VALUE,
    );
    // The LED text takes its colour from the LED's own palette, so "lit" and
    // "dark" read the same way they do in the three-LED view.
    let lit = sim.led_lit();
    d.draw_text(
        &format!(
            "LED D9   {:.3} mA   Vf {:+.4} V   brightness {:.0}%",
            sim.led_current() * 1e3,
            sim.led_voltage(),
            sim.led_brightness() * 100.0,
        ),
        20,
        154,
        15,
        LedPalette::RED.body(if lit { 1.0 } else { 0.35 }),
    );

    // The comparison is the reason there are two of these views.
    let other = match wiring {
        Wiring::PullUp => "the inverse wiring would read HIGH released, LOW held",
        Wiring::PullDown => "the simple wiring would read LOW released, HIGH held",
    };
    d.draw_text(other, 20, 180, 13, HINT);

    if matches!(sim.outcome(), CouplingOutcome::Indeterminate(_)) {
        d.draw_text("ANALOG: no unique operating point", 20, 196, 13, PAUSED);
    }

    // Live stats, on the right, so the breadboard on the left stays clear.
    let panel = Rectangle::new(width as f32 - 318.0, h - 128.0, 300.0, 108.0);
    d.draw_rectangle_rounded(panel, 0.16, 8, PANEL);
    let x = width - 304;
    let mut y = (h - 112.0) as i32;
    for (label, value) in [
        ("sim time", format!("{:.0} ms", sim.millis())),
        ("cycles", format!("{}", sim.cycles())),
        ("instructions", format!("{}", sim.instructions())),
        ("dc solves", format!("{}", sim.solves())),
    ] {
        d.draw_text(label, x, y, 15, SUBTLE);
        d.draw_text(&value, x + 226 - text_width(d, &value, 15), y, 15, VALUE);
        y += 23;
    }

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

    // ASCII only: raylib's built-in font is CP437, so '?' replaces anything
    // outside that range.
    let hint = "switch input - hold B to press  space pause  r reboot  < > speed  v view  drag/scroll orbit";
    d.draw_text(
        hint,
        (width - text_width(d, hint, 13)) / 2,
        height - 26,
        13,
        HINT,
    );
}
