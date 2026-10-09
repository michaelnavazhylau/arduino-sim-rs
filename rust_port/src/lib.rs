// SPDX-License-Identifier: MIT

//! Dependency-free native Rust AVR8 simulator core and scenario runner.
//!
//! The Rust scenario representation deliberately separates test behavior from the
//! simulator's evolving ownership model. See [`Backend`] and the project README.
//!
//! The converted AVR8js contract these types express — the generated scenarios,
//! the converter that produces them and the pinned upstream revision — lives in
//! the separate `avr8js-parity` repository. This crate keeps an empty
//! `[dependencies]` table by construction, so the offline engine gate stays
//! hermetic and the crate stays publishable.
pub mod board;
pub mod runtime;
pub mod scenario;
pub mod sim;
pub use board::Board;
pub use runtime::{Backend, Runtime, Value};
pub use scenario::Case;

/// The native Rust AVR simulator that backs every converted scenario.
pub fn native_backend() -> std::rc::Rc<dyn Backend> {
    std::rc::Rc::new(sim::Simulator::new())
}
