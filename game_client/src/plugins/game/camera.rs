//! Client-only camera rig. World movement is projected from this camera once, in input capture.
use super::*;
use game_shared::HERO_SPEED;

pub(super) struct GameCameraPlugin;

impl Plugin for GameCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FollowSettings>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                RunFixedMainLoop,
                camera_controls
                    .before(capture_input)
                    .in_set(ClientUpdateSet::Input),
            )
            .add_systems(
                Update,
                update_camera
                    .after(sync_dynamic_actors)
                    .before(publish_unit_bar_camera_updates)
                    .in_set(ClientUpdateSet::Visual),
            );
    }
}

#[derive(Component)]
pub(super) struct GameCamera {
    distance: f32,
    yaw: f32,
    follow: bool,
    focus: Vec3,
    motion: FollowMotion,
}

impl Default for GameCamera {
    fn default() -> Self {
        Self {
            distance: 42.0,
            yaw: 0.0,
            follow: true,
            focus: Vec3::ZERO,
            motion: FollowMotion::default(),
        }
    }
}

fn rig_transform(rig: &GameCamera) -> Transform {
    let rotation = Quat::from_rotation_y(rig.yaw);
    Transform::from_translation(rig.focus + rotation * Vec3::new(0.0, rig.distance, 0.01))
        .looking_at(rig.focus, rotation * Vec3::Z)
}

#[derive(Component)]
struct CameraHint;

fn camera_hint(follow: bool) -> String {
    format!(
        "Camera: {} | PgUp/Dn zoom | [ ] rotate | C follow | Home reset",
        if follow { "Follow" } else { "Arena" }
    )
}

fn spawn_camera(mut commands: Commands) {
    let rig = GameCamera::default();
    commands.spawn((Camera3d::default(), rig_transform(&rig), rig, UnitBarCamera));
    commands.spawn((
        CameraHint,
        Text::new(camera_hint(true)),
        TextFont {
            font_size: FontSize::Px(11.0),
            ..default()
        },
        TextColor(Color::srgba(0.9, 0.94, 1.0, 0.7)),
        Node {
            position_type: PositionType::Absolute,
            left: px(16),
            bottom: px(3),
            ..default()
        },
        bevy::ui::FocusPolicy::Pass,
    ));
}

fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut cameras: Query<(&mut GameCamera, &mut Transform)>,
    mut hints: Query<&mut Text, With<CameraHint>>,
) {
    for (mut rig, mut transform) in &mut cameras {
        if keys.just_pressed(KeyCode::Home) {
            *rig = GameCamera {
                focus: rig.focus,
                ..default()
            };
        }
        if keys.just_pressed(KeyCode::KeyC) {
            rig.follow = !rig.follow;
            rig.motion = FollowMotion::default();
        }
        if keys.just_pressed(KeyCode::Home) || keys.just_pressed(KeyCode::KeyC) {
            for mut hint in &mut hints {
                hint.0 = camera_hint(rig.follow);
            }
        }
        let zoom =
            f32::from(keys.pressed(KeyCode::PageDown)) - f32::from(keys.pressed(KeyCode::PageUp));
        rig.distance = (rig.distance * (zoom * time.delta_secs()).exp()).clamp(16.0, 64.0);
        let turn = f32::from(keys.pressed(KeyCode::BracketRight))
            - f32::from(keys.pressed(KeyCode::BracketLeft));
        rig.yaw = (rig.yaw + turn * time.delta_secs()).rem_euclid(std::f32::consts::TAU);
        *transform = rig_transform(&rig);
    }
}

/// Fractions are relative to the viewport, so portrait layouts and zoom stay safe.
#[derive(Resource)]
struct FollowSettings {
    dead_zone_fraction: Vec2,
    outer_fraction: f32,
    look_ahead_seconds: f32,
    max_look_ahead: f32,
    velocity_response: f32,
    look_ahead_response: f32,
    follow_response: f32,
    catch_up_response: f32,
    discontinuity_distance: f32,
}

impl Default for FollowSettings {
    fn default() -> Self {
        Self {
            dead_zone_fraction: Vec2::splat(0.14),
            outer_fraction: 0.65,
            look_ahead_seconds: 0.3,
            max_look_ahead: 3.0,
            velocity_response: 10.0,
            look_ahead_response: 5.0,
            follow_response: 7.0,
            catch_up_response: 22.0,
            discontinuity_distance: 20.0,
        }
    }
}

