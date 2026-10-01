//! Owner replay through the shared gameplay simulation.
use dreamwake_protocol::{
    DreamAction,
    live::{TickInput, Welcome},
};
use dreamwake_sim::collision::CollisionWorld;
use dreamwake_sim::replication::{
    OwnerExpectation, OwnerPredictionRunner, OwnerPredictionState, PublicEnemyView,
    PublicPlatformView,
};
use engine_net::{
    commands::Command,
    prediction::rewind::{Rewind, RewindInput, RewindModel},
    types::*,
};
use std::sync::Arc;

pub(super) type InputCommand = Command<TickInput, DreamAction>;
pub(super) type Predictor = Rewind<OwnerModel>;
/// Ticks prediction may run ahead of the newest owner state and still replay:
/// about four seconds at 60 Hz.
pub(super) const HISTORY_TICKS: usize = 256;

/// What a tick reads besides the owner's own state. The collision world is
/// shared, platforms are the replicated poses the owner rides, and targets are
/// independently disclosed enemy poses that only guide reversible companion
/// targeting; none of them is an authoritative contact or damage source.
#[derive(Clone, Debug)]
pub(super) struct Dependencies {
    pub collision: Arc<CollisionWorld>,
    pub platforms: Vec<PublicPlatformView>,
    pub targets: Vec<PublicEnemyView>,
}
pub(super) use dreamwake_sim::replication::MAX_PREDICTION_TARGETS as MAX_PRESENTATION_TARGETS;

pub(super) struct OwnerModel {
    pub welcome: Welcome,
    pub collision: Arc<CollisionWorld>,
    runner: std::sync::Mutex<OwnerPredictionRunner>,
}
impl OwnerModel {
    /// The scene's collision is part of the content both sides agreed on in
    /// the Welcome, so the client builds it locally instead of receiving it.
    pub fn new(welcome: Welcome) -> Result<Self, String> {
        let manifest = dreamwake_sim::collision::CollisionManifest::current(u64::from(
            welcome.content.scene_revision.0,
        ));
        if manifest.identity() != welcome.content.scene {
            return Err("scene collision differs from the server's".into());
        }
        let collision = Arc::new(manifest.build().map_err(|e| format!("{e:?}"))?);
        Ok(Self {
            welcome,
            collision,
            runner: std::sync::Mutex::new(OwnerPredictionRunner::new()),
        })
    }
    pub fn expectation(&self) -> OwnerExpectation {
        OwnerExpectation {
            owner: self.welcome.player.get(),
            match_epoch: self.welcome.match_epoch,
            ownership_revision: self.welcome.stream.ownership.0,
            scene_revision: u64::from(self.welcome.content.scene_revision.0),
            minimum_revision: 0,
        }
    }
}
impl RewindModel for OwnerModel {
    type State = OwnerPredictionState;
    type Command = InputCommand;
    type Dependencies = Dependencies;
    type Error = String;
    fn step(
        &self,
        state: &mut OwnerPredictionState,
        _: ServerTick,
        input: &RewindInput<InputCommand>,
        dependencies: &Dependencies,
    ) -> Result<(), String> {
        let command = input.command();
        let bases: Vec<_> = state
            .required_bases()
            .iter()
            .map(|key| {
                dependencies
                    .platforms
                    .iter()
                    .find(|platform| platform.collider == *key)
                    .cloned()
                    .ok_or("missing required platform")
            })
            .collect::<Result<_, _>>()?;
        let mut held = match command {
            Some(command) => command.input.held,
            None => state.substitute_predicted_input(),
        };
        held.dash = false;
        held.casts = [false; 4];
        use dreamwake_sim::{combat::RayActionKey, replication::OwnerCombatAction};
        let mut action = None;
        let mut beam = None;
        if let Some(command) = command {
            let key = |slot| RayActionKey {
                match_epoch: self.welcome.match_epoch,
                connection_epoch: self.welcome.stream.epoch.0,
                command_stream: self.welcome.stream.stream.0,
                ownership_epoch: self.welcome.stream.ownership.0,
                actor: self.welcome.player.get(),
                actor_generation: state.combat_generation(),
                command_sequence: command.sequence.0,
                action_slot: slot,
            };
            if let Some(edge) = command
                .actions
                .as_slice()
                .iter()
                .min_by_key(|edge| edge.slot)
            {
                action = match edge.action {
                    DreamAction::Cast { slot, aim } => {
                        held.aim = aim;
                        Some(OwnerCombatAction::Cast {
                            key: key(edge.slot),
                            slot,
                            aim,
                        })
                    }
                    DreamAction::Dash { direction } => {
                        held.dash = true;
                        held.movement = direction;
                        Some(OwnerCombatAction::Dash {
                            key: key(edge.slot),
                            direction,
                        })
                    }
                    DreamAction::Dreamlance { aim, .. } => Some(OwnerCombatAction::Ray {
                        key: key(edge.slot),
                        aim,
                    }),
                    DreamAction::BeamBegin { aim, .. } => Some(OwnerCombatAction::BeamBegin {
                        key: key(edge.slot),
                        aim,
                    }),
                    DreamAction::ChargeBegin
                    | DreamAction::ChargeRelease { .. }
                    | DreamAction::ChargeCancel { .. } => Some(OwnerCombatAction::Charge {
                        key: key(edge.slot),
                        command: match edge.action {
                            DreamAction::ChargeBegin => engine_core::ChargeCommand::Begin {
                                episode: command.sequence.0,
                            },
                            DreamAction::ChargeRelease { episode } => {
                                engine_core::ChargeCommand::Release { episode }
                            }
                            DreamAction::ChargeCancel { episode } => {
                                engine_core::ChargeCommand::Cancel { episode }
                            }
                            _ => unreachable!(),
                        },
                        aim: held.aim,
                    }),
                    DreamAction::BeamStop => Some(OwnerCombatAction::BeamStop {
                        key: key(edge.slot),
                    }),
                    _ => None,
                };
            }
            beam = command.input.beam.map(|sample| (key(0), sample.aim));
            state.record_predicted_input(command.input.held);
        }
        if dependencies.targets.len() > MAX_PRESENTATION_TARGETS {
            return Err("prediction target capacity".into());
        }
        self.runner
            .lock()
            .map_err(|_| "prediction runner poisoned")?
            .step_with_targets(
                state,
                held,
                &dependencies.collision,
                &bases,
                action,
                beam,
                &dependencies.targets,
            )
            .map_err(|error| error.to_string())
    }
}
