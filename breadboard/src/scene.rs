// SPDX-License-Identifier: MIT

//! Physical world model: what a distance sensor can actually measure.
//!
//! The scene is deliberately geometry-light. A reflector sits at a distance
//! along the sensor axis, and time of flight — not rendering — decides what the
//! sensor reports. A renderer may visualise a scene, but it never defines one.

use std::error::Error;
use std::fmt;

/// A flat reflector crossing the sensor axis at `distance_m`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflector {
    distance_m: f64,
    reflectivity: f64,
}

impl Reflector {
    /// A reflector at `distance_m` with `reflectivity` in `[0, 1]`.
    ///
    /// Reflectivity scales returned energy, not time of flight. It is how a
    /// weakly reflecting or oblique target can fall below a sensor's detection
    /// threshold without changing where it is.
    pub fn new(distance_m: f64, reflectivity: f64) -> Result<Self, SceneError> {
        if !distance_m.is_finite() || distance_m <= 0.0 {
            return Err(SceneError::InvalidDistance);
        }
        if !reflectivity.is_finite() || !(0.0..=1.0).contains(&reflectivity) {
            return Err(SceneError::InvalidReflectivity);
        }
        Ok(Self {
            distance_m,
            reflectivity,
        })
    }

    /// Distance from the sensor, in metres.
    pub fn distance_m(self) -> f64 {
        self.distance_m
    }

    /// Fraction of incident energy returned, in `[0, 1]`.
    pub fn reflectivity(self) -> f64 {
        self.reflectivity
    }
}

/// Why a scene could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneError {
    /// A distance must be finite and strictly positive.
    InvalidDistance,
    /// Reflectivity must be finite and within `[0, 1]`.
    InvalidReflectivity,
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDistance => write!(f, "scene: distance must be finite and positive"),
            Self::InvalidReflectivity => {
                write!(f, "scene: reflectivity must be finite and within [0, 1]")
            }
        }
    }
}

impl Error for SceneError {}

/// The reflectors a sensor's beam can reach.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    reflectors: Vec<Reflector>,
}

impl Scene {
    /// A scene with nothing in range, so every sensor reads "no echo".
    pub fn empty() -> Self {
        Self::default()
    }

    /// A single fully reflective wall at `distance_m`.
    pub fn wall(distance_m: f64) -> Result<Self, SceneError> {
        Ok(Self {
            reflectors: vec![Reflector::new(distance_m, 1.0)?],
        })
    }

    /// Add a reflector.
    pub fn with_reflector(mut self, reflector: Reflector) -> Self {
        self.reflectors.push(reflector);
        self
    }

    /// Reflectors in insertion order.
    pub fn reflectors(&self) -> &[Reflector] {
        &self.reflectors
    }

    /// Nearest reflector whose reflectivity reaches `min_reflectivity`.
    ///
    /// Returns `None` when nothing in the beam returns enough energy, which is
    /// how an out-of-range or absorbing target reads.
    pub fn nearest_echo(&self, min_reflectivity: f64) -> Option<Reflector> {
        self.reflectors
            .iter()
            .copied()
            .filter(|reflector| reflector.reflectivity() >= min_reflectivity)
            .min_by(|left, right| left.distance_m().total_cmp(&right.distance_m()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_echo_returns_the_closest_qualifying_reflector() {
        let scene = Scene::empty()
            .with_reflector(Reflector::new(1.5, 1.0).unwrap())
            .with_reflector(Reflector::new(0.4, 0.9).unwrap())
            .with_reflector(Reflector::new(0.1, 0.01).unwrap());
        let nearest = scene.nearest_echo(0.5).unwrap();
        assert!((nearest.distance_m() - 0.4).abs() < 1e-12);
    }

    #[test]
    fn a_dim_reflector_returns_no_echo_rather_than_a_false_distance() {
        let scene = Scene::empty().with_reflector(Reflector::new(0.5, 0.01).unwrap());
        assert!(scene.nearest_echo(0.05).is_none());
        assert!(scene.nearest_echo(0.01).is_some());
        assert!(Scene::empty().nearest_echo(0.0).is_none());
    }

    #[test]
    fn invalid_reflectors_are_rejected() {
        assert_eq!(Reflector::new(0.0, 1.0), Err(SceneError::InvalidDistance));
        assert_eq!(Reflector::new(-1.0, 1.0), Err(SceneError::InvalidDistance));
        assert_eq!(
            Reflector::new(f64::NAN, 1.0),
            Err(SceneError::InvalidDistance)
        );
        assert_eq!(
            Reflector::new(1.0, 1.5),
            Err(SceneError::InvalidReflectivity)
        );
        assert_eq!(
            Reflector::new(1.0, -0.1),
            Err(SceneError::InvalidReflectivity)
        );
    }
}
