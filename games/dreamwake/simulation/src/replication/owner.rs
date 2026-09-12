use super::*;
use crate::{CheckpointIdentity, HeroView, SavedHero};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const OWNER_CHECKPOINT_SCHEMA: u32 = 8;
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
    pub(crate) starfall: crate::starfall::StarfallDomain,
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
            starfall: Default::default(),
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
        if !self
            .starfall
            .valid(self.owner, self.hero.combat_identity.generation)
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
/// Selected ordered owner action. This predicts resources and visuals only.
#[derive(Debug, Clone, Copy)]
pub enum OwnerCombatAction {
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
/// Isolated owner checkpoint storage for the restricted predictor. Public render
/// replicas and server AI cannot enter this state. Restricted stepping shares
/// authority transitions; casts and impact-dependent effects await authority.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerPredictionState {
    pub(super) checkpoint: OwnerCheckpoint,
    predicted_gameplay_tick: u32,
    pub(super) starfall_cues: Vec<crate::starfall::SpawnKey>,
    pub(super) starfall_retired: Vec<crate::starfall::SpawnKey>,
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
            starfall_cues: Vec::new(),
            starfall_retired: Vec::new(),
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
    /// Predict one fixed command. Casts remain authoritative because delayed
    /// abilities and impacts require state outside this restricted checkpoint.
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
    /// Predict only the selected owner ray cost. Historical targeting and damage
    /// remain authoritative; callers arbitrate ordered edges before this step.
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
        self.validate_collision_with_bases(collision, bases)?;
        let context = &self.checkpoint.context;
        if bases.is_empty()
            && self.checkpoint.hero.active
            && !context.paused
            && context.phase == crate::RunPhase::Combat
            && crate::platform::missing_base_may_interact(
                &self.checkpoint.hero.motion,
                self.checkpoint.hero.view.movement_speed,
                input,
                self.checkpoint
                    .hero
                    .charge
                    .maximum_displacement(match action {
                        Some(OwnerCombatAction::Charge { command, .. }) => command,
                        _ => input.charge,
                    }),
                &collision.manifest().config(),
            )
        {
            return Err(ReplicationError::InvalidState);
        }
        let environment = crate::platform::MotionEnvironment::from_dependencies(
            collision,
            bases,
            self.predicted_gameplay_tick,
        )?;
        let mut next = self.clone();
        next.step_inner(input, &environment, action, beam_aim)?;
        *self = next;
        Ok(())
    }
    fn step_inner(
        &mut self,
        input: crate::DreamInput,
        collision: &crate::platform::MotionEnvironment,
        action: Option<OwnerCombatAction>,
        beam_aim: Option<(crate::combat::RayActionKey, [f32; 2])>,
    ) -> Result<(), ReplicationError> {
        if input
            .movement
            .into_iter()
            .chain(input.aim)
            .any(|v| !v.is_finite())
        {
            return Err(ReplicationError::InvalidState);
        }
        self.starfall_cues.clear();
        self.starfall_retired.clear();
        let context = &self.checkpoint.context;
        if !self.checkpoint.hero.active || context.paused || context.phase == crate::RunPhase::Intro
        {
            return Ok(());
        }
        let hero = &mut self.checkpoint.hero;
        if let Some(OwnerCombatAction::Charge { key, aim, .. }) = action {
            if !key.valid()
                || key.actor != hero.view.id
                || key.actor_generation != hero.combat_identity.generation
                || aim.iter().any(|v| !v.is_finite() || v.abs() > 1.0)
            {
                return Err(ReplicationError::InvalidState);
            }
        }
        if context.phase == crate::RunPhase::Combat {
            hero.combat.tick(crate::DT);
            hero.ray.tick();
            hero.action.tick(crate::DT);
            hero.loadout.tick(crate::DT);
        }
        crate::systems::age_owner_visuals(&mut hero.actor.view);
        if context.phase != crate::RunPhase::Combat {
            return Ok(());
        }
        self.predicted_gameplay_tick = self
            .predicted_gameplay_tick
            .checked_add(1)
            .ok_or(ReplicationError::InvalidState)?;
        if hero.health.hp <= 0.0 {
            hero.ray.beam = None;
            hero.charge.state.interrupt();
            let tick = hero
                .combat_identity
                .tick
                .checked_add(1)
                .ok_or(ReplicationError::InvalidState)?;
            crate::systems::advance_combat_owner(
                &mut hero.combat_identity,
                &mut hero.defense_episodes,
                &hero.combat,
                tick,
            )
            .map_err(|_| ReplicationError::InvalidState)?;
            if hero.motion.base.attachment.is_some() {
                let speed = hero.view.movement_speed;
                crate::systems::advance_owner_motion(
                    &mut hero.motion,
                    &mut hero.combat,
                    speed,
                    crate::DreamInput::default(),
                    collision,
                )
                .map_err(|_| ReplicationError::InvalidState)?;
            }
            if hero.motion.base.attachment.is_none() {
                hero.motion.velocity = [0.0; 3];
            }
            return Ok(());
        }
        let (charge_key, input) =
            if let Some(OwnerCombatAction::Charge { key, command, aim }) = action {
                (
                    Some(key),
                    crate::DreamInput {
                        charge: command,
                        aim,
                        ..input
                    },
                )
            } else {
                (None, input)
            };
        let (motion, _) = crate::systems::advance_owner_charged_motion(
            &mut hero.charge,
            charge_key,
            &mut hero.motion,
            &mut hero.combat,
            hero.actor.view.movement_speed,
            input,
            collision,
        )
        .map_err(|_| ReplicationError::InvalidState)?;
        crate::systems::accept_owner_attack(
            &mut hero.actor.view,
            &mut hero.motion,
            &mut hero.action,
            input.attack,
            motion.dashing_this_tick,
        );
        let tick = hero
            .combat_identity
            .tick
            .checked_add(1)
            .ok_or(ReplicationError::InvalidState)?;
        if let Some(OwnerCombatAction::Cast { key, slot, aim }) = action
            && key.valid()
            && u32::try_from(key.command_sequence).is_ok()
            && key.actor == hero.view.id
            && key.actor_generation == hero.combat_identity.generation
            && aim.into_iter().all(f32::is_finite)
            && usize::from(slot) < hero.loadout.0.len()
        {
            let domain = &mut self.checkpoint.starfall;
            if !domain.flights.iter().any(|f| f.key.action == key)
                && !domain.repeats.iter().any(|r| r.key.action == key)
                && crate::starfall::accept(
                    &mut hero.loadout.0[usize::from(slot)],
                    domain.flights.len() + domain.repeats.len(),
                )
                .is_ok()
            {
                self.starfall_cues.push(crate::starfall::SpawnKey {
                    action: key,
                    ordinal: 0,
                });
                let memory = hero.loadout.0[usize::from(slot)];
                crate::starfall::spawn(
                    domain,
                    key,
                    tick,
                    crate::collision::planar_position(&hero.motion),
                    hero.motion.facing,
                    memory.modifier,
                );
                hero.view.attack_flash = 0.22;
            }
        }
        self.starfall_retired = self
            .checkpoint
            .starfall
            .advance(collision.scene())
            .map_err(|_| ReplicationError::InvalidState)?;
        let had_beam = hero.ray.beam.is_some();
        let saved = |key, aim| crate::BeamSample {
            key,
            aim,
            execution_server_tick: 0,
            query_server_tick: 0,
            query_gameplay_tick: tick,
            query_fraction: 0,
            accepted_gameplay_tick: tick,
        };
        if let Some(OwnerCombatAction::BeamStop { key }) = action
            && key.valid()
            && key.actor == hero.actor.view.id
            && key.actor_generation == hero.combat_identity.generation
            && hero.ray.last_key.is_none_or(|last| {
                !last.same_stream(key) || key.command_sequence > last.command_sequence
            })
        {
            hero.ray.beam = None;
            hero.ray.last_key = Some(key);
        }
        if let Some((key, aim)) = beam_aim
            && let Some(beam) = &mut hero.ray.beam
            && key.valid()
            && key.same_stream(beam.key)
            && key.command_sequence > beam.sample.key.command_sequence
            && aim.iter().all(|v| v.is_finite() && v.abs() <= 1.0)
            && engine_core::spatial::length(engine_core::spatial::normalize(aim)) > 0.0
        {
            beam.sample = saved(key, aim);
        }
        if let Some(beam) = &hero.ray.beam {
            if hero.motion.dash_ticks > 0
                || tick - beam.started_tick >= crate::BEAM_DURATION_TICKS
                || tick - beam.sample.accepted_gameplay_tick > 9
            {
                hero.ray.beam = None;
            } else if tick - beam.last_damage_tick >= crate::BEAM_DAMAGE_TICKS {
                let mut next = beam.clone();
                next.last_damage_tick = tick;
                if (tick - beam.started_tick) % crate::BEAM_AMMO_TICKS == 0
                    && !hero.ray.ammo.try_spend(1.0)
                {
                    hero.ray.beam = None;
                } else {
                    hero.ray.beam = Some(next);
                }
            }
        }
        if let Some(OwnerCombatAction::Ray { key, aim } | OwnerCombatAction::BeamBegin { key, aim }) =
            action
            && key.actor == hero.actor.view.id
            && key.actor_generation == hero.combat_identity.generation
            && hero.motion.dash_ticks == 0
            && !had_beam
            && hero.ray.beam.is_none()
            && hero.ray.fire_eligibility(key, aim).is_ok()
            && (!matches!(action, Some(OwnerCombatAction::BeamBegin { .. }))
                || u32::try_from(key.command_sequence).is_ok())
        {
            hero.ray.accept(key);
            if matches!(action, Some(OwnerCombatAction::BeamBegin { .. })) {
                hero.ray.beam = Some(crate::BeamEpisode {
                    key,
                    started_tick: tick,
                    last_damage_tick: tick,
                    sample: saved(key, aim),
                });
            }
        }
        let tick = hero
            .combat_identity
            .tick
            .checked_add(1)
            .ok_or(ReplicationError::InvalidState)?;
        crate::systems::advance_combat_owner(
            &mut hero.combat_identity,
            &mut hero.defense_episodes,
            &hero.combat,
            tick,
        )
        .map_err(|_| ReplicationError::InvalidState)?;
        Ok(())
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
        self.checkpoint
            .hero
            .ray
            .graphics(&self.checkpoint.hero.motion)
    }
    pub fn starfall_retired(&self) -> &[crate::starfall::SpawnKey] {
        &self.starfall_retired
    }
    pub fn starfall_cues(&self) -> &[crate::starfall::SpawnKey] {
        &self.starfall_cues
    }
    pub fn starfall_tick(&self) -> u32 {
        self.checkpoint.hero.combat_identity.tick
    }
    pub fn starfall_repeats(&self) -> &[crate::starfall::StarfallRepeat] {
        &self.checkpoint.starfall.repeats
    }
    pub fn starfall_flights(&self) -> &[crate::starfall::StarfallFlight] {
        &self.checkpoint.starfall.flights
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
}
