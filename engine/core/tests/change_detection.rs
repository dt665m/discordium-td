use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use engine_core::*;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Tick;
#[derive(Resource, Default, Debug, PartialEq)]
struct Changes([usize; 5]);

fn observe(
    actions: Query<Entity, Changed<ActionState>>,
    motors: Query<Entity, Changed<MotorState>>,
    combat: Query<Entity, Changed<CombatState>>,
    loadouts: Query<Entity, Changed<Loadout<u8, u8>>>,
    delays: Query<Entity, Changed<DelayedAction<u8>>>,
    mut changes: ResMut<Changes>,
) {
    changes.0 = [
        actions.iter().count(),
        motors.iter().count(),
        combat.iter().count(),
        loadouts.iter().count(),
        delays.iter().count(),
    ];
}

#[test]
fn timers_report_transitions_but_do_not_dirty_idle_components() {
    let mut app = App::new();
    app.init_schedule(Tick)
        .insert_resource(SimulationStep(0.25))
        .init_resource::<Changes>()
        .add_plugins((
            ActionPlugin(Tick),
            MotorPlugin(Tick),
            CombatEffectsPlugin(Tick),
            LoadoutPlugin::<u8, u8, _>::new(Tick),
            DelayedActionPlugin::<u8, _>::new(Tick),
        ))
        .add_systems(
            Tick,
            observe
                .after(ActionStep)
                .after(MotorStep)
                .after(CombatStep)
                .after(LoadoutStep)
                .after(DelayedActionStep),
        );
    let mut action = ActionState::default();
    action.commit([1.0, 0.0], 0.5);
    let mut combat = CombatState::default();
    combat.grant_shield(10.0, 0.5, ShieldPolicy::Replace, DurationPolicy::Reset);
    combat.apply_status(StatusId(1), 0.5, 0.5, DurationPolicy::Reset);
    combat.grant_invulnerability(0.5, DurationPolicy::Reset);
    let mut slot = AbilitySlot::new(1, 0.5);
    slot.activate();
    app.world_mut().spawn((
        action,
        MotorState {
            dash_remaining: 0.5,
            ..Default::default()
        },
        combat,
        Loadout::<u8, u8>(vec![slot]),
        DelayedAction::new(0.5, 1_u8),
    ));
    app.world_mut().run_schedule(Tick);
    assert_eq!(app.world().resource::<Changes>().0, [1; 5]);
    app.world_mut().run_schedule(Tick);
    assert_eq!(app.world().resource::<Changes>().0, [1; 5]);
    // The action is still due and the delay still ready: neither should emit
    // additional changes while a game waits to consume those states.
    app.world_mut().run_schedule(Tick);
    assert_eq!(app.world().resource::<Changes>().0, [0; 5]);
    let world = app.world_mut();
    let combat = world.query::<&CombatState>().single(world).unwrap();
    assert_eq!(combat.shield, 0.0);
    assert!(combat.statuses.is_empty());
}
