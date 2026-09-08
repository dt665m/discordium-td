//! Canonical reductions retain collision, damage and reward ordering.
use super::*;

pub(crate) fn resolve_enemies(world: &mut World) {
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        globals.resolve_enemy_collisions(world);
        let mut query = world.query::<(&EnemyState, &EnemyStep)>();
        let mut hits: Vec<_> = query
            .iter(world)
            .map(|(enemy, outcome)| (enemy.id, outcome.hero_hit, outcome.objective_hit))
            .collect();
        hits.sort_unstable_by_key(|(id, _, _)| *id);
        for &(_, hit, _) in &hits {
            if let Some((hero, damage)) = hit {
                globals.apply_hero_damage(world, hero, damage);
            }
        }
        let mut events = Vec::new();
        for (enemy, _, objective) in hits {
            if objective && remove_enemy(world, &enemy).is_some() {
                globals.apply_objective_damage(world, &mut events);
            }
        }
        world.resource_mut::<TickEvents>().0.extend(events);
    });
}
pub(crate) fn resolve_heroes(world: &mut World) {
    let mut query = world.query::<(&game_replication::NetId, &HeroStep)>();
    let mut attackers: Vec<_> = query
        .iter(world)
        .filter_map(|(id, outcome)| outcome.attack_triggered.then_some(id.value))
        .collect();
    attackers.sort_unstable();
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        let mut events = Vec::new();
        globals.resolve_hero_attacks(world, attackers, &mut events);
        world.resource_mut::<TickEvents>().0.extend(events);
    });
}
#[derive(Resource, Default)]
pub(crate) struct TowerTargets(Vec<(u64, [f32; 2])>);
pub(crate) fn prepare_towers(
    enemies: Query<(&EnemyState, &Position)>,
    mut targets: ResMut<TowerTargets>,
) {
    targets.0.clear();
    targets
        .0
        .extend(enemies.iter().map(|(enemy, pos)| (enemy.id, pos.pos)));
    targets.0.sort_unstable_by_key(|(id, _)| *id);
}
pub(crate) fn advance_towers(
    targets: Res<TowerTargets>,
    mut towers: Query<(&mut TowerState, &Position, &mut TowerStep)>,
) {
    let update = |(mut tower, position, mut outcome): (
        Mut<'_, TowerState>,
        &Position,
        Mut<'_, TowerStep>,
    )| {
        outcome.shot = None;
        if tower.reload_ticks > 0 {
            tower.reload_ticks -= 1;
            return;
        }
        let mut best_enemy = None;
        let mut best_distance = f32::INFINITY;
        for &(id, pos) in &targets.0 {
            let distance = distance_sq(pos, position.pos);
            if distance > TOWER_RANGE * TOWER_RANGE {
                continue;
            }
            if distance < best_distance {
                best_distance = distance;
                best_enemy = Some(id);
            }
        }
        if let Some(id) = best_enemy {
            tower.reload_ticks = TOWER_RELOAD_TICKS;
            outcome.shot = Some((id, tower.owner));
        }
    };
    if towers.iter().len() >= 128 {
        towers
            .par_iter_mut()
            .batching_strategy(bevy::ecs::batching::BatchingStrategy::new().min_batch_size(128))
            .for_each(update);
    } else {
        towers.iter_mut().for_each(update);
    }
}
pub(crate) fn resolve_towers(world: &mut World) {
    let mut query = world.query::<(&TowerState, &TowerStep)>();
    let mut shots: Vec<_> = query
        .iter(world)
        .filter_map(|(tower, outcome)| outcome.shot.map(|shot| (tower.id, shot)))
        .collect();
    shots.sort_unstable_by_key(|(id, _)| *id);
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        let mut events = Vec::new();
        for (_, (enemy, owner)) in shots {
            globals.apply_enemy_damage(world, enemy, TOWER_DAMAGE, owner, &mut events);
        }
        world.resource_mut::<TickEvents>().0.extend(events);
    });
}

