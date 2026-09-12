use super::*;
#[path = "substitute_tests.rs"]
mod substitutes;
#[path = "topology_tests.rs"]
mod topology;
use crate::{commands::OwnerStream, replication::*, types::*};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

fn entity(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn scope(index: u64) -> ScopeIdentity {
    ScopeIdentity {
        connection: ConnectionEpoch(1),
        entity: entity(index),
        scope: ScopeEpoch(1),
        representation: RepresentationRevision(1),
    }
}
fn binding(owner: u64) -> PredictionBinding {
    PredictionBinding {
        owner: OwnerStream {
            connection: ConnectionId(7),
            epoch: ConnectionEpoch(1),
            stream: CommandStream(owner as u32),
            owner: entity(owner),
            ownership: OwnershipEpoch(1),
        },
        scene_revision: SceneRevision(1),
        teleport_segment: 0,
    }
}
#[derive(Debug, Clone, PartialEq)]
struct State {
    tick: u64,
    position: i64,
    velocity: i64,
    cooldown: u32,
    rng: u64,
    grounded: bool,
    removals: u32,
    dependency: Option<EntityId>,
    padding: Vec<u8>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            tick: 0,
            position: 0,
            velocity: 0,
            cooldown: 0,
            rng: 7,
            grounded: true,
            removals: 0,
            dependency: None,
            padding: Vec::new(),
        }
    }
}
impl Payload for State {
    fn retained_bytes(&self) -> usize {
        self.padding.capacity()
    }
}
#[derive(Debug, Clone, PartialEq)]
struct Input {
    owner: OwnerStream,
    sequence: CommandSeq,
    target: TargetTick,
    movement: i64,
    fire: bool,
}
impl Payload for Input {
    fn retained_bytes(&self) -> usize {
        0
    }
}
fn input(owner: u64, tick: u64) -> Input {
    Input {
        owner: binding(owner).owner,
        sequence: CommandSeq(tick),
        target: TargetTick(tick),
        movement: 1,
        fire: tick % 2 == 1,
    }
}
#[derive(Debug, Clone, PartialEq)]
struct Dependency(i64);
impl Payload for Dependency {
    fn retained_bytes(&self) -> usize {
        0
    }
}
#[derive(Debug, Clone, PartialEq)]
struct Wire {
    state: State,
    dependencies: DependencyFrame<Dependency>,
}
impl Payload for Wire {
    fn retained_bytes(&self) -> usize {
        self.state.retained_bytes() + dependency_bytes(&self.dependencies).unwrap()
    }
}
#[derive(Default)]
struct Model {
    calls: RefCell<Vec<ServerTick>>,
    fail_at: Option<ServerTick>,
}
impl PredictionModel for Model {
    type Wire = Wire;
    type State = State;
    type Command = Input;
    type Dependency = Dependency;
    fn decode_group(
        &self,
        group: &PublishedGroup<'_, Wire>,
        owner: PredictionBinding,
    ) -> Result<DecodedPrediction<State, Dependency>, PredictionError> {
        let state = group
            .states()
            .find(|state| state.scope.entity == owner.owner.owner)
            .ok_or(PredictionError::InvalidIdentity)?;
        if state.payload.state.tick != group.end_tick().0 {
            return Err(PredictionError::InvalidState);
        }
        Ok(DecodedPrediction {
            state: state.payload.state.clone(),
            dependencies: state.payload.dependencies.clone(),
        })
    }
    fn valid_state(&self, state: &State) -> bool {
        state.rng != 0
    }
    fn command_identity(&self, command: &Input) -> (OwnerStream, CommandSeq, TargetTick) {
        (command.owner, command.sequence, command.target)
    }
    fn valid_command(&self, command: &Input) -> bool {
        (-1..=1).contains(&command.movement)
    }
    fn required_dependencies(&self, state: &State, _: Option<&Input>) -> Vec<EntityId> {
        state.dependency.into_iter().collect()
    }
    fn step(
        &self,
        state: &mut State,
        tick: ServerTick,
        command: &Input,
        dependencies: &DependencyFrame<Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        self.advance(state, tick, Some(command), dependencies)
    }
    fn step_substitute(
        &self,
        state: &mut State,
        tick: ServerTick,
        dependencies: &DependencyFrame<Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        self.advance(state, tick, None, dependencies)
    }
}
impl Model {
    fn advance(
        &self,
        state: &mut State,
        tick: ServerTick,
        command: Option<&Input>,
        dependencies: &DependencyFrame<Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        self.calls.borrow_mut().push(tick);
        if self.fail_at == Some(tick) {
            return Err(PredictionError::InvalidState);
        }
        if state.tick + 1 != tick.0 {
            return Err(PredictionError::WrongTick);
        }
        if let Some(dependency) = state.dependency {
            match dependencies.value(dependency) {
                Some(DependencyValue::Known(value)) => state.velocity += value.0,
                Some(DependencyValue::ExplicitlyAbsent) => {
                    if state.grounded {
                        state.removals += 1;
                    }
                    state.grounded = false;
                }
                _ => {
                    return Err(PredictionError::ResyncRequired(
                        RecoveryReason::MissingDependency,
                    ));
                }
            }
        }
        state.velocity += command.map_or(0, |command| command.movement);
        state.position += state.velocity;
        state.cooldown = state.cooldown.saturating_sub(1);
        let mut events = BTreeSet::new();
        if let Some(command) =
            command.filter(|command| command.fire && state.cooldown == 0 && state.grounded)
        {
            state.rng = state.rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            state.cooldown = 2;
            events.insert(ActionKey {
                connection: command.owner.epoch,
                stream: command.owner.stream,
                command: command.sequence,
                slot: 0,
            });
        }
        state.tick = tick.0;
        Ok(events)
    }
}
fn dependency(version: u64, value: DependencyValue<Dependency>) -> DependencyFrame<Dependency> {
    DependencyFrame {
        records: BTreeMap::from([(
            entity(99),
            DependencyRecord {
                scope: scope(99),
                scene_revision: SceneRevision(1),
                version: StateVersion(version),
                value,
            },
        )]),
    }
}
struct Replicas {
    groups: GroupAssembler<Wire>,
    client: ClientScopes<Wire>,
    now: u64,
}
impl Replicas {
    fn new() -> Self {
        Self {
            groups: GroupAssembler::new(ConnectionEpoch(1), GroupLimits::default()).unwrap(),
            client: ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap(),
            now: 0,
        }
    }
    fn publish(
        &mut self,
        owner: u64,
        group: u32,
        snapshot: u64,
        state: State,
        dependencies: DependencyFrame<Dependency>,
    ) {
        self.now += 1;
        let mut manifest = vec![scope(owner)];
        for record in dependencies.records.values() {
            manifest.push(record.scope);
        }
        manifest.sort_unstable();
        manifest.dedup();
        let end_tick = ServerTick(state.tick);
        let payload = Wire {
            state,
            dependencies,
        };
        let chunk = GroupChunk {
            publication: GroupPublication {
                connection: ConnectionEpoch(1),
                group: GroupId(group),
                revision: GroupRevision(1),
                snapshot: SnapshotId(snapshot),
            },
            end_tick,
            manifest: manifest.clone(),
            index: 0,
            count: 1,
            members: manifest
                .into_iter()
                .map(|scope| FullState {
                    scope,
                    baseline_generation: BaselineGeneration(1),
                    snapshot: SnapshotId(snapshot),
                    version: StateVersion(snapshot),
                    end_tick,
                    payload: payload.clone(),
                })
                .collect(),
        };
        assert!(matches!(
            self.groups
                .receive(chunk, self.now, &mut self.client, |_| true)
                .unwrap(),
            GroupProgress::Published(_)
        ));
    }
    fn proof(&self, group: u32) -> PublishedGroup<'_, Wire> {
        self.groups
            .published_group(GroupId(group), &self.client)
            .unwrap()
    }
}
fn initialized(
    limits: PredictionLimits,
    state: State,
    dependencies: DependencyFrame<Dependency>,
) -> (PredictionManager<Model>, Replicas, Model) {
    let model = Model::default();
    let mut replicas = Replicas::new();
    replicas.publish(1, 1, 1, state, dependencies);
    let mut manager = PredictionManager::new(ConnectionEpoch(1), limits).unwrap();
    manager
        .initialize(&replicas.proof(1), binding(1), &model)
        .unwrap();
    manager.begin_frame(ReplicationFrame(1)).unwrap();
    (manager, replicas, model)
}
fn predict(
    manager: &mut PredictionManager<Model>,
    through: u64,
    deps: &DependencyFrame<Dependency>,
    model: &Model,
) {
    let start = manager.predicted_tick(GroupId(1)).unwrap().0 + 1;
    for tick in start..=through {
        manager
            .predict(GroupId(1), input(1, tick), deps.clone(), model)
            .unwrap();
    }
}

