use crate::combat::*;
use crate::*;

fn crossing(cut_target: bool) -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [-2.0, -3.0];
        enemy.action.due = false;
        enemy.action.windup = 100.0;
    }
    sim.step(DreamInput::default());
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [2.0, -3.0];
        if cut_target {
            enemy.combat_identity.discontinuity(false).unwrap();
        }
    }
    sim.step(DreamInput::default());
    sim
}
fn ray(sequence: u64) -> ValidatedRay {
    ValidatedRay {
        key: RayActionKey {
            match_epoch: 1,
            connection_epoch: 9,
            command_stream: 1,
            ownership_epoch: 1,
            actor: 1,
            actor_generation: 1,
            command_sequence: sequence,
            action_slot: 0,
        },
        execution_server_tick: 3,
        query_server_tick: 1,
        query_gameplay_tick: 1,
        command_fraction: Some(32768),
        query_fraction: 32768,
        aim: [0.0, -1.0],
    }
}
fn movement() -> DreamInput {
    DreamInput {
        movement: [1.0, 0.0],
        aim: [0.0, -1.0],
        ..Default::default()
    }
}

#[test]
fn fractional_ray_reconstructs_shooter_and_target_without_extra_movement_and_replays() {
    let mut sim = crossing(false);
    let snapshot = sim.snapshot();
    let checkpoint = DreamCheckpoint::decode(
        &DreamCheckpoint::new(snapshot.clone(), 1)
            .unwrap()
            .encode()
            .unwrap(),
        1,
    )
    .unwrap();
    let mut replay = DreamSimulation::try_from_checkpoint(&checkpoint, 1).unwrap();
    let mut movement_only = DreamSimulation::try_from_checkpoint(&checkpoint, 1).unwrap();
    movement_only.step(movement());
    sim.set_combat_trace_enabled(true);
    let action = CombatAction::Ray(ray(1));
    let verdict = sim
        .step_multiplayer_with_actions(&[(1, movement())], &[action.clone()])
        .unwrap();
    let replayed = replay
        .step_multiplayer_with_actions(&[(1, movement())], &[action])
        .unwrap();
    assert_eq!(verdict, replayed);
    assert_eq!(sim.snapshot(), replay.snapshot());
    assert_eq!(
        sim.snapshot().state.heroes[0].motion,
        movement_only.snapshot().state.heroes[0].motion
    );
    assert_eq!(
        verdict[0].combat.as_ref().unwrap().reason,
        CombatReason::Hit
    );
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 5);
    assert_eq!(sim.snapshot().hero.dreamlance_cooldown, DREAMLANCE_COOLDOWN);
    let trace = sim.take_combat_traces().pop().unwrap();
    assert_eq!(trace.command_fraction, Some(32768));
    assert_eq!(trace.query_fraction, 32768);
    assert!((trace.muzzle.unwrap()[0] - sim.snapshot().hero.position[0] * 0.5).abs() < 0.000001);
    assert_eq!(trace.selected.unwrap().position[0], 0.0);
}

#[test]
fn missing_and_discontinuous_fractional_endpoints_reject_before_resource_commit() {
    for source_cut in [false, true] {
        let mut sim = crossing(!source_cut);
        if source_cut {
            for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
                hero.combat_identity.discontinuity(false).unwrap();
            }
        }
        let result = sim
            .step_multiplayer_with_actions(&[], &[CombatAction::Ray(ray(1))])
            .unwrap();
        assert_eq!(
            result[0].combat.as_ref().unwrap().reason,
            CombatReason::WrongLifecycle
        );
        assert_eq!(sim.snapshot().hero.dreamlance_ammo, 6);
        assert_eq!(sim.snapshot().hero.dreamlance_cooldown, 0.0);
    }
    let mut sim = super::combat::arena(1);
    let result = sim
        .step_multiplayer_with_actions(&[], &[CombatAction::Ray(ray(1))])
        .unwrap();
    assert_eq!(result[0].reason, ActionReason::MissingHistory);
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 6);
}

#[test]
fn duplicate_or_reordered_fractional_edges_cannot_add_fire_opportunities() {
    let mut a = crossing(false);
    let mut b = DreamSimulation::from_snapshot(&a.snapshot());
    let first = CombatAction::Ray(ray(1));
    let mut next = ray(2);
    next.command_fraction = Some(u16::MAX);
    let second = CombatAction::Ray(next);
    let one = a
        .step_multiplayer_with_actions(&[], &[first.clone(), second.clone()])
        .unwrap();
    let two = b
        .step_multiplayer_with_actions(&[], &[second, first])
        .unwrap();
    assert_eq!(one, two);
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(
        one.iter()
            .filter(|v| v.reason == ActionReason::Accepted)
            .count(),
        1
    );
    assert_eq!(a.snapshot().hero.dreamlance_ammo, 5);
    let mut sim = crossing(false);
    let first = ray(1);
    let mut conflicting = first;
    conflicting.command_fraction = Some(1);
    let results = sim
        .step_multiplayer_with_actions(
            &[],
            &[CombatAction::Ray(first), CombatAction::Ray(conflicting)],
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].reason, ActionReason::InvalidAction);
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 6);
}

#[test]
fn beam_fractional_view_survives_snapshot_but_recurring_origin_and_cadence_stay_fixed() {
    let mut sim = crossing(false);
    let result = sim
        .step_multiplayer_with_actions(&[(1, movement())], &[CombatAction::BeamBegin(ray(1))])
        .unwrap();
    assert_eq!(result[0].reason, ActionReason::Accepted);
    assert_eq!(
        sim.snapshot().state.heroes[0]
            .ray
            .beam
            .as_ref()
            .unwrap()
            .sample
            .query_fraction,
        32768
    );
    let mut replay = DreamSimulation::from_snapshot(&sim.snapshot());
    sim.set_combat_trace_enabled(true);
    for _ in 0..BEAM_DAMAGE_TICKS - 1 {
        sim.step(movement());
        replay.step(movement());
        assert!(sim.take_combat_traces().is_empty());
    }
    sim.step(movement());
    replay.step(movement());
    assert_eq!(sim.snapshot(), replay.snapshot());
    let traces = sim.take_combat_traces();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].command_fraction, None);
    assert_eq!(traces[0].query_fraction, 32768);
    assert_eq!(
        traces[0].muzzle.unwrap()[0],
        sim.snapshot().hero.position[0]
    );
    assert_eq!(sim.snapshot().hero.dreamlance_ammo, 5);
}
