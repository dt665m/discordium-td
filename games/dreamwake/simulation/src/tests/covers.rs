use crate::combat::*;
use crate::*;
fn arena(open: bool) -> DreamSimulation {
    let mut sim = super::combat::arena(1);
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [0.0, -9.0];
        enemy.action.due = false;
        enemy.action.recovery = 100.0;
    }
    for mut cover in sim.world.query::<&mut Cover>().iter_mut(&mut sim.world) {
        cover.open = open;
        cover.next_transition_tick = COVER_PERIOD_TICKS;
    }
    sim
}
fn step_to(sim: &mut DreamSimulation, tick: u32) {
    while sim.world.resource::<Run>().tick < tick {
        sim.step(DreamInput::default());
    }
}
fn ray(execution_tick: u32, query_tick: u32, query_fraction: u16) -> CombatAction {
    CombatAction::Ray(ValidatedRay {
        command_fraction: None,
        query_fraction,
        key: RayActionKey {
            match_epoch: 1,
            connection_epoch: 9,
            command_stream: 1,
            ownership_epoch: 1,
            actor: 1,
            actor_generation: 1,
            command_sequence: 1,
            action_slot: 0,
        },
        execution_server_tick: u64::from(execution_tick),
        query_server_tick: u64::from(query_tick),
        query_gameplay_tick: query_tick,
        aim: [0.0, -1.0],
    })
}
#[test]
fn shutter_blocking_at_reference_blocks_after_current_geometry_clears() {
    let mut sim = arena(false);
    step_to(&mut sim, 109);
    let result = sim
        .step_multiplayer_with_actions(&[], &[ray(110, 101, 0)])
        .unwrap();
    let snapshot = sim.snapshot();
    assert!(!snapshot.covers[0].open);
    assert!(snapshot.covers[0].position[1] - COVER_HALF_EXTENTS[1] > 0.9);
    let reference = snapshot
        .state
        .combat_history
        .frames
        .iter()
        .find(|frame| frame.tick == 101)
        .unwrap()
        .poses
        .iter()
        .find(|pose| pose.id == snapshot.covers[0].id)
        .unwrap();
    assert!(reference.position[1] - COVER_HALF_EXTENTS[1] < 0.9);
    assert_eq!(
        result[0].combat.as_ref().unwrap().reason,
        CombatReason::DynamicBlocker
    );
    assert_eq!(sim.snapshot().enemies[0].hp, 200.0);
}
#[test]
fn shutter_clear_at_reference_does_not_block_after_current_geometry_closes() {
    let mut sim = arena(true);
    step_to(&mut sim, 108);
    let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).unwrap();
    let mut restored = DreamSimulation::try_from_checkpoint(
        &DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap(),
        1,
    )
    .unwrap();
    step_to(&mut sim, 116);
    step_to(&mut restored, 116);
    let a = sim
        .step_multiplayer_with_actions(&[], &[ray(117, 108, 0)])
        .unwrap();
    let b = restored
        .step_multiplayer_with_actions(&[], &[ray(117, 108, 0)])
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(sim.snapshot(), restored.snapshot());
    assert!(sim.snapshot().covers[0].open);
    assert!(sim.snapshot().covers[0].position[1] - COVER_HALF_EXTENTS[1] < 0.9);
    assert_eq!(a[0].combat.as_ref().unwrap().reason, CombatReason::Hit);
    assert_eq!(sim.snapshot().enemies[0].hp, 168.0);
}