#[test]
fn complete_group_proof_requires_all_current_decoded_members() {
    let mut replicas = Replicas::new();
    let payload = Wire {
        state: State::default(),
        dependencies: DependencyFrame::default(),
    };
    let manifest = vec![scope(1), scope(2)];
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: GroupId(1),
        revision: GroupRevision(1),
        snapshot: SnapshotId(1),
    };
    let chunk = |index| GroupChunk {
        publication,
        end_tick: ServerTick(0),
        manifest: manifest.clone(),
        index,
        count: 2,
        members: vec![FullState {
            scope: manifest[index],
            baseline_generation: BaselineGeneration(1),
            snapshot: SnapshotId(1),
            version: StateVersion(1),
            end_tick: ServerTick(0),
            payload: payload.clone(),
        }],
    };
    assert_eq!(
        replicas
            .groups
            .receive(chunk(0), 0, &mut replicas.client, |_| true)
            .unwrap(),
        GroupProgress::Staged
    );
    assert!(
        replicas
            .groups
            .published_group(GroupId(1), &replicas.client)
            .is_none()
    );
    replicas
        .groups
        .receive(chunk(1), 1, &mut replicas.client, |_| true)
        .unwrap();
    assert_eq!(replicas.proof(1).states().len(), 2);
    replicas
        .client
        .apply_exit(ScopeExit { scope: scope(2) })
        .unwrap();
    assert!(
        replicas
            .groups
            .published_group(GroupId(1), &replicas.client)
            .is_none()
    );
}

