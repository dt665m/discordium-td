use super::*;
use crate::*;

fn arena(players: u64) -> DreamSimulation {
    let mut sim = DreamSimulation::new(19, false);
    for owner in 2..=players {
        assert!(sim.add_player(owner));
    }
    for owner in 1..=players {
        assert!(sim.continue_run_for(owner));
    }
    for mut hero in sim.world.query::<HeroActor>().iter_mut(&mut sim.world) {
        hero.motion.position = [-6.0, 0.0, 12.0];
    }
    for mut enemy in sim.world.query::<EnemyActor>().iter_mut(&mut sim.world) {
        enemy.motor.position = [50.0, 50.0];
        enemy.motor.movement_lock = 1000.0;
    }
    sim
}

fn checkpoint(sim: &DreamSimulation, tick: u64) -> OwnerCheckpoint {
    let state = sim.snapshot();
    sim.capture_replication(ReplicationStamp {
        match_epoch: 1,
        server_tick: tick,
        gameplay_tick: state.tick,
        scene_revision: state.collision_manifest().scene_revision(),
        revision: tick + 1,
    })
    .unwrap()
    .owner_checkpoint(1, 1)
    .unwrap()
}

#[test]
fn eight_overlapping_and_crossing_players_do_not_change_owner_collision_or_replay() {
    let mut crowded = arena(8);
    let mut alone = arena(1);
    let collision = crowded
        .world
        .resource::<collision::CollisionWorld>()
        .clone();
    let initial = checkpoint(&crowded, 0);
    let mut predicted =
        OwnerPredictionState::from_checkpoint(&initial, initial.expectation()).unwrap();
    let mut history = vec![predicted.clone()];
    let inputs: Vec<_> = (0..180)
        .map(|tick| DreamInput {
            movement: if tick < 90 { [1.0, 0.0] } else { [-1.0, 0.0] },
            aim: [1.0, 0.0],
            dash: tick == 20 || tick == 110,
            ..Default::default()
        })
        .collect();
    for (tick, input) in inputs.iter().copied().enumerate() {
        // Four peers overlap the owner exactly; the rest cross the same lane.
        let party: Vec<_> = (1..=8)
            .map(|owner| {
                (
                    owner,
                    if owner <= 4 {
                        input
                    } else {
                        DreamInput {
                            movement: [-input.movement[0], 0.0],
                            ..input
                        }
                    },
                )
            })
            .collect();
        crowded.step_multiplayer(&party);
        alone.step(input);
        predicted.step_restricted(input, &collision).unwrap();
        let authoritative = checkpoint(&crowded, tick as u64 + 1);
        assert_eq!(
            authoritative.hero.motion,
            checkpoint(&alone, tick as u64 + 1).hero.motion,
            "crowding changed authoritative movement at tick {tick}"
        );
        assert_eq!(
            predicted.checkpoint.hero.motion, authoritative.hero.motion,
            "crowding induced an owner correction at tick {tick}"
        );
        let owner_position = crowded.snapshot_for(1).hero.position;
        for owner in 2..=4 {
            assert_eq!(
                crowded.snapshot_for(owner).hero.position,
                owner_position,
                "the declared non-blocking policy permits coincident actors"
            );
        }
        history.push(predicted.clone());
    }
    // Reconcile from delayed owner checkpoints while remote presentation could
    // be arbitrarily far behind. The replay API has no rendered actor input.
    for boundary in [0, 17, 60, 125, 170] {
        let mut replay = history[boundary].clone();
        for input in &inputs[boundary..] {
            replay.step_restricted(*input, &collision).unwrap();
        }
        assert_eq!(
            replay.checkpoint.hero.motion,
            predicted.checkpoint.hero.motion
        );
    }
}
