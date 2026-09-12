use crate::replication::*;
use crate::*;

fn capture(sim: &DreamSimulation, revision: u64) -> CommittedReplication {
    sim.capture_replication(ReplicationStamp {
        match_epoch: 1,
        server_tick: u64::from(sim.gameplay_tick()),
        gameplay_tick: sim.gameplay_tick(),
        scene_revision: 1,
        revision,
    })
    .unwrap()
}

#[test]
fn pending_join_is_checkpointable_and_does_not_participate_or_disclose_publicly() {
    let mut sim = DreamSimulation::new(77, false);
    assert!(sim.add_pending_player(2));
    assert!(!sim.player_is_active(2));
    assert_eq!(sim.world.resource::<Run>().party_size, 1);
    assert!(!sim.continue_run_for(2));
    assert!(sim.continue_run_for(1));
    assert_eq!(
        sim.phase(),
        RunPhase::Combat,
        "pending session must not block ready players"
    );
    let initial = sim.snapshot_for(2).hero;
    let input = DreamInput {
        movement: [1.0, 0.0],
        aim: [0.0, -1.0],
        attack: true,
        dash: true,
        casts: [true; 4],
        ..Default::default()
    };
    for _ in 0..120 {
        sim.step_multiplayer(&[(2, input)]);
    }
    let pending = sim.snapshot_for(2).hero;
    assert_eq!(pending.position, initial.position);
    assert_eq!(pending.hp, initial.hp);
    assert_eq!(pending.shards, initial.shards);
    assert_eq!(pending.xp, initial.xp);
    assert_eq!(pending.attack_cooldown, 0.0);
    assert!(!sim.swap_memories_for(2, 0, 1));
    let captured = capture(&sim, 1);
    assert!(!captured.keys().contains(&ReplicationKey::Hero(2)));
    let owner = captured.owner_checkpoint(2, 1).unwrap();
    assert!(!owner.is_active());
    let mut predicted = OwnerPredictionState::from_checkpoint(&owner, owner.expectation()).unwrap();
    predicted.step_restricted(input, &collision()).unwrap();
    assert_eq!(predicted.hero_view(), owner.hero_view());
    let restored = DreamSimulation::try_from_snapshot(&sim.snapshot()).unwrap();
    assert!(
        !capture(&restored, 2)
            .owner_checkpoint(2, 1)
            .unwrap()
            .is_active()
    );
}

#[test]
fn activation_scales_party_once_and_pending_departure_never_changes_difficulty() {
    let mut sim = DreamSimulation::new(88, false);
    sim.continue_run_for(1);
    let before = sim.snapshot().enemies;
    sim.add_pending_player(2);
    assert_eq!(sim.snapshot().enemies, before);
    assert!(sim.remove_player(2));
    assert_eq!(sim.snapshot().enemies, before);
    assert_eq!(sim.world.resource::<Run>().party_size, 1);
    sim.add_pending_player(2);
    assert!(sim.set_player_active(2, true));
    assert!(sim.player_is_active(2));
    let after = sim.snapshot().enemies;
    assert!((after[0].max_hp - before[0].max_hp * 1.65).abs() < 0.001);
    sim.set_player_active(2, true);
    assert_eq!(sim.snapshot().enemies, after);
    let captured = capture(&sim, 1);
    assert!(captured.keys().contains(&ReplicationKey::Hero(2)));
    assert!(captured.owner_checkpoint(2, 1).unwrap().is_active());
    sim.restart_party(89, false);
    assert_eq!(sim.world.resource::<Run>().party_size, 2);
}

#[test]
fn pending_actor_cannot_soak_enemy_aim_damage_or_keep_defeated_party_alive() {
    let mut ordinary = DreamSimulation::new(99, false);
    let mut pending = DreamSimulation::new(99, false);
    pending.add_pending_player(2);
    ordinary.continue_run_for(1);
    pending.continue_run_for(1);
    for _ in 0..90 {
        ordinary.step(DreamInput::default());
        pending.step(DreamInput::default());
    }
    assert_eq!(ordinary.snapshot().enemies, pending.snapshot().enemies);
    assert_eq!(ordinary.snapshot().hero, pending.snapshot().hero);
    for mut hero in pending
        .world
        .query::<HeroActor>()
        .iter_mut(&mut pending.world)
    {
        if hero.view.id == 1 {
            hero.health.hp = 0.0;
        }
    }
    pending.step(DreamInput::default());
    assert_eq!(pending.phase(), RunPhase::Defeat);
    assert!(pending.snapshot_for(2).hero.hp > 0.0);
}

#[test]
fn disconnect_and_resume_keep_progression_damage_and_protection_without_free_heal() {
    let mut sim = DreamSimulation::new(101, false);
    sim.add_player(2);
    sim.continue_run_for(1);
    sim.continue_run_for(2);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == 2 {
            hero.health.hp = 23.0;
            hero.combat.invulnerability_remaining = 0.0;
            hero.progression.xp = 17.0;
            hero.actor.view.shards = 31;
        }
    }
    let before = sim.snapshot_for(2).hero;
    assert!(sim.set_player_active(2, false));
    for _ in 0..30 {
        sim.step_multiplayer(&[
            (1, DreamInput::default()),
            (
                2,
                DreamInput {
                    attack: true,
                    casts: [true; 4],
                    ..Default::default()
                },
            ),
        ]);
    }
    assert_eq!(sim.snapshot_for(2).hero.hp, before.hp);
    assert!(sim.add_pending_player(2));
    assert!(sim.resume_player(2));
    let after = sim.snapshot_for(2).hero;
    assert_eq!(after.hp, before.hp);
    assert_eq!(after.memories, before.memories);
    assert_eq!(after.xp, before.xp);
    assert_eq!(after.shards, before.shards);
    for hero in sim.world.query::<HeroActorReadOnly>().iter(&sim.world) {
        if hero.view.id == 2 {
            assert_eq!(hero.combat.invulnerability_remaining, 0.0);
        }
    }
}

#[test]
fn disconnected_player_leaves_no_owned_offensive_entities() {
    let mut sim = DreamSimulation::new(102, false);
    sim.add_player(2);
    sim.continue_run_for(1);
    sim.continue_run_for(2);
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        if hero.view.id == 2 {
            hero.loadout.0[0] = memory_slot(MemoryKind::Starfall);
            hero.loadout.0[0].modifier = Some(EssenceKind::Echo);
            hero.loadout.0[1] = memory_slot(MemoryKind::Wisp);
        }
    }
    sim.step_multiplayer(&[(
        2,
        DreamInput {
            aim: [0.0, -1.0],
            casts: [true, true, false, false],
            ..Default::default()
        },
    )]);
    let before = sim.snapshot();
    assert!(!before.state.projectiles.is_empty());
    assert!(!before.state.wisps.is_empty());
    assert!(!before.state.delayed.is_empty());
    sim.set_player_active(2, false);
    let after = sim.snapshot();
    assert!(after.state.projectiles.iter().all(|p| p.state.owner != 2));
    assert!(after.state.wisps.iter().all(|p| p.state.owner != 2));
    assert!(after.state.delayed.iter().all(|p| p.payload.owner != 2));
    assert!(after.state.effects.iter().all(|p| p.id.owner != 2));
}

fn collision() -> crate::collision::CollisionWorld {
    crate::collision::CollisionManifest::current(1)
        .build()
        .unwrap()
}
