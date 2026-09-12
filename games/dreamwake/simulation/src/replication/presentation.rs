use super::*;
use crate::{
    DamageNumber, EnemyView, HeroView, MemoryKind, MemorySlot, ProjectileView, Reward, RunPhase,
    WispView,
};
use std::collections::BTreeSet;

/// Renderer/UI-only data. This deliberately has no serialized SavedState, seed,
/// AI, allocation counters or simulation restore API (and no Serialize impl).
/// Remote HeroView private fields are placeholders; only the owner has a real
/// loadout, progression, rewards and stats. Gameplay must not read this structure.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamPresentation {
    pub stamp: ReplicationStamp,
    pub collision: Option<std::sync::Arc<crate::collision::CollisionManifest>>,
    pub tick: u32,
    pub phase: RunPhase,
    pub active_travelers: u16,
    pub room: usize,
    pub realm: usize,
    pub encounter_name: String,
    pub hero: HeroView,
    pub heroes: Vec<HeroView>,
    pub ready: bool,
    pub awaiting_party: bool,
    pub enemies: Vec<EnemyView>,
    pub covers: Vec<PublicCoverView>,
    pub cover_markers: Vec<CoverMarkerPresentation>,
    pub platforms: Vec<PublicPlatformView>,
    pub projectiles: Vec<ProjectileView>,
    pub wisps: Vec<WispView>,
    pub presentations: Vec<engine_core::GraphicsInstance>,
    pub damage_numbers: Vec<DamageNumber>,
    pub rewards: Vec<Reward>,
    pub kills: u32,
    pub elapsed: f32,
    pub lucid: bool,
    pub paused: bool,
    pub cleared: u32,
    /// Count of currently disclosed enemies, not hidden/future encounter members.
    pub enemies_remaining: usize,
    pub message: String,
}
impl DreamPresentation {
    pub fn attach_collision(
        &mut self,
        collision: &crate::collision::CollisionWorld,
        owner: &OwnerPredictionState,
    ) -> Result<(), ReplicationError> {
        if collision.manifest().scene_revision() != self.stamp.scene_revision
            || collision.identity() != owner.checkpoint().collision_identity()
        {
            return Err(ReplicationError::WrongScene);
        }
        self.collision = Some(collision.manifest_arc());
        Ok(())
    }
    pub fn from_replicas(
        global: &DreamGlobalView,
        owner: &OwnerPredictionState,
        replicas: &[PublicReplica],
    ) -> Result<Self, ReplicationError> {
        global.validate()?;
        if global.stamp.match_epoch != owner.stamp().match_epoch {
            return Err(ReplicationError::WrongEpoch);
        }
        if global.stamp.scene_revision != owner.stamp().scene_revision {
            return Err(ReplicationError::WrongScene);
        }
        if replicas.len() > 4096 {
            return Err(ReplicationError::BudgetExceeded);
        }
        let mut sources = BTreeSet::from([owner.owner()]);
        for replica in replicas {
            match replica {
                PublicReplica::Hero(v) => {
                    sources.insert(v.id);
                }
                PublicReplica::Enemy(v) => {
                    sources.insert(v.id);
                }
                _ => {}
            }
        }
        let hero = owner.hero_view();
        let mut result = Self {
            stamp: global.stamp,
            collision: None,
            tick: global.stamp.gameplay_tick,
            phase: global.phase,
            active_travelers: global.active_travelers,
            room: global.room,
            realm: global.realm,
            encounter_name: global.encounter_name.clone(),
            hero: hero.clone(),
            heroes: vec![],
            ready: owner.ready(),
            awaiting_party: owner.ready()
                && matches!(
                    global.phase,
                    RunPhase::Intro | RunPhase::Reward | RunPhase::Rest | RunPhase::Transition
                ),
            enemies: vec![],
            covers: vec![],
            cover_markers: vec![],
            platforms: vec![],
            projectiles: vec![],
            wisps: vec![],
            presentations: vec![],
            damage_numbers: vec![],
            rewards: owner.rewards().to_vec(),
            kills: global.kills,
            elapsed: global.elapsed,
            lucid: global.lucid,
            paused: global.paused,
            cleared: global.cleared,
            enemies_remaining: 0,
            message: global.message.clone(),
        };
        let mut ids = BTreeSet::new();
        let mut effect_ids = BTreeSet::new();
        for replica in replicas {
            validate_replica(replica)?;
            let id = match replica {
                PublicReplica::Hero(v) => {
                    if v.id != owner.owner() {
                        result.heroes.push(v.presentation());
                    }
                    Some(v.id)
                }
                PublicReplica::Enemy(v) => {
                    result.enemies.push(EnemyView {
                        id: v.id,
                        position: v.position,
                        facing: v.facing,
                        hp: v.hp,
                        max_hp: v.max_hp,
                        kind: v.kind,
                        windup: v.windup,
                        target: v.target,
                        warn_radius: v.warn_radius,
                        phase: v.phase,
                        slowed: v.slowed,
                        hit_flash: v.hit_flash,
                    });
                    Some(v.id)
                }
                PublicReplica::Projectile(v) => {
                    // Independently delivered scopes may precede their source
                    // or outlive its exit. Retain them in ClientScopes but omit
                    // their visuals until that source is published again.
                    if v.owner.is_none_or(|id| sources.contains(&id)) {
                        result.projectiles.push(ProjectileView {
                            id: v.id,
                            owner: v.owner.unwrap_or(0),
                            position: v.position,
                            direction: v.direction,
                            radius: v.radius,
                            friendly: v.friendly,
                            essence: v.essence,
                        });
                    }
                    Some(v.id)
                }
                PublicReplica::Wisp(v) => {
                    if v.owner.is_none_or(|id| sources.contains(&id)) {
                        result.wisps.push(WispView {
                            id: v.id,
                            owner: v.owner.unwrap_or(0),
                            position: v.position,
                            remaining: v.remaining,
                            essence: v.essence,
                        });
                    }
                    Some(v.id)
                }
                PublicReplica::Effect(v) => {
                    if v.id.match_epoch != global.stamp.match_epoch {
                        return Err(ReplicationError::WrongEpoch);
                    }
                    if !effect_ids.insert(v.id) {
                        return Err(ReplicationError::DuplicateReplica);
                    }
                    if v.id.owner != 0 && !sources.contains(&v.id.owner) {
                        continue;
                    }
                    if v.id.owner == owner.owner()
                        && matches!(v.kind, engine_core::GraphicsKind::Beam { .. })
                    {
                        continue;
                    }
                    result.presentations.push(engine_core::GraphicsInstance {
                        id: v.id,
                        kind: v.kind,
                        pos: v.position,
                        radius: v.radius,
                        age_ticks: v.age_ticks,
                        duration_ticks: v.duration_ticks,
                    });
                    None
                }
                PublicReplica::Platform(v) => {
                    result.platforms.push(*v);
                    Some(v.id)
                }
                PublicReplica::Cover(v) => {
                    if v.region_manifest(global.stamp)?.presence(1, 1)
                        == engine_net::replication::regions::MapObjectPresence::Present
                    {
                        result.covers.push(*v);
                    }
                    Some(v.id)
                }
                // Exact parent scope resolution belongs to the connection's
                // published ClientScopes, before adding resolved marker output.
                PublicReplica::CoverMarker(_) => None,
                PublicReplica::Damage(v) => {
                    result.damage_numbers.push(DamageNumber {
                        id: v.id,
                        position: v.position,
                        amount: v.amount,
                        critical: v.critical,
                        friendly: v.friendly,
                        age: v.age,
                    });
                    Some(v.id)
                }
            };
            if id.is_some_and(|id| id == 0 || !ids.insert(id)) {
                return Err(ReplicationError::DuplicateReplica);
            }
        }
        if let Some(beam) = owner.beam_graphics() {
            result.presentations.push(beam);
        }
        result.heroes.push(hero);
        result.heroes.sort_by_key(|v| v.id);
        result.enemies.sort_by_key(|v| v.id);
        result.covers.sort_by_key(|v| v.id);
        result.projectiles.sort_by_key(|v| v.id);
        result.wisps.sort_by_key(|v| v.id);
        result.damage_numbers.sort_by_key(|v| v.id);
        result.presentations.sort_by_key(|v| v.id);
        result.enemies_remaining = result.enemies.len();
        Ok(result)
    }
    /// Applies only an already-isolated owner predictor's display data. Other
    /// actors remain from authorized public replication; no fake world is built.
    pub fn update_owner(&mut self, owner: &OwnerPredictionState) -> Result<(), ReplicationError> {
        if owner.owner() != self.hero.id {
            return Err(ReplicationError::WrongOwner);
        }
        if owner.stamp().match_epoch != self.stamp.match_epoch {
            return Err(ReplicationError::WrongEpoch);
        }
        if owner.stamp().scene_revision != self.stamp.scene_revision {
            return Err(ReplicationError::WrongScene);
        }
        self.hero = owner.hero_view();
        if let Some(hero) = self.heroes.iter_mut().find(|v| v.id == self.hero.id) {
            *hero = self.hero.clone();
        }
        self.ready = owner.ready();
        self.rewards = owner.rewards().to_vec();
        Ok(())
    }
}
impl PublicHeroView {
    /// Compatibility adapter for existing rendering code. These zeroed private
    /// fields mean undisclosed, not authoritative zeros and not usable gameplay.
    pub fn presentation(&self) -> HeroView {
        HeroView {
            id: self.id,
            position: self.position,
            elevation: self.elevation,
            crouched: self.crouched,
            facing: self.facing,
            velocity: self.velocity,
            hp: self.hp,
            max_hp: self.max_hp,
            shield: self.shield,
            level: self.level,
            xp: 0.0,
            xp_next: 0.0,
            shards: 0,
            memories: std::array::from_fn(|_| MemorySlot {
                kind: MemoryKind::Crescent,
                level: 0,
                essence: None,
                cooldown: 0.0,
                max_cooldown: 0.0,
            }),
            attack_cooldown: 0.0,
            dash_cooldown: 0.0,
            stamina: 0.0,
            max_stamina: 0.0,
            charge_ticks: 0,
            charge_executing: false,
            charge_cooldown_ticks: 0,
            dreamlance_ammo: 0,
            dreamlance_cooldown: 0.0,
            invulnerable: self.invulnerable,
            dashing: self.dashing,
            combo: self.combo,
            hit_flash: self.hit_flash,
            attack_flash: self.attack_flash,
            attack_power: 0.0,
            ability_power: 0.0,
            movement_speed: 0.0,
            critical_chance: 0.0,
            recovery: 0.0,
            defense: 0.0,
        }
    }
}
pub(super) fn validate_replica(replica: &PublicReplica) -> Result<(), ReplicationError> {
    let bytes = serde_json::to_vec(replica).map_err(codec)?;
    let restored: PublicReplica = serde_json::from_slice(&bytes).map_err(codec)?;
    if restored != *replica {
        return Err(ReplicationError::InvalidState);
    }
    let invalid = match replica {
        PublicReplica::Hero(v) => {
            v.id == 0
                || v.id >= 1 << 63
                || v.max_hp <= 0.0
                || v.hp < 0.0
                || v.hp > v.max_hp
                || v.level == 0
        }
        PublicReplica::Enemy(v) => {
            v.id == 0
                || v.max_hp <= 0.0
                || v.hp < 0.0
                || v.hp > v.max_hp
                || v.windup < 0.0
                || v.warn_radius < 0.0
        }
        PublicReplica::Cover(v) => !v.valid(),
        PublicReplica::CoverMarker(v) => !v.valid(),
        PublicReplica::Platform(v) => !v.valid(),
        PublicReplica::Projectile(v) => v.id == 0 || v.owner == Some(0) || v.radius < 0.0,
        PublicReplica::Wisp(v) => v.id == 0 || v.owner == Some(0) || v.remaining < 0.0,
        PublicReplica::Effect(v) => {
            !crate::state::valid_graphics_kind(v.kind)
                || !crate::state::valid_graphics_id(v.id)
                || v.id.match_epoch == 0
                || v.radius < 0.0
                || v.duration_ticks == 0
                || v.age_ticks > v.duration_ticks
                || (v.id.owner == 0 && (v.id.slot != u16::MAX || v.id.action_seq == 0))
        }
        PublicReplica::Damage(v) => v.id == 0 || v.amount < 0.0 || v.age < 0.0,
    };
    if invalid {
        Err(ReplicationError::InvalidState)
    } else {
        Ok(())
    }
}
