use crate::*;

fn arena(count: usize) -> DreamSimulation {
    let mut sim = super::combat::arena(count);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.action.due = false;
        enemy.action.recovery = 100.0;
        enemy.motor.position = [0.0, -4.0];
    }
    sim
}
fn bolt(sim: &mut DreamSimulation, friendly: bool, piercing: bool) {
    sim.world.resource_scope(|world, mut run: Mut<Run>| {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        systems::projectile(
            &mut Commands::new(&mut queue, world),
            &mut run,
            if friendly { 1 } else { 999 },
            [0.0, 0.0],
            [0.0, -1.0],
            20.0,
            friendly,
            piercing.then_some(EssenceKind::Vast),
            600.0,
            0.1,
        );
        queue.apply(world);
    });
}
fn wall(sim: &mut DreamSimulation, z: f32) {
    let original = sim
        .world
        .resource::<collision::CollisionWorld>()
        .manifest()
        .clone();
    let mut colliders = original.colliders().to_vec();
    colliders.push(engine_core::StaticCollider::cuboid(
        engine_core::ColliderKey {
            index: 1000,
            generation: 1,
        },
        [0.0, 1.0, z],
        [1.0, 1.0, 0.001],
    ));
    sim.world.insert_resource(
        collision::CollisionManifest::from_parts(1, original.config(), colliders)
            .unwrap()
            .build()
            .unwrap(),
    );
}
#[test]
fn fast_projectile_cannot_tunnel_through_thin_wall() {
    let mut sim = arena(1);
    wall(&mut sim, -2.0);
    bolt(&mut sim, true, false);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
    assert!(sim.snapshot().projectiles.is_empty());
}
#[test]
fn piercing_projectile_hits_before_wall_and_stops_before_targets_behind_it() {
    let mut sim = arena(2);
    let mut positions = [-1.5, -4.0].into_iter();
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [0.0, positions.next().unwrap()];
    }
    wall(&mut sim, -2.5);
    bolt(&mut sim, true, true);
    let snapshot = sim.snapshot();
    let mut restored = DreamSimulation::from_snapshot(&snapshot);
    sim.step(DreamInput::default());
    restored.step(DreamInput::default());
    assert_eq!(sim.snapshot(), restored.snapshot());
    let mut hp = sim
        .snapshot()
        .enemies
        .iter()
        .map(|enemy| enemy.hp)
        .collect::<Vec<_>>();
    hp.sort_by(f32::total_cmp);
    assert_eq!(hp, [180.0, 200.0]);
    assert!(sim.snapshot().projectiles.is_empty());
}
#[test]
fn first_projectile_contact_uses_surface_distance_not_center_distance() {
    let mut sim = arena(2);
    let mut index = 0;
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [0.0, if index == 0 { -4.0 } else { -4.4 }];
        if index == 1 {
            enemy.view.kind = EnemyKind::Boss;
        }
        index += 1;
    }
    bolt(&mut sim, true, false);
    sim.step(DreamInput::default());
    let snapshot = sim.snapshot();
    assert_eq!(
        snapshot
            .enemies
            .iter()
            .find(|enemy| enemy.kind == EnemyKind::Boss)
            .unwrap()
            .hp,
        180.0
    );
    assert_eq!(
        snapshot
            .enemies
            .iter()
            .find(|enemy| enemy.kind != EnemyKind::Boss)
            .unwrap()
            .hp,
        200.0
    );
}
#[test]
fn horizontal_projectile_does_not_hit_airborne_actor() {
    let mut sim = arena(1);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [0.0, 4.0, -3.0];
        hero.motion.grounded = false;
    }
    bolt(&mut sim, false, false);
    sim.step(DreamInput::default());
    assert_eq!(sim.snapshot().hero.hp, 100.0);
}