#[test]
fn correction_compares_complete_state_and_replays_k_plus_one_only() {
    for field in 0..4 {
        let (mut manager, mut replicas, model) = initialized(
            PredictionLimits::default(),
            State::default(),
            DependencyFrame::default(),
        );
        predict(&mut manager, 3, &DependencyFrame::default(), &model);
        let mut authority = State::default();
        match field {
            0 => authority.velocity = 5,
            1 => authority.cooldown = 10,
            2 => authority.rng = 12,
            _ => authority.grounded = false,
        }
        let mut expected = authority.clone();
        for tick in 1..=3 {
            model
                .step(
                    &mut expected,
                    ServerTick(tick),
                    &input(1, tick),
                    &DependencyFrame::default(),
                )
                .unwrap();
        }
        replicas.publish(1, 1, 2, authority, DependencyFrame::default());
        model.calls.borrow_mut().clear();
        let result = manager
            .reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model)
            .unwrap();
        assert!(matches!(
            result,
            Correction::Replayed {
                from: ServerTick(1),
                through: ServerTick(3),
                ..
            }
        ));
        assert_eq!(
            *model.calls.borrow(),
            vec![ServerTick(1), ServerTick(2), ServerTick(3)]
        );
        assert_eq!(manager.state(GroupId(1)), Some(&expected));
    }
}

