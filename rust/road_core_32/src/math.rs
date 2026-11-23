//! Vector and bounding-box math on top of `glam`.
//!
//! Every function is `f32` and compiled without fast-math, so results match the
//! game's IEEE-754 semantics bit for bit for the operations the engine relies on
//! (add, sub, mul, div, sqrt). `glam`'s `libm` feature supplies a correctly
//! rounded `sqrt` on targets without hardware intrinsics.

use glam::{Mat4, Vec3};

/// A 32-bit axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_center(center: Vec3, half_extents: Vec3) -> Self {
        Self { min: center - half_extents, max: center + half_extents }
    }

    pub fn from_points(a: Vec3, b: Vec3) -> Self {
        Self { min: a.min(b), max: a.max(b) }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn expand(&self, amount: f32) -> Self {
        Self { min: self.min - Vec3::splat(amount), max: self.max + Vec3::splat(amount) }
    }

    /// Standard inclusive-overlap test used by the engine's hit registration.
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    pub fn contains_point(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    pub fn translated(&self, offset: Vec3) -> Self {
        Self { min: self.min + offset, max: self.max + offset }
    }
}

/// `Vec3::distance` equivalent, exported to the game through a trampoline.
pub fn distance(a: Vec3, b: Vec3) -> f32 {
    (b - a).length()
}

/// `Vec3::length` equivalent.
pub fn length(v: Vec3) -> f32 {
    v.length()
}

/// AABB/AABB overlap test, exported to the game through a trampoline.
pub fn intersects(a: &Aabb, b: &Aabb) -> bool {
    a.intersects(b)
}

/// Applies a `Mat4` to a point, matching the engine's column-major convention.
pub fn transform_point(matrix: &Mat4, point: Vec3) -> Vec3 {
    matrix.transform_point3(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_matches_expected_values() {
        assert_eq!(distance(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0)), 5.0);
        assert_eq!(distance(Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 1.0, 1.0)), 0.0);
        let value = distance(Vec3::new(-2.0, 0.5, 7.0), Vec3::new(4.0, -3.5, 7.0));
        assert!((value - 7.211_102_5).abs() < 1e-5);
    }

    #[test]
    fn length_is_ieee_754_sqrt() {
        assert_eq!(length(Vec3::new(3.0, 4.0, 0.0)), 5.0);
        assert_eq!(length(Vec3::ZERO), 0.0);
        assert!(length(Vec3::new(f32::INFINITY, 0.0, 0.0)).is_infinite());
    }

    #[test]
    fn aabb_intersection_is_inclusive_on_touching_faces() {
        let a = Aabb::new(Vec3::ZERO, Vec3::splat(1.0));
        let b = Aabb::new(Vec3::splat(1.0), Vec3::splat(2.0));
        assert!(a.intersects(&b));
        let c = Aabb::new(Vec3::splat(1.001), Vec3::splat(2.0));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn aabb_from_points_normalises() {
        let boxed = Aabb::from_points(Vec3::new(2.0, -1.0, 5.0), Vec3::new(-3.0, 4.0, 1.0));
        assert_eq!(boxed.min, Vec3::new(-3.0, -1.0, 1.0));
        assert_eq!(boxed.max, Vec3::new(2.0, 4.0, 5.0));
        assert_eq!(boxed.size(), Vec3::new(5.0, 5.0, 4.0));
    }

    #[test]
    fn contains_point_is_inclusive() {
        let a = Aabb::new(Vec3::ZERO, Vec3::splat(2.0));
        assert!(a.contains_point(Vec3::splat(2.0)));
        assert!(a.contains_point(Vec3::ZERO));
        assert!(!a.contains_point(Vec3::splat(2.5)));
    }

    #[test]
    fn transform_point_uses_column_major_convention() {
        let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(transform_point(&matrix, Vec3::new(4.0, 5.0, 6.0)), Vec3::new(5.0, 7.0, 9.0));
    }
}
