use super::*;
use dreamwake_protocol::replication::{ReplicaPayload, decode_replica};
use dreamwake_sim::replication::{OwnerCheckpoint, OwnerExpectation};
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

fn decoded_frozen_owner(authority: &DreamAuthority) -> OwnerCheckpoint {
    let peer = &authority.peers[&41];
    let state = peer
        .frozen
        .as_ref()
        .unwrap()
        .members
        .iter()
        .find(|state| state.scope.entity == peer.welcome.owner_entity)
        .unwrap();
    let expected = OwnerExpectation {
        owner: peer.welcome.player.get(),
        match_epoch: peer.welcome.match_epoch,
        ownership_revision: peer.welcome.stream.ownership.0,
        scene_revision: u64::from(peer.welcome.content.scene_revision.0),
        minimum_revision: 0,
    };
    let ReplicaPayload::Owner(owner) = decode_replica(&state.payload, Some(expected)).unwrap()
    else {
        panic!("frozen owner member must retain a complete owner checkpoint");
    };
    assert_eq!(owner.stamp().server_tick, state.end_tick.0);
    owner
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
        authority.receive_bundle(
            41,
            live::CommandBundle {
                records: BoundedVec::new(vec![command.clone()]).unwrap(),
            },
        ),
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
fn frozen_owner_keeps_same_capture_input_continuity_while_authority_advances() {
    let (mut authority, mut server) = active_authority();
    let first = held_command(&authority, 1, 2, [1.0, 0.0]);
    authority
        .receive_bundle(
            41,
            live::CommandBundle {
                records: BoundedVec::new(vec![first]).unwrap(),
            },
        )
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
    let checkpoint = decoded_frozen_owner(&authority);
    assert_eq!(checkpoint.stamp().server_tick, 3);
    assert_eq!(checkpoint.input_continuity(), &captured_continuity);
    let frozen_members = authority.peers[&41]
        .frozen
        .as_ref()
        .unwrap()
        .members
        .clone();

    // An input snapshot for another tick must be rejected before changing the
    // prepared graph scopes, even if its held values happen to be identical.
    assert!(
        authority
            .replication
            .gather_peer_with_frozen(41, &BTreeSet::new(), ServerTick(4), &captured_continuity,)
            .is_err()
    );
    assert_eq!(
        authority.peers[&41].frozen.as_ref().unwrap().members,
        frozen_members
    );

    // Keep this immutable transfer pending as a delayed/partial send would.
    // The authority continues committing new input while these bytes wait.
    let frozen = authority
        .peers
        .get_mut(&41)
        .unwrap()
        .frozen
        .as_mut()
        .unwrap();
    frozen.sent_once = false;
    frozen.next_fragment = 0;
    let second = held_command(&authority, 2, 4, [0.0, 1.0]);
    authority
        .receive_bundle(
            41,
            live::CommandBundle {
                records: BoundedVec::new(vec![second]).unwrap(),
            },
        )
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
    assert_eq!(
        authority.peers[&41].frozen.as_ref().unwrap().members,
        frozen_members
    );
    let delayed = decoded_frozen_owner(&authority);
    assert_eq!(delayed.stamp().server_tick, 3);
    assert_eq!(delayed.input_continuity(), &captured_continuity);
    assert_ne!(delayed.input_continuity(), &current_continuity);

    // Once the old immutable transfer can coalesce, the next capture carries
    // its own matching continuity rather than the retained tick-3 value.
    let frozen = authority
        .peers
        .get_mut(&41)
        .unwrap()
        .frozen
        .as_mut()
        .unwrap();
    frozen.sent_once = true;
    frozen.next_fragment = frozen.fragments.len();
    authority.step(Duration::from_millis(85));
    authority.publish(&mut server);
    assert!(authority.peers.contains_key(&41));
    let next = decoded_frozen_owner(&authority);
    assert_eq!(next.stamp().server_tick, 5);
    assert_eq!(
        next.input_continuity(),
        &authority.peers[&41]
            .inbox
            .input_continuity()
            .map(|input| input.held.held_only())
    );
}
