use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub(crate) const DREAMLANCE_CAPACITY: u8 = 6;
pub(crate) const DREAMLANCE_COOLDOWN: f32 = 0.5;
pub(crate) const DREAMLANCE_RANGE: f32 = 20.0;
pub(crate) const DREAMLANCE_DAMAGE: f32 = 32.0;
#[derive(Debug)]
pub(crate) struct RayAmmo;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RayState {
    pub ammo: engine_core::Meter<RayAmmo>,
    pub cooldown: f32,
    pub last_key: Option<crate::combat::RayActionKey>,
    pub beam: Option<BeamEpisode>,
}
impl Default for RayState {
    fn default() -> Self {
        Self {
            ammo: engine_core::Meter::new(
                f32::from(DREAMLANCE_CAPACITY),
                f32::from(DREAMLANCE_CAPACITY),
            )
            .expect("game ammo capacity"),
            cooldown: 0.0,
            last_key: None,
            beam: None,
        }
    }
}
impl RayState {
    pub fn fire_eligibility(
        &self,
        key: crate::combat::RayActionKey,
        aim: [f32; 2],
    ) -> Result<(), crate::combat::CombatReason> {
        use crate::combat::CombatReason;
        if !key.valid()
            || aim.iter().any(|v| !v.is_finite() || v.abs() > 1.0)
            || engine_core::spatial::length(engine_core::spatial::normalize(aim)) == 0.0
            || self.last_key.is_some_and(|last| {
                last.same_stream(key) && key.command_sequence <= last.command_sequence
            })
        {
            return Err(CombatReason::InvalidAction);
        }
        if self.cooldown > 0.0 {
            return Err(CombatReason::Cooldown);
        }
        if self.ammo.current() < 1.0 {
            return Err(CombatReason::NoAmmo);
        }
        Ok(())
    }
    pub fn accept(&mut self, key: crate::combat::RayActionKey) {
        assert!(self.ammo.try_spend(1.0));
        self.cooldown = DREAMLANCE_COOLDOWN;
        self.last_key = Some(key);
    }
    pub fn tick(&mut self) {
        engine_core::advance_cooldown(&mut self.cooldown, crate::DT);
    }
    pub fn refill(&mut self) {
        self.ammo.restore(self.ammo.capacity());
        self.cooldown = 0.0;
        self.beam = None;
    }
    pub fn valid(&self, actor: u64, generation: u32) -> bool {
        self.beam
            .as_ref()
            .is_none_or(|beam| beam.valid(actor, generation))
            && self.ammo.capacity() == f32::from(DREAMLANCE_CAPACITY)
            && self.ammo.current().fract() == 0.0
            && self.cooldown.is_finite()
            && (0.0..=DREAMLANCE_COOLDOWN).contains(&self.cooldown)
            && self
                .last_key
                .is_none_or(|k| k.actor == actor && k.actor_generation == generation && k.valid())
    }
}

pub(crate) const BEAM_DURATION_TICKS: u32 = 120;
pub(crate) const BEAM_DAMAGE_TICKS: u32 = 6;
pub(crate) const BEAM_AMMO_TICKS: u32 = 30;
/// Saved timing is data, not proof that an inbound client sample was validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BeamSample {
    pub key: crate::combat::RayActionKey,
    pub execution_server_tick: u64,
    pub query_server_tick: u64,
    pub query_gameplay_tick: u32,
    pub query_fraction: u16,
    pub accepted_gameplay_tick: u32,
    pub aim: [f32; 2],
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BeamEpisode {
    pub key: crate::combat::RayActionKey,
    pub started_tick: u32,
    pub last_damage_tick: u32,
    pub sample: BeamSample,
}
impl BeamEpisode {
    fn valid(&self, actor: u64, generation: u32) -> bool {
        self.key.valid()
            && u32::try_from(self.key.command_sequence).is_ok()
            && self.key.actor == actor
            && self.key.actor_generation == generation
            && self.started_tick > 0
            && self.last_damage_tick >= self.started_tick
            && self.last_damage_tick - self.started_tick < BEAM_DURATION_TICKS
            && self.sample.key.same_stream(self.key)
            && self.sample.key.valid()
            && self.sample.key.command_sequence >= self.key.command_sequence
            && self.sample.execution_server_tick >= self.sample.query_server_tick
            && self.sample.accepted_gameplay_tick >= self.started_tick
            && self.sample.query_gameplay_tick <= self.sample.accepted_gameplay_tick
            && (self.sample.query_fraction == 0
                || self.sample.query_gameplay_tick < self.sample.accepted_gameplay_tick)
            && self.sample.accepted_gameplay_tick - self.sample.query_gameplay_tick <= 9
            && self
                .sample
                .aim
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1.0)
            && engine_core::spatial::length(engine_core::spatial::normalize(self.sample.aim)) > 0.0
    }
}

