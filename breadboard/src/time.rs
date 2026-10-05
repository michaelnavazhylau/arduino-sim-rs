// SPDX-License-Identifier: MIT

//! Simulated-time conversions on the AVR cycle clock.
//!
//! External-component timing is expressed in **AVR cycles** at the Uno's 16 MHz
//! clock, never in wall-clock time, frame counts or host scheduling order. A
//! paused or slow host therefore cannot change a sensor's behaviour, which is
//! the property the sim2real roadmap requires of AVR/analog time coupling.

use avr_port_tests::board::UNO_CLOCK_HZ;

/// Convert an AVR cycle count to microseconds at [`UNO_CLOCK_HZ`].
pub fn cycles_to_micros(cycles: u64) -> f64 {
    cycles as f64 * 1e6 / UNO_CLOCK_HZ
}

/// Convert microseconds to the nearest whole AVR cycle.
///
/// Non-finite or non-positive input converts to zero rather than wrapping.
pub fn micros_to_cycles(micros: f64) -> u64 {
    if !micros.is_finite() || micros <= 0.0 {
        return 0;
    }
    (micros * UNO_CLOCK_HZ / 1e6).round() as u64
}

/// Speed of sound in dry air at `celsius`, in metres per second.
///
/// A linear approximation around room temperature: enough to make a distance
/// measurement temperature-dependent in the right direction. It is not a full
/// psychrometric model, and humidity is not represented.
pub fn speed_of_sound_m_per_s(celsius: f64) -> f64 {
    331.3 + 0.606 * celsius
}
