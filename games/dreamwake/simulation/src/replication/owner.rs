use super::*;
use crate::{
    CheckpointIdentity, HeroView, SavedHero, SavedProjectile, SavedWisp,
    state::{DelayedCast, Effect},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

pub const OWNER_CHECKPOINT_SCHEMA: u32 = 10;
pub(crate) const OWNER_TRANSIENT_BYTES: usize = 24 * 1024;
pub(crate) const MAX_OWNER_PROJECTILES: usize = 64;
pub(crate) const MAX_OWNER_WISPS: usize = 16;
pub(crate) const MAX_OWNER_EFFECTS: usize = 32;
pub(crate) const MAX_OWNER_DELAYED: usize = 8;
/// Public enemy replicas a client may hand to prediction as a retained
/// dependency frame. The simulation owns the bound because it is what sizes
/// the replayed `PredictionTarget` set.
pub const MAX_PREDICTION_TARGETS: usize = 64;

/// Complete owner-created latent state. These are ordinary shared-simulation
/// entities, captured generically so adding an ability does not require another
/// prediction implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub(crate) struct OwnerTransients {
    pub projectiles: Vec<SavedProjectile>,
    pub wisps: Vec<SavedWisp>,
    pub effects: Vec<Effect>,
    pub delayed: Vec<DelayedCast>,
}

