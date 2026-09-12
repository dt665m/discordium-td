use super::*;
use engine_net::commands::{ActionEdge, Command};
fn command(
    authority: &DreamAuthority,
    seq: u64,
    tick: u64,
) -> Command<live::TickInput, DreamAction> {
    Command {
        owner: authority.peers[&41].welcome.stream,
        sequence: CommandSeq(seq),
        target: TargetTick(tick),
        input: Default::default(),
        actions: BoundedVec::new(vec![ActionEdge {
            slot: 0,
            action: DreamAction::Dash {
                direction: [1.0, 0.0],
            },
        }])
        .unwrap(),
    }
}
fn submit(authority: &mut DreamAuthority, command: Command<live::TickInput, DreamAction>) {
    authority
        .receive_bundle(
            41,
            live::CommandBundle {
                records: BoundedVec::new(vec![command]).unwrap(),
            },
        )
        .unwrap();
}
#[test]
fn terminal_duplicate_late_and_reconnect_lookup_do_not_reexecute() {
    let mut authority = DreamAuthority::new(17, false, 8);
    assert!(authority.admit(41));
    authority.peers.get_mut(&41).unwrap().active = true;
    authority.step(Duration::from_millis(17));
    let owner = authority.peers[&41].welcome.player.get();
    authority.sim.set_player_active(owner, true);
    authority.begin_run();
    let edge = command(&authority, 1, 2);
    let key = edge.action_key(0);
    submit(&mut authority, edge.clone());
    assert!(
        authority.outcomes.lookup(owner, key).is_none(),
        "admission is not execution"
    );
    authority.step(Duration::from_millis(34));
    let terminal = authority.outcomes.lookup(owner, key).unwrap();
    assert_eq!(terminal.data.status, live::TerminalStatus::Accepted);
    let snapshot = authority.sim.snapshot_for(owner);
    submit(&mut authority, edge);
    assert_eq!(
        authority.outcomes.lookup(owner, key),
        Some(terminal.clone())
    );
    assert_eq!(authority.sim.snapshot_for(owner), snapshot);
    let late = command(&authority, 2, 1);
    let late_key = late.action_key(0);
    submit(&mut authority, late);
    assert_eq!(
        authority
            .outcomes
            .lookup(owner, late_key)
            .unwrap()
            .data
            .status,
        live::TerminalStatus::Expired
    );
    authority.disconnect(41);
    assert_eq!(authority.outcomes.lookup(owner, key), Some(terminal));
    assert!(authority.outcomes.lookup(owner + 1, key).is_none());
}
#[test]
fn capacity_refusal_has_terminal_record_in_reserved_overload_pool() {
    use engine_net::outcomes::{OutcomeLedger, OutcomeLimits};
    let mut authority = DreamAuthority::new(17, false, 8);
    assert!(authority.admit(41));
    authority.peers.get_mut(&41).unwrap().active = true;
    authority.outcomes.normal = OutcomeLedger::new(OutcomeLimits {
        records: 1,
        records_per_owner: 1,
        retained_bytes: 4096,
        maximum_payload_bytes: 256,
        retention_ticks: 1800,
    })
    .unwrap();
    let first = command(&authority, 1, 1);
    submit(&mut authority, first);
    let second = command(&authority, 2, 2);
    let key = second.action_key(0);
    let owner = authority.peers[&41].welcome.player.get();
    submit(&mut authority, second);
    authority.step(Duration::from_millis(17));
    authority.step(Duration::from_millis(34));
    let refusal = authority.outcomes.lookup(owner, key).unwrap();
    assert_eq!(refusal.data.status, live::TerminalStatus::Rejected);
    assert_eq!(refusal.data.reason, live::OutcomeReason::Capacity);
}

