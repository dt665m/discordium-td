use super::*;
use std::f32::consts::TAU;

pub(super) fn spawn_enemy(
    commands: &mut Commands,
    run: &mut Run,
    kind: EnemyKind,
    position: [f32; 2],
) {
    let scale = 1.0 + run.room as f32 * 0.105;
    let hp = match kind {
        EnemyKind::Melee => 92.0,
        EnemyKind::Ranged => 76.0,
        EnemyKind::Ambusher => 65.0,
        EnemyKind::Support => 102.0,
        EnemyKind::Elite => 320.0,
        EnemyKind::Boss => 6000.0,
    } * scale
        * if run.lucid { 1.28 } else { 1.0 }
        * (1.0 + run.party_size.saturating_sub(1) as f32 * 0.65);
    let id = run.id();
    commands.spawn((
        DreamOwned,
        Health::new(hp),
        Enemy {
            view: EnemyBody {
                id,
                position,
                facing: [0.0, 1.0],
                kind,
                windup: 0.0,
                target: position,
                warn_radius: 0.0,
                phase: 1,
                slowed: false,
                hit_flash: 0.0,
            },
            recovery: 0.9 + run.random() * 0.9,
            slow_left: 0.0,
            attack_index: 0,
        },
    ));
}

fn enter_room(world: &mut World) {
    let cleanup: Vec<_> = world
        .query_filtered::<Entity, (With<DreamOwned>, Without<Hero>)>()
        .iter(world)
        .collect();
    for entity in cleanup {
        world.despawn(entity);
    }
    world.resource_scope(|world, mut run: Mut<Run>| {
        run.paused = false;
        run.rewards.clear();
        run.reinforcements = 0;
        let room = run.room;
        let mut hero_query = world.query::<HeroActor>();
        let mut party: Vec<_> = hero_query.iter_mut(world).collect();
        party.sort_by_key(|h| h.view.id);
        for (index, mut hero) in party.into_iter().enumerate() {
            hero.ready = false;
            hero.rewards.clear();
            hero.view.position = if run.party_size <= 8 {
                [
                    (index as f32 - (run.party_size as f32 - 1.0) * 0.5) * 1.8,
                    3.5,
                ]
            } else {
                let angle = index as f32 * 2.399963;
                let radius = ((index as f32 + 1.0).sqrt() * 0.9).min(9.0);
                [angle.cos() * radius, angle.sin() * radius + 3.5]
            };
            hero.view.velocity = [0.0; 2];
            hero.dash_left = 0.0;
            hero.dodge_left = 0.9;
            hero.view.invulnerable = true;
            hero.view.dashing = false;
            hero.view.dash_cooldown = 0.0;
            hero.view.attack_cooldown = 0.0;
            hero.health.hp = (hero.health.hp + hero.health.max_hp * 0.10).min(hero.health.max_hp);
            for memory in &mut hero.view.memories {
                memory.cooldown = 0.0;
            }
        }
        if room == 3 || room == 7 {
            run.phase = RunPhase::Rest;
            run.encounter_name = if room == 3 {
                "The Lantern Refuge"
            } else {
                "Sanctuary of Last Light"
            }
            .into();
            run.message =
                "Restored 40% health. Spend 45 shards to refine a Memory, then choose a blessing."
                    .into();
            for mut hero in world.query::<HeroActor>().iter_mut(world) {
                hero.health.hp =
                    (hero.health.hp + hero.health.max_hp * 0.4).min(hero.health.max_hp);
            }
            run.rewards = make_rewards(&mut run, false);
            for mut hero in world.query::<HeroActor>().iter_mut(world) {
                hero.rewards = run.rewards.clone();
            }
            return;
        }
        run.phase = RunPhase::Combat;
        run.message = if run.lucid {
            "Lucid pact: enemies are stronger. Rewards grant extra shards."
        } else {
            "Unravel every hostile dream to open the way."
        }
        .into();
        let names = [
            [
                "Whispering Canopy",
                "Garden of Lost Names",
                "Moonlit Crossing",
            ],
            [
                "The Glass Orchard",
                "Prismwake Ruins",
                "The Inverted Archive",
            ],
            [
                "Sea of Fallen Stars",
                "The Midnight Causeway",
                "The Unwaking Throne",
            ],
        ];
        let variant = run.index(3);
        run.encounter_name = names[(room / 3).min(2)][variant].into();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, world);
            if room == TOTAL_ROOMS - 1 {
                run.encounter_name = "The Somnarch · Keeper of the Unwaking".into();
                run.message =
                    "Break the keeper's three dreams. Strike between the warning circles.".into();
                spawn_enemy(&mut commands, &mut run, EnemyKind::Boss, [0.0, -8.0]);
            } else {
                let count = 5 + room / 2 + usize::from(run.lucid);
                run.reinforcements = if room >= 4 { 8 } else { 4 } + room as u32 / 2;
                let elite = match room {
                    1 | 2 => room == 1 + (run.seed as usize & 1),
                    4 | 5 => room == 4 + ((run.seed >> 1) as usize & 1),
                    6 | 8 => room == if run.seed & 4 == 0 { 6 } else { 8 },
                    _ => false,
                };
                if elite {
                    run.encounter_name = format!("{} · Oathbreaker", run.encounter_name);
                }
                let offset = run.random() * TAU;
                for i in 0..count {
                    let angle = offset + i as f32 / count as f32 * TAU;
                    let position = [angle.cos() * 11.5, angle.sin() * 11.5];
                    let kind = if elite && i == 0 {
                        EnemyKind::Elite
                    } else if i == count - 1 && room >= 1 {
                        EnemyKind::Support
                    } else {
                        match run.index(if room == 0 { 3 } else { 4 }) {
                            0 => EnemyKind::Melee,
                            1 => EnemyKind::Ranged,
                            2 => EnemyKind::Ambusher,
                            _ => EnemyKind::Melee,
                        }
                    };
                    spawn_enemy(&mut commands, &mut run, kind, position);
                }
            }
        }
        queue.apply(world);
    });
}

