//! Behavioral tests for a native AVR simulator. No JavaScript runtime is used.
//!
//! The Rust scenario representation deliberately separates test behavior from the
//! simulator's evolving ownership model. See `Backend` and the project README.
pub mod runtime;
pub mod scenario;
pub mod sim;
pub mod suites;
pub use runtime::{Backend, Runtime, Value};
pub use scenario::Case;

/// The native Rust AVR simulator that backs every converted scenario.
pub fn native_backend() -> std::rc::Rc<dyn Backend> {
    std::rc::Rc::new(sim::Simulator::new())
}

pub fn run_native(case: Case) {
    Runtime::new(native_backend()).run(&case);
}
