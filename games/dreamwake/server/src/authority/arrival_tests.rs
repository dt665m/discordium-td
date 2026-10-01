use super::*;
use dreamwake_sim::replication::{OwnerCheckpoint, OwnerExpectation, Replica, ReplicaState};
use engine_net::commands::{ActionEdge, Command};
use engine_server::Authority;

fn active_authority() -> (DreamAuthority, RenetServer) {
    let mut authority = DreamAuthority::new(71, false, 1);
    assert!(authority.admit(41));
    let mut server = RenetServer::new(connection_config());
    server.add_connection(41);
    let content = authority.peers[&41].welcome.content;
    authority
        .receive_control(41, live::ClientControl::Ready { content }, &mut server)
        .unwrap();
    let peer = authority.peers.get_mut(&41).unwrap();
    peer.welcome_pending = false;
    peer.active = true;
    peer.activate_pending = true;
    authority.step(Duration::from_millis(17));
    assert_eq!(authority.tick, ServerTick(1));
    assert!(authority.failed.is_empty());
    (authority, server)
}

fn held_command(
    authority: &DreamAuthority,
    sequence: u64,
    tick: u64,
    movement: [f32; 2],
) -> Command<live::TickInput, DreamAction> {
    Command {
        owner: authority.peers[&41].welcome.stream,
        sequence: CommandSeq(sequence),
        target: TargetTick(tick),
        input: DreamInput {
            movement,
            aim: [0.0, -1.0],
            attack: true,
            ..Default::default()
        }
        .into(),
        actions: BoundedVec::default(),
    }
}

/// Every owner checkpoint in the state packets the server has sent, oldest
/// first. Nothing is acknowledged, so every owner entry is a whole state.
fn decoded_queued_owners(
    authority: &DreamAuthority,
    server: &mut RenetServer,
    client: &mut renet::RenetClient,
) -> Vec<OwnerCheckpoint> {
    let peer = &authority.peers[&41];
    for packet in server.get_packets_to_send(41).unwrap() {
        client.process_packet(&packet);
    }
    let expected = OwnerExpectation {
        owner: peer.welcome.player.get(),
        match_epoch: peer.welcome.match_epoch,
        ownership_revision: peer.welcome.stream.ownership.0,
        scene_revision: u64::from(peer.welcome.content.scene_revision.0),
        minimum_revision: 0,
    };
    let limit = engine_net::codec::FrameLimit::new(live::MAX_STATE_FRAME_BYTES).unwrap();
    let mut owners = Vec::new();
    while let Some(message) = client.receive_message(wire::STATE_CHANNEL) {
        let packet = live::decode_state(&message, peer.welcome.stream.epoch, limit).unwrap();
        for entry in packet.entries {
            if entry.entity != peer.welcome.owner_entity {
                continue;
            }
            assert_eq!(entry.base, None, "unacknowledged owner state is sent whole");
            let Replica::Owner(owner) = ReplicaState::decode(&entry.body, None)
                .unwrap()
                .restore(Some(expected))
                .unwrap()
            else {
                panic!("owner entity carries the owner checkpoint");
            };
            assert_eq!(owner.stamp().server_tick, packet.tick.0);
            owners.push(owner);
        }
    }
    owners
}

#[test]
fn prepared_receive_guard_rejects_before_admission_feedback_or_action_reservation() {
    let (mut authority, _) = active_authority();
    let mut command = held_command(&authority, 1, 2, [1.0, 0.0]);
    command
        .actions
        .push(ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        })
        .unwrap();
    let peer = authority.peers.get_mut(&41).unwrap();
    let continuity = peer.inbox.input_continuity().clone();
    let arrival = peer.arrival;
    let prepared = peer.inbox.prepare_next(&Rules(command.owner)).unwrap();
    assert_eq!(prepared.tick(), ServerTick(2));
    assert_eq!(peer.inbox.prepared_tick(), Some(ServerTick(2)));
    assert_eq!(peer.inbox.finalized_through(), ServerTick(1));
    let normal_outcomes = authority.outcomes.normal.len();
    let overload_outcomes = authority.outcomes.overload.len();

    assert_eq!(
        authority.receive_commands(41, vec![command.clone()]),
        Err("Input received during prepared simulation tick".into())
    );
    let peer = &authority.peers[&41];
    assert_eq!(authority.tick, ServerTick(1));
    assert_eq!(peer.inbox.finalized_through(), ServerTick(1));
    assert_eq!(peer.inbox.prepared_tick(), Some(ServerTick(2)));
    assert_eq!(peer.inbox.input_continuity(), &continuity);
    assert_eq!(peer.inbox.pending_len(), 0);
    assert_eq!(peer.inbox.retained_admission(command.sequence), None);
    assert_eq!(peer.arrival, arrival);
    assert_eq!(authority.outcomes.normal.len(), normal_outcomes);
    assert_eq!(authority.outcomes.overload.len(), overload_outcomes);
}

#[test]
fn queued_owner_keeps_same_capture_input_continuity_while_authority_advances() {
    let (mut authority, mut server) = active_authority();
    let mut client = renet::RenetClient::new(wire::connection_config());
    client.set_connected();
    let first = held_command(&authority, 1, 2, [1.0, 0.0]);
    authority
        .receive_commands(41, vec![first])
        .unwrap();
    authority.step(Duration::from_millis(34));
    authority.step(Duration::from_millis(51));
    let captured_continuity = authority.peers[&41]
        .inbox
        .input_continuity()
        .map(|input| input.held.held_only());
    assert_eq!(captured_continuity.missing_streak, 1);
    assert_eq!(
        captured_continuity.last_input.as_ref().unwrap().movement,
        [1.0, 0.0]
    );
    authority.publish(&mut server);
    assert!(authority.peers.contains_key(&41));
    // An input snapshot for another tick is rejected before projecting.
    assert!(
        authority
            .replication
            .gather_peer(41, ServerTick(4), &captured_continuity)
            .is_err()
    );

    // Leave the packet undelivered while authority advances.
    let second = held_command(&authority, 2, 4, [0.0, 1.0]);
    authority
        .receive_commands(41, vec![second])
        .unwrap();
    authority.step(Duration::from_millis(68));
    let current_continuity = authority.peers[&41]
        .inbox
        .input_continuity()
        .map(|input| input.held.held_only());
    assert_eq!(current_continuity.missing_streak, 0);
    assert_ne!(current_continuity, captured_continuity);
    authority.publish(&mut server);
    assert!(authority.peers.contains_key(&41));
    let queued = decoded_queued_owners(&authority, &mut server, &mut client);
    assert_eq!(queued.len(), 2);
    let delayed = &queued[0];
    assert_eq!(delayed.stamp().server_tick, 3);
    assert_eq!(delayed.input_continuity(), &captured_continuity);
    assert_ne!(delayed.input_continuity(), &current_continuity);
    assert_eq!(queued[1].stamp().server_tick, 4);
    assert_eq!(queued[1].input_continuity(), &current_continuity);

    // A newer capture carries its own matching continuity.
    authority.step(Duration::from_millis(85));
    authority.publish(&mut server);
    assert!(authority.peers.contains_key(&41));
    let next = decoded_queued_owners(&authority, &mut server, &mut client)
        .pop()
        .unwrap();
    assert_eq!(next.stamp().server_tick, 5);
    assert_eq!(
        next.input_continuity(),
        &authority.peers[&41]
            .inbox
            .input_continuity()
            .map(|input| input.held.held_only())
    );
}
