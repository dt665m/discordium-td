mod input;
use std::{
    collections::{BTreeMap, VecDeque},
    f32::consts::TAU,
};

use game_shared::{
    AbilityId, AttackPhase, BASE_ENEMIES_PER_WAVE, BASE_POSITION, BUILD_COMMAND_MAX_DISTANCE,
    BUILD_NODES, BuildRejectReason, ChargePhase, ChargeProfileComponent, ChargeStateComponent,
    ClientCommand, DirectionalAttackComponent, DirectionalAttackStateComponent,
    ENEMY_OBJECTIVE_DAMAGE, ENEMY_REGULAR_ATTACK, ENEMY_SPAWN_POINTS, EXTRA_ENEMIES_PER_WAVE,
    EnemyLockTarget, EnemySnapshot, EnemyType, FIXED_DT_SECONDS, FacingComponent,
    HERO_ABILITY_COOLDOWN_TICKS, HERO_ABILITY_DAMAGE, HERO_ABILITY_MANA_COST, HERO_ABILITY_RADIUS,
    HERO_CHARGE_PROFILE, HERO_COLLIDER_RADIUS, HERO_MANA_REGEN_PER_TICK, HERO_MAX_HP,
    HERO_MAX_MANA, HERO_POWER_DECAY_PROFILE, HERO_POWERED_MODIFIERS, HERO_REGULAR_ATTACK,
    HERO_SPEED, INITIAL_GOLD, INITIAL_TEAM_LIFE, JoinSnapshot, MATCH_RESET_TICKS, MAX_WAVES,
    MatchPhase, OBJECTIVE_COLLIDER_RADIUS, OBJECTIVE_MAX_HP, ObjectiveSnapshot,
    PowerDecayProfileComponent, PoweredUpModifiersComponent, ReliableGameEvent, SimMeta,
    TOWER_BUILD_COST, TOWER_COLLIDER_RADIUS, TOWER_DAMAGE, TOWER_RANGE, TOWER_RELOAD_TICKS,
    TowerSnapshot, TowerType, WAVE_PREP_TICKS, WAVE_SPAWN_INTERVAL_TICKS, WORLD_HALF_HEIGHT,
    WORLD_HALF_WIDTH, WorldDelta, cardinalize_dir_or, clamp_to_world, directional_attack_can_hit,
    distance_sq, enemy_collider_radius, is_newer_input_seq, normalize_or_zero,
};
use vleue_navigator::NavMesh;

const ENEMY_HERO_AGGRO_RANGE: f32 = 8.5;
const ENEMY_HERO_DISENGAGE_RANGE: f32 = 11.0;
const ENEMY_BASE_ENGAGE_RING_PADDING: f32 = 0.32;
const ENEMY_STEERING_TARGET_WEIGHT: f32 = 1.0;
const ENEMY_STEERING_SEPARATION_WEIGHT: f32 = 1.7;
const ENEMY_SEPARATION_BUFFER: f32 = 0.55;
const ENEMY_ACCEL_FACTOR: f32 = 18.0;
const ENEMY_DAMPING_FACTOR: f32 = 4.3;
const ENEMY_SPATIAL_CELL_MIN_SIZE: f32 = 0.5;
const ENEMY_REPATH_TICKS: u32 = 8;
const ENEMY_WAYPOINT_REACH_RADIUS: f32 = 0.65;
const ENEMY_COLLISION_DAMPING: f32 = 0.82;
const HERO_COLLISION_MAX_DISPLACEMENT_PER_TICK: f32 = HERO_SPEED * FIXED_DT_SECONDS * 0.8;
const NAVMESH_MARGIN: f32 = 0.15;
const TOWER_NAV_BLOCK_RADIUS: f32 = 1.05;
const TOWER_NAV_BLOCK_VERTICES: usize = 8;
const NAVMESH_SEARCH_DELTA: f32 = 0.015;
const NAVMESH_SEARCH_STEPS: u32 = 220;

#[derive(Debug, Clone)]
struct HeroState {
    pos: [f32; 2],
    move_dir: [f32; 2],
    facing: FacingComponent,
    lock_mode_active: bool,
    lock_target_id: Option<u64>,
    regular_attack: DirectionalAttackStateComponent,
    regular_attack_profile: DirectionalAttackComponent,
    charge_profile: ChargeProfileComponent,
    power_decay_profile: PowerDecayProfileComponent,
    powered_modifiers: PoweredUpModifiersComponent,
    charge_state: ChargeStateComponent,
    hp: f32,
    mana: f32,
    gold: u32,
    ability_cooldown_ticks: u32,
    last_processed_seq: Option<u32>,
    last_action_seq: Option<u32>,
    last_attack_seq: Option<u32>,
    pending_actions: VecDeque<game_shared::ClientAction>,
}

#[derive(Debug, Clone)]
struct EnemyState {
    id: u64,
    lane: u8,
    pos: [f32; 2],
    vel: [f32; 2],
    hp: f32,
    max_hp: f32,
    speed: f32,
    reward: u32,
    enemy_type: EnemyType,
    radius: f32,
    facing: FacingComponent,
    regular_attack: DirectionalAttackStateComponent,
    regular_attack_profile: DirectionalAttackComponent,
    lock_target: EnemyLockTarget,
    target_pos: [f32; 2],
    waypoint: [f32; 2],
    repath_cooldown: u32,
}

#[derive(Debug, Clone, Copy)]
enum EnemyTarget {
    Hero { client_id: u64, pos: [f32; 2] },
    Base,
}

#[derive(Debug, Clone, Copy)]
struct EnemyMotionSnapshot {
    id: u64,
    pos: [f32; 2],
    radius: f32,
}

#[derive(Debug, Clone, Copy)]
struct EnemyCollisionSnapshot {
    id: u64,
    pos: [f32; 2],
    radius: f32,
}

#[derive(Debug, Clone, Copy)]
struct TowerState {
    id: u64,
    owner: u64,
    lane: u8,
    node_id: u32,
    pos: [f32; 2],
    reload_ticks: u32,
}

#[derive(Debug, Clone)]
pub struct TickOutput {
    pub reliable_events: Vec<ReliableGameEvent>,
}

#[derive(Debug)]
#[cfg_attr(feature = "bevy", derive(bevy::prelude::Resource))]
pub struct Simulation {
    historical_targets: BTreeMap<(u64, u32), Vec<(u64, [f32; 2])>>,
    presentations: Vec<game_shared::PresentationInstance>,
    match_epoch: u32,
    tick: u32,
    phase: MatchPhase,
    team_life: i32,
    wave: u32,
    objective_hp: f32,
    heroes: BTreeMap<u64, HeroState>,
    enemies: BTreeMap<u64, EnemyState>,
    towers: BTreeMap<u64, TowerState>,
    node_occupancy: BTreeMap<u32, u64>,
    navmesh_cache: Option<NavMesh>,
    navmesh_dirty: bool,
    pending_commands: Vec<(u64, ClientCommand)>,
    move_queues: BTreeMap<u64, VecDeque<(u32, [f32; 2])>>,
    next_entity_id: u64,
    wave_remaining: u32,
    next_spawn_tick: u32,
    next_spawn_point_index: usize,
    intermission_until: u32,
    reset_at_tick: Option<u32>,
}

impl Default for Simulation {
    fn default() -> Self {
        Self::new()
    }
}

impl Simulation {
    pub fn new() -> Self {
        Self {
            historical_targets: BTreeMap::new(),
            presentations: Vec::new(),
            match_epoch: 0,
            tick: 0,
            phase: MatchPhase::InProgress,
            team_life: INITIAL_TEAM_LIFE,
            wave: 0,
            objective_hp: OBJECTIVE_MAX_HP,
            heroes: BTreeMap::new(),
            enemies: BTreeMap::new(),
            towers: BTreeMap::new(),
            node_occupancy: BTreeMap::new(),
            navmesh_cache: None,
            navmesh_dirty: true,
            pending_commands: Vec::new(),
            move_queues: BTreeMap::new(),
            next_entity_id: 1_000_000,
            wave_remaining: 0,
            next_spawn_tick: 0,
            next_spawn_point_index: 0,
            intermission_until: WAVE_PREP_TICKS,
            reset_at_tick: None,
        }
    }

    pub fn add_player(&mut self, client_id: u64) {
        if self.heroes.contains_key(&client_id) {
            return;
        }

        self.heroes.insert(
            client_id,
            HeroState {
                pos: spawn_position_for_client(client_id),
                move_dir: [0.0, 0.0],
                facing: FacingComponent::default(),
                lock_mode_active: false,
                lock_target_id: None,
                regular_attack: DirectionalAttackStateComponent::default(),
                regular_attack_profile: HERO_REGULAR_ATTACK,
                charge_profile: HERO_CHARGE_PROFILE,
                power_decay_profile: HERO_POWER_DECAY_PROFILE,
                powered_modifiers: HERO_POWERED_MODIFIERS,
                charge_state: ChargeStateComponent::default(),
                hp: HERO_MAX_HP,
                mana: HERO_MAX_MANA,
                gold: INITIAL_GOLD,
                ability_cooldown_ticks: 0,
                last_processed_seq: None,
                last_action_seq: None,
                last_attack_seq: None,
                pending_actions: VecDeque::new(),
            },
        );
        self.move_queues.insert(client_id, VecDeque::new());
    }

    pub fn remove_player(&mut self, client_id: u64) {
        self.heroes.remove(&client_id);
        self.move_queues.remove(&client_id);
        self.pending_commands.retain(|(id, _)| *id != client_id);

        let owned_towers: Vec<u64> = self
            .towers
            .values()
            .filter(|tower| tower.owner == client_id)
            .map(|tower| tower.id)
            .collect();

        let mut removed_any_tower = false;
        for tower_id in owned_towers {
            if let Some(tower) = self.towers.remove(&tower_id) {
                self.node_occupancy.remove(&tower.node_id);
                removed_any_tower = true;
            }
        }
        if removed_any_tower {
            self.navmesh_dirty = true;
        }
    }

