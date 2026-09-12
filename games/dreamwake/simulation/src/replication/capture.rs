use super::*;
use crate::{DreamSimulation, RunPhase, SavedHero, snapshot_capture::CapturedState};
use std::collections::{BTreeSet, HashMap};

/// Server-side only keys used to connect committed actors to graph eligibility.
/// Deliberately not serializable: effect keys may contain a hidden source ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplicationKey {
    Hero(u64),
    Enemy(u64),
    Cover(u64),
    CoverMarker(u64),
    Platform(u64),
    Projectile(u64),
    Wisp(u64),
    Effect(engine_core::GraphicsId),
    Damage(u64),
}
/// References independently permitted by the current connection's graph policy.
/// The constructor is a trusted-server adapter boundary, not authentication.
#[derive(Default)]
pub struct ApprovedSources(BTreeSet<u64>);
impl ApprovedSources {
    pub fn new(ids: impl IntoIterator<Item = u64>) -> Result<Self, ReplicationError> {
        let mut sources = BTreeSet::new();
        for (i, id) in ids.into_iter().enumerate() {
            if i >= 4096 {
                return Err(ReplicationError::BudgetExceeded);
            }
            if id == 0 {
                return Err(ReplicationError::InvalidIdentity);
            }
            sources.insert(id);
        }
        Ok(Self(sources))
    }
    pub fn allows(&self, id: u64) -> bool {
        self.0.contains(&id)
    }
}
/// One current committed authority capture supports all peers. Its components are
/// private and this type has no Serialize implementation or raw snapshot getter.
/// Only positive projection methods can produce an encodable peer payload.
pub struct CommittedReplication {
    global: DreamGlobalView,
    state: CapturedState,
    index: HashMap<ReplicationKey, usize>,
}
impl DreamSimulation {
    pub fn capture_replication(
        &self,
        stamp: ReplicationStamp,
    ) -> Result<CommittedReplication, ReplicationError> {
        stamp.validate()?;
        let state = self.capture_current();
        if stamp.scene_revision != state.collision.scene_revision() {
            return Err(ReplicationError::WrongScene);
        }
        if stamp.gameplay_tick != state.run.tick {
            return Err(ReplicationError::InvalidIdentity);
        }
        crate::checkpoint::validate_capture(&state)
            .map_err(|e| ReplicationError::Codec(e.to_string()))?;
        let run = &state.run;
        let global = DreamGlobalView {
            stamp,
            phase: run.phase,
            active_travelers: u16::try_from(run.party_size)
                .map_err(|_| ReplicationError::BudgetExceeded)?,
            paused: run.paused,
            room: run.room,
            realm: (run.room / 3).min(2),
            encounter_name: run.encounter_name.clone(),
            kills: run.kills,
            elapsed: run.elapsed,
            lucid: run.lucid,
            cleared: run.cleared,
            message: public_summary(run.phase).into(),
        };
        global.validate()?;
        let mut index = HashMap::new();
        for (i, v) in state.heroes.iter().enumerate() {
            index.insert(ReplicationKey::Hero(v.view.id), i);
        }
        for (i, v) in state.enemies.iter().enumerate() {
            index.insert(ReplicationKey::Enemy(v.view.id), i);
        }
        for (i, v) in state.platforms.iter().enumerate() {
            index.insert(ReplicationKey::Platform(v.id), i);
        }
        for (i, v) in state.covers.iter().enumerate() {
            index.insert(ReplicationKey::Cover(v.id), i);
            index.insert(ReplicationKey::CoverMarker(v.marker_id), i);
        }
        for (i, v) in state.projectiles.iter().enumerate() {
            index.insert(ReplicationKey::Projectile(v.state.id), i);
        }
        for (i, v) in state.wisps.iter().enumerate() {
            index.insert(ReplicationKey::Wisp(v.state.id), i);
        }
        for (i, v) in state.effects.iter().enumerate() {
            index.insert(ReplicationKey::Effect(v.id), i);
        }
        for (i, v) in state.numbers.iter().enumerate() {
            index.insert(ReplicationKey::Damage(v.0.id), i);
        }
        Ok(CommittedReplication {
            global,
            state,
            index,
        })
    }
}
fn public_summary(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Intro => "Vesper · Moonbound",
        RunPhase::Combat => "Unravel the hostile dreams to open the way.",
        RunPhase::Reward => "Choose one gift to carry onward.",
        RunPhase::Rest => "Rest, refine a Memory, then choose a blessing.",
        RunPhase::Transition => "The next dream awaits.",
        RunPhase::Victory => "You carry the dawn home.",
        RunPhase::Defeat => "Begin another dream together.",
    }
}
impl CommittedReplication {
    pub fn collision(&self) -> crate::collision::CollisionManifest {
        self.state.collision.clone()
    }
    pub fn global(&self) -> &DreamGlobalView {
        &self.global
    }
    pub fn keys(&self) -> Vec<ReplicationKey> {
        self.state
            .heroes
            .iter()
            .filter(|v| v.active)
            .map(|v| ReplicationKey::Hero(v.view.id))
            .chain(
                self.state
                    .enemies
                    .iter()
                    .map(|v| ReplicationKey::Enemy(v.view.id)),
            )
            .chain(
                self.state
                    .covers
                    .iter()
                    .map(|v| ReplicationKey::Cover(v.id)),
            )
            .chain(
                self.state
                    .covers
                    .iter()
                    .filter(|v| v.present)
                    .map(|v| ReplicationKey::CoverMarker(v.marker_id)),
            )
            .chain(
                self.state
                    .projectiles
                    .iter()
                    .map(|v| ReplicationKey::Projectile(v.state.id)),
            )
            .chain(
                self.state
                    .wisps
                    .iter()
                    .map(|v| ReplicationKey::Wisp(v.state.id)),
            )
            .chain(
                self.state
                    .effects
                    .iter()
                    .map(|v| ReplicationKey::Effect(v.id)),
            )
            .chain(
                self.state
                    .numbers
                    .iter()
                    .map(|v| ReplicationKey::Damage(v.0.id)),
            )
            .chain(
                self.state
                    .platforms
                    .iter()
                    .map(|v| ReplicationKey::Platform(v.id)),
            )
            .collect()
    }
    /// Server-only graph bounds input. Calling this does not grant disclosure.
    pub fn position(&self, key: ReplicationKey) -> Option<[f32; 2]> {
        let i = *self.index.get(&key)?;
        Some(match key {
            ReplicationKey::Hero(_) => {
                crate::collision::planar_position(&self.state.heroes[i].motion)
            }
            ReplicationKey::Enemy(_) => self.state.enemies[i].motor.position,
            ReplicationKey::Platform(_) => {
                let p = self.state.platforms[i]
                    .presentation(self.global.stamp.scene_revision)
                    .pose
                    .position;
                [p[0], p[2]]
            }
            ReplicationKey::Cover(_) | ReplicationKey::CoverMarker(_) => {
                self.state.covers[i].ground_position
            }
            ReplicationKey::Projectile(_) => self.state.projectiles[i].state.position,
            ReplicationKey::Wisp(_) => self.state.wisps[i].state.position,
            ReplicationKey::Effect(_) => self.state.effects[i].pos,
            ReplicationKey::Damage(_) => self.state.numbers[i].0.position,
        })
    }
    /// Conservative current world-space extent for graph registration. Warning
    /// circles are included only while committed, with their offset from actor.
    pub fn bounds(&self, key: ReplicationKey) -> Option<([f32; 2], f32)> {
        let i = *self.index.get(&key)?;
        let position = self.position(key)?;
        let radius = match key {
            ReplicationKey::Hero(_) => 0.8,
            ReplicationKey::Cover(_) => 1.51,
            ReplicationKey::CoverMarker(_) => 0.3,
            ReplicationKey::Platform(_) => 3.36,
            ReplicationKey::Enemy(_) => {
                let e = &self.state.enemies[i];
                let body: f32 = match e.view.kind {
                    crate::EnemyKind::Boss => 1.5,
                    crate::EnemyKind::Elite => 0.9,
                    _ => 0.6,
                };
                if e.action.windup > 0.0 {
                    body.max(
                        bevy::prelude::Vec2::from_array(e.action.target)
                            .distance(bevy::prelude::Vec2::from_array(position))
                            + e.view.warn_radius,
                    )
                } else {
                    body
                }
            }
            ReplicationKey::Projectile(_) => self.state.projectiles[i].state.radius,
            ReplicationKey::Wisp(_) => 0.4,
            ReplicationKey::Effect(_) => {
                let effect = &self.state.effects[i];
                match effect.kind {
                    engine_core::GraphicsKind::Beam { length, .. } => length + effect.radius,
                    _ => effect.radius,
                }
            }
            ReplicationKey::Damage(_) => 0.0,
        };
        Some((position, radius))
    }
    /// Trusted server metadata only; never an independently approved wire field.
    pub fn source_id(&self, key: ReplicationKey) -> Option<u64> {
        let i = *self.index.get(&key)?;
        match key {
            ReplicationKey::Hero(id) => Some(id),
            ReplicationKey::Projectile(_) => Some(self.state.projectiles[i].state.owner),
            ReplicationKey::Wisp(_) => Some(self.state.wisps[i].state.owner),
            ReplicationKey::Effect(_) => Some(self.state.effects[i].id.owner),
            _ => None,
        }
    }
    fn hero(&self, id: u64) -> Option<&SavedHero> {
        self.index
            .get(&ReplicationKey::Hero(id))
            .map(|i| &self.state.heroes[*i])
    }
    pub fn owner_checkpoint(
        &self,
        owner: u64,
        ownership_revision: u32,
    ) -> Result<OwnerCheckpoint, ReplicationError> {
        let hero = self.hero(owner).ok_or(ReplicationError::MissingOwner)?;
        let mut checkpoint = OwnerCheckpoint::new(
            hero.clone(),
            self.global.clone(),
            ownership_revision,
            self.state.collision.identity(),
        )?;
        checkpoint.required_bases = self.required_bases(owner);
        checkpoint.starfall.flights = self
            .state
            .projectiles
            .iter()
            .filter_map(|p| {
                let (key, origin_tick) = p.payload.spawn?;
                (p.state.owner == owner
                    && key.action.actor_generation == hero.combat_identity.generation
                    && (key.action.connection_epoch == 0
                        || key.action.ownership_epoch == ownership_revision)
                    && !p.state.finished())
                .then_some(crate::starfall::StarfallFlight {
                    key,
                    origin_tick,
                    position: p.state.position,
                    direction: p.state.direction,
                    radius: p.state.radius,
                    remaining: p.state.remaining,
                    essence: p.payload.essence,
                    authority_id: Some(p.state.id),
                })
            })
            .collect();
        checkpoint.starfall.repeats = self
            .state
            .delayed
            .iter()
            .filter_map(|d| {
                let (key, origin_tick) = d.payload.spawn?;
                (d.payload.owner == owner
                    && key.action.actor_generation == hero.combat_identity.generation
                    && (key.action.connection_epoch == 0
                        || key.action.ownership_epoch == ownership_revision))
                    .then_some(crate::starfall::StarfallRepeat {
                        authority_id: Some(d.payload.id),
                        key,
                        origin_tick,
                        remaining: d.remaining,
                        origin: d.payload.origin,
                        direction: d.payload.direction,
                    })
            })
            .collect();
        checkpoint.validate_for(checkpoint.expectation())?;
        Ok(checkpoint)
    }
    /// Trusted graph lifecycle metadata; reserved entities are never public actors.
    pub fn reserved_projectiles(&self) -> Vec<(u64, u64)> {
        self.state
            .delayed
            .iter()
            .filter(|d| d.payload.spawn.is_some())
            .map(|d| (d.payload.owner, d.payload.id))
            .collect()
    }
    pub fn required_bases(&self, owner: u64) -> Vec<engine_core::ColliderKey> {
        let Some(hero) = self.hero(owner) else {
            return Vec::new();
        };
        crate::platform::required(
            &hero.motion,
            hero.view.movement_speed,
            &self.state.collision.config(),
            hero.active && self.global.phase == RunPhase::Combat,
            self.state
                .platforms
                .iter()
                .map(|p| p.presentation(self.global.stamp.scene_revision)),
        )
    }
    pub fn project_platform(&self, id: u64) -> Option<PublicPlatformView> {
        Some(
            self.state.platforms[*self.index.get(&ReplicationKey::Platform(id))?]
                .presentation(self.global.stamp.scene_revision),
        )
    }
    pub fn project_hero(&self, id: u64) -> Option<PublicHeroView> {
        self.hero(id).map(public_hero)
    }
    pub fn project_cover(&self, id: u64) -> Option<PublicCoverView> {
        let cover = &self.state.covers[*self.index.get(&ReplicationKey::Cover(id))?];
        Some(cover.actor().presentation(&cover.identity))
    }
    pub fn marker_parent(&self, id: u64) -> Option<u64> {
        self.marker_state(id).map(|state| state.0)
    }
    /// Semantic marker state, independent of a recipient's parent incarnation.
    pub fn marker_state(&self, id: u64) -> Option<(u64, u64, bool)> {
        let cover = &self.state.covers[*self.index.get(&ReplicationKey::CoverMarker(id))?];
        cover
            .present
            .then_some((cover.id, cover.identity.segment, cover.open))
    }
    pub fn project_cover_marker(
        &self,
        id: u64,
        parent: engine_net::types::ScopeIdentity,
    ) -> Option<PublicCoverMarkerView> {
        let cover = &self.state.covers[*self.index.get(&ReplicationKey::CoverMarker(id))?];
        if !cover.present {
            return None;
        }
        let marker = PublicCoverMarkerView {
            id,
            parent,
            revision: cover.identity.segment,
            local_offset: [0.0, 1.35, 0.0],
            open: cover.open,
        };
        marker.valid().then_some(marker)
    }
    pub fn project_enemy(&self, id: u64) -> Option<PublicEnemyView> {
        let enemy = &self.state.enemies[*self.index.get(&ReplicationKey::Enemy(id))?];
        let view = enemy.snapshot();
        Some(PublicEnemyView {
            id: view.id,
            position: view.position,
            facing: view.facing,
            hp: view.hp,
            max_hp: view.max_hp,
            kind: view.kind,
            windup: view.windup,
            target: if view.windup > 0.0 {
                view.target
            } else {
                view.position
            },
            warn_radius: if view.windup > 0.0 {
                view.warn_radius
            } else {
                0.0
            },
            phase: view.phase,
            slowed: view.slowed,
            hit_flash: view.hit_flash,
        })
    }
    pub fn project_projectile(
        &self,
        id: u64,
        sources: &ApprovedSources,
    ) -> Option<PublicProjectileView> {
        let projectile =
            &self.state.projectiles[*self.index.get(&ReplicationKey::Projectile(id))?];
        Some(PublicProjectileView {
            id,
            owner: sources
                .allows(projectile.state.owner)
                .then_some(projectile.state.owner),
            position: projectile.state.position,
            direction: projectile.state.direction,
            radius: projectile.state.radius,
            friendly: projectile.state.faction == 1,
            essence: projectile.payload.essence,
        })
    }
    pub fn project_wisp(&self, id: u64, sources: &ApprovedSources) -> Option<PublicWispView> {
        let wisp = &self.state.wisps[*self.index.get(&ReplicationKey::Wisp(id))?];
        Some(PublicWispView {
            id,
            owner: sources.allows(wisp.state.owner).then_some(wisp.state.owner),
            position: wisp.state.position,
            remaining: wisp.state.remaining,
            essence: wisp.payload.essence,
        })
    }
    /// Hidden sources are omitted, not merely removed from a nested owner field:
    /// the original effect identity itself contains its source and action sequence.
    /// For a separately permitted coarse audience, use CoarseEffectProxy instead.
    pub fn project_effect(
        &self,
        id: engine_core::GraphicsId,
        sources: &ApprovedSources,
    ) -> Option<PublicEffectView> {
        if !sources.allows(id.owner) {
            return None;
        }
        let effect = &self.state.effects[*self.index.get(&ReplicationKey::Effect(id))?];
        Some(PublicEffectView {
            id: engine_core::GraphicsId {
                match_epoch: self.global.stamp.match_epoch,
                ..effect.id
            },
            kind: effect.kind,
            position: effect.pos,
            radius: effect.radius,
            age_ticks: effect.age_ticks,
            duration_ticks: effect.duration_ticks,
        })
    }
    /// The entity graph approves the event's spatial audience separately from
    /// source disclosure. The game then permits its class and coarse origin.
    /// The proxy identity must be an opaque lifecycle identity supplied by the
    /// server adapter; it must not encode the hidden source or command sequence.
    pub fn project_event(
        &self,
        id: engine_core::GraphicsId,
        sources: &ApprovedSources,
        connection: engine_net::types::ConnectionId,
        policy: engine_net::types::PolicyRevision,
        proxy_id: u32,
    ) -> Option<PublicEffectView> {
        use engine_net::replication::audience::EventAudience;
        let effect = &self.state.effects[*self.index.get(&ReplicationKey::Effect(id))?];
        let rules = super::audience::DreamEventAudience { sources, policy };
        let audience = EventAudience::authorize(connection, effect, &rules)?;
        audience
            .project(
                connection,
                policy,
                |_| self.project_effect(id, sources),
                |effect| {
                    CoarseEffectProxy {
                        proxy_id,
                        cell: effect.pos.map(|v| (v / 8.0).floor() as i32),
                        cell_size: 8,
                        age_ticks: effect.age_ticks,
                        duration_ticks: effect.duration_ticks,
                    }
                    .project(self.global.stamp)
                    .ok()
                },
            )
            .ok()
    }
    /// The graph must approve the damage event audience/position independently;
    /// a nearby actor is not evidence that a hidden damage event is permitted.
    pub fn project_damage(&self, id: u64) -> Option<PublicDamageView> {
        let number = &self.state.numbers[*self.index.get(&ReplicationKey::Damage(id))?].0;
        Some(PublicDamageView {
            id,
            position: number.position,
            amount: number.amount,
            critical: number.critical,
            friendly: number.friendly,
            age: number.age,
        })
    }
    pub fn project(&self, key: ReplicationKey, sources: &ApprovedSources) -> Option<PublicReplica> {
        match key {
            ReplicationKey::Hero(id) => self.project_hero(id).map(PublicReplica::Hero),
            ReplicationKey::Platform(id) => self.project_platform(id).map(PublicReplica::Platform),
            ReplicationKey::Enemy(id) => self.project_enemy(id).map(PublicReplica::Enemy),
            ReplicationKey::Projectile(id) => self
                .project_projectile(id, sources)
                .map(PublicReplica::Projectile),
            ReplicationKey::Wisp(id) => self.project_wisp(id, sources).map(PublicReplica::Wisp),
            ReplicationKey::Effect(id) => {
                self.project_effect(id, sources).map(PublicReplica::Effect)
            }
            ReplicationKey::Damage(id) => self.project_damage(id).map(PublicReplica::Damage),
            ReplicationKey::Cover(id) => self.project_cover(id).map(PublicReplica::Cover),
            // Parent scopes are connection-local and supplied only by the trusted
            // adapter after both representations are independently eligible.
            ReplicationKey::CoverMarker(_) => None,
        }
    }
}
pub(super) fn public_hero(hero: &SavedHero) -> PublicHeroView {
    PublicHeroView {
        id: hero.view.id,
        position: crate::collision::planar_position(&hero.motion),
        elevation: hero.motion.position[1],
        crouched: hero.motion.stance == engine_core::Stance::Crouched,
        facing: hero.motion.facing,
        velocity: crate::collision::planar_velocity(&hero.motion),
        hp: hero.health.hp,
        max_hp: hero.health.max_hp,
        shield: hero.combat.shield,
        level: hero.progression.level,
        invulnerable: hero.combat.is_invulnerable() || hero.motion.dash_ticks > 0,
        dashing: hero.motion.dash_ticks > 0 && hero.health.hp > 0.0,
        combo: hero.view.combo,
        hit_flash: hero.view.hit_flash,
        attack_flash: hero.view.attack_flash,
    }
}
