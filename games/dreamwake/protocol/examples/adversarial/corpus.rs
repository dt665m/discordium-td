use super::*;
pub struct Corpus {
    pub welcome: live::Welcome,
    pub expected: OwnerExpectation,
    pub input: Vec<u8>,
    pub client_control: Vec<Vec<u8>>,
    pub server_control: Vec<Vec<u8>>,
    pub welcome_bytes: Vec<u8>,
    /// Whole live states, by root name.
    pub live_states: Vec<(&'static str, Vec<u8>)>,
    pub owner_state: Vec<u8>,
    pub delta_base: ReplicaState,
    pub delta: Vec<u8>,
    pub state_packet: Vec<u8>,
    pub compressed: Vec<u8>,
    pub nonfinite: Vec<Vec<u8>>,
    /// Canonical (offline) schema encodings, by root name.
    pub schema_seeds: Vec<(&'static str, Vec<u8>)>,
    pub schema_cases: Vec<generated::SchemaCase>,
    pub combat_cases: Vec<generated::SchemaCase>,
    pub outcome_cases: Vec<generated::SchemaCase>,
    pub outcomes_ack_cases: Vec<generated::SchemaCase>,
}
pub fn entity(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn live(replica: &Replica) -> Vec<u8> {
    let mut bytes = Vec::new();
    ReplicaState::capture(replica)
        .unwrap()
        .encode(None, &mut bytes)
        .unwrap();
    bytes
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
        let actors = generated::actors(&capture);
        let schema_seeds = generated::seeds(&capture, checkpoint.clone(), &actors);
        let schema_cases = generated::cases(&schema_seeds);
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
            application_frame_bytes: 1100,
        };
        let base_record = live::InputRecord {
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
        let (combat_cases, outcome_cases) = p11::cases(&base_record);
        let input = live::encode_input(
            &live::InputPacket {
                ack: StreamAck {
                    latest: 9,
                    received: 0b1011,
                },
                records: BoundedVec::new(vec![base_record.clone()]).unwrap(),
            },
            EPOCH,
            1,
            limit(),
        )
        .unwrap();
        let client_control = [
            p11::lookup(),
            live::ClientControl::OutcomesAck { through: 1 },
            live::ClientControl::Ready {
                content: welcome.content,
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
        let global = Replica::Global(capture.global().clone());
        let owner = Replica::Owner(checkpoint);
        let mut live_states = vec![("global", live(&global))];
        live_states.extend(
            actors
                .iter()
                .map(|(name, actor)| (*name, live(&Replica::Actor(actor.clone())))),
        );
        let owner_state = live(&owner);
        // A moving enemy and its change against an earlier position.
        let (_, moving) = actors.iter().find(|(name, _)| *name == "enemy").unwrap();
        let PublicReplica::Enemy(mut earlier) = moving.clone() else {
            unreachable!()
        };
        earlier.position[0] -= 0.5;
        let delta_base =
            ReplicaState::capture(&Replica::Actor(PublicReplica::Enemy(earlier))).unwrap();
        let mut delta = Vec::new();
        ReplicaState::capture(&Replica::Actor(moving.clone()))
            .unwrap()
            .encode(Some(&delta_base), &mut delta)
            .unwrap();
        let state_packet = live::encode_state(
            &live::StatePacket {
                seq: 5,
                tick: ServerTick(10),
                status: live::OwnerStatus {
                    finalized: ServerTick(9),
                    menu_applied: Some(CommandSeq(1)),
                    arrival: Default::default(),
                },
                entries: vec![
                    live::StateEntry {
                        entity: entity(1),
                        base: None,
                        body: live(&global),
                    },
                    live::StateEntry {
                        entity: entity(7),
                        base: Some(3),
                        body: delta.clone(),
                    },
                ],
                removed: vec![entity(8)],
            },
            EPOCH,
            state_limit(),
        )
        .unwrap();
        let compressed = engine_net::encode(&BoundedVec::<u8, 16384>::new(vec![0; 8192]).unwrap());
        let mut nonfinite = Vec::new();
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let mut bad = base_record.clone();
            bad.input.held.movement[0] = invalid;
            nonfinite.push(p11::framed(
                &live::InputPacket {
                    ack: StreamAck::default(),
                    records: BoundedVec::new(vec![bad]).unwrap(),
                },
                Lane::Input,
            ));
        }
        Self {
            welcome_bytes: live::encode_welcome(&welcome).unwrap(),
            welcome,
            expected,
            input,
            client_control,
            server_control,
            live_states,
            owner_state,
            delta_base,
            delta,
            state_packet,
            compressed,
            nonfinite,
            schema_seeds,
            schema_cases,
            combat_cases,
            outcome_cases,
            outcomes_ack_cases: p11::outcomes_ack_cases(),
        }
    }
}