/// Readiness is attached to each connected player, so departure removes blockers.
pub(super) fn reconcile_ready(world: &mut World) -> bool {
    let phase = world.resource::<Run>().phase;
    if !matches!(
        phase,
        RunPhase::Intro | RunPhase::Reward | RunPhase::Rest | RunPhase::Transition
    ) {
        return false;
    }
    let mut query = world.query::<HeroActorReadOnly>();
    if query.iter(world).len() == 0 || query.iter(world).any(|hero| !hero.ready) {
        return false;
    }
    if matches!(phase, RunPhase::Reward | RunPhase::Rest) {
        world.resource_mut::<Run>().phase = RunPhase::Transition;
        world.resource_mut::<Run>().rewards.clear();
        for mut hero in world.query::<HeroActor>().iter_mut(world) {
            hero.ready = false;
            hero.rewards.clear();
        }
    } else {
        if phase == RunPhase::Transition {
            world.resource_mut::<Run>().room += 1;
        }
        if world.resource::<Run>().room >= TOTAL_ROOMS {
            world.resource_mut::<Run>().phase = RunPhase::Victory;
        } else {
            enter_room(world);
        }
    }
    true
}

pub(super) fn continue_run(world: &mut World, id: u64) -> bool {
    let phase = world.resource::<Run>().phase;
    if !matches!(
        phase,
        RunPhase::Intro | RunPhase::Rest | RunPhase::Transition
    ) {
        return false;
    }
    {
        let mut query = world.query::<HeroActor>();
        let Some(mut hero) = query.iter_mut(world).find(|h| h.view.id == id) else {
            return false;
        };
        hero.ready = true;
        if phase == RunPhase::Rest {
            hero.rewards.clear();
        }
    }
    reconcile_ready(world);
    true
}

pub(super) fn make_rewards(run: &mut Run, leveling: bool) -> Vec<Reward> {
    let mut out = vec![];
    let memory_offset = run.index(MemoryKind::ALL.len());
    let essence_offset = run.index(EssenceKind::ALL.len());
    let upgrade_offset = run.index(UpgradeKind::ALL.len());
    for index in 0..3 {
        let kind = if leveling {
            RewardKind::Upgrade(
                UpgradeKind::ALL[(upgrade_offset + index * 2) % UpgradeKind::ALL.len()],
            )
        } else {
            match index {
                0 => RewardKind::Memory(MemoryKind::ALL[memory_offset]),
                1 => RewardKind::Essence(EssenceKind::ALL[essence_offset]),
                _ => RewardKind::Upgrade(UpgradeKind::ALL[upgrade_offset]),
            }
        };
        let (title, description, rarity) = match kind {
            RewardKind::Memory(k) => (
                k.name().into(),
                format!(
                    "{} Equip in any slot. Selecting the same Memory upgrades it by one rank (+25% base power).",
                    k.description()
                ),
                Rarity::Rare,
            ),
            RewardKind::Essence(k) => (
                format!("{} Essence", k.name()),
                k.description().into(),
                Rarity::Epic,
            ),
            RewardKind::Upgrade(k) => (
                k.name().into(),
                k.description().into(),
                if leveling {
                    Rarity::Common
                } else {
                    Rarity::Rare
                },
            ),
        };
        out.push(Reward {
            kind,
            title,
            description,
            rarity,
        });
    }
    out
}