#[test]
fn shutter_travels_in_equal_steps_between_ninety_tick_rests() {
    let mut sim = arena(false);
    let mut previous_height = sim.snapshot().covers[0].position[1];
    for tick in 1..=240 {
        sim.step(DreamInput::default());
        let snapshot = sim.snapshot();
        let cover = snapshot.covers[0];
        let expected_height = match tick {
            1..=90 => 1.0,
            91..=120 => 1.0 + (tick - 90) as f32 * 0.08,
            121..=210 => 3.4,
            _ => 3.4 - (tick - 210) as f32 * 0.08,
        };
        assert!((cover.position[1] - expected_height).abs() < 0.000001);
        assert!((cover.position[1] - previous_height).abs() <= 0.080001);
        assert_eq!([cover.position[0], cover.position[2]], [0.0, -6.0]);
        assert_eq!(cover.open, (120..240).contains(&tick));
        assert_eq!(cover.revision, 1);
        assert!(cover.valid());
        let pose = snapshot
            .state
            .combat_history
            .frames
            .last()
            .unwrap()
            .poses
            .iter()
            .find(|pose| pose.id == cover.id)
            .unwrap();
        assert_eq!(pose.position, cover.position);
        assert_eq!(pose.segment, cover.revision);
        previous_height = cover.position[1];
    }
}

#[test]
fn midtravel_checkpoint_restores_both_directions_through_the_phase_endpoint() {
    for open in [false, true] {
        let mut sim = arena(open);
        step_to(&mut sim, 105);
        assert!((sim.snapshot().covers[0].position[1] - 2.2).abs() < 0.000001);
        let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).unwrap();
        let mut restored = DreamSimulation::try_from_checkpoint(
            &DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap(),
            1,
        )
        .unwrap();
        assert_eq!(sim.snapshot(), restored.snapshot());
        for _ in 106..=121 {
            sim.step(DreamInput::default());
            restored.step(DreamInput::default());
            assert_eq!(sim.snapshot(), restored.snapshot());
        }
        let cover = sim.snapshot().covers[0];
        assert_eq!(cover.open, !open);
        assert_eq!(cover.position[1], if open { 1.0 } else { 3.4 });
        assert_eq!(cover.revision, 1);
    }
}

#[test]
fn fractional_history_interpolates_across_both_phase_endpoints() {
    use crate::combat_history::CombatHistory;
    use engine_core::{ColliderKey, QueryBudget};
    for (open, expected) in [
        (false, CombatReason::Hit),
        (true, CombatReason::DynamicBlocker),
    ] {
        let mut sim = arena(open);
        step_to(&mut sim, 120);
        let snapshot = sim.snapshot();
        let saved = &snapshot.state.covers[0];
        let history = sim.world.resource::<CombatHistory>();
        let scene = history.archive.frames.last().unwrap().scene;
        let pose = history
            .prepared
            .sample_pose(
                119,
                32768,
                scene,
                ColliderKey {
                    index: saved.id,
                    generation: saved.identity.generation,
                },
                saved.identity.segment,
                &mut QueryBudget::new(2, 1).unwrap(),
            )
            .unwrap();
        assert!((pose.position.y - if open { 1.04 } else { 3.36 }).abs() < 0.000001);
        let result = sim
            .step_multiplayer_with_actions(&[], &[ray(121, 119, 32768)])
            .unwrap();
        assert_eq!(result[0].combat.as_ref().unwrap().reason, expected);
    }
}
#[test]
fn dynamic_cover_uses_current_swept_geometry_for_projectiles() {
    use bevy::ecs::world::CommandQueue;
    for (open, before_tick, expected) in [
        (false, 0, 200.0),
        (true, 0, 180.0),
        (false, 100, 200.0),
        (false, 109, 180.0),
        (true, 106, 180.0),
        (true, 107, 200.0),
        (true, 116, 200.0),
    ] {
        let mut sim = arena(open);
        step_to(&mut sim, before_tick);
        sim.world.resource_scope(|world, mut run: Mut<Run>| {
            let mut queue = CommandQueue::default();
            systems::projectile(
                &mut Commands::new(&mut queue, world),
                &mut run,
                1,
                [0.0; 2],
                [0.0, -1.0],
                20.0,
                true,
                None,
                600.0,
                0.1,
            );
            queue.apply(world);
        });
        sim.step(DreamInput::default());
        assert_eq!(
            sim.snapshot().enemies[0].hp,
            expected,
            "phase open={open}, impact tick={}",
            before_tick + 1
        );
    }
}
#[test]
fn moving_cover_replication_roundtrips_the_same_geometry_as_combat_history() {
    use crate::replication::*;
    let mut sim = arena(false);
    step_to(&mut sim, 105);
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: 105,
        gameplay_tick: 105,
        scene_revision: 1,
        revision: 1,
    };
    let capture = sim.capture_replication(stamp).unwrap();
    let id = sim.snapshot().covers[0].id;
    assert!(capture.keys().contains(&ReplicationKey::Cover(id)));
    let cover = capture.project_cover(id).unwrap();
    assert!(cover.valid());
    assert!((cover.position[1] - 2.2).abs() < 0.000001);
    assert_eq!(
        PublicReplica::decode(&PublicReplica::Cover(cover).encode().unwrap()).unwrap(),
        PublicReplica::Cover(cover)
    );
    assert_eq!(
        PublicReplica::decode_compact(&PublicReplica::Cover(cover).encode_compact().unwrap())
            .unwrap(),
        PublicReplica::Cover(cover)
    );
    assert_eq!(
        sim.snapshot()
            .state
            .combat_history
            .frames
            .last()
            .unwrap()
            .poses
            .iter()
            .find(|pose| pose.id == id)
            .unwrap()
            .position,
        cover.position
    );
    for height in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.99, 3.41] {
        let mut invalid = cover;
        invalid.position[1] = height;
        assert!(!invalid.valid());
    }
    let data = serde_json::to_value(cover).unwrap();
    assert!(data.get("next_transition_tick").is_none());
    assert!(data.get("identity").is_none());
    let cp = capture.owner_checkpoint(1, 1).unwrap();
    let owner = OwnerPredictionState::from_checkpoint(&cp, cp.expectation()).unwrap();
    let view =
        DreamPresentation::from_replicas(capture.global(), &owner, &[PublicReplica::Cover(cover)])
            .unwrap();
    assert_eq!(view.covers, sim.snapshot().covers);
}

