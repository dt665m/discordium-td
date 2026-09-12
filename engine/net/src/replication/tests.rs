use super::*;

fn entity(index: u64) -> EntityId {
    EntityId {
        index,
        generation: 1,
    }
}
fn scope(index: u64, incarnation: u64) -> ScopeIdentity {
    ScopeIdentity {
        connection: ConnectionEpoch(1),
        entity: entity(index),
        scope: ScopeEpoch(incarnation),
        representation: RepresentationRevision(1),
    }
}
fn full(index: u64, incarnation: u64, version: u64) -> FullState<Vec<u8>> {
    FullState {
        scope: scope(index, incarnation),
        baseline_generation: BaselineGeneration(version),
        snapshot: SnapshotId(version),
        version: StateVersion(version),
        end_tick: ServerTick(version),
        payload: vec![version as u8],
    }
}
fn client() -> ClientScopes<Vec<u8>> {
    ClientScopes::new(ConnectionEpoch(1), ScopeLimits::default()).unwrap()
}
fn server() -> ServerScopes<Vec<u8>> {
    ServerScopes::new(
        ConnectionId(10),
        ConnectionEpoch(1),
        1,
        PolicyRevision(1),
        ScopeLimits::default(),
    )
    .unwrap()
}
fn publish(
    server: &mut ServerScopes<Vec<u8>>,
    client: &mut ClientScopes<Vec<u8>>,
    index: u64,
) -> StateReceipt {
    let state = server.pending(entity(index)).unwrap().clone();
    server.mark_sent(state.receipt()).unwrap();
    let ack = client.apply_full(state, |_| true).unwrap();
    server.acknowledge(ack).unwrap();
    ack
}

#[test]
fn selection_unsent_and_future_ack_never_authorize_baselines() {
    let mut server = server();
    server
        .fixture_offer(entity(1), 1, 7, true, vec![7])
        .unwrap();
    let ack = server.pending(entity(1)).unwrap().receipt();
    assert_eq!(server.acknowledge(ack), Err(ReplicationError::Unknown));
    assert!(server.decoded_current(entity(1)).is_none());
    server.mark_sent(ack).unwrap();
    server.validate_acknowledgment(ack).unwrap();
    assert!(server.decoded_current(entity(1)).is_none());
    let mut future = ack;
    future.version = StateVersion(8);
    assert_eq!(server.acknowledge(future), Err(ReplicationError::Unknown));
    future = ack;
    future.baseline_generation = BaselineGeneration(99);
    assert_eq!(
        server.validate_acknowledgment(future),
        Err(ReplicationError::Unknown)
    );
    assert_eq!(server.acknowledge(future), Err(ReplicationError::Unknown));
    server.acknowledge(ack).unwrap();
    server.acknowledge(ack).unwrap();
    assert_eq!(server.decoded_current(entity(1)), Some(ack));
    assert_eq!(server.phase(entity(1)), Some(DeliveryPhase::DormantKnown));
}

#[test]
fn independent_dormancy_dirty_retry_and_late_join() {
    let mut a = server();
    let mut b = server();
    let mut ca = client();
    let mut cb = client();
    for peer in [&mut a, &mut b] {
        peer.fixture_offer(entity(1), 1, 7, true, vec![7]).unwrap();
    }
    publish(&mut a, &mut ca, 1);
    assert!(a.pending(entity(1)).is_none());
    assert_eq!(b.pending(entity(1)).unwrap().version, StateVersion(7));
    let retry = b.pending(entity(1)).unwrap().clone();
    b.mark_sent(retry.receipt()).unwrap();
    assert_eq!(b.pending(entity(1)), Some(&retry));
    publish(&mut b, &mut cb, 1);
    for peer in [&mut a, &mut b] {
        peer.fixture_offer(entity(1), 1, 8, true, vec![8]).unwrap();
    }
    publish(&mut a, &mut ca, 1);
    assert_eq!(a.phase(entity(1)), Some(DeliveryPhase::DormantKnown));
    assert_eq!(b.phase(entity(1)), Some(DeliveryPhase::Active));
    assert_eq!(b.pending(entity(1)).unwrap().version, StateVersion(8));
    let mut late = server();
    late.fixture_offer(entity(1), 1, 8, true, vec![8]).unwrap();
    assert_eq!(late.phase(entity(1)), Some(DeliveryPhase::Entering));
    assert_eq!(late.pending(entity(1)).unwrap().payload, vec![8]);
}

