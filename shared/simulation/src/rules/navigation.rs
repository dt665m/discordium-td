use crate::*;

impl Globals {
    #[cfg(test)]
    pub(crate) fn navmesh_for_tick(&mut self, world: &mut World) -> NavMesh {
        if world.resource::<NavigationCache>().dirty
            || world.resource::<NavigationCache>().mesh.is_none()
        {
            let mesh = build_navigation_mesh(world);
            let mut cache = world.resource_mut::<NavigationCache>();
            cache.mesh = Some(mesh);
            cache.dirty = false;
        }
        world
            .resource::<NavigationCache>()
            .mesh
            .as_ref()
            .unwrap()
            .clone()
    }
}

pub(crate) fn find_next_waypoint(
    start_pos: [f32; 2],
    goal_pos: [f32; 2],
    navmesh: &NavMesh,
) -> Option<[f32; 2]> {
    let start = clamp_navmesh_point(start_pos);
    let mut goal = clamp_navmesh_point(goal_pos);
    if !navmesh.is_in_mesh(goal.into()) {
        goal = project_to_walkable(goal, navmesh, 10, 0.45)?;
    }

    let mut from = start;
    if !navmesh.is_in_mesh(from.into()) {
        from = project_to_walkable(from, navmesh, 8, 0.35)?;
    }

    if distance_sq(from, goal) <= ENEMY_WAYPOINT_REACH_RADIUS.powi(2) {
        return Some(goal);
    }

    let path = navmesh.path(from.into(), goal.into())?;
    let min_step_sq = (ENEMY_WAYPOINT_REACH_RADIUS * 0.5).powi(2);
    for step in path.path {
        let waypoint = clamp_to_world([step.x, step.y]);
        if distance_sq(waypoint, start_pos) > min_step_sq {
            return Some(waypoint);
        }
    }

    Some(goal)
}

#[cfg(test)]
pub(crate) fn build_navigation_mesh(world: &World) -> NavMesh {
    let positions: Vec<_> = storage::towers(world)
        .map(|tower| tower.position.pos)
        .collect();
    build_navigation_mesh_from_positions(&positions)
}

pub(crate) fn build_navigation_mesh_from_positions(positions: &[[f32; 2]]) -> NavMesh {
    let half_width = WORLD_HALF_WIDTH - NAVMESH_MARGIN;
    let half_height = WORLD_HALF_HEIGHT - NAVMESH_MARGIN;

    let edges = vec![
        [-half_width, -half_height].into(),
        [half_width, -half_height].into(),
        [half_width, half_height].into(),
        [-half_width, half_height].into(),
    ];

    let mut obstacles = Vec::with_capacity(positions.len());
    for &pos in positions {
        if let Some(obstacle) = tower_obstacle_polygon(pos) {
            obstacles.push(obstacle.into_iter().map(Into::into).collect());
        }
    }

    let mut navmesh = NavMesh::from_edge_and_obstacles(edges, obstacles);
    let _ = navmesh.set_search_delta(NAVMESH_SEARCH_DELTA);
    let _ = navmesh.set_search_steps(NAVMESH_SEARCH_STEPS);
    navmesh
}

pub(crate) fn tower_obstacle_polygon(pos: [f32; 2]) -> Option<Vec<[f32; 2]>> {
    let max_radius_x = (WORLD_HALF_WIDTH - NAVMESH_MARGIN - pos[0].abs()).max(0.0);
    let max_radius_y = (WORLD_HALF_HEIGHT - NAVMESH_MARGIN - pos[1].abs()).max(0.0);
    let radius = TOWER_NAV_BLOCK_RADIUS.min(max_radius_x).min(max_radius_y);

    if radius <= 0.05 {
        return None;
    }

    let mut points = Vec::with_capacity(TOWER_NAV_BLOCK_VERTICES);
    for i in 0..TOWER_NAV_BLOCK_VERTICES {
        let angle = (i as f32 / TOWER_NAV_BLOCK_VERTICES as f32) * TAU;
        let point =
            clamp_navmesh_point([pos[0] + radius * angle.cos(), pos[1] + radius * angle.sin()]);
        points.push(point);
    }

    Some(points)
}

pub(crate) fn clamp_navmesh_point(pos: [f32; 2]) -> [f32; 2] {
    [
        pos[0].clamp(
            -WORLD_HALF_WIDTH + NAVMESH_MARGIN,
            WORLD_HALF_WIDTH - NAVMESH_MARGIN,
        ),
        pos[1].clamp(
            -WORLD_HALF_HEIGHT + NAVMESH_MARGIN,
            WORLD_HALF_HEIGHT - NAVMESH_MARGIN,
        ),
    ]
}

pub(crate) fn project_to_walkable(
    origin: [f32; 2],
    navmesh: &NavMesh,
    max_radius: i32,
    step: f32,
) -> Option<[f32; 2]> {
    let origin = clamp_navmesh_point(origin);
    if navmesh.is_in_mesh(origin.into()) {
        return Some(origin);
    }

    for ring in 1..=max_radius {
        let ringf = ring as f32 * step;
        let samples = (ring * 10).max(10);

        for i in 0..samples {
            let angle = (i as f32 / samples as f32) * TAU;
            let candidate = clamp_navmesh_point([
                origin[0] + ringf * angle.cos(),
                origin[1] + ringf * angle.sin(),
            ]);
            if navmesh.is_in_mesh(candidate.into()) {
                return Some(candidate);
            }
        }
    }

    None
}