pub(super) fn upgrade(hero: &mut HeroActorItem<'_, '_>, kind: UpgradeKind) {
    match kind {
        UpgradeKind::Attack => hero.view.attack_power += 0.30,
        UpgradeKind::Ability => hero.view.ability_power += 0.28,
        UpgradeKind::Movement => {
            hero.view.movement_speed = (hero.view.movement_speed * 1.14).min(13.0);
            hero.view.dash_cooldown = 0.0;
        }
        UpgradeKind::Critical => {
            hero.view.critical_chance = (hero.view.critical_chance + 0.15).min(0.85)
        }
        UpgradeKind::Recovery => {
            hero.view.recovery = (hero.view.recovery * 0.82).max(0.28);
            for slot in &mut hero.view.memories {
                slot.cooldown *= 0.82;
            }
        }
        UpgradeKind::Health => {
            hero.health.max_hp += 50.0;
            hero.health.hp = (hero.health.hp + 75.0).min(hero.health.max_hp);
        }
        UpgradeKind::Defense => {
            hero.view.defense = (hero.view.defense + 0.10).min(0.60);
            hero.view.shield += 35.0;
            hero.shield_left = 12.0;
        }
    }
    recalculate_cooldowns(hero);
}

pub(super) fn recalculate_cooldowns(hero: &mut HeroActorItem<'_, '_>) {
    let recovery = hero.view.recovery;
    for slot in &mut hero.view.memories {
        slot.max_cooldown = slot.kind.cooldown()
            * recovery
            * if slot.essence == Some(EssenceKind::Haste) {
                0.62
            } else {
                1.0
            };
    }
}

pub(super) fn choose_reward(world: &mut World, id: u64, choice: usize, slot: usize) -> bool {
    let accepted = world.resource_scope(|world, mut run: Mut<Run>| {
        if !matches!(run.phase, RunPhase::Reward | RunPhase::Rest) || slot >= 4 {
            return false;
        }
        let mut query = world.query::<HeroActor>();
        let Some(mut hero) = query.iter_mut(world).find(|h| h.view.id == id) else {
            return false;
        };
        if hero.ready {
            return false;
        }
        let Some(reward) = hero.rewards.get(choice).cloned() else {
            return false;
        };
        match reward.kind {
            RewardKind::Memory(kind) => {
                let memory = &mut hero.view.memories[slot];
                if memory.kind == kind {
                    if memory.level >= 8 {
                        return false;
                    }
                    memory.level += 1;
                } else {
                    memory.kind = kind;
                    memory.level = 1;
                }
                memory.cooldown = 0.0;
            }
            RewardKind::Essence(kind) => {
                hero.view.memories[slot].essence = Some(kind);
                hero.view.memories[slot].cooldown = 0.0;
            }
            RewardKind::Upgrade(kind) => upgrade(&mut hero, kind),
        }
        recalculate_cooldowns(&mut hero);
        if hero.pending_levels > 0 && run.phase == RunPhase::Reward {
            hero.pending_levels -= 1;
            hero.rewards = make_rewards(&mut run, true);
        } else {
            hero.rewards.clear();
            hero.ready = true;
        }
        true
    });
    if accepted {
        reconcile_ready(world);
    }
    accepted
}

pub(super) fn buy_memory_upgrade(world: &mut World, id: u64, slot: usize) -> bool {
    if world.resource::<Run>().phase != RunPhase::Rest || slot >= 4 {
        return false;
    }
    let mut query = world.query::<HeroActor>();
    let Some(mut hero) = query.iter_mut(world).find(|h| h.view.id == id) else {
        return false;
    };
    if hero.ready || hero.view.shards < 45 || hero.view.memories[slot].level >= 8 {
        return false;
    }
    hero.view.shards -= 45;
    hero.view.memories[slot].level += 1;
    true
}
