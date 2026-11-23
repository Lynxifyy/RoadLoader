//! Entity ticking and lightweight mob pathfinding for the 32-bit engine.
//!
//! `tick_entities` mirrors what `Level::tickEntities` does for the simple mob
//! set: gravity, swept collision and ground detection. `find_path` is a bounded
//! A* over the block grid used by the Rust AI pass. Both are pure functions so
//! they can run on a scoped thread pool without touching engine state.

use glam::Vec3;

use crate::math::Aabb;
use crate::physics::{sweep, SweepResult};

/// Minimal entity representation shared with the C++ trampoline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entity {
    pub id: u32,
    pub position: Vec3,
    pub velocity: Vec3,
    pub half_extents: Vec3,
    pub on_ground: bool,
}

impl Entity {
    pub fn new(id: u32, position: Vec3) -> Self {
        Self {
            id,
            position,
            velocity: Vec3::ZERO,
            half_extents: Vec3::new(0.3, 0.9, 0.3),
            on_ground: false,
        }
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::from_center(self.position, self.half_extents)
    }
}

/// Advances every entity by `dt`, resolving against static solids.
pub fn tick_entities(
    entities: &mut [Entity],
    solids: &[Aabb],
    dt: f32,
    gravity: f32,
    max_speed: f32,
) {
    for entity in entities.iter_mut() {
        entity.velocity.y -= gravity * dt;
        let horizontal = Vec3::new(entity.velocity.x, 0.0, entity.velocity.z);
        if horizontal.length() > max_speed {
            let clamped = horizontal.normalize_or_zero() * max_speed;
            entity.velocity.x = clamped.x;
            entity.velocity.z = clamped.z;
        }
        let result = sweep(entity.position, entity.half_extents, entity.velocity, solids, dt);
        apply_sweep(entity, result);
    }
}

/// Same as [`tick_entities`], split across scoped threads.
///
/// The split is only used for large populations; below the threshold the
/// single-threaded path avoids the synchronisation cost entirely.
pub fn tick_entities_parallel(
    entities: &mut [Entity],
    solids: &[Aabb],
    dt: f32,
    gravity: f32,
    max_speed: f32,
    threads: usize,
) {
    const PARALLEL_THRESHOLD: usize = 64;
    if threads <= 1 || entities.len() < PARALLEL_THRESHOLD {
        tick_entities(entities, solids, dt, gravity, max_speed);
        return;
    }
    let chunk = entities.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for slice in entities.chunks_mut(chunk) {
            scope.spawn(move || tick_entities(slice, solids, dt, gravity, max_speed));
        }
    });
}

fn apply_sweep(entity: &mut Entity, result: SweepResult) {
    entity.position = result.position;
    entity.velocity = result.velocity;
    entity.on_ground = result.on_ground;
}

