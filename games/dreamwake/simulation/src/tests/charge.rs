use crate::*;
use engine_core::{ChargeCommand, ChargePhase};
fn arena() -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [15.0, 15.0];
        enemy.action.recovery = 100.0;
    }
    sim
}
fn input(command: ChargeCommand) -> DreamInput {
    DreamInput {
        charge: command,
        aim: [1.0, 0.0],
        ..Default::default()
    }
}
fn key(sequence: u64) -> combat::RayActionKey {
    combat::RayActionKey {
        match_epoch: 7,
        connection_epoch: 3,
        command_stream: 2,
        ownership_epoch: 1,
        actor: 1,
        actor_generation: 1,
        command_sequence: sequence,
        action_slot: 7,
    }
}
fn owner(sim: &DreamSimulation) -> replication::OwnerCheckpoint {
    sim.capture_replication(replication::ReplicationStamp {
        match_epoch: 7,
        server_tick: u64::from(sim.snapshot().tick) + 100,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: 1,
        revision: u64::from(sim.snapshot().tick) + 1,
    })
    .unwrap()
    .owner_checkpoint(1, 1)
    .unwrap()
}
#[test]
fn charge_restore_every_phase_matches_owner_and_authority() {
    let mut sim = arena();
    let initial = owner(&sim);
    let mut prediction =
        replication::OwnerPredictionState::from_checkpoint(&initial, initial.expectation())
            .unwrap();
    let collision = sim.world.resource::<collision::CollisionWorld>().clone();
    let bases = sim
        .snapshot()
        .state
        .platforms
        .iter()
        .map(|p| p.presentation(1))
        .collect::<Vec<_>>();
    let mut restored = DreamSimulation::from_snapshot(&sim.snapshot());
    for tick in 0..50 {
        let command = match tick {
            0 => ChargeCommand::Begin { episode: 1 },
            36 => ChargeCommand::Release { episode: 1 },
            _ => ChargeCommand::None,
        };
        let edge = command != ChargeCommand::None;
        let sequence = if tick == 0 { 1 } else { 2 };
        let action = edge.then_some(replication::OwnerCombatAction::Charge {
            key: key(sequence),
            command,
            aim: [1.0, 0.0],
        });
        prediction
            .step_restricted_combat_with_bases(
                input(ChargeCommand::None),
                &collision,
                &bases,
                action,
                None,
            )
            .unwrap();
        let actions = if edge {
            vec![combat::CombatAction::Charge {
                key: key(sequence),
                execution_server_tick: 100 + tick,
                command,
                aim: [1.0, 0.0],
            }]
        } else {
            vec![]
        };
        sim.step_multiplayer_with_actions(&[(1, input(ChargeCommand::None))], &actions)
            .unwrap();
        restored
            .step_multiplayer_with_actions(&[(1, input(ChargeCommand::None))], &actions)
            .unwrap();
        assert_eq!(
            sim.snapshot(),
            restored.snapshot(),
            "restored phase at tick {tick}"
        );
        let authoritative = owner(&sim);
        assert_eq!(
            prediction.hero_view().stamina,
            authoritative.hero_view().stamina,
            "tick {tick}"
        );
        assert_eq!(
            prediction.hero_view().position,
            authoritative.hero_view().position,
            "tick {tick}"
        );
        assert_eq!(
            prediction.hero_view().charge_ticks,
            authoritative.hero_view().charge_ticks
        );
        assert_eq!(
            prediction.hero_view().charge_executing,
            authoritative.hero_view().charge_executing
        );
        restored = DreamSimulation::from_snapshot(&sim.snapshot());
        prediction = replication::OwnerPredictionState::from_checkpoint(
            &authoritative,
            authoritative.expectation(),
        )
        .unwrap();
    }
    assert!(sim.snapshot().hero.position[0] > 7.0);
    assert!(sim.snapshot().hero.stamina < 72.0);
}
#[test]
fn charge_cancel_stun_pause_and_resource_cost_are_saved_policy() {
    let mut sim = arena();
    sim.step(input(ChargeCommand::Begin { episode: 1 }));
    sim.step(input(ChargeCommand::Release { episode: 1 }));
    assert_eq!(sim.snapshot().hero.stamina, 100.0);
    assert!(!sim.snapshot().hero.charge_executing);
    sim.step(input(ChargeCommand::Begin { episode: 2 }));
    for _ in 0..8 {
        sim.step(input(ChargeCommand::None));
    }
    let before = sim.snapshot();
    sim.world.resource_mut::<Run>().paused = true;
    for _ in 0..4 {
        sim.step(input(ChargeCommand::None));
    }
    assert_eq!(sim.snapshot().hero.charge_ticks, before.hero.charge_ticks);
    sim.world.resource_mut::<Run>().paused = false;
    sim.step(input(ChargeCommand::Cancel { episode: 1 }));
    assert!(sim.snapshot().hero.charge_ticks > 0);
    sim.step(input(ChargeCommand::Release { episode: 2 }));
    assert_eq!(sim.snapshot().hero.stamina, 70.0);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.combat
            .apply_status(STUN_STATUS, 1.0, 1.0, engine_core::DurationPolicy::Reset);
    }
    sim.step(input(ChargeCommand::None));
    assert!(!sim.snapshot().hero.charge_executing);
    assert_eq!(sim.snapshot().hero.stamina, 70.0);
    assert!(sim.snapshot().hero.charge_cooldown_ticks > 0);
    assert!(sim.set_player_active(1, false));
    assert!(sim.resume_player(1));
    assert_eq!(sim.snapshot().hero.stamina, 70.0);
}
#[test]
fn charged_motion_failure_does_not_spend_or_consume_curve() {
    let sim = arena();
    let mut charge = ChargedMovement::default();
    charge.state.episode = 1;
    charge.state.phase = ChargePhase::Charging {
        ticks: CHARGE_MAX_TICKS,
    };
    let mut motion = sim.snapshot().state.heroes[0].motion;
    motion.scene_revision += 1;
    let mut combat = engine_core::CombatState::default();
    let before = (charge.clone(), motion, combat.clone());
    let result = systems::advance_owner_charged_motion(
        &mut charge,
        None,
        &mut motion,
        &mut combat,
        7.8,
        input(ChargeCommand::Release { episode: 1 }),
        sim.world.resource::<platform::MotionEnvironment>(),
    );
    assert!(result.is_err());
    assert_eq!((charge, motion, combat), before);
}

