//! Match boundaries enclose the shared fixed tick.
use super::*;
#[derive(Resource, Default)]
pub(crate) struct TickStatus {
    pub running: bool,
}
pub(crate) fn begin(
    mut globals: ResMut<Globals>,
    mut status: ResMut<TickStatus>,
    mut events: ResMut<TickEvents>,
) {
    globals.tick = globals.tick.wrapping_add(1);
    status.running = globals.phase == MatchPhase::InProgress;
    events.0.clear();
}
pub(crate) fn reset_if_finished(world: &mut World) {
    if !world.resource::<TickStatus>().running {
        world.resource_scope(|world, mut globals: Mut<Globals>| globals.maybe_reset_match(world));
    }
}
pub(crate) fn running(status: Res<TickStatus>) -> bool {
    status.running
}
pub(crate) fn spawn(world: &mut World) {
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        let mut events = Vec::new();
        globals.spawn_wave_units(world, &mut events);
        world.resource_mut::<TickEvents>().0.extend(events);
    });
}
pub(crate) fn finish(world: &mut World) {
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        if globals.team_life <= 0 && globals.phase == MatchPhase::InProgress {
            globals.phase = MatchPhase::Defeat;
            globals.schedule_match_reset(world);
            world
                .resource_mut::<TickEvents>()
                .0
                .push(ReliableGameEvent::Defeat);
        }
        world.resource_mut::<HistoricalTargets>().0.clear();
    });
}

impl Globals {
    pub(crate) fn add_player(&mut self, world: &mut World, client_id: u64) {
        if storage::hero(world, &client_id).is_some() {
            return;
        }

        insert_hero(
            world,
            client_id,
            HeroBundle {
                role: HeroState {
                    respawn_generation: 0,
                    move_dir: [0.0, 0.0],
                    lock_mode_active: false,
                    lock_target_id: None,
                    charge_profile: HERO_CHARGE_PROFILE,
                    power_decay_profile: HERO_POWER_DECAY_PROFILE,
                    powered_modifiers: HERO_POWERED_MODIFIERS,
                    charge_state: ChargeStateComponent::default(),
                    mana: HERO_MAX_MANA,
                    gold: INITIAL_GOLD,
                    ability_cooldown_ticks: 0,
                    last_processed_seq: None,
                    last_action_seq: None,
                    last_attack_seq: None,
                    pending_actions: VecDeque::new(),
                    pending_moves: VecDeque::new(),
                },
                position: Position {
                    pos: spawn_position_for_client(client_id),
                },
                health: Health {
                    hp: HERO_MAX_HP,
                    max_hp: HERO_MAX_HP,
                },
                facing: Facing {
                    direction: FacingComponent::default(),
                },
                attack: Attack {
                    state: DirectionalAttackStateComponent::default(),
                    profile: HERO_REGULAR_ATTACK,
                },
            },
        );
    }

    pub(crate) fn remove_player(&mut self, world: &mut World, client_id: u64) {
        remove_hero(world, &client_id);
        world
            .resource_mut::<CommandInbox>()
            .0
            .retain(|(id, _)| *id != client_id);

        let owned_towers: Vec<u64> = towers(world)
            .filter(|tower| tower.role.owner == client_id)
            .map(|tower| tower.role.id)
            .collect();

        let mut removed_any_tower = false;
        for tower_id in owned_towers {
            if let Some(tower) = remove_tower(world, &tower_id) {
                self.node_occupancy.remove(&tower.node_id);
                removed_any_tower = true;
            }
        }
        if removed_any_tower {
            world.resource_mut::<NavigationCache>().dirty = true;
        }
    }

    pub(crate) fn has_players(&self, world: &World) -> bool {
        !actor_ids::<HeroState>(world).is_empty()
    }

    pub(crate) fn match_restart_ticks_remaining(&self, _world: &World) -> Option<u32> {
        self.reset_at_tick.map(|reset_at_tick| {
            if is_newer_input_seq(reset_at_tick, self.tick) {
                reset_at_tick.wrapping_sub(self.tick)
            } else {
                0
            }
        })
    }

    pub(crate) fn spawn_wave_units(
        &mut self,
        world: &mut World,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        if is_newer_input_seq(self.intermission_until, self.tick) {
            return;
        }

        if self.wave_remaining == 0 {
            if !actor_ids::<EnemyState>(world).is_empty() {
                return;
            }

            if self.wave >= MAX_WAVES {
                self.phase = MatchPhase::Victory;
                self.schedule_match_reset(world);
                reliable_events.push(ReliableGameEvent::Victory);
                return;
            }

            self.wave += 1;
            let spawn_count = BASE_ENEMIES_PER_WAVE + (self.wave - 1) * EXTRA_ENEMIES_PER_WAVE;
            self.wave_remaining = spawn_count;
            self.next_spawn_tick = self.tick;
            self.next_spawn_point_index = 0;

            reliable_events.push(ReliableGameEvent::WaveStarted { wave: self.wave });
        }

        if is_newer_input_seq(self.next_spawn_tick, self.tick) {
            return;
        }

        if ENEMY_SPAWN_POINTS.is_empty() {
            return;
        }

        let batch_size = ENEMY_SPAWN_POINTS.len().min(self.wave_remaining as usize);
        for _ in 0..batch_size {
            let spawn_idx = self.next_spawn_point_index % ENEMY_SPAWN_POINTS.len();
            self.spawn_enemy(world, spawn_idx, self.wave);
            self.wave_remaining -= 1;
            self.next_spawn_point_index = self.next_spawn_point_index.wrapping_add(1);
        }

        self.next_spawn_tick = self.tick.wrapping_add(WAVE_SPAWN_INTERVAL_TICKS);

        if self.wave_remaining == 0 {
            self.intermission_until = self.tick.wrapping_add(WAVE_PREP_TICKS);
        }
    }

