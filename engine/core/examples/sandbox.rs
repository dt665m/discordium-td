//! A non-networked service drone using Bevy's Transform and Timer directly.
//! Run: cargo run -p engine_core --example sandbox
use bevy::{
    ecs::schedule::{LogLevel, ScheduleBuildSettings, ScheduleLabel},
    prelude::*,
};
use engine_core::*;
use std::time::Duration;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct Tick;
#[derive(Component)]
struct Drone;
#[derive(Component)]
struct Velocity(Vec3);
#[derive(Component)]
struct Scan(Timer);

fn move_drones(
    step: Res<SimulationStep>,
    mut drones: Query<(&mut Transform, &Velocity), With<Drone>>,
) {
    for (mut transform, velocity) in &mut drones {
        transform.translation += velocity.0 * step.0;
    }
}
fn recharge_scans(step: Res<SimulationStep>, mut scans: Query<&mut Scan, With<Drone>>) {
    let duration = Duration::from_secs_f32(step.0);
    for mut scan in &mut scans {
        if !scan.0.is_finished() {
            scan.0.tick(duration);
        }
    }
}

pub fn demonstrate() {
    let mut app = App::new();
    app.init_schedule(Tick)
        .edit_schedule(Tick, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..Default::default()
            });
        })
        .add_plugins(SystemPlugin::new(0.25))
        .add_plugins((HealthPlugin(Tick), GraphicsPlugin(Tick)))
        // Movement and recharge touch independent components and need no ordering.
        .add_systems(Tick, (move_drones, recharge_scans));
    let drone = app
        .world_mut()
        .spawn((
            Drone,
            Transform::default(),
            Velocity(Vec3::new(4.0, 0.0, -2.0)),
            Scan(Timer::from_seconds(0.5, TimerMode::Once)),
            Health::new(30.0),
        ))
        .id();

    // Stable presentation identity and its lifetime remain simulation-owned.
    let id = GraphicsId {
        match_epoch: 1,
        owner: 1,
        action_seq: 1,
        slot: 0,
    };
    let scan = app
        .world_mut()
        .spawn(GraphicsInstance {
            id,
            kind: GraphicsKind::RadialPulse,
            pos: [0.0; 2],
            radius: 3.0,
            age_ticks: 0,
            duration_ticks: 2,
        })
        .id();
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "travel");
    assert_eq!(
        app.world().get::<Transform>(drone).unwrap().translation,
        Vec3::new(1.0, 0.0, -0.5)
    );
    assert_eq!(
        app.world().get::<Scan>(drone).unwrap().0.remaining_secs(),
        0.25
    );
    assert_eq!(app.world().get::<GraphicsInstance>(scan).unwrap().id, id);
    assert_eq!(
        app.world().get::<GraphicsInstance>(scan).unwrap().age_ticks,
        1
    );

    // Damage occurs before the schedule; HealthPlugin observes it this step.
    app.world_mut()
        .get_mut::<Health>(drone)
        .unwrap()
        .damage(50.0);
    // Stopping a depleted vehicle is the prototype's explicit policy.
    app.world_mut().get_mut::<Velocity>(drone).unwrap().0 = Vec3::ZERO;
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "depleted");
    assert!(app.world().get::<Depleted>(drone).is_some());
    assert!(app.world().get::<Scan>(drone).unwrap().0.is_finished());
    assert_eq!(
        app.world().get::<Scan>(drone).unwrap().0.remaining_secs(),
        0.0
    );
    assert!(app.world().get::<GraphicsInstance>(scan).is_none());
    assert_eq!(
        app.world().get::<Transform>(drone).unwrap().translation,
        Vec3::new(1.0, 0.0, -0.5)
    );

    // Repair/revival is explicit policy; normal healing never revives an actor.
    app.world_mut().get_mut::<Health>(drone).unwrap().hp = 15.0;
    app.world_mut().get_mut::<Velocity>(drone).unwrap().0 = Vec3::new(4.0, 0.0, 0.0);
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "repaired");
    assert!(app.world().get::<Depleted>(drone).is_none());
    assert_eq!(
        app.world().get::<Transform>(drone).unwrap().translation,
        Vec3::new(2.0, 0.0, -0.5)
    );
}

fn report(world: &World, entity: Entity, phase: &str) {
    let transform = world.get::<Transform>(entity).unwrap();
    let scan = world.get::<Scan>(entity).unwrap();
    let health = world.get::<Health>(entity).unwrap();
    println!(
        "{phase}: position={:?}, scan ready={}, hp={}/{}",
        transform.translation,
        scan.0.is_finished(),
        health.hp,
        health.max_hp
    );
}
fn main() {
    demonstrate();
}
