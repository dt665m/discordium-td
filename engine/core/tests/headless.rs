use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use engine_core::*;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct Step;

// A moving service drone has no knowledge of any game's actor catalog or rules.
#[derive(Component)]
struct ServiceDrone {
    position: [f32; 2],
    velocity: [f32; 2],
    recharge: f32,
}
impl MovementOwner for ServiceDrone {
    fn integrate_motion(&mut self, dt: f32) {
        self.position = spatial::integrate(self.position, self.velocity, dt);
    }
}
impl CooldownOwner for ServiceDrone {
    fn tick_cooldowns(&mut self, dt: f32) {
        advance_cooldown(&mut self.recharge, dt);
    }
}

#[test]
fn unrelated_actor_reuses_headless_plugins_and_simulation_owned_lifetime() {
    let mut app = App::new();
    app.init_schedule(Step)
        .insert_resource(SimulationStep(0.25))
        .add_plugins((
            HealthPlugin(Step),
            PresentationPlugin(Step),
            MovementPlugin::<ServiceDrone, _>::new(Step),
            CooldownPlugin::<ServiceDrone, _>::new(Step),
        ));
    let actor = app
        .world_mut()
        .spawn((
            ServiceDrone {
                position: [1.0, 2.0],
                velocity: [4.0, -2.0],
                recharge: 0.3,
            },
            Health::new(25.0),
        ))
        .id();
    let id = PresentationId {
        match_epoch: 7,
        owner: 12,
        action_seq: 30,
        slot: 2,
    };
    let effect = app
        .world_mut()
        .spawn(PresentationInstance {
            id,
            kind: PresentationKind::RadialPulse,
            pos: [1.0, 2.0],
            radius: 2.0,
            age_ticks: 0,
            duration_ticks: 2,
        })
        .id();
    app.world_mut().run_schedule(Step);
    let drone = app.world().get::<ServiceDrone>(actor).unwrap();
    assert_eq!(drone.position, [2.0, 1.5]);
    assert!((drone.recharge - 0.05).abs() < 0.0001);
    assert_eq!(
        app.world().get::<PresentationInstance>(effect).unwrap().id,
        id
    );
    app.world_mut()
        .get_mut::<Health>(actor)
        .unwrap()
        .damage(40.0);
    app.world_mut().run_schedule(Step);
    assert!(app.world().get::<Depleted>(actor).is_some());
    assert!(app.world().get::<PresentationInstance>(effect).is_none());
    assert_eq!(
        app.world().get::<ServiceDrone>(actor).unwrap().recharge,
        0.0
    );
    app.world_mut().get_mut::<Health>(actor).unwrap().hp = 10.0;
    app.world_mut().run_schedule(Step);
    assert!(app.world().get::<Depleted>(actor).is_none());
}
