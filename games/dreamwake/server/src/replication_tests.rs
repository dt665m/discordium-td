use super::*;
use dreamwake_sim::replication::ReplicationStamp;
use dreamwake_sim::{DreamInput, DreamSimulation};

fn capture(sim: &DreamSimulation, revision: u64) -> CommittedReplication {
    sim.capture_replication(ReplicationStamp {
        match_epoch: 7,
        server_tick: revision,
        gameplay_tick: sim.snapshot().tick,
        scene_revision: 1,
        revision,
    })
    .unwrap()
}
fn adapter() -> DreamReplication {
    DreamReplication::new(
        7,
        12.0,
        Limits {
            prefetch: 0.0,
            leave_margin: 0.0,
            ..Limits::default()
        },
    )
    .unwrap()
}
fn party() -> DreamSimulation {
    let mut sim = DreamSimulation::new(42, false);
    assert!(sim.add_player(2));
    sim
}
fn join(rep: &mut DreamReplication, id: u64) {
    rep.set_peer(id, ConnectionEpoch(id), SceneRevision(1))
        .unwrap();
}
fn gather(rep: &mut DreamReplication, id: u64) -> PeerGather {
    let tick = ServerTick(rep.capture.as_ref().unwrap().global().stamp.server_tick);
    rep.gather_peer(id, tick, &Default::default()).unwrap()
}
fn find(gather: &PeerGather, entity: EntityId) -> Option<&Replica> {
    gather
        .entities
        .iter()
        .find(|entry| entry.entity == entity)
        .map(|entry| &entry.replica)
}
#[test]
fn owner_uses_private_checkpoint_without_duplicate_public_hero() {
    let sim = party();
    let mut rep = adapter();
    join(&mut rep, 1);
    join(&mut rep, 2);
    rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
    let one = rep.actor_entity(ReplicationKey::Hero(1)).unwrap();
    let two = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
    let first = gather(&mut rep, 1);
    let second = gather(&mut rep, 2);
    assert!(matches!(
        find(&first, rep.owner_entity(1).unwrap()),
        Some(Replica::Owner(owner)) if owner.owner() == 1
    ));
    assert!(matches!(
        find(&first, rep.global_entity().unwrap()),
        Some(Replica::Global(_))
    ));
    assert!(
        find(&first, one).is_none(),
        "no public copy of one's own hero"
    );
    assert!(find(&second, two).is_none());
    assert!(find(&first, two).is_some());
    assert!(find(&second, one).is_some());
    // Another player's owner state is never projected to this connection.
    assert!(find(&first, rep.owner_entity(2).unwrap()).is_none());
}

#[test]
fn separated_observers_do_not_receive_distant_heroes() {
    let mut sim = party();
    sim.continue_run_for(1);
    sim.continue_run_for(2);
    for _ in 0..180 {
        sim.step_multiplayer(&[
            (
                1,
                DreamInput {
                    movement: [-1.0, 0.0],
                    ..Default::default()
                },
            ),
            (
                2,
                DreamInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                },
            ),
        ]);
    }
    let cap = capture(&sim, 1);
    let p1 = cap.position(ReplicationKey::Hero(1)).unwrap();
    let p2 = cap.position(ReplicationKey::Hero(2)).unwrap();
    assert!((p1[0] - p2[0]).abs() > 13.0, "{p1:?} {p2:?}");
    let mut rep = adapter();
    join(&mut rep, 1);
    join(&mut rep, 2);
    rep.prepare(cap, &[1, 2]).unwrap();
    let one = rep.actor_entity(ReplicationKey::Hero(1)).unwrap();
    let two = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
    assert!(find(&gather(&mut rep, 1), two).is_none());
    assert!(find(&gather(&mut rep, 2), one).is_none());
}

#[test]
fn leaving_relevance_or_despawning_is_reported_and_recreation_changes_generation() {
    let mut sim = party();
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
    let old = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
    assert!(find(&gather(&mut rep, 1), old).is_some());
    assert!(sim.remove_player(2));
    rep.prepare(capture(&sim, 2), &[1]).unwrap();
    assert_eq!(gather(&mut rep, 1).removed, vec![old]);
    assert!(sim.add_player(2));
    rep.prepare(capture(&sim, 3), &[1, 2]).unwrap();
    let new = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
    assert_eq!(old.index, new.index);
    assert!(new.generation > old.generation);
    let current = gather(&mut rep, 1);
    assert!(find(&current, new).is_some());
    assert!(current.removed.is_empty());
    // A capture older than the graph's state is refused outright.
    assert!(rep.prepare(capture(&sim, 2), &[1, 2]).is_err());
}

