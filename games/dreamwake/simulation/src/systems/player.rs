use super::*;

pub(crate) fn player_actions(
    mut commands: Commands,
    mut run: ResMut<Run>,
    mut batch: ResMut<CombatBatch>,
    mut transactions: ResMut<crate::combat::ActionTransactions>,
    input: Res<Input>,
    collision: Res<crate::platform::MotionEnvironment>,
    mut heroes: Query<HeroActor, Without<Enemy>>,
    mut enemies: Query<(Entity, EnemyActor), Without<Hero>>,
    projectiles: Query<(&Projectile, &engine_core::ProjectileState)>,
    wisps: Query<&engine_core::CompanionState>,
    delayed: Query<&DelayedCast>,
) {
    // One pass per query rather than one per hero: these bounds are the owner
    // checkpoint's, so every hero reads the same tallies.
    type OwnerCounts = std::collections::HashMap<u64, usize>;
    let (mut occupied_counts, mut live_projectiles) = (OwnerCounts::new(), OwnerCounts::new());
    for (payload, state) in &projectiles {
        if payload.spawn.is_some() {
            *occupied_counts.entry(state.owner).or_default() += 1;
        }
        if !state.finished() {
            *live_projectiles.entry(state.owner).or_default() += 1;
        }
    }
    let mut live_wisps = OwnerCounts::new();
    for state in &wisps {
        if !state.expired {
            *live_wisps.entry(state.owner).or_default() += 1;
        }
    }
    let mut delayed_counts = OwnerCounts::new();
    for cast in &delayed {
        *delayed_counts.entry(cast.payload.owner).or_default() += 1;
        if cast.payload.spawn.is_some() {
            *occupied_counts.entry(cast.payload.owner).or_default() += 1;
        }
        if cast.payload.kind == MemoryKind::Wisp {
            *live_wisps.entry(cast.payload.owner).or_default() += 1;
        }
    }
    let mut party: Vec<_> = heroes.iter_mut().collect();
    party.sort_by_key(|h| h.view.id);
    for mut hero in party {
        if !hero.active || hero.health.hp <= 0.0 {
            hero.charge.state.interrupt();
            let speed = hero.view.movement_speed;
            if hero.motion.base.attachment.is_some() {
                hero.motion_status.last_error = advance_owner_motion(
                    &mut hero.motion,
                    &mut hero.combat,
                    speed,
                    DreamInput::default(),
                    &collision,
                )
                .err();
            }
            if hero.motion.base.attachment.is_none() {
                hero.motion.velocity = [0.0; 3];
            }
            continue;
        }
        let input = input
            .0
            .iter()
            .find(|(id, _)| *id == hero.view.id)
            .map(|(_, input)| *input)
            .unwrap_or_default();
        let dash_origin = planar_position(&hero.motion);
        let speed = hero.view.movement_speed;
        let charge_key = transactions.requests.iter().find_map(|r| match r {
            crate::combat::CombatAction::Charge { key, .. } if key.actor == hero.view.id => {
                Some(*key)
            }
            _ => None,
        });
        let motion = match advance_owner_charged_motion(
            &mut hero.charge,
            charge_key,
            &mut hero.motion,
            &mut hero.combat,
            speed,
            input,
            &collision,
        ) {
            Ok((motion, reason)) => {
                transactions.charge(hero.view.id, run.tick, reason);
                motion
            }
            Err(error) => {
                hero.motion_status.last_error = Some(error);
                transactions.charge(
                    hero.view.id,
                    run.tick,
                    crate::combat::ActionReason::Collision,
                );
                transactions.simple(
                    hero.view.id,
                    None,
                    run.tick,
                    crate::combat::ActionReason::Collision,
                );
                for slot in 0..4 {
                    transactions.simple(
                        hero.view.id,
                        Some(slot),
                        run.tick,
                        crate::combat::ActionReason::Collision,
                    );
                }
                continue;
            }
        };
        hero.motion_status.last_error = None;
        if input.dash {
            transactions.simple(
                hero.view.id,
                None,
                run.tick,
                if motion.dash_started {
                    crate::combat::ActionReason::Accepted
                } else {
                    crate::combat::ActionReason::Cooldown
                },
            );
        }
        if motion.dash_started {
            let source = transactions
                .requests
                .iter()
                .find_map(|request| match request {
                    crate::combat::CombatAction::Dash { key, .. } if key.actor == hero.view.id => {
                        Some(ActionEffect::network(*key, 0))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| ActionEffect::input(input.action_sequences[0], run.tick, 0));
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                source,
                dash_origin,
                1.5,
                18,
            );
        }
        if let Some(attack) = accept_owner_attack(
            &mut hero.actor.view,
            &mut hero.motion,
            &mut hero.action,
            input.attack,
            motion.dashing_this_tick,
        ) {
            let BasicAttack {
                empowered,
                reach,
                origin,
                direction,
                damage,
            } = attack;
            let mut hit_any = false;
            let mut ordered: Vec<_> = enemies.iter().map(|(e, v)| (v.view.id, e)).collect();
            ordered.sort_unstable();
            for (_, entity) in ordered {
                let Ok((_, mut enemy)) = enemies.get_mut(entity) else {
                    continue;
                };
                let offset = sub(enemy.motor.position, origin);
                let toward = normalize(offset);
                if enemy.health.hp > 0.0
                    && engine_core::contains_cone(
                        origin,
                        direction,
                        reach + 0.55,
                        -0.05,
                        enemy.motor.position,
                    )
                {
                    hit_any = true;
                    hit_enemy(
                        &mut batch,
                        CombatActionKey::new(
                            run.tick,
                            hero.view.id,
                            hero.combat_identity.generation,
                            0,
                            u64::from(hero.action.sequence),
                            0,
                        ),
                        &mut hero,
                        &mut enemy,
                        damage,
                        None,
                        scale(toward, if empowered { 0.75 } else { 0.25 }),
                        false,
                    );
                }
            }
            if empowered && hit_any {
                heal(&mut hero, 5.0);
                for slot in &mut hero.loadout.0 {
                    slot.reduce_cooldown(0.50);
                }
            }
            action_effect(
                &mut commands,
                &run,
                hero.view.id,
                ActionEffect {
                    sequence: hero.action.sequence,
                    slot: 0x8000,
                    scope: None,
                    match_epoch: None,
                },
                add(origin, scale(direction, 1.2)),
                reach * 0.55,
                13,
            );
        }
        let count = |counts: &OwnerCounts| counts.get(&hero.view.id).copied().unwrap_or_default();
        let mut occupied = count(&occupied_counts);
        let mut owner_projectiles = count(&live_projectiles);
        let mut owner_wisps = count(&live_wisps);
        let mut owner_delayed = count(&delayed_counts);
        for (index, pressed) in input.casts.into_iter().enumerate() {
            if !pressed {
                continue;
            }
            let key = transactions
                .requests
                .iter()
                .find_map(|request| match request {
                    crate::combat::CombatAction::Cast { key, slot, .. }
                        if key.actor == hero.view.id && usize::from(*slot) == index =>
                    {
                        Some(*key)
                    }
                    _ => None,
                })
                .unwrap_or(crate::combat::RayActionKey {
                    match_epoch: ((run.seed ^ (run.seed >> 32)) as u32).max(1),
                    connection_epoch: 0,
                    command_stream: 0,
                    ownership_epoch: 0,
                    actor: hero.view.id,
                    actor_generation: hero.combat_identity.generation,
                    command_sequence: u64::from(input.action_sequences[index + 1].max(run.tick)),
                    action_slot: index as u8,
                });
            let source = if key.connection_epoch == 0 {
                ActionEffect::input(
                    input.action_sequences[index + 1],
                    run.tick,
                    ((index + 1) as u16) << 12,
                )
            } else {
                ActionEffect::network(key, ((index + 1) as u16) << 12)
            };
            let memory = hero.loadout.0[index];
            let projectile_reservation = if memory.kind == MemoryKind::Starfall {
                crate::starfall::spawn_count(memory.modifier)
            } else {
                0
            };
            let wisp_reservation = if memory.kind == MemoryKind::Wisp {
                if matches!(memory.modifier, Some(EssenceKind::Twin | EssenceKind::Echo)) {
                    2
                } else {
                    1
                }
            } else {
                0
            };
            let delayed_reservation = usize::from(
                memory.modifier == Some(EssenceKind::Echo)
                    || (memory.modifier == Some(EssenceKind::Twin)
                        && matches!(
                            memory.kind,
                            MemoryKind::Crescent | MemoryKind::Nova | MemoryKind::Blink
                        )),
            );
            if owner_projectiles + projectile_reservation
                > crate::replication::MAX_OWNER_PROJECTILES
                || owner_wisps + wisp_reservation > crate::replication::MAX_OWNER_WISPS
                || owner_delayed + delayed_reservation > crate::replication::MAX_OWNER_DELAYED
            {
                transactions.simple(
                    hero.view.id,
                    Some(index as u8),
                    run.tick,
                    crate::combat::ActionReason::Budget,
                );
                continue;
            }
            if hero.loadout.0[index].kind == MemoryKind::Starfall {
                let result = if key.valid() && u32::try_from(key.command_sequence).is_ok() {
                    crate::starfall::accept(&mut hero.loadout.0[index], occupied)
                } else {
                    Err(crate::combat::ActionReason::InvalidAction)
                };
                transactions.simple(
                    hero.view.id,
                    Some(index as u8),
                    run.tick,
                    result
                        .err()
                        .unwrap_or(crate::combat::ActionReason::Accepted),
                );
                if result.is_err() {
                    continue;
                }
                occupied += crate::starfall::spawn_count(memory.modifier);
                owner_projectiles += projectile_reservation;
                owner_delayed += delayed_reservation;
                let power = hero.view.ability_power * (1.0 + (memory.level - 1) as f32 * 0.25);
                let origin = planar_position(&hero.motion);
                let direction = hero.motion.facing;
                let mut domain = crate::starfall::StarfallDomain::default();
                crate::starfall::spawn(
                    &mut domain,
                    key,
                    run.tick,
                    origin,
                    direction,
                    memory.modifier,
                );
                for flight in domain.flights {
                    spawn_starfall(&mut commands, &mut run, flight, 38.0 * power);
                }
                for repeat in domain.repeats {
                    let id = run.id();
                    commands.spawn((
                        DreamOwned,
                        DelayedCast::new(
                            0.55,
                            CastPayload {
                                spawn: Some((repeat.key, repeat.origin_tick)),
                                action_sequence: input.action_sequences[index + 1],
                                presentation_slot: (((index + 1) as u16) << 12) | 0x0800,
                                owner: hero.view.id,
                                id,
                                kind: MemoryKind::Starfall,
                                essence: None,
                                power: power * 0.65,
                                origin,
                                direction,
                                graphics: source.repeat().id(key.match_epoch, hero.view.id),
                                authority_id: repeat.authority_id,
                            },
                        ),
                    ));
                }
                hero.view.attack_flash = 0.22;
                action_effect(
                    &mut commands,
                    &run,
                    hero.view.id,
                    source,
                    add(origin, scale(direction, 0.9)),
                    0.8,
                    12,
                );
                continue;
            }
            if !hero.loadout.0[index].activate() {
                transactions.simple(
                    hero.view.id,
                    Some(index as u8),
                    run.tick,
                    crate::combat::ActionReason::Cooldown,
                );
                continue;
            }
            transactions.simple(
                hero.view.id,
                Some(index as u8),
                run.tick,
                crate::combat::ActionReason::Accepted,
            );
            owner_wisps += wisp_reservation;
            owner_delayed += delayed_reservation;
            let power = hero.view.ability_power * (1.0 + (memory.level - 1) as f32 * 0.25);
            let origin = planar_position(&hero.motion);
            let direction = hero.motion.facing;
            execute_memory(
                &mut commands,
                &mut run,
                &mut batch,
                &mut hero,
                &mut enemies,
                collision.collision(),
                memory.kind,
                memory.modifier,
                power,
                origin,
                direction,
                source,
            );
            if memory.modifier == Some(EssenceKind::Echo) {
                let id = run.id();
                commands.spawn((
                    DreamOwned,
                    DelayedCast::new(
                        0.55,
                        CastPayload {
                            spawn: None,
                            action_sequence: source.sequence,
                            presentation_slot: source.repeat().slot,
                            owner: hero.view.id,
                            id,
                            kind: memory.kind,
                            essence: None,
                            power: power * 0.65,
                            origin,
                            direction,
                            graphics: source.repeat().id(key.match_epoch, hero.view.id),
                            authority_id: None,
                        },
                    ),
                ));
            }
        }
    }
}

pub(crate) struct OwnerMotion {
    pub dash_started: bool,
    pub dashing_this_tick: bool,
}
/// One shared capsule writer. Combat targeting remains an explicit X/Z policy.
pub(crate) fn advance_owner_motion(
    motion: &mut engine_core::KinematicState,
    combat: &mut engine_core::CombatState,
    speed: f32,
    input: DreamInput,
    collision: &crate::platform::MotionEnvironment,
) -> Result<OwnerMotion, engine_core::KinematicError> {
    advance_owner_motion_inner(motion, combat, speed, input, collision, None)
}
fn advance_owner_motion_inner(
    motion: &mut engine_core::KinematicState,
    combat: &mut engine_core::CombatState,
    speed: f32,
    input: DreamInput,
    collision: &crate::platform::MotionEnvironment,
    authored: Option<[f32; 2]>,
) -> Result<OwnerMotion, engine_core::KinematicError> {
    let mut next = *motion;
    let aim = normalize(input.aim);
    if length(aim) > 0.0 {
        next.facing = aim;
    }
    let facing = next.facing;
    let dashed = input.dash && next.dash_cooldown_ticks == 0 && next.dash_ticks == 0;
    if dashed {
        next.movement_lock_ticks = 0;
    }
    let dashing_this_tick = dashed || next.dash_ticks > 0;
    let config = collision.collision().config_for_speed(speed);
    let kinematic_input = engine_core::KinematicInput {
        movement: input.movement,
        dash: dashed,
        ..Default::default()
    };
    let bases = engine_core::BaseStep {
        frame: collision.bases(),
        config: collision.base_config(),
        detach: dashed || authored.is_some(),
    };
    if let Some(delta) = authored {
        engine_core::advance_kinematic_with_authored_motion(
            &mut next,
            &config,
            kinematic_input,
            collision.scene(),
            bases,
            delta,
        )?;
    } else {
        engine_core::advance_kinematic_with_bases(
            &mut next,
            &config,
            kinematic_input,
            collision.scene(),
            bases,
        )?;
    }
    // Input aim owns attack facing; the generic controller uses movement facing
    // only for accepting the directional dash above.
    next.facing = facing;
    *motion = next;
    if dashed {
        combat.grant_invulnerability(0.26, engine_core::DurationPolicy::Reset);
    }
    Ok(OwnerMotion {
        dash_started: dashed,
        dashing_this_tick,
    })
}
pub(crate) struct BasicAttack {
    pub empowered: bool,
    pub reach: f32,
    pub origin: [f32; 2],
    pub direction: [f32; 2],
    pub damage: f32,
}
pub(crate) fn accept_owner_attack(
    view: &mut HeroBody,
    motion: &mut engine_core::KinematicState,
    action: &mut engine_core::ActionState,
    pressed: bool,
    dashing_this_tick: bool,
) -> Option<BasicAttack> {
    if !pressed || dashing_this_tick || !action.commit(planar_position(motion), 0.0) {
        return None;
    }
    action.take_due();
    action.begin_recovery(0.32);
    view.attack_flash = 0.19;
    // Attack acceptance follows movement; three following ticks remain locked.
    motion.movement_lock_ticks = 3;
    view.combo = (view.combo + 1) % 3;
    let empowered = view.combo == 0;
    Some(BasicAttack {
        empowered,
        reach: if empowered { 3.7 } else { 3.2 },
        origin: planar_position(motion),
        direction: motion.facing,
        damage: 24.0 * view.attack_power * if empowered { 1.65 } else { 1.0 },
    })
}
pub(crate) fn age_owner_visuals(view: &mut HeroBody) {
    decrement(&mut view.hit_flash);
    decrement(&mut view.attack_flash);
}

/// Charge resource, phase and capsule advancement commit as one transaction.
pub(crate) fn advance_owner_charged_motion(
    charge: &mut ChargedMovement,
    key: Option<crate::combat::RayActionKey>,
    motion: &mut engine_core::KinematicState,
    combat: &mut engine_core::CombatState,
    speed: f32,
    input: DreamInput,
    collision: &crate::platform::MotionEnvironment,
) -> Result<(OwnerMotion, crate::combat::ActionReason), engine_core::KinematicError> {
    let mut staged = charge.clone();
    let reason = staged.prepare(
        input.charge,
        key,
        input.aim,
        input.dash || motion.dash_ticks > 0 || combat.status(STUN_STATUS).is_some(),
    );
    let executing = matches!(
        staged.state.phase,
        engine_core::ChargePhase::Executing { .. }
    );
    let delta = if executing {
        Some(
            engine_core::authored_motion_delta(&mut staged.state, CHARGE_CURVE, &CHARGE_SAMPLES)
                .ok_or(engine_core::KinematicError::InvalidState)?,
        )
    } else {
        None
    };
    let mut result = advance_owner_motion_inner(motion, combat, speed, input, collision, delta)?;
    result.dashing_this_tick |= executing;
    *charge = staged;
    Ok((result, reason))
}
