//! Swept-AABB collision solver for the companion process.
//!
//! The solver is deliberately the same shape as the engine's `Actor::move`:
//! axis-separated movement with resolution against static boxes. It is written
//! with `glam` so the 32-bit build uses NEON for the vector operations and stays
//! within IEEE-754 semantics.

use glam::Vec3;

use crate::math::Aabb;

/// Skin width used when snapping a body against a face; large enough to avoid
/// re-penetration from float rounding, small enough to be invisible.
const EPSILON: f32 = 1e-4;
/// Maximum distance moved per sub-step. Must stay below half a block so a body
/// can never tunnel through a one-block wall at any frame rate.
const MAX_STEP: f32 = 0.4;
const MAX_SUBSTEPS: usize = 64;

/// Result of a swept move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepResult {
    pub position: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub hit_x: bool,
    pub hit_y: bool,
    pub hit_z: bool,
}

/// Moves a body with the given half-extents by `velocity * dt`, resolving
/// against `solids`.
pub fn sweep(start: Vec3, half_extents: Vec3, velocity: Vec3, solids: &[Aabb], dt: f32) -> SweepResult {
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let displacement = velocity * dt;
    let largest = displacement.abs().max_element();
    if !velocity.is_finite() || !displacement.is_finite() || !largest.is_finite() {
        return SweepResult {
            position: start,
            velocity: Vec3::ZERO,
            on_ground: false,
            hit_x: false,
            hit_y: false,
            hit_z: false,
        };
    }
    let substeps = ((largest / MAX_STEP).ceil() as usize).clamp(1, MAX_SUBSTEPS);
    let step = displacement / substeps as f32;
    let mut position = start;
    let mut result = SweepResult {
        position: start,
        velocity,
        on_ground: false,
        hit_x: false,
        hit_y: false,
        hit_z: false,
    };

    for _ in 0..substeps {
        if step.x != 0.0 {
            position.x += step.x;
            if resolve_axis(&mut position, half_extents, solids, 0, step.x) {
                result.hit_x = true;
                result.velocity.x = 0.0;
            }
        }
        if step.y != 0.0 {
            position.y += step.y;
            if resolve_axis(&mut position, half_extents, solids, 1, step.y) {
                result.hit_y = true;
                result.velocity.y = 0.0;
                result.on_ground = step.y < 0.0;
            }
        }
        if step.z != 0.0 {
            position.z += step.z;
            if resolve_axis(&mut position, half_extents, solids, 2, step.z) {
                result.hit_z = true;
                result.velocity.z = 0.0;
            }
        }
    }
    result.position = position;
    result
}

/// Strict overlap used for resolution: boxes that merely touch do not collide.
/// This is what keeps a body standing on a floor from being pushed sideways by
/// the floor it rests on.
fn overlaps(body: &Aabb, solid: &Aabb) -> bool {
    body.min.x < solid.max.x - EPSILON
        && body.max.x > solid.min.x + EPSILON
        && body.min.y < solid.max.y - EPSILON
        && body.max.y > solid.min.y + EPSILON
        && body.min.z < solid.max.z - EPSILON
        && body.max.z > solid.min.z + EPSILON
}

fn resolve_axis(position: &mut Vec3, half: Vec3, solids: &[Aabb], axis: usize, motion: f32) -> bool {
    let mut collided = false;
    // Two passes: snapping out of one box can push the body into another.
    for _ in 0..2 {
        let body = Aabb::from_center(*position, half);
        let mut resolved_this_pass = false;
        for solid in solids {
            if !overlaps(&body, solid) {
                continue;
            }
            match axis {
                0 => {
                    if motion > 0.0 {
                        position.x = solid.min.x - half.x - EPSILON;
                    } else {
                        position.x = solid.max.x + half.x + EPSILON;
                    }
                }
                1 => {
                    if motion > 0.0 {
                        position.y = solid.min.y - half.y - EPSILON;
                    } else {
                        position.y = solid.max.y + half.y + EPSILON;
                    }
                }
                _ => {
                    if motion > 0.0 {
                        position.z = solid.min.z - half.z - EPSILON;
                    } else {
                        position.z = solid.max.z + half.z + EPSILON;
                    }
                }
            }
            collided = true;
            resolved_this_pass = true;
        }
        if !resolved_this_pass {
            break;
        }
    }
    collided
}