impl Globals {
    pub(crate) fn try_cast_ability(
        &mut self,
        world: &mut World,
        client_id: u64,
        ability: AbilityId,
        action_seq: u32,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        if ability != AbilityId::ArcBurst {
            return;
        }

        let Some(hero) = hero_mut(world, &client_id) else {
            return;
        };

        let mana_cost = hero_ability_mana_cost(&hero.role);
        if hero.role.ability_cooldown_ticks > 0
            || hero.role.mana < mana_cost
            || is_charge_locked(hero.role.charge_state)
        {
            return;
        }

        hero.role.ability_cooldown_ticks = hero_ability_cooldown_ticks(&hero.role);
        hero.role.mana -= mana_cost;
        let cast_origin = hero.position.pos;
        let ability_radius = hero_ability_radius(&hero.role);
        let damage_multiplier = hero_attack_damage_multiplier(&hero.role);

        systems::presentation::insert(
            world,
            game_shared::PresentationInstance {
                id: game_shared::PresentationId {
                    match_epoch: world.resource::<Epoch>().0,
                    owner: client_id,
                    action_seq,
                    slot: 0,
                },
                kind: game_shared::PresentationKind::ArcBurst,
                pos: cast_origin,
                radius: ability_radius,
                age_ticks: 0,
                duration_ticks: 17,
            },
        );

        reliable_events.push(ReliableGameEvent::AbilityCast {
            owner: client_id,
            pos: cast_origin,
            radius: ability_radius,
        });

        let mut targets = Vec::new();
        let radius_sq = ability_radius * ability_radius;
        let historical = world
            .resource_mut::<HistoricalTargets>()
            .0
            .remove(&(client_id, action_seq));
        if let Some(history) = historical {
            for (id, pos) in history {
                // Historical geometry cannot resurrect a removed enemy or award
                // a second kill. All damage still applies to current state.
                if enemy(world, &id).is_some() && distance_sq(pos, cast_origin) <= radius_sq {
                    targets.push(id);
                }
            }
        } else {
            for enemy in storage::enemies(world) {
                if distance_sq(enemy.position.pos, cast_origin) <= radius_sq {
                    targets.push(enemy.role.id);
                }
            }
        }

        for enemy_id in targets {
            self.apply_enemy_damage(
                world,
                enemy_id,
                HERO_ABILITY_DAMAGE * damage_multiplier,
                client_id,
                reliable_events,
            );
        }
    }

    pub(crate) fn try_start_regular_attack(&mut self, world: &mut World, client_id: u64) {
        let Some(hero) = hero_mut(world, &client_id) else {
            return;
        };
        if is_charge_locked(hero.role.charge_state) {
            return;
        }

        let regular_attack_profile = hero_regular_attack_profile(&hero.role, hero.attack.profile);
        let _ = start_attack(&mut hero.attack.state, regular_attack_profile);
    }

    pub(crate) fn try_set_charging(&mut self, world: &mut World, client_id: u64, active: bool) {
        let Some(mut hero) = hero_mut(world, &client_id) else {
            return;
        };
        set_charge_input(&mut hero.role, hero.attack.state, active);
    }

    pub(crate) fn try_set_lock_target(
        &mut self,
        world: &mut World,
        client_id: u64,
        target_id: Option<u64>,
    ) {
        let validated_target_id = match target_id {
            Some(enemy_id) if enemy(world, &enemy_id).is_some() => Some(enemy_id),
            Some(_) => return,
            None => None,
        };

        let Some(hero) = hero_mut(world, &client_id) else {
            return;
        };
        hero.role.lock_mode_active = validated_target_id.is_some();
        hero.role.lock_target_id = validated_target_id;
    }

