//! Dreamlance content: authoritative origin and resources, immutable historical targets.
use super::*;
use crate::combat::*;
use crate::combat_history::CombatHistory;
use engine_core::{
    CombatPose, HistoryError, HistoryLimits, HitHistory, HitMetadata, HitQuery, QueryBudget,
};

#[derive(Resource, Default)]
pub(crate) struct StaticCombatHistory {
    identity: Option<[u8; 32]>,
    prepared: Option<HitHistory>,
}
impl StaticCombatHistory {
    fn update(&mut self, collision: &CollisionWorld) -> Result<(), CombatReason> {
        if self.identity == Some(collision.identity()) {
            return Ok(());
        }
        let colliders = collision.manifest().colliders();
        let mut prepared = HitHistory::new(HistoryLimits {
            frames: 1,
            poses_per_frame: 64,
            total_poses: 64,
        })
        .map_err(|_| CombatReason::Budget)?;
        let poses = colliders
            .iter()
            .map(|c| CombatPose {
                entity: c.key,
                segment: 1,
                pose_revision: collision.manifest().scene_revision(),
                position: Vec3::from_array(c.position),
                rotation: Quat::from_array(c.rotation),
                shape: c.shape,
                metadata: HitMetadata::default(),
            })
            .collect::<Vec<_>>();
        prepared
            .capture(1, collision.manifest().scene_revision(), &poses)
            .map_err(|_| CombatReason::Budget)?;
        self.identity = Some(collision.identity());
        self.prepared = Some(prepared);
        Ok(())
    }
}
pub(crate) fn tick_dreamlance(mut heroes: Query<&mut RayState>) {
    for mut ray in &mut heroes {
        if ray.cooldown > 0.0 {
            ray.tick();
        }
    }
}
fn verdict(ray: ValidatedRay, tick: u32, query: u32, reason: CombatReason) -> ActionVerdict {
    let action_reason = match reason {
        CombatReason::Hit
        | CombatReason::Miss
        | CombatReason::StaticBlocker
        | CombatReason::DynamicBlocker
        | CombatReason::HistoricalInvulnerability => ActionReason::Accepted,
        CombatReason::MissingHistory => ActionReason::MissingHistory,
        CombatReason::Cooldown => ActionReason::Cooldown,
        CombatReason::NoAmmo => ActionReason::Resource,
        CombatReason::Budget => ActionReason::Budget,
        CombatReason::NotEligible => ActionReason::Inactive,
        _ => ActionReason::InvalidAction,
    };
    ActionVerdict {
        key: ray.key,
        execution_server_tick: ray.execution_server_tick,
        execution_gameplay_tick: tick,
        reason: action_reason,
        combat: Some(CombatVerdict {
            key: ray.key,
            execution_server_tick: ray.execution_server_tick,
            execution_gameplay_tick: tick,
            query_server_tick: ray.query_server_tick,
            query_gameplay_tick: query,
            reason,
            target: None,
            hit_region: 0,
            damage: 0.0,
            transaction: None,
        }),
    }
}
pub(crate) fn ray_actions(
    run: Res<Run>,
    collision: Res<CollisionWorld>,
    history: Res<CombatHistory>,
    mut static_history: ResMut<StaticCombatHistory>,
    mut transactions: ResMut<ActionTransactions>,
    mut batch: ResMut<CombatBatch>,
    mut trace: ResMut<crate::combat_trace::CombatTraceState>,
    heroes: Query<HeroActorReadOnly>,
) {
    let explicit = transactions.requests.clone();
    let mut requests = Vec::new();
    for action in &explicit {
        match action {
            CombatAction::Ray(ray) => requests.push((*ray, None, false)),
            CombatAction::BeamBegin(ray) => requests.push((*ray, None, true)),
            _ => {}
        }
    }
    for hero in &heroes {
        let mut next = hero.ray.clone();
        for action in &explicit {
            match action {
                CombatAction::BeamStop {
                    key,
                    execution_server_tick,
                } if key.actor == hero.view.id => {
                    let reason = if next.last_key.is_some_and(|last| {
                        last.same_stream(*key) && key.command_sequence <= last.command_sequence
                    }) {
                        ActionReason::InvalidAction
                    } else {
                        next.beam = None;
                        next.last_key = Some(*key);
                        ActionReason::Accepted
                    };
                    transactions.verdicts.push(ActionVerdict {
                        key: *key,
                        execution_server_tick: *execution_server_tick,
                        execution_gameplay_tick: run.tick,
                        reason,
                        combat: None,
                    });
                }
                CombatAction::BeamAim(sample) if sample.key.actor == hero.view.id => {
                    if let Some(beam) = &mut next.beam
                        && sample.key.valid()
                        && sample.key.same_stream(beam.key)
                        && sample.key.command_sequence > beam.sample.key.command_sequence
                        && valid_beam_sample(*sample, run.tick)
                    {
                        beam.sample = saved_sample(*sample, run.tick);
                    }
                }
                _ => {}
            }
        }
        if let Some(beam) = &next.beam {
            if !hero.active
                || hero.health.hp <= 0.0
                || hero.motion.dash_ticks > 0
                || hero.motion_status.last_error.is_some()
                || run.tick - beam.started_tick >= BEAM_DURATION_TICKS
                || run.tick - beam.sample.accepted_gameplay_tick > 9
            {
                next.beam = None;
            } else if run.tick - beam.last_damage_tick >= BEAM_DAMAGE_TICKS {
                let sample = &beam.sample;
                if let Some(execution_server_tick) = sample
                    .execution_server_tick
                    .checked_add(u64::from(run.tick - sample.accepted_gameplay_tick))
                {
                    requests.push((
                        ValidatedRay {
                            command_fraction: None,
                            query_fraction: sample.query_fraction,
                            key: beam.key,
                            execution_server_tick,
                            query_server_tick: sample.query_server_tick,
                            query_gameplay_tick: sample.query_gameplay_tick,
                            aim: sample.aim,
                        },
                        Some(beam.clone()),
                        false,
                    ));
                } else {
                    next.beam = None;
                }
            }
        }
        if next != *hero.ray {
            transactions.ray_commits.push((hero.view.id, next));
        }
    }
    let mut budget = QueryBudget::new(32_768, 4096).expect("bounded game ray budget");
    for (ray, episode, begin) in requests {
        if episode.is_some() && transactions.internal_verdict_start.is_none() {
            transactions.internal_verdict_start = Some(transactions.verdicts.len());
        }
        let query = if ray.key.connection_epoch == 0 {
            run.tick
        } else {
            ray.query_gameplay_tick
        };
        let trace_index = trace.begin(ray, run.tick, query, transactions.verdicts.len());
        if let Some(index) = trace_index {
            trace.pending[index].record.sample_key =
                episode.as_ref().map_or(ray.key, |beam| beam.sample.key);
        }
        let reject = |reason| verdict(ray, run.tick, query, reason);
        let Some(hero) = heroes.iter().find(|h| {
            h.view.id == ray.key.actor && h.combat_identity.generation == ray.key.actor_generation
        }) else {
            transactions
                .verdicts
                .push(reject(CombatReason::WrongLifecycle));
            continue;
        };
        if !hero.active
            || hero.health.hp <= 0.0
            || hero.motion.dash_ticks > 0
            || hero.motion_status.last_error.is_some()
        {
            transactions
                .verdicts
                .push(reject(CombatReason::NotEligible));
            continue;
        }
        if ray.query_server_tick > ray.execution_server_tick {
            transactions
                .verdicts
                .push(reject(CombatReason::InvalidAction));
            continue;
        }
        if episode.is_none() && hero.ray.beam.is_some() {
            transactions
                .verdicts
                .push(reject(CombatReason::NotEligible));
            continue;
        }
        if begin && u32::try_from(ray.key.command_sequence).is_err() {
            transactions
                .verdicts
                .push(reject(CombatReason::InvalidAction));
            continue;
        }
        if let Err(reason) = if episode.is_none() {
            hero.ray.fire_eligibility(ray.key, ray.aim)
        } else {
            Ok(())
        } {
            transactions.verdicts.push(reject(reason));
            continue;
        }
        if episode
            .as_ref()
            .is_some_and(|beam| (run.tick - beam.started_tick) % BEAM_AMMO_TICKS == 0)
            && hero.ray.ammo.current() < 1.0
        {
            let mut next = hero.ray.clone();
            next.beam = None;
            transactions.ray_commits.push((hero.view.id, next));
            transactions.verdicts.push(reject(CombatReason::NoAmmo));
            continue;
        }
        let aim = normalize(ray.aim);
        // Network timing already clamps C/R on the match clock. This independent
        // game-clock cap also prevents an adapter from inventing arbitrarily old poses.
        let maximum_rewind = (0.150 / DT).ceil() as u32;
        if query > run.tick || run.tick - query > maximum_rewind {
            transactions
                .verdicts
                .push(reject(CombatReason::MissingHistory));
            continue;
        }
        let scene = collision.manifest().scene_revision();
        let origin = if let Some(fraction) = ray.command_fraction {
            let Some(previous) = run.tick.checked_sub(1) else {
                transactions
                    .verdicts
                    .push(reject(CombatReason::MissingHistory));
                continue;
            };
            let pose = match history.prepared.sample_pose(
                u64::from(previous),
                fraction,
                scene,
                engine_core::ColliderKey {
                    index: hero.view.id,
                    generation: hero.combat_identity.generation,
                },
                hero.combat_identity.segment,
                &mut budget,
            ) {
                Ok(pose) => pose,
                Err(error) => {
                    transactions.verdicts.push(reject(history_reason(error)));
                    continue;
                }
            };
            if pose.metadata.0[0] & ((1 << 8) | (1 << 9)) != (1 << 8) | (1 << 9) {
                transactions
                    .verdicts
                    .push(reject(CombatReason::NotEligible));
                continue;
            }
            pose.position
        } else {
            Vec3::from_array(hero.motion.position)
                + Vec3::Y
                    * if hero.motion.stance == engine_core::Stance::Crouched {
                        0.8
                    } else {
                        0.9
                    }
        };
        if let Some(index) = trace_index {
            trace.pending[index].record.muzzle = Some(origin.to_array());
        }
        let interpolated = if ray.query_fraction == 0 {
            None
        } else {
            match history.prepared.sample_frame(
                u64::from(query),
                ray.query_fraction,
                scene,
                &mut budget,
                ray_target,
            ) {
                Ok(frame) => Some(frame),
                Err(error) => {
                    transactions.verdicts.push(reject(history_reason(error)));
                    continue;
                }
            }
        };
        let frame = if let Some(frame) = &interpolated {
            frame
        } else {
            match history.prepared.frame(u64::from(query), scene) {
                Ok(frame) => frame,
                Err(error) => {
                    transactions.verdicts.push(reject(history_reason(error)));
                    continue;
                }
            }
        };
        if batch.error.is_some()
            || !batch.has_capacity()
            || static_history.update(&collision).is_err()
        {
            transactions.verdicts.push(reject(CombatReason::Budget));
            continue;
        }
        let ray_geometry = HitQuery::Ray {
            ray: Ray3d::new(
                origin,
                Dir3::new(Vec3::new(aim[0], 0.0, aim[1])).expect("normalized planar aim"),
            ),
            distance: DREAMLANCE_RANGE,
        };
        let Ok(targets) = frame.query(ray_geometry, &mut budget, ray_target) else {
            transactions.verdicts.push(reject(CombatReason::Budget));
            continue;
        };
        let static_frame = static_history
            .prepared
            .as_ref()
            .unwrap()
            .frame(1, collision.manifest().scene_revision())
            .expect("prepared static scene");
        let Ok(walls) = static_frame.query(ray_geometry, &mut budget, |_| true) else {
            transactions.verdicts.push(reject(CombatReason::Budget));
            continue;
        };
        let target = targets.first();
        let wall = walls.first();
        let reason = if wall.is_some_and(|w| target.is_none_or(|t| w.distance <= t.distance)) {
            CombatReason::StaticBlocker
        } else if target.is_some_and(|t| t.pose.metadata.0[0] & 255 == 0) {
            CombatReason::DynamicBlocker
        } else if target.is_some_and(|t| t.pose.metadata.0[0] & (1 << 10) != 0) {
            CombatReason::HistoricalInvulnerability
        } else if target.is_some() {
            CombatReason::Hit
        } else {
            CombatReason::Miss
        };
        if reason == CombatReason::StaticBlocker {
            if let Some(wall) = wall {
                trace.select(trace_index, &wall.pose);
            }
        } else if let Some(target) = target {
            trace.select(trace_index, &target.pose);
        }
        let mut result = reject(reason);
        if reason == CombatReason::Hit {
            let target = target.unwrap().pose.entity;
            // Historical target must still be that lifecycle at commitment; a
            // new generation never inherits an old hit's health transaction.
            let Some(current) = history.archive.frames.last().and_then(|f| {
                f.poses
                    .iter()
                    .find(|p| p.id == target.index && p.generation == target.generation)
            }) else {
                transactions
                    .verdicts
                    .push(reject(CombatReason::WrongLifecycle));
                continue;
            };
            if !current.alive {
                transactions
                    .verdicts
                    .push(reject(CombatReason::NotEligible));
                continue;
            }
            let receipt = transactions.verdicts.len();
            if let Some(combat) = &mut result.combat {
                combat.target = Some(CombatTarget {
                    id: target.index,
                    generation: target.generation,
                });
                combat.hit_region = 1;
                combat.transaction = Some(DamageTransaction {
                    action: ray.key,
                    target: combat.target.unwrap(),
                    hit_slot: episode
                        .as_ref()
                        .map_or(0, |beam| (run.tick - beam.started_tick) as u16),
                });
            }
            stage_ray_damage(
                &mut batch,
                CombatActionKey::new(
                    run.tick,
                    ray.key.actor,
                    ray.key.actor_generation,
                    4,
                    ray.key.command_sequence,
                    u16::from(ray.key.action_slot),
                ),
                target.index,
                target.generation,
                query,
                if begin || episode.is_some() {
                    4.0
                } else {
                    DREAMLANCE_DAMAGE
                } * hero.view.attack_power,
                receipt,
            );
        }
        let mut next = transactions
            .ray_commits
            .iter()
            .rev()
            .find(|(id, _)| *id == hero.view.id)
            .map_or_else(|| hero.ray.clone(), |(_, state)| state.clone());
        if let Some(mut beam) = episode {
            if (run.tick - beam.started_tick) % BEAM_AMMO_TICKS == 0 {
                assert!(
                    next.ammo.try_spend(1.0),
                    "beam resource preflight precedes query"
                );
            }
            beam.last_damage_tick = run.tick;
            next.beam = Some(beam);
        } else {
            next.accept(ray.key);
            if begin {
                next.beam = Some(BeamEpisode {
                    key: ray.key,
                    started_tick: run.tick,
                    last_damage_tick: run.tick,
                    sample: saved_sample(ray, run.tick),
                });
            }
        }
        transactions.ray_commits.push((hero.view.id, next));
        transactions.verdicts.push(result);
    }
}