#[test]
fn equal_checkpoint_fast_path_retains_future_inputs_and_rejects_duplicate_corrections() {
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    predict(&mut manager, 4, &DependencyFrame::default(), &model);
    let authority = manager
        .history_state(GroupId(1), ServerTick(2))
        .unwrap()
        .clone();
    let expected = manager.state(GroupId(1)).unwrap().clone();
    replicas.publish(1, 1, 2, authority, DependencyFrame::default());
    model.calls.borrow_mut().clear();
    assert_eq!(
        manager
            .reconcile(&replicas.proof(1), binding(1), ServerTick(4), &model)
            .unwrap(),
        Correction::Equal
    );
    assert!(model.calls.borrow().is_empty());
    assert_eq!(manager.state(GroupId(1)), Some(&expected));
    assert!(manager.history_state(GroupId(1), ServerTick(1)).is_none());
    assert!(manager.history_state(GroupId(1), ServerTick(3)).is_some());
    assert_eq!(
        manager
            .reconcile(&replicas.proof(1), binding(1), ServerTick(4), &model)
            .unwrap(),
        Correction::Stale
    );
}

#[test]
fn replay_failure_or_missing_history_never_publishes_partial_world() {
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    predict(&mut manager, 4, &DependencyFrame::default(), &model);
    let expected = manager.state(GroupId(1)).unwrap().clone();
    replicas.publish(
        1,
        1,
        2,
        State {
            velocity: 5,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    let failing = Model {
        fail_at: Some(ServerTick(3)),
        ..Default::default()
    };
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(0), &failing),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::InvalidState
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&expected));
    assert_eq!(
        *failing.calls.borrow(),
        vec![ServerTick(1), ServerTick(2), ServerTick(3)]
    );
    assert_eq!(manager.work().replay_ticks, 4);
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits {
            history_ticks: 2,
            ..Default::default()
        },
        State::default(),
        DependencyFrame::default(),
    );
    predict(&mut manager, 4, &DependencyFrame::default(), &model);
    let expected = manager.state(GroupId(1)).unwrap().clone();
    replicas.publish(
        1,
        1,
        2,
        State {
            tick: 1,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    assert_eq!(
        manager.reconcile(&replicas.proof(1), binding(1), ServerTick(1), &model),
        Err(PredictionError::ResyncRequired(
            RecoveryReason::MissingHistory
        ))
    );
    assert_eq!(manager.state(GroupId(1)), Some(&expected));
}

#[test]
fn ahead_and_binding_changes_require_explicit_reinitialization() {
    for identity_change in [false, true] {
        let (mut manager, mut replicas, model) = initialized(
            PredictionLimits::default(),
            State::default(),
            DependencyFrame::default(),
        );
        let mut changed = binding(1);
        let tick = if identity_change {
            changed.teleport_segment = 1;
            0
        } else {
            1
        };
        replicas.publish(
            1,
            1,
            2,
            State {
                tick,
                ..Default::default()
            },
            DependencyFrame::default(),
        );
        assert_eq!(
            manager.reconcile(&replicas.proof(1), changed, ServerTick(tick), &model),
            Err(PredictionError::ResyncRequired(if identity_change {
                RecoveryReason::IdentityChanged
            } else {
                RecoveryReason::CheckpointAhead
            }))
        );
        assert_eq!(manager.predicted_tick(GroupId(1)), Some(ServerTick(0)));
        assert!(
            manager
                .predict(GroupId(1), input(1, 1), DependencyFrame::default(), &model)
                .is_err()
        );
    }
}

#[test]
fn checkpoint_dependencies_refresh_new_commands_without_rewriting_replay_history() {
    let initial = State {
        dependency: Some(entity(99)),
        ..Default::default()
    };
    let deps = dependency(1, DependencyValue::Known(Dependency(0)));
    let (mut manager, mut replicas, model) =
        initialized(PredictionLimits::default(), initial, deps.clone());
    assert!(manager.checkpoint_dependencies(GroupId(2)).is_none());
    predict(&mut manager, 4, &deps, &model);
    for checkpoint in 1..=320 {
        manager
            .begin_frame(ReplicationFrame(checkpoint + 1))
            .unwrap();
        let frontier = manager.predicted_tick(GroupId(1)).unwrap().0;
        assert_eq!(frontier - checkpoint, 3);
        let retained: Vec<_> = (checkpoint + 1..=frontier)
            .map(|tick| {
                (
                    ServerTick(tick),
                    manager
                        .dependency_history(GroupId(1), ServerTick(tick))
                        .unwrap()
                        .clone(),
                )
            })
            .collect();
        let authority = manager
            .history_state(GroupId(1), ServerTick(checkpoint))
            .unwrap()
            .clone();
        let fresh = dependency(checkpoint + 1, DependencyValue::Known(Dependency(0)));
        replicas.publish(1, 1, checkpoint + 1, authority, fresh.clone());
        assert!(matches!(
            manager
                .reconcile(
                    &replicas.proof(1),
                    binding(1),
                    ServerTick(checkpoint),
                    &model,
                )
                .unwrap(),
            Correction::Replayed { .. }
        ));
        assert_eq!(manager.checkpoint_dependencies(GroupId(1)), Some(&fresh));
        for (tick, original) in retained {
            assert_eq!(
                manager.dependency_history(GroupId(1), tick),
                Some(&original)
            );
        }
        let next_dependencies = manager.checkpoint_dependencies(GroupId(1)).unwrap().clone();
        predict(&mut manager, frontier + 1, &next_dependencies, &model);
        assert_eq!(
            manager.dependency_history(GroupId(1), ServerTick(frontier + 1)),
            Some(&fresh)
        );
        assert!(manager.recovery(GroupId(1)).is_none());
    }
}

#[test]
fn dependency_only_correction_forces_replay_despite_equal_owner_checkpoint() {
    let initial = State {
        dependency: Some(entity(99)),
        ..Default::default()
    };
    let deps = dependency(1, DependencyValue::Known(Dependency(1)));
    let (mut manager, mut replicas, model) =
        initialized(PredictionLimits::default(), initial.clone(), deps.clone());
    predict(&mut manager, 3, &deps, &model);
    let previous = manager.state(GroupId(1)).unwrap().position;
    manager
        .update_dependencies(
            GroupId(1),
            ServerTick(2),
            dependency(2, DependencyValue::Known(Dependency(10))),
        )
        .unwrap();
    replicas.publish(1, 1, 2, initial, deps);
    model.calls.borrow_mut().clear();
    assert!(matches!(
        manager
            .reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model)
            .unwrap(),
        Correction::Replayed { .. }
    ));
    assert!(manager.state(GroupId(1)).unwrap().position > previous);
    assert_eq!(
        *model.calls.borrow(),
        vec![ServerTick(1), ServerTick(2), ServerTick(3)]
    );
}