    pub fn has_players(&self) -> bool {
        !self.heroes.is_empty()
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn join_snapshot(&self, client_id: u64) -> JoinSnapshot {
        JoinSnapshot {
            you: client_id,
            build_nodes: BUILD_NODES.to_vec(),
            world: self.world_delta_for(client_id),
        }
    }

    pub fn world_delta_for(&self, client_id: u64) -> WorldDelta {
        let mut heroes = Vec::with_capacity(self.heroes.len());
        for (id, hero) in &self.heroes {
            heroes.push(game_shared::HeroSnapshot {
                client_id: *id,
                pos: hero.pos,
                facing: hero.facing,
                lock_mode_active: hero.lock_mode_active,
                lock_target_id: hero.lock_target_id,
                regular_attack: hero.regular_attack,
                charge_profile: hero.charge_profile,
                power_decay_profile: hero.power_decay_profile,
                powered_modifiers: hero.powered_modifiers,
                charge_state: hero.charge_state,
                hp: hero.hp,
                mana: hero.mana,
                gold: hero.gold,
                ability_cooldown_ticks: hero.ability_cooldown_ticks,
                move_dir: hero.move_dir,
                pending_moves: self
                    .move_queues
                    .get(id)
                    .map_or_else(Vec::new, |q| q.iter().copied().collect()),
                last_move_seq: hero.last_processed_seq,
                last_action_seq: hero.last_action_seq,
                last_attack_seq: hero.last_attack_seq,
                pending_actions: hero.pending_actions.iter().copied().collect(),
            });
        }
        heroes.sort_unstable_by_key(|hero| hero.client_id);

        let mut enemies = Vec::with_capacity(self.enemies.len());
        for enemy in self.enemies.values() {
            enemies.push(EnemySnapshot {
                id: enemy.id,
                lane: enemy.lane,
                pos: enemy.pos,
                vel: enemy.vel,
                facing: enemy.facing,
                regular_attack: enemy.regular_attack,
                hp: enemy.hp.max(0.0),
                max_hp: enemy.max_hp,
                enemy_type: enemy.enemy_type,
                speed: enemy.speed,
                reward: enemy.reward,
                lock_target: enemy.lock_target,
                target_pos: enemy.target_pos,
                waypoint: enemy.waypoint,
                repath_cooldown: enemy.repath_cooldown,
            });
        }
        enemies.sort_unstable_by_key(|enemy| enemy.id);

        let mut towers = Vec::with_capacity(self.towers.len());
        for tower in self.towers.values() {
            towers.push(TowerSnapshot {
                id: tower.id,
                owner: tower.owner,
                lane: tower.lane,
                node_id: tower.node_id,
                pos: tower.pos,
                reload_ticks_remaining: tower.reload_ticks,
            });
        }
        towers.sort_unstable_by_key(|tower| tower.id);

        let objectives = vec![ObjectiveSnapshot {
            lane: 0,
            hp: self.objective_hp,
            max_hp: OBJECTIVE_MAX_HP,
        }];

        let your_last_input_seq = self
            .heroes
            .get(&client_id)
            .and_then(|hero| hero.last_processed_seq);

        WorldDelta {
            presentations: self.presentations.clone(),
            tick: self.tick,
            phase: self.phase,
            match_restart_ticks_remaining: self.match_restart_ticks_remaining(),
            wave: self.wave,
            team_life: self.team_life,
            objectives,
            heroes,
            enemies,
            towers,
            your_last_input_seq,
            sim_meta: Some(self.sim_meta()),
        }
    }

    pub fn sim_meta(&self) -> SimMeta {
        SimMeta {
            match_epoch: self.match_epoch,
            next_entity_id: self.next_entity_id,
            wave_remaining: self.wave_remaining,
            next_spawn_tick: self.next_spawn_tick,
            next_spawn_point_index: self.next_spawn_point_index as u8,
            intermission_until: self.intermission_until,
        }
    }

    pub fn presentations(&self) -> &[game_shared::PresentationInstance] {
        &self.presentations
    }

    pub fn set_tick(&mut self, tick: u32) {
        self.tick = tick;
    }

    pub fn from_snapshot(delta: &WorldDelta, meta: &SimMeta) -> Self {
        let mut sim = Self {
            historical_targets: BTreeMap::new(),
            presentations: delta.presentations.clone(),
            match_epoch: meta.match_epoch,
            tick: delta.tick,
            phase: delta.phase,
            team_life: delta.team_life,
            wave: delta.wave,
            objective_hp: delta
                .objectives
                .first()
                .map(|o| o.hp)
                .unwrap_or(OBJECTIVE_MAX_HP),
            heroes: BTreeMap::new(),
            enemies: BTreeMap::new(),
            towers: BTreeMap::new(),
            node_occupancy: BTreeMap::new(),
            navmesh_cache: None,
            navmesh_dirty: true,
            pending_commands: Vec::new(),
            move_queues: BTreeMap::new(),
            next_entity_id: meta.next_entity_id,
            wave_remaining: meta.wave_remaining,
            next_spawn_tick: meta.next_spawn_tick,
            next_spawn_point_index: meta.next_spawn_point_index as usize,
            intermission_until: meta.intermission_until,
            reset_at_tick: delta
                .match_restart_ticks_remaining
                .map(|remaining| delta.tick.wrapping_add(remaining)),
        };
        populate_from_delta(&mut sim, delta);
        sim
    }

    pub fn apply_snapshot(&mut self, delta: &WorldDelta, meta: &SimMeta) {
        self.historical_targets.clear();
        self.presentations.clone_from(&delta.presentations);
        self.match_epoch = meta.match_epoch;
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
        self.navmesh_dirty = true;
        self.pending_commands.clear();
        self.heroes.clear();
        self.move_queues.clear();
        self.enemies.clear();
        self.towers.clear();
        self.node_occupancy.clear();
        populate_from_delta(self, delta);
    }

    fn match_restart_ticks_remaining(&self) -> Option<u32> {
        self.reset_at_tick.map(|reset_at_tick| {
            if is_newer_input_seq(reset_at_tick, self.tick) {
                reset_at_tick.wrapping_sub(self.tick)
            } else {
                0
            }
        })
    }

    pub fn step(&mut self) -> TickOutput {
        self.tick = self.tick.wrapping_add(1);
        for instance in &mut self.presentations {
            instance.age_ticks += 1;
        }
        self.presentations
            .retain(|instance| instance.age_ticks < instance.duration_ticks);

        let mut reliable_events = Vec::new();
        if self.phase != MatchPhase::InProgress {
            self.maybe_reset_match();
            return TickOutput { reliable_events };
        }

        self.release_ready_actions();
        self.apply_pending_commands(&mut reliable_events);
        self.consume_move_queues();
        self.advance_enemies(&mut reliable_events);
        self.advance_heroes(&mut reliable_events);
        self.resolve_tower_attacks(&mut reliable_events);
        self.spawn_wave_units(&mut reliable_events);

        if self.team_life <= 0 && self.phase == MatchPhase::InProgress {
            self.phase = MatchPhase::Defeat;
            self.schedule_match_reset();
            reliable_events.push(ReliableGameEvent::Defeat);
        }

        self.historical_targets.clear();
        TickOutput { reliable_events }
    }

    fn try_cast_ability(
        &mut self,
        client_id: u64,
        ability: AbilityId,
        action_seq: u32,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        if ability != AbilityId::ArcBurst {
            return;
        }

        let Some(hero) = self.heroes.get_mut(&client_id) else {
            return;
        };

        let mana_cost = hero_ability_mana_cost(hero);
        if hero.ability_cooldown_ticks > 0
            || hero.mana < mana_cost
            || is_charge_locked(hero.charge_state)
        {
            return;
        }

        hero.ability_cooldown_ticks = hero_ability_cooldown_ticks(hero);
        hero.mana -= mana_cost;
        let cast_origin = hero.pos;
        let ability_radius = hero_ability_radius(hero);
        let damage_multiplier = hero_attack_damage_multiplier(hero);

        self.presentations.push(game_shared::PresentationInstance {
            id: game_shared::PresentationId {
                match_epoch: self.match_epoch,
                owner: client_id,
                action_seq,
                slot: 0,
            },
            kind: game_shared::PresentationKind::ArcBurst,
            pos: cast_origin,
            radius: ability_radius,
            age_ticks: 0,
            duration_ticks: 17,
        });

        reliable_events.push(ReliableGameEvent::AbilityCast {
            owner: client_id,
            pos: cast_origin,
            radius: ability_radius,
        });

        let mut targets = Vec::new();
        let radius_sq = ability_radius * ability_radius;
        if let Some(history) = self.historical_targets.remove(&(client_id, action_seq)) {
            for (id, pos) in history {
                // Historical geometry cannot resurrect a removed enemy or award
                // a second kill. All damage still applies to current state.
                if self.enemies.contains_key(&id) && distance_sq(pos, cast_origin) <= radius_sq {
                    targets.push(id);
                }
            }
        } else {
            for enemy in self.enemies.values() {
                if distance_sq(enemy.pos, cast_origin) <= radius_sq {
                    targets.push(enemy.id);
                }
            }
        }

        for enemy_id in targets {
            self.apply_enemy_damage(
                enemy_id,
                HERO_ABILITY_DAMAGE * damage_multiplier,
                client_id,
                reliable_events,
            );
        }
    }

    fn try_start_regular_attack(&mut self, client_id: u64) {
        let Some(hero) = self.heroes.get_mut(&client_id) else {
            return;
        };
        if is_charge_locked(hero.charge_state) {
            return;
        }

        let regular_attack_profile = hero_regular_attack_profile(hero);
        let _ = start_attack(&mut hero.regular_attack, regular_attack_profile);
    }

    fn try_set_charging(&mut self, client_id: u64, active: bool) {
        let Some(hero) = self.heroes.get_mut(&client_id) else {
            return;
        };
        set_charge_input(hero, active);
    }

    fn try_set_lock_target(&mut self, client_id: u64, target_id: Option<u64>) {
        let validated_target_id = match target_id {
            Some(enemy_id) if self.enemies.contains_key(&enemy_id) => Some(enemy_id),
            Some(_) => return,
            None => None,
        };

        let Some(hero) = self.heroes.get_mut(&client_id) else {
            return;
        };
        hero.lock_mode_active = validated_target_id.is_some();
        hero.lock_target_id = validated_target_id;
    }

    fn try_build_tower(
        &mut self,
        client_id: u64,
        node_id: u32,
        tower_type: TowerType,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        if tower_type != TowerType::Arrow {
            return;
        }

        let Some(node) = BUILD_NODES.iter().find(|node| node.node_id == node_id) else {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::InvalidNode,
            });
            return;
        };