#[derive(Default)]
struct FollowMotion {
    previous: Option<(Entity, u32, Vec3)>,
    velocity: Vec3,
    look_ahead: Vec3,
}

fn response(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

fn follow_step(
    rig: &mut GameCamera,
    settings: &FollowSettings,
    target: Vec3,
    identity: (Entity, u32),
    view_half: Vec2,
    dt: f32,
) {
    if dt <= 0.0 {
        return;
    }
    let previous = rig.motion.previous;
    rig.motion.previous = Some((identity.0, identity.1, target));
    let Some((entity, epoch, position)) = previous else {
        rig.focus = target;
        return;
    };
    // Rejoins, round resets, teleports, and resuming a suspended tab must not
    // turn into a huge inferred velocity or a long flight across the map.
    if (entity, epoch) != identity
        || dt > 0.25
        || position.distance(target) > settings.discontinuity_distance
    {
        rig.focus = target;
        rig.motion.velocity = Vec3::ZERO;
        rig.motion.look_ahead = Vec3::ZERO;
        return;
    }
    rig.motion.velocity = rig.motion.velocity.lerp(
        (target - position) / dt,
        response(settings.velocity_response, dt),
    );
    let ahead = (rig.motion.velocity * settings.look_ahead_seconds)
        .clamp_length_max(settings.max_look_ahead.min(view_half.min_element() * 0.3));
    rig.motion.look_ahead = rig
        .motion
        .look_ahead
        .lerp(ahead, response(settings.look_ahead_response, dt));

    let camera = rig_transform(rig);
    let right = Vec3::new(camera.right().x, 0.0, camera.right().z).normalize();
    let up = Vec3::new(camera.up().x, 0.0, camera.up().z).normalize();
    let project = |v: Vec3| Vec2::new(v.dot(right), v.dot(up));
    let expand = |v: Vec2| right * v.x + up * v.y;
    let dead = view_half * settings.dead_zone_fraction;
    let error = project(target + rig.motion.look_ahead - rig.focus);
    let correction = error - error.clamp(-dead, dead);
    let urgency = (correction / view_half).length().clamp(0.0, 1.0);
    let speed = (rig.motion.velocity.length() / HERO_SPEED).clamp(0.0, 3.0);
    let rate = settings.follow_response + settings.catch_up_response * urgency + speed * 2.0;
    rig.focus += expand(correction) * response(rate, dt);

    // A hard outer boundary protects visibility during bursts even when the
    // soft follow is deliberately lagging. It moves only as much as necessary.
    let outer = view_half * settings.outer_fraction;
    let player_error = project(target - rig.focus);
    rig.focus += expand(player_error - player_error.clamp(-outer, outer));
}

fn update_camera(
    time: Res<Time>,
    settings: Res<FollowSettings>,
    local: Res<LocalSimulation>,
    heroes: Query<(Entity, &Transform), (With<LocalHeroActor>, Without<GameCamera>)>,
    mut cameras: Query<
        (&mut GameCamera, &mut Transform, &Camera, &Projection),
        Without<LocalHeroActor>,
    >,
) {
    for (mut rig, mut transform, camera, projection) in &mut cameras {
        if rig.follow {
            if let Ok((entity, hero)) = heroes.single() {
                let viewport = camera
                    .logical_viewport_size()
                    .unwrap_or(Vec2::new(1280.0, 720.0));
                let half_height = match projection {
                    Projection::Perspective(p) => rig.distance * (p.fov * 0.5).tan(),
                    Projection::Orthographic(p) => p.area.height() * 0.5,
                    _ => rig.distance * 0.414,
                };
                let view_half =
                    Vec2::new(half_height * viewport.x / viewport.y.max(1.0), half_height)
                        .max(Vec2::splat(0.1));
                follow_step(
                    &mut rig,
                    &settings,
                    Vec3::new(hero.translation.x, 0.0, hero.translation.z),
                    (entity, local.sim.sim_meta().match_epoch),
                    view_half,
                    time.delta_secs(),
                );
            } else {
                rig.motion = FollowMotion::default();
                rig.focus = rig.focus.lerp(Vec3::ZERO, response(8.0, time.delta_secs()));
            }
        } else {
            rig.focus = rig.focus.lerp(Vec3::ZERO, response(8.0, time.delta_secs()));
        }
        *transform = rig_transform(&rig);
    }
}

pub(super) fn movement_in_world(input: [f32; 2], camera: &Transform) -> [f32; 2] {
    let right = camera.right();
    let up = camera.up();
    let right = Vec2::new(right.x, right.z).normalize_or_zero();
    let up = Vec2::new(up.x, up.z).normalize_or_zero();
    let movement = (right * input[0] + up * input[1]).normalize_or_zero();
    movement.to_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_tracks_screen_axes_through_camera_rotation() {
        for yaw in [0.0, 0.7, 1.57, 3.14] {
            let camera = rig_transform(&GameCamera { yaw, ..default() });
            for input in [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
                let world = movement_in_world(input, &camera);
                let screen = camera.rotation.inverse() * Vec3::new(world[0], 0.0, world[1]);
                let expected = Vec2::from_array(input).normalize();
                assert!((screen.x - expected.x).abs() < 0.001);
                assert!((screen.y - expected.y).abs() < 0.001);
            }
        }
        assert_eq!(
            movement_in_world([0.0, 0.0], &rig_transform(&GameCamera::default())),
            [0.0, 0.0]
        );
    }
    fn step(rig: &mut GameCamera, pos: Vec3, dt: f32) {
        follow_step(
            rig,
            &FollowSettings::default(),
            pos,
            (Entity::PLACEHOLDER, 1),
            Vec2::new(16.0, 10.0),
            dt,
        );
    }

    #[test]
    fn small_adjustments_do_not_move_the_camera() {
        let mut rig = GameCamera::default();
        step(&mut rig, Vec3::ZERO, 1.0 / 60.0);
        for frame in 1..60 {
            step(&mut rig, Vec3::X * (frame as f32 * 0.008), 1.0 / 60.0);
        }
        assert!(rig.focus.length() < 0.0001);
    }

    #[test]
    fn travel_looks_ahead_without_frame_rate_dependent_drift() {
        let run = |fps: usize| {
            let mut rig = GameCamera::default();
            let dt = 1.0 / fps as f32;
            step(&mut rig, Vec3::ZERO, dt);
            for frame in 1..=fps * 3 {
                step(&mut rig, Vec3::X * (frame as f32 * dt * HERO_SPEED), dt);
            }
            assert!(rig.motion.look_ahead.x > 2.5);
            assert!(rig.focus.x > HERO_SPEED * 3.0 - 2.0);
            rig.focus
        };
        assert!(run(30).distance(run(120)) < 0.3);
    }

    #[test]
    fn burst_stays_inside_outer_boundary_on_rotated_portrait_view() {
        let mut rig = GameCamera {
            yaw: 0.8,
            ..default()
        };
        let settings = FollowSettings::default();
        let half = Vec2::new(4.0, 8.0);
        let id = (Entity::PLACEHOLDER, 1);
        follow_step(&mut rig, &settings, Vec3::ZERO, id, half, 0.016);
        let target = Vec3::new(8.0, 0.0, 5.0);
        follow_step(&mut rig, &settings, target, id, half, 0.016);
        let view = rig_transform(&rig);
        let error = target - rig.focus;
        assert!(error.dot(*view.right()).abs() <= half.x * settings.outer_fraction + 0.001);
        assert!(error.dot(*view.up()).abs() <= half.y * settings.outer_fraction + 0.001);
        assert!(
            rig.focus.distance(target) > 0.1,
            "a burst should not snap to center"
        );
    }

    #[test]
    fn reversals_are_smoothed_and_round_resets_clear_look_ahead() {
        let mut rig = GameCamera::default();
        step(&mut rig, Vec3::ZERO, 0.016);
        for i in 1..60 {
            step(&mut rig, Vec3::X * (i as f32 * 0.15), 0.016);
        }
        let before = rig.motion.look_ahead;
        step(&mut rig, Vec3::X * 8.7, 0.016);
        assert!(rig.motion.look_ahead.distance(before) < 0.2);
        follow_step(
            &mut rig,
            &FollowSettings::default(),
            Vec3::ZERO,
            (Entity::PLACEHOLDER, 2),
            Vec2::splat(10.0),
            0.016,
        );
        assert_eq!(rig.focus, Vec3::ZERO);
        assert_eq!(rig.motion.look_ahead, Vec3::ZERO);
        assert_eq!(rig.motion.velocity, Vec3::ZERO);
        step(&mut rig, Vec3::X * 40.0, 0.016);
        assert_eq!(rig.focus, Vec3::X * 40.0);
        assert_eq!(rig.motion.look_ahead, Vec3::ZERO);
    }
}