#[test]
fn unavailable_is_not_absence_and_explicit_removal_is_replayed_once() {
    for unavailable in [false, true] {
        let initial = State {
            dependency: Some(entity(99)),
            ..Default::default()
        };
        let deps = dependency(1, DependencyValue::Known(Dependency(1)));
        let (mut manager, mut replicas, model) =
            initialized(PredictionLimits::default(), initial.clone(), deps.clone());
        predict(&mut manager, 3, &deps, &model);
        let expected = manager.state(GroupId(1)).unwrap().clone();
        let changed = if unavailable {
            DependencyFrame::default()
        } else {
            dependency(2, DependencyValue::ExplicitlyAbsent)
        };
        manager
            .update_dependencies(GroupId(1), ServerTick(2), changed.clone())
            .unwrap();
        manager
            .update_dependencies(GroupId(1), ServerTick(3), changed)
            .unwrap();
        replicas.publish(1, 1, 2, initial, deps);
        let result = manager.reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model);
        if unavailable {
            assert_eq!(
                result,
                Err(PredictionError::ResyncRequired(
                    RecoveryReason::MissingDependency
                ))
            );
            assert_eq!(manager.state(GroupId(1)), Some(&expected));
        } else {
            assert!(result.is_ok());
            let state = manager.state(GroupId(1)).unwrap();
            assert!(!state.grounded);
            assert_eq!(state.removals, 1);
        }
    }
}