        if self.node_occupancy.contains_key(&node.node_id) {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::Occupied,
            });
            return;
        }

        let Some(hero) = self.heroes.get(&client_id).cloned() else {
            return;
        };

        if distance_sq(hero.pos, node.pos) > BUILD_COMMAND_MAX_DISTANCE * BUILD_COMMAND_MAX_DISTANCE
        {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::TooFar,
            });
            return;
        }

        if hero.gold < TOWER_BUILD_COST {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::InsufficientGold,
            });
            return;
        }

        if let Some(hero_mut) = self.heroes.get_mut(&client_id) {
            hero_mut.gold -= TOWER_BUILD_COST;
        }

        let tower_id = self.alloc_entity_id();
        self.towers.insert(
            tower_id,
            TowerState {
                id: tower_id,
                owner: client_id,
                lane: 0,
                node_id: node.node_id,
                pos: node.pos,
                reload_ticks: 0,
            },
        );
        self.node_occupancy.insert(node.node_id, tower_id);
        self.navmesh_dirty = true;

        reliable_events.push(ReliableGameEvent::TowerBuilt {
            owner: client_id,
            node_id,
            tower_id,
        });
    }

    fn advance_heroes(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        let hero_ids: Vec<u64> = self.heroes.keys().copied().collect();
        let enemy_positions: BTreeMap<u64, [f32; 2]> = self
            .enemies
            .iter()
            .map(|(id, enemy)| (*id, enemy.pos))
            .collect();
        let mut triggered_attackers = Vec::new();

        for hero_id in hero_ids {
            let Some(hero) = self.heroes.get_mut(&hero_id) else {
                continue;
            };
            let regular_attack_profile = hero_regular_attack_profile(hero);

            if hero.lock_mode_active {
                if hero
                    .lock_target_id
                    .is_some_and(|target_id| !enemy_positions.contains_key(&target_id))
                {
                    hero.lock_target_id = nearest_enemy_id(hero.pos, &enemy_positions);
                } else if hero.lock_target_id.is_none() {
                    hero.lock_target_id = nearest_enemy_id(hero.pos, &enemy_positions);
                }

                if hero.lock_target_id.is_none() {
                    hero.lock_mode_active = false;
                }
            } else {
                hero.lock_target_id = None;
            }

            let attack_triggered =
                advance_attack_state(&mut hero.regular_attack, regular_attack_profile);
            if attack_triggered {
                triggered_attackers.push(hero_id);
            }
            advance_charge_state(hero);

            if let Some(target_id) = hero.lock_target_id {
                if let Some(target_pos) = enemy_positions.get(&target_id).copied() {
                    let to_target = normalize_or_zero([
                        target_pos[0] - hero.pos[0],
                        target_pos[1] - hero.pos[1],
                    ]);
                    if to_target != [0.0, 0.0] {
                        hero.facing.dir = to_target;
                    }
                }
            }

            let attack_locked = is_attack_locked(hero.regular_attack);
            let charge_locked = is_charge_locked(hero.charge_state);
            if !attack_locked && !charge_locked {
                if hero.move_dir != [0.0, 0.0]
                    && (!hero.lock_mode_active || hero.lock_target_id.is_none())
                {
                    hero.facing.dir = hero.move_dir;
                }

                hero.pos = clamp_to_world([
                    hero.pos[0] + hero.move_dir[0] * HERO_SPEED * FIXED_DT_SECONDS,
                    hero.pos[1] + hero.move_dir[1] * HERO_SPEED * FIXED_DT_SECONDS,
                ]);
            }

            hero.mana = (hero.mana + HERO_MANA_REGEN_PER_TICK).min(HERO_MAX_MANA);
            if hero.ability_cooldown_ticks > 0 {
                hero.ability_cooldown_ticks -= 1;
            }
        }

        self.resolve_hero_collisions();

        for attacker_id in triggered_attackers {
            let Some(hero) = self.heroes.get(&attacker_id).cloned() else {
                continue;
            };
            let regular_attack_profile = hero_regular_attack_profile(&hero);

            let mut hit_targets: Vec<(u64, f32)> = self
                .enemies
                .values()
                .filter_map(|enemy| {
                    directional_attack_can_hit(
                        hero.pos,
                        hero.facing.dir,
                        enemy.pos,
                        regular_attack_profile,
                    )
                    .then_some((enemy.id, distance_sq(hero.pos, enemy.pos)))
                })
                .collect();

            if hit_targets.is_empty() {
                continue;
            }

            hit_targets.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

            if hero.lock_mode_active
                && let Some(locked_target_id) = hero.lock_target_id
                && let Some(locked_idx) = hit_targets
                    .iter()
                    .position(|(enemy_id, _)| *enemy_id == locked_target_id)
            {
                hit_targets.swap(0, locked_idx);
            }

            let damage = regular_attack_profile.damage * hero_attack_damage_multiplier(&hero);
            for (enemy_id, _) in hit_targets {
                self.apply_enemy_damage(enemy_id, damage, attacker_id, reliable_events);
            }
        }
    }

    fn resolve_hero_collisions(&mut self) {
        #[derive(Clone, Copy)]
        struct HeroCollisionSnapshot {
            client_id: u64,
            pos: [f32; 2],
        }

        #[derive(Clone, Copy)]
        struct EnemyCollisionSnapshot {
            pos: [f32; 2],
            radius: f32,
        }

        let hero_snapshots: Vec<HeroCollisionSnapshot> = self
            .heroes
            .iter()
            .map(|(client_id, hero)| HeroCollisionSnapshot {
                client_id: *client_id,
                pos: hero.pos,
            })
            .collect();
        let enemy_snapshots: Vec<EnemyCollisionSnapshot> = self
            .enemies
            .values()
            .map(|enemy| EnemyCollisionSnapshot {
                pos: enemy.pos,
                radius: enemy.radius,
            })
            .collect();

        let mut displacements: BTreeMap<u64, [f32; 2]> = BTreeMap::new();

        for i in 0..hero_snapshots.len() {
            for j in (i + 1)..hero_snapshots.len() {
                let a = hero_snapshots[i];
                let b = hero_snapshots[j];

                let mut delta = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS * 2.0;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    let angle = ((a.client_id ^ (b.client_id << 1)) % 6283) as f32 * 0.001;
                    delta = [angle.cos(), angle.sin()];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                let push = overlap * 0.5;

                add_displacement(
                    &mut displacements,
                    a.client_id,
                    [-normal[0] * push, -normal[1] * push],
                );
                add_displacement(
                    &mut displacements,
                    b.client_id,
                    [normal[0] * push, normal[1] * push],
                );
            }
        }

        for snapshot in &hero_snapshots {
            for tower in self.towers.values() {
                let mut delta = [
                    snapshot.pos[0] - tower.pos[0],
                    snapshot.pos[1] - tower.pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS + TOWER_COLLIDER_RADIUS;
                let min_dist_sq = min_dist * min_dist;
                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.client_id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        for snapshot in &hero_snapshots {
            let mut delta = [
                snapshot.pos[0] - BASE_POSITION[0],
                snapshot.pos[1] - BASE_POSITION[1],
            ];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let min_dist = HERO_COLLIDER_RADIUS + OBJECTIVE_COLLIDER_RADIUS;
            let min_dist_sq = min_dist * min_dist;
            if dist_sq >= min_dist_sq {
                continue;
            }

            if dist_sq <= f32::EPSILON {
                delta = [1.0, 0.0];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let normal = [delta[0] / dist, delta[1] / dist];
            let overlap = (min_dist - dist).max(0.0);
            add_displacement(
                &mut displacements,
                snapshot.client_id,
                [normal[0] * overlap, normal[1] * overlap],
            );
        }

        for snapshot in &hero_snapshots {
            for enemy in &enemy_snapshots {
                let mut delta = [
                    snapshot.pos[0] - enemy.pos[0],
                    snapshot.pos[1] - enemy.pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = HERO_COLLIDER_RADIUS + enemy.radius;
                let min_dist_sq = min_dist * min_dist;
                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.client_id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        for (client_id, displacement) in displacements {
            if let Some(hero) = self.heroes.get_mut(&client_id) {
                let limited =
                    clamp_vector_length(displacement, HERO_COLLISION_MAX_DISPLACEMENT_PER_TICK);
                hero.pos = clamp_to_world([hero.pos[0] + limited[0], hero.pos[1] + limited[1]]);
            }
        }
    }

    fn advance_enemies(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        let navmesh = self.navmesh_for_tick();
        let hero_targets: Vec<(u64, [f32; 2])> = self
            .heroes
            .iter()
            .map(|(id, hero)| (*id, hero.pos))
            .collect();
        let motion_snapshots: Vec<EnemyMotionSnapshot> = self
            .enemies
            .values()
            .map(|enemy| EnemyMotionSnapshot {
                id: enemy.id,
                pos: enemy.pos,
                radius: enemy.radius,
            })
            .collect();
        let separation_cell_size = separation_spatial_cell_size(&motion_snapshots);
        let separation_cells =
            build_spatial_cells(&motion_snapshots, separation_cell_size, |snapshot| {
                snapshot.pos
            });
        let mut pending_hero_hits: Vec<(u64, f32)> = Vec::new();
        let mut pending_objective_hits: Vec<u64> = Vec::new();

        for enemy in self.enemies.values_mut() {
            enemy.lock_target = choose_enemy_target(enemy.pos, enemy.lock_target, &hero_targets);
            let target = resolve_enemy_target(enemy.lock_target, &hero_targets);
            let target_pos = enemy_target_position(target);
            let previous_target = enemy.target_pos;
            enemy.target_pos =
                enemy_target_attack_anchor(target, enemy.id, enemy.regular_attack_profile.range);
            let facing_to_target =
                normalize_or_zero([target_pos[0] - enemy.pos[0], target_pos[1] - enemy.pos[1]]);
            if facing_to_target != [0.0, 0.0] {
                // AI lock-on: face the active attack target (hero/base), not the steering anchor.
                enemy.facing.dir = facing_to_target;
            }

            let attack_triggered =
                advance_attack_state(&mut enemy.regular_attack, enemy.regular_attack_profile);
            if attack_triggered {
                match target {
                    EnemyTarget::Hero { client_id, pos } => {
                        if directional_attack_can_hit(
                            enemy.pos,
                            enemy.facing.dir,
                            pos,
                            enemy.regular_attack_profile,
                        ) {
                            pending_hero_hits
                                .push((client_id, enemy.regular_attack_profile.damage));
                        }
                    }
                    EnemyTarget::Base => {
                        if directional_attack_can_hit(
                            enemy.pos,
                            enemy.facing.dir,
                            BASE_POSITION,
                            enemy.regular_attack_profile,
                        ) {
                            pending_objective_hits.push(enemy.id);
                        }
                    }
                }
            }

            if is_attack_locked(enemy.regular_attack) {
                enemy.vel = [0.0, 0.0];
                continue;
            }

            if distance_sq(enemy.pos, target_pos)
                <= enemy.regular_attack_profile.range * enemy.regular_attack_profile.range
            {
                let _ = start_attack(&mut enemy.regular_attack, enemy.regular_attack_profile);
                enemy.vel = [0.0, 0.0];
                continue;
            }

            let target_changed = distance_sq(enemy.target_pos, previous_target) > 0.5;

            let reached_waypoint =
                distance_sq(enemy.pos, enemy.waypoint) <= ENEMY_WAYPOINT_REACH_RADIUS.powi(2);
            if enemy.repath_cooldown == 0 || target_changed || reached_waypoint {
                enemy.waypoint = find_next_waypoint(enemy.pos, enemy.target_pos, &navmesh)
                    .unwrap_or(enemy.target_pos);
                enemy.repath_cooldown = ENEMY_REPATH_TICKS;
            } else {
                enemy.repath_cooldown -= 1;
            }

            let mut desired = normalize_or_zero([
                enemy.waypoint[0] - enemy.pos[0],
                enemy.waypoint[1] - enemy.pos[1],
            ]);
            if desired == [0.0, 0.0] {
                desired = normalize_or_zero([
                    enemy.target_pos[0] - enemy.pos[0],
                    enemy.target_pos[1] - enemy.pos[1],
                ]);
            }

            let separation = enemy_separation_direction(
                enemy.id,
                enemy.pos,
                enemy.radius,
                &motion_snapshots,
                &separation_cells,
                separation_cell_size,
            );
            let steer = normalize_or_zero([
                desired[0] * ENEMY_STEERING_TARGET_WEIGHT
                    + separation[0] * ENEMY_STEERING_SEPARATION_WEIGHT,
                desired[1] * ENEMY_STEERING_TARGET_WEIGHT
                    + separation[1] * ENEMY_STEERING_SEPARATION_WEIGHT,
            ]);

            enemy.vel[0] += steer[0] * enemy.speed * ENEMY_ACCEL_FACTOR * FIXED_DT_SECONDS;
            enemy.vel[1] += steer[1] * enemy.speed * ENEMY_ACCEL_FACTOR * FIXED_DT_SECONDS;

            let damping = (1.0 - ENEMY_DAMPING_FACTOR * FIXED_DT_SECONDS).clamp(0.0, 1.0);
            enemy.vel[0] *= damping;
            enemy.vel[1] *= damping;

            let vel_len_sq = enemy.vel[0] * enemy.vel[0] + enemy.vel[1] * enemy.vel[1];
            let max_len_sq = enemy.speed * enemy.speed;
            if vel_len_sq > max_len_sq {
                let scale = (max_len_sq / vel_len_sq).sqrt();
                enemy.vel[0] *= scale;
                enemy.vel[1] *= scale;
            }

            enemy.pos = clamp_to_world([
                enemy.pos[0] + enemy.vel[0] * FIXED_DT_SECONDS,
                enemy.pos[1] + enemy.vel[1] * FIXED_DT_SECONDS,
            ]);
        }

        self.resolve_enemy_collisions();

        for (hero_id, damage) in pending_hero_hits {
            self.apply_hero_damage(hero_id, damage);
        }

        for enemy_id in pending_objective_hits {
            if self.enemies.remove(&enemy_id).is_none() {
                continue;
            }
            self.apply_objective_damage(reliable_events);
        }
    }

    fn apply_hero_damage(&mut self, hero_id: u64, damage: f32) {
        let Some(hero) = self.heroes.get_mut(&hero_id) else {
            return;
        };

        hero.hp = (hero.hp - damage).max(0.0);
        if hero.hp > 0.0 {
            return;
        }

        hero.pos = spawn_position_for_client(hero_id);
        hero.move_dir = [0.0, 0.0];
        hero.facing = FacingComponent::default();
        hero.lock_mode_active = false;
        hero.lock_target_id = None;
        hero.regular_attack = DirectionalAttackStateComponent::default();
        hero.charge_state = ChargeStateComponent::default();
        hero.hp = HERO_MAX_HP;
        hero.mana = HERO_MAX_MANA;
    }

    fn apply_objective_damage(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        self.objective_hp -= ENEMY_OBJECTIVE_DAMAGE;
        if self.objective_hp <= 0.0 {
            self.objective_hp = OBJECTIVE_MAX_HP;
            self.team_life -= 1;
        }

        reliable_events.push(ReliableGameEvent::ObjectiveDamaged {
            lane: 0,
            hp: self.objective_hp,
            team_life: self.team_life,
        });
    }

    fn resolve_enemy_collisions(&mut self) {
        let snapshots: Vec<EnemyCollisionSnapshot> = self
            .enemies
            .values()
            .map(|enemy| EnemyCollisionSnapshot {
                id: enemy.id,
                pos: enemy.pos,
                radius: enemy.radius,
            })
            .collect();
        let collision_cell_size = collision_spatial_cell_size(&snapshots);
        let collision_cells =
            build_spatial_cells(&snapshots, collision_cell_size, |snapshot| snapshot.pos);

        let mut displacements: BTreeMap<u64, [f32; 2]> = BTreeMap::new();

        for i in 0..snapshots.len() {
            let a = snapshots[i];
            for_each_spatial_neighbor(&collision_cells, collision_cell_size, a.pos, |j| {
                if j <= i {
                    return;
                }
                let b = snapshots[j];

                let mut delta = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = a.radius + b.radius;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    return;
                }

                if dist_sq <= f32::EPSILON {
                    let angle = ((a.id ^ (b.id << 1)) % 6283) as f32 * 0.001;
                    delta = [angle.cos(), angle.sin()];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                let push = overlap * 0.5;

                add_displacement(
                    &mut displacements,
                    a.id,
                    [-normal[0] * push, -normal[1] * push],
                );
                add_displacement(
                    &mut displacements,
                    b.id,
                    [normal[0] * push, normal[1] * push],
                );
            });
        }

        for snapshot in &snapshots {
            for tower in self.towers.values() {
                let mut delta = [
                    snapshot.pos[0] - tower.pos[0],
                    snapshot.pos[1] - tower.pos[1],
                ];
                let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
                let min_dist = snapshot.radius + TOWER_COLLIDER_RADIUS;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq >= min_dist_sq {
                    continue;
                }

                if dist_sq <= f32::EPSILON {
                    delta = [1.0, 0.0];
                    dist_sq = 1.0;
                }

                let dist = dist_sq.sqrt();
                let normal = [delta[0] / dist, delta[1] / dist];
                let overlap = (min_dist - dist).max(0.0);
                add_displacement(
                    &mut displacements,
                    snapshot.id,
                    [normal[0] * overlap, normal[1] * overlap],
                );
            }
        }

        for snapshot in &snapshots {
            let mut delta = [
                snapshot.pos[0] - BASE_POSITION[0],
                snapshot.pos[1] - BASE_POSITION[1],
            ];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let min_dist = snapshot.radius + OBJECTIVE_COLLIDER_RADIUS;
            let min_dist_sq = min_dist * min_dist;

            if dist_sq >= min_dist_sq {
                continue;
            }

            if dist_sq <= f32::EPSILON {
                delta = [1.0, 0.0];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let normal = [delta[0] / dist, delta[1] / dist];
            let overlap = (min_dist - dist).max(0.0);
            add_displacement(
                &mut displacements,
                snapshot.id,
                [normal[0] * overlap, normal[1] * overlap],
            );
        }

        for (enemy_id, displacement) in displacements {
            if let Some(enemy) = self.enemies.get_mut(&enemy_id) {
                enemy.pos = clamp_to_world([
                    enemy.pos[0] + displacement[0],
                    enemy.pos[1] + displacement[1],
                ]);

                if displacement[0].abs() > 0.0001 || displacement[1].abs() > 0.0001 {
                    enemy.vel[0] *= ENEMY_COLLISION_DAMPING;
                    enemy.vel[1] *= ENEMY_COLLISION_DAMPING;
                }
            }
        }
    }

    fn resolve_tower_attacks(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        let mut pending_shots = Vec::new();

        for tower in self.towers.values_mut() {
            if tower.reload_ticks > 0 {
                tower.reload_ticks -= 1;
                continue;
            }

            let mut best_enemy = None;
            let mut best_distance = f32::INFINITY;
            for enemy in self.enemies.values() {
                let dist_sq = distance_sq(enemy.pos, tower.pos);
                if dist_sq > TOWER_RANGE * TOWER_RANGE {
                    continue;
                }

                if dist_sq < best_distance {
                    best_distance = dist_sq;
                    best_enemy = Some(enemy.id);
                }
            }

            if let Some(enemy_id) = best_enemy {
                tower.reload_ticks = TOWER_RELOAD_TICKS;
                pending_shots.push((enemy_id, tower.owner));
            }
        }

        for (enemy_id, owner) in pending_shots {
            self.apply_enemy_damage(enemy_id, TOWER_DAMAGE, owner, reliable_events);
        }
    }

    fn apply_enemy_damage(
        &mut self,
        enemy_id: u64,
        damage: f32,
        killer: u64,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        let mut reward = None;

        if let Some(enemy) = self.enemies.get_mut(&enemy_id) {
            enemy.hp -= damage;
            if enemy.hp <= 0.0 {
                reward = Some(enemy.reward);
            }
        }

        let Some(reward) = reward else {
            return;
        };

        if self.enemies.remove(&enemy_id).is_none() {
            return;
        }

        if let Some(hero) = self.heroes.get_mut(&killer) {
            hero.gold = hero.gold.saturating_add(reward);
        }

        reliable_events.push(ReliableGameEvent::EnemyKilled {
            enemy_id,
            killer,
            reward,
        });
    }

    fn spawn_wave_units(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        if is_newer_input_seq(self.intermission_until, self.tick) {
            return;
        }

        if self.wave_remaining == 0 {
            if !self.enemies.is_empty() {
                return;
            }

            if self.wave >= MAX_WAVES {
                self.phase = MatchPhase::Victory;
                self.schedule_match_reset();
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
            self.spawn_enemy(spawn_idx, self.wave);
            self.wave_remaining -= 1;
            self.next_spawn_point_index = self.next_spawn_point_index.wrapping_add(1);
        }

        self.next_spawn_tick = self.tick.wrapping_add(WAVE_SPAWN_INTERVAL_TICKS);

        if self.wave_remaining == 0 {
            self.intermission_until = self.tick.wrapping_add(WAVE_PREP_TICKS);
        }
    }

    fn spawn_enemy(&mut self, spawn_point_index: usize, wave: u32) {
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
        let enemy_id = self.alloc_entity_id();
        self.enemies.insert(
            enemy_id,
            EnemyState {
                id: enemy_id,
                lane: 0,
                pos: spawn_pos,
                vel: [0.0, 0.0],
                hp: max_hp,
                max_hp,
                speed,
                reward,
                enemy_type,
                radius,
                facing: FacingComponent {
                    dir: initial_facing,
                },
                regular_attack: DirectionalAttackStateComponent::default(),
                regular_attack_profile: ENEMY_REGULAR_ATTACK,
                lock_target: EnemyLockTarget::Base,
                target_pos: home,
                waypoint: home,
                repath_cooldown: 0,
            },
        );
    }

    fn alloc_entity_id(&mut self) -> u64 {
        let next = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.wrapping_add(1);
        next
    }

    fn schedule_match_reset(&mut self) {
        if self.reset_at_tick.is_some() {
            return;
        }
        self.reset_at_tick = Some(self.tick.wrapping_add(MATCH_RESET_TICKS));
    }

    fn maybe_reset_match(&mut self) {
        let Some(reset_at_tick) = self.reset_at_tick else {
            return;
        };
        if is_newer_input_seq(reset_at_tick, self.tick) {
            return;
        }
        self.reset_match_state();
    }

    fn reset_match_state(&mut self) {
        self.historical_targets.clear();
        self.presentations.clear();
        self.match_epoch = self.match_epoch.wrapping_add(1);
        self.phase = MatchPhase::InProgress;
        self.team_life = INITIAL_TEAM_LIFE;
        self.wave = 0;
        self.objective_hp = OBJECTIVE_MAX_HP;
        self.enemies.clear();
        self.towers.clear();
        self.node_occupancy.clear();
        self.navmesh_dirty = true;
        self.pending_commands.clear();
        for hero in self.heroes.values_mut() {
            hero.pending_actions.clear();
        }
        for queue in self.move_queues.values_mut() {
            queue.clear();
        }
        self.wave_remaining = 0;
        self.next_spawn_tick = 0;
        self.next_spawn_point_index = 0;
        self.intermission_until = self.tick.wrapping_add(WAVE_PREP_TICKS);
        self.reset_at_tick = None;

        for (client_id, hero) in &mut self.heroes {
            hero.pos = spawn_position_for_client(*client_id);
            hero.move_dir = [0.0, 0.0];
            hero.facing = FacingComponent::default();
            hero.lock_mode_active = false;
            hero.lock_target_id = None;
            hero.regular_attack = DirectionalAttackStateComponent::default();
            hero.regular_attack_profile = HERO_REGULAR_ATTACK;
            hero.charge_profile = HERO_CHARGE_PROFILE;
            hero.power_decay_profile = HERO_POWER_DECAY_PROFILE;
            hero.powered_modifiers = HERO_POWERED_MODIFIERS;
            hero.charge_state = ChargeStateComponent::default();
            hero.hp = HERO_MAX_HP;
            hero.mana = HERO_MAX_MANA;
            hero.gold = INITIAL_GOLD;
            hero.ability_cooldown_ticks = 0;
        }
    }

    fn navmesh_for_tick(&mut self) -> NavMesh {
        if self.navmesh_dirty || self.navmesh_cache.is_none() {
            self.navmesh_cache = Some(build_navigation_mesh(&self.towers));
            self.navmesh_dirty = false;
        }

        self.navmesh_cache
            .as_ref()
            .cloned()
            .unwrap_or_else(|| build_navigation_mesh(&self.towers))
    }
}

fn populate_from_delta(sim: &mut Simulation, delta: &WorldDelta) {
    for hero_snap in &delta.heroes {
        sim.heroes.insert(
            hero_snap.client_id,
            HeroState {
                pos: hero_snap.pos,
                move_dir: hero_snap.move_dir,
                facing: hero_snap.facing,
                lock_mode_active: hero_snap.lock_mode_active,
                lock_target_id: hero_snap.lock_target_id,
                regular_attack: hero_snap.regular_attack,
                regular_attack_profile: HERO_REGULAR_ATTACK,
                charge_profile: hero_snap.charge_profile,
                power_decay_profile: hero_snap.power_decay_profile,
                powered_modifiers: hero_snap.powered_modifiers,
                charge_state: hero_snap.charge_state,
                hp: hero_snap.hp,
                mana: hero_snap.mana,
                gold: hero_snap.gold,
                ability_cooldown_ticks: hero_snap.ability_cooldown_ticks,
                last_processed_seq: hero_snap.last_move_seq,
                last_action_seq: hero_snap.last_action_seq,
                last_attack_seq: hero_snap.last_attack_seq,
                pending_actions: hero_snap.pending_actions.iter().copied().collect(),
            },
        );
        sim.move_queues.insert(
            hero_snap.client_id,
            hero_snap.pending_moves.iter().copied().collect(),
        );
    }

    for enemy_snap in &delta.enemies {
        let radius = enemy_collider_radius(enemy_snap.enemy_type);

        sim.enemies.insert(
            enemy_snap.id,
            EnemyState {
                id: enemy_snap.id,
                lane: enemy_snap.lane,
                pos: enemy_snap.pos,
                vel: enemy_snap.vel,
                hp: enemy_snap.hp,
                max_hp: enemy_snap.max_hp,
                speed: enemy_snap.speed,
                reward: enemy_snap.reward,
                enemy_type: enemy_snap.enemy_type,
                radius,
                facing: enemy_snap.facing,
                regular_attack: enemy_snap.regular_attack,
                regular_attack_profile: ENEMY_REGULAR_ATTACK,
                lock_target: enemy_snap.lock_target,
                target_pos: enemy_snap.target_pos,
                waypoint: enemy_snap.waypoint,
                repath_cooldown: enemy_snap.repath_cooldown,
            },
        );
    }

    for tower_snap in &delta.towers {
        sim.towers.insert(
            tower_snap.id,
            TowerState {
                id: tower_snap.id,
                owner: tower_snap.owner,
                lane: tower_snap.lane,
                node_id: tower_snap.node_id,
                pos: tower_snap.pos,
                reload_ticks: tower_snap.reload_ticks_remaining,
            },
        );
        sim.node_occupancy.insert(tower_snap.node_id, tower_snap.id);
    }
}

fn enemy_target_position(target: EnemyTarget) -> [f32; 2] {
    match target {
        EnemyTarget::Hero { pos, .. } => pos,
        EnemyTarget::Base => BASE_POSITION,
    }
}

fn choose_enemy_target(
    enemy_pos: [f32; 2],
    current_target: EnemyLockTarget,
    hero_targets: &[(u64, [f32; 2])],
) -> EnemyLockTarget {
    let hero_range_sq = ENEMY_HERO_AGGRO_RANGE * ENEMY_HERO_AGGRO_RANGE;
    let hero_disengage_sq = ENEMY_HERO_DISENGAGE_RANGE * ENEMY_HERO_DISENGAGE_RANGE;

    if let EnemyLockTarget::Hero(current_hero_id) = current_target {
        if let Some((_, current_hero_pos)) = hero_targets
            .iter()
            .find(|(hero_id, _)| *hero_id == current_hero_id)
        {
            if distance_sq(enemy_pos, *current_hero_pos) <= hero_disengage_sq {
                return current_target;
            }
        }
    }

    let mut best: Option<(u64, f32)> = None;
    for (hero_id, hero_pos) in hero_targets {
        let dist_sq = distance_sq(enemy_pos, *hero_pos);
        if dist_sq > hero_range_sq {
            continue;
        }

        match best {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => best = Some((*hero_id, dist_sq)),
        }
    }

    if let Some((client_id, _)) = best {
        EnemyLockTarget::Hero(client_id)
    } else {
        EnemyLockTarget::Base
    }
}

fn resolve_enemy_target(target: EnemyLockTarget, hero_targets: &[(u64, [f32; 2])]) -> EnemyTarget {
    match target {
        EnemyLockTarget::Hero(client_id) => hero_targets
            .iter()
            .find(|(hero_id, _)| *hero_id == client_id)
            .map(|(_, pos)| EnemyTarget::Hero {
                client_id,
                pos: *pos,
            })
            .unwrap_or(EnemyTarget::Base),
        EnemyLockTarget::Base => EnemyTarget::Base,
    }
}

fn enemy_target_attack_anchor(target: EnemyTarget, enemy_id: u64, attack_range: f32) -> [f32; 2] {
    let target_pos = enemy_target_position(target);
    let offset_radius = match target {
        EnemyTarget::Hero { .. } => (attack_range - 0.18).max(0.7),
        EnemyTarget::Base => (attack_range + ENEMY_BASE_ENGAGE_RING_PADDING).max(0.9),
    };

    let angle = enemy_slot_angle(enemy_id);
    clamp_to_world([
        target_pos[0] + angle.cos() * offset_radius,
        target_pos[1] + angle.sin() * offset_radius,
    ])
}

fn enemy_slot_angle(enemy_id: u64) -> f32 {
    // Golden-angle distribution produces stable spread around a target.
    let golden_angle = 2.399_963_1_f32;
    ((enemy_id as f32) * golden_angle) % TAU
}

fn enemy_separation_direction(
    enemy_id: u64,
    enemy_pos: [f32; 2],
    enemy_radius: f32,
    snapshots: &[EnemyMotionSnapshot],
    separation_cells: &BTreeMap<(i32, i32), Vec<usize>>,
    separation_cell_size: f32,
) -> [f32; 2] {
    let mut separation = [0.0, 0.0];

    for_each_spatial_neighbor(
        separation_cells,
        separation_cell_size,
        enemy_pos,
        |other_idx| {
            let other = snapshots[other_idx];
            if other.id == enemy_id {
                return;
            }

            let mut delta = [enemy_pos[0] - other.pos[0], enemy_pos[1] - other.pos[1]];
            let mut dist_sq = delta[0] * delta[0] + delta[1] * delta[1];
            let avoid_distance = enemy_radius + other.radius + ENEMY_SEPARATION_BUFFER;
            let avoid_distance_sq = avoid_distance * avoid_distance;
            if dist_sq >= avoid_distance_sq {
                return;
            }

            if dist_sq <= f32::EPSILON {
                let angle = ((enemy_id ^ (other.id << 1)) % 6283) as f32 * 0.001;
                delta = [angle.cos(), angle.sin()];
                dist_sq = 1.0;
            }

            let dist = dist_sq.sqrt();
            let weight = ((avoid_distance - dist) / avoid_distance).clamp(0.0, 1.0);
            separation[0] += (delta[0] / dist) * weight;
            separation[1] += (delta[1] / dist) * weight;
        },
    );

    normalize_or_zero(separation)
}

fn separation_spatial_cell_size(snapshots: &[EnemyMotionSnapshot]) -> f32 {
    let max_radius = snapshots
        .iter()
        .map(|snapshot| snapshot.radius)
        .fold(0.0, f32::max);
    (max_radius * 2.0 + ENEMY_SEPARATION_BUFFER).max(ENEMY_SPATIAL_CELL_MIN_SIZE)
}

fn collision_spatial_cell_size(snapshots: &[EnemyCollisionSnapshot]) -> f32 {
    let max_radius = snapshots
        .iter()
        .map(|snapshot| snapshot.radius)
        .fold(0.0, f32::max);
    (max_radius * 2.0).max(ENEMY_SPATIAL_CELL_MIN_SIZE)
}

fn build_spatial_cells<T>(
    values: &[T],
    cell_size: f32,
    pos_of: impl Fn(&T) -> [f32; 2],
) -> BTreeMap<(i32, i32), Vec<usize>> {
    let mut cells: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let cell = spatial_cell_key(pos_of(value), cell_size);
        cells.entry(cell).or_default().push(index);
    }
    cells
}

fn for_each_spatial_neighbor(
    cells: &BTreeMap<(i32, i32), Vec<usize>>,
    cell_size: f32,
    pos: [f32; 2],
    mut f: impl FnMut(usize),
) {
    let (cx, cy) = spatial_cell_key(pos, cell_size);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let key = (cx + dx, cy + dy);
            let Some(indices) = cells.get(&key) else {
                continue;
            };
            for &index in indices {
                f(index);
            }
        }
    }
}

fn spatial_cell_key(pos: [f32; 2], cell_size: f32) -> (i32, i32) {
    (
        (pos[0] / cell_size).floor() as i32,
        (pos[1] / cell_size).floor() as i32,
    )
}

fn is_charge_locked(state: ChargeStateComponent) -> bool {
    state.phase != ChargePhase::Idle
}

fn set_charge_input(hero: &mut HeroState, active: bool) {
    if active {
        if is_attack_locked(hero.regular_attack) || hero.charge_state.power_active {
            return;
        }

        match hero.charge_state.phase {
            ChargePhase::Idle => {
                hero.charge_state.input_held = true;
                let startup_ticks = hero.charge_profile.startup_ticks;
                if startup_ticks == 0 {
                    hero.charge_state.phase = ChargePhase::Charging;
                    hero.charge_state.phase_ticks_remaining = 0;
                } else {
                    hero.charge_state.phase = ChargePhase::Startup;
                    hero.charge_state.phase_ticks_remaining = startup_ticks;
                }
            }
            ChargePhase::Startup | ChargePhase::Charging => {
                hero.charge_state.input_held = true;
            }
            ChargePhase::Recovery => {}
        }
        return;
    }

    hero.charge_state.input_held = false;
    if matches!(
        hero.charge_state.phase,
        ChargePhase::Startup | ChargePhase::Charging
    ) {
        begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
    }
}

fn begin_charge_recovery(state: &mut ChargeStateComponent, profile: ChargeProfileComponent) {
    let release_lag_ticks = profile.release_lag_ticks;
    if release_lag_ticks == 0 {
        state.phase = ChargePhase::Idle;
        state.phase_ticks_remaining = 0;
    } else {
        state.phase = ChargePhase::Recovery;
        state.phase_ticks_remaining = release_lag_ticks;
    }
}

fn advance_charge_state(hero: &mut HeroState) {
    match hero.charge_state.phase {
        ChargePhase::Idle => {}
        ChargePhase::Startup => {
            if hero.charge_state.phase_ticks_remaining > 0 {
                hero.charge_state.phase_ticks_remaining -= 1;
            }

            if hero.charge_state.phase_ticks_remaining == 0 {
                if hero.charge_state.input_held {
                    hero.charge_state.phase = ChargePhase::Charging;
                } else {
                    begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                }
            }
        }
        ChargePhase::Charging => {
            if !hero.charge_state.input_held {
                begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                return;
            }

            if hero.mana + f32::EPSILON < HERO_MAX_MANA {
                hero.mana = (hero.mana + hero.charge_profile.mana_per_tick).min(HERO_MAX_MANA);
                return;
            }

            if hero.charge_state.power_meter + f32::EPSILON < hero.charge_profile.power_meter_max {
                let previous_power_meter = hero.charge_state.power_meter;
                hero.charge_state.power_meter = (hero.charge_state.power_meter
                    + hero.charge_profile.power_gain_per_tick)
                    .min(hero.charge_profile.power_meter_max);
                if previous_power_meter <= f32::EPSILON
                    && hero.charge_state.power_meter > f32::EPSILON
                {
                    hero.charge_state.power_decay_ticks_remaining =
                        hero.power_decay_profile.interval_ticks.max(1);
                }
                if hero.charge_state.power_meter + f32::EPSILON
                    < hero.charge_profile.power_meter_max
                {
                    return;
                }
            }

            if hero.charge_state.power_meter + f32::EPSILON >= hero.charge_profile.power_meter_max {
                hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
                hero.charge_state.power_active = true;
                if hero.charge_state.power_decay_ticks_remaining == 0 {
                    hero.charge_state.power_decay_ticks_remaining =
                        hero.power_decay_profile.interval_ticks.max(1);
                }
            }

            if hero.hp + f32::EPSILON < HERO_MAX_HP {
                hero.hp = (hero.hp + hero.charge_profile.health_per_tick).min(HERO_MAX_HP);
                if hero.hp + f32::EPSILON < HERO_MAX_HP {
                    return;
                }
            }

            if hero.charge_state.power_active {
                hero.charge_state.input_held = false;
                begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                return;
            }
        }
        ChargePhase::Recovery => {
            if hero.charge_state.phase_ticks_remaining > 0 {
                hero.charge_state.phase_ticks_remaining -= 1;
            }

            if hero.charge_state.phase_ticks_remaining == 0 {
                hero.charge_state.phase = ChargePhase::Idle;
            }
        }
    }

    let is_actively_charging = hero.charge_state.phase == ChargePhase::Charging;
    if is_actively_charging || hero.charge_state.power_meter <= f32::EPSILON {
        if hero.charge_state.power_meter <= f32::EPSILON {
            hero.charge_state.power_meter = 0.0;
            hero.charge_state.power_active = false;
            hero.charge_state.power_decay_ticks_remaining = 0;
        }
        return;
    }

    if hero.charge_state.power_decay_ticks_remaining > 0 {
        hero.charge_state.power_decay_ticks_remaining -= 1;
    }

    if hero.charge_state.power_decay_ticks_remaining == 0 {
        hero.charge_state.power_meter =
            (hero.charge_state.power_meter - hero.power_decay_profile.amount_per_interval).max(0.0);
        if hero.charge_state.power_meter <= f32::EPSILON {
            hero.charge_state.power_meter = 0.0;
            hero.charge_state.power_active = false;
            hero.charge_state.power_decay_ticks_remaining = 0;
        } else {
            hero.charge_state.power_decay_ticks_remaining =
                hero.power_decay_profile.interval_ticks.max(1);
        }
    }
}

fn hero_attack_damage_multiplier(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        hero.powered_modifiers.attack_damage_multiplier.max(1.0)
    } else {
        1.0
    }
}

fn hero_ability_radius(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        HERO_ABILITY_RADIUS * hero.powered_modifiers.ability_radius_multiplier.max(0.0)
    } else {
        HERO_ABILITY_RADIUS
    }
}

fn hero_regular_attack_profile(hero: &HeroState) -> DirectionalAttackComponent {
    let mut profile = hero.regular_attack_profile;
    if hero.charge_state.power_active {
        profile.range *= hero
            .powered_modifiers
            .regular_attack_range_multiplier
            .max(0.0);
    }
    profile
}

fn hero_ability_mana_cost(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        HERO_ABILITY_MANA_COST * hero.powered_modifiers.ability_mana_cost_multiplier.max(0.0)
    } else {
        HERO_ABILITY_MANA_COST
    }
}

fn hero_ability_cooldown_ticks(hero: &HeroState) -> u32 {
    let multiplier = if hero.charge_state.power_active {
        hero.powered_modifiers.ability_cooldown_multiplier.max(0.0)
    } else {
        1.0
    };
    let scaled = (HERO_ABILITY_COOLDOWN_TICKS as f32 * multiplier).round();
    if scaled <= 0.0 { 0 } else { scaled as u32 }
}

fn is_attack_locked(state: DirectionalAttackStateComponent) -> bool {
    state.phase != AttackPhase::Ready
}

fn start_attack(
    state: &mut DirectionalAttackStateComponent,
    profile: DirectionalAttackComponent,
) -> bool {
    if is_attack_locked(*state) {
        return false;
    }

    state.phase = AttackPhase::Windup;
    state.ticks_remaining = profile.windup_ticks.max(1);
    true
}

fn advance_attack_state(
    state: &mut DirectionalAttackStateComponent,
    profile: DirectionalAttackComponent,
) -> bool {
    match state.phase {
        AttackPhase::Ready => false,
        AttackPhase::Windup => {
            if state.ticks_remaining > 0 {
                state.ticks_remaining -= 1;
            }

            if state.ticks_remaining == 0 {
                if profile.recovery_ticks == 0 {
                    state.phase = AttackPhase::Ready;
                } else {
                    state.phase = AttackPhase::Recovery;
                    state.ticks_remaining = profile.recovery_ticks;
                }
                return true;
            }

            false
        }
        AttackPhase::Recovery => {
            if state.ticks_remaining > 0 {
                state.ticks_remaining -= 1;
            }
            if state.ticks_remaining == 0 {
                state.phase = AttackPhase::Ready;
            }
            false
        }
    }
}

fn find_next_waypoint(
    start_pos: [f32; 2],
    goal_pos: [f32; 2],
    navmesh: &NavMesh,
) -> Option<[f32; 2]> {
    let start = clamp_navmesh_point(start_pos);
    let mut goal = clamp_navmesh_point(goal_pos);
    if !navmesh.is_in_mesh(goal.into()) {
        goal = project_to_walkable(goal, navmesh, 10, 0.45)?;
    }

    let mut from = start;
    if !navmesh.is_in_mesh(from.into()) {
        from = project_to_walkable(from, navmesh, 8, 0.35)?;
    }

    if distance_sq(from, goal) <= ENEMY_WAYPOINT_REACH_RADIUS.powi(2) {
        return Some(goal);
    }

    let path = navmesh.path(from.into(), goal.into())?;
    let min_step_sq = (ENEMY_WAYPOINT_REACH_RADIUS * 0.5).powi(2);
    for step in path.path {
        let waypoint = clamp_to_world([step.x, step.y]);
        if distance_sq(waypoint, start_pos) > min_step_sq {
            return Some(waypoint);
        }
    }

    Some(goal)
}

fn build_navigation_mesh(towers: &BTreeMap<u64, TowerState>) -> NavMesh {
    let half_width = WORLD_HALF_WIDTH - NAVMESH_MARGIN;
    let half_height = WORLD_HALF_HEIGHT - NAVMESH_MARGIN;

    let edges = vec![
        [-half_width, -half_height].into(),
        [half_width, -half_height].into(),
        [half_width, half_height].into(),
        [-half_width, half_height].into(),
    ];

    let mut obstacles = Vec::with_capacity(towers.len());
    for tower in towers.values() {
        if let Some(obstacle) = tower_obstacle_polygon(tower.pos) {
            obstacles.push(obstacle.into_iter().map(Into::into).collect());
        }
    }

    let mut navmesh = NavMesh::from_edge_and_obstacles(edges, obstacles);
    let _ = navmesh.set_search_delta(NAVMESH_SEARCH_DELTA);
    let _ = navmesh.set_search_steps(NAVMESH_SEARCH_STEPS);
    navmesh
}

fn tower_obstacle_polygon(pos: [f32; 2]) -> Option<Vec<[f32; 2]>> {
    let max_radius_x = (WORLD_HALF_WIDTH - NAVMESH_MARGIN - pos[0].abs()).max(0.0);
    let max_radius_y = (WORLD_HALF_HEIGHT - NAVMESH_MARGIN - pos[1].abs()).max(0.0);
    let radius = TOWER_NAV_BLOCK_RADIUS.min(max_radius_x).min(max_radius_y);

    if radius <= 0.05 {
        return None;
    }

    let mut points = Vec::with_capacity(TOWER_NAV_BLOCK_VERTICES);
    for i in 0..TOWER_NAV_BLOCK_VERTICES {
        let angle = (i as f32 / TOWER_NAV_BLOCK_VERTICES as f32) * TAU;
        let point =
            clamp_navmesh_point([pos[0] + radius * angle.cos(), pos[1] + radius * angle.sin()]);
        points.push(point);
    }

    Some(points)
}

fn clamp_navmesh_point(pos: [f32; 2]) -> [f32; 2] {
    [
        pos[0].clamp(
            -WORLD_HALF_WIDTH + NAVMESH_MARGIN,
            WORLD_HALF_WIDTH - NAVMESH_MARGIN,
        ),
        pos[1].clamp(
            -WORLD_HALF_HEIGHT + NAVMESH_MARGIN,
            WORLD_HALF_HEIGHT - NAVMESH_MARGIN,
        ),
    ]
}

fn project_to_walkable(
    origin: [f32; 2],
    navmesh: &NavMesh,
    max_radius: i32,
    step: f32,
) -> Option<[f32; 2]> {
    let origin = clamp_navmesh_point(origin);
    if navmesh.is_in_mesh(origin.into()) {
        return Some(origin);
    }

    for ring in 1..=max_radius {
        let ringf = ring as f32 * step;
        let samples = (ring * 10).max(10);

        for i in 0..samples {
            let angle = (i as f32 / samples as f32) * TAU;
            let candidate = clamp_navmesh_point([
                origin[0] + ringf * angle.cos(),
                origin[1] + ringf * angle.sin(),
            ]);
            if navmesh.is_in_mesh(candidate.into()) {
                return Some(candidate);
            }
        }
    }

    None
}

fn add_displacement(map: &mut BTreeMap<u64, [f32; 2]>, id: u64, delta: [f32; 2]) {
    let entry = map.entry(id).or_insert([0.0, 0.0]);
    entry[0] += delta[0];
    entry[1] += delta[1];
}

fn nearest_enemy_id(hero_pos: [f32; 2], enemies: &BTreeMap<u64, [f32; 2]>) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for (enemy_id, enemy_pos) in enemies {
        let dist_sq = distance_sq(hero_pos, *enemy_pos);
        match best {
            Some((_, best_dist_sq)) if dist_sq >= best_dist_sq => {}
            _ => best = Some((*enemy_id, dist_sq)),
        }
    }
    best.map(|(enemy_id, _)| enemy_id)
}

fn clamp_vector_length(delta: [f32; 2], max_length: f32) -> [f32; 2] {
    let length_sq = delta[0] * delta[0] + delta[1] * delta[1];
    let max_sq = max_length * max_length;
    if length_sq <= max_sq {
        return delta;
    }

    if length_sq <= f32::EPSILON {
        return [0.0, 0.0];
    }

    let scale = (max_sq / length_sq).sqrt();
    [delta[0] * scale, delta[1] * scale]
}

fn spawn_position_for_client(client_id: u64) -> [f32; 2] {
    let angle = ((client_id % 16) as f32 / 16.0) * TAU;
    let radius = 2.5 + ((client_id % 3) as f32 * 0.45);
    clamp_to_world([angle.cos() * radius, angle.sin() * radius])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_static_enemy(sim: &mut Simulation, id: u64, pos: [f32; 2], hp: f32) {
        sim.enemies.insert(
            id,
            EnemyState {
                id,
                lane: 0,
                pos,
                vel: [0.0, 0.0],
                hp,
                max_hp: hp,
                speed: 0.0,
                reward: 0,
                enemy_type: EnemyType::Grunt,
                radius: enemy_collider_radius(EnemyType::Grunt),
                facing: FacingComponent { dir: [-1.0, 0.0] },
                regular_attack: DirectionalAttackStateComponent::default(),
                regular_attack_profile: DirectionalAttackComponent {
                    range: 1.0,
                    arc_dot_threshold: 0.0,
                    damage: 0.0,
                    windup_ticks: 1,
                    recovery_ticks: 1,
                },
                lock_target: EnemyLockTarget::Base,
                target_pos: BASE_POSITION,
                waypoint: BASE_POSITION,
                repath_cooldown: 0,
            },
        );
    }

    #[test]
    fn lock_target_set_clear_validation() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1001, [2.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1001),
            },
        );
        sim.step();
        assert!(
            sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            Some(1001)
        );

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 2,
                target_id: Some(999_999),
            },
        );
        sim.step();
        assert!(
            sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            Some(1001)
        );

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 3,
                target_id: None,
            },
        );
        sim.step();
        assert!(
            !sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(true)
        );
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            None
        );
    }

    #[test]
    fn hero_facing_tracks_locked_target_while_moving() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1002, [0.0, 8.0], 100.0);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 1,
                dir: [1.0, 0.0],
            },
        );
        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 2,
                target_id: Some(1002),
            },
        );
        sim.step();

        let hero = sim.heroes.get(&1).expect("hero should exist");
        assert!(hero.pos[0] > 0.0);
        assert!(hero.facing.dir[1] > 0.9);
        assert!(hero.facing.dir[0].abs() < 0.25);
    }

    #[test]
    fn objective_collision_blocks_hero_from_entering_base() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [
                BASE_POSITION[0] + OBJECTIVE_COLLIDER_RADIUS + HERO_COLLIDER_RADIUS - 0.2,
                BASE_POSITION[1],
            ];
            hero.move_dir = [0.0, 0.0];
        }

        sim.step();
        let hero = sim.heroes.get(&1).expect("hero should exist");
        let dist = distance_sq(hero.pos, BASE_POSITION).sqrt();
        assert!(dist + 0.0001 >= OBJECTIVE_COLLIDER_RADIUS + HERO_COLLIDER_RADIUS - 0.01);
    }

    #[test]
    fn enemy_facing_tracks_locked_target_position() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1099, [0.0, 0.0], 100.0);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 3.0];
            hero.move_dir = [0.0, 0.0];
        }
        if let Some(enemy) = sim.enemies.get_mut(&1099) {
            enemy.facing = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.step();
        let enemy = sim.enemies.get(&1099).expect("enemy should exist");
        assert!(enemy.facing.dir[1] > 0.95);
        assert!(enemy.facing.dir[0].abs() < 0.2);
    }

    #[test]
    fn lock_target_retargets_when_enemy_removed_if_mode_active() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1003, [2.0, 0.0], 100.0);
        insert_static_enemy(&mut sim, 1004, [4.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1003),
            },
        );
        sim.step();
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            Some(1003)
        );

        sim.enemies.remove(&1003);
        sim.step();
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            Some(1004)
        );
        assert!(
            sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(false)
        );
    }

    #[test]
    fn lock_mode_disables_when_no_targets_remain() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 1101, [2.0, 0.0], 100.0);

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(1101),
            },
        );
        sim.step();
        assert!(
            sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(false)
        );
        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            Some(1101)
        );

        sim.enemies.remove(&1101);
        sim.step();

        assert_eq!(
            sim.heroes.get(&1).and_then(|hero| hero.lock_target_id),
            None
        );
        assert!(
            !sim.heroes
                .get(&1)
                .map(|hero| hero.lock_mode_active)
                .unwrap_or(true)
        );
    }

    #[test]
    fn regular_attack_hits_all_enemies_in_cone() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 2001, [1.0, 0.0], 120.0);
        insert_static_enemy(&mut sim, 2002, [2.0, 0.0], 120.0);
        insert_static_enemy(&mut sim, 2003, [0.0, 2.0], 120.0);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.move_dir = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
        }

        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 1,
                target_id: Some(2002),
            },
        );
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 2 });
        for _ in 0..5 {
            sim.step();
        }

        let near_hp = sim
            .enemies
            .get(&2001)
            .map(|enemy| enemy.hp)
            .expect("near enemy should exist");
        let locked_hp = sim
            .enemies
            .get(&2002)
            .map(|enemy| enemy.hp)
            .expect("locked enemy should exist");
        let outside_hp = sim
            .enemies
            .get(&2003)
            .map(|enemy| enemy.hp)
            .expect("outside-cone enemy should exist");

        assert!((near_hp - (120.0 - HERO_REGULAR_ATTACK.damage)).abs() < 0.001);
        assert!((locked_hp - (120.0 - HERO_REGULAR_ATTACK.damage)).abs() < 0.001);
        assert!((outside_hp - 120.0).abs() < 0.001);
    }

    #[test]
    fn charge_startup_and_release_lag_lock_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.move_dir = [0.0, 0.0];
        }

        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 1,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        let baseline_x = sim.heroes.get(&1).map(|hero| hero.pos[0]).unwrap_or(0.0);
        assert!(baseline_x > 0.0);

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 2,
                active: true,
            },
        );
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 3,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        let locked_x = sim.heroes.get(&1).map(|hero| hero.pos[0]).unwrap_or(0.0);
        assert!((locked_x - baseline_x).abs() < 0.0001);

        let startup_ticks = sim
            .heroes
            .get(&1)
            .map(|hero| hero.charge_profile.startup_ticks)
            .unwrap_or(0);
        for _ in 0..startup_ticks {
            sim.step();
        }
        let charging_locked_x = sim.heroes.get(&1).map(|hero| hero.pos[0]).unwrap_or(0.0);
        assert!((charging_locked_x - baseline_x).abs() < 0.0001);

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 4,
                active: false,
            },
        );
        sim.step();
        let release_x = sim.heroes.get(&1).map(|hero| hero.pos[0]).unwrap_or(0.0);

        let release_lag_ticks = sim
            .heroes
            .get(&1)
            .map(|hero| hero.charge_profile.release_lag_ticks)
            .unwrap_or(0);
        let mut steps_after_release = 0_u32;
        let mut resumed_x = release_x;
        let mut next_seq = 5_u32;
        while steps_after_release <= release_lag_ticks.saturating_add(2) {
            // Send a move command each tick (like a real client would)
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq: next_seq,
                    dir: [1.0, 0.0],
                },
            );
            next_seq += 1;
            sim.step();
            steps_after_release += 1;
            resumed_x = sim.heroes.get(&1).map(|hero| hero.pos[0]).unwrap_or(0.0);
            if resumed_x > release_x {
                break;
            }
        }

        assert!(resumed_x > release_x);
        assert!(steps_after_release >= release_lag_ticks.saturating_sub(1));
    }

    #[test]
    fn charge_fills_mana_then_power_then_health_before_auto_exit() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.mana = 0.0;
            hero.hp = 60.0;
            hero.charge_profile.startup_ticks = 0;
            hero.charge_profile.release_lag_ticks = 1;
            hero.charge_profile.mana_per_tick = 20.0;
            hero.charge_profile.health_per_tick = 10.0;
            hero.charge_profile.power_gain_per_tick = 60.0;
            hero.charge_profile.power_meter_max = 100.0;
            hero.charge_state = ChargeStateComponent::default();
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();
        let first = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert!(first.mana > 0.0);
        assert!((first.hp - 60.0).abs() < 0.0001);

        while sim
            .heroes
            .get(&1)
            .map(|hero| hero.mana + 0.0001 < HERO_MAX_MANA)
            .unwrap_or(false)
        {
            sim.step();
        }
        let mana_full_hp = sim.heroes.get(&1).map(|hero| hero.hp).unwrap_or(0.0);
        let mana_full_power = sim
            .heroes
            .get(&1)
            .map(|hero| hero.charge_state.power_meter)
            .unwrap_or(0.0);
        sim.step();
        let post_mana = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert!((post_mana.hp - mana_full_hp).abs() < 0.0001);
        assert!(post_mana.charge_state.power_meter > mana_full_power);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.mana = HERO_MAX_MANA;
            hero.hp = 80.0;
            hero.charge_state.phase = ChargePhase::Charging;
            hero.charge_state.input_held = true;
            hero.charge_state.power_meter = 95.0;
        }
        sim.step();
        let after_full_power = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert!(after_full_power.charge_state.power_active);
        assert_eq!(after_full_power.charge_state.phase, ChargePhase::Charging);
        assert!(after_full_power.hp > 80.0);

        sim.step();
        let charged = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert!((charged.hp - HERO_MAX_HP).abs() < 0.001);
        assert!(
            charged.charge_state.phase == ChargePhase::Recovery
                || charged.charge_state.phase == ChargePhase::Idle
        );
    }

    #[test]
    fn charging_is_blocked_while_power_mode_is_active() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.charge_state.power_active = true;
            hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
            hero.charge_state.phase = ChargePhase::Idle;
            hero.charge_state.input_held = false;
            hero.charge_state.power_decay_ticks_remaining = 10_000;
            hero.power_decay_profile.interval_ticks = 10_000;
            hero.power_decay_profile.amount_per_interval = 0.0;
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();

        let hero = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert_eq!(hero.charge_state.phase, ChargePhase::Idle);
        assert!(!hero.charge_state.input_held);
        assert!(hero.charge_state.power_active);
    }

    #[test]
    fn power_meter_buff_doubles_damage_and_decays_to_disable() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 3001, [1.0, 0.0], 200.0);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
            hero.charge_state.power_active = true;
            hero.charge_state.power_meter = 100.0;
            hero.charge_state.power_decay_ticks_remaining = 10_000;
            hero.powered_modifiers.attack_damage_multiplier = 2.0;
            hero.power_decay_profile.interval_ticks = 10_000;
        }

        sim.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            sim.step();
        }

        let hp_after = sim
            .enemies
            .get(&3001)
            .map(|enemy| enemy.hp)
            .expect("enemy should exist");
        assert!((hp_after - (200.0 - HERO_REGULAR_ATTACK.damage * 2.0)).abs() < 0.001);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.charge_state.power_active = true;
            hero.charge_state.power_meter = 9.0;
            hero.charge_state.power_decay_ticks_remaining = 1;
            hero.power_decay_profile.interval_ticks = 1;
            hero.power_decay_profile.amount_per_interval = 5.0;
        }

        sim.step();
        assert!(
            sim.heroes
                .get(&1)
                .map(|hero| hero.charge_state.power_active)
                .unwrap_or(false)
        );
        sim.step();
        let power_active = sim
            .heroes
            .get(&1)
            .map(|hero| hero.charge_state.power_active)
            .unwrap_or(true);
        assert!(!power_active);
    }

    #[test]
    fn powered_ability_radius_hits_farther_targets() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        insert_static_enemy(&mut sim, 3101, [9.0, 0.0], 200.0);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
            hero.charge_state.power_active = true;
            hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
            hero.charge_state.power_decay_ticks_remaining = 10_000;
            hero.power_decay_profile.interval_ticks = 10_000;
        }

        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();

        let hp_after = sim
            .enemies
            .get(&3101)
            .map(|enemy| enemy.hp)
            .expect("enemy should exist");
        assert!((hp_after - (200.0 - HERO_ABILITY_DAMAGE * 2.0)).abs() < 0.001);
    }

    #[test]
    fn powered_regular_attack_range_reaches_far_targets() {
        let mut baseline = Simulation::new();
        baseline.add_player(1);
        insert_static_enemy(&mut baseline, 3201, [8.0, 0.0], 200.0);

        if let Some(hero) = baseline.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
        }

        baseline.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            baseline.step();
        }
        let baseline_hp = baseline
            .enemies
            .get(&3201)
            .map(|enemy| enemy.hp)
            .expect("enemy should exist");
        assert!((baseline_hp - 200.0).abs() < 0.001);

        let mut powered = Simulation::new();
        powered.add_player(1);
        insert_static_enemy(&mut powered, 3201, [8.0, 0.0], 200.0);

        if let Some(hero) = powered.heroes.get_mut(&1) {
            hero.pos = [0.0, 0.0];
            hero.facing = FacingComponent { dir: [1.0, 0.0] };
            hero.charge_state.power_active = true;
            hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
            hero.charge_state.power_decay_ticks_remaining = 10_000;
            hero.power_decay_profile.interval_ticks = 10_000;
        }

        powered.queue_command(1, ClientCommand::BasicAttack { seq: 1 });
        for _ in 0..=HERO_REGULAR_ATTACK.windup_ticks {
            powered.step();
        }
        let powered_hp = powered
            .enemies
            .get(&3201)
            .map(|enemy| enemy.hp)
            .expect("enemy should exist");
        assert!((powered_hp - (200.0 - HERO_REGULAR_ATTACK.damage * 2.0)).abs() < 0.001);
    }

    #[test]
    fn partial_power_meter_decays_without_activating_power_mode() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.mana = HERO_MAX_MANA;
            hero.hp = HERO_MAX_HP;
            hero.charge_profile.startup_ticks = 0;
            hero.charge_profile.release_lag_ticks = 0;
            hero.charge_profile.power_gain_per_tick = 20.0;
            hero.charge_profile.power_meter_max = 100.0;
            hero.power_decay_profile.interval_ticks = 1;
            hero.power_decay_profile.amount_per_interval = 5.0;
        }

        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 1,
                active: true,
            },
        );
        sim.step();
        sim.step();
        sim.queue_command(
            1,
            ClientCommand::SetCharging {
                seq: 2,
                active: false,
            },
        );
        sim.step();

        let partial = sim.heroes.get(&1).cloned().expect("hero should exist");
        assert!(partial.charge_state.power_meter > 0.0);
        assert!(partial.charge_state.power_meter < partial.charge_profile.power_meter_max);
        assert!(!partial.charge_state.power_active);

        let mut last_power_meter = partial.charge_state.power_meter;
        for _ in 0..4 {
            sim.step();
            let hero = sim.heroes.get(&1).cloned().expect("hero should exist");
            assert!(!hero.charge_state.power_active);
            assert!(hero.charge_state.power_meter <= last_power_meter + 0.0001);
            last_power_meter = hero.charge_state.power_meter;
        }
    }

    #[test]
    fn powered_mode_halves_special_mana_cost_and_cooldown() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.mana = HERO_ABILITY_MANA_COST;
            hero.ability_cooldown_ticks = 0;
            hero.charge_state.power_active = true;
            hero.powered_modifiers.ability_mana_cost_multiplier = 0.5;
            hero.powered_modifiers.ability_cooldown_multiplier = 0.5;
            hero.power_decay_profile.interval_ticks = 10_000;
            hero.power_decay_profile.amount_per_interval = 0.0;
            hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
            hero.charge_state.power_decay_ticks_remaining = 10_000;
        }

        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();

        let hero = sim.heroes.get(&1).cloned().expect("hero should exist");
        let expected_mana = HERO_ABILITY_MANA_COST * 0.5 + HERO_MANA_REGEN_PER_TICK;
        assert!((hero.mana - expected_mana).abs() < 0.001);
        assert_eq!(
            hero.ability_cooldown_ticks,
            HERO_ABILITY_COOLDOWN_TICKS / 2 - 1
        );
    }

    #[test]
    fn defeat_phase_auto_resets_match_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);

        if let Some(hero) = sim.heroes.get_mut(&1) {
            hero.gold = 999;
            hero.hp = 12.0;
            hero.mana = 4.0;
            hero.pos = [7.0, -3.0];
            hero.lock_mode_active = true;
            hero.lock_target_id = Some(42);
        }
        sim.towers.insert(
            777,
            TowerState {
                id: 777,
                owner: 1,
                lane: 0,
                node_id: BUILD_NODES[0].node_id,
                pos: BUILD_NODES[0].pos,
                reload_ticks: 0,
            },
        );
        sim.node_occupancy.insert(BUILD_NODES[0].node_id, 777);
        sim.team_life = 0;
        sim.wave = 9;

        sim.step();
        assert_eq!(sim.phase, MatchPhase::Defeat);
        let reset_at = sim.reset_at_tick.expect("reset should be scheduled");

        while sim.tick < reset_at {
            sim.step();
        }

        assert_eq!(sim.phase, MatchPhase::InProgress);
        assert_eq!(sim.wave, 0);
        assert_eq!(sim.team_life, INITIAL_TEAM_LIFE);
        assert_eq!(sim.objective_hp, OBJECTIVE_MAX_HP);
        assert!(sim.towers.is_empty());
        assert!(sim.node_occupancy.is_empty());
        assert!(sim.reset_at_tick.is_none());
        let hero = sim.heroes.get(&1).expect("hero should exist");
        assert_eq!(hero.gold, INITIAL_GOLD);
        assert!((hero.hp - HERO_MAX_HP).abs() < 0.001);
        assert!((hero.mana - HERO_MAX_MANA).abs() < 0.001);
        assert!(!hero.lock_mode_active);
        assert!(hero.lock_target_id.is_none());
    }

    #[test]
    fn victory_phase_auto_resets_match_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.wave = MAX_WAVES;
        sim.wave_remaining = 0;
        sim.intermission_until = 0;

        sim.step();
        assert_eq!(sim.phase, MatchPhase::Victory);
        let reset_at = sim.reset_at_tick.expect("reset should be scheduled");

        while sim.tick < reset_at {
            sim.step();
        }

        assert_eq!(sim.phase, MatchPhase::InProgress);
        assert_eq!(sim.wave, 0);
        assert_eq!(sim.team_life, INITIAL_TEAM_LIFE);
        assert!(sim.intermission_until > sim.tick);
        assert!(sim.reset_at_tick.is_none());
    }

    #[test]
    fn enemy_target_keeps_current_hero_until_disengage_then_retargets() {
        let heroes = vec![(1_u64, [0.0, 0.0]), (2_u64, [4.0, 0.0])];

        let kept = choose_enemy_target([10.5, 0.0], EnemyLockTarget::Hero(1), &heroes);
        assert_eq!(kept, EnemyLockTarget::Hero(1));

        let retargeted = choose_enemy_target([12.0, 0.0], EnemyLockTarget::Hero(1), &heroes);
        assert_eq!(retargeted, EnemyLockTarget::Hero(2));
    }
}

