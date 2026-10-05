// SPDX-License-Identifier: MIT

//! Uno pin identity, port mapping and multi-driver resolution.
//!
//! External components talk in Arduino pin numbers (`D9`, `A0`) rather than raw
//! AVR ports. The Uno mapping is fixed: `D0`-`D7` are `PORTD` bits 0-7,
//! `D8`-`D13` are `PORTB` bits 0-5, and `A0`-`A5` are `PORTC` bits 0-5.

use std::error::Error;
use std::fmt;

/// An Uno digital pin, `D0`..=`D13`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DigitalPin(u8);

impl DigitalPin {
    /// Number of digital pins on an Uno.
    pub const COUNT: u8 = 14;

    /// Construct a valid digital pin, or `None` when out of range.
    pub fn new(index: u8) -> Option<Self> {
        (index < Self::COUNT).then_some(Self(index))
    }

    /// Zero-based index, matching the number silkscreened on the board.
    pub fn index(self) -> u8 {
        self.0
    }
}

/// An Uno analog input pin, `A0`..=`A5`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AnalogPin(u8);

impl AnalogPin {
    /// Number of analog input pins on an Uno.
    pub const COUNT: u8 = 6;

    /// Construct a valid analog pin, or `None` when out of range.
    pub fn new(index: u8) -> Option<Self> {
        (index < Self::COUNT).then_some(Self(index))
    }

    /// Zero-based index, matching the `A`-prefixed label on the board.
    pub fn index(self) -> u8 {
        self.0
    }
}

/// A physical AVR port a Uno pin belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Port {
    /// `PORTB`, carrying digital pins 8-13.
    B,
    /// `PORTC`, carrying analog inputs `A0`-`A5`.
    C,
    /// `PORTD`, carrying digital pins 0-7.
    D,
}

impl Port {
    /// Data-space address of this port's `PINx` input register.
    ///
    /// This is the pad readback: it reflects the driven value for an output pin
    /// and the externally applied value for an input pin, which is what an
    /// attached component observes.
    pub fn pin_register(self) -> usize {
        match self {
            Self::B => 0x23,
            Self::C => 0x26,
            Self::D => 0x29,
        }
    }
}

/// Any Uno pin an external component can attach to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Pin {
    /// A digital pin, `D0`..=`D13`.
    Digital(DigitalPin),
    /// An analog input pin, `A0`..=`A5`.
    Analog(AnalogPin),
}

impl Pin {
    /// Digital pin `index`, panicking when it is not a valid Uno digital pin.
    pub fn digital(index: u8) -> Self {
        Self::try_digital(index).expect("digital pin out of range: an Uno has D0..=D13")
    }

    /// Analog pin `index`, panicking when it is not a valid Uno analog pin.
    pub fn analog(index: u8) -> Self {
        Self::try_analog(index).expect("analog pin out of range: an Uno has A0..=A5")
    }

    /// Digital pin `index`, or `None` when out of range.
    pub fn try_digital(index: u8) -> Option<Self> {
        DigitalPin::new(index).map(Self::Digital)
    }

    /// Analog pin `index`, or `None` when out of range.
    pub fn try_analog(index: u8) -> Option<Self> {
        AnalogPin::new(index).map(Self::Analog)
    }

    /// The AVR port and bit this pin maps to.
    pub fn port_bit(self) -> (Port, u8) {
        match self {
            Self::Digital(pin) if pin.index() < 8 => (Port::D, pin.index()),
            Self::Digital(pin) => (Port::B, pin.index() - 8),
            Self::Analog(pin) => (Port::C, pin.index()),
        }
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Digital(pin) => write!(f, "D{}", pin.index()),
            Self::Analog(pin) => write!(f, "A{}", pin.index()),
        }
    }
}

/// What a single driver asserts on a shared pin.
///
/// This is a digital abstraction, not a voltage. `PullUp` is a weak high, so a
/// strong `Low` beats it; two strong drivers in opposite directions are a
/// conflict rather than an invented midpoint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drive {
    /// Not driving: the pin floats as far as this driver is concerned.
    HighZ,
    /// Weak high through a pull-up.
    PullUp,
    /// Strongly driven high.
    High,
    /// Strongly driven low.
    Low,
}

impl Drive {
    /// True for a low-impedance output.
    pub fn is_strong(self) -> bool {
        matches!(self, Self::High | Self::Low)
    }

    /// The level a digital input would read, or `None` for a floating net.
    pub fn level(self) -> Option<bool> {
        match self {
            Self::High | Self::PullUp => Some(true),
            Self::Low => Some(false),
            Self::HighZ => None,
        }
    }

    /// Resolve two drivers on the same net.
    ///
    /// `High` against `Low` is a short circuit and returns [`PinConflict`]
    /// rather than silently picking a winner.
    pub fn combine(self, other: Self) -> Result<Self, PinConflict> {
        match (self, other) {
            (Self::HighZ, other) | (other, Self::HighZ) => Ok(other),
            (Self::High, Self::Low) | (Self::Low, Self::High) => Err(PinConflict),
            (Self::High, _) | (_, Self::High) => Ok(Self::High),
            (Self::Low, _) | (_, Self::Low) => Ok(Self::Low),
            (Self::PullUp, Self::PullUp) => Ok(Self::PullUp),
        }
    }
}

/// Two drivers short a pin by driving it to opposite levels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PinConflict;

impl fmt::Display for PinConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "two drivers short the same pin")
    }
}

impl Error for PinConflict {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uno_pins_map_to_the_expected_ports_and_bits() {
        assert_eq!(Pin::digital(0).port_bit(), (Port::D, 0));
        assert_eq!(Pin::digital(7).port_bit(), (Port::D, 7));
        assert_eq!(Pin::digital(8).port_bit(), (Port::B, 0));
        assert_eq!(Pin::digital(13).port_bit(), (Port::B, 5));
        assert_eq!(Pin::analog(0).port_bit(), (Port::C, 0));
        assert_eq!(Pin::analog(5).port_bit(), (Port::C, 5));
    }

    #[test]
    fn pin_register_addresses_match_the_device_profile() {
        assert_eq!(Port::B.pin_register(), 0x23);
        assert_eq!(Port::C.pin_register(), 0x26);
        assert_eq!(Port::D.pin_register(), 0x29);
    }

    #[test]
    fn out_of_range_pins_are_rejected() {
        assert!(Pin::try_digital(14).is_none());
        assert!(Pin::try_analog(6).is_none());
        assert_eq!(Pin::try_digital(13), Some(Pin::digital(13)));
    }

    #[test]
    fn floating_and_weak_drivers_never_win_over_a_strong_one() {
        assert_eq!(Drive::HighZ.combine(Drive::HighZ), Ok(Drive::HighZ));
        assert_eq!(Drive::HighZ.combine(Drive::High), Ok(Drive::High));
        assert_eq!(Drive::PullUp.combine(Drive::HighZ), Ok(Drive::PullUp));
        assert_eq!(Drive::PullUp.combine(Drive::Low), Ok(Drive::Low));
        assert_eq!(Drive::PullUp.combine(Drive::High), Ok(Drive::High));
        // Two drivers agreeing is not a conflict.
        assert_eq!(Drive::High.combine(Drive::High), Ok(Drive::High));
        assert_eq!(Drive::Low.combine(Drive::Low), Ok(Drive::Low));
    }

    #[test]
    fn opposing_strong_drivers_are_reported_rather_than_merged() {
        assert_eq!(Drive::High.combine(Drive::Low), Err(PinConflict));
        assert_eq!(Drive::Low.combine(Drive::High), Err(PinConflict));
    }
}
