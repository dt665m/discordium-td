//! Read-only snapshot extraction from authoritative ECS components.
//!
//! QueryState caches archetype matching, not gameplay state. Its short locks let
//! callers retain an `&World` API while Bevy updates those caches after spawns.
use crate::{
    Globals,
    components::*,
    storage::{Epoch, SimulationOwned},
};
use bevy::{ecs::query::QueryState, prelude::*};
use game_replication::NetId;
use game_shared::{
    EnemySnapshot, HeroSnapshot, OBJECTIVE_MAX_HP, ObjectiveSnapshot, TowerSnapshot, WorldDelta,
    actor_namespace,
};
use std::sync::Mutex;

type HeroData = (
    &'static NetId,
    &'static HeroState,
    &'static Position,
    &'static Health,
    &'static Facing,
    &'static Attack,
);
type EnemyData = (
    &'static NetId,
    &'static EnemyState,
    &'static Position,
    &'static Health,
    &'static Facing,
    &'static Attack,
);
type TowerData = (&'static NetId, &'static TowerState, &'static Position);

#[derive(Resource)]
pub(crate) struct SnapshotQueries {
    heroes: Mutex<QueryState<HeroData, With<SimulationOwned>>>,
    enemies: Mutex<QueryState<EnemyData, With<SimulationOwned>>>,
    towers: Mutex<QueryState<TowerData, With<SimulationOwned>>>,
}

impl FromWorld for SnapshotQueries {
    fn from_world(world: &mut World) -> Self {
        Self {
            heroes: Mutex::new(world.query_filtered()),
            enemies: Mutex::new(world.query_filtered()),
            towers: Mutex::new(world.query_filtered()),
        }
    }
}

pub(crate) fn world_delta(world: &World) -> WorldDelta {
    capture(world, world.resource::<Globals>())
}

pub(crate) fn capture(world: &World, globals: &Globals) -> WorldDelta {
    let queries = world.resource::<SnapshotQueries>();
    let epoch = world.resource::<Epoch>().0;
    let mut heroes: Vec<_> = queries
        .heroes
        .lock()
        .expect("snapshot hero query poisoned")
        .iter(world)
        .filter(|(id, ..)| id.epoch == epoch && id.namespace == actor_namespace::HERO)
        .map(
            |(id, hero, position, health, facing, attack)| HeroSnapshot {
                respawn_generation: hero.respawn_generation,
                client_id: id.value,
                pos: position.pos,
                facing: facing.direction,
                lock_mode_active: hero.lock_mode_active,
                lock_target_id: hero.lock_target_id,
                regular_attack: attack.state,
                charge_profile: hero.charge_profile,
                power_decay_profile: hero.power_decay_profile,
                powered_modifiers: hero.powered_modifiers,
                charge_state: hero.charge_state,
                hp: health.hp,
                mana: hero.mana,
                gold: hero.gold,
                ability_cooldown_ticks: hero.ability_cooldown_ticks,
                move_dir: hero.move_dir,
                pending_moves: hero.pending_moves.iter().copied().collect(),
                last_move_seq: hero.last_processed_seq,
                last_action_seq: hero.last_action_seq,
                last_attack_seq: hero.last_attack_seq,
                pending_actions: hero.pending_actions.iter().copied().collect(),
            },
        )
        .collect();
    heroes.sort_unstable_by_key(|hero| hero.client_id);

    let mut enemies: Vec<_> = queries
        .enemies
        .lock()
        .expect("snapshot enemy query poisoned")
        .iter(world)
        .filter(|(id, ..)| id.epoch == epoch && id.namespace == actor_namespace::ENEMY)
        .map(
            |(id, enemy, position, health, facing, attack)| EnemySnapshot {
                spawn: enemy.spawn,
                id: id.value,
                lane: enemy.lane,
                pos: position.pos,
                vel: enemy.vel,
                facing: facing.direction,
                regular_attack: attack.state,
                hp: health.hp.max(0.0),
                max_hp: health.max_hp,
                enemy_type: enemy.enemy_type,
                speed: enemy.speed,
                reward: enemy.reward,
                lock_target: enemy.lock_target,
                target_pos: enemy.target_pos,
                waypoint: enemy.waypoint,
                repath_cooldown: enemy.repath_cooldown,
            },
        )
        .collect();
    enemies.sort_unstable_by_key(|enemy| enemy.id);

    let mut towers: Vec<_> = queries
        .towers
        .lock()
        .expect("snapshot tower query poisoned")
        .iter(world)
        .filter(|(id, ..)| id.epoch == epoch && id.namespace == actor_namespace::TOWER)
        .map(|(id, tower, position)| TowerSnapshot {
            id: id.value,
            owner: tower.owner,
            lane: tower.lane,
            node_id: tower.node_id,
            pos: position.pos,
            reload_ticks_remaining: tower.reload_ticks,
        })
        .collect();
    towers.sort_unstable_by_key(|tower| tower.id);

    WorldDelta {
        presentations: crate::world::presentations(world)
            .into_iter()
            .copied()
            .collect(),
        tick: globals.tick,
        phase: globals.phase,
        match_restart_ticks_remaining: globals.match_restart_ticks_remaining(world),
        wave: globals.wave,
        team_life: globals.team_life,
        objectives: vec![ObjectiveSnapshot {
            lane: 0,
            hp: globals.objective_hp,
            max_hp: OBJECTIVE_MAX_HP,
        }],
        heroes,
        enemies,
        towers,
        sim_meta: Some(globals.sim_meta(world)),
    }
}
