use super::{
    model::*,
    scene::{CollisionScene, Hit, MAX_COORDINATE, Queries, bounded},
};
use bevy::prelude::*;
const EPSILON: f32 = 0.00001;

/// Validate a saved capsule/config against its exact immutable collision scene.
/// This runs the controller's invariants without queries or state advancement.
pub fn validate_kinematic_state(
    state: &KinematicState,
    config: &KinematicConfig,
    scene: &CollisionScene,
) -> Result<(), KinematicError> {
    validate(config, state, KinematicInput::default(), scene)
}

/// Executes exactly one configured fixed step. Every error leaves the supplied
/// state unchanged. Results require the same config, scene revision and stored
/// input during replay; no live input, wall clock or rendering state is read.
pub fn advance_kinematic(
    state: &mut KinematicState,
    config: &KinematicConfig,
    input: KinematicInput,
    scene: &CollisionScene,
) -> Result<KinematicReport, KinematicError> {
    if state.base.attachment.is_some() {
        return Err(KinematicError::MissingBaseHistory);
    }
    advance(state, config, input, scene, None, None)
}
/// Advances against one immutable historical base interval. Base carry runs
/// before player intent; jump/detach inherits before the movement sweep, and
/// walking off inherits after the final support query. Attached velocity is
/// support-relative, and parent changes convert through world velocity once.
pub fn advance_kinematic_with_bases(
    state: &mut KinematicState,
    config: &KinematicConfig,
    input: KinematicInput,
    scene: &CollisionScene,
    bases: super::BaseStep<'_>,
) -> Result<KinematicReport, KinematicError> {
    advance(state, config, input, scene, Some(bases), None)
}
/// Consumes authored planar displacement through the same capsule sweep as
/// ordinary motion. The caller stages its curve cursor until this succeeds.
pub fn advance_kinematic_with_authored_motion(
    state: &mut KinematicState,
    config: &KinematicConfig,
    input: KinematicInput,
    scene: &CollisionScene,
    bases: super::BaseStep<'_>,
    planar_delta: [f32; 2],
) -> Result<KinematicReport, KinematicError> {
    if !bounded(planar_delta, 100.0) {
        return Err(KinematicError::InvalidInput);
    }
    advance(state, config, input, scene, Some(bases), Some(planar_delta))
}
fn advance(
    state: &mut KinematicState,
    config: &KinematicConfig,
    input: KinematicInput,
    scene: &CollisionScene,
    bases: Option<super::BaseStep<'_>>,
    authored: Option<[f32; 2]>,
) -> Result<KinematicReport, KinematicError> {
    validate(config, state, input, scene)?;
    let mut next = *state;
    let mut queries = Queries::new(scene, config.max_queries);
    let mut report = KinematicReport::default();
    let mut base_transition = bases
        .map(|step| {
            super::bases::prepare(&mut next, config, scene, step, &mut queries, &mut report)
        })
        .transpose()?;
    let mut position = Vec3::from_array(next.position);
    let mut velocity = Vec3::from_array(next.velocity);
    if input.crouch {
        next.stance = Stance::Crouched;
    } else if next.stance == Stance::Crouched {
        if queries
            .overlap(position, config.standing_height, config.radius, EPSILON)?
            .is_none()
        {
            next.stance = Stance::Standing;
        } else {
            report.stand_blocked = true;
        }
    }
    let height = config.height(next.stance);
    depenetrate(&mut position, height, config, &mut queries, &mut report)?;

    // Establish support from this exact scene, including the first step after
    // spawn. A teleport/jump suppression fence prevents artificial ground snap.
    let previously_grounded = next.grounded;
    next.grounded = false;
    next.ground = None;
    if next.suppress_snap_ticks == 0 && (velocity.y <= 0.0 || previously_grounded) {
        snap(
            &mut position,
            &mut next,
            height,
            config,
            scene,
            &mut queries,
        )?;
    }
    let supported = next.grounded;
    if supported {
        next.coyote_ticks = config.coyote_ticks;
    }
    if input.jump {
        next.jump_buffer_ticks = config.jump_buffer_ticks.max(1);
    }
    let movement = Vec2::from_array(input.movement).clamp_length_max(1.0);
    if movement.length_squared() > EPSILON * EPSILON {
        next.facing = movement.normalize().to_array();
    }
    if next.jump_buffer_ticks > 0
        && (supported || next.coyote_ticks > 0)
        && next.movement_lock_ticks == 0
        && next.dash_ticks == 0
    {
        velocity.y = config.jump_speed;
        next.jump_buffer_ticks = 0;
        next.coyote_ticks = 0;
        next.grounded = false;
        next.ground = None;
        report.jumped = true;
    }
    if input.dash
        && next.dash_ticks == 0
        && next.dash_cooldown_ticks == 0
        && next.movement_lock_ticks == 0
    {
        next.dash_direction = next.facing;
        next.dash_ticks = config.dash_duration_ticks;
        next.dash_cooldown_ticks = config.dash_cooldown_ticks;
    }
    let mut horizontal = Vec2::new(velocity.x, velocity.z);
    if supported && horizontal.length_squared() > EPSILON * EPSILON {
        horizontal = horizontal.normalize() * velocity.length();
    }
    if next.dash_ticks > 0 {
        horizontal = Vec2::from_array(next.dash_direction) * config.dash_speed;
    } else if next.movement_lock_ticks > 0 {
        horizontal = Vec2::ZERO;
    } else {
        let speed = if next.stance == Stance::Crouched {
            config.crouched_speed
        } else {
            config.speed
        };
        let target = movement * speed;
        let acceleration = if !supported {
            config.air_acceleration
        } else if movement == Vec2::ZERO {
            config.braking
        } else {
            config.acceleration
        };
        horizontal = move_towards(horizontal, target, acceleration * config.fixed_dt);
    }
    if let Some(delta) = authored {
        horizontal = Vec2::from_array(delta) / config.fixed_dt;
    }
    velocity.x = horizontal.x;
    velocity.z = horizontal.y;
    if supported && !report.jumped {
        let normal = Vec3::from_array(next.ground.expect("supported contact").normal);
        let tangent = Vec3::new(horizontal.x, 0.0, horizontal.y);
        velocity =
            (tangent - normal * tangent.dot(normal)).normalize_or_zero() * horizontal.length();
    } else if !report.jumped {
        velocity.y = (velocity.y - config.gravity * config.fixed_dt).max(-config.terminal_speed);
    }
    if report.jumped || !supported {
        if let Some(transition) = &mut base_transition {
            transition.inherit_departure(&mut velocity);
        }
    }
    let mut remaining = velocity * config.fixed_dt;
    for _ in 0..config.max_slide_iterations {
        if remaining.length_squared() <= EPSILON * EPSILON {
            remaining = Vec3::ZERO;
            break;
        }
        let Some(hit) = queries.cast(position, height, config.radius, remaining, config.skin)?
        else {
            position += remaining;
            remaining = Vec3::ZERO;
            break;
        };
        report.slide_contacts += 1;
        // Step tests consume the same horizontal intent, never an extra forward
        // probe that could grant more movement than the fixed tick budget.
        if supported
            && !report.jumped
            && hit.normal.y < config.walkable_normal_y
            && config.max_step_height > 0.0
            && !report.stepped
        {
            if let Some((landing, ground)) =
                try_step(position, remaining, height, config, &mut queries)?
            {
                position = landing;
                next.grounded = true;
                next.ground = Some(scene.ground(ground, position));
                velocity.y = 0.0;
                remaining = Vec3::ZERO;
                report.stepped = true;
                break;
            }
        }
        position += remaining * hit.fraction;
        remaining *= 1.0 - hit.fraction;
        remaining = slide(remaining, hit.normal, config.walkable_normal_y);
        velocity = slide(velocity, hit.normal, config.walkable_normal_y);
    }
    if remaining.length_squared() > EPSILON * EPSILON {
        report.slide_limit_reached = true;
    }
    // On slopes the sweep may lift the capsule; allow support recovery when it
    // began supported. Airborne rising characters never snap down to ground.
    if !report.jumped && next.suppress_snap_ticks == 0 && (velocity.y <= 0.0 || supported) {
        snap(
            &mut position,
            &mut next,
            height,
            config,
            scene,
            &mut queries,
        )?;
        if let Some(ground) = next.ground {
            let normal = Vec3::from_array(ground.normal);
            velocity -= normal * velocity.dot(normal);
        }
    } else {
        next.grounded = false;
        next.ground = None;
    }
    depenetrate(&mut position, height, config, &mut queries, &mut report)?;
    if let Some(ground) = next.ground.as_mut() {
        // Depenetration may have adjusted the last support-relative point.
        *ground = scene.ground(
            Hit {
                collider: ground.collider,
                fraction: 0.0,
                normal: Vec3::from_array(ground.normal),
                point: position,
            },
            position,
        );
    }
    if let (Some(step), Some(transition)) = (bases, &mut base_transition) {
        super::bases::finish(
            &mut next,
            position,
            &mut velocity,
            step,
            transition,
            &mut report,
        )?;
    }
    next.position = position.to_array();
    next.velocity = velocity.to_array();
    for ticks in [
        &mut next.jump_buffer_ticks,
        &mut next.dash_ticks,
        &mut next.dash_cooldown_ticks,
        &mut next.movement_lock_ticks,
        &mut next.suppress_snap_ticks,
    ] {
        *ticks = ticks.saturating_sub(1);
    }
    if !supported {
        next.coyote_ticks = next.coyote_ticks.saturating_sub(1);
    }
    if !bounded(next.position, MAX_COORDINATE) || !bounded(next.velocity, 10_000.0) {
        return Err(KinematicError::InvalidState);
    }
    report.queries = queries.used;
    *state = next;
    Ok(report)
}
fn move_towards(current: Vec2, target: Vec2, distance: f32) -> Vec2 {
    current + (target - current).clamp_length_max(distance)
}
fn slide(mut movement: Vec3, normal: Vec3, walkable: f32) -> Vec3 {
    if normal.y > 0.0 && normal.y < walkable {
        // An unwalkable slope cannot turn horizontal intent into upward speed.
        let horizontal_normal = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
        let inward = movement.dot(horizontal_normal);
        if inward < 0.0 {
            movement -= horizontal_normal * inward;
        }
    }
    let inward = movement.dot(normal);
    if inward < 0.0 {
        movement -= normal * inward;
    }
    movement
}
fn depenetrate(
    position: &mut Vec3,
    height: f32,
    config: &KinematicConfig,
    queries: &mut Queries<'_>,
    report: &mut KinematicReport,
) -> Result<(), KinematicError> {
    let mut distance = 0.0;
    for _ in 0..config.max_depenetration_iterations {
        let Some(overlap) = queries.overlap(*position, height, config.radius, EPSILON)? else {
            return Ok(());
        };
        distance += overlap.depth + config.skin;
        if distance > config.max_depenetration_distance {
            return Err(KinematicError::DepenetrationLimit);
        }
        *position += overlap.normal * (overlap.depth + config.skin);
        report.depenetrations += 1;
    }
    if queries
        .overlap(*position, height, config.radius, EPSILON)?
        .is_some()
    {
        return Err(KinematicError::DepenetrationLimit);
    }
    Ok(())
}
fn snap(
    position: &mut Vec3,
    state: &mut KinematicState,
    height: f32,
    config: &KinematicConfig,
    scene: &CollisionScene,
    queries: &mut Queries<'_>,
) -> Result<(), KinematicError> {
    state.grounded = false;
    state.ground = None;
    // A near-contact query stabilizes support when GJK returns no cast at a
    // near-zero time of impact. Correction is bounded by the skin and slope.
    if let Some((hit, distance)) = queries.support_near(
        *position,
        height,
        config.radius,
        config.skin,
        config.walkable_normal_y,
    )? {
        *position += Vec3::Y * ((config.skin - distance) / hit.normal.y);
        state.grounded = true;
        state.ground = Some(scene.ground(hit, *position));
        return Ok(());
    }
    let down = -Vec3::Y * config.ground_snap;
    if let Some(hit) = queries.cast(*position, height, config.radius, down, config.skin)? {
        if hit.normal.y >= config.walkable_normal_y {
            *position += down * hit.fraction;
            state.grounded = true;
            state.ground = Some(scene.ground(hit, *position));
        }
    }
    Ok(())
}
fn try_step(
    position: Vec3,
    movement: Vec3,
    height: f32,
    config: &KinematicConfig,
    queries: &mut Queries<'_>,
) -> Result<Option<(Vec3, Hit)>, KinematicError> {
    let forward = Vec3::new(movement.x, 0.0, movement.z);
    if forward.length_squared() < EPSILON * EPSILON {
        return Ok(None);
    }
    let up = Vec3::Y * config.max_step_height;
    if queries
        .cast(position, height, config.radius, up, config.skin)?
        .is_some()
    {
        return Ok(None);
    }
    let elevated = position + up;
    if queries
        .cast(elevated, height, config.radius, forward, config.skin)?
        .is_some()
    {
        return Ok(None);
    }
    let forward_position = elevated + forward;
    let down = -Vec3::Y * (config.max_step_height + config.ground_snap);
    let Some(hit) = queries.cast(forward_position, height, config.radius, down, config.skin)?
    else {
        return Ok(None);
    };
    let landing = forward_position + down * hit.fraction;
    let rise = landing.y - position.y;
    if hit.normal.y < config.walkable_normal_y
        || hit.point.y - (position.y - config.skin)
            > config.max_step_height + config.skin * 0.1 + EPSILON
        || rise > config.max_step_height + config.skin * 0.1 + EPSILON
        || rise < -EPSILON
    {
        return Ok(None);
    }
    if queries
        .overlap(landing, height, config.radius, EPSILON)?
        .is_some()
    {
        return Ok(None);
    }
    Ok(Some((landing, hit)))
}
fn validate(
    config: &KinematicConfig,
    state: &KinematicState,
    input: KinematicInput,
    scene: &CollisionScene,
) -> Result<(), KinematicError> {
    let values = [
        config.fixed_dt,
        config.radius,
        config.standing_height,
        config.crouched_height,
        config.speed,
        config.crouched_speed,
        config.acceleration,
        config.braking,
        config.air_acceleration,
        config.gravity,
        config.jump_speed,
        config.terminal_speed,
        config.dash_speed,
        config.skin,
        config.ground_snap,
        config.max_step_height,
        config.walkable_normal_y,
        config.max_depenetration_distance,
    ];
    if values
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0 || *v > 10_000.0)
        || !(0.0001..=0.1).contains(&config.fixed_dt)
        || !(0.001..=100.0).contains(&config.radius)
        || config.crouched_height < config.radius * 2.0
        || config.standing_height < config.crouched_height
        || config.standing_height > 200.0
        || !(0.00001..=config.radius * 0.1).contains(&config.skin)
        || config.ground_snap < config.skin
        || config.ground_snap > config.radius
        || config.max_step_height > config.standing_height
        || config.walkable_normal_y < 0.1
        || config.walkable_normal_y > 1.0
        || !(1..=16).contains(&config.max_slide_iterations)
        || !(1..=16).contains(&config.max_depenetration_iterations)
        || !(1..=65_536).contains(&config.max_queries)
        || config.max_depenetration_distance > 100.0
        || config.dash_duration_ticks > 600
        || config.dash_cooldown_ticks > 3600
        || config.jump_buffer_ticks > 600
        || config.coyote_ticks > 600
    {
        return Err(KinematicError::InvalidConfig);
    }
    if !bounded(input.movement, 1.0) {
        return Err(KinematicError::InvalidInput);
    }
    if state.scene_revision != scene.revision() {
        return Err(KinematicError::SceneRevisionMismatch);
    }
    if !bounded(state.position, MAX_COORDINATE)
        || !bounded(state.velocity, 10_000.0)
        || !bounded(state.facing, 1.0)
        || !bounded(state.dash_direction, 1.0)
        || (Vec2::from_array(state.facing).length_squared() - 1.0).abs() > 0.001
        || (Vec2::from_array(state.dash_direction).length_squared() - 1.0).abs() > 0.001
        || state.grounded != state.ground.is_some()
        || [
            state.jump_buffer_ticks,
            state.coyote_ticks,
            state.dash_ticks,
            state.dash_cooldown_ticks,
            state.movement_lock_ticks,
            state.suppress_snap_ticks,
        ]
        .into_iter()
        .any(|v| v > 4096)
    {
        return Err(KinematicError::InvalidState);
    }
    if let Some(ground) = state.ground {
        if ground.scene_revision != scene.revision()
            || scene.collider(ground.collider).is_none()
            || !bounded(ground.local_position, MAX_COORDINATE * 2.0)
            || !bounded(ground.normal, 1.001)
            || (Vec3::from_array(ground.normal).length_squared() - 1.0).abs() > 0.001
            || ground.normal[1] < config.walkable_normal_y
        {
            return Err(KinematicError::InvalidState);
        }
    }
    Ok(())
}