/// A* over a 3D block grid. `grid` uses 0 for walkable and non-zero for solid.
pub fn find_path(
    grid: &[u8],
    dims: [u32; 3],
    start: [u32; 3],
    goal: [u32; 3],
    max_nodes: usize,
) -> Option<Vec<[u32; 3]>> {
    let [dx, dy, dz] = dims;
    if dx == 0 || dy == 0 || dz == 0 {
        return None;
    }
    let index = |p: [u32; 3]| -> usize { (p[0] + dx * (p[1] + dy * p[2])) as usize };
    let inside = |p: [u32; 3]| p[0] < dx && p[1] < dy && p[2] < dz;
    if !inside(start) || !inside(goal) || grid.len() < (dx as usize * dy as usize * dz as usize) {
        return None;
    }
    if grid[index(start)] != 0 || grid[index(goal)] != 0 {
        return None;
    }
    if start == goal {
        return Some(vec![start]);
    }

    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    let heuristic = |p: [u32; 3]| -> u32 {
        p[0].abs_diff(goal[0]) + p[1].abs_diff(goal[1]) + p[2].abs_diff(goal[2])
    };
    let mut open: BinaryHeap<Reverse<(u32, u32, [u32; 3])>> = BinaryHeap::new();
    let mut best: HashMap<[u32; 3], u32> = HashMap::new();
    let mut came_from: HashMap<[u32; 3], [u32; 3]> = HashMap::new();
    best.insert(start, 0);
    open.push(Reverse((heuristic(start), 0, start)));
    let mut expanded = 0usize;
    let neighbours = [
        [1i32, 0, 0],
        [-1, 0, 0],
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, 1],
        [0, 0, -1],
    ];

    while let Some(Reverse((_, cost, current))) = open.pop() {
        if current == goal {
            let mut path = vec![current];
            let mut node = current;
            while let Some(&previous) = came_from.get(&node) {
                path.push(previous);
                node = previous;
            }
            path.reverse();
            return Some(path);
        }
        expanded += 1;
        if expanded > max_nodes {
            return None;
        }
        for offset in neighbours {
            let next = [
                current[0].wrapping_add_signed(offset[0]),
                current[1].wrapping_add_signed(offset[1]),
                current[2].wrapping_add_signed(offset[2]),
            ];
            if !inside(next) || grid[index(next)] != 0 {
                continue;
            }
            let next_cost = cost + 1;
            if best.get(&next).is_some_and(|&known| known <= next_cost) {
                continue;
            }
            best.insert(next, next_cost);
            came_from.insert(next, current);
            open.push(Reverse((next_cost + heuristic(next), next_cost, next)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> Aabb {
        Aabb::new(Vec3::new(-8.0, -1.0, -8.0), Vec3::new(8.0, 0.0, 8.0))
    }

    #[test]
    fn entities_fall_and_land() {
        let mut entities = vec![Entity::new(1, Vec3::new(0.0, 4.0, 0.0))];
        for _ in 0..60 {
            tick_entities(&mut entities, &[floor()], 1.0 / 60.0, 20.0, 10.0);
        }
        assert!(entities[0].on_ground);
        assert!(entities[0].position.y < 1.1);
    }

    #[test]
    fn horizontal_speed_is_clamped() {
        let mut entity = Entity::new(1, Vec3::new(0.0, 5.0, 0.0));
        entity.velocity = Vec3::new(100.0, 0.0, 0.0);
        let mut entities = [entity];
        tick_entities(&mut entities, &[], 0.016, 0.0, 5.0);
        assert!(entities[0].velocity.x.abs() <= 5.000_001);
    }

    #[test]
    fn parallel_and_serial_ticks_agree() {
        let solids = [floor()];
        let mut serial: Vec<Entity> = (0..128)
            .map(|id| Entity::new(id, Vec3::new((id % 8) as f32 * 0.5, 3.0, 0.0)))
            .collect();
        let mut parallel = serial.clone();
        tick_entities(&mut serial, &solids, 1.0 / 60.0, 20.0, 10.0);
        tick_entities_parallel(&mut parallel, &solids, 1.0 / 60.0, 20.0, 10.0, 4);
        for (a, b) in serial.iter().zip(&parallel) {
            assert_eq!(a, b);
        }
    }

    fn grid_3d() -> ([u8; 27], [u32; 3]) {
        ([0u8; 27], [3, 3, 3])
    }

    #[test]
    fn path_is_a_connected_chain() {
        let (grid, dims) = grid_3d();
        let path = find_path(&grid, dims, [0, 0, 0], [2, 0, 0], 64).expect("path");
        assert_eq!(path.first(), Some(&[0, 0, 0]));
        assert_eq!(path.last(), Some(&[2, 0, 0]));
        for pair in path.windows(2) {
            let distance = pair[0][0].abs_diff(pair[1][0])
                + pair[0][1].abs_diff(pair[1][1])
                + pair[0][2].abs_diff(pair[1][2]);
            assert_eq!(distance, 1);
        }
    }

    #[test]
    fn path_routes_around_a_wall() {
        let mut grid = [0u8; 27];
        // Wall in the x=1 plane, leaving a gap at y=2.
        for y in 0..2 {
            for z in 0..3 {
                grid[(1 + 3 * (y + 3 * z)) as usize] = 1;
            }
        }
        let path = find_path(&grid, [3, 3, 3], [0, 0, 0], [2, 0, 0], 128).expect("path");
        assert!(path.contains(&[1, 2, 0]), "path={path:?}");
    }

    #[test]
    fn unreachable_goal_returns_none() {
        let mut grid = [0u8; 27];
        for y in 0..3 {
            for z in 0..3 {
                grid[(1 + 3 * (y + 3 * z)) as usize] = 1;
            }
        }
        assert_eq!(find_path(&grid, [3, 3, 3], [0, 0, 0], [2, 0, 0], 128), None);
    }

    #[test]
    fn blocked_endpoints_are_rejected() {
        let mut grid = [0u8; 27];
        grid[0] = 1;
        assert_eq!(find_path(&grid, [3, 3, 3], [0, 0, 0], [2, 2, 2], 128), None);
    }

    #[test]
    fn node_cap_bounds_the_search() {
        let (grid, dims) = grid_3d();
        assert_eq!(find_path(&grid, dims, [0, 0, 0], [2, 2, 2], 1), None);
    }
}