#[test]
fn hidden_source_pulse_uses_independent_coarse_event_audience() {
    let mut sim = party();
    sim.continue_run_for(1);
    sim.continue_run_for(2);
    for _ in 0..180 {
        sim.step_multiplayer(&[
            (
                1,
                DreamInput {
                    movement: [-1.0, 0.0],
                    ..Default::default()
                },
            ),
            (
                2,
                DreamInput {
                    movement: [1.0, 0.0],
                    ..Default::default()
                },
            ),
        ]);
    }
    let position = capture(&sim, 1).position(ReplicationKey::Hero(1)).unwrap();
    sim.step_multiplayer(&[(
        2,
        DreamInput {
            aim: [-1.0, 0.0],
            casts: [false, true, false, false],
            action_sequences: [0, 0, 909_991, 0, 0],
            ..Default::default()
        },
    )]);
    let id = {
        let world = sim.world_mut();
        let mut query = world.query::<&mut engine_core::GraphicsInstance>();
        let mut effect = query
            .iter_mut(world)
            .find(|effect| {
                effect.id.owner == 2
                    && matches!(effect.kind, engine_core::GraphicsKind::RadialPulse)
            })
            .expect("cast pulse");
        effect.pos = position;
        effect.id
    };
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
    let source = rep.actor_entity(ReplicationKey::Hero(2)).unwrap();
    let event = rep.actor_entity(ReplicationKey::Effect(id)).unwrap();
    let seen = gather(&mut rep, 1);
    assert!(find(&seen, source).is_none());
    let Some(Replica::Actor(PublicReplica::Effect(proxy))) = find(&seen, event) else {
        panic!("event proxy")
    };
    assert_eq!(proxy.id.owner, 0);
    assert!(proxy.id.scope.is_none());
    assert_ne!(proxy.id.action_seq, id.action_seq);
    assert_eq!(
        proxy.position,
        position.map(|v| ((v / 8.0).floor() + 0.5) * 8.0)
    );
    assert_eq!(proxy.radius, 8.0);
}

#[test]
fn cover_markers_name_their_cover_entity_and_follow_it() {
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    let cover_id = sim.snapshot().covers[0].id;
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 1), &[1]).unwrap();
    let cover = rep.actor_entity(ReplicationKey::Cover(cover_id)).unwrap();
    let seen = gather(&mut rep, 1);
    let marker = seen
        .entities
        .iter()
        .find_map(|entry| match &entry.replica {
            Replica::Actor(PublicReplica::CoverMarker(marker)) if marker.parent == cover => {
                Some(entry.entity)
            }
            _ => None,
        })
        .expect("the present cover's marker names it");
    assert!(find(&seen, cover).is_some());
    // Removing the cover removes its marker; the cover region keeps reporting
    // its current absence.
    assert!(sim.remove_cover(cover_id).unwrap());
    sim.step(DreamInput::default());
    rep.prepare(capture(&sim, 2), &[1]).unwrap();
    let seen = gather(&mut rep, 1);
    assert!(seen.removed.contains(&marker));
    assert!(matches!(
        find(&seen, cover),
        Some(Replica::Actor(PublicReplica::Cover(view))) if !view.present
    ));
}

#[test]
fn owner_platform_dependency_is_projected_with_the_owner() {
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run_for(1);
    {
        let world = sim.world_mut();
        let mut query = world.query::<&mut engine_core::KinematicState>();
        *query.single_mut(world).unwrap() = engine_core::KinematicState::new([1.0, 0.25, -10.0], 1);
    }
    sim.step(DreamInput::default());
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 1), &[1]).unwrap();
    let base = *rep.owner_bases[&1].first().unwrap();
    let seen = gather(&mut rep, 1);
    let Some(Replica::Owner(owner)) = find(&seen, rep.owner_entity(1).unwrap()) else {
        panic!("owner")
    };
    let Some(Replica::Actor(PublicReplica::Platform(platform))) = find(&seen, base) else {
        panic!("required platform travels with its rider")
    };
    assert_eq!(owner.required_bases(), [platform.collider]);
}

#[test]
fn owner_continuity_must_match_the_capture_tick() {
    let sim = party();
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 3), &[1, 2]).unwrap();
    assert!(
        rep.gather_peer(1, ServerTick(2), &Default::default())
            .is_err()
    );
    assert!(
        rep.gather_peer(1, ServerTick(3), &Default::default())
            .is_ok()
    );
}

#[test]
fn unready_scene_withholds_everything_until_ready_again() {
    let sim = party();
    let mut rep = adapter();
    join(&mut rep, 1);
    rep.prepare(capture(&sim, 1), &[1, 2]).unwrap();
    let first = gather(&mut rep, 1);
    assert!(!first.entities.is_empty());
    rep.set_peer(1, ConnectionEpoch(1), SceneRevision(2))
        .unwrap();
    rep.prepare(capture(&sim, 2), &[1, 2]).unwrap();
    let unready = gather(&mut rep, 1);
    assert!(unready.entities.is_empty());
    let mut removed = unready.removed.clone();
    removed.sort();
    let mut previous: Vec<_> = first.entities.iter().map(|entry| entry.entity).collect();
    previous.sort();
    assert_eq!(removed, previous);
}