#[cfg(test)]
mod netcode_regressions {
    use super::*;

    #[test]
    fn delayed_reliable_action_survives_newer_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 101,
                dir: [0.0, 0.0],
            },
        );
        sim.step();
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 100 });
        sim.step();
        assert_eq!(sim.heroes[&1].regular_attack.phase, AttackPhase::Windup);
    }

    #[test]
    fn actions_cannot_fast_forward_queued_movement() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let start = sim.heroes[&1].pos;
        for seq in 1..=10 {
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        sim.queue_command(
            1,
            ClientCommand::SetLockTarget {
                seq: 11,
                target_id: None,
            },
        );
        sim.step();
        assert!(sim.heroes[&1].pos[0] - start[0] <= HERO_SPEED * FIXED_DT_SECONDS + 0.00001);
    }

    #[test]
    fn restored_world_replays_identically_with_movement_backlog() {
        let mut original = Simulation::new();
        original.add_player(1);
        original.add_player(2);
        original.heroes.get_mut(&1).unwrap().pos = [0.0, 3.0];
        original.heroes.get_mut(&2).unwrap().pos = [1.0, 3.0];
        for i in 0..8 {
            original.spawn_enemy(i % ENEMY_SPAWN_POINTS.len(), 1);
        }
        for (i, enemy) in original.enemies.values_mut().enumerate() {
            enemy.pos = [i as f32 * 0.35, 3.5];
        }
        for seq in 1..=5 {
            original.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
            original.queue_command(
                2,
                ClientCommand::Move {
                    seq,
                    dir: [-1.0, 0.0],
                },
            );
        }
        original.step();
        let delta = original.world_delta_for(1);
        let mut restored = Simulation::from_snapshot(&delta, &original.sim_meta());
        assert_eq!(delta, restored.world_delta_for(1));
        for tick in 0..30 {
            let a = original.step();
            let b = restored.step();
            assert_eq!(
                a.reliable_events, b.reliable_events,
                "events at replay {tick}"
            );
            assert_eq!(
                original.world_delta_for(1),
                restored.world_delta_for(1),
                "replay {tick}"
            );
        }
    }

    #[test]
    fn snapshot_restores_enemy_decision_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.spawn_enemy(0, 1);
        let enemy = sim.enemies.values_mut().next().unwrap();
        enemy.lock_target = EnemyLockTarget::Hero(1);
        enemy.target_pos = [3.0, 4.0];
        enemy.waypoint = [2.0, 3.0];
        enemy.repath_cooldown = 5;
        let delta = sim.world_delta_for(1);
        let restored = Simulation::from_snapshot(&delta, &sim.sim_meta());
        let a = sim.enemies.values().next().unwrap();
        let b = restored.enemies.values().next().unwrap();
        assert_eq!(a.lock_target, b.lock_target);
        assert_eq!(a.target_pos, b.target_pos);
        assert_eq!(a.waypoint, b.waypoint);
        assert_eq!(a.repath_cooldown, b.repath_cooldown);
    }
}

