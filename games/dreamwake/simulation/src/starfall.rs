//! Owner-safe Starfall flight. Damage and target contact remain authoritative.
use crate::{EssenceKind, combat::RayActionKey};
use engine_core::{
    CollisionScene, CollisionTarget, ProjectileSourcePolicy, ProjectileState, SceneQueryBudget,
};
use serde::{Deserialize, Serialize};

/// Live bolts plus reserved Echo bolts share one hard per-owner allowance.
pub const STARFALL_CAPACITY: usize = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnKey {
    pub action: RayActionKey,
    pub ordinal: u8,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StarfallFlight {
    pub key: SpawnKey,
    pub origin_tick: u32,
    pub position: [f32; 2],
    pub direction: [f32; 2],
    pub radius: f32,
    pub remaining: f32,
    pub essence: Option<EssenceKind>,
    /// Authority identity only; predicted execution never allocates one.
    pub authority_id: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StarfallRepeat {
    pub authority_id: Option<u64>,
    pub key: SpawnKey,
    pub origin_tick: u32,
    pub remaining: f32,
    pub origin: [f32; 2],
    pub direction: [f32; 2],
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StarfallDomain {
    pub flights: Vec<StarfallFlight>,
    pub repeats: Vec<StarfallRepeat>,
}
impl StarfallDomain {
    pub fn valid(&self, owner: u64, generation: u32) -> bool {
        let keys = self
            .flights
            .iter()
            .map(|f| f.key)
            .chain(self.repeats.iter().map(|r| r.key))
            .collect::<Vec<_>>();
        keys.len() <= STARFALL_CAPACITY
            && keys.iter().enumerate().all(|(i, k)| {
                k.action.valid()
                    && k.action.actor == owner
                    && k.action.actor_generation == generation
                    && k.ordinal <= 3
                    && !keys[..i].contains(k)
            })
            && self.flights.iter().all(|f| {
                f.position
                    .into_iter()
                    .chain(f.direction)
                    .all(f32::is_finite)
                    && f.radius > 0.0
                    && f.radius <= 0.527
                    && f.remaining > 0.0
                    && f.remaining <= 2.0
                    && f.authority_id != Some(0)
            })
            && self.repeats.iter().all(|r| {
                r.key.ordinal == 3
                    && r.authority_id != Some(0)
                    && r.remaining >= 0.0
                    && r.remaining <= 0.55
                    && r.origin.into_iter().chain(r.direction).all(f32::is_finite)
            })
    }
    pub(crate) fn advance(
        &mut self,
        scene: &CollisionScene,
    ) -> Result<Vec<SpawnKey>, engine_core::KinematicError> {
        // Echo follows the same delayed-action phase as authority: before flight.
        for repeat in &mut self.repeats {
            repeat.remaining = (repeat.remaining - crate::DT).max(0.0);
        }
        for repeat in self.repeats.iter().filter(|r| r.remaining == 0.0) {
            let mut spawned = flight(
                repeat.key,
                repeat.origin_tick,
                repeat.origin,
                repeat.direction,
                None,
            );
            spawned.authority_id = repeat.authority_id;
            self.flights.push(spawned);
        }
        self.repeats.retain(|r| r.remaining > 0.0);
        let mut budget = SceneQueryBudget::new(1_048_576, 65_536).expect("fixed flight budget");
        for f in &mut self.flights {
            let mut state = f.engine_state();
            state.advance(
                crate::DT,
                &[CollisionTarget {
                    id: f.key.action.actor,
                    faction: 1,
                    position: [0.0; 2],
                    radius: 0.48,
                    active: true,
                }],
            );
            f.position = state.position;
            f.remaining = state.remaining;
            let start = bevy::prelude::Vec3::new(
                state.previous_position[0],
                0.9,
                state.previous_position[1],
            );
            let end = bevy::prelude::Vec3::new(f.position[0], 0.9, f.position[1]);
            if state.finished()
                || scene
                    .sphere_cast(start, end - start, f.radius, &mut budget)?
                    .is_some()
            {
                f.remaining = 0.0;
            }
        }
        let retired = self
            .flights
            .iter()
            .filter(|f| f.remaining <= 0.0)
            .map(|f| f.key)
            .collect();
        self.flights.retain(|f| f.remaining > 0.0);
        Ok(retired)
    }
}
impl StarfallFlight {
    pub(crate) fn engine_state(&self) -> ProjectileState {
        ProjectileState {
            id: self.authority_id.unwrap_or(0),
            owner: self.key.action.actor,
            faction: 1,
            position: self.position,
            previous_position: self.position,
            direction: self.direction,
            speed: 23.0,
            remaining: self.remaining,
            radius: self.radius,
            hits_remaining: if self.essence == Some(EssenceKind::Vast) {
                4
            } else {
                1
            },
            hit_ids: vec![],
            max_distance: Some(crate::ARENA_RADIUS + 4.0),
            source_policy: ProjectileSourcePolicy::RequirePresent,
            expired: false,
            pending_impacts: vec![],
        }
    }
}
pub(crate) fn spawn_count(essence: Option<EssenceKind>) -> usize {
    match essence {
        Some(EssenceKind::Twin) => 3,
        Some(EssenceKind::Echo) => 2,
        _ => 1,
    }
}
pub(crate) fn accept(
    memory: &mut crate::EquippedMemory,
    occupied: usize,
) -> Result<(), crate::combat::ActionReason> {
    if memory.kind != crate::MemoryKind::Starfall {
        return Err(crate::combat::ActionReason::InvalidAction);
    }
    if occupied + spawn_count(memory.modifier) > STARFALL_CAPACITY {
        return Err(crate::combat::ActionReason::Budget);
    }
    if !memory.activate() {
        return Err(crate::combat::ActionReason::Cooldown);
    }
    Ok(())
}
pub(crate) fn flight(
    key: SpawnKey,
    tick: u32,
    origin: [f32; 2],
    direction: [f32; 2],
    essence: Option<EssenceKind>,
) -> StarfallFlight {
    StarfallFlight {
        key,
        origin_tick: tick,
        position: engine_core::spatial::add(origin, engine_core::spatial::scale(direction, 0.9)),
        direction: engine_core::spatial::normalize(direction),
        radius: 0.34
            * if essence == Some(EssenceKind::Vast) {
                1.55
            } else {
                1.0
            },
        remaining: 2.0,
        essence,
        authority_id: None,
    }
}
pub(crate) fn spawn(
    domain: &mut StarfallDomain,
    key: RayActionKey,
    tick: u32,
    origin: [f32; 2],
    direction: [f32; 2],
    essence: Option<EssenceKind>,
) {
    let angles: &[(u8, f32)] = if essence == Some(EssenceKind::Twin) {
        &[(1, -0.19), (0, 0.0), (2, 0.19)]
    } else {
        &[(0, 0.0)]
    };
    for &(ordinal, angle) in angles {
        domain.flights.push(flight(
            SpawnKey {
                action: key,
                ordinal,
            },
            tick,
            origin,
            crate::systems::rotate(direction, angle),
            essence,
        ));
    }
    if essence == Some(EssenceKind::Echo) {
        domain.repeats.push(StarfallRepeat {
            authority_id: None,
            key: SpawnKey {
                action: key,
                ordinal: 3,
            },
            origin_tick: tick,
            remaining: 0.55,
            origin,
            direction,
        });
    }
}
impl crate::DreamSimulation {
    /// Trusted authority metadata for binding accepted, source-owned spawns.
    pub fn starfall_bindings(&mut self, key: RayActionKey) -> Vec<(u8, u64)> {
        let mut bindings = self
            .world
            .query::<(&crate::Projectile, &ProjectileState)>()
            .iter(&self.world)
            .filter_map(|(payload, state)| {
                let (spawn, _) = payload.spawn?;
                (spawn.action == key).then_some((spawn.ordinal, state.id))
            })
            .collect::<Vec<_>>();
        bindings.extend(
            self.world
                .query::<&crate::DelayedCast>()
                .iter(&self.world)
                .filter_map(|delayed| {
                    let (spawn, _) = delayed.payload.spawn?;
                    (spawn.action == key).then_some((spawn.ordinal, delayed.payload.id))
                }),
        );
        bindings
    }
}
impl StarfallFlight {
    pub fn graphics_instance(&self) -> Option<engine_core::GraphicsInstance> {
        let action = self.key.action;
        Some(engine_core::GraphicsInstance {
            id: engine_core::GraphicsId {
                match_epoch: action.match_epoch,
                owner: action.actor,
                action_seq: u32::try_from(action.command_sequence).ok()?,
                slot: 61_000 + u16::from(action.action_slot) * 4 + u16::from(self.key.ordinal),
                scope: Some(engine_core::GraphicsScope {
                    source_epoch: action.connection_epoch,
                    stream: action.command_stream,
                    control_epoch: action.ownership_epoch,
                    generation: action.actor_generation,
                }),
            },
            kind: engine_core::GraphicsKind::Orb { elevation: 0.9 },
            pos: self.position,
            radius: self.radius,
            age_ticks: ((2.0 - self.remaining).max(0.0) / crate::DT).round() as u32,
            duration_ticks: 120,
        })
    }
}
