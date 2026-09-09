//! A headless service-drone prototype built from reusable simulation plugins.
//! Run: cargo run -p engine_core --example sandbox
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use engine_core::*;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct Tick;

// Keep application-specific state together and expose only the operations needed
// by generic plugins. Position is canonical X/Z, unrelated to any camera.
#[derive(Component)]
struct Drone {
    position: [f32; 2],
    velocity: [f32; 2],
    scan_cooldown: f32,
}
impl MovementOwner for Drone {
    fn integrate_motion(&mut self, dt: f32) {
        self.position = spatial::integrate(self.position, self.velocity, dt);
    }
}
impl CooldownOwner for Drone {
    fn tick_cooldowns(&mut self, dt: f32) {
        advance_cooldown(&mut self.scan_cooldown, dt);
    }
}

fn main() {
    let mut app = App::new();
    app.init_schedule(Tick)
        .insert_resource(SimulationStep(0.25))
        .add_plugins((
            MovementPlugin::<Drone, _>::new(Tick),
            CooldownPlugin::<Drone, _>::new(Tick),
            HealthPlugin(Tick),
            PresentationPlugin(Tick),
        ));
    let drone = app
        .world_mut()
        .spawn((
            Drone {
                position: [0.0; 2],
                velocity: [4.0, -2.0],
                scan_cooldown: 0.5,
            },
            Health::new(30.0),
        ))
        .id();

    // The prototype owns acceptance and stable action identities; a renderer can
    // later consume this component without changing simulation or network code.
    let scan = app
        .world_mut()
        .spawn(PresentationInstance {
            id: PresentationId {
                match_epoch: 1,
                owner: 1,
                action_seq: 1,
                slot: 0,
            },
            kind: PresentationKind::RadialPulse,
            pos: [0.0; 2],
            radius: 3.0,
            age_ticks: 0,
            duration_ticks: 2,
        })
        .id();
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "travel");
    assert_eq!(
        app.world().get::<Drone>(drone).unwrap().position,
        [1.0, -0.5]
    );
    assert!(app.world().get::<PresentationInstance>(scan).is_some());

    app.world_mut()
        .get_mut::<Health>(drone)
        .unwrap()
        .damage(50.0);
    // Whether depletion stops motion is a prototype policy, so stop the vehicle
    // explicitly rather than coupling all movement to a health rule.
    app.world_mut().get_mut::<Drone>(drone).unwrap().velocity = [0.0; 2];
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "depleted");
    assert!(app.world().get::<Depleted>(drone).is_some());
    assert_eq!(app.world().get::<Drone>(drone).unwrap().scan_cooldown, 0.0);
    assert!(app.world().get::<PresentationInstance>(scan).is_none());
    println!("scan expired after two simulation ticks");

    // Repair/revival is explicit policy; normal healing never revives an actor.
    app.world_mut().get_mut::<Health>(drone).unwrap().hp = 15.0;
    app.world_mut().get_mut::<Drone>(drone).unwrap().velocity = [4.0, 0.0];
    app.world_mut().run_schedule(Tick);
    report(app.world(), drone, "repaired");
    assert!(app.world().get::<Depleted>(drone).is_none());
    assert_eq!(
        app.world().get::<Drone>(drone).unwrap().position,
        [2.0, -0.5]
    );
}

fn report(world: &World, entity: Entity, phase: &str) {
    let drone = world.get::<Drone>(entity).unwrap();
    let health = world.get::<Health>(entity).unwrap();
    println!(
        "{phase}: position={:?}, scan ready={}, hp={}/{}",
        drone.position,
        drone.scan_cooldown == 0.0,
        health.hp,
        health.max_hp
    );
}