impl RayState {
    pub fn graphics(
        &self,
        motion: &engine_core::KinematicState,
    ) -> Option<engine_core::GraphicsInstance> {
        let beam = self.beam.as_ref()?;
        let aim = engine_core::spatial::normalize(beam.sample.aim);
        Some(engine_core::GraphicsInstance {
            id: engine_core::GraphicsId {
                scope: Some(engine_core::GraphicsScope {
                    source_epoch: beam.key.connection_epoch,
                    stream: beam.key.command_stream,
                    control_epoch: beam.key.ownership_epoch,
                    generation: beam.key.actor_generation,
                }),
                match_epoch: beam.key.match_epoch,
                owner: beam.key.actor,
                action_seq: u32::try_from(beam.key.command_sequence).ok()?,
                slot: 60_000 + u16::from(beam.key.action_slot),
            },
            kind: engine_core::GraphicsKind::Beam {
                direction: [aim[0], 0.0, aim[1]],
                elevation: motion.position[1]
                    + if motion.stance == engine_core::Stance::Crouched {
                        0.8
                    } else {
                        0.9
                    },
                length: DREAMLANCE_RANGE,
            },
            pos: [motion.position[0], motion.position[2]],
            radius: 0.06,
            age_ticks: 0,
            duration_ticks: 2,
        })
    }
    pub fn valid_at(&self, actor: u64, generation: u32, tick: u32) -> bool {
        self.valid(actor, generation)
            && self.beam.as_ref().is_none_or(|beam| {
                beam.last_damage_tick <= tick
                    && beam.sample.accepted_gameplay_tick <= tick
                    && tick - beam.started_tick < BEAM_DURATION_TICKS
            })
    }
}

pub(crate) fn valid_graphics_kind(kind: engine_core::GraphicsKind) -> bool {
    match kind {
        engine_core::GraphicsKind::RadialPulse => true,
        engine_core::GraphicsKind::Orb { elevation } => {
            elevation.is_finite() && elevation.abs() <= 100_000.0
        }
        engine_core::GraphicsKind::Beam {
            direction,
            elevation,
            length,
        } => {
            direction.iter().all(|v| v.is_finite() && v.abs() <= 1.0)
                && (Vec3::from_array(direction).length_squared() - 1.0).abs() <= 0.001
                && elevation.is_finite()
                && elevation.abs() <= 100_000.0
                && length.is_finite()
                && (0.0..=DREAMLANCE_RANGE).contains(&length)
        }
    }
}

pub(crate) fn valid_graphics_id(id: engine_core::GraphicsId) -> bool {
    id.match_epoch > 0
        && (id.owner != 0 || id.scope.is_none())
        && id.scope.is_none_or(|scope| {
            scope.generation > 0
                && if scope.source_epoch == 0 {
                    scope.stream == 0 && scope.control_epoch == 0
                } else {
                    scope.stream > 0 && scope.control_epoch > 0
                }
        })
}