fn ray_target(pose: &CombatPose) -> bool {
    let flags = pose.metadata.0[0];
    let faction = flags & 255;
    faction == 0 || (faction == 2 && flags & (1 << 9) != 0)
}
fn history_reason(error: HistoryError) -> CombatReason {
    match error {
        HistoryError::QueryBudget | HistoryError::ResultBudget | HistoryError::Capacity => {
            CombatReason::Budget
        }
        HistoryError::WrongLifecycle
        | HistoryError::Discontinuity
        | HistoryError::MissingEntity => CombatReason::WrongLifecycle,
        _ => CombatReason::MissingHistory,
    }
}

fn valid_beam_sample(sample: ValidatedRay, tick: u32) -> bool {
    sample.query_server_tick <= sample.execution_server_tick
        && sample.query_gameplay_tick <= tick
        && (sample.query_fraction == 0 || sample.query_gameplay_tick < tick)
        && tick - sample.query_gameplay_tick <= 9
        && sample.aim.iter().all(|v| v.is_finite() && v.abs() <= 1.0)
        && length(normalize(sample.aim)) > 0.0
}
fn saved_sample(sample: ValidatedRay, tick: u32) -> BeamSample {
    BeamSample {
        key: sample.key,
        execution_server_tick: sample.execution_server_tick,
        query_server_tick: sample.query_server_tick,
        query_gameplay_tick: sample.query_gameplay_tick,
        query_fraction: sample.query_fraction,
        accepted_gameplay_tick: tick,
        aim: sample.aim,
    }
}

pub(crate) fn beam_graphics(
    mut commands: Commands,
    heroes: Query<HeroActorReadOnly>,
    mut graphics: Query<(Entity, &mut engine_core::GraphicsInstance)>,
) {
    let mut active = std::collections::BTreeSet::new();
    for hero in &heroes {
        if hero.ray.beam.is_none() {
            continue;
        }
        if !hero.active || hero.health.hp <= 0.0 {
            continue;
        }
        let Some(next) = hero.ray.graphics(&hero.motion) else {
            continue;
        };
        let id = next.id;
        active.insert((id.match_epoch, id.owner, id.action_seq, id.slot));
        if let Some((_, mut current)) = graphics.iter_mut().find(|(_, current)| current.id == id) {
            if *current != next {
                *current = next;
            }
        } else {
            commands.spawn((DreamOwned, next));
        }
    }
    for (entity, current) in &graphics {
        if matches!(current.kind, engine_core::GraphicsKind::Beam { .. })
            && !active.contains(&(
                current.id.match_epoch,
                current.id.owner,
                current.id.action_seq,
                current.id.slot,
            ))
        {
            commands.entity(entity).despawn();
        }
    }
}
