// Exercise the public standalone composition without maintaining a second harness.
#[allow(dead_code)]
#[path = "../examples/gameplay.rs"]
mod robot_arena;

#[test]
fn standalone_plugins_restore_and_replay_robot_arena() {
    robot_arena::demonstrate();
}

#[test]
fn inactive_owner_stops_owned_projectile_and_companion_firing() {
    use engine_core::{CollisionTarget, Health, ProjectileState, SummonState};
    let mut app = robot_arena::arena();
    robot_arena::populate(&mut app);
    let world = app.world_mut();
    for (target, mut health) in world
        .query::<(&CollisionTarget, &mut Health)>()
        .iter_mut(world)
    {
        if target.id == 1 {
            let remaining = health.hp;
            health.damage(remaining);
        }
    }
    robot_arena::tick(&mut app);
    let world = app.world_mut();
    let projectile = world.query::<&ProjectileState>().single(world).unwrap();
    assert!(projectile.expired);
    assert!(projectile.hit_ids.is_empty());
    let companion = world.query::<&SummonState>().single(world).unwrap();
    assert!(!companion.expired);
    assert_eq!(companion.fire_remaining, 0.0);
    assert_eq!(companion.remaining, 0.75);
}