#[cfg(test)]
mod protocol_edge_tests {
    use super::*;

    #[test]
    fn duplicate_and_reordered_redundant_moves_are_consumed_once() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let start = sim.heroes[&1].pos;
        for seq in [u32::MAX, 0, 1, u32::MAX, 0, 1] {
            sim.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        for _ in 0..4 {
            sim.step();
        }
        assert_eq!(sim.heroes[&1].last_processed_seq, Some(1));
        assert!(
            (sim.heroes[&1].pos[0] - start[0] - 3.0 * HERO_SPEED * FIXED_DT_SECONDS).abs()
                < 0.00001
        );
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 0,
                dir: [1.0, 0.0],
            },
        );
        sim.step();
        assert_eq!(sim.heroes[&1].last_processed_seq, Some(1));
    }

    #[test]
    fn nonfinite_input_and_disconnect_cannot_leak_into_new_session() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: 100,
                dir: [f32::NAN, 0.0],
            },
        );
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 101 });
        sim.remove_player(1);
        sim.add_player(1);
        sim.step();
        assert_eq!(sim.heroes[&1].last_action_seq, None);
        assert_eq!(sim.heroes[&1].last_processed_seq, None);
        assert!(sim.heroes[&1].pos.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn duplicate_actions_do_not_restart_after_cooldown() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(1, ClientCommand::BasicAttack { seq: u32::MAX });
        sim.step();
        for _ in 0..60 {
            sim.step();
        }
        sim.queue_command(1, ClientCommand::BasicAttack { seq: u32::MAX });
        sim.step();
        assert_eq!(sim.heroes[&1].regular_attack.phase, AttackPhase::Ready);
        sim.queue_command(1, ClientCommand::BasicAttack { seq: 0 });
        sim.step();
        assert_eq!(sim.heroes[&1].regular_attack.phase, AttackPhase::Windup);
    }

    #[test]
    fn match_reset_waits_through_tick_wrap_and_discards_old_moves() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.set_tick(u32::MAX - 2);
        sim.phase = MatchPhase::Defeat;
        sim.move_queues
            .get_mut(&1)
            .unwrap()
            .push_back((5, [1.0, 0.0]));
        sim.schedule_match_reset();
        assert_eq!(sim.match_restart_ticks_remaining(), Some(MATCH_RESET_TICKS));
        for _ in 0..MATCH_RESET_TICKS - 1 {
            sim.step();
        }
        assert_eq!(sim.phase, MatchPhase::Defeat);
        sim.step();
        assert_eq!(sim.phase, MatchPhase::InProgress);
        assert!(sim.move_queues[&1].is_empty());
        assert_eq!(
            sim.intermission_until,
            sim.tick.wrapping_add(WAVE_PREP_TICKS)
        );
    }
}

