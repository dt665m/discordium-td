use super::*;

fn state(generation: u32, version: u64) -> FullState<Vec<u8>> {
    FullState {
        scope: ScopeIdentity {
            connection: EPOCH,
            entity: EntityId {
                index: 201,
                generation,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        baseline_generation: BaselineGeneration(1),
        snapshot: SnapshotId(version),
        version: StateVersion(version),
        end_tick: ServerTick(version),
        payload: vec![version as u8; 64],
    }
}

fn queued_peer(current: &FullState<Vec<u8>>, delayed: &FullState<Vec<u8>>) -> Peer {
    let mut peer = Peer::new(0);
    peer.client
        .apply_full(current.clone(), |p| p.len() == 64)
        .unwrap();
    let bytes = encode_full_state(delayed, 16384).unwrap();
    peer.queue_bytes = bytes.capacity();
    peer.queue.push_back(Packet {
        due: 3,
        group: false,
        bytes,
    });
    peer
}

#[test]
fn delayed_publication_is_counted_without_acknowledging_or_replacing_current() {
    let current = state(1, 3);
    let mut peer = queued_peer(&current, &state(1, 2));
    let mut metrics = Metrics::default();
    peer.deliver(3, &mut metrics);
    assert_eq!(metrics.stale, 1);
    assert_eq!(metrics.decoded, 0);
    assert_eq!(peer.client.state(current.scope.entity), Some(&current));
    assert!(peer.server.decoded_current(current.scope.entity).is_none());
    assert!(peer.queue.is_empty());
    assert_eq!(peer.queue_bytes, 0);
}

#[test]
fn delayed_retired_generation_keeps_current_scope() {
    let current = state(2, 3);
    let mut peer = queued_peer(&current, &state(1, 2));
    let mut metrics = Metrics::default();
    peer.deliver(3, &mut metrics);
    assert_eq!(metrics.stale, 1);
    assert_eq!(metrics.decoded, 0);
    assert_eq!(peer.client.state(current.scope.entity), Some(&current));
}

#[test]
fn old_packets_with_invalid_payload_or_conflicting_watermarks_still_fail() {
    let current = state(1, 3);
    let mut malformed = state(1, 2);
    malformed.payload.pop();
    let mut conflicting = state(1, 2);
    conflicting.version = StateVersion(4);
    for delayed in [malformed, conflicting] {
        let mut peer = queued_peer(&current, &delayed);
        let mut metrics = Metrics::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            peer.deliver(3, &mut metrics);
        }));
        assert!(result.is_err());
        assert_eq!(metrics.stale, 0);
        assert_eq!(metrics.decoded, 0);
        assert_eq!(peer.client.state(current.scope.entity), Some(&current));
    }
}
