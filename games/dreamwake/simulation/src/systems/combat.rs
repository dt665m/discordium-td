use super::*;
use crate::combat_history::{CombatHistory, SavedCombatFrame, SavedCombatPose};

const MAX_DAMAGE_INTENTS: usize = 4096;
/// Stable simulation action identity. Target identity is appended when sorting
/// impacts; neither query iteration nor packet arrival order allocates this key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CombatActionKey {
    pub tick: u32,
    pub actor: u64,
    pub generation: u32,
    pub kind: u8,
    pub sequence: u64,
    pub slot: u16,
}
impl CombatActionKey {
    pub fn new(tick: u32, actor: u64, generation: u32, kind: u8, sequence: u64, slot: u16) -> Self {
        Self {
            tick,
            actor,
            generation,
            kind,
            sequence,
            slot,
        }
    }
}
#[derive(Debug, Clone)]
struct DamageIntent {
    action: CombatActionKey,
    target: u64,
    query_tick: u32,
    generation: u32,
    friendly: bool,
    damage: f32,
    essence: Option<EssenceKind>,
    knockback: [f32; 2],
    ability: bool,
    receipt: Option<usize>,
}
#[derive(Resource, Default)]
pub(crate) struct CombatBatch {
    intents: Vec<DamageIntent>,
    pub error: Option<&'static str>,
}
impl CombatBatch {
    pub fn has_capacity(&self) -> bool {
        self.error.is_none() && self.intents.len() < MAX_DAMAGE_INTENTS
    }
    fn push(&mut self, intent: DamageIntent) {
        if self.error.is_some() {
            return;
        }
        if self.intents.len() >= MAX_DAMAGE_INTENTS {
            self.error = Some("combat intent budget");
            self.intents.clear();
        } else if intent.damage.is_finite() && intent.damage >= 0.0 {
            self.intents.push(intent);
        } else {
            self.error = Some("invalid combat damage");
            self.intents.clear();
        }
    }
}
pub(crate) fn heal(hero: &mut HeroActorItem<'_, '_>, amount: f32) {
    if hero.active && hero.health.hp > 0.0 {
        hero.health.heal(amount);
    }
}
pub(crate) fn hit_enemy(
    batch: &mut CombatBatch,
    action: CombatActionKey,
    hero: &HeroActorItem<'_, '_>,
    enemy: &EnemyActorItem<'_, '_>,
    damage: f32,
    essence: Option<EssenceKind>,
    knockback: [f32; 2],
    ability: bool,
) {
    if !hero.active || enemy.health.hp <= 0.0 {
        return;
    }
    batch.push(DamageIntent {
        action,
        target: enemy.view.id,
        query_tick: action.tick,
        generation: enemy.combat_identity.generation,
        friendly: true,
        damage,
        essence,
        knockback,
        ability,
        receipt: None,
    });
}
pub(crate) fn hit_hero(
    batch: &mut CombatBatch,
    action: CombatActionKey,
    hero: &HeroActorItem<'_, '_>,
    damage: f32,
) {
    if !hero.active || hero.health.hp <= 0.0 {
        return;
    }
    batch.push(DamageIntent {
        action,
        target: hero.view.id,
        query_tick: action.tick,
        generation: hero.combat_identity.generation,
        friendly: false,
        damage,
        essence: None,
        knockback: [0.0; 2],
        ability: false,
        receipt: None,
    });
}

/// Game authority submits only a validated historical target identity. The ray
/// query and network timing adapter remain separate from damage commitment.
#[cfg(test)]
pub(crate) fn stage_historical_damage(
    batch: &mut CombatBatch,
    action: CombatActionKey,
    target: u64,
    generation: u32,
    query_tick: u32,
    damage: f32,
) {
    batch.push(DamageIntent {
        action,
        target,
        query_tick,
        generation,
        friendly: true,
        damage,
        essence: None,
        knockback: [0.0; 2],
        ability: true,
        receipt: None,
    });
}

pub(crate) fn stage_ray_damage(
    batch: &mut CombatBatch,
    action: CombatActionKey,
    target: u64,
    generation: u32,
    query_tick: u32,
    damage: f32,
    receipt: usize,
) {
    batch.push(DamageIntent {
        action,
        target,
        query_tick,
        generation,
        friendly: true,
        damage,
        essence: None,
        knockback: [0.0; 2],
        ability: true,
        receipt: Some(receipt),
    });
}

