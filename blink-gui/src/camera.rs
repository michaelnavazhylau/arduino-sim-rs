// SPDX-License-Identifier: MIT

//! An orbit camera built directly on `raylib::ffi::Camera3D`.
//!
//! raylib-rs gates its *entire* camera module behind
//! `#[cfg(not(feature = "nobuild"))]` (see `core/mod.rs` and `prelude.rs`), so
//! under this crate's `nobuild` setup there is no `Camera3D` wrapper,
//! `Camera3D::perspective`, `Camera2D`, `UpdateCamera` or mouse-orbit helper.
//!
//! Every symbol that module needs is nevertheless present in the system raylib
//! (checked with `nm` against the Homebrew dylib), so the gate is conservative
//! rather than a real limitation. The struct below owns the handful of lines
//! those helpers would have supplied.

use raylib::ffi::{Camera3D as RawCamera, CameraProjection};
use raylib::prelude::*;

/// A camera that orbits a fixed target point.
pub struct OrbitCamera {
    /// Rotation about the world Y axis, in degrees.
    yaw: f32,
    /// Elevation above the XZ plane, in degrees.
    pitch: f32,
    /// Distance from `target`.
    distance: f32,
    target: Vector3,
}

impl OrbitCamera {
    /// A three-quarter view of a board centred on the origin.
    pub fn new(target: Vector3, distance: f32) -> Self {
        Self {
            yaw: 34.0,
            pitch: 30.0,
            distance,
            target,
        }
    }

    /// Apply this frame's mouse input. Left-drag orbits, the wheel zooms.
    pub fn update(&mut self, rl: &RaylibHandle) {
        if rl.is_mouse_button_down(MouseButton::MOUSE_BUTTON_LEFT) {
            let delta = rl.get_mouse_delta();
            self.yaw -= delta.x * 0.28;
            self.pitch = (self.pitch + delta.y * 0.28).clamp(3.0, 87.0);
        }
        let wheel = rl.get_mouse_wheel_move();
        if wheel != 0.0 {
            self.distance = (self.distance * (1.0 - wheel * 0.1)).clamp(4.0, 40.0);
        }
    }

    /// The raw camera struct `BeginMode3D` expects.
    ///
    /// `projection` is a plain `i32` in the raylib-sys 6.0 bindings rather than
    /// the `CameraProjection` enum, hence the cast.
    pub fn raw(&self) -> RawCamera {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        let offset = Vector3::new(
            self.distance * pitch.cos() * yaw.sin(),
            self.distance * pitch.sin(),
            self.distance * pitch.cos() * yaw.cos(),
        );
        RawCamera {
            position: Vector3::new(
                self.target.x + offset.x,
                self.target.y + offset.y,
                self.target.z + offset.z,
            ),
            target: self.target,
            up: Vector3::new(0.0, 1.0, 0.0),
            fovy: 40.0,
            projection: CameraProjection::CAMERA_PERSPECTIVE as i32,
        }
    }
}