/// Ray/AABB intersection returning the entry distance, used for attacks and
/// block picking. `direction` must be normalised.
pub fn ray_aabb(origin: Vec3, direction: Vec3, solid: &Aabb, max_distance: f32) -> Option<f32> {
    let inv = Vec3::new(
        safe_inverse(direction.x),
        safe_inverse(direction.y),
        safe_inverse(direction.z),
    );
    let t1 = (solid.min - origin) * inv;
    let t2 = (solid.max - origin) * inv;
    let tmin = t1.min(t2);
    let tmax = t1.max(t2);
    let entry = tmin.x.max(tmin.y).max(tmin.z).max(0.0);
    let exit = tmax.x.min(tmax.y).min(tmax.z);
    if entry <= exit && entry <= max_distance {
        Some(entry)
    } else {
        None
    }
}

fn safe_inverse(value: f32) -> f32 {
    if value.abs() < 1e-8 {
        // Treat as a ray parallel to the slab: a huge finite value keeps the
        // slab test well defined without producing NaNs.
        if value.is_sign_negative() {
            -1e30
        } else {
            1e30
        }
    } else {
        1.0 / value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: Vec3 = Vec3::new(0.3, 0.9, 0.3);

    fn floor() -> Aabb {
        Aabb::new(Vec3::new(-8.0, -1.0, -8.0), Vec3::new(8.0, 0.0, 8.0))
    }

    #[test]
    fn falling_body_lands_on_the_floor() {
        let result = sweep(Vec3::new(0.0, 2.0, 0.0), HALF, Vec3::new(0.0, -10.0, 0.0), &[floor()], 0.5);
        assert!(result.on_ground);
        assert!((result.position.y - (0.0 + HALF.y + EPSILON)).abs() < 1e-3, "y={}", result.position.y);
        assert_eq!(result.velocity.y, 0.0);
    }

    #[test]
    fn walking_into_a_wall_stops_horizontally() {
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0));
        let result = sweep(Vec3::new(0.0, 0.9, 0.0), HALF, Vec3::new(10.0, 0.0, 0.0), &[floor(), wall], 0.5);
        assert!(result.hit_x);
        assert!(result.position.x < 1.0, "x={} pos={:?}", result.position.x, result.position);
        assert!((result.position.x - (1.0 - HALF.x - EPSILON)).abs() < 1e-3, "x={}", result.position.x);
        assert_eq!(result.velocity.x, 0.0);
    }

    #[test]
    fn high_speed_does_not_tunnel_through_a_wall() {
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0));
        let result = sweep(Vec3::new(0.0, 0.9, 0.0), HALF, Vec3::new(400.0, 0.0, 0.0), &[floor(), wall], 0.5);
        assert!(result.hit_x, "position={:?}", result.position);
        assert!(result.position.x < 1.0);
    }

    #[test]
    fn sliding_along_a_wall_preserves_the_other_axis() {
        let wall = Aabb::new(Vec3::new(1.0, 0.0, -8.0), Vec3::new(2.0, 3.0, 8.0));
        let result = sweep(Vec3::new(0.0, 0.9, 0.0), HALF, Vec3::new(4.0, 0.0, 4.0), &[floor(), wall], 0.5);
        assert!(result.hit_x);
        assert!(result.position.z > 0.5, "z={}", result.position.z);
    }

    #[test]
    fn no_solids_means_unobstructed_motion() {
        // `sweep` clamps dt to 0.25 s per call, like the engine's fixed tick.
        let result = sweep(Vec3::ZERO, HALF, Vec3::new(1.0, 2.0, 3.0), &[], 0.25);
        assert!((result.position - Vec3::new(0.25, 0.5, 0.75)).length() < 1e-5, "position={:?}", result.position);
        assert!(!result.on_ground);
    }

    #[test]
    fn non_finite_velocity_is_rejected() {
        let result = sweep(Vec3::ZERO, HALF, Vec3::new(f32::NAN, 0.0, 0.0), &[floor()], 0.1);
        assert_eq!(result.position, Vec3::ZERO);
    }

    #[test]
    fn ray_hits_box_from_outside() {
        let solid = Aabb::new(Vec3::new(2.0, -1.0, -1.0), Vec3::new(4.0, 1.0, 1.0));
        let hit = ray_aabb(Vec3::ZERO, Vec3::X, &solid, 10.0);
        assert_eq!(hit, Some(2.0));
        assert_eq!(ray_aabb(Vec3::ZERO, Vec3::X, &solid, 1.0), None);
        assert_eq!(ray_aabb(Vec3::ZERO, -Vec3::X, &solid, 10.0), None);
    }

    #[test]
    fn ray_parallel_to_slab_is_safe() {
        let solid = Aabb::new(Vec3::new(-1.0, 2.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
        assert_eq!(ray_aabb(Vec3::ZERO, Vec3::X, &solid, 10.0), None);
    }
}