    pub(crate) fn spawn_enemy(&mut self, world: &mut World, spawn_point_index: usize, wave: u32) {
        let Some(spawn_pos) = ENEMY_SPAWN_POINTS.get(spawn_point_index).copied() else {
            return;
        };

        let enemy_type = if wave.is_multiple_of(4) {
            EnemyType::Tank
        } else {
            EnemyType::Grunt
        };

        let (max_hp, speed, reward) = match enemy_type {
            EnemyType::Grunt => (
                34.0 + wave as f32 * 9.0,
                2.0 + wave as f32 * 0.055,
                14 + wave * 2,
            ),
            EnemyType::Tank => (
                84.0 + wave as f32 * 18.0,
                1.45 + wave as f32 * 0.045,
                28 + wave * 3,
            ),
        };
        let radius = enemy_collider_radius(enemy_type);

        let home = BASE_POSITION;
        let initial_facing =
            cardinalize_dir_or([home[0] - spawn_pos[0], home[1] - spawn_pos[1]], [1.0, 0.0]);
        let enemy_id = self.alloc_entity_id(world);
        insert_enemy(
            world,
            enemy_id,
            EnemyBundle {
                role: EnemyState {
                    spawn: game_shared::EnemySpawnIdentity {
                        wave,
                        ordinal: self.next_spawn_point_index as u32,
                    },
                    id: enemy_id,
                    lane: 0,
                    vel: [0.0, 0.0],
                    speed: speed,
                    reward: reward,
                    enemy_type: enemy_type,
                    radius: radius,
                    lock_target: EnemyLockTarget::Base,
                    target_pos: home,
                    waypoint: home,
                    repath_cooldown: 0,
                },
                position: Position { pos: spawn_pos },
                health: Health {
                    hp: max_hp,
                    max_hp: max_hp,
                },
                facing: Facing {
                    direction: FacingComponent {
                        dir: initial_facing,
                    },
                },
                attack: Attack {
                    state: DirectionalAttackStateComponent::default(),
                    profile: ENEMY_REGULAR_ATTACK,
                },
            },
        );
    }

    pub(crate) fn alloc_entity_id(&mut self, _world: &mut World) -> u64 {
        let next = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.wrapping_add(1);
        next
    }

    pub(crate) fn schedule_match_reset(&mut self, _world: &mut World) {
        if self.reset_at_tick.is_some() {
            return;
        }
        self.reset_at_tick = Some(self.tick.wrapping_add(MATCH_RESET_TICKS));
    }

    pub(crate) fn maybe_reset_match(&mut self, world: &mut World) {
        let Some(reset_at_tick) = self.reset_at_tick else {
            return;
        };
        if is_newer_input_seq(reset_at_tick, self.tick) {
            return;
        }
        self.reset_match_state(world);
    }

    pub(crate) fn reset_match_state(&mut self, world: &mut World) {
        world.resource_mut::<HistoricalTargets>().0.clear();
        systems::presentation::clear(world);
        let next_epoch = world.resource::<Epoch>().0.wrapping_add(1);
        self.phase = MatchPhase::InProgress;
        self.team_life = INITIAL_TEAM_LIFE;
        self.wave = 0;
        self.objective_hp = OBJECTIVE_MAX_HP;
        clear_actors::<EnemyState>(world);
        clear_actors::<TowerState>(world);
        self.node_occupancy.clear();
        world.resource_mut::<NavigationCache>().dirty = true;
        world.resource_mut::<CommandInbox>().0.clear();
        for actor_id in actor_ids::<HeroState>(world) {
            let hero = hero_mut(world, &actor_id).unwrap();
            hero.role.pending_actions.clear();
        }
        for id in actor_ids::<HeroState>(world) {
            hero_mut(world, &id).unwrap().role.pending_moves.clear();
        }
        self.wave_remaining = 0;
        self.next_spawn_tick = 0;
        self.next_spawn_point_index = 0;
        self.intermission_until = self.tick.wrapping_add(WAVE_PREP_TICKS);
        self.reset_at_tick = None;

        for client_id in actor_ids::<HeroState>(world) {
            let hero = hero_mut(world, &client_id).unwrap();
            hero.position.pos = spawn_position_for_client(client_id);
            hero.role.move_dir = [0.0, 0.0];
            hero.facing.direction = FacingComponent::default();
            hero.role.lock_mode_active = false;
            hero.role.lock_target_id = None;
            hero.attack.state = DirectionalAttackStateComponent::default();
            hero.attack.profile = HERO_REGULAR_ATTACK;
            hero.role.charge_profile = HERO_CHARGE_PROFILE;
            hero.role.power_decay_profile = HERO_POWER_DECAY_PROFILE;
            hero.role.powered_modifiers = HERO_POWERED_MODIFIERS;
            hero.role.charge_state = ChargeStateComponent::default();
            hero.health.hp = HERO_MAX_HP;
            hero.role.mana = HERO_MAX_MANA;
            hero.role.gold = INITIAL_GOLD;
            hero.role.ability_cooldown_ticks = 0;
        }
        let survivors: Vec<_> = actor_ids::<HeroState>(world)
            .into_iter()
            .filter_map(|id| local::<HeroState>(world, id).map(|e| (id, e)))
            .collect();
        for (id, entity) in survivors {
            world
                .entity_mut(entity)
                .insert(game_replication::NetId::new(
                    next_epoch,
                    game_shared::actor_namespace::HERO,
                    id,
                ));
        }
        world.resource_mut::<Epoch>().0 = next_epoch;
    }
}