#[cfg(test)]
mod action_timeline_tests {
    use super::*;
    use game_shared::ClientAction;

    #[test]
    fn early_attack_waits_for_walk_and_snapshot_restore_keeps_boundary() {
        let mut server = Simulation::new();
        server.add_player(1);
        let start = server.heroes[&1].pos;
        let action = ClientAction {
            view_tick: None,
            match_epoch: 0,
            command: ClientCommand::BasicAttack { seq: 6 },
            after_move_seq: Some(5),
        };
        server.queue_action(1, action); // reliable channel wins the race
        server.step();
        assert_eq!(server.heroes[&1].regular_attack.phase, AttackPhase::Ready);
        for seq in 1..=5 {
            server.queue_command(
                1,
                ClientCommand::Move {
                    seq,
                    dir: [1.0, 0.0],
                },
            );
        }
        for _ in 0..2 {
            server.step();
        }
        let snapshot = server.world_delta_for(1);
        let mut restored = Simulation::from_snapshot(&snapshot, &server.sim_meta());
        for _ in 0..3 {
            server.queue_action(1, action); // repeated in movement packets
            restored.queue_action(1, action);
            server.step();
            restored.step();
            assert_eq!(server.world_delta_for(1), restored.world_delta_for(1));
            assert_eq!(server.heroes[&1].regular_attack.phase, AttackPhase::Ready);
        }
        let stop = server.heroes[&1].pos;
        assert!((stop[0] - start[0] - 5.0 * HERO_SPEED * FIXED_DT_SECONDS).abs() < 0.0001);
        server.queue_command(
            1,
            ClientCommand::Move {
                seq: 7,
                dir: [1.0, 0.0],
            },
        );
        server.step();
        assert_eq!(server.heroes[&1].pos, stop);
        assert_eq!(server.heroes[&1].regular_attack.phase, AttackPhase::Windup);
        assert_eq!(server.heroes[&1].last_action_seq, Some(6));
        assert!(server.heroes[&1].pending_actions.is_empty());
    }

