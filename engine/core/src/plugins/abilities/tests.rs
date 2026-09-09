use crate::SimulationStep;

use super::*;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct TestTick;
#[test]
fn action_plugins_advance_component_state_on_selected_schedule() {
    let mut app = App::new();
    app.init_schedule(TestTick)
        .insert_resource(SimulationStep(0.25))
        .add_plugins((
            ActionPlugin(TestTick),
            DelayedActionPlugin::<u64, _>::new(TestTick),
        ));
    let mut action = ActionState::default();
    action.commit([2.0, 3.0], 0.25);
    let actor = app.world_mut().spawn(action).id();
    let delayed = app.world_mut().spawn(DelayedAction::new(0.5, 42_u64)).id();
    app.world_mut().run_schedule(TestTick);
    assert!(app.world().get::<ActionState>(actor).unwrap().due);
    assert!(
        !app.world()
            .get::<DelayedAction<u64>>(delayed)
            .unwrap()
            .ready
    );
    app.world_mut().run_schedule(TestTick);
    assert!(
        app.world()
            .get::<DelayedAction<u64>>(delayed)
            .unwrap()
            .ready
    );
}
#[test]
fn committed_action_is_consumed_once_and_recovers() {
    let mut action = ActionState::default();
    assert!(action.commit([1.0, 2.0], 0.5));
    assert!(!action.commit([9.0; 2], 0.0));
    action.tick(0.5);
    assert_eq!(action.target, [1.0, 2.0]);
    assert!(action.take_due());
    assert!(!action.take_due());
    action.begin_recovery(0.25);
    assert!(!action.idle());
    action.tick(0.25);
    assert!(action.commit([0.0; 2], 0.0));
    assert!(action.take_due());
    action.cancel();
    assert!(action.idle());
    assert_eq!(action.sequence, 2);
}
#[test]
fn delayed_continuation_preserves_payload_and_readiness() {
    let mut original = DelayedAction::new(0.5, (17_u64, [1.0, 2.0]));
    original.tick(0.25);
    let saved = serde_json::to_string(&original).unwrap();
    let mut restored: DelayedAction<(u64, [f32; 2])> = serde_json::from_str(&saved).unwrap();
    original.tick(0.25);
    restored.tick(0.25);
    assert_eq!(original, restored);
    assert!(restored.ready);
    restored.tick(0.25);
    assert!(restored.ready);
    assert_eq!(restored.payload.0, 17);
}

#[test]
fn slots_activate_independently_and_preserve_selected_socket_policy() {
    let mut slot = AbilitySlot::new(1_u8, 2.0);
    slot.modifier = Some(9_u8);
    assert!(slot.activate());
    assert!(!slot.activate());
    let mut loadout = Loadout(vec![slot]);
    loadout.tick(1.0);
    assert_eq!(loadout.0[0].cooldown, 1.0);
    loadout.0[0].replace(2, 1, true);
    assert_eq!(loadout.0[0].modifier, Some(9));
    assert!(loadout.0[0].upgrade(2));
    assert!(!loadout.0[0].upgrade(2));
    assert!(!loadout.swap(0, 1));
    assert!(loadout.0[0].ready());
}