#[test]
fn mutation_requires_dirty_version_and_failed_offer_is_transactional() {
    let limits = ScopeLimits {
        payload_bytes: 2,
        retained_payload_bytes: 2,
        ..ScopeLimits::default()
    };
    let mut server = ServerScopes::new(
        ConnectionId(1),
        ConnectionEpoch(1),
        1,
        PolicyRevision(1),
        limits,
    )
    .unwrap();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    let original = server.pending(entity(1)).unwrap().clone();
    assert_eq!(
        server.fixture_offer(entity(1), 1, 1, false, vec![2]),
        Err(ReplicationError::Conflict)
    );
    assert_eq!(
        server.fixture_offer(entity(1), 1, 2, false, vec![0; 3]),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(server.pending(entity(1)), Some(&original));
    assert_eq!(server.retained_bytes(), 1);
    server
        .fixture_offer(entity(1), 1, 2, false, vec![2])
        .unwrap();
    assert_eq!(server.pending(entity(1)).unwrap().snapshot, SnapshotId(2));
}

#[test]
fn representation_reentry_and_policy_barrier_cancel_unsent_work() {
    let mut server = server();
    let mut client = client();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    let old = publish(&mut server, &mut client, 1);
    server
        .fixture_offer(entity(1), 1, 2, false, vec![2])
        .unwrap();
    let queued = server.pending(entity(1)).unwrap().receipt();
    server.advance_barrier(2, PolicyRevision(2)).unwrap();
    assert!(server.pending(entity(1)).is_none());
    assert_eq!(server.retained_bytes(), 0);
    assert_eq!(server.mark_sent(queued), Err(ReplicationError::Stale));
    assert_eq!(server.acknowledge(old), Err(ReplicationError::Stale));
    let exit = server.pending_exits().next().unwrap();
    server
        .fixture_offer(entity(1), 2, 2, false, vec![2])
        .unwrap();
    let new = publish(&mut server, &mut client, 1);
    assert!(new.scope.scope > old.scope.scope);
    assert_eq!(new.scope.representation, RepresentationRevision(2));
    client.apply_exit(exit).unwrap();
    assert_eq!(client.state(entity(1)).unwrap().receipt(), new);
    assert_eq!(server.acknowledge_exit(exit), Err(ReplicationError::Stale));
}

#[test]
fn exit_retry_requires_sent_receipt_and_keeps_fence() {
    let mut server = server();
    let mut client = client();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    let entry = server.pending(entity(1)).unwrap().clone();
    let exit = server.exit(entity(1)).unwrap();
    assert_eq!(server.acknowledge_exit(exit), Err(ReplicationError::Stale));
    assert_eq!(server.unsent_exits().collect::<Vec<_>>(), vec![exit]);
    server.mark_exit_sent(exit).unwrap();
    assert!(server.unsent_exits().next().is_none());
    assert_eq!(server.pending_exits().collect::<Vec<_>>(), vec![exit]);
    client.apply_exit(exit).unwrap();
    assert_eq!(
        client.apply_full(entry, |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Scope))
    );
    server
        .acknowledge_exit(client.apply_exit(exit).unwrap())
        .unwrap();
    server.acknowledge_exit(exit).unwrap();
    assert_eq!(server.phase(entity(1)), Some(DeliveryPhase::Forgotten));
    assert_eq!(server.known_entities(), 1);
    server
        .fixture_offer(entity(1), 1, 2, false, vec![2])
        .unwrap();
    assert_eq!(
        server.pending(entity(1)).unwrap().scope.scope,
        ScopeEpoch(2)
    );
}

#[test]
fn old_generations_and_destroy_never_resurrect_or_remove_new_generation() {
    let mut client = client();
    let old = full(1, 1, 1);
    client.apply_full(old.clone(), |_| true).unwrap();
    let death = Destroy {
        connection: ConnectionEpoch(1),
        entity: entity(1),
    };
    client.apply_destroy(death).unwrap();
    assert_eq!(
        client.apply_full(full(1, 2, 2), |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Scope))
    );
    let mut next = full(1, 1, 3);
    next.scope.entity.generation = 2;
    client.apply_full(next.clone(), |_| true).unwrap();
    client.apply_destroy(death).unwrap();
    client.apply_exit(ScopeExit { scope: old.scope }).unwrap();
    assert_eq!(client.state(next.scope.entity), Some(&next));
    assert_eq!(
        client.apply_full(old, |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Scope))
    );
}