    #[test]
    fn boundary_wrap_and_invalid_dependency_are_handled() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.queue_command(
            1,
            ClientCommand::Move {
                seq: u32::MAX,
                dir: [0.0, 0.0],
            },
        );
        sim.step();
        sim.queue_action(
            1,
            ClientAction {
                view_tick: None,
                match_epoch: 0,
                command: ClientCommand::BasicAttack { seq: 0 },
                after_move_seq: Some(u32::MAX),
            },
        );
        sim.step();
        assert_eq!(sim.heroes[&1].last_action_seq, Some(0));
        sim.queue_action(
            1,
            ClientAction {
                view_tick: None,
                match_epoch: 0,
                command: ClientCommand::BasicAttack { seq: 1 },
                after_move_seq: Some(2),
            },
        );
        assert!(sim.heroes[&1].pending_actions.is_empty());
    }
}

#[cfg(test)]
mod round_input_tests {
    use super::*;
    #[test]
    fn old_round_actions_cannot_execute_after_reset() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let action = game_shared::ClientAction {
            view_tick: None,
            match_epoch: 0,
            command: ClientCommand::BasicAttack { seq: 2 },
            after_move_seq: Some(1),
        };
        sim.queue_action(1, action);
        sim.reset_match_state();
        sim.queue_action(1, action);
        assert!(sim.heroes[&1].pending_actions.is_empty());
        assert_eq!(sim.sim_meta().match_epoch, 1);
        sim.queue_action(
            1,
            game_shared::ClientAction {
                view_tick: None,
                match_epoch: 1,
                after_move_seq: None,
                ..action
            },
        );
        sim.step();
        assert_eq!(sim.heroes[&1].last_attack_seq, Some(2));
    }
    #[test]
    fn deferred_actions_are_bounded_and_removed_with_player() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        for seq in 2..200 {
            sim.queue_action(
                1,
                game_shared::ClientAction {
                    view_tick: None,
                    match_epoch: 0,
                    command: ClientCommand::BasicAttack { seq },
                    after_move_seq: Some(1),
                },
            );
        }
        assert_eq!(sim.heroes[&1].pending_actions.len(), 64);
        sim.remove_player(1);
        sim.add_player(1);
        assert!(sim.heroes[&1].pending_actions.is_empty());
    }
}