#[test]
fn removed_cover_restores_current_region_absence_and_no_combat_blocker() {
    use crate::replication::*;
    let mut sim = arena(false);
    let cover_id = sim.snapshot().covers[0].id;
    let original_segment = sim.snapshot().state.covers[0].identity.segment;
    assert!(sim.remove_cover(cover_id).unwrap());
    assert!(!sim.remove_cover(cover_id).unwrap());
    sim.step(DreamInput::default());
    assert!(sim.snapshot().covers.is_empty());
    assert!(!sim.snapshot().state.covers[0].present);
    assert_eq!(
        sim.snapshot().state.covers[0].identity.segment,
        original_segment + 1
    );
    assert!(
        sim.snapshot()
            .state
            .combat_history
            .frames
            .last()
            .unwrap()
            .poses
            .iter()
            .all(|pose| pose.id != cover_id)
    );
    let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).unwrap();
    let restored = DreamSimulation::try_from_checkpoint(
        &DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap(),
        1,
    )
    .unwrap();
    assert_eq!(sim.snapshot(), restored.snapshot());
    let capture = restored
        .capture_replication(ReplicationStamp {
            match_epoch: 1,
            server_tick: 2,
            gameplay_tick: 1,
            scene_revision: 1,
            revision: 2,
        })
        .unwrap();
    assert!(capture.keys().contains(&ReplicationKey::Cover(cover_id)));
    assert!(
        !capture
            .keys()
            .iter()
            .any(|key| matches!(key, ReplicationKey::CoverMarker(_)))
    );
    let cover = capture.project_cover(cover_id).unwrap();
    assert!(!cover.present);
    let wire = PublicReplica::Cover(cover).encode().unwrap();
    let decoded = PublicReplica::decode(&wire).unwrap();
    let owner = capture.owner_checkpoint(1, 1).unwrap();
    let owner = OwnerPredictionState::from_checkpoint(&owner, owner.expectation()).unwrap();
    let shown = DreamPresentation::from_replicas(capture.global(), &owner, &[decoded]).unwrap();
    assert!(shown.covers.is_empty());
    assert_eq!(
        cover
            .region_manifest(capture.global().stamp)
            .unwrap()
            .presence(1, 1),
        engine_net::replication::regions::MapObjectPresence::Absent
    );
}