pub(crate) fn advance_combat_owner(
    identity: &mut CombatIdentity,
    defense: &mut DefenseEpisodes,
    combat: &engine_core::CombatState,
    tick: u32,
) -> Result<(), &'static str> {
    let mut next = *identity;
    next.advance(tick)?;
    defense.reconcile(combat, tick)?;
    *identity = next;
    Ok(())
}
/// Authoritative collision rig is evaluated headlessly after action movement and
/// defense activation, before any eligible damage changes health or hit immunity.
pub(crate) fn capture_combat_poses(
    run: Res<Run>,
    collision: Res<CollisionWorld>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
    covers: Query<(&Cover, &CombatIdentity), (Without<Hero>, Without<Enemy>)>,
    mut history: ResMut<CombatHistory>,
    mut batch: ResMut<CombatBatch>,
) {
    let mut poses = Vec::new();
    for mut hero in &mut heroes {
        let combat = hero.combat.clone();
        if let Err(error) = advance_combat_owner(
            &mut hero.combat_identity,
            &mut hero.defense_episodes,
            &combat,
            run.tick,
        ) {
            batch.error = Some(error);
            return;
        }
        let height = if hero.motion.stance == engine_core::Stance::Crouched {
            1.6
        } else {
            1.8
        };
        let radius = 0.8;
        poses.push(SavedCombatPose {
            id: hero.view.id,
            generation: hero.combat_identity.generation,
            segment: hero.combat_identity.segment,
            pose_revision: hero.combat_identity.pose_revision,
            position: [
                hero.motion.position[0],
                hero.motion.position[1] + height * 0.5,
                hero.motion.position[2],
            ],
            rotation: Quat::IDENTITY.to_array(),
            shape: engine_core::CollisionShape::Capsule {
                half_segment: height * 0.5 - radius,
                radius,
            },
            faction: 1,
            alive: hero.health.hp > 0.0,
            damageable: hero.active && hero.health.hp > 0.0,
            invulnerable: hero.combat.is_invulnerable() || hero.motion.dash_ticks > 0,
            shield_cutoff: hero.defense_episodes.cutoff(),
            shield: hero.combat.shield,
        });
    }
    for mut enemy in &mut enemies {
        let combat = enemy.combat.clone();
        if let Err(error) = advance_combat_owner(
            &mut enemy.combat_identity,
            &mut enemy.defense_episodes,
            &combat,
            run.tick,
        ) {
            batch.error = Some(error);
            return;
        }
        let radius = super::projectiles::enemy_radius(enemy.view.kind);
        let height = radius * 2.0;
        poses.push(SavedCombatPose {
            id: enemy.view.id,
            generation: enemy.combat_identity.generation,
            segment: enemy.combat_identity.segment,
            pose_revision: enemy.combat_identity.pose_revision,
            position: [
                enemy.motor.position[0],
                height * 0.5,
                enemy.motor.position[1],
            ],
            rotation: Quat::IDENTITY.to_array(),
            shape: engine_core::CollisionShape::Ball { radius },
            faction: 2,
            alive: enemy.health.hp > 0.0,
            damageable: enemy.health.hp > 0.0,
            invulnerable: enemy.combat.is_invulnerable(),
            shield_cutoff: enemy.defense_episodes.cutoff(),
            shield: enemy.combat.shield,
        });
    }
    for (cover, identity) in &covers {
        if !cover.present {
            continue;
        }
        poses.push(SavedCombatPose {
            id: cover.id,
            generation: identity.generation,
            segment: identity.segment,
            pose_revision: identity.pose_revision,
            position: cover.position(identity.tick),
            rotation: Quat::IDENTITY.to_array(),
            shape: engine_core::CollisionShape::Box {
                half_extents: COVER_HALF_EXTENTS,
            },
            faction: 0,
            alive: true,
            damageable: false,
            invulnerable: false,
            shield_cutoff: 0,
            shield: 0.0,
        });
    }
    if let Err(error) = history.capture(SavedCombatFrame {
        tick: run.tick,
        scene: collision.manifest().scene_revision(),
        poses,
    }) {
        batch.error = Some(error);
    }
}