#[test]
fn historical_dependency_samples_outlive_visible_scope_exit_but_revocation_blocks_prediction() {
    let deps = dependency(1, DependencyValue::Known(Dependency(1)));
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State {
            dependency: Some(entity(99)),
            ..Default::default()
        },
        deps.clone(),
    );
    predict(&mut manager, 2, &deps, &model);
    let bytes = manager.retained_bytes();
    replicas
        .client
        .apply_exit(ScopeExit { scope: scope(99) })
        .unwrap();
    assert!(replicas.client.state(entity(99)).is_none());
    assert_eq!(
        manager
            .dependency_history(GroupId(1), ServerTick(1))
            .unwrap()
            .value(entity(99)),
        Some(&DependencyValue::Known(Dependency(1)))
    );
    assert_eq!(manager.retained_bytes(), bytes);
    manager.revoke_scope(scope(99));
    assert_eq!(
        manager.recovery(GroupId(1)),
        Some(RecoveryReason::DisclosureRevoked)
    );
    assert!(
        manager
            .predict(GroupId(1), input(1, 3), deps, &model)
            .is_err()
    );
    assert!(
        manager
            .dependency_history(GroupId(1), ServerTick(1))
            .is_some()
    );
}

#[test]
fn corrected_action_events_cancel_once_without_restarting_unchanged_events() {
    let (mut manager, mut replicas, model) = initialized(
        PredictionLimits::default(),
        State::default(),
        DependencyFrame::default(),
    );
    let first = manager
        .predict(GroupId(1), input(1, 1), DependencyFrame::default(), &model)
        .unwrap();
    assert_eq!(first.started.len(), 1);
    assert!(
        manager
            .predict(GroupId(1), input(1, 1), DependencyFrame::default(), &model)
            .unwrap()
            .started
            .is_empty()
    );
    predict(&mut manager, 3, &DependencyFrame::default(), &model);
    replicas.publish(
        1,
        1,
        2,
        State {
            cooldown: 99,
            ..Default::default()
        },
        DependencyFrame::default(),
    );
    let Correction::Replayed { events, .. } = manager
        .reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model)
        .unwrap()
    else {
        panic!("changed cooldown requires replay")
    };
    assert_eq!(events.canceled.len(), 2);
    assert!(events.started.is_empty());
    assert_eq!(
        manager
            .reconcile(&replicas.proof(1), binding(1), ServerTick(0), &model)
            .unwrap(),
        Correction::Stale
    );
}

#[test]
fn aggregate_frame_replay_budget_cannot_be_reset_per_group() {
    let model = Model::default();
    let mut replicas = Replicas::new();
    let mut manager = PredictionManager::new(
        ConnectionEpoch(1),
        PredictionLimits {
            replay_ticks_per_frame: 2,
            ..Default::default()
        },
    )
    .unwrap();
    for owner in 1..=2 {
        replicas.publish(
            owner,
            owner as u32,
            1,
            State::default(),
            DependencyFrame::default(),
        );
        manager
            .initialize(&replicas.proof(owner as u32), binding(owner), &model)
            .unwrap();
    }
    manager.begin_frame(ReplicationFrame(1)).unwrap();
    for tick in 1..=2 {
        for owner in 1..=2 {
            manager
                .predict(
                    GroupId(owner as u32),
                    input(owner, tick),
                    DependencyFrame::default(),
                    &model,
                )
                .unwrap();
        }
    }
    let second = manager.state(GroupId(2)).unwrap().clone();
    for owner in 1..=2 {
        replicas.publish(
            owner,
            owner as u32,
            2,
            State {
                velocity: 1,
                ..Default::default()
            },
            DependencyFrame::default(),
        );
        let result = manager.reconcile(
            &replicas.proof(owner as u32),
            binding(owner),
            ServerTick(0),
            &model,
        );
        if owner == 1 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result,
                Err(PredictionError::ResyncRequired(
                    RecoveryReason::ReplayBudget
                ))
            );
        }
    }
    assert_eq!(manager.state(GroupId(2)), Some(&second));
    assert_eq!(manager.work().replay_ticks, 2);
    assert_eq!(
        manager.begin_frame(ReplicationFrame(1)),
        Err(PredictionError::NonMonotonicFrame)
    );
}

