use super::*;

#[derive(Debug, Clone, PartialEq)]
struct CoupledState {
    tick: u64,
    bodies: BTreeMap<u64, i64>,
    dependency: Option<EntityId>,
}
impl Payload for CoupledState {
    fn retained_bytes(&self) -> usize {
        self.bodies.len() * (ENTRY_OVERHEAD + 16)
    }
}
struct Coupled;
impl PredictionModel for Coupled {
    type Wire = Wire;
    type State = CoupledState;
    type Command = Input;
    type Dependency = Dependency;
    fn decode_group(
        &self,
        proof: &PublishedGroup<'_, Wire>,
        owner: PredictionBinding,
    ) -> Result<DecodedPrediction<CoupledState, Dependency>, PredictionError> {
        let members: Vec<_> = proof.states().collect();
        if members.iter().any(|member| {
            member.payload.state.rng == 0 || member.payload.state.tick != proof.end_tick().0
        }) {
            return Err(PredictionError::InvalidState);
        }
        let local = members
            .iter()
            .find(|member| member.scope.entity == owner.owner.owner)
            .ok_or(PredictionError::InvalidIdentity)?;
        Ok(DecodedPrediction {
            state: CoupledState {
                tick: proof.end_tick().0,
                bodies: members
                    .iter()
                    .map(|member| (member.scope.entity.index, member.payload.state.position))
                    .collect(),
                dependency: local.payload.state.dependency,
            },
            dependencies: local.payload.dependencies.clone(),
        })
    }
    fn valid_state(&self, state: &CoupledState) -> bool {
        !state.bodies.is_empty()
    }
    fn command_identity(&self, command: &Input) -> (OwnerStream, CommandSeq, TargetTick) {
        (command.owner, command.sequence, command.target)
    }
    fn valid_command(&self, command: &Input) -> bool {
        (-1..=1).contains(&command.movement)
    }
    fn required_dependencies(&self, state: &CoupledState, _: Option<&Input>) -> Vec<EntityId> {
        state.dependency.into_iter().collect()
    }
    fn step(
        &self,
        state: &mut CoupledState,
        tick: ServerTick,
        command: &Input,
        _: &DependencyFrame<Dependency>,
    ) -> Result<BTreeSet<ActionKey>, PredictionError> {
        if tick.0 != state.tick + 1 {
            return Err(PredictionError::WrongTick);
        }
        // Every writable body influences every other body in this group. This is
        // a coupled simulation fixture, not read-only dependency closure accounting.
        let sum: i64 = state.bodies.values().sum();
        for value in state.bodies.values_mut() {
            *value += sum + command.movement;
        }
        state.tick = tick.0;
        Ok(if command.fire {
            BTreeSet::from([ActionKey {
                connection: command.owner.epoch,
                stream: command.owner.stream,
                command: command.sequence,
                slot: 0,
            }])
        } else {
            BTreeSet::new()
        })
    }
}

fn publish(
    replicas: &mut Replicas,
    group: u32,
    revision: u64,
    snapshot: u64,
    tick: u64,
    members: &[(u64, i64)],
    corrupt: bool,
    missing_dependency: bool,
) {
    replicas.now += 1;
    let publication = GroupPublication {
        connection: ConnectionEpoch(1),
        group: GroupId(group),
        revision: GroupRevision(revision),
        snapshot: SnapshotId(snapshot),
    };
    let chunk = GroupChunk {
        publication,
        end_tick: ServerTick(tick),
        manifest: members.iter().map(|(id, _)| scope(*id)).collect(),
        index: 0,
        count: 1,
        members: members
            .iter()
            .map(|(id, position)| FullState {
                scope: scope(*id),
                baseline_generation: BaselineGeneration(1),
                snapshot: SnapshotId(snapshot),
                version: StateVersion(snapshot),
                end_tick: ServerTick(tick),
                payload: Wire {
                    state: State {
                        tick,
                        position: *position,
                        rng: if corrupt { 0 } else { 7 },
                        dependency: missing_dependency.then_some(entity(99)),
                        ..State::default()
                    },
                    dependencies: DependencyFrame::default(),
                },
            })
            .collect(),
    };
    assert!(matches!(
        replicas
            .groups
            .receive(chunk, replicas.now, &mut replicas.client, |_| true)
            .unwrap(),
        GroupProgress::Published(_)
    ));
}
fn initial() -> PredictionManager<Coupled> {
    let mut manager = PredictionManager::new(
        ConnectionEpoch(1),
        PredictionLimits {
            total_entities: 3,
            entities_per_group: 3,
            ..PredictionLimits::default()
        },
    )
    .unwrap();
    for (id, position) in [(1, 1), (2, 2), (3, 10)] {
        let mut replicas = Replicas::new();
        publish(
            &mut replicas,
            id,
            1,
            1,
            0,
            &[(id as u64, position)],
            false,
            false,
        );
        manager
            .initialize(&replicas.proof(id), binding(id as u64), &Coupled)
            .unwrap();
    }
    manager.begin_frame(ReplicationFrame(1)).unwrap();
    for id in 1..=3 {
        manager
            .predict(
                GroupId(id),
                input(id as u64, 1),
                DependencyFrame::default(),
                &Coupled,
            )
            .unwrap();
    }
    manager
}

