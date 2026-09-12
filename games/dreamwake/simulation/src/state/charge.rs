//! Dreamwake charge cost, interruption and authored travel policy.
use bevy::prelude::*;
use engine_core::{ChargeCommand, ChargePhase, ChargeState, Meter, MotionCurveKey};
use serde::{Deserialize, Serialize};
pub(crate) const CHARGE_MIN_TICKS: u16 = 6;
pub(crate) const CHARGE_MAX_TICKS: u16 = 36;
pub(crate) const CHARGE_COOLDOWN: u16 = 90;
pub(crate) const CHARGE_MAX_DISTANCE: f32 = 7.5;
pub(crate) const CHARGE_CURVE: MotionCurveKey = MotionCurveKey { id: 1, version: 1 };
pub(crate) const CHARGE_SAMPLES: [f32; 13] = [
    0.0, 0.04, 0.11, 0.21, 0.34, 0.49, 0.64, 0.77, 0.87, 0.94, 0.98, 0.995, 1.0,
];
pub(crate) const STUN_STATUS: engine_core::StatusId = engine_core::StatusId(2);
#[derive(Debug)]
pub(crate) struct Stamina;
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChargedMovement {
    pub state: ChargeState,
    pub stamina: Meter<Stamina>,
    pub begin_key: Option<crate::combat::RayActionKey>,
}
impl Default for ChargedMovement {
    fn default() -> Self {
        Self {
            state: ChargeState::default(),
            stamina: Meter::new(100.0, 100.0).unwrap(),
            begin_key: None,
        }
    }
}
impl ChargedMovement {
    /// Conservative next-sample reach for missing collision dependency admission.
    pub fn maximum_displacement(&self, command: ChargeCommand) -> f32 {
        match self.state.phase {
            ChargePhase::Executing {
                cursor, distance, ..
            } => CHARGE_SAMPLES
                .get(usize::from(cursor)..usize::from(cursor) + 2)
                .map_or(CHARGE_MAX_DISTANCE, |s| (s[1] - s[0]) * distance),
            ChargePhase::Charging { .. } if matches!(command, ChargeCommand::Release { .. }) => {
                CHARGE_SAMPLES[1] * CHARGE_MAX_DISTANCE
            }
            _ => 0.0,
        }
    }
    pub fn valid(&self, actor: u64, generation: u32) -> bool {
        self.stamina.capacity() == 100.0
            && self.state.cooldown_ticks <= CHARGE_COOLDOWN
            && self.begin_key.is_none_or(|k| {
                k.valid()
                    && k.actor == actor
                    && k.actor_generation == generation
                    && k.command_sequence == self.state.episode
            })
            && match self.state.phase {
                ChargePhase::Idle => true,
                ChargePhase::Charging { ticks } => {
                    self.state.episode > 0 && ticks <= CHARGE_MAX_TICKS
                }
                ChargePhase::Executing {
                    curve,
                    cursor,
                    direction,
                    distance,
                } => {
                    self.state.episode > 0
                        && curve == CHARGE_CURVE
                        && usize::from(cursor) < CHARGE_SAMPLES.len() - 1
                        && distance.is_finite()
                        && (3.0..=CHARGE_MAX_DISTANCE).contains(&distance)
                        && direction.iter().all(|v| v.is_finite())
                        && (Vec2::from_array(direction).length_squared() - 1.0).abs() < 0.001
                }
            }
    }
    pub fn prepare(
        &mut self,
        command: ChargeCommand,
        key: Option<crate::combat::RayActionKey>,
        aim: [f32; 2],
        interrupted: bool,
    ) -> crate::combat::ActionReason {
        use crate::combat::ActionReason as R;
        self.state.tick(CHARGE_MAX_TICKS);
        if self.state.phase == ChargePhase::Idle {
            self.stamina.restore(18.0 * crate::DT);
        }
        if interrupted {
            self.state.interrupt();
            return R::Inactive;
        }
        if let Some(key) = key {
            if !key.valid() {
                return R::InvalidAction;
            }
            if !matches!(command, ChargeCommand::Begin { .. })
                && self.begin_key.is_none_or(|begin| {
                    !begin.same_stream(key) || key.command_sequence <= begin.command_sequence
                })
            {
                return R::InvalidAction;
            }
        }
        match command {
            ChargeCommand::None => R::Accepted,
            ChargeCommand::Begin { episode } => {
                if key.is_some_and(|k| k.command_sequence != episode) {
                    return R::InvalidAction;
                }
                if self.stamina.current() < crate::CHARGE_STAMINA_COST {
                    return R::Resource;
                }
                // A new authenticated command stream starts a new episode namespace.
                if key.is_some_and(|k| self.begin_key.is_some_and(|b| !b.same_stream(k)))
                    && self.state.phase == ChargePhase::Idle
                    && self.state.cooldown_ticks == 0
                {
                    self.state.episode = 0;
                }
                if self.state.begin(episode) {
                    self.begin_key = key;
                    R::Accepted
                } else {
                    R::Cooldown
                }
            }
            ChargeCommand::Cancel { episode } => {
                if self.state.cancel(episode) {
                    R::Accepted
                } else {
                    R::InvalidAction
                }
            }
            ChargeCommand::Release { episode } => {
                let ChargePhase::Charging { ticks } = self.state.phase else {
                    return R::InvalidAction;
                };
                if episode != self.state.episode {
                    return R::InvalidAction;
                }
                if ticks < CHARGE_MIN_TICKS {
                    self.state.interrupt();
                    return R::Accepted;
                }
                let direction = Vec2::from_array(aim).normalize_or_zero();
                if direction == Vec2::ZERO || !direction.is_finite() {
                    return R::InvalidAction;
                }
                if !self.stamina.try_spend(crate::CHARGE_STAMINA_COST) {
                    self.state.interrupt();
                    return R::Resource;
                }
                self.state.cooldown_ticks = CHARGE_COOLDOWN;
                self.state.phase = ChargePhase::Executing {
                    curve: CHARGE_CURVE,
                    cursor: 0,
                    direction: direction.to_array(),
                    distance: 3.0
                        + (CHARGE_MAX_DISTANCE - 3.0) * f32::from(ticks - CHARGE_MIN_TICKS)
                            / f32::from(CHARGE_MAX_TICKS - CHARGE_MIN_TICKS),
                };
                R::Accepted
            }
        }
    }
}