    pub(crate) fn try_build_tower(
        &mut self,
        world: &mut World,
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

        let Some(hero) = storage::hero(world, &client_id) else {
            return;
        };

        if distance_sq(hero.position.pos, node.pos)
            > BUILD_COMMAND_MAX_DISTANCE * BUILD_COMMAND_MAX_DISTANCE
        {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::TooFar,
            });
            return;
        }

        if hero.role.gold < TOWER_BUILD_COST {
            reliable_events.push(ReliableGameEvent::BuildRejected {
                owner: client_id,
                node_id,
                reason: BuildRejectReason::InsufficientGold,
            });
            return;
        }

        if let Some(hero_mut) = hero_mut(world, &client_id) {
            hero_mut.role.gold -= TOWER_BUILD_COST;
        }

        let tower_id = self.alloc_entity_id(world);
        insert_tower(
            world,
            tower_id,
            TowerBundle {
                role: TowerState {
                    id: tower_id,
                    owner: client_id,
                    lane: 0,
                    node_id: node.node_id,
                    reload_ticks: 0,
                },
                position: Position { pos: node.pos },
            },
        );
        self.node_occupancy.insert(node.node_id, tower_id);
        world.resource_mut::<NavigationCache>().dirty = true;

        reliable_events.push(ReliableGameEvent::TowerBuilt {
            owner: client_id,
            node_id,
            tower_id,
        });
    }

    pub(crate) fn resolve_hero_attacks(
        &mut self,
        world: &mut World,
        triggered_attackers: Vec<u64>,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        self.resolve_hero_collisions(world);

        if triggered_attackers.is_empty() {
            return;
        }
        // Geometry is fixed throughout this damage phase. Read it once rather
        // than walking the identity index and fetching every actor component
        // for every attacker. Earlier attackers can still remove targets.
        let epoch = world.resource::<Epoch>().0;
        let mut query = world
            .query_filtered::<(Entity, &game_replication::NetId, &Position), With<EnemyState>>();
        let targets: Vec<_> = query
            .iter(world)
            .filter(|(_, id, _)| {
                id.epoch == epoch && id.namespace == game_shared::actor_namespace::ENEMY
            })
            .map(|(entity, id, position)| (entity, id.value, position.pos))
            .collect();

        for attacker_id in triggered_attackers {
            let Some(hero) = storage::hero(world, &attacker_id) else {
                continue;
            };
            let regular_attack_profile =
                hero_regular_attack_profile(&hero.role, hero.attack.profile);

            let mut hit_targets: Vec<(u64, f32)> = targets
                .iter()
                .filter_map(|&(entity, id, pos)| {
                    (directional_attack_can_hit(
                        hero.position.pos,
                        hero.facing.direction.dir,
                        pos,
                        regular_attack_profile,
                    ) && world.get::<EnemyState>(entity).is_some())
                    .then_some((id, distance_sq(hero.position.pos, pos)))
                })
                .collect();

            if hit_targets.is_empty() {
                continue;
            }

            hit_targets.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

            if hero.role.lock_mode_active
                && let Some(locked_target_id) = hero.role.lock_target_id
                && let Some(locked_idx) = hit_targets
                    .iter()
                    .position(|(enemy_id, _)| *enemy_id == locked_target_id)
            {
                hit_targets.swap(0, locked_idx);
            }

            let damage = regular_attack_profile.damage * hero_attack_damage_multiplier(&hero.role);
            for (enemy_id, _) in hit_targets {
                self.apply_enemy_damage(world, enemy_id, damage, attacker_id, reliable_events);
            }
        }
    }

    pub(crate) fn apply_hero_damage(&mut self, world: &mut World, hero_id: u64, damage: f32) {
        let Some(hero) = hero_mut(world, &hero_id) else {
            return;
        };

        hero.health.hp = (hero.health.hp - damage).max(0.0);
        if hero.health.hp > 0.0 {
            return;
        }

        hero.role.respawn_generation = hero.role.respawn_generation.wrapping_add(1);
        hero.position.pos = spawn_position_for_client(hero_id);
        hero.role.move_dir = [0.0, 0.0];
        hero.facing.direction = FacingComponent::default();
        hero.role.lock_mode_active = false;
        hero.role.lock_target_id = None;
        hero.attack.state = DirectionalAttackStateComponent::default();
        hero.role.charge_state = ChargeStateComponent::default();
        hero.health.hp = HERO_MAX_HP;
        hero.role.mana = HERO_MAX_MANA;
    }

    pub(crate) fn apply_objective_damage(
        &mut self,
        _world: &mut World,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
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

    pub(crate) fn apply_enemy_damage(
        &mut self,
        world: &mut World,
        enemy_id: u64,
        damage: f32,
        killer: u64,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        let mut reward = None;

        if let Some(enemy) = enemy_mut(world, &enemy_id) {
            enemy.health.hp -= damage;
            if enemy.health.hp <= 0.0 {
                reward = Some(enemy.role.reward);
            }
        }

        let Some(reward) = reward else {
            return;
        };

        if remove_enemy(world, &enemy_id).is_none() {
            return;
        }

        if let Some(hero) = hero_mut(world, &killer) {
            hero.role.gold = hero.role.gold.saturating_add(reward);
        }

        reliable_events.push(ReliableGameEvent::EnemyKilled {
            enemy_id,
            killer,
            reward,
        });
    }
}