#[test]
fn memory_pressure_requires_recovery_without_partial_tick_or_unbounded_history() {
    let limits = PredictionLimits {
        history_bytes: 4096,
        checkpoint_bytes: 128,
        dependency_bytes: 128,
        command_bytes: 128,
        events_per_tick: 1,
        ..Default::default()
    };
    let (mut manager, _replicas, model) =
        initialized(limits, State::default(), DependencyFrame::default());
    let mut failed = false;
    for tick in 1..=20 {
        let before = manager.state(GroupId(1)).unwrap().clone();
        if let Err(error) = manager.predict(
            GroupId(1),
            input(1, tick),
            DependencyFrame::default(),
            &model,
        ) {
            assert_eq!(
                error,
                PredictionError::ResyncRequired(RecoveryReason::MemoryBudget)
            );
            assert_eq!(manager.state(GroupId(1)), Some(&before));
            failed = true;
            break;
        }
        assert!(manager.retained_bytes() <= limits.history_bytes);
    }
    assert!(failed);
}

#[test]
fn restore_at_every_tick_converges_with_complete_latent_state() {
    let model = Model::default();
    let mut authority = State::default();
    let mut checkpoints = vec![authority.clone()];
    for tick in 1..=8 {
        model
            .step(
                &mut authority,
                ServerTick(tick),
                &input(1, tick),
                &DependencyFrame::default(),
            )
            .unwrap();
        checkpoints.push(authority.clone());
    }
    for (tick, checkpoint) in checkpoints.into_iter().enumerate() {
        let (mut manager, mut replicas, local) = initialized(
            PredictionLimits::default(),
            State {
                velocity: 2,
                ..Default::default()
            },
            DependencyFrame::default(),
        );
        predict(&mut manager, 8, &DependencyFrame::default(), &local);
        replicas.publish(1, 1, 2, checkpoint, DependencyFrame::default());
        manager
            .reconcile(
                &replicas.proof(1),
                binding(1),
                ServerTick(tick as u64),
                &local,
            )
            .unwrap();
        assert_eq!(
            manager.state(GroupId(1)),
            Some(&authority),
            "checkpoint {tick}"
        );
    }
}

#[test]
fn remaining_prediction_ticks_accounts_for_members_and_shared_work() {
    let deps = dependency(1, DependencyValue::Known(Dependency(1)));
    let limits = PredictionLimits {
        prediction_ticks_per_frame: 32,
        prediction_entity_steps_per_frame: 5,
        ..Default::default()
    };
    let (mut manager, _, model) = initialized(
        limits,
        State {
            dependency: Some(entity(99)),
            ..Default::default()
        },
        deps.clone(),
    );
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(2));
    assert_eq!(manager.remaining_prediction_ticks(GroupId(99)), None);
    predict(&mut manager, 1, &deps, &model);
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(1));
    predict(&mut manager, 2, &deps, &model);
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(0));
    manager.begin_frame(ReplicationFrame(2)).unwrap();
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(2));
    let (mut manager, _, model) = initialized(
        PredictionLimits {
            prediction_ticks_per_frame: 1,
            prediction_entity_steps_per_frame: 100,
            ..Default::default()
        },
        State::default(),
        DependencyFrame::default(),
    );
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(1));
    predict(&mut manager, 1, &DependencyFrame::default(), &model);
    assert_eq!(manager.remaining_prediction_ticks(GroupId(1)), Some(0));
}
