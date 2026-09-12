use super::*;
pub struct Corpus {
    pub welcome: live::Welcome,
    pub expected: OwnerExpectation,
    pub command: Vec<u8>,
    pub client_control: Vec<Vec<u8>>,
    pub server_control: Vec<Vec<u8>>,
    pub welcome_bytes: Vec<u8>,
    pub global: Vec<u8>,
    pub owner: Vec<u8>,
    pub collision: Vec<u8>,
    pub schema_seeds: Vec<(&'static str, Vec<u8>)>,
    pub schema_cases: Vec<generated::SchemaCase>,
    pub dependency_cases: Vec<generated::SchemaCase>,
    pub state: FullState<Vec<u8>>,
    pub full: Vec<u8>,
    pub live_state: Vec<u8>,
    pub group: Vec<u8>,
    pub fragments: Vec<Vec<u8>>,
    pub compressed: Vec<u8>,
    pub baseline: BaselinePacket,
    pub delta: Vec<u8>,
    pub nonfinite: Vec<Vec<u8>>,
    pub combat_cases: Vec<generated::SchemaCase>,
    pub outcome_cases: Vec<generated::SchemaCase>,
    pub finalized_ack_cases: Vec<generated::SchemaCase>,
    pub outcomes_ack_cases: Vec<generated::SchemaCase>,
}
pub fn entity(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
pub fn state(index: u64, payload: Vec<u8>, revision: u64) -> FullState<Vec<u8>> {
    FullState {
        scope: ScopeIdentity {
            connection: EPOCH,
            entity: entity(index),
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(1),
        snapshot: SnapshotId(revision),
        version: StateVersion(revision),
        end_tick: ServerTick(10),
        payload,
    }
}
impl Corpus {
    pub fn new() -> Self {
        let mut sim = dreamwake_sim::DreamSimulation::new(123, false);
        sim.continue_run();
        sim.step(DreamInput::default());
        let sample = dreamwake_sim::combat::ValidatedRay {
            command_fraction: None,
            query_fraction: 0,
            key: dreamwake_sim::combat::RayActionKey {
                match_epoch: 3,
                connection_epoch: EPOCH.0,
                command_stream: 1,
                ownership_epoch: 1,
                actor: 1,
                actor_generation: 1,
                command_sequence: 1,
                action_slot: 0,
            },
            execution_server_tick: 2,
            query_server_tick: 1,
            query_gameplay_tick: 1,
            aim: [1.0, 0.0],
        };
        let verdicts = sim
            .step_multiplayer_with_actions(
                &[],
                &[dreamwake_sim::combat::CombatAction::BeamBegin(sample)],
            )
            .unwrap();
        assert_eq!(
            verdicts[0].reason,
            dreamwake_sim::combat::ActionReason::Accepted
        );
        let capture = sim
            .capture_replication(ReplicationStamp {
                match_epoch: 3,
                server_tick: 10,
                gameplay_tick: sim.snapshot().tick,
                scene_revision: 1,
                revision: 1,
            })
            .unwrap();
        let checkpoint = capture.owner_checkpoint(1, 1).unwrap();
        let expected = checkpoint.expectation();
        let schema_seeds = generated::seeds(&capture, checkpoint.clone());
        let schema_cases = generated::cases(&schema_seeds);
        let owner = encode_replica(&ReplicaPayload::Owner(checkpoint)).unwrap();
        let collision = encode_replica(&ReplicaPayload::Collision(
            dreamwake_sim::collision::CollisionManifest::current(1),
        ))
        .unwrap();
        let global = encode_replica(&ReplicaPayload::Global(capture.global().clone())).unwrap();
        let welcome = live::Welcome {
            server_instance: 17,
            protocol: dreamwake_protocol::PROTOCOL_ID,
            client: ConnectionId(9),
            player: dreamwake_protocol::player::PlayerId::new(1).unwrap(),
            stream: OwnerStream {
                connection: ConnectionId(9),
                epoch: EPOCH,
                stream: CommandStream(1),
                owner: entity(2),
                ownership: OwnershipEpoch(1),
            },
            match_epoch: 3,
            tick: ServerTick(10),
            server_time_nanos: 100,
            content: live::ContentIdentity::current(SceneRevision(1)),
            global_entity: entity(1),
            owner_entity: entity(2),
            collision_entity: entity(3),
            application_frame_bytes: 1100,
        };
        let base_command = Command {
            owner: welcome.stream,
            sequence: CommandSeq(1),
            target: TargetTick(11),
            input: DreamInput {
                movement: [0.6, 0.8],
                aim: [1.0, 0.0],
                attack: true,
                ..Default::default()
            }
            .into(),
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        };
        let (combat_cases, outcome_cases) =
            p11::cases(&base_command, state(2, owner.clone(), 1).receipt());
        let command = live::encode_commands(
            &live::CommandBundle {
                records: BoundedVec::new(vec![base_command.clone()]).unwrap(),
            },
            EPOCH,
            1,
            limit(),
        )
        .unwrap();
        let client_control = [
            p11::lookup(),
            live::ClientControl::OutcomesAck { through: 1 },
            live::ClientControl::FinalizedAck {
                through: ServerTick(10),
            },
            live::ClientControl::Ready {
                content: welcome.content,
            },
            live::ClientControl::Decoded {
                receipts: BoundedVec::new(vec![state(1, global.clone(), 1).receipt()]).unwrap(),
            },
            live::ClientControl::Menu {
                sequence: CommandSeq(1),
                action: DreamAction::Pause { paused: true },
            },
            live::ClientControl::ClockProbe {
                client_send_nanos: 100,
            },
        ]
        .iter()
        .map(|v| live::encode_control(v, EPOCH, 1, limit()).unwrap())
        .collect();
        let server_control = [
            p11::unavailable(),
            live::ServerControl::Active {
                owner_snapshot: SnapshotId(1),
                through: ServerTick(10),
            },
            live::ServerControl::ClockReply {
                client_send_nanos: 1,
                server_receive_nanos: 2,
                server_send_nanos: 3,
                tick: ServerTick(10),
            },
            live::ServerControl::Notice {
                utf8: BoundedVec::new(b"synthetic codec corpus".to_vec()).unwrap(),
            },
        ]
        .iter()
        .map(|v| live::encode_control(v, EPOCH, 1, limit()).unwrap())
        .collect();
        let scope = state(1, global.clone(), 2);
        let full = encode_full_state(&scope, 16384).unwrap();
        let live_state =
            live::encode_state(&live::StateFrame::Full(scope.clone()), EPOCH, 1, limit()).unwrap();
        let group = encode_group(
            &GroupChunk {
                publication: GroupPublication {
                    connection: EPOCH,
                    group: live::OWNER_GROUP,
                    revision: GroupRevision(1),
                    snapshot: SnapshotId(2),
                },
                end_tick: ServerTick(10),
                manifest: vec![
                    scope.scope,
                    state(2, owner.clone(), 2).scope,
                    state(3, collision.clone(), 2).scope,
                ],
                index: 0,
                count: 1,
                members: vec![
                    scope.clone(),
                    state(2, owner.clone(), 2),
                    state(3, collision.clone(), 2),
                ],
            },
            group_limits(),
            |p| Ok(p.clone()),
        )
        .unwrap();
        let dependency_cases = generated::dependency_cases(&global, &owner, &collision);
        let publication = GroupPublication {
            connection: EPOCH,
            group: live::OWNER_GROUP,
            revision: GroupRevision(1),
            snapshot: SnapshotId(2),
        };
        let fragments = fragment_group(publication, &group, live::byte_limits(limit()).unwrap())
            .unwrap()
            .into_iter()
            .map(|v| live::encode_state(&live::StateFrame::Fragment(v), EPOCH, 1, limit()).unwrap())
            .collect();
        let baseline = BaselinePacket {
            target: BaselineReceipt {
                context: BaselineContext {
                    scope: scope.scope,
                    schema: SchemaId(1),
                    group: None,
                },
                generation: BaselineGeneration(1),
                snapshot: SnapshotId(1),
                version: StateVersion(1),
                end_tick: ServerTick(10),
            },
            encoding: BaselineEncoding::Full,
            bytes: global.clone(),
        };
        let delta = BytePatch.encode_delta(&global, &global, 16384).unwrap();
        let compressed = engine_net::encode(&BoundedVec::<u8, 16384>::new(vec![0; 8192]).unwrap());
        let mut nonfinite = Vec::new();
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let mut bad = base_command.clone();
            bad.input.held.movement[0] = invalid;
            let raw = engine_net::encode(&live::CommandBundle {
                records: BoundedVec::new(vec![bad]).unwrap(),
            });
            assert_eq!(raw[0], 0, "small synthetic input stays raw");
            nonfinite.push(
                codec::encode_frame(
                    FrameHeader {
                        lane: Lane::Input,
                        connection: EPOCH,
                        sequence: 1,
                    },
                    &raw[1..],
                    limit(),
                )
                .unwrap(),
            );
        }
        Self {
            welcome_bytes: live::encode_welcome(&welcome).unwrap(),
            welcome,
            expected,
            command,
            client_control,
            server_control,
            global,
            owner,
            collision,
            schema_seeds,
            schema_cases,
            dependency_cases,
            state: scope,
            full,
            live_state,
            group,
            fragments,
            compressed,
            baseline,
            delta,
            nonfinite,
            combat_cases,
            outcome_cases,
            finalized_ack_cases: p11::finalized_ack_cases(),
            outcomes_ack_cases: p11::outcomes_ack_cases(),
        }
    }
}
