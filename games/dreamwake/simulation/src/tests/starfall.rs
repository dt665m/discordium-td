use crate::replication::{OwnerCombatAction, OwnerPredictionState, ReplicationStamp};
use crate::*;
fn key(sequence: u64) -> combat::RayActionKey {
    combat::RayActionKey {
        match_epoch: 7,
        connection_epoch: 3,
        command_stream: 2,
        ownership_epoch: 1,
        actor: 1,
        actor_generation: 1,
        command_sequence: sequence,
        action_slot: 0,
    }
}
fn checkpoint(sim: &DreamSimulation) -> replication::OwnerCheckpoint {
    sim.capture_replication(ReplicationStamp {
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
fn arena(essence: Option<EssenceKind>) -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut h in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        h.loadout.0[1].modifier = essence;
    }
    for mut e in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        e.motor.position = [15.0, 15.0];
        e.action.recovery = 100.0;
    }
    sim
}
#[test]
fn starfall_owner_and_authority_share_acceptance_flight_and_echo_restore() {
    for essence in [
        None,
        Some(EssenceKind::Twin),
        Some(EssenceKind::Echo),
        Some(EssenceKind::Vast),
    ] {
        let mut sim = arena(essence);
        let initial = checkpoint(&sim);
        let mut owner =
            OwnerPredictionState::from_checkpoint(&initial, initial.expectation()).unwrap();
        let collision = sim.world.resource::<collision::CollisionWorld>().clone();
        let bases = sim
            .snapshot()
            .state
            .platforms
            .iter()
            .map(|p| p.presentation(1))
            .collect::<Vec<_>>();
        let input = DreamInput {
            aim: [1.0, 0.0],
            ..Default::default()
        };
        for n in 0..40 {
            let action = (n == 0).then_some(OwnerCombatAction::Cast {
                key: key(1),
                slot: 1,
                aim: input.aim,
            });
            owner
                .step_restricted_combat_with_bases(input, &collision, &bases, action, None)
                .unwrap();
            let actions = if n == 0 {
                vec![combat::CombatAction::Cast {
                    key: key(1),
                    execution_server_tick: 101,
                    slot: 1,
                    aim: input.aim,
                }]
            } else {
                vec![]
            };
            sim.step_multiplayer_with_actions(&[(1, input)], &actions)
                .unwrap();
            let authoritative = checkpoint(&sim);
            assert_eq!(
                owner.hero_view().memories[1].cooldown,
                authoritative.hero_view().memories[1].cooldown
            );
            let mut predicted = owner.starfall_flights().to_vec();
            let mut actual = authoritative.starfall.flights.clone();
            for flight in &mut predicted {
                flight.authority_id = None;
            }
            for flight in &mut actual {
                flight.authority_id = None;
            }
            predicted.sort_by_key(|f| f.key.ordinal);
            actual.sort_by_key(|f| f.key.ordinal);
            assert_eq!(predicted, actual, "essence {essence:?}, step {n}");
            if n == 15 {
                owner
                    .try_restore(&authoritative, authoritative.expectation())
                    .unwrap();
            }
        }
    }
}
#[test]
fn starfall_capacity_reserves_echo_and_rejects_whole_twin_before_cooldown() {
    let mut memory = state::memory_slot(MemoryKind::Starfall);
    memory.modifier = Some(EssenceKind::Twin);
    let saved = memory;
    assert_eq!(
        starfall::accept(&mut memory, 6),
        Err(combat::ActionReason::Budget)
    );
    assert_eq!(memory, saved);
    memory.modifier = Some(EssenceKind::Echo);
    assert_eq!(
        starfall::accept(&mut memory, 7),
        Err(combat::ActionReason::Budget)
    );
    assert!(starfall::accept(&mut memory, 6).is_ok());
}
#[test]
fn starfall_owner_schema_is_bounded_and_excludes_target_hit_history() {
    let sim = arena(None);
    let mut owner = checkpoint(&sim);
    for n in 1..=8 {
        let mut flight = starfall::flight(
            starfall::SpawnKey {
                action: key(n),
                ordinal: 0,
            },
            0,
            [0.0, 0.0],
            [1.0, 0.0],
            Some(EssenceKind::Vast),
        );
        flight.authority_id = Some(u64::MAX - n);
        owner.starfall.flights.push(flight);
    }
    let encoded = owner.encode().unwrap();
    assert!(
        encoded.len() < 12 * 1024,
        "owner maximum active flight fixture uses {} bytes",
        encoded.len()
    );
    eprintln!("Starfall cap8 populated owner bytes: {}", encoded.len());
    let restored = replication::OwnerCheckpoint::decode(&encoded, owner.expectation()).unwrap();
    assert_eq!(restored, owner);
    let json = serde_json::to_string(&owner).unwrap();
    assert!(!json.contains("hit_ids"));
    assert!(!json.contains("damage"));
    owner
        .starfall
        .flights
        .push(owner.starfall.flights[0].clone());
    assert!(owner.encode().is_err());
}
#[test]
fn starfall_scope_fences_and_ordinals_produce_distinct_graphics() {
    let mut domain = starfall::StarfallDomain::default();
    starfall::spawn(
        &mut domain,
        key(1),
        1,
        [0.0, 0.0],
        [1.0, 0.0],
        Some(EssenceKind::Twin),
    );
    let ids = domain
        .flights
        .iter()
        .map(|f| f.graphics_instance().unwrap().id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 3);
    let mut next = domain.flights[0].clone();
    next.key.action.connection_epoch += 1;
    assert!(!ids.contains(&next.graphics_instance().unwrap().id));
}
#[test]
fn starfall_from_outside_base_closure_clears_every_rotating_support_pose() {
    let top = platform::PLATFORM_POSITION[1] + platform::PLATFORM_HALF_EXTENTS[1];
    for essence in [None, Some(EssenceKind::Vast)] {
        let mut domain = starfall::StarfallDomain::default();
        starfall::spawn(&mut domain, key(1), 0, [-15.0, -10.0], [1.0, 0.0], essence);
        assert!(
            0.9 - domain.flights[0].radius > top,
            "larger/lower Starfall requires platform prediction dependency"
        );
        for phase in [0, 128, 511, 1023] {
            let pose = platform::pose(phase);
            let mut collider = engine_core::StaticCollider::cuboid(
                engine_core::ColliderKey {
                    index: 77,
                    generation: 1,
                },
                pose.position,
                platform::PLATFORM_HALF_EXTENTS,
            );
            collider.rotation = pose.rotation;
            let scene = engine_core::CollisionScene::new(1, vec![collider]).unwrap();
            let empty = engine_core::CollisionScene::new(1, vec![]).unwrap();
            let mut without = domain.clone();
            let mut with = domain.clone();
            for _ in 0..130 {
                assert_eq!(
                    without.advance(&empty).unwrap(),
                    with.advance(&scene).unwrap()
                );
                assert_eq!(without, with);
            }
        }
    }
}
#[test]
fn echo_reserves_identity_before_spawn_and_cancels_on_departure() {
    let mut sim = arena(Some(EssenceKind::Echo));
    sim.step_multiplayer_with_actions(
        &[],
        &[combat::CombatAction::Cast {
            key: key(1),
            execution_server_tick: 1,
            slot: 1,
            aim: [1.0, 0.0],
        }],
    )
    .unwrap();
    let bindings = sim.starfall_bindings(key(1));
    assert_eq!(bindings.len(), 2);
    let future = bindings
        .iter()
        .find(|(ordinal, _)| *ordinal == 3)
        .unwrap()
        .1;
    let cp = checkpoint(&sim);
    assert_eq!(cp.starfall.repeats[0].authority_id, Some(future));
    let mut resumed = DreamSimulation::from_snapshot(&sim.snapshot());
    sim.set_player_active(1, false);
    assert!(sim.starfall_bindings(key(1)).is_empty());
    for _ in 0..33 {
        resumed.step(DreamInput::default());
    }
    assert!(resumed.starfall_bindings(key(1)).contains(&(3, future)));
}