pub(crate) fn resolve_combat_batch(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut batch: ResMut<CombatBatch>,
    history: Res<CombatHistory>,
    mut transactions: ResMut<crate::combat::ActionTransactions>,
    mut trace: ResMut<crate::combat_trace::CombatTraceState>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<EnemyActor, Without<Hero>>,
) {
    if batch.error.is_some() {
        batch.intents.clear();
        transactions.fail_pending(run.tick);
        return;
    }
    let Some(frame) = history.archive.frames.last().filter(|f| f.tick == run.tick) else {
        batch.error = Some("missing current combat poses");
        batch.intents.clear();
        transactions.fail_pending(run.tick);
        return;
    };
    let mut intents = std::mem::take(&mut batch.intents);
    intents.sort_by_key(|i| (i.action, i.target, i.generation));
    intents.dedup_by_key(|i| (i.action, i.target, i.generation));
    // Freeze eligibility and damage modifiers before applying HP, statuses,
    // knockback or post-hit invulnerability. A same-time lethal hit allows trades.
    let mut resolved = Vec::with_capacity(intents.len());
    let mut random = std::collections::BTreeMap::new();
    for intent in intents {
        let Some(query_frame) = history.logical_frame(intent.query_tick, frame.scene) else {
            batch.error = Some("missing historical combat poses");
            transactions.fail_pending(run.tick);
            return;
        };
        let Some(pose) = query_frame
            .poses
            .iter()
            .find(|p| p.id == intent.target && p.generation == intent.generation && p.damageable)
        else {
            continue;
        };
        if intent.friendly {
            let Some(hero) = heroes.iter().find(|h| {
                h.view.id == intent.action.actor
                    && h.combat_identity.generation == intent.action.generation
            }) else {
                continue;
            };
            let Some(enemy) = enemies.iter().find(|e| {
                e.view.id == intent.target && e.combat_identity.generation == intent.generation
            }) else {
                continue;
            };
            let stream = random
                .entry(hero.view.id)
                .or_insert_with(|| hero.critical_rng.clone());
            let Ok(roll) = stream.next_unit_f32() else {
                batch.error = Some("critical random stream exhausted");
                transactions.fail_pending(run.tick);
                return;
            };
            let critical = roll < hero.view.critical_chance;
            let amount = intent.damage
                * if critical { 1.9 } else { 1.0 }
                * if intent.ability && enemy.combat.status(FROST_STATUS).is_some() {
                    1.20
                } else {
                    1.0
                };
            resolved.push((intent, pose, amount, critical));
        } else {
            let Some(hero) = heroes.iter().find(|h| {
                h.view.id == intent.target && h.combat_identity.generation == intent.generation
            }) else {
                continue;
            };
            let amount =
                intent.damage * (1.0 - hero.view.defense) * if run.lucid { 1.22 } else { 1.0 };
            resolved.push((intent, pose, amount, false));
        }
    }
    for (id, stream) in random {
        if let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == id) {
            hero.critical_rng = stream;
        }
    }
    for (id, ray) in std::mem::take(&mut transactions.ray_commits) {
        if let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == id) {
            *hero.ray = ray;
        }
    }
    let mut healing = Vec::new();
    let mut hit_heroes = Vec::new();
    for (intent, pose, amount, critical) in resolved {
        if pose.invulnerable {
            continue;
        }
        if intent.friendly {
            let Some(mut enemy) = enemies.iter_mut().find(|e| {
                e.view.id == intent.target && e.combat_identity.generation == intent.generation
            }) else {
                continue;
            };
            let trace_index = intent.receipt.and_then(|receipt| {
                trace
                    .pending
                    .iter()
                    .position(|entry| entry.receipt == receipt)
            });
            if let Some(index) = trace_index {
                trace.pending[index].record.shield_credits = enemy
                    .defense_episodes
                    .episodes
                    .iter()
                    .filter(|e| {
                        e.id <= pose.shield_cutoff
                            && e.activated_tick <= intent.query_tick
                            && intent.query_tick < e.expires_tick
                    })
                    .take(crate::combat_trace::MAX_COMBAT_TRACE_CREDITS)
                    .map(|e| crate::combat_trace::CombatTraceShieldCredit {
                        id: e.id,
                        activated_tick: e.activated_tick,
                        expires_tick: e.expires_tick,
                        granted: e.granted,
                        spent_before: e.spent,
                        spent_after: e.spent,
                    })
                    .collect();
            }
            let (absorbed, current) = enemy.defense_episodes.absorb(
                intent.query_tick,
                pose.shield_cutoff,
                pose.shield,
                amount,
                run.tick,
            );
            if let Some(index) = trace_index {
                for credit in &mut trace.pending[index].record.shield_credits {
                    if let Some(episode) = enemy
                        .defense_episodes
                        .episodes
                        .iter()
                        .find(|e| e.id == credit.id)
                    {
                        credit.spent_after = episode.spent;
                    }
                }
            }
            enemy.combat.shield = (enemy.combat.shield - current).max(0.0);
            let hp_damage = enemy.health.damage((amount - absorbed).max(0.0));
            if let Some(receipt) = intent.receipt {
                if let Some(combat) = transactions
                    .verdicts
                    .get_mut(receipt)
                    .and_then(|v| v.combat.as_mut())
                {
                    combat.damage = hp_damage;
                }
            }
            enemy.view.hit_flash = 0.14;
            let response = if enemy.view.kind == EnemyKind::Boss {
                0.0
            } else {
                1.0
            };
            enemy.motor.displace(
                intent.knockback,
                response,
                Some(engine_core::CircleBounds {
                    center: [0.0; 2],
                    radius: ARENA_RADIUS - 0.8,
                }),
            );
            if intent.essence == Some(EssenceKind::Frost) {
                enemy.combat.apply_status(
                    FROST_STATUS,
                    0.45,
                    2.5,
                    engine_core::DurationPolicy::Reset,
                );
            }
            if intent.essence == Some(EssenceKind::Leech) {
                healing.push((intent.action.actor, hp_damage * 0.12));
            }
            number(
                &mut commands,
                &mut run,
                enemy.motor.position,
                amount,
                critical,
                true,
            );
        } else {
            let Some(mut hero) = heroes.iter_mut().find(|h| {
                h.view.id == intent.target && h.combat_identity.generation == intent.generation
            }) else {
                continue;
            };
            let (absorbed, current) = hero.defense_episodes.absorb(
                intent.query_tick,
                pose.shield_cutoff,
                pose.shield,
                amount,
                run.tick,
            );
            hero.combat.shield = (hero.combat.shield - current).max(0.0);
            hero.health.damage((amount - absorbed).max(0.0));
            hero.view.hit_flash = 0.22;
            hit_heroes.push(intent.target);
            number(
                &mut commands,
                &mut run,
                planar_position(&hero.motion),
                amount,
                false,
                false,
            );
        }
    }
    for id in hit_heroes {
        if let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == id) {
            hero.combat
                .grant_invulnerability(0.23, engine_core::DurationPolicy::Reset);
        }
    }
    for (id, amount) in healing {
        if let Some(mut hero) = heroes.iter_mut().find(|h| h.view.id == id) {
            heal(&mut hero, amount);
        }
    }
}
pub(crate) fn begin_tick(
    mut run: ResMut<Run>,
    mut batch: ResMut<CombatBatch>,
    mut trace: ResMut<crate::combat_trace::CombatTraceState>,
) {
    trace.pending.clear();
    batch.intents.clear();
    batch.error = None;
    let Some(tick) = run.tick.checked_add(1) else {
        batch.error = Some("combat clock exhausted");
        return;
    };
    run.tick = tick;
    run.elapsed += DT;
}

/// Visual lifetimes continue through reward and ending scenes; pausing freezes them.
pub(crate) fn age_transients(
    mut commands: Commands,
    mut heroes: Query<&mut Hero>,
    mut enemies: Query<&mut Enemy>,
    mut numbers: Query<(Entity, &mut Number)>,
) {
    for mut hero in &mut heroes {
        age_owner_visuals(&mut hero.view);
    }
    for mut enemy in &mut enemies {
        if enemy.view.hit_flash > 0.0 {
            decrement(&mut enemy.view.hit_flash);
        }
    }
    for (entity, mut value) in &mut numbers {
        value.0.age += DT;
        if value.0.age > 0.85 {
            commands.entity(entity).despawn();
        }
    }
}