#[test]
fn charge_stream_fence_and_low_stamina_reject_without_execution() {
    use combat::ActionReason;
    let mut charge = ChargedMovement::default();
    assert!(charge.stamina.try_spend(80.0));
    assert_eq!(
        charge.prepare(
            ChargeCommand::Begin { episode: 1 },
            Some(key(1)),
            [1.0, 0.0],
            false
        ),
        ActionReason::Resource
    );
    assert_eq!(charge.state.phase, ChargePhase::Idle);
    charge.stamina.restore(100.0);
    assert_eq!(
        charge.prepare(
            ChargeCommand::Begin { episode: 1 },
            Some(key(1)),
            [1.0, 0.0],
            false
        ),
        ActionReason::Accepted
    );
    for _ in 0..8 {
        charge.prepare(ChargeCommand::None, None, [1.0, 0.0], false);
    }
    let mut stale = key(2);
    stale.command_stream += 1;
    assert_eq!(
        charge.prepare(
            ChargeCommand::Release { episode: 1 },
            Some(stale),
            [1.0, 0.0],
            false
        ),
        ActionReason::InvalidAction
    );
    assert_eq!(charge.stamina.current(), 100.0);
    charge.state.interrupt();
    stale.command_sequence = 1;
    assert_eq!(
        charge.prepare(
            ChargeCommand::Begin { episode: 1 },
            Some(stale),
            [1.0, 0.0],
            false
        ),
        ActionReason::Accepted
    );
    assert_eq!(charge.begin_key, Some(stale));
}

#[test]
fn recovery_cancels_held_episode_and_cooldown_rejection_preserves_saved_fence() {
    let mut sim = arena();
    sim.step_multiplayer_with_actions(
        &[],
        &[combat::CombatAction::Charge {
            key: key(8),
            execution_server_tick: 1,
            command: ChargeCommand::Begin { episode: 8 },
            aim: [1.0, 0.0],
        }],
    )
    .unwrap();
    assert!(sim.interrupt_player_charge(1));
    assert!(sim.player_is_active(1));
    let saved = sim.snapshot();
    assert_eq!(saved.state.heroes[0].charge.state.phase, ChargePhase::Idle);
    assert_eq!(saved.hero.stamina, 100.0);
    let mut charge = saved.state.heroes[0].charge.clone();
    charge.state.cooldown_ticks = 20;
    let mut next = key(1);
    next.command_stream += 1;
    assert_eq!(
        charge.prepare(
            ChargeCommand::Begin { episode: 1 },
            Some(next),
            [1.0, 0.0],
            false
        ),
        combat::ActionReason::Cooldown
    );
    assert_eq!(charge.state.episode, 8);
    assert!(charge.valid(1, 1));
}

#[test]
fn blocked_surge_consumes_curve_once_and_immediate_dash_interrupts() {
    let mut sim = arena();
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [ARENA_RADIUS - 1.0, 0.01, 0.0];
        hero.charge.state.episode = 1;
        hero.charge.state.phase = ChargePhase::Charging { ticks: 36 };
    }
    sim.step(input(ChargeCommand::Release { episode: 1 }));
    for _ in 0..11 {
        sim.step(input(ChargeCommand::None));
    }
    let saved = sim.snapshot();
    assert_eq!(saved.state.heroes[0].charge.state.phase, ChargePhase::Idle);
    assert_eq!(saved.hero.stamina, 70.0);
    assert!(saved.hero.position[0] < ARENA_RADIUS);
    let mut sim = arena();
    sim.step(input(ChargeCommand::Begin { episode: 1 }));
    sim.step(DreamInput {
        dash: true,
        aim: [1.0, 0.0],
        ..Default::default()
    });
    let saved = sim.snapshot();
    assert_eq!(saved.state.heroes[0].charge.state.phase, ChargePhase::Idle);
    assert!(saved.hero.dashing);
    assert_eq!(saved.hero.stamina, 100.0);
}