#[test]
fn distributed_demo_props_capture_roundtrip_and_preserve_individual_absence() {
    use crate::replication::*;
    use std::collections::BTreeSet;
    let mut sim = DreamSimulation::new(73, false);
    sim.continue_run();
    for _ in 0..COMBAT_HISTORY_TICKS {
        sim.step(DreamInput::default());
    }
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.covers.len(), DEMO_COVER_POSITIONS.len());
    assert_eq!(snapshot.covers[0].position, [0.0, 1.0, -6.0]);
    let cells = snapshot
        .covers
        .iter()
        .map(|cover| {
            (
                (cover.position[0] / 32.0).floor() as i32,
                (cover.position[2] / 32.0).floor() as i32,
            )
        })
        .collect::<BTreeSet<_>>();
    assert!(cells.len() >= 40);
    assert!(
        snapshot
            .covers
            .iter()
            .any(|cover| cover.position[0] == -96.0)
    );
    assert!(
        snapshot
            .covers
            .iter()
            .any(|cover| cover.position[0] == 96.0)
    );
    assert!(
        snapshot
            .state
            .combat_history
            .frames
            .iter()
            .all(|frame| frame.poses.len() < crate::combat_history::MAX_COMBAT_POSES)
    );
    let capture = sim
        .capture_replication(ReplicationStamp {
            match_epoch: 1,
            server_tick: 40,
            gameplay_tick: snapshot.tick,
            scene_revision: 1,
            revision: 40,
        })
        .unwrap();
    for saved in &snapshot.state.covers {
        let key = ReplicationKey::Cover(saved.id);
        assert!(capture.keys().contains(&key));
        let cover = capture.project_cover(saved.id).unwrap();
        assert_eq!(
            PublicReplica::decode(&PublicReplica::Cover(cover).encode().unwrap()).unwrap(),
            PublicReplica::Cover(cover)
        );
        assert_eq!(capture.marker_parent(saved.marker_id), Some(saved.id));
        assert_eq!(capture.position(key), Some(saved.ground_position));
    }
    let checkpoint = DreamCheckpoint::new(snapshot.clone(), 1).unwrap();
    let decoded = DreamCheckpoint::decode(&checkpoint.encode().unwrap(), 1).unwrap();
    let mut restored = DreamSimulation::try_from_checkpoint(&decoded, 1).unwrap();
    assert_eq!(restored.snapshot(), snapshot);
    let outer = snapshot.state.covers.last().unwrap();
    assert!(restored.remove_cover(outer.id).unwrap());
    restored.step(DreamInput::default());
    let after = restored.snapshot();
    assert_eq!(after.covers.len(), DEMO_COVER_POSITIONS.len() - 1);
    assert!(
        after
            .covers
            .iter()
            .any(|cover| cover.id == snapshot.covers[0].id)
    );
    assert!(
        !after
            .state
            .covers
            .iter()
            .find(|cover| cover.id == outer.id)
            .unwrap()
            .present
    );
    let after_capture = restored
        .capture_replication(ReplicationStamp {
            match_epoch: 1,
            server_tick: 41,
            gameplay_tick: after.tick,
            scene_revision: 1,
            revision: 41,
        })
        .unwrap();
    assert!(!after_capture.project_cover(outer.id).unwrap().present);
    assert_eq!(after_capture.marker_parent(outer.marker_id), None);
    let roundtrip = DreamCheckpoint::decode(
        &DreamCheckpoint::new(after.clone(), 1)
            .unwrap()
            .encode()
            .unwrap(),
        1,
    )
    .unwrap();
    assert_eq!(roundtrip.snapshot, after);
    let mut invalid = after;
    invalid.state.covers[0].ground_position = [ARENA_RADIUS + 1.0, 0.0];
    assert!(DreamCheckpoint::new(invalid, 1).is_err());
}