#[test]
fn client_conflicting_or_invalid_payload_and_budget_failures_are_atomic() {
    let limits = ScopeLimits {
        payload_bytes: 2,
        retained_payload_bytes: 2,
        ..ScopeLimits::default()
    };
    let mut client = ClientScopes::new(ConnectionEpoch(1), limits).unwrap();
    let initial = full(1, 1, 1);
    client.apply_full(initial.clone(), |_| true).unwrap();
    let mut invalid = full(1, 1, 2);
    invalid.payload = vec![9];
    assert_eq!(
        client.apply_full(invalid, |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    let mut large = full(1, 2, 2);
    large.payload = vec![0; 3];
    assert_eq!(
        client.apply_full(large, |_| true),
        Err(ReplicationError::Capacity)
    );
    let mut conflict = initial.clone();
    conflict.payload = vec![5];
    assert_eq!(
        client.apply_full(conflict, |_| true),
        Err(ReplicationError::Conflict)
    );
    assert_eq!(client.state(entity(1)), Some(&initial));
    assert_eq!(client.retained_bytes(), 1);
}

#[test]
fn identity_caps_require_explicit_epoch_reset() {
    let limits = ScopeLimits {
        known_entities: 1,
        ..ScopeLimits::default()
    };
    let mut client = ClientScopes::new(ConnectionEpoch(1), limits).unwrap();
    client.apply_exit(ScopeExit { scope: scope(1, 1) }).unwrap();
    assert_eq!(
        client.apply_full(full(2, 1, 1), |_| true),
        Err(ReplicationError::ResetRequired)
    );
    assert_eq!(
        client.reset(ConnectionEpoch(1)),
        Err(ReplicationError::Stale)
    );
    client.reset(ConnectionEpoch(2)).unwrap();
    assert_eq!(
        client.apply_full(full(2, 1, 1), |_| true),
        Err(ReplicationError::Stale)
    );
    let mut fresh = full(2, 1, 1);
    fresh.scope.connection = ConnectionEpoch(2);
    client.apply_full(fresh, |_| true).unwrap();
    let mut server = ServerScopes::new(
        ConnectionId(1),
        ConnectionEpoch(1),
        1,
        PolicyRevision(1),
        limits,
    )
    .unwrap();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    server.exit(entity(1)).unwrap();
    assert_eq!(
        server.fixture_offer(entity(2), 1, 1, false, vec![1]),
        Err(ReplicationError::ResetRequired)
    );
    server.reset(ConnectionEpoch(2)).unwrap();
    server
        .fixture_offer(entity(2), 1, 1, false, vec![1])
        .unwrap();
}

fn permutations(items: &mut [usize], at: usize, visit: &mut impl FnMut(&[usize])) {
    if at == items.len() {
        visit(items);
        return;
    }
    for i in at..items.len() {
        items.swap(i, at);
        permutations(items, at + 1, visit);
        items.swap(i, at);
    }
}
#[test]
fn all_120_lifecycle_and_state_orderings_converge() {
    let mut checked = 0;
    permutations(&mut [0, 1, 2, 3, 4], 0, &mut |order| {
        let mut client = client();
        for item in order {
            match item {
                0 => {
                    let _ = client.apply_full(full(1, 1, 1), |_| true);
                }
                1 => {
                    let _ = client.apply_exit(ScopeExit { scope: scope(1, 1) });
                }
                2 => {
                    let _ = client.apply_full(full(1, 2, 2), |_| true);
                }
                3 => {
                    let _ = client.apply_full(full(1, 2, 3), |_| true);
                }
                4 => {
                    let _ = client.apply_full(full(1, 1, 4), |_| true);
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(
            client.state(entity(1)),
            Some(&full(1, 2, 3)),
            "order {order:?}"
        );
        checked += 1;
    });
    assert_eq!(checked, 120);
}

fn chunks() -> [GroupChunk<Vec<u8>>; 2] {
    let key = GroupPublication {
        connection: ConnectionEpoch(1),
        group: GroupId(1),
        revision: GroupRevision(1),
        snapshot: SnapshotId(1),
    };
    let manifest = vec![scope(1, 1), scope(2, 1)];
    std::array::from_fn(|i| GroupChunk {
        publication: key,
        end_tick: ServerTick(1),
        manifest: manifest.clone(),
        index: i,
        count: 2,
        members: vec![full(i as u64 + 1, 1, 1)],
    })
}
fn assembler() -> GroupAssembler<Vec<u8>> {
    GroupAssembler::new(ConnectionEpoch(1), GroupLimits::default()).unwrap()
}
#[test]
fn group_reordering_duplicate_and_complete_publication() {
    let mut client = client();
    let mut assembler = assembler();
    let chunks = chunks();
    assert_eq!(
        assembler
            .receive(chunks[1].clone(), 0, &mut client, |_| true)
            .unwrap(),
        GroupProgress::Staged
    );
    assert!(client.state(entity(2)).is_none());
    assert_eq!(
        assembler
            .receive(chunks[1].clone(), 1, &mut client, |_| true)
            .unwrap(),
        GroupProgress::Staged
    );
    let result = assembler
        .receive(chunks[0].clone(), 2, &mut client, |_| true)
        .unwrap();
    assert!(matches!(&result, GroupProgress::Published(r) if r.len() == 2));
    assert_eq!(
        assembler
            .receive(chunks[0].clone(), 3, &mut client, |_| true)
            .unwrap(),
        result
    );
    assert!(client.state(entity(1)).is_some());
    assert!(client.state(entity(2)).is_some());
    assert_eq!(assembler.staged_bytes(), 0);
}

#[test]
fn group_scope_conflict_leaves_all_published_replicas_unchanged() {
    let mut client = client();
    let mut assembler = assembler();
    let chunks = chunks();
    client.apply_exit(ScopeExit { scope: scope(2, 1) }).unwrap();
    assembler
        .receive(chunks[0].clone(), 0, &mut client, |_| true)
        .unwrap();
    assert_eq!(
        assembler.receive(chunks[1].clone(), 1, &mut client, |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Scope))
    );
    assert!(client.state(entity(1)).is_none());
    assert_eq!(client.known_entities(), 1);
    assert_eq!(client.retained_bytes(), 0);
    assert_eq!(assembler.staged_bytes(), 0);
}

#[test]
fn group_final_publish_respects_aggregate_replica_budget_atomically() {
    let limits = ScopeLimits {
        payload_bytes: 1,
        retained_payload_bytes: 1,
        ..ScopeLimits::default()
    };
    let mut client = ClientScopes::new(ConnectionEpoch(1), limits).unwrap();
    let mut assembler = assembler();
    let chunks = chunks();
    assembler
        .receive(chunks[0].clone(), 0, &mut client, |_| true)
        .unwrap();
    assert_eq!(
        assembler.receive(chunks[1].clone(), 1, &mut client, |_| true),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(client.known_entities(), 0);
    assert_eq!(client.retained_bytes(), 0);
}

#[test]
fn group_deadline_cannot_be_restarted_by_newer_snapshots_or_revisions() {
    let mut client = client();
    let mut assembler = assembler();
    let chunks = chunks();
    assembler
        .receive(chunks[0].clone(), 0, &mut client, |_| true)
        .unwrap();
    let mut newer = chunks[0].clone();
    newer.publication.snapshot = SnapshotId(2);
    assert_eq!(
        assembler.receive(newer.clone(), 1000, &mut client, |_| true),
        Ok(GroupProgress::Staged)
    );
    newer.publication.revision = GroupRevision(2);
    assembler
        .receive(newer.clone(), 1500, &mut client, |_| true)
        .unwrap();
    assert_eq!(assembler.expire(2000).unwrap(), 1);
    assert_eq!(assembler.staged_bytes(), 0);
    assert_eq!(
        assembler.receive(newer.clone(), 2001, &mut client, |_| true),
        Err(ReplicationError::Expired)
    );
    newer.publication.snapshot = SnapshotId(3);
    assert_eq!(
        assembler.receive(newer, 2002, &mut client, |_| true),
        Err(ReplicationError::Expired)
    );
    assert_eq!(assembler.expire(1000), Err(ReplicationError::Stale));
}

#[test]
fn group_rejects_mixed_manifest_duplicate_member_and_changed_revision() {
    let mut client = client();
    let mut assembler = assembler();
    let chunks = chunks();
    assembler
        .receive(chunks[0].clone(), 0, &mut client, |_| true)
        .unwrap();
    let initial_bytes = assembler.staged_bytes();
    let mut wrong = chunks[1].clone();
    wrong.members[0].scope.scope = ScopeEpoch(2);
    assert_eq!(
        assembler.receive(wrong, 1, &mut client, |_| true),
        Err(ReplicationError::Conflict)
    );
    let mut duplicate = chunks[1].clone();
    duplicate.members = chunks[0].members.clone();
    assert_eq!(
        assembler.receive(duplicate, 2, &mut client, |_| true),
        Err(ReplicationError::Conflict)
    );
    let mut wrong = chunks[1].clone();
    wrong.manifest[0].scope = ScopeEpoch(2);
    wrong.publication.snapshot = SnapshotId(2);
    assert_eq!(
        assembler.receive(wrong, 3, &mut client, |_| true),
        Err(ReplicationError::Conflict)
    );
    assert_eq!(assembler.staged_bytes(), initial_bytes);
    assert_eq!(assembler.incomplete(), 1);
}

#[test]
fn group_caps_charge_owned_capacity_without_partial_admission() {
    let limits = GroupLimits {
        group_bytes: 1024,
        staging_bytes: 1024,
        ..GroupLimits::default()
    };
    let mut assembler = GroupAssembler::new(ConnectionEpoch(1), limits).unwrap();
    let mut client = client();
    let mut chunk = chunks()[0].clone();
    chunk.members[0].payload = Vec::with_capacity(2048);
    chunk.members[0].payload.push(1);
    assert_eq!(
        assembler.receive(chunk, 0, &mut client, |_| true),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(assembler.incomplete(), 0);
    assert_eq!(assembler.staged_bytes(), 0);
    let mut chunk = chunks()[0].clone();
    chunk.members.reserve_exact(1000);
    assert_eq!(
        assembler.receive(chunk, 1, &mut client, |_| true),
        Err(ReplicationError::Capacity)
    );
    assert_eq!(assembler.incomplete(), 0);
}

#[test]
fn full_state_loss_beyond_proof_cap_recovers_without_untracked_send() {
    let mut server = server();
    let mut client = client();
    let mut first = None;
    for version in 1..=20 {
        server
            .fixture_offer(entity(1), 1, version, true, vec![version as u8])
            .unwrap();
        let receipt = server.pending(entity(1)).unwrap().receipt();
        first.get_or_insert(receipt);
        assert!(server.transmit_pending(entity(1), |_| true).unwrap());
    }
    assert_eq!(
        server.acknowledge(first.unwrap()),
        Err(ReplicationError::Unknown)
    );
    let latest = publish(&mut server, &mut client, 1);
    assert_eq!(latest.version, StateVersion(20));
    assert_eq!(server.phase(entity(1)), Some(DeliveryPhase::DormantKnown));
    server
        .fixture_offer(entity(1), 1, 21, true, vec![21])
        .unwrap();
    assert!(!server.transmit_pending(entity(1), |_| false).unwrap());
    assert_eq!(
        server.acknowledge(server.pending(entity(1)).unwrap().receipt()),
        Err(ReplicationError::Unknown)
    );
}

#[test]
fn transition_cap_is_independent_and_revocation_overflow_fails_closed() {
    let limits = ScopeLimits {
        pending_transitions: 1,
        ..ScopeLimits::default()
    };
    let mut server = ServerScopes::new(
        ConnectionId(1),
        ConnectionEpoch(1),
        1,
        PolicyRevision(1),
        limits,
    )
    .unwrap();
    let mut client = client();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    assert_eq!(
        server.fixture_offer(entity(2), 1, 1, false, vec![1]),
        Err(ReplicationError::Capacity)
    );
    publish(&mut server, &mut client, 1);
    server
        .fixture_offer(entity(2), 1, 1, false, vec![1])
        .unwrap();
    publish(&mut server, &mut client, 2);
    assert_eq!(server.pending_transitions(), 0);
    assert_eq!(
        server.advance_barrier(2, PolicyRevision(2)),
        Err(ReplicationError::ResetRequired)
    );
    assert_eq!(server.retained_bytes(), 0);
    assert!(server.pending(entity(1)).is_none());
    assert!(server.pending(entity(2)).is_none());
    assert_eq!(
        server.fixture_offer(entity(1), 2, 2, false, vec![1]),
        Err(ReplicationError::ResetRequired)
    );
    server.reset(ConnectionEpoch(2)).unwrap();
    server.advance_barrier(2, PolicyRevision(2)).unwrap();
    server
        .fixture_offer(entity(1), 2, 2, false, vec![1])
        .unwrap();
}

#[test]
fn server_death_is_retryable_and_distinct_from_revocation() {
    let mut server = server();
    let mut client = client();
    server
        .fixture_offer(entity(1), 1, 1, false, vec![1])
        .unwrap();
    let death = server.destroy(entity(1)).unwrap();
    assert!(server.pending_exits().next().is_none());
    assert_eq!(server.pending_destroys().collect::<Vec<_>>(), vec![death]);
    assert_eq!(
        server.acknowledge_destroy(death),
        Err(ReplicationError::Stale)
    );
    assert_eq!(server.unsent_destroys().collect::<Vec<_>>(), vec![death]);
    server.mark_destroy_sent(death).unwrap();
    assert!(server.unsent_destroys().next().is_none());
    assert_eq!(server.pending_destroys().collect::<Vec<_>>(), vec![death]);
    server
        .acknowledge_destroy(client.apply_destroy(death).unwrap())
        .unwrap();
    server.acknowledge_destroy(death).unwrap();
    assert_eq!(
        server.fixture_offer(entity(1), 1, 2, false, vec![2]),
        Err(ReplicationError::Stale)
    );
    let new = EntityId {
        index: 1,
        generation: 2,
    };
    server.fixture_offer(new, 1, 2, false, vec![2]).unwrap();
    server.advance_barrier(2, PolicyRevision(2)).unwrap();
    assert_eq!(server.destroy(new), Err(ReplicationError::Unauthorized));
    assert!(server.pending_destroys().next().is_none());
}

fn byte_limits() -> ByteLimits {
    ByteLimits {
        fragment_payload_bytes: 1100,
        group_bytes: 16384,
        fragments: 16,
        incomplete: 8,
        known_groups: 8,
        staging_bytes: 8 * 16384,
        max_age_ms: 2000,
    }
}
#[test]
fn sixteen_kib_bytes_fit_bounded_frames_and_reassemble_under_reordering() {
    let limits = byte_limits();
    let key = chunks()[0].publication;
    let original: Vec<u8> = (0..16384).map(|i| (i % 251) as u8).collect();
    let fragments = fragment_group(key, &original, limits).unwrap();
    assert_eq!(fragments.len(), 15);
    let mut assembler = ByteAssembler::new(ConnectionEpoch(1), limits).unwrap();
    let mut complete = None;
    for (i, fragment) in fragments.into_iter().rev().enumerate() {
        let wire = fragment.encode(limits).unwrap();
        assert!(wire.len() <= 1139); // 1100 + fixed39; transport overhead is additional.
        let decoded = ByteFragment::decode(&wire, limits).unwrap();
        assert_eq!(decoded, fragment);
        complete = assembler.receive(decoded, i as u64).unwrap().or(complete);
    }
    assert_eq!(complete.unwrap().as_bytes(), original.as_slice());
    assert_eq!(assembler.staged_bytes(), 0);
}
#[test]
fn byte_decode_rejects_lengths_before_allocating_and_assembly_expires() {
    let limits = byte_limits();
    let fragments = fragment_group(chunks()[0].publication, &vec![7; 3000], limits).unwrap();
    let wire = fragments[0].encode(limits).unwrap();
    for length in 0..wire.len() {
        assert!(ByteFragment::decode(&wire[..length], limits).is_err());
    }
    let mut malicious = wire.clone();
    malicious[29..33].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ByteFragment::decode(&malicious, limits).is_err());
    malicious = wire.clone();
    malicious[33..35].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(ByteFragment::decode(&malicious, limits).is_err());
    let mut assembler = ByteAssembler::new(ConnectionEpoch(1), limits).unwrap();
    assembler.receive(fragments[0].clone(), 0).unwrap();
    assembler.receive(fragments[0].clone(), 1000).unwrap();
    let mut conflict = fragments[0].clone();
    conflict.payload[0] ^= 1;
    assert_eq!(
        assembler.receive(conflict, 1001).unwrap_err(),
        ReplicationError::Conflict
    );
    assert_eq!(assembler.expire(2000).unwrap(), 1);
    assert_eq!(
        assembler.receive(fragments[1].clone(), 2001).unwrap_err(),
        ReplicationError::Expired
    );
    assert_eq!(assembler.staged_bytes(), 0);
}
#[test]
fn complete_byte_group_decodes_and_publishes_transactionally() {
    let mut group = chunks()[0].clone();
    group.count = 1;
    group.members.push(full(2, 1, 1));
    group.members[0].payload = vec![9; 4000];
    let limits = GroupLimits::default();
    let bytes = encode_group(&group, limits, |p| Ok(p.clone())).unwrap();
    assert_eq!(
        decode_group(&bytes, limits, |p| Ok(p.to_vec())).unwrap(),
        group
    );
    let mut bytes_assembler = ByteAssembler::new(ConnectionEpoch(1), byte_limits()).unwrap();
    let fragments = fragment_group(group.publication, &bytes, byte_limits()).unwrap();
    let mut client = client();
    let mut groups = assembler();
    let mut completed = None;
    for fragment in fragments {
        completed = bytes_assembler.receive(fragment, 0).unwrap().or(completed);
    }
    assert!(client.state(entity(1)).is_none());
    let result = completed
        .unwrap()
        .decode_and_publish(
            &mut groups,
            &mut client,
            0,
            |bytes| decode_group(bytes, limits, |p| Ok(p.to_vec())),
            |_| true,
        )
        .unwrap();
    assert!(matches!(result, GroupProgress::Published(r) if r.len() == 2));
    assert_eq!(client.state(entity(1)).unwrap().payload.len(), 4000);
    let mut corrupted = bytes.clone();
    corrupted[37..39].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode_group(&corrupted, limits, |p| Ok(p.to_vec())).is_err());
    for length in [0, 1, 38, 39, bytes.len() - 1] {
        assert!(decode_group(&bytes[..length], limits, |p| Ok(p.to_vec())).is_err());
    }
}

#[test]
fn server_projection_requires_opaque_current_connection_grant() {
    use crate::interest::*;
    use std::collections::BTreeSet;
    struct Policy;
    impl ConnectionAuthorizer for Policy {
        fn authorize(&self, _: ConnectionId, _: &ConnectionView) -> bool {
            true
        }
    }
    impl DisclosurePolicy for Policy {
        fn revision(&self) -> PolicyRevision {
            PolicyRevision(1)
        }
        fn representation(
            &self,
            _: ConnectionId,
            _: &EntityRegistration,
        ) -> Option<RepresentationGrant> {
            Some(RepresentationGrant {
                schema_id: 1,
                revision: RepresentationRevision(1),
                fields: 1,
                prediction_allowed: false,
            })
        }
        fn permits_dependency(&self, _: ConnectionId, _: EntityId, _: EntityId) -> bool {
            false
        }
    }
    let mut graph = Graph::new(Limits::default()).unwrap();
    graph
        .set_connection(
            ServerTick(1),
            ConnectionId(10),
            ConnectionView {
                observers: vec![ObserverGrant {
                    position: [0.0; 3],
                    scene_revision: SceneRevision(1),
                }],
                semantic_grants: BTreeSet::new(),
                ready_scenes: BTreeSet::from([SceneRevision(1)]),
            },
            &Policy,
        )
        .unwrap();
    graph
        .apply(
            ServerTick(1),
            Change::Upsert(EntityRegistration {
                id: entity(1),
                bounds: Bounds {
                    center: [0.0; 3],
                    radius: 1.0,
                },
                cull_radius: 8.0,
                routes: Routes {
                    spatial: Some(SpatialRoute::Dynamic),
                    ..Default::default()
                },
                visibility: Visibility::Public,
                dependencies: BTreeSet::new(),
                state_version: StateVersion(7),
                scene_revision: SceneRevision(1),
                dormant: false,
            }),
        )
        .unwrap();
    let eligible = graph
        .prepare(ReplicationFrame(1), ServerTick(1), PolicyRevision(1))
        .unwrap()
        .gather(
            ConnectionId(10),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &Policy,
        )
        .unwrap();
    let mut server = ServerScopes::new(
        ConnectionId(10),
        ConnectionEpoch(1),
        eligible.world_revision(),
        PolicyRevision(1),
        ScopeLimits::default(),
    )
    .unwrap();
    server
        .offer(&eligible, entity(1), false, |grant| {
            assert_eq!(grant.fields, 1);
            vec![5]
        })
        .unwrap();
    assert_eq!(server.pending(entity(1)).unwrap().version, StateVersion(7));
    assert_eq!(server.pending(entity(1)).unwrap().end_tick, ServerTick(1));
    assert_eq!(
        server.offer(&eligible, entity(2), false, |_| panic!(
            "denied projection ran"
        )),
        Err(ReplicationError::Unauthorized)
    );
    server
        .advance_barrier(eligible.world_revision() + 1, PolicyRevision(1))
        .unwrap();
    assert_eq!(
        server.offer(&eligible, entity(1), false, |_| panic!(
            "stale projection ran"
        )),
        Err(ReplicationError::Unauthorized)
    );
    let mut other: ServerScopes<Vec<u8>> = ServerScopes::new(
        ConnectionId(11),
        ConnectionEpoch(1),
        eligible.world_revision(),
        PolicyRevision(1),
        ScopeLimits::default(),
    )
    .unwrap();
    assert_eq!(
        other.offer(&eligible, entity(1), false, |_| panic!(
            "wrong peer projection ran"
        )),
        Err(ReplicationError::Unauthorized)
    );
}

#[test]
fn newer_same_topology_full_group_replaces_partial_bytes_and_publishes_atomically() {
    let limits = byte_limits();
    let mut first = chunks()[0].clone();
    first.count = 1;
    first.members = vec![full(1, 1, 1), full(2, 1, 1)];
    first.members[0].payload = vec![1; 4000];
    let first_bytes = encode_group(
        &first,
        GroupLimits::default(),
        |payload| Ok(payload.clone()),
    )
    .unwrap();
    let old_fragments = fragment_group(first.publication, &first_bytes, limits).unwrap();
    let mut next = first.clone();
    next.publication.snapshot = SnapshotId(2);
    next.end_tick = ServerTick(2);
    next.members = vec![full(1, 1, 2), full(2, 1, 2)];
    next.members[0].payload = vec![2; 4000];
    let next_bytes =
        encode_group(&next, GroupLimits::default(), |payload| Ok(payload.clone())).unwrap();
    let fragments = fragment_group(next.publication, &next_bytes, limits).unwrap();
    let mut bytes = ByteAssembler::new(ConnectionEpoch(1), limits).unwrap();
    let mut client = client();
    let mut groups = assembler();
    bytes.receive(old_fragments[0].clone(), 0).unwrap();
    assert!(client.state(entity(1)).is_none());
    let mut complete = None;
    for (i, fragment) in fragments.into_iter().enumerate() {
        complete = bytes.receive(fragment, 10 + i as u64).unwrap().or(complete);
        assert!(client.state(entity(1)).is_none());
    }
    assert!(matches!(
        complete
            .unwrap()
            .decode_and_publish(
                &mut groups,
                &mut client,
                100,
                |raw| decode_group(raw, GroupLimits::default(), |payload| Ok(payload.to_vec())),
                |_| true
            )
            .unwrap(),
        GroupProgress::Published(_)
    ));
    assert_eq!(client.state(entity(1)).unwrap().version, StateVersion(2));
    assert_eq!(client.state(entity(2)).unwrap().version, StateVersion(2));
    assert_eq!(
        bytes.receive(old_fragments[1].clone(), 101).unwrap_err(),
        ReplicationError::Obsolete(ObsoleteReason::Publication)
    );
    assert_eq!(bytes.staged_bytes(), 0);
}

#[test]
fn partial_byte_supersession_keeps_original_deadline_and_requires_reset_after_expiry() {
    let limits = byte_limits();
    let key = chunks()[0].publication;
    let mut bytes = ByteAssembler::new(ConnectionEpoch(1), limits).unwrap();
    for (snapshot, time) in [(1, 0), (2, 1000), (3, 1999)] {
        let key = GroupPublication {
            snapshot: SnapshotId(snapshot),
            ..key
        };
        let fragments = fragment_group(key, &vec![snapshot as u8; 3000], limits).unwrap();
        bytes.receive(fragments[0].clone(), time).unwrap();
        assert_eq!(bytes.staged_bytes(), limits.fragment_payload_bytes);
    }
    assert_eq!(bytes.expire(2000).unwrap(), 1);
    let newer = GroupPublication {
        snapshot: SnapshotId(4),
        ..key
    };
    let fragments = fragment_group(newer, &vec![4; 3000], limits).unwrap();
    assert_eq!(
        bytes.receive(fragments[0].clone(), 2001).unwrap_err(),
        ReplicationError::Expired
    );
    assert_eq!(bytes.staged_bytes(), 0);
    bytes.reset(ConnectionEpoch(2)).unwrap();
    let reset_key = GroupPublication {
        connection: ConnectionEpoch(2),
        ..newer
    };
    let fragments = fragment_group(reset_key, &vec![4; 3000], limits).unwrap();
    for fragment in fragments {
        bytes.receive(fragment, 2002).unwrap();
    }
    assert_eq!(bytes.staged_bytes(), 0);
}

#[test]
fn complete_new_snapshot_replaces_partial_member_chunks_without_mixing_versions() {
    let mut client = client();
    let mut assembler = assembler();
    let old = chunks();
    assembler
        .receive(old[0].clone(), 0, &mut client, |_| true)
        .unwrap();
    let mut next = old[0].clone();
    next.publication.snapshot = SnapshotId(2);
    next.end_tick = ServerTick(2);
    next.index = 0;
    next.count = 1;
    next.members = vec![full(1, 1, 2), full(2, 1, 2)];
    assert!(matches!(
        assembler.receive(next, 100, &mut client, |_| true).unwrap(),
        GroupProgress::Published(_)
    ));
    assert_eq!(client.state(entity(1)).unwrap().version, StateVersion(2));
    assert_eq!(client.state(entity(2)).unwrap().version, StateVersion(2));
    assert_eq!(
        assembler.receive(old[1].clone(), 101, &mut client, |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Publication))
    );
}

#[test]
fn obsolete_full_requires_valid_payload_and_consistent_watermarks() {
    let mut client = client();
    let current = full(1, 1, 3);
    client.apply_full(current.clone(), |_| true).unwrap();
    assert_eq!(
        client.apply_full(full(1, 1, 2), |_| true),
        Err(ReplicationError::Obsolete(ObsoleteReason::Publication))
    );
    assert_eq!(
        client.apply_full(full(1, 1, 2), |_| false),
        Err(ReplicationError::InvalidPayload)
    );
    let mut equal = current.clone();
    equal.payload = vec![99];
    let mut old_new_version = full(1, 1, 2);
    old_new_version.version = StateVersion(4);
    let mut old_new_tick = full(1, 1, 2);
    old_new_tick.end_tick = ServerTick(4);
    let mut new_old_version = full(1, 1, 4);
    new_old_version.version = StateVersion(2);
    let mut new_old_tick = full(1, 1, 4);
    new_old_tick.end_tick = ServerTick(2);
    for invalid in [
        equal,
        old_new_version,
        old_new_tick,
        new_old_version,
        new_old_tick,
    ] {
        assert_eq!(
            client.apply_full(invalid, |_| true),
            Err(ReplicationError::Conflict)
        );
        assert_eq!(client.state(entity(1)), Some(&current));
    }
}

#[test]
fn byte_and_group_receive_clock_regression_remains_hard_stale() {
    let fragments = fragment_group(chunks()[0].publication, &vec![1; 3000], byte_limits()).unwrap();
    let mut bytes = ByteAssembler::new(ConnectionEpoch(1), byte_limits()).unwrap();
    bytes.receive(fragments[0].clone(), 10).unwrap();
    assert!(matches!(
        bytes.receive(fragments[1].clone(), 9),
        Err(ReplicationError::Stale)
    ));
    let mut groups = assembler();
    let mut client = client();
    groups
        .receive(chunks()[0].clone(), 10, &mut client, |_| true)
        .unwrap();
    assert_eq!(
        groups.receive(chunks()[1].clone(), 9, &mut client, |_| true),
        Err(ReplicationError::Stale)
    );
    assert!(client.state(entity(1)).is_none());
}

#[test]
fn same_revision_group_snapshot_tick_disagreement_is_hard_conflict() {
    let mut client = client();
    let mut groups = assembler();
    let mut initial = chunks()[0].clone();
    initial.publication.snapshot = SnapshotId(3);
    initial.end_tick = ServerTick(3);
    initial.count = 1;
    initial.members = vec![full(1, 1, 3), full(2, 1, 3)];
    groups
        .receive(initial.clone(), 1, &mut client, |_| true)
        .unwrap();
    for (snapshot, tick) in [(2, 4), (4, 2), (3, 4)] {
        let mut invalid = initial.clone();
        invalid.publication.snapshot = SnapshotId(snapshot);
        invalid.end_tick = ServerTick(tick);
        for member in &mut invalid.members {
            member.snapshot = SnapshotId(snapshot);
            member.end_tick = ServerTick(tick);
        }
        assert_eq!(
            groups.receive(invalid, 2, &mut client, |_| true),
            Err(ReplicationError::Conflict)
        );
        assert_eq!(client.state(entity(1)), Some(&full(1, 1, 3)));
        assert_eq!(client.state(entity(2)), Some(&full(2, 1, 3)));
    }
}
