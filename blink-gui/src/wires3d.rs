// SPDX-License-Identifier: MIT

//! Cylinder-and-sphere drawing helpers shared by the 3D views.
//!
//! Both the blink circuit and the ranging rig are built from the same generated
//! unit meshes, so the segment maths lives here instead of being duplicated.
//! Generated raylib cylinders run from `y = 0` to `y = 1` rather than being
//! centred, so a segment's draw position is its start point.

use raylib::prelude::*;

/// Draw a ball of `radius` at `position`.
pub fn ball<D: RaylibDraw3D>(
    d: &mut D,
    sphere: &Model,
    position: Vector3,
    radius: f32,
    colour: Color,
) {
    d.draw_model(sphere, position, radius, colour);
}

/// Rotate a `+Y` unit cylinder onto a segment.
///
/// Returns `(axis, angle_degrees, length)`, or `None` for a degenerate segment.
pub fn segment_transform(start: Vector3, end: Vector3) -> Option<(Vector3, f32, f32)> {
    let v = Vector3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    let length = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if length < 0.00001 {
        return None;
    }
    // Cross product of +Y with the segment direction.
    let mut axis = Vector3::new(v.z, 0.0, -v.x);
    let axis_length = (axis.x * axis.x + axis.z * axis.z).sqrt();
    if axis_length < 0.00001 {
        axis = Vector3::new(1.0, 0.0, 0.0);
    } else {
        axis.x /= axis_length;
        axis.z /= axis_length;
    }
    let angle = (v.y / length).clamp(-1.0, 1.0).acos().to_degrees();
    Some((axis, angle, length))
}

/// Draw one straight tube of `radius` between two points.
pub fn tube<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    start: Vector3,
    end: Vector3,
    radius: f32,
    colour: Color,
) {
    if let Some((axis, angle, length)) = segment_transform(start, end) {
        d.draw_model_ex(
            cylinder,
            start,
            axis,
            angle,
            Vector3::new(radius, length, radius),
            colour,
        );
    }
}

/// Draw a polyline as a wire: tubes between the points, balls at the joins.
pub fn wire<D: RaylibDraw3D>(
    d: &mut D,
    cylinder: &Model,
    sphere: &Model,
    points: &[Vector3],
    radius: f32,
    colour: Color,
) {
    for pair in points.windows(2) {
        tube(d, cylinder, pair[0], pair[1], radius, colour);
    }
    for point in points {
        ball(d, sphere, *point, radius, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cylinder_rotation_reaches_segment_endpoint() {
        let start = Vector3::new(0.3, 0.4, 0.5);
        for direction in [
            Vector3::new(0.0, 2.0, 0.0),
            Vector3::new(0.0, -2.0, 0.0),
            Vector3::new(2.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, -2.0),
            Vector3::new(-1.0, 0.7, 2.0),
        ] {
            let end = Vector3::new(
                start.x + direction.x,
                start.y + direction.y,
                start.z + direction.z,
            );
            let (axis, angle, length) = segment_transform(start, end).unwrap();
            let (sin, cos) = angle.to_radians().sin_cos();
            // Rodrigues rotation of (0,length,0), with axis.y == 0.
            let rotated = Vector3::new(-axis.z * length * sin, length * cos, axis.x * length * sin);
            assert!((rotated.x - direction.x).abs() < 0.0001);
            assert!((rotated.y - direction.y).abs() < 0.0001);
            assert!((rotated.z - direction.z).abs() < 0.0001);
        }
    }

    #[test]
    fn zero_length_wire_segments_are_skipped() {
        assert!(segment_transform(Vector3::zero(), Vector3::zero()).is_none());
    }
}