/// Allocation-bounded binary form. Contact identities and previous sweep state
/// are deliberately absent: an owner replay continues from the committed pose
/// without receiving hidden authoritative contacts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct OwnerProjectileWire {
    payload: crate::state::Projectile,
    id: u64,
    owner: u64,
    faction: u8,
    position: [f32; 2],
    direction: [f32; 2],
    speed: f32,
    remaining: f32,
    radius: f32,
    hits_remaining: u32,
    max_distance: Option<f32>,
    source_policy: engine_core::ProjectileSourcePolicy,
}
impl From<&SavedProjectile> for OwnerProjectileWire {
    fn from(value: &SavedProjectile) -> Self {
        Self {
            payload: value.payload.clone(),
            id: value.state.id,
            owner: value.state.owner,
            faction: value.state.faction,
            position: value.state.position,
            direction: value.state.direction,
            speed: value.state.speed,
            remaining: value.state.remaining,
            radius: value.state.radius,
            hits_remaining: value.state.hits_remaining,
            max_distance: value.state.max_distance,
            source_policy: value.state.source_policy,
        }
    }
}
impl From<OwnerProjectileWire> for SavedProjectile {
    fn from(value: OwnerProjectileWire) -> Self {
        Self {
            payload: value.payload,
            state: engine_core::ProjectileState {
                id: value.id,
                owner: value.owner,
                faction: value.faction,
                position: value.position,
                previous_position: value.position,
                direction: value.direction,
                speed: value.speed,
                remaining: value.remaining,
                radius: value.radius,
                hits_remaining: value.hits_remaining,
                hit_ids: Vec::new(),
                max_distance: value.max_distance,
                source_policy: value.source_policy,
                expired: false,
                pending_impacts: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct OwnerTransientsWire {
    projectiles: engine_net::codec::BoundedVec<OwnerProjectileWire, MAX_OWNER_PROJECTILES>,
    wisps: engine_net::codec::BoundedVec<SavedWisp, MAX_OWNER_WISPS>,
    effects: engine_net::codec::BoundedVec<Effect, MAX_OWNER_EFFECTS>,
    delayed: engine_net::codec::BoundedVec<DelayedCast, MAX_OWNER_DELAYED>,
}
impl engine_net::codec::Validate for OwnerTransientsWire {
    fn validate(&self) -> Result<(), engine_net::codec::CodecError> {
        Ok(())
    }
}
impl TryFrom<&OwnerTransients> for OwnerTransientsWire {
    type Error = engine_net::codec::CodecError;
    fn try_from(value: &OwnerTransients) -> Result<Self, Self::Error> {
        Ok(Self {
            projectiles: engine_net::codec::BoundedVec::new(
                value.projectiles.iter().map(Into::into).collect(),
            )?,
            wisps: engine_net::codec::BoundedVec::new(value.wisps.clone())?,
            effects: engine_net::codec::BoundedVec::new(value.effects.clone())?,
            delayed: engine_net::codec::BoundedVec::new(value.delayed.clone())?,
        })
    }
}
impl From<OwnerTransientsWire> for OwnerTransients {
    fn from(value: OwnerTransientsWire) -> Self {
        Self {
            projectiles: value
                .projectiles
                .into_vec()
                .into_iter()
                .map(Into::into)
                .collect(),
            wisps: value.wisps.into_vec(),
            effects: value.effects.into_vec(),
            delayed: value.delayed.into_vec(),
        }
    }
}

pub(crate) fn encode_owner_transients(
    value: &OwnerTransients,
) -> Result<Vec<u8>, ReplicationError> {
    let wire = OwnerTransientsWire::try_from(value)
        .map_err(|error| ReplicationError::Codec(error.to_string()))?;
    engine_net::codec::encode_payload(&wire, OWNER_TRANSIENT_BYTES)
        .map_err(|error| ReplicationError::Codec(error.to_string()))
}
pub(crate) fn decode_owner_transients(bytes: &[u8]) -> Result<OwnerTransients, ReplicationError> {
    engine_net::codec::decode_payload::<OwnerTransientsWire>(bytes, OWNER_TRANSIENT_BYTES)
        .map(Into::into)
        .map_err(|error| ReplicationError::Codec(error.to_string()))
}
/// Authenticated caller supplies every scope identity. Revision is a freshness
/// floor; transport receipt alone must not advance it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerExpectation {
    pub owner: u64,
    pub match_epoch: u32,
    pub ownership_revision: u32,
    pub scene_revision: u64,
    pub minimum_revision: u64,
}
/// Only the selected owner's complete latent hero state and public context.
/// There is no Run, global allocator, other hero, enemy AI or future encounter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerCheckpoint {
    pub(super) schema: u32,
    pub(super) ruleset: String,
    pub(super) owner: u64,
    pub(super) ownership_revision: u32,
    pub(super) fixed_step_hz: u32,
    pub(super) collision_identity: [u8; 32],
    pub(super) context: DreamGlobalView,
    pub(super) hero: SavedHero,
    pub(crate) transients: OwnerTransients,
    pub(super) required_bases: Vec<engine_core::ColliderKey>,
    pub(super) input_continuity: engine_net::commands::InputContinuity<crate::DreamInput>,
}
impl OwnerCheckpoint {
    pub(super) fn new(
        hero: SavedHero,
        context: DreamGlobalView,
        ownership_revision: u32,
        collision_identity: [u8; 32],
    ) -> Result<Self, ReplicationError> {
        let required_bases = hero
            .motion
            .base
            .attachment
            .map(|a| vec![a.collider])
            .unwrap_or_default();
        let checkpoint = Self {
            schema: OWNER_CHECKPOINT_SCHEMA,
            ruleset: CheckpointIdentity::current(context.stamp.scene_revision).ruleset,
            owner: hero.view.id,
            ownership_revision,
            fixed_step_hz: crate::TICK_HZ,
            collision_identity,
            context,
            hero,
            transients: Default::default(),
            required_bases,
            input_continuity: Default::default(),
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }
    pub fn collision_identity(&self) -> [u8; 32] {
        self.collision_identity
    }
    /// Server adapter supplies input committed at this exact atomic checkpoint.
    /// Frozen publications retain this value even when the live inbox advances.
    pub fn with_input_continuity(
        mut self,
        at: engine_net::types::ServerTick,
        continuity: engine_net::commands::InputContinuity<crate::DreamInput>,
    ) -> Result<Self, ReplicationError> {
        if at.0 != self.stamp().server_tick {
            return Err(ReplicationError::InvalidIdentity);
        }
        self.input_continuity = continuity;
        self.validate()?;
        Ok(self)
    }
    pub fn input_continuity(&self) -> &engine_net::commands::InputContinuity<crate::DreamInput> {
        &self.input_continuity
    }
    pub fn required_bases(&self) -> &[engine_core::ColliderKey] {
        &self.required_bases
    }
    pub fn owner(&self) -> u64 {
        self.owner
    }
    pub fn ownership_revision(&self) -> u32 {
        self.ownership_revision
    }
    pub fn stamp(&self) -> ReplicationStamp {
        self.context.stamp
    }
    pub fn context(&self) -> &DreamGlobalView {
        &self.context
    }
    /// Owner-only display data, never used to build another player's view.
    pub fn hero_view(&self) -> HeroView {
        self.hero.snapshot()
    }
    pub fn rewards(&self) -> &[crate::Reward] {
        &self.hero.rewards
    }
    pub fn ready(&self) -> bool {
        self.hero.ready
    }
    pub fn is_active(&self) -> bool {
        self.hero.active
    }
    pub fn expectation(&self) -> OwnerExpectation {
        OwnerExpectation {
            owner: self.owner,
            match_epoch: self.context.stamp.match_epoch,
            ownership_revision: self.ownership_revision,
            scene_revision: self.context.stamp.scene_revision,
            minimum_revision: self.context.stamp.revision,
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, ReplicationError> {
        super::schema::encode_owner(self)
    }
    pub fn decode(bytes: &[u8], expected: OwnerExpectation) -> Result<Self, ReplicationError> {
        let checkpoint = super::schema::decode_owner(bytes)?;
        checkpoint.validate_for(expected)?;
        Ok(checkpoint)
    }
    pub fn canonical_digest(&self) -> Result<[u8; 32], ReplicationError> {
        Ok(*blake3::hash(&self.encode()?).as_bytes())
    }
    pub fn validate_for(&self, expected: OwnerExpectation) -> Result<(), ReplicationError> {
        self.validate()?;
        if expected.owner == 0
            || expected.ownership_revision == 0
            || expected.match_epoch == 0
            || expected.scene_revision == 0
        {
            return Err(ReplicationError::InvalidIdentity);
        }
        if self.owner != expected.owner || self.ownership_revision != expected.ownership_revision {
            return Err(ReplicationError::WrongOwner);
        }
        if self.context.stamp.match_epoch != expected.match_epoch {
            return Err(ReplicationError::WrongEpoch);
        }
        if self.context.stamp.scene_revision != expected.scene_revision {
            return Err(ReplicationError::WrongScene);
        }
        if self.context.stamp.revision < expected.minimum_revision {
            return Err(ReplicationError::StaleRevision);
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), ReplicationError> {
        self.context.validate()?;
        if self
            .input_continuity
            .last_input
            .as_ref()
            .is_some_and(|input| {
                *input != input.held_only()
                    || [input.movement, input.aim].iter().any(|value| {
                        !value.iter().all(|axis| axis.is_finite())
                            || f64::from(value[0]).hypot(f64::from(value[1])) > 1.000_001
                    })
            })
        {
            return Err(ReplicationError::InvalidState);
        }
        if self
            .transients
            .validate_for(
                self.owner,
                self.hero.combat_identity.generation,
                self.ownership_revision,
                self.context.stamp.match_epoch,
            )
            .is_err()
        {
            return Err(ReplicationError::InvalidState);
        }
        if !self
            .hero
            .charge
            .valid(self.hero.view.id, self.hero.combat_identity.generation)
            || !self.hero.ray.valid_at(
                self.hero.view.id,
                self.hero.combat_identity.generation,
                self.hero.combat_identity.tick,
            )
        {
            return Err(ReplicationError::InvalidState);
        }
        if !self.hero.combat_identity.valid()
            || !self
                .hero
                .defense_episodes
                .valid(self.hero.combat_identity.tick)
        {
            return Err(ReplicationError::InvalidState);
        }
        if self.required_bases.len() > crate::platform::MAX_PLATFORMS
            || self
                .required_bases
                .iter()
                .any(|k| k.index == 0 || k.generation == 0)
            || self
                .hero
                .motion
                .base
                .attachment
                .is_some_and(|a| !self.required_bases.contains(&a.collider))
        {
            return Err(ReplicationError::InvalidState);
        }
        if self.schema != OWNER_CHECKPOINT_SCHEMA
            || self.fixed_step_hz != crate::TICK_HZ
            || self.ruleset
                != CheckpointIdentity::current(self.context.stamp.scene_revision).ruleset
        {
            return Err(ReplicationError::SchemaMismatch);
        }
        if self.owner == 0
            || self.owner >= 1 << 63
            || self.hero.view.id != self.owner
            || self.ownership_revision == 0
        {
            return Err(ReplicationError::WrongOwner);
        }
        let bytes = serde_json::to_vec(self).map_err(codec)?;
        // Covers every nested serialized float and RNG field without copying a
        // partial field whitelist. Optional nonfinite values cannot become None.
        let restored: Self = serde_json::from_slice(&bytes).map_err(codec)?;
        if restored != *self {
            return Err(ReplicationError::InvalidState);
        }
        let hero = &self.hero;
        if hero.motion.scene_revision != self.context.stamp.scene_revision
            || self.collision_identity == [0; 32]
        {
            return Err(ReplicationError::WrongScene);
        }
        if self.ruleset.len() > 64
            || hero
                .rewards
                .iter()
                .any(|v| v.title.len() > 128 || v.description.len() > 512)
            || hero.loadout.0.len() != 4
            || !crate::valid_player_color(hero.view.color)
            || hero.rewards.len() > 8
            || hero.combat.statuses.len() > 64
            || hero.health.max_hp <= 0.0
            || hero.health.hp < 0.0
            || hero.health.hp > hero.health.max_hp
            || hero.progression.level == 0
            || hero.progression.xp < 0.0
            || hero.progression.xp_next <= 0.0
            || hero
                .loadout
                .0
                .iter()
                .any(|v| v.level == 0 || v.cooldown < 0.0 || v.max_cooldown < 0.0)
            || [
                hero.combat.shield,
                hero.combat.shield_remaining,
                hero.combat.invulnerability_remaining,
                hero.action.windup,
                hero.action.recovery,
                hero.view.movement_speed,
                hero.view.attack_power,
                hero.view.ability_power,
                hero.view.critical_chance,
                hero.view.recovery,
            ]
            .into_iter()
            .any(|v| v < 0.0)
            || hero.combat.statuses.iter().any(|v| v.remaining < 0.0)
            || hero
                .combat
                .statuses
                .iter()
                .map(|v| v.id.0)
                .collect::<BTreeSet<_>>()
                .len()
                != hero.combat.statuses.len()
        {
            return Err(ReplicationError::InvalidState);
        }
        // The private random stream key is authenticated owner state. It cannot
        // be recomputed here without disclosing the server's match seed. This
        // validates shape/counter representation; it does not authenticate data.
        Ok(())
    }
}

impl OwnerTransients {
    pub(super) fn bind_match_epoch(&mut self, match_epoch: u32) {
        for projectile in &mut self.projectiles {
            if let Some(graphics) = &mut projectile.payload.graphics {
                graphics.match_epoch = match_epoch;
            }
            if let Some((spawn, _)) = &mut projectile.payload.spawn {
                spawn.action.match_epoch = match_epoch;
            }
        }
        for wisp in &mut self.wisps {
            wisp.payload.graphics.match_epoch = match_epoch;
        }
        for effect in &mut self.effects {
            effect.id.match_epoch = match_epoch;
        }
        for delayed in &mut self.delayed {
            delayed.payload.graphics.match_epoch = match_epoch;
            if let Some((spawn, _)) = &mut delayed.payload.spawn {
                spawn.action.match_epoch = match_epoch;
            }
        }
    }
    pub(super) fn canonicalize(&mut self, authority: bool) {
        for projectile in &mut self.projectiles {
            if authority {
                projectile.payload.authority_id = Some(projectile.state.id);
            }
            projectile.state.pending_impacts.clear();
            projectile.state.hit_ids.clear();
            projectile.state.previous_position = projectile.state.position;
            if let Some(graphics) = projectile.payload.graphics {
                projectile.state.id = stable_transient_id(graphics);
            }
        }
        for wisp in &mut self.wisps {
            wisp.state.pending_shot = None;
            wisp.state.id = stable_transient_id(wisp.payload.graphics);
        }
        for delayed in &mut self.delayed {
            if authority {
                delayed.payload.authority_id = Some(delayed.payload.id);
            }
            delayed.payload.id = stable_transient_id(delayed.payload.graphics);
        }
        self.projectiles.sort_by_key(|value| value.state.id);
        self.wisps.sort_by_key(|value| value.state.id);
        self.effects.sort_by_key(|value| value.id);
        self.delayed.sort_by_key(|value| value.payload.id);
    }
    fn validate_for(
        &self,
        owner: u64,
        generation: u32,
        ownership_revision: u32,
        match_epoch: u32,
    ) -> Result<(), ReplicationError> {
        if self.projectiles.len() > MAX_OWNER_PROJECTILES
            || self.wisps.len() > MAX_OWNER_WISPS
            || self.effects.len() > MAX_OWNER_EFFECTS
            || self.delayed.len() > MAX_OWNER_DELAYED
        {
            return Err(ReplicationError::InvalidState);
        }
        let mut entity_ids = HashSet::new();
        for projectile in &self.projectiles {
            let state = &projectile.state;
            let Some(graphics) = projectile.payload.graphics else {
                return Err(ReplicationError::InvalidState);
            };
            if state.id == 0
                || state.id != stable_transient_id(graphics)
                || state.owner != owner
                || state.faction != 1
                || state.expired
                || state.hits_remaining == 0
                || state.hits_remaining
                    > if projectile.payload.essence == Some(crate::EssenceKind::Vast) {
                        4
                    } else {
                        1
                    }
                || state.hit_ids.len() > 4
                || !entity_ids.insert(state.id)
                || [
                    state.position[0],
                    state.position[1],
                    state.previous_position[0],
                    state.previous_position[1],
                    state.direction[0],
                    state.direction[1],
                    state.speed,
                    state.remaining,
                    state.radius,
                    projectile.payload.damage,
                ]
                .into_iter()
                .any(|v| !v.is_finite())
                || state.speed < 0.0
                || state.remaining <= 0.0
                || state.radius < 0.0
                || projectile.payload.damage < 0.0
                || !state.pending_impacts.is_empty()
            {
                return Err(ReplicationError::InvalidState);
            }
            if let Some((key, _)) = projectile.payload.spawn
                && (!key.action.valid()
                    || key.action.actor != owner
                    || key.action.actor_generation != generation
                    || (key.action.connection_epoch != 0
                        && key.action.ownership_epoch != ownership_revision))
            {
                return Err(ReplicationError::InvalidState);
            }
            if graphics.owner != owner
                || graphics.match_epoch != match_epoch
                || projectile.payload.authority_id == Some(0)
            {
                return Err(ReplicationError::InvalidState);
            }
        }
        for wisp in &self.wisps {
            let state = &wisp.state;
            if state.id == 0
                || state.id != stable_transient_id(wisp.payload.graphics)
                || state.owner != owner
                || state.faction != 1
                || state.expired
                || !entity_ids.insert(state.id)
                || [
                    state.position[0],
                    state.position[1],
                    state.remaining,
                    state.orbit_angle,
                    state.orbit_radius,
                    state.orbit_speed,
                    state.fire_remaining,
                    state.fire_interval,
                    state.range,
                    wisp.payload.damage,
                ]
                .into_iter()
                .any(|v| !v.is_finite())
                || state.remaining <= 0.0
                || state.orbit_radius < 0.0
                || state.fire_interval <= 0.0
                || state.range < 0.0
                || wisp.payload.damage < 0.0
                || state.pending_shot.is_some()
                || wisp.payload.graphics.owner != owner
                || wisp.payload.graphics.match_epoch != match_epoch
            {
                return Err(ReplicationError::InvalidState);
            }
        }
        let mut effect_ids = HashSet::new();
        for effect in &self.effects {
            let kind_valid = match effect.kind {
                engine_core::GraphicsKind::RadialPulse => true,
                engine_core::GraphicsKind::Orb { elevation } => elevation.is_finite(),
                engine_core::GraphicsKind::Beam {
                    direction,
                    elevation,
                    length,
                } => {
                    direction.into_iter().all(f32::is_finite)
                        && elevation.is_finite()
                        && length.is_finite()
                        && length >= 0.0
                }
            };
            if effect.id.owner != owner
                || effect.id.match_epoch != match_epoch
                || !effect_ids.insert(effect.id)
                || !kind_valid
                || effect.pos.into_iter().any(|v| !v.is_finite())
                || !effect.radius.is_finite()
                || effect.radius < 0.0
                || effect.duration_ticks == 0
                || effect.age_ticks >= effect.duration_ticks
            {
                return Err(ReplicationError::InvalidState);
            }
        }
        for delayed in &self.delayed {
            let payload = &delayed.payload;
            if payload.owner != owner
                || payload.id == 0
                || payload.id != stable_transient_id(payload.graphics)
                || !entity_ids.insert(payload.id)
                || !delayed.remaining.is_finite()
                || delayed.remaining < 0.0
                || delayed.ready != (delayed.remaining == 0.0)
                || [
                    payload.power,
                    payload.origin[0],
                    payload.origin[1],
                    payload.direction[0],
                    payload.direction[1],
                ]
                .into_iter()
                .any(|v| !v.is_finite())
                || payload.power < 0.0
                || payload.graphics.owner != owner
                || payload.graphics.match_epoch != match_epoch
                || payload.authority_id == Some(0)
            {
                return Err(ReplicationError::InvalidState);
            }
            if let Some((key, _)) = payload.spawn
                && (!key.action.valid()
                    || key.action.actor != owner
                    || key.action.actor_generation != generation
                    || (key.action.connection_epoch != 0
                        && key.action.ownership_epoch != ownership_revision))
            {
                return Err(ReplicationError::InvalidState);
            }
        }
        Ok(())
    }
}

impl engine_net::codec::Validate for OwnerTransients {
    fn validate(&self) -> Result<(), engine_net::codec::CodecError> {
        OwnerTransientsWire::try_from(self).map(|_| ())
    }
}

/// Selected ordered owner action. This predicts resources and visuals only.
#[derive(Debug, Clone, Copy)]
pub enum OwnerCombatAction {
    Dash {
        key: crate::combat::RayActionKey,
        direction: [f32; 2],
    },
    Charge {
        key: crate::combat::RayActionKey,
        command: engine_core::ChargeCommand,
        aim: [f32; 2],
    },
    Cast {
        key: crate::combat::RayActionKey,
        slot: u8,
        aim: [f32; 2],
    },
    Ray {
        key: crate::combat::RayActionKey,
        aim: [f32; 2],
    },
    BeamBegin {
        key: crate::combat::RayActionKey,
        aim: [f32; 2],
    },
    BeamStop {
        key: crate::combat::RayActionKey,
    },
}
/// Cloneable canonical state retained by rollback history. The Bevy world used
/// to execute it belongs to `OwnerPredictionRunner`, not to every history frame.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerPredictionState {
    pub(super) checkpoint: OwnerCheckpoint,
    predicted_gameplay_tick: u32,
    cast_cues: Vec<crate::combat::RayActionKey>,
    pub(super) starfall_cues: Vec<crate::starfall::SpawnKey>,
    pub(super) starfall_retired: Vec<crate::starfall::SpawnKey>,
    starfall_flights: Vec<crate::starfall::StarfallFlight>,
    starfall_repeats: Vec<crate::starfall::StarfallRepeat>,
}
impl OwnerPredictionState {
    /// Record authored held intent only in speculative state. The next complete
    /// authoritative checkpoint replaces this continuation before replay.
    pub fn record_predicted_input(&mut self, input: crate::DreamInput) {
        self.checkpoint
            .input_continuity
            .commit_input(input.held_only());
    }
    /// Shared inbox substitution policy, without a command identity or edge.
    pub fn substitute_predicted_input(&mut self) -> crate::DreamInput {
        let (input, missing_streak) = self.checkpoint.input_continuity.substitute(
            crate::HELD_INPUT_GRACE_TICKS,
            crate::DreamInput::held_only,
            crate::DreamInput::default,
        );
        self.checkpoint.input_continuity.missing_streak = missing_streak;
        input
    }
    pub fn from_checkpoint(
        checkpoint: &OwnerCheckpoint,
        expected: OwnerExpectation,
    ) -> Result<Self, ReplicationError> {
        checkpoint.validate_for(expected)?;
        if checkpoint.hero.combat_identity.tick > checkpoint.stamp().gameplay_tick {
            return Err(ReplicationError::InvalidState);
        }
        Ok(Self {
            checkpoint: checkpoint.clone(),
            predicted_gameplay_tick: checkpoint.stamp().gameplay_tick,
            cast_cues: Vec::new(),
            starfall_cues: Vec::new(),
            starfall_retired: Vec::new(),
            starfall_flights: starfall_flights(&checkpoint.transients),
            starfall_repeats: starfall_repeats(&checkpoint.transients),
        })
    }
    pub fn try_restore(
        &mut self,
        checkpoint: &OwnerCheckpoint,
        expected: OwnerExpectation,
    ) -> Result<(), ReplicationError> {
        let replacement = Self::from_checkpoint(checkpoint, expected)?;
        if checkpoint.stamp().match_epoch == self.stamp().match_epoch
            && checkpoint.stamp().revision < self.stamp().revision
        {
            return Err(ReplicationError::StaleRevision);
        }
        *self = replacement;
        Ok(())
    }
    /// The stamp remains the authoritative baseline, not a receipt/acknowledgement.
    pub fn gameplay_tick(&self) -> u32 {
        self.predicted_gameplay_tick
    }
    pub fn base_attachment(&self) -> Option<engine_core::BaseAttachment> {
        self.checkpoint.hero.motion.base.attachment
    }
    pub fn required_bases(&self) -> &[engine_core::ColliderKey] {
        self.checkpoint.required_bases()
    }
    pub fn validate_collision(
        &self,
        collision: &crate::collision::CollisionWorld,
    ) -> Result<(), ReplicationError> {
        self.validate_collision_with_bases(collision, &[])
    }
    pub fn validate_collision_with_bases(
        &self,
        collision: &crate::collision::CollisionWorld,
        bases: &[PublicPlatformView],
    ) -> Result<(), ReplicationError> {
        if bases.len() != self.required_bases().len()
            || bases
                .iter()
                .any(|v| !self.required_bases().contains(&v.collider))
        {
            return Err(ReplicationError::InvalidState);
        }
        if collision.identity() != self.checkpoint.collision_identity
            || collision.manifest().scene_revision() != self.stamp().scene_revision
        {
            return Err(ReplicationError::WrongScene);
        }
        crate::platform::MotionEnvironment::committed(
            collision,
            bases,
            self.predicted_gameplay_tick,
        )?
        .validate_motion(
            &self.checkpoint.hero.motion,
            self.checkpoint.hero.view.movement_speed,
        )
    }
    pub fn step_restricted(
        &mut self,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
    ) -> Result<(), ReplicationError> {
        self.step_restricted_with_ray(input, collision, None)
    }
    /// Execute an already-arbitrated owner action through the shared simulation.
    /// Historical targeting and damage remain authoritative.
    pub fn step_restricted_with_ray(
        &mut self,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
        ray: Option<(crate::combat::RayActionKey, [f32; 2])>,
    ) -> Result<(), ReplicationError> {
        self.step_restricted_combat(
            input,
            collision,
            ray.map(|(key, aim)| OwnerCombatAction::Ray { key, aim }),
            None,
        )
    }
    pub fn step_restricted_combat(
        &mut self,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
        action: Option<OwnerCombatAction>,
        beam_aim: Option<(crate::combat::RayActionKey, [f32; 2])>,
    ) -> Result<(), ReplicationError> {
        self.step_restricted_combat_with_bases(input, collision, &[], action, beam_aim)
    }
    pub fn step_restricted_combat_with_bases(
        &mut self,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
        bases: &[PublicPlatformView],
        action: Option<OwnerCombatAction>,
        beam_aim: Option<(crate::combat::RayActionKey, [f32; 2])>,
    ) -> Result<(), ReplicationError> {
        let mut runner = OwnerPredictionRunner::new();
        runner.step(self, input, collision, bases, action, beam_aim)
    }
    /// Full predicted action fence associated with the visible beam primitive.
    pub fn beam_graphics_action(
        &self,
    ) -> Option<(crate::combat::RayActionKey, engine_core::GraphicsId)> {
        let beam = self.checkpoint.hero.ray.beam.as_ref()?;
        Some((beam.key, self.beam_graphics()?.id))
    }
    pub fn beam_graphics(&self) -> Option<engine_core::GraphicsInstance> {
        if !self.checkpoint.hero.active
            || self.checkpoint.hero.health.hp <= 0.0
            || self.checkpoint.context.phase != crate::RunPhase::Combat
        {
            return None;
        }
        self.checkpoint.hero.ray.graphics(
            &self.checkpoint.hero.motion,
            self.checkpoint.hero.combat_identity.tick,
        )
    }
    pub fn starfall_retired(&self) -> &[crate::starfall::SpawnKey] {
        &self.starfall_retired
    }
    pub fn starfall_cues(&self) -> &[crate::starfall::SpawnKey] {
        &self.starfall_cues
    }
    /// Accepted cast edges from the most recent shared step. Presentation can
    /// trigger one-shot feedback without knowing which Memory occupied the slot.
    pub fn cast_cues(&self) -> &[crate::combat::RayActionKey] {
        &self.cast_cues
    }
    pub fn starfall_tick(&self) -> u32 {
        self.checkpoint.hero.combat_identity.tick
    }
    pub fn starfall_repeats(&self) -> &[crate::starfall::StarfallRepeat] {
        &self.starfall_repeats
    }
    pub fn starfall_flights(&self) -> &[crate::starfall::StarfallFlight] {
        &self.starfall_flights
    }
    pub fn combat_generation(&self) -> u32 {
        self.checkpoint.hero.combat_identity.generation
    }
    pub fn owner(&self) -> u64 {
        self.checkpoint.owner()
    }
    pub fn is_active(&self) -> bool {
        self.checkpoint.is_active()
    }
    pub fn stamp(&self) -> ReplicationStamp {
        self.checkpoint.stamp()
    }
    pub fn hero_view(&self) -> HeroView {
        self.checkpoint.hero_view()
    }
    pub fn rewards(&self) -> &[crate::Reward] {
        self.checkpoint.rewards()
    }
    pub fn ready(&self) -> bool {
        self.checkpoint.ready()
    }
    pub fn context(&self) -> &DreamGlobalView {
        self.checkpoint.context()
    }
    pub fn checkpoint(&self) -> &OwnerCheckpoint {
        &self.checkpoint
    }
    pub fn graphics(&self) -> &[engine_core::GraphicsInstance] {
        &self.checkpoint.transients.effects
    }
    pub fn projectile_views(&self) -> Vec<crate::ProjectileView> {
        self.checkpoint
            .transients
            .projectiles
            .iter()
            .map(SavedProjectile::snapshot)
            .collect()
    }
    pub fn projectile_visual_id(&self, key: crate::starfall::SpawnKey) -> Option<u64> {
        self.checkpoint
            .transients
            .projectiles
            .iter()
            .find(|value| value.payload.spawn.is_some_and(|spawn| spawn.0 == key))
            .map(|value| value.state.id)
    }
    pub fn wisp_views(&self) -> Vec<crate::WispView> {
        self.checkpoint
            .transients
            .wisps
            .iter()
            .map(SavedWisp::snapshot)
            .collect()
    }
}

/// Reusable execution world for owner rollback. Callers keep one runner outside
/// cloneable prediction history and serialize access when their model is shared.
pub struct OwnerPredictionRunner {
    simulation: crate::DreamSimulation,
}
impl Default for OwnerPredictionRunner {
    fn default() -> Self {
        Self::new()
    }
}
impl OwnerPredictionRunner {
    pub fn new() -> Self {
        Self {
            simulation: crate::DreamSimulation::new(1, false),
        }
    }
    pub fn step(
        &mut self,
        state: &mut OwnerPredictionState,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
        bases: &[PublicPlatformView],
        action: Option<OwnerCombatAction>,
        beam_aim: Option<(crate::combat::RayActionKey, [f32; 2])>,
    ) -> Result<(), ReplicationError> {
        self.step_with_targets(state, input, collision, bases, action, beam_aim, &[])
    }
    pub fn step_with_targets(
        &mut self,
        state: &mut OwnerPredictionState,
        input: crate::DreamInput,
        collision: &crate::collision::CollisionWorld,
        bases: &[PublicPlatformView],
        action: Option<OwnerCombatAction>,
        beam_aim: Option<(crate::combat::RayActionKey, [f32; 2])>,
        targets: &[PublicEnemyView],
    ) -> Result<(), ReplicationError> {
        if targets.len() > MAX_PREDICTION_TARGETS
            || targets.iter().any(|target| {
                target.id == 0
                    || target.hp < 0.0
                    || target.max_hp <= 0.0
                    || target.hp > target.max_hp
                    || target.position.into_iter().any(|axis| !axis.is_finite())
            })
        {
            return Err(ReplicationError::InvalidState);
        }
        // Reliable edges are admitted by the caller and represented by
        // `action`. Rebuild their input-side trigger here so stale/unselected
        // edge bits cannot execute a second action during rollback.
        let mut simulation_input = input.held_only();
        match action {
            Some(OwnerCombatAction::Dash { key, direction }) => {
                simulation_input.dash = true;
                simulation_input.movement = direction;
                simulation_input.action_sequences[0] =
                    u32::try_from(key.command_sequence).unwrap_or_default();
            }
            Some(OwnerCombatAction::Charge { command, aim, .. }) => {
                simulation_input.charge = command;
                simulation_input.aim = aim;
            }
            Some(OwnerCombatAction::Cast { key, slot, aim, .. }) => {
                if let Some(pressed) = simulation_input.casts.get_mut(usize::from(slot)) {
                    *pressed = true;
                }
                if let Some(sequence) = simulation_input
                    .action_sequences
                    .get_mut(usize::from(slot) + 1)
                {
                    *sequence = u32::try_from(key.command_sequence).unwrap_or_default();
                }
                simulation_input.aim = aim;
            }
            _ => {}
        }
        state.validate_collision_with_bases(collision, bases)?;
        let context = &state.checkpoint.context;
        if bases.is_empty()
            && state.checkpoint.hero.active
            && !context.paused
            && context.phase == crate::RunPhase::Combat
            && crate::platform::missing_base_may_interact(
                &state.checkpoint.hero.motion,
                state.checkpoint.hero.view.movement_speed,
                simulation_input,
                state
                    .checkpoint
                    .hero
                    .charge
                    .maximum_displacement(match action {
                        Some(OwnerCombatAction::Charge { command, .. }) => command,
                        _ => simulation_input.charge,
                    })
                    .max(match action {
                        Some(OwnerCombatAction::Cast { slot, .. }) => state
                            .checkpoint
                            .hero
                            .loadout
                            .0
                            .get(usize::from(slot))
                            .map_or(0.0, |memory| memory.kind.displacement(memory.modifier)),
                        _ => 0.0,
                    }),
                &collision.manifest().config(),
            )
        {
            return Err(ReplicationError::InvalidState);
        }
        let mut next = state.clone();
        self.restore_world(&next, collision, bases, targets)?;
        let next_gameplay_tick = next
            .predicted_gameplay_tick
            .checked_add(1)
            .ok_or(ReplicationError::InvalidState)?;
        let ahead = u64::from(
            next_gameplay_tick
                .checked_sub(next.checkpoint.stamp().gameplay_tick)
                .ok_or(ReplicationError::InvalidState)?,
        );
        let execution_server_tick = next
            .checkpoint
            .stamp()
            .server_tick
            .checked_add(ahead)
            .ok_or(ReplicationError::InvalidState)?;
        let validated = |key, aim| crate::combat::ValidatedRay {
            key,
            execution_server_tick,
            query_server_tick: execution_server_tick,
            query_gameplay_tick: next_gameplay_tick,
            command_fraction: None,
            query_fraction: 0,
            aim,
        };
        let combat_action = action.map(|action| match action {
            OwnerCombatAction::Dash { key, direction } => crate::combat::CombatAction::Dash {
                key,
                execution_server_tick,
                direction,
            },
            OwnerCombatAction::Charge { key, command, aim } => {
                crate::combat::CombatAction::Charge {
                    key,
                    execution_server_tick,
                    command,
                    aim,
                }
            }
            OwnerCombatAction::Cast { key, slot, aim } => crate::combat::CombatAction::Cast {
                key,
                execution_server_tick,
                slot,
                aim,
            },
            OwnerCombatAction::Ray { key, aim } => {
                crate::combat::CombatAction::Ray(validated(key, aim))
            }
            OwnerCombatAction::BeamBegin { key, aim } => {
                crate::combat::CombatAction::BeamBegin(validated(key, aim))
            }
            OwnerCombatAction::BeamStop { key } => crate::combat::CombatAction::BeamStop {
                key,
                execution_server_tick,
            },
        });
        let mut actions = combat_action.into_iter().collect::<Vec<_>>();
        if let Some((key, aim)) = beam_aim {
            actions.push(crate::combat::CombatAction::BeamAim(validated(key, aim)));
        }
        let before = starfall_keys(&next.checkpoint.transients);
        let verdicts = self
            .simulation
            .step_multiplayer_with_actions(&[(next.owner(), simulation_input)], &actions)
            .map_err(|_| ReplicationError::InvalidState)?;
        next.cast_cues = match action {
            Some(OwnerCombatAction::Cast { key, .. })
                if verdicts.iter().any(|verdict| {
                    verdict.key == key && verdict.reason == crate::combat::ActionReason::Accepted
                }) =>
            {
                vec![key]
            }
            _ => Vec::new(),
        };
        self.capture_world(&mut next)?;
        let after = starfall_keys(&next.checkpoint.transients);
        next.starfall_cues = after.difference(&before).copied().collect();
        next.starfall_retired = before.difference(&after).copied().collect();
        next.starfall_flights = starfall_flights(&next.checkpoint.transients);
        next.starfall_repeats = starfall_repeats(&next.checkpoint.transients);
        next.checkpoint
            .validate_for(next.checkpoint.expectation())?;
        *state = next;
        Ok(())
    }

    fn restore_world(
        &mut self,
        state: &OwnerPredictionState,
        collision: &crate::collision::CollisionWorld,
        bases: &[PublicPlatformView],
        targets: &[PublicEnemyView],
    ) -> Result<(), ReplicationError> {
        let world = self.simulation.world_mut();
        let owned = world
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<crate::DreamOwned>>()
            .iter(world)
            .collect::<Vec<_>>();
        for entity in owned {
            world.despawn(entity);
        }
        let context = state.context();
        world.insert_resource(crate::Run {
            tick: state.predicted_gameplay_tick,
            seed: u64::from(context.stamp.match_epoch),
            rng: u64::from(context.stamp.match_epoch).max(1),
            phase: context.phase,
            room: context.room,
            encounter_name: context.encounter_name.clone(),
            rewards: Vec::new(),
            kills: context.kills,
            elapsed: context.elapsed
                + (state.predicted_gameplay_tick - context.stamp.gameplay_tick) as f32 * crate::DT,
            lucid: context.lucid,
            paused: context.paused,
            cleared: context.cleared,
            // This allocator exists only while executing one scoped step. Every
            // retained owner transient is re-keyed from its full action identity
            // before capture, so replay never depends on an opaque Run counter.
            next_id: prediction_id_base(state.owner(), state.checkpoint.ownership_revision),
            message: context.message.clone(),
            party_size: usize::from(context.active_travelers),
            reinforcements: 0,
            encounter_spawns: 0,
        });
        world.insert_resource(crate::plugin::SimulationScope::OwnerPrediction);
        world.insert_resource(collision.clone());
        world.insert_resource(crate::combat_history::CombatHistory::default());
        world.insert_resource(crate::systems::StaticCombatHistory::default());
        world.insert_resource(crate::combat::ActionTransactions::default());
        world.insert_resource(crate::systems::CombatBatch::default());
        world.insert_resource(crate::combat_trace::CombatTraceState::default());
        state.checkpoint.hero.clone().spawn(world);
        for projectile in &state.checkpoint.transients.projectiles {
            world.spawn((
                crate::DreamOwned,
                projectile.payload.clone(),
                projectile.state.clone(),
            ));
        }
        for wisp in &state.checkpoint.transients.wisps {
            world.spawn((crate::DreamOwned, wisp.payload.clone(), wisp.state.clone()));
        }
        for effect in &state.checkpoint.transients.effects {
            world.spawn((crate::DreamOwned, *effect));
        }
        for delayed in &state.checkpoint.transients.delayed {
            world.spawn((crate::DreamOwned, delayed.clone()));
        }
        for target in targets {
            world.spawn((
                crate::DreamOwned,
                crate::systems::PredictionTarget(engine_core::CollisionTarget {
                    id: target.id,
                    faction: 2,
                    position: target.position,
                    radius: crate::systems::enemy_radius(target.kind),
                    active: target.hp > 0.0,
                }),
            ));
        }
        for base in bases {
            world.spawn((
                crate::DreamOwned,
                crate::state::Platform {
                    id: base.id,
                    collider: base.collider,
                    motion_revision: base.motion_revision,
                    origin_gameplay_tick: base.origin_gameplay_tick,
                    gameplay_tick: base.gameplay_tick,
                    phase: base.phase,
                },
            ));
        }
        world.insert_resource(crate::platform::MotionEnvironment::committed(
            collision,
            bases,
            state.predicted_gameplay_tick,
        )?);
        Ok(())
    }

    fn capture_world(&mut self, state: &mut OwnerPredictionState) -> Result<(), ReplicationError> {
        let projectile_authority = state
            .checkpoint
            .transients
            .projectiles
            .iter()
            .filter_map(|value| value.payload.graphics.zip(value.payload.authority_id))
            .collect::<std::collections::HashMap<_, _>>();
        let delayed_authority = state
            .checkpoint
            .transients
            .delayed
            .iter()
            .filter_map(|value| Some((value.payload.graphics, value.payload.authority_id?)))
            .collect::<std::collections::HashMap<_, _>>();
        let captured = self.simulation.capture_current();
        state.checkpoint.hero = captured
            .heroes
            .into_iter()
            .find(|hero| hero.view.id == state.owner())
            .ok_or(ReplicationError::MissingOwner)?;
        state.checkpoint.transients = OwnerTransients {
            projectiles: captured
                .projectiles
                .into_iter()
                .filter(|value| value.state.owner == state.owner() && !value.state.finished())
                .collect(),
            wisps: captured
                .wisps
                .into_iter()
                .filter(|value| value.state.owner == state.owner() && !value.state.expired)
                .collect(),
            effects: captured
                .effects
                .into_iter()
                .filter(|value| value.id.owner == state.owner())
                .collect(),
            delayed: captured
                .delayed
                .into_iter()
                .filter(|value| value.payload.owner == state.owner())
                .collect(),
        };
        for projectile in &mut state.checkpoint.transients.projectiles {
            projectile.payload.authority_id = projectile
                .payload
                .graphics
                .and_then(|id| projectile_authority.get(&id).copied());
        }
        for delayed in &mut state.checkpoint.transients.delayed {
            delayed.payload.authority_id =
                delayed_authority.get(&delayed.payload.graphics).copied();
        }
        state.checkpoint.transients.canonicalize(false);
        let run = self.simulation.world().resource::<crate::Run>();
        state.predicted_gameplay_tick = run.tick;
        Ok(())
    }
}

fn prediction_id_base(owner: u64, ownership_revision: u32) -> u64 {
    stable_transient_id(engine_core::GraphicsId {
        scope: None,
        match_epoch: 1,
        owner,
        action_seq: ownership_revision,
        slot: 0xffff,
    })
}
fn stable_transient_id(id: engine_core::GraphicsId) -> u64 {
    let mut hash = blake3::Hasher::new();
    hash.update(&id.match_epoch.to_le_bytes());
    hash.update(&id.owner.to_le_bytes());
    hash.update(&id.action_seq.to_le_bytes());
    hash.update(&id.slot.to_le_bytes());
    if let Some(scope) = id.scope {
        hash.update(&[1]);
        hash.update(&scope.source_epoch.to_le_bytes());
        hash.update(&scope.stream.to_le_bytes());
        hash.update(&scope.control_epoch.to_le_bytes());
        hash.update(&scope.generation.to_le_bytes());
    } else {
        hash.update(&[0]);
    }
    u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap()) | (1_u64 << 63)
}
fn starfall_keys(transients: &OwnerTransients) -> BTreeSet<crate::starfall::SpawnKey> {
    transients
        .projectiles
        .iter()
        .filter_map(|value| value.payload.spawn.map(|spawn| spawn.0))
        .chain(
            transients
                .delayed
                .iter()
                .filter_map(|value| value.payload.spawn.map(|spawn| spawn.0)),
        )
        .collect()
}
fn starfall_flights(transients: &OwnerTransients) -> Vec<crate::starfall::StarfallFlight> {
    transients
        .projectiles
        .iter()
        .filter_map(|value| {
            let (key, origin_tick) = value.payload.spawn?;
            Some(crate::starfall::StarfallFlight {
                key,
                origin_tick,
                position: value.state.position,
                direction: value.state.direction,
                radius: value.state.radius,
                remaining: value.state.remaining,
                essence: value.payload.essence,
                authority_id: value.payload.authority_id,
            })
        })
        .collect()
}
fn starfall_repeats(transients: &OwnerTransients) -> Vec<crate::starfall::StarfallRepeat> {
    transients
        .delayed
        .iter()
        .filter_map(|value| {
            let (key, origin_tick) = value.payload.spawn?;
            Some(crate::starfall::StarfallRepeat {
                key,
                origin_tick,
                remaining: value.remaining,
                origin: value.payload.origin,
                direction: value.payload.direction,
                authority_id: value.payload.authority_id,
            })
        })
        .collect()
}