#[cfg(test)]
mod presentation_contract_tests {
    use super::*;
    #[test]
    fn ability_state_survives_restore_and_replay_without_duplicate_identity() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        let before = sim.world_delta_for(1);
        let command = ClientCommand::CastAbility {
            seq: 7,
            ability: AbilityId::ArcBurst,
        };
        sim.queue_command(1, command);
        sim.step();
        let predicted = sim.world_delta_for(1);
        assert_eq!(predicted.presentations.len(), 1);
        let instance = predicted.presentations[0];
        assert_eq!(instance.id.action_seq, 7);
        assert_eq!(instance.age_ticks, 0);
        let mut replay = Simulation::from_snapshot(&before, &before.sim_meta.unwrap());
        replay.queue_command(1, command);
        replay.step();
        assert_eq!(
            replay.world_delta_for(1).presentations,
            predicted.presentations
        );
        let mut confirmed = Simulation::from_snapshot(&predicted, &predicted.sim_meta.unwrap());
        confirmed.queue_command(1, command);
        confirmed.step();
        let state = confirmed.world_delta_for(1);
        assert_eq!(state.presentations.len(), 1);
        assert_eq!(state.presentations[0].id, instance.id);
        assert_eq!(state.presentations[0].age_ticks, 1);
        for _ in 0..instance.duration_ticks {
            confirmed.step();
        }
        assert!(confirmed.world_delta_for(1).presentations.is_empty());
        sim.reset_match_state();
        assert!(sim.world_delta_for(1).presentations.is_empty());
    }

    #[test]
    fn rejected_ability_does_not_create_presentation_state() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        sim.heroes.get_mut(&1).unwrap().mana = 0.0;
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert!(sim.world_delta_for(1).presentations.is_empty());
    }
}

#[cfg(test)]
mod historical_ability_tests {
    use super::*;
    #[test]
    fn history_changes_hit_geometry_but_not_origin_or_ability_validation() {
        let mut sim = Simulation::new();
        sim.add_player(1);
        // Let the real spawner create an enemy, then put it beyond current range.
        for _ in 0..1000 {
            sim.step();
            if !sim.enemies.is_empty() {
                break;
            }
        }
        let id = *sim.enemies.keys().next().unwrap();
        let origin = sim.heroes[&1].pos;
        sim.enemies.get_mut(&id).unwrap().pos = [origin[0] + 20.0, origin[1]];
        sim.enemies.get_mut(&id).unwrap().hp = 1000.0;
        sim.set_historical_targets(1, 1, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(sim.enemies[&id].hp, 1000.0 - HERO_ABILITY_DAMAGE);
        let hp = sim.enemies[&id].hp;
        sim.set_historical_targets(1, 1, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 1,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(sim.enemies[&id].hp, hp); // duplicate cannot damage twice
        sim.set_historical_targets(1, 2, vec![(id, origin)]);
        sim.queue_command(
            1,
            ClientCommand::CastAbility {
                seq: 2,
                ability: AbilityId::ArcBurst,
            },
        );
        sim.step();
        assert_eq!(sim.enemies[&id].hp, hp); // history cannot bypass cooldown
        assert!(sim.historical_targets.is_empty());
    }
}
