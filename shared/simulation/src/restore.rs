use crate::*;

impl Globals {
    pub(crate) fn apply_snapshot(&mut self, world: &mut World, delta: &WorldDelta, meta: &SimMeta) {
        world.resource_mut::<HistoricalTargets>().0.clear();
        systems::presentation::restore(world, &delta.presentations);
        self.tick = delta.tick;
        self.phase = delta.phase;
        self.team_life = delta.team_life;
        self.wave = delta.wave;
        self.objective_hp = delta
            .objectives
            .first()
            .map(|o| o.hp)
            .unwrap_or(OBJECTIVE_MAX_HP);
        self.next_entity_id = meta.next_entity_id;
        self.wave_remaining = meta.wave_remaining;
        self.next_spawn_tick = meta.next_spawn_tick;
        self.next_spawn_point_index = meta.next_spawn_point_index as usize;
        self.intermission_until = meta.intermission_until;
        self.reset_at_tick = delta
            .match_restart_ticks_remaining
            .map(|remaining| delta.tick.wrapping_add(remaining));
        // Only obstacle geometry invalidates the navigation cache. Snapshot
        // restore/replay happens at network cadence, including unchanged towers.
        let obstacles_changed = actor_ids::<TowerState>(world).len() != delta.towers.len()
            || delta.towers.iter().any(|tower| {
                storage::tower(world, &tower.id).is_none_or(|old| old.position.pos != tower.pos)
            });
        world.resource_mut::<NavigationCache>().dirty |= obstacles_changed;
        world.resource_mut::<CommandInbox>().0.clear();
        prune_snapshot_actors(world, delta, meta);
        world.resource_mut::<Epoch>().0 = meta.match_epoch;
        self.node_occupancy.clear();
        populate_from_delta(self, world, delta);
    }
}

pub(crate) fn populate_from_delta(sim: &mut Globals, world: &mut World, delta: &WorldDelta) {
    for hero_snap in &delta.heroes {
        insert_hero(
            world,
            hero_snap.client_id,
            HeroBundle {
                role: HeroState {
                    respawn_generation: hero_snap.respawn_generation,
                    move_dir: hero_snap.move_dir,
                    lock_mode_active: hero_snap.lock_mode_active,
                    lock_target_id: hero_snap.lock_target_id,
                    charge_profile: hero_snap.charge_profile,
                    power_decay_profile: hero_snap.power_decay_profile,
                    powered_modifiers: hero_snap.powered_modifiers,
                    charge_state: hero_snap.charge_state,
                    mana: hero_snap.mana,
                    gold: hero_snap.gold,
                    ability_cooldown_ticks: hero_snap.ability_cooldown_ticks,
                    last_processed_seq: hero_snap.last_move_seq,
                    last_action_seq: hero_snap.last_action_seq,
                    last_attack_seq: hero_snap.last_attack_seq,
                    pending_actions: hero_snap.pending_actions.iter().copied().collect(),
                    pending_moves: hero_snap.pending_moves.iter().copied().collect(),
                },
                position: Position { pos: hero_snap.pos },
                health: Health {
                    hp: hero_snap.hp,
                    max_hp: HERO_MAX_HP,
                },
                facing: Facing {
                    direction: hero_snap.facing,
                },
                attack: Attack {
                    state: hero_snap.regular_attack,
                    profile: HERO_REGULAR_ATTACK,
                },
            },
        );
    }

    for enemy_snap in &delta.enemies {
        let radius = enemy_collider_radius(enemy_snap.enemy_type);

        insert_enemy(
            world,
            enemy_snap.id,
            EnemyBundle {
                role: EnemyState {
                    spawn: enemy_snap.spawn,
                    id: enemy_snap.id,
                    lane: enemy_snap.lane,
                    vel: enemy_snap.vel,
                    speed: enemy_snap.speed,
                    reward: enemy_snap.reward,
                    enemy_type: enemy_snap.enemy_type,
                    radius: radius,
                    lock_target: enemy_snap.lock_target,
                    target_pos: enemy_snap.target_pos,
                    waypoint: enemy_snap.waypoint,
                    repath_cooldown: enemy_snap.repath_cooldown,
                },
                position: Position {
                    pos: enemy_snap.pos,
                },
                health: Health {
                    hp: enemy_snap.hp,
                    max_hp: enemy_snap.max_hp,
                },
                facing: Facing {
                    direction: enemy_snap.facing,
                },
                attack: Attack {
                    state: enemy_snap.regular_attack,
                    profile: ENEMY_REGULAR_ATTACK,
                },
            },
        );
    }

    for tower_snap in &delta.towers {
        insert_tower(
            world,
            tower_snap.id,
            TowerBundle {
                role: TowerState {
                    id: tower_snap.id,
                    owner: tower_snap.owner,
                    lane: tower_snap.lane,
                    node_id: tower_snap.node_id,
                    reload_ticks: tower_snap.reload_ticks_remaining,
                },
                position: Position {
                    pos: tower_snap.pos,
                },
            },
        );
        sim.node_occupancy.insert(tower_snap.node_id, tower_snap.id);
    }
}