#[test]
fn merge_and_split_rebase_all_writable_members_preserve_third_group_and_work() {
    let mut manager = initial();
    let third = manager.state(GroupId(3)).unwrap().clone();
    let third_command = manager
        .command_history(GroupId(3), ServerTick(1))
        .unwrap()
        .clone();
    let work = manager.work();
    let old: Vec<_> = [1, 2]
        .map(|id| manager.identity(GroupId(id)).unwrap().clone())
        .into();
    let mut merged = Replicas::new();
    publish(&mut merged, 1, 2, 2, 0, &[(1, 1), (2, 2)], false, false);
    let result = manager
        .replace_groups(&old, &[(&merged.proof(1), binding(1))], &Coupled)
        .unwrap();
    assert_eq!(result.tick, ServerTick(0));
    assert_eq!(result.events.canceled.len(), 2);
    assert!(result.events.started.is_empty());
    assert_eq!(manager.work(), work);
    assert_eq!(manager.group_count(), 2);
    assert_eq!(manager.predicted_entities(), 3);
    assert!(manager.command_history(GroupId(1), ServerTick(1)).is_none());
    assert_eq!(manager.state(GroupId(3)), Some(&third));
    assert_eq!(
        manager.command_history(GroupId(3), ServerTick(1)),
        Some(&third_command)
    );
    manager
        .predict(
            GroupId(1),
            input(1, 1),
            DependencyFrame::default(),
            &Coupled,
        )
        .unwrap();
    assert_eq!(
        manager.state(GroupId(1)).unwrap().bodies,
        BTreeMap::from([(1, 5), (2, 6)])
    );
    assert_eq!(
        manager.work().predicted_entity_steps,
        work.predicted_entity_steps + 2
    );
    let old = manager.identity(GroupId(1)).unwrap().clone();
    let mut first = Replicas::new();
    let mut second = Replicas::new();
    publish(&mut first, 1, 3, 3, 1, &[(1, 5)], false, false);
    publish(&mut second, 2, 3, 3, 1, &[(2, 6)], false, false);
    let before = manager.work();
    let result = manager
        .replace_groups(
            &[old],
            &[
                (&first.proof(1), binding(1)),
                (&second.proof(2), binding(2)),
            ],
            &Coupled,
        )
        .unwrap();
    assert!(result.events.canceled.is_empty()); // Events at K are finalized, not speculative absence.
    assert_eq!(manager.work(), before);
    for id in 1..=3 {
        manager
            .predict(
                GroupId(id),
                input(id as u64, 2),
                DependencyFrame::default(),
                &Coupled,
            )
            .unwrap();
    }
    assert_eq!(manager.state(GroupId(1)).unwrap().bodies[&1], 11);
    assert_eq!(manager.state(GroupId(2)).unwrap().bodies[&2], 13);
    assert_eq!(manager.state(GroupId(3)).unwrap().bodies[&3], 43);
    assert_eq!(manager.group_count(), 3);
}

#[test]
fn topology_failures_leave_all_previous_histories_and_counters_exact() {
    for failure in 0..9 {
        let mut manager = initial();
        let old: Vec<_> = [1, 2]
            .map(|id| manager.identity(GroupId(id)).unwrap().clone())
            .into();
        let states: Vec<_> = (1..=3)
            .map(|id| manager.state(GroupId(id)).unwrap().clone())
            .collect();
        let bytes = manager.retained_bytes();
        let work = manager.work();
        let mut a = Replicas::new();
        let mut b = Replicas::new();
        publish(
            &mut a,
            1,
            if failure == 0 { 1 } else { 2 },
            2,
            1,
            &[(1, 1)],
            false,
            false,
        );
        publish(
            &mut b,
            2,
            2,
            2,
            if failure == 1 { 0 } else { 1 },
            &[(
                if failure == 2 {
                    1
                } else if failure == 3 {
                    3
                } else {
                    2
                },
                2,
            )],
            failure == 4,
            failure == 5,
        );
        let mut retired = old.clone();
        if failure == 6 {
            retired[0].binding.teleport_segment += 1;
        }
        if failure == 7 {
            retired.pop();
        }
        let mut owner = binding(if failure == 2 {
            1
        } else if failure == 3 {
            3
        } else {
            2
        });
        if failure == 8 {
            owner.owner.stream = CommandStream(0);
        }
        assert!(
            manager
                .replace_groups(
                    &retired,
                    &[(&a.proof(1), binding(1)), (&b.proof(2), owner)],
                    &Coupled
                )
                .is_err(),
            "case {failure}"
        );
        assert_eq!(manager.retained_bytes(), bytes);
        assert_eq!(manager.work(), work);
        assert_eq!(manager.group_count(), 3);
        assert_eq!(manager.predicted_entities(), 3);
        for id in 1..=3 {
            assert_eq!(manager.state(GroupId(id)), Some(&states[id as usize - 1]));
            assert_eq!(
                manager.command_history(GroupId(id), ServerTick(1)),
                Some(&input(id as u64, 1))
            );
            assert!(
                manager
                    .dependency_history(GroupId(id), ServerTick(0))
                    .is_some()
            );
            assert_eq!(manager.recovery(GroupId(id)), None);
        }
        for identity in old {
            assert_eq!(manager.identity(identity.group), Some(&identity));
        }
    }
}

