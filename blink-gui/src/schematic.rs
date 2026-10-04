// SPDX-License-Identifier: MIT

//! Flat presentation of the external LED, driven by solved forward current.
//!
//! Layout is derived from the live window size so the view stays centred
//! whatever the window is resized to.

use crate::circuit3d::led_colour;
use raylib::prelude::*;

/// Body colours.
const OUTLINE: Color = Color::new(54, 118, 130, 255);
const BOARD_FILL: Color = Color::new(23, 52, 58, 255);
const LABEL: Color = Color::new(138, 146, 170, 255);

/// Both external-LED views share the analog operating point, not a GPIO bool.
pub fn draw<D: RaylibDraw>(d: &mut D, width: i32, height: i32, brightness: f32, reversed: bool) {
    let (w, h) = (width as f32, height as f32);
    let cx = (w / 2.0) as i32;
    // Sit the LED slightly above centre to leave room for the caption.
    let cy = (h / 2.0 - 14.0) as i32;

    let board = Rectangle::new(w / 2.0 - 168.0, h / 2.0 - 112.0, 336.0, 208.0);
    d.draw_rectangle_rounded(board, 0.10, 8, BOARD_FILL);
    d.draw_rectangle_rounded_lines(board, 0.10, 8, OUTLINE);

    let brightness = brightness.clamp(0.0, 1.0);
    if brightness > 0.001 {
        d.draw_circle(
            cx,
            cy,
            66.0,
            Color::new(255, 74, 42, (30.0 * brightness) as u8),
        );
        d.draw_circle(
            cx,
            cy,
            52.0,
            Color::new(255, 86, 48, (58.0 * brightness) as u8),
        );
    }
    d.draw_circle(cx, cy, 40.0, led_colour(brightness));

    // ASCII only: the built-in font is CP437. The middle dot happens to be in
    // that set, but arrows are not.
    let label = if reversed {
        "External LED: K <- A"
    } else {
        "External LED: A -> K"
    };
    d.draw_text(label, cx - 90, cy + 74, 16, LABEL);
}
