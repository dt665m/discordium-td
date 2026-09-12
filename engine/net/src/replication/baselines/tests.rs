use super::*;

fn context() -> BaselineContext {
    BaselineContext {
        scope: ScopeIdentity {
            connection: ConnectionEpoch(1),
            entity: EntityId {
                index: 7,
                generation: 1,
            },
            scope: ScopeEpoch(1),
            representation: RepresentationRevision(1),
        },
        schema: SchemaId(1),
        group: Some((GroupId(1), GroupRevision(1))),
    }
}
fn state(snapshot: u64) -> BaselineState<Vec<u8>> {
    let mut payload = vec![b'a'; 100];
    payload[40] = snapshot as u8;
    BaselineState {
        receipt: BaselineReceipt {
            context: context(),
            generation: BaselineGeneration(1),
            snapshot: SnapshotId(snapshot),
            version: StateVersion(snapshot),
            end_tick: ServerTick(snapshot),
        },
        payload,
    }
}
fn pair() -> (ServerBaselines<Vec<u8>>, ClientBaselines<Vec<u8>>) {
    (
        ServerBaselines::new(context(), BaselineGeneration(1), BaselineLimits::default()).unwrap(),
        ClientBaselines::new(context(), BaselineGeneration(1), BaselineLimits::default()).unwrap(),
    )
}
fn send(
    server: &mut ServerBaselines<Vec<u8>>,
    state: BaselineState<Vec<u8>>,
    base: Option<BaselineReceipt>,
) -> Result<BaselinePacket, ReplicationError> {
    let mut packet = None;
    server.transmit(state, base, &BytePatch, |p| {
        packet = Some(p.clone());
        true
    })?;
    Ok(packet.unwrap())
}
fn deliver(
    server: &mut ServerBaselines<Vec<u8>>,
    client: &mut ClientBaselines<Vec<u8>>,
    value: BaselineState<Vec<u8>>,
    base: Option<BaselineReceipt>,
) -> BaselinePacket {
    let packet = send(server, value, base).unwrap();
    let receipt = client.receive(&packet, &BytePatch, |_| true).unwrap();
    server.acknowledge(receipt).unwrap();
    packet
}
#[test]
fn exact_decode_proof_required_and_transport_rejection_retains_nothing() {
    let (mut server, mut client) = pair();
    assert!(
        !server
            .transmit(state(1), None, &BytePatch, |_| false)
            .unwrap()
    );
    assert_eq!(server.retained_states(), 0);
    assert_eq!(
        server.acknowledge(state(1).receipt),
        Err(ReplicationError::Unknown)
    );
    let full = send(&mut server, state(1), None).unwrap();
    assert_eq!(
        send(&mut server, state(2), Some(full.target)),
        Err(ReplicationError::Unknown)
    );
    let proof = client.receive(&full, &BytePatch, |_| true).unwrap();
    server.validate_acknowledgment(proof).unwrap();
    assert_eq!(server.newest_decoded(), None);
    let mut forged = proof;
    forged.end_tick = ServerTick(99);
    assert_eq!(
        server.validate_acknowledgment(forged),
        Err(ReplicationError::Conflict)
    );
    assert_eq!(server.acknowledge(forged), Err(ReplicationError::Conflict));
    server.acknowledge(proof).unwrap();
    let delta = deliver(&mut server, &mut client, state(2), Some(proof));
    assert!(matches!(delta.encoding, BaselineEncoding::Delta { .. }));
    assert_eq!(delta.bytes.len(), 13);
    assert_eq!(client.current(), Some(&state(2)));
    assert_eq!(client.state(full.target), Ok(&state(1)));
    let mut forged = full.target;
    forged.end_tick = ServerTick(99);
    assert_eq!(client.state(forged), Err(ReplicationError::Conflict));
}
#[test]
fn unknown_base_never_publishes_or_acks_and_repairs_are_rate_bounded() {
    let (mut server, mut client) = pair();
    let full = deliver(&mut server, &mut client, state(1), None);
    let mut delta = send(&mut server, state(2), Some(full.target)).unwrap();
    if let BaselineEncoding::Delta { base } = &mut delta.encoding {
        base.snapshot = SnapshotId(999);
    }
    assert_eq!(
        client.receive(&delta, &BytePatch, |_| true),
        Err(ReplicationError::InvalidIdentity)
    );
    if let BaselineEncoding::Delta { base } = &mut delta.encoding {
        base.snapshot = SnapshotId(2);
    }
    delta.target = state(3).receipt;
    let before = client.retained_bytes();
    for _ in 0..100 {
        assert_eq!(
            client.receive(&delta, &BytePatch, |_| true),
            Err(ReplicationError::Unknown)
        );
    }
    assert_eq!(client.current(), Some(&state(1)));
    assert_eq!(client.retained_bytes(), before);
    assert!(client.repair_request(10).is_some());
    assert!(client.repair_request(10).is_none());
    assert!(client.repair_request(0).is_none());
    assert!(client.repair_request(1009).is_none());
    assert!(client.repair_request(1010).is_some());
    deliver(&mut server, &mut client, state(3), None);
    assert!(client.repair_request(10000).is_none());
}
#[test]
fn equivocation_and_failed_validation_are_transactional() {
    let (mut server, mut client) = pair();
    let full = deliver(&mut server, &mut client, state(1), None);
    let mut changed = full.clone();
    changed.bytes[0] = b'b';
    assert_eq!(
        client.receive(&changed, &BytePatch, |_| true),
        Err(ReplicationError::Conflict)
    );
    let mut changed_state = state(1);
    changed_state.payload[0] = b'b';
    assert_eq!(
        send(&mut server, changed_state, None),
        Err(ReplicationError::Conflict)
    );
    let delta = send(&mut server, state(2), Some(full.target)).unwrap();
    assert_eq!(
        client.receive(&delta, &BytePatch, |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    assert_eq!(client.current(), Some(&state(1)));
    assert_eq!(client.retained_states(), 1);
    client.receive(&delta, &BytePatch, |_| true).unwrap();
    assert_eq!(client.current(), Some(&state(2)));
    // An old exact duplicate can be re-ACKed without moving current backwards.
    assert_eq!(
        client.receive(&full, &BytePatch, |_| true).unwrap(),
        full.target
    );
    assert_eq!(client.current(), Some(&state(2)));
}
#[test]
fn retirement_requires_decoded_replacement_and_fences_late_useful_deltas() {
    let (mut server, mut client) = pair();
    let full = deliver(&mut server, &mut client, state(1), None);
    assert_eq!(
        client.propose_retirement(SnapshotId(1)),
        Err(ReplicationError::Incomplete)
    );
    let second = send(&mut server, state(2), Some(full.target)).unwrap();
    client.receive(&second, &BytePatch, |_| true).unwrap();
    let request = client.propose_retirement(SnapshotId(1)).unwrap();
    assert_eq!(client.retained_states(), 2);
    assert_eq!(server.retire(request), Err(ReplicationError::Incomplete));
    server.acknowledge(second.target).unwrap();
    let before = (server.retained_bytes(), server.retained_states());
    server.validate_retirement(request).unwrap();
    assert_eq!(before, (server.retained_bytes(), server.retained_states()));
    let late = send(&mut server, state(3), Some(full.target)).unwrap();
    let fence = server.retire(request).unwrap();
    assert_eq!(server.retained_states(), 2);
    assert_eq!(server.retire(request).unwrap(), fence); // lost fence retry
    assert_eq!(
        send(&mut server, state(4), Some(full.target)),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
    assert_eq!(client.propose_retirement(SnapshotId(1)).unwrap(), request);
    client.acknowledge_retirement(fence).unwrap();
    client.acknowledge_retirement(fence).unwrap();
    assert_eq!(
        client.state(full.target),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
    assert_eq!(client.retained_states(), 1);
    assert_eq!(
        client.receive(&late, &BytePatch, |_| true),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
    assert_eq!(client.current(), Some(&state(2)));
    assert!(client.repair_request(0).is_none());
    deliver(&mut server, &mut client, state(4), Some(second.target));
    assert_eq!(client.current(), Some(&state(4)));
    assert_eq!(
        client.receive(&full, &BytePatch, |_| true),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
}
#[test]
fn unsolicited_retirement_cannot_evict_promised_states() {
    let (mut server, mut client) = pair();
    deliver(&mut server, &mut client, state(1), None);
    deliver(&mut server, &mut client, state(2), None);
    let fence = BaselineRetirement {
        context: context(),
        generation: BaselineGeneration(1),
        through: SnapshotId(1),
    };
    assert_eq!(
        client.acknowledge_retirement(fence),
        Err(ReplicationError::Unknown)
    );
    assert_eq!(client.retained_states(), 2);
}
#[test]
fn reset_requires_new_full_and_rejects_old_packets_receipts_and_fences() {
    let (mut server, mut client) = pair();
    let full = deliver(&mut server, &mut client, state(1), None);
    let reset = server.begin_reset().unwrap();
    assert_eq!(server.retained_states(), 0);
    assert_eq!(
        server.acknowledge(full.target),
        Err(ReplicationError::Stale)
    );
    let mut new = state(2);
    new.receipt.generation = reset.generation;
    assert_eq!(
        send(&mut server, new.clone(), Some(full.target)),
        Err(ReplicationError::Stale)
    );
    let new_full = send(&mut server, new.clone(), None).unwrap();
    assert_eq!(
        client.receive(&new_full, &BytePatch, |_| true),
        Err(ReplicationError::BaselineResetPending)
    );
    assert_eq!(client.current(), Some(&state(1)));
    assert_eq!(
        client.receive(&new_full, &BytePatch, |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    let mut future = new_full.clone();
    future.target.generation = BaselineGeneration(3);
    assert_eq!(
        client.receive(&future, &BytePatch, |_| true),
        Err(ReplicationError::Stale)
    );
    client.apply_reset(reset).unwrap();
    assert!(client.current().is_none());
    assert_eq!(
        client.receive(&full, &BytePatch, |_| true),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
    assert_eq!(
        client.receive(&full, &BytePatch, |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    let mut contradictory_old = full.clone();
    contradictory_old.target.snapshot = SnapshotId(2);
    contradictory_old.target.end_tick = ServerTick(0);
    assert_eq!(
        client.receive(&contradictory_old, &BytePatch, |_| true),
        Err(ReplicationError::Conflict)
    );
    let receipt = client.receive(&new_full, &BytePatch, |_| true).unwrap();
    server.acknowledge(receipt).unwrap();
    client.apply_reset(reset).unwrap(); // reordered duplicate cannot purge new state
    assert_eq!(client.current(), Some(&new));
    let old = BaselineReset {
        generation: BaselineGeneration(1),
        ..reset
    };
    assert_eq!(client.apply_reset(old), Err(ReplicationError::Stale));
    let mut reused = state(1);
    reused.receipt.generation = reset.generation;
    assert_eq!(
        send(&mut server, reused, None),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::Publication
        ))
    );
}
#[test]
fn every_context_dimension_is_an_independent_fence() {
    let (mut server, mut client) = pair();
    let full = deliver(&mut server, &mut client, state(1), None);
    let mut contexts = vec![context(); 7];
    contexts[0].scope.connection = ConnectionEpoch(2);
    contexts[1].scope.entity.generation = 2;
    contexts[2].scope.scope = ScopeEpoch(2);
    contexts[3].scope.representation = RepresentationRevision(2);
    contexts[4].schema = SchemaId(2);
    contexts[5].group = Some((GroupId(2), GroupRevision(1)));
    contexts[6].group = Some((GroupId(1), GroupRevision(2)));
    for context in contexts {
        let mut wrong = full.clone();
        wrong.target.context = context;
        assert_eq!(
            client.receive(&wrong, &BytePatch, |_| true),
            Err(ReplicationError::Stale)
        );
        assert_eq!(
            server.acknowledge(wrong.target),
            Err(ReplicationError::Stale)
        );
    }
    client.close();
    server.close();
    assert!(client.current().is_none());
    assert_eq!(
        client.receive(&full, &BytePatch, |_| true),
        Err(ReplicationError::Stale)
    );
    assert_eq!(server.begin_reset(), Err(ReplicationError::Stale));
    assert_eq!(
        send(&mut server, state(2), None),
        Err(ReplicationError::Stale)
    );
}
#[test]
fn bounded_slots_never_silently_evict_and_retirement_recovers_capacity() {
    let limits = BaselineLimits {
        slots: 2,
        ..BaselineLimits::default()
    };
    let mut server = ServerBaselines::new(context(), BaselineGeneration(1), limits).unwrap();
    let mut client = ClientBaselines::new(context(), BaselineGeneration(1), limits).unwrap();
    let first = deliver(&mut server, &mut client, state(1), None);
    deliver(&mut server, &mut client, state(2), None);
    let before = server.retained_bytes();
    assert_eq!(
        send(&mut server, state(3), None),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(server.retained_bytes(), before);
    assert_eq!(server.newest_decoded(), Some(state(2).receipt));
    let request = client.propose_retirement(SnapshotId(1)).unwrap();
    let fence = server.retire(request).unwrap();
    let third = send(&mut server, state(3), None).unwrap();
    assert_eq!(
        client.receive(&third, &BytePatch, |_| true),
        Err(ReplicationError::Capacity)
    );
    // The retirement proposal did not break the retention promise.
    assert_eq!(
        client.receive(&first, &BytePatch, |_| true).unwrap(),
        first.target
    );
    client.acknowledge_retirement(fence).unwrap();
    client.receive(&third, &BytePatch, |_| true).unwrap();
    assert_eq!(client.current(), Some(&state(3)));
}
#[test]
fn aggregate_payload_packet_and_peak_limits_are_enforced() {
    let (mut server, mut client) = pair();
    let original = server.retained_bytes();
    let mut huge = state(1);
    huge.payload = vec![0; BaselineLimits::default().payload_bytes + 1];
    assert_eq!(
        send(&mut server, huge, None),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(server.retained_bytes(), original);
    let limits = BaselineLimits {
        peak_bytes: 1,
        ..BaselineLimits::default()
    };
    assert!(matches!(
        ServerBaselines::<Vec<u8>>::new(context(), BaselineGeneration(1), limits),
        Err(ReplicationError::InvalidConfiguration)
    ));
    let limits = BaselineLimits {
        payload_bytes: 100,
        resident_bytes: original + 150,
        ..BaselineLimits::default()
    };
    let mut tight = ServerBaselines::new(context(), BaselineGeneration(1), limits).unwrap();
    send(&mut tight, state(1), None).unwrap();
    assert_eq!(
        send(&mut tight, state(2), None),
        Err(ReplicationError::Capacity)
    );
    let mut packet = send(&mut server, state(1), None).unwrap();
    packet.bytes.reserve(BaselineLimits::default().packet_bytes);
    assert_eq!(
        client.receive(&packet, &BytePatch, |_| true),
        Err(ReplicationError::Capacity)
    );
    assert!(client.current().is_none());
}
#[test]
fn byte_patch_handles_insert_delete_empty_and_malformed_without_large_allocations() {
    let cases = [
        (b"".as_slice(), b"".as_slice()),
        (b"abc", b"abxc"),
        (b"abc", b"ac"),
        (b"aaaa", b"aa"),
        (b"abc", b"xyz"),
        (b"", b"abc"),
        (b"abc", b""),
    ];
    for (base, target) in cases {
        let patch = BytePatch
            .encode_delta(&base.to_vec(), &target.to_vec(), 100)
            .unwrap();
        assert_eq!(
            BytePatch.decode_delta(&base.to_vec(), &patch, 100).unwrap(),
            target
        );
        assert_eq!(
            patch,
            BytePatch
                .encode_delta(&base.to_vec(), &target.to_vec(), 100)
                .unwrap()
        );
    }
    assert_eq!(
        BytePatch.decode_delta(&vec![], &[0; 11], 100),
        Err(ReplicationError::InvalidPayload)
    );
    let mut malformed = vec![0; 12];
    malformed[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        BytePatch.decode_delta(&vec![], &malformed, 100),
        Err(ReplicationError::Capacity)
    );
    malformed[..4].copy_from_slice(&1u32.to_le_bytes());
    malformed[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        BytePatch.decode_delta(&vec![0; 3], &malformed, 100),
        Err(ReplicationError::InvalidPayload)
    );
}
#[test]
fn staged_group_clone_does_not_publish_partial_members() {
    let (mut server, mut client) = pair();
    deliver(&mut server, &mut client, state(1), None);
    let second = send(&mut server, state(2), Some(state(1).receipt)).unwrap();
    let mut staging = client.clone();
    let receipt = staging.receive(&second, &BytePatch, |_| true).unwrap();
    assert_eq!(client.current(), Some(&state(1)));
    assert_eq!(server.newest_decoded(), Some(state(1).receipt));
    // Outer group validates all its other members before this commit/ACK.
    client = staging;
    server.acknowledge(receipt).unwrap();
    assert_eq!(client.current(), Some(&state(2)));
    for n in 3..=8 {
        deliver(
            &mut server,
            &mut client,
            state(n),
            Some(state(n - 1).receipt),
        );
    }
    assert_eq!(client.retained_states(), 8);
    assert!(client.retained_bytes() <= BaselineLimits::default().resident_bytes);
}

#[test]
fn delta_larger_than_packet_budget_falls_back_to_complete_bytes() {
    let limits = BaselineLimits {
        packet_bytes: 100,
        ..BaselineLimits::default()
    };
    let mut server = ServerBaselines::new(context(), BaselineGeneration(1), limits).unwrap();
    let mut client = ClientBaselines::new(context(), BaselineGeneration(1), limits).unwrap();
    deliver(&mut server, &mut client, state(1), None);
    let mut changed = state(2);
    changed.payload.fill(b'z');
    let full = deliver(
        &mut server,
        &mut client,
        changed.clone(),
        Some(state(1).receipt),
    );
    assert_eq!(full.encoding, BaselineEncoding::Full);
    assert_eq!(full.bytes.len(), 100);
    assert_eq!(client.current(), Some(&changed));
}

#[test]
fn deterministic_byte_patch_roundtrip_for_varied_overlapping_spans() {
    for base_len in 0..32 {
        for target_len in 0..32 {
            for changed in [0, target_len / 2, target_len] {
                let base = vec![b'a'; base_len];
                let mut target = vec![b'a'; target_len];
                if changed < target_len {
                    target[changed] = b'b';
                }
                let patch = BytePatch.encode_delta(&base, &target, 100).unwrap();
                assert_eq!(BytePatch.decode_delta(&base, &patch, 100).unwrap(), target);
                // Truncation and unaccounted trailing data cannot partially decode.
                let mut extra = patch.clone();
                extra.push(0);
                assert_eq!(
                    BytePatch.decode_delta(&base, &extra, 100),
                    Err(ReplicationError::InvalidPayload)
                );
            }
        }
    }
}

#[test]
fn already_retired_proposals_cannot_poison_next_or_outstanding_retirement() {
    let (mut server, mut client) = pair();
    let one = deliver(&mut server, &mut client, state(1), None);
    let two = deliver(&mut server, &mut client, state(2), Some(one.target));
    deliver(&mut server, &mut client, state(3), Some(two.target));
    let first = client.propose_retirement(SnapshotId(1)).unwrap();
    client
        .acknowledge_retirement(server.retire(first).unwrap())
        .unwrap();
    assert_eq!(
        client.propose_retirement(SnapshotId(1)),
        Err(ReplicationError::Stale)
    );
    let second = client.propose_retirement(SnapshotId(2)).unwrap();
    assert_eq!(
        client.propose_retirement(SnapshotId(1)),
        Err(ReplicationError::Stale)
    );
    // Neither a stale proposal nor its duplicate ACK can replace the live proposal.
    client.acknowledge_retirement(first).unwrap();
    client
        .acknowledge_retirement(server.retire(second).unwrap())
        .unwrap();
    assert_eq!(client.retained_states(), 1);
    assert_eq!(server.retained_states(), 1);
    assert_eq!(client.current(), Some(&state(3)));
}

#[test]
fn retired_base_cannot_hide_mixed_publication_metadata_or_unknown_context() {
    let (mut server, mut client) = pair();
    let first = deliver(&mut server, &mut client, state(1), None);
    deliver(&mut server, &mut client, state(3), Some(first.target));
    let late = send(&mut server, state(4), Some(first.target)).unwrap();
    let request = client.propose_retirement(SnapshotId(1)).unwrap();
    client
        .acknowledge_retirement(server.retire(request).unwrap())
        .unwrap();
    assert_eq!(
        client.receive(&late, &BytePatch, |_| true),
        Err(ReplicationError::Obsolete(
            crate::replication::ObsoleteReason::RetiredBaseline
        ))
    );
    for (snapshot, version, tick) in [(4, 2, 4), (4, 4, 2), (2, 4, 2), (2, 2, 4), (3, 4, 3)] {
        let mut invalid = late.clone();
        invalid.target.snapshot = SnapshotId(snapshot);
        invalid.target.version = StateVersion(version);
        invalid.target.end_tick = ServerTick(tick);
        assert_eq!(
            client.receive(&invalid, &BytePatch, |_| true),
            Err(ReplicationError::Conflict)
        );
        assert_eq!(client.current(), Some(&state(3)));
    }
    let mut unknown = late.clone();
    unknown.target.context.schema = SchemaId(99);
    let BaselineEncoding::Delta { ref mut base } = unknown.encoding else {
        panic!("delta")
    };
    base.context = unknown.target.context;
    assert_eq!(
        client.receive(&unknown, &BytePatch, |_| true),
        Err(ReplicationError::Stale)
    );
    assert_eq!(
        client.receive(&first, &BytePatch, |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    assert_eq!(client.current(), Some(&state(3)));
    assert!(client.repair_request(0).is_none());
}