#[test]
fn combat_arrival_uses_committed_time_under_authority_debt() {
    use engine_net::replication::StateReceipt;

    let mut authority = DreamAuthority::new(17, false, 8);
    assert!(authority.admit(41));
    authority.peers.get_mut(&41).unwrap().active = true;
    authority.step(Duration::from_millis(17));
    let owner = authority.peers[&41].welcome.player.get();
    authority.sim.set_player_active(owner, true);
    authority.begin_run();
    for tick in 2..=20 {
        authority.step(Duration::from_millis(tick * 17));
    }
    let peer = authority.peers.get_mut(&41).unwrap();
    let receipt = StateReceipt {
        scope: ScopeIdentity {
            connection: peer.welcome.stream.epoch,
            entity: peer.welcome.owner_entity,
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(1),
        snapshot: SnapshotId(18),
        version: StateVersion(18),
    };
    peer.combat.sent(receipt, ServerTick(18), ServerTick(18));
    peer.combat.decoded(receipt);
    // A receive pass can observe much later wall time before the fixed driver
    // repays its debt. That must not put arrival after command execution.
    authority.now = Duration::from_secs(10);
    let view = live::CombatViewStamp {
        sampled_at: ServerTick(20),
        sampled_fraction: 0,
        viewed_at: ServerTick(18),
        viewed_fraction: 0,
        reference: receipt,
    };
    let mut shot = command(&authority, 1, 21);
    shot.actions = BoundedVec::new(vec![ActionEdge {
        slot: 0,
        action: DreamAction::Dreamlance {
            aim: [1.0, 0.0],
            view,
        },
    }])
    .unwrap();
    let key = shot.action_key(0);
    submit(&mut authority, shot.clone());
    // A redundant bundle does not replace the original arrival evidence.
    authority.now = Duration::from_secs(11);
    submit(&mut authority, shot);
    authority.step(Duration::from_secs(11));
    let terminal = authority.outcomes.lookup(owner, key).unwrap();
    assert_eq!(
        terminal.data.status,
        live::TerminalStatus::Accepted,
        "{:?}",
        terminal.data
    );

    let mut future = command(&authority, 2, 22);
    future.actions = BoundedVec::new(vec![ActionEdge {
        slot: 0,
        action: DreamAction::Dreamlance {
            aim: [1.0, 0.0],
            view: live::CombatViewStamp {
                sampled_at: ServerTick(660),
                ..view
            },
        },
    }])
    .unwrap();
    let future_key = future.action_key(0);
    submit(&mut authority, future);
    authority.step(Duration::from_secs(11));
    let terminal = authority.outcomes.lookup(owner, future_key).unwrap();
    assert_eq!(terminal.data.status, live::TerminalStatus::Rejected);
    assert_eq!(terminal.data.reason, live::OutcomeReason::Timing);
}

#[test]
fn future_action_retry_waits_for_admission_without_reserving_an_outcome() {
    let mut authority = DreamAuthority::new(17, false, 8);
    assert!(authority.admit(41));
    authority.peers.get_mut(&41).unwrap().active = true;
    authority.step(Duration::from_millis(17));
    let owner = authority.peers[&41].welcome.player.get();
    let connection = authority.peers[&41].welcome.stream.epoch;
    authority.sim.set_player_active(owner, true);
    authority.begin_run();
    let edge = command(&authority, 1, authority.tick.0 + 13);
    let key = edge.action_key(0);
    authority.now = Duration::from_secs(2);
    for _ in 0..3 {
        submit(&mut authority, edge.clone());
        assert!(authority.outcomes.lookup(owner, key).is_none());
        assert!(
            authority
                .outcomes
                .normal
                .pending(owner, connection)
                .is_empty()
        );
        assert!(
            authority
                .outcomes
                .overload
                .pending(owner, connection)
                .is_empty()
        );
        assert_eq!(
            authority
                .peers
                .get_mut(&41)
                .unwrap()
                .combat
                .take_arrival(edge.sequence),
            None
        );
    }
    // Only committed progress makes the unchanged target eligible at +12.
    authority.step(Duration::from_secs(2));
    submit(&mut authority, edge.clone());
    assert!(authority.outcomes.lookup(owner, key).is_none());
    assert_eq!(
        authority.outcomes.normal.pending(owner, connection),
        vec![key]
    );
    for _ in authority.tick.0..edge.target.0 {
        authority.step(Duration::from_secs(2));
    }
    let terminal = authority.outcomes.lookup(owner, key).unwrap();
    assert_eq!(terminal.data.status, live::TerminalStatus::Accepted);
    let snapshot = authority.sim.snapshot_for(owner);
    submit(&mut authority, edge);
    assert_eq!(authority.outcomes.lookup(owner, key), Some(terminal));
    assert_eq!(authority.sim.snapshot_for(owner), snapshot);
}
