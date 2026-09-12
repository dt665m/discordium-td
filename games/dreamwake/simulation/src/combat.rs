//! Game-owned authority inputs and terminal combat results.
//! Network timing validation belongs to the authority adapter. These types never
//! accept a client origin, target selection, damage amount, range or weapon state.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RayActionKey {
    pub match_epoch: u32,
    pub connection_epoch: u64,
    pub command_stream: u32,
    pub ownership_epoch: u32,
    pub actor: u64,
    pub actor_generation: u32,
    pub command_sequence: u64,
    pub action_slot: u8,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValidatedRay {
    pub key: RayActionKey,
    pub execution_server_tick: u64,
    pub query_server_tick: u64,
    /// Exact mapping from the authority's committed server/gameplay tick ledger.
    pub query_gameplay_tick: u32,
    /// Hardware-edge phase within command C's authoritative S[C-1]→S[C] interval.
    /// None is the trusted fixed-tick origin policy (offline actions and beam pulses).
    pub command_fraction: Option<u16>,
    pub query_fraction: u16,
    pub aim: [f32; 2],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombatTarget {
    pub id: u64,
    pub generation: u32,
}
/// Deterministic transaction identity, with no independently allocated counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DamageTransaction {
    pub action: RayActionKey,
    pub target: CombatTarget,
    pub hit_slot: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombatReason {
    Hit,
    Miss,
    StaticBlocker,
    DynamicBlocker,
    HistoricalInvulnerability,
    MissingHistory,
    WrongLifecycle,
    InvalidAction,
    NotEligible,
    NoAmmo,
    Cooldown,
    Budget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombatVerdict {
    pub key: RayActionKey,
    pub execution_server_tick: u64,
    pub execution_gameplay_tick: u32,
    pub query_server_tick: u64,
    pub query_gameplay_tick: u32,
    pub reason: CombatReason,
    pub target: Option<CombatTarget>,
    pub hit_region: u8,
    pub damage: f32,
    pub transaction: Option<DamageTransaction>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionReason {
    Accepted,
    Conflict,
    Inactive,
    InvalidAction,
    Cooldown,
    Resource,
    Collision,
    MissingHistory,
    Budget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionVerdict {
    pub key: RayActionKey,
    pub execution_server_tick: u64,
    pub execution_gameplay_tick: u32,
    pub reason: ActionReason,
    pub combat: Option<CombatVerdict>,
}

impl RayActionKey {
    pub(crate) fn valid(self) -> bool {
        self.match_epoch > 0
            && self.actor > 0
            && self.actor < 1 << 63
            && self.actor_generation > 0
            && self.command_sequence > 0
            && self.action_slot < 8
            && if self.connection_epoch == 0 {
                self.command_stream == 0 && self.ownership_epoch == 0
            } else {
                self.command_stream > 0 && self.ownership_epoch > 0
            }
    }
    pub(crate) fn same_stream(self, other: Self) -> bool {
        self.match_epoch == other.match_epoch
            && self.connection_epoch == other.connection_epoch
            && self.command_stream == other.command_stream
            && self.ownership_epoch == other.ownership_epoch
            && self.actor == other.actor
            && self.actor_generation == other.actor_generation
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum CombatAction {
    Charge {
        key: RayActionKey,
        execution_server_tick: u64,
        command: engine_core::ChargeCommand,
        aim: [f32; 2],
    },
    Dash {
        key: RayActionKey,
        execution_server_tick: u64,
        direction: [f32; 2],
    },
    Cast {
        key: RayActionKey,
        execution_server_tick: u64,
        slot: u8,
        aim: [f32; 2],
    },
    Ray(ValidatedRay),
    BeamBegin(ValidatedRay),
    BeamStop {
        key: RayActionKey,
        execution_server_tick: u64,
    },
    /// Continuous latest accepted sample; never a terminal edge receipt.
    BeamAim(ValidatedRay),
    /// Offline action at this fixed tick's authoritative pose; no fabricated
    /// network reference or compensated-view claim is created.
    RayCurrent {
        actor: u64,
        aim: [f32; 2],
    },
}
impl CombatAction {
    pub(crate) fn key(&self) -> RayActionKey {
        match self {
            Self::Charge { key, .. }
            | Self::Dash { key, .. }
            | Self::Cast { key, .. }
            | Self::BeamStop { key, .. } => *key,
            Self::Ray(ray) | Self::BeamBegin(ray) | Self::BeamAim(ray) => ray.key,
            Self::RayCurrent { .. } => unreachable!("offline request normalized before insertion"),
        }
    }
    pub(crate) fn execution(&self) -> u64 {
        match self {
            Self::Charge {
                execution_server_tick,
                ..
            }
            | Self::Dash {
                execution_server_tick,
                ..
            }
            | Self::Cast {
                execution_server_tick,
                ..
            }
            | Self::BeamStop {
                execution_server_tick,
                ..
            } => *execution_server_tick,
            Self::Ray(ray) | Self::BeamBegin(ray) | Self::BeamAim(ray) => ray.execution_server_tick,
            Self::RayCurrent { .. } => 0,
        }
    }
}
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct ActionTransactions {
    pub requests: Vec<CombatAction>,
    pub verdicts: Vec<ActionVerdict>,
    pub ray_commits: Vec<(u64, crate::RayState)>,
    pub internal_verdict_start: Option<usize>,
}
impl ActionTransactions {
    pub fn charge(&mut self, actor: u64, tick: u32, reason: ActionReason) {
        if let Some(request) = self
            .requests
            .iter()
            .find(|r| matches!(r, CombatAction::Charge { key, .. } if key.actor == actor))
        {
            self.verdicts.push(ActionVerdict {
                key: request.key(),
                execution_server_tick: request.execution(),
                execution_gameplay_tick: tick,
                reason,
                combat: None,
            });
        }
    }

    pub fn simple(&mut self, actor: u64, slot: Option<u8>, tick: u32, reason: ActionReason) {
        let request = self.requests.iter().find(|request| match request {
            CombatAction::Dash { key, .. } => key.actor == actor && slot.is_none(),
            CombatAction::Cast {
                key, slot: cast, ..
            } => key.actor == actor && slot == Some(*cast),
            _ => false,
        });
        if let Some(request) = request {
            let key = request.key();
            if self.verdicts.iter().all(|v| v.key != key) {
                self.verdicts.push(ActionVerdict {
                    key,
                    execution_server_tick: request.execution(),
                    execution_gameplay_tick: tick,
                    reason,
                    combat: None,
                });
            }
        }
    }
    pub fn fail_pending(&mut self, tick: u32) {
        self.ray_commits.clear();
        for verdict in &mut self.verdicts {
            if verdict.reason == ActionReason::Accepted && verdict.combat.is_some() {
                verdict.reason = ActionReason::Budget;
                if let Some(combat) = &mut verdict.combat {
                    combat.reason = CombatReason::Budget;
                    combat.damage = 0.0;
                    combat.transaction = None;
                }
            }
        }
        let missing = self
            .requests
            .iter()
            .filter(|r| !matches!(r, CombatAction::BeamAim(_)))
            .filter(|r| self.verdicts.iter().all(|v| v.key != r.key()))
            .map(|r| ActionVerdict {
                key: r.key(),
                execution_server_tick: r.execution(),
                execution_gameplay_tick: tick,
                reason: ActionReason::Budget,
                combat: None,
            })
            .collect::<Vec<_>>();
        self.verdicts.extend(missing);
    }
}
impl crate::DreamSimulation {
    pub fn combat_generation(&mut self, actor: u64) -> Option<u32> {
        self.world
            .query::<crate::HeroActorReadOnly>()
            .iter(&self.world)
            .find(|h| h.view.id == actor)
            .map(|h| h.combat_identity.generation)
    }
    /// Validate a bounded set of explicit actions, execute exactly one shared
    /// fixed tick, and return terminal gameplay outcomes. Structural overflow
    /// rejects before the tick; ordinary action rejection still executes it.
    pub fn step_multiplayer_with_actions(
        &mut self,
        inputs: &[(u64, crate::DreamInput)],
        actions: &[CombatAction],
    ) -> Result<Vec<ActionVerdict>, CombatReason> {
        use bevy::prelude::*;
        self.world
            .resource_mut::<crate::combat_trace::CombatTraceState>()
            .pending
            .clear();
        if actions.len() > 64 {
            return Err(CombatReason::Budget);
        }
        let run = self.world.resource::<crate::Run>().clone();
        let roster = self
            .world
            .query::<crate::HeroActorReadOnly>()
            .iter(&self.world)
            .map(|h| {
                (
                    h.view.id,
                    (h.combat_identity.generation, h.active && h.health.hp > 0.0),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut commands = inputs.to_vec();
        let mut normalized = std::collections::BTreeMap::new();
        let mut conflicts = std::collections::BTreeSet::new();
        let mut beam_samples = Vec::new();
        for action in actions {
            if let CombatAction::BeamAim(sample) = action {
                beam_samples.push(*sample);
                continue;
            }
            let action = match action {
                CombatAction::RayCurrent { actor, aim } => {
                    let generation = roster.get(actor).map_or(1, |r| r.0);
                    CombatAction::Ray(ValidatedRay {
                        command_fraction: None,
                        query_fraction: 0,
                        key: RayActionKey {
                            match_epoch: ((run.seed ^ (run.seed >> 32)) as u32).max(1),
                            connection_epoch: 0,
                            command_stream: 0,
                            ownership_epoch: 0,
                            actor: *actor,
                            actor_generation: generation,
                            command_sequence: u64::from(run.tick) + 1,
                            action_slot: 0,
                        },
                        execution_server_tick: 0,
                        query_server_tick: 0,
                        query_gameplay_tick: u32::MAX,
                        aim: *aim,
                    })
                }
                _ => action.clone(),
            };
            let key = action.key();
            if normalized.get(&key).is_some_and(|old| old != &action) {
                conflicts.insert(key);
            }
            normalized.insert(key, action);
        }
        let mut transactions = ActionTransactions::default();
        let mut actors = std::collections::BTreeSet::new();
        for (key, action) in normalized {
            let reject = if !key.valid()
                || conflicts.contains(&key)
                || roster
                    .get(&key.actor)
                    .is_none_or(|r| r.0 != key.actor_generation)
            {
                Some(ActionReason::InvalidAction)
            } else if run.phase != crate::RunPhase::Combat
                || run.paused
                || roster.get(&key.actor).is_none_or(|r| !r.1)
            {
                Some(ActionReason::Inactive)
            } else if !actors.insert(key.actor) {
                Some(ActionReason::Conflict)
            } else {
                None
            };
            if let Some(reason) = reject {
                transactions.verdicts.push(ActionVerdict {
                    key,
                    execution_server_tick: action.execution(),
                    execution_gameplay_tick: run.tick,
                    reason,
                    combat: None,
                });
                continue;
            }
            if commands.iter().all(|(id, _)| *id != key.actor) {
                commands.push((key.actor, crate::DreamInput::default()));
            }
            let input = &mut commands
                .iter_mut()
                .find(|(id, _)| *id == key.actor)
                .unwrap()
                .1;
            input.dash = false;
            input.charge = engine_core::ChargeCommand::None;
            input.casts = [false; 4];
            let valid = match &action {
                CombatAction::Charge { command, aim, .. } => {
                    input.charge = *command;
                    input.aim = *aim;
                    aim.iter().all(|v| v.is_finite() && v.abs() <= 1.0)
                }
                CombatAction::Dash { direction, .. } => {
                    if direction.iter().all(|v| v.is_finite() && v.abs() <= 1.0) {
                        input.dash = true;
                        input.movement = *direction;
                        if let Ok(sequence) = u32::try_from(key.command_sequence) {
                            input.action_sequences[0] = sequence;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                CombatAction::Cast { slot, aim, .. } => {
                    if *slot < 4 && aim.iter().all(|v| v.is_finite() && v.abs() <= 1.0) {
                        input.casts[usize::from(*slot)] = true;
                        input.aim = *aim;
                        if let Ok(sequence) = u32::try_from(key.command_sequence) {
                            input.action_sequences[usize::from(*slot) + 1] = sequence;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                CombatAction::Ray(_)
                | CombatAction::BeamBegin(_)
                | CombatAction::BeamStop { .. } => true,
                CombatAction::RayCurrent { .. } | CombatAction::BeamAim(_) => unreachable!(),
            };
            if valid {
                transactions.requests.push(action);
            } else {
                input.dash = false;
                input.charge = engine_core::ChargeCommand::None;
                input.casts = [false; 4];
                transactions.verdicts.push(ActionVerdict {
                    key,
                    execution_server_tick: action.execution(),
                    execution_gameplay_tick: run.tick,
                    reason: ActionReason::InvalidAction,
                    combat: None,
                });
            }
        }
        beam_samples.sort_by_key(|sample| sample.key);
        transactions
            .requests
            .extend(beam_samples.into_iter().map(CombatAction::BeamAim));
        self.world.insert_resource(transactions);
        self.world.resource_mut::<crate::DreamInputs>().0 = commands;
        self.world.run_schedule(crate::DreamStep);
        let tick = self.world.resource::<crate::Run>().tick;
        let mut transactions = self.world.resource_mut::<ActionTransactions>();
        if let Some(start) = transactions.internal_verdict_start.take() {
            transactions.verdicts.truncate(start);
        }
        let missing = transactions
            .requests
            .iter()
            .filter(|r| !matches!(r, CombatAction::BeamAim(_)))
            .filter(|r| transactions.verdicts.iter().all(|v| v.key != r.key()))
            .map(|r| ActionVerdict {
                key: r.key(),
                execution_server_tick: r.execution(),
                execution_gameplay_tick: tick,
                reason: ActionReason::Inactive,
                combat: None,
            })
            .collect::<Vec<_>>();
        transactions.verdicts.extend(missing);
        transactions.requests.clear();
        transactions.ray_commits.clear();
        for verdict in &mut transactions.verdicts {
            verdict.execution_gameplay_tick = tick;
        }
        transactions.verdicts.sort_by_key(|v| v.key);
        Ok(std::mem::take(&mut transactions.verdicts))
    }
}