#[test]
fn existing_event_journal_survives_topology_rebase_and_does_not_recue_stable_action() {
    use crate::events::*;
    let mut manager = initial();
    let action = ActionKey {
        connection: ConnectionEpoch(1),
        stream: CommandStream(1),
        command: CommandSeq(1),
        slot: 0,
    };
    let record = EventRecord {
        identity: EventIdentity {
            key: EventKey {
                action,
                kind: EventKind(1),
                spawn_ordinal: 0,
            },
            origin_tick: ServerTick(1),
            schema: SchemaId(1),
        },
        expires_at: ServerTick(10),
        delivery: Delivery::OneShot,
        payload: Dependency(1),
    };
    let mut journal = EventJournal::new(
        JournalScope {
            connection: ConnectionEpoch(1),
            stream: CommandStream(1),
        },
        Limits::default(),
    )
    .unwrap();
    let range = ReplayRange {
        first: ServerTick(1),
        last: ServerTick(1),
    };
    let first = journal
        .reconcile(ServerTick(1), range, &[record.clone()], |_| true)
        .unwrap();
    assert!(
        first
            .as_slice()
            .iter()
            .any(|change| matches!(change, Change::Create { .. }))
    );
    let old: Vec<_> = [1, 2]
        .map(|id| manager.identity(GroupId(id)).unwrap().clone())
        .into();
    let mut merged = Replicas::new();
    publish(&mut merged, 1, 2, 2, 0, &[(1, 1), (2, 2)], false, false);
    let rebase = manager
        .replace_groups(&old, &[(&merged.proof(1), binding(1))], &Coupled)
        .unwrap();
    assert!(rebase.events.canceled.contains(&action));
    journal
        .reconcile(ServerTick(1), range, &[], |_| true)
        .unwrap();
    let predicted = manager
        .predict(
            GroupId(1),
            input(1, 1),
            DependencyFrame::default(),
            &Coupled,
        )
        .unwrap();
    assert!(predicted.started.contains(&action));
    let repeated = journal
        .reconcile(ServerTick(1), range, &[record.clone()], |_| true)
        .unwrap();
    assert!(
        !repeated
            .as_slice()
            .iter()
            .any(|change| matches!(change, Change::Create { .. }))
    );
    journal
        .resolve(
            ServerTick(1),
            record.identity,
            Outcome::Accepted { binding: None },
        )
        .unwrap();
    journal
        .reconcile(ServerTick(1), range, &[], |_| true)
        .unwrap();
    assert_eq!(
        journal.get(record.identity.key).unwrap().decision,
        Decision::Accepted
    );
}

#[test]
fn split_cannot_reuse_pre_merge_group_revision_or_owner_stream() {
    for stale_stream in [false, true] {
        let mut manager = initial();
        let old: Vec<_> = [1, 2]
            .map(|id| manager.identity(GroupId(id)).unwrap().clone())
            .into();
        let mut merged = Replicas::new();
        publish(&mut merged, 1, 2, 2, 1, &[(1, 5), (2, 6)], false, false);
        manager
            .replace_groups(&old, &[(&merged.proof(1), binding(1))], &Coupled)
            .unwrap();
        let old = manager.identity(GroupId(1)).unwrap().clone();
        let bytes = manager.retained_bytes();
        let state = manager.state(GroupId(1)).unwrap().clone();
        let mut first = Replicas::new();
        let mut second = Replicas::new();
        publish(&mut first, 1, 3, 3, 1, &[(1, 5)], false, false);
        publish(
            &mut second,
            2,
            if stale_stream { 3 } else { 1 },
            3,
            1,
            &[(2, 6)],
            false,
            false,
        );
        let mut owner = binding(2);
        if stale_stream {
            owner.owner.stream = CommandStream(1);
        }
        assert_eq!(
            manager.replace_groups(
                &[old.clone()],
                &[(&first.proof(1), binding(1)), (&second.proof(2), owner)],
                &Coupled
            ),
            Err(PredictionError::InvalidIdentity)
        );
        assert_eq!(manager.retained_bytes(), bytes);
        assert_eq!(manager.state(GroupId(1)), Some(&state));
        assert_eq!(manager.identity(GroupId(1)), Some(&old));
        assert!(manager.identity(GroupId(2)).is_none());
    }
}
