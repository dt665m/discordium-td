use super::*;

/// A packet as the game would build it: per entity, the base it names and the
/// value. The test "codec" carries the whole value but checks the receiver
/// resolved the same base the sender used.
#[derive(Clone, Debug)]
struct Packet {
    seq: u32,
    entries: Vec<(u8, Option<(u32, u32)>, u32)>,
    removals: Vec<u8>,
}

fn send(server: &mut ServerStream<u8, u32>, states: &[(u8, u32)]) -> Packet {
    let seq = server.begin().unwrap();
    let mut entries = Vec::new();
    for &(key, value) in states {
        let base = server.base(key).map(|(seq, state)| (seq, *state));
        server.include(key, value).unwrap();
        entries.push((key, base, value));
    }
    let removals: Vec<_> = server.pending_removals().collect();
    for key in &removals {
        server.include_removal(*key).unwrap();
    }
    server.finish().unwrap();
    Packet {
        seq,
        entries,
        removals,
    }
}

fn receive(
    client: &mut ClientStream<u8, u32>,
    packet: &Packet,
) -> Result<StreamChanges<u8>, StreamError> {
    if !client.accepts(packet.seq) {
        return Err(StreamError::Malformed);
    }
    let mut decoded = Vec::new();
    for (key, base, value) in &packet.entries {
        match client.base(*key, base.map(|(seq, _)| seq), packet.seq)? {
            Base::Superseded => continue,
            Base::Whole => assert_eq!(*base, None),
            Base::Delta(held) => assert_eq!(Some(*held), base.map(|(_, state)| state)),
        }
        decoded.push((*key, *value));
    }
    client.accept(packet.seq, decoded, &packet.removals)
}

#[test]
fn acknowledgement_masks_name_each_packet_once() {
    let mut client = ClientStream::<u8, u32>::new(8);
    for seq in [1, 2, 4, 66, 65] {
        client.accept(seq, vec![], &[]).unwrap();
    }
    let ack = client.ack();
    assert_eq!(ack.latest, 66);
    assert!(ack.covers(66) && ack.covers(65));
    // 4 and 2 are within 64 of 66; 1 is not, so the mask cannot claim it.
    assert!(ack.covers(4) && ack.covers(2) && !ack.covers(5) && !ack.covers(1));
    assert!(!ack.covers(67) && !ack.covers(0));
    // A repeat or a packet older than the window is refused.
    assert!(!client.accepts(65) && !client.accepts(2) && !client.accepts(1));
    assert!(client.accepts(3) && client.accepts(64));
}

#[test]
fn entities_are_sent_whole_until_acknowledged_then_against_the_newest_ack() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 10)]);
    let second = send(&mut server, &[(1, 11)]);
    assert_eq!(first.entries[0].1, None);
    assert_eq!(second.entries[0].1, None, "nothing acknowledged yet");
    receive(&mut client, &first).unwrap();
    receive(&mut client, &second).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let third = send(&mut server, &[(1, 12)]);
    assert_eq!(third.entries[0].1, Some((second.seq, 11)));
    let changes = receive(&mut client, &third).unwrap();
    assert_eq!(changes.updated, vec![1]);
    assert_eq!(client.current(1), Some((third.seq, &12)));
}

#[test]
fn a_lost_packet_is_covered_by_encoding_against_the_older_acknowledgement() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 10), (2, 20)]);
    receive(&mut client, &first).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let lost = send(&mut server, &[(1, 11), (2, 21)]);
    let _ = lost;
    let next = send(&mut server, &[(1, 12)]);
    // Still against the first packet: the lost one was never acknowledged.
    assert_eq!(next.entries[0].1, Some((first.seq, 10)));
    receive(&mut client, &next).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let after = send(&mut server, &[(1, 13), (2, 22)]);
    assert_eq!(after.entries[0].1, Some((next.seq, 12)));
    assert_eq!(after.entries[1].1, Some((first.seq, 20)));
    receive(&mut client, &after).unwrap();
    assert_eq!(client.current(2).map(|(_, v)| *v), Some(22));
}

#[test]
fn lost_acknowledgements_fall_back_to_whole_states_outside_the_window() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 0)]);
    receive(&mut client, &first).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let mut packet = first;
    for value in 1..=STREAM_WINDOW {
        packet = send(&mut server, &[(1, value)]);
        receive(&mut client, &packet).unwrap();
        // Every acknowledgement is lost on the way back.
    }
    assert_eq!(packet.entries[0].1, None, "the base aged out of the window");
    // The receiver still decodes whatever the sender chose.
    server.acknowledge(client.ack()).unwrap();
    let resumed = send(&mut server, &[(1, 100)]);
    assert_eq!(resumed.entries[0].1, Some((packet.seq, STREAM_WINDOW)));
    receive(&mut client, &resumed).unwrap();
}

#[test]
fn a_missing_base_rejects_the_packet_without_acknowledging_it() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 10)]);
    receive(&mut client, &first).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let delta = send(&mut server, &[(1, 11)]);
    // A receiver that lost its state (or a forged base) refuses the packet.
    let mut fresh = ClientStream::<u8, u32>::new(8);
    assert_eq!(receive(&mut fresh, &delta), Err(StreamError::MissingBase));
    assert_eq!(fresh.ack(), StreamAck::default());
    assert_eq!(
        client.base(1, Some(delta.seq), delta.seq).map(|_| ()),
        Err(StreamError::Malformed)
    );
}

#[test]
fn removals_repeat_until_acknowledged_and_never_resurrect() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let created = send(&mut server, &[(1, 10), (2, 20)]);
    receive(&mut client, &created).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let stale = send(&mut server, &[(2, 21)]);
    server.remove(2);
    let lost = send(&mut server, &[(1, 11)]);
    assert_eq!(lost.removals, vec![2]);
    let repeated = send(&mut server, &[(1, 12)]);
    assert_eq!(repeated.removals, vec![2], "repeated until acknowledged");
    let changes = receive(&mut client, &repeated).unwrap();
    assert_eq!(changes.removed, vec![2]);
    assert!(client.current(2).is_none());
    // The update sent before the removal arrives late and is ignored.
    let changes = receive(&mut client, &stale).unwrap();
    assert!(changes.updated.is_empty());
    assert!(client.current(2).is_none());
    server.acknowledge(client.ack()).unwrap();
    let quiet = send(&mut server, &[(1, 13)]);
    assert!(quiet.removals.is_empty());
    assert!(!server.contains(2));
}

#[test]
fn a_returning_entity_starts_over_whole() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let created = send(&mut server, &[(1, 10)]);
    receive(&mut client, &created).unwrap();
    server.acknowledge(client.ack()).unwrap();
    server.remove(1);
    let removal = send(&mut server, &[]);
    let back = send(&mut server, &[(1, 30)]);
    assert_eq!(back.entries[0].1, None);
    assert!(back.removals.is_empty(), "inclusion supersedes the removal");
    // Delivered out of order: the newer state wins regardless.
    receive(&mut client, &back).unwrap();
    let changes = receive(&mut client, &removal).unwrap();
    assert!(changes.removed.is_empty());
    assert_eq!(client.current(1), Some((back.seq, &30)));
}

#[test]
fn reordered_packets_keep_the_newest_state_and_their_bases() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 10)]);
    receive(&mut client, &first).unwrap();
    server.acknowledge(client.ack()).unwrap();
    let older = send(&mut server, &[(1, 11)]);
    let newer = send(&mut server, &[(1, 12)]);
    receive(&mut client, &newer).unwrap();
    let changes = receive(&mut client, &older).unwrap();
    assert!(changes.updated.is_empty());
    assert_eq!(client.current(1), Some((newer.seq, &12)));
    // Both are acknowledged; the newer becomes the base.
    server.acknowledge(client.ack()).unwrap();
    let next = send(&mut server, &[(1, 13)]);
    assert_eq!(next.entries[0].1, Some((newer.seq, 12)));
}

#[test]
fn acknowledgements_of_unsent_packets_are_refused() {
    let mut server = ServerStream::<u8, u32>::new(8);
    send(&mut server, &[(1, 1)]);
    assert_eq!(
        server.acknowledge(StreamAck {
            latest: 2,
            received: 0
        }),
        Err(StreamError::FutureAck)
    );
}

#[test]
fn a_cancelled_packet_records_nothing() {
    let mut server = ServerStream::<u8, u32>::new(8);
    server.begin().unwrap();
    server.include(1, 5).unwrap();
    server.cancel();
    assert!(!server.contains(1));
    let packet = send(&mut server, &[(1, 6)]);
    assert_eq!(packet.entries[0].1, None);
    assert_eq!(server.begin(), Ok(packet.seq + 1));
}

/// Random loss and reordering in both directions, with entities coming and
/// going, always converges to the sender's current set once packets get
/// through, and never hands the receiver a base it does not hold.
#[test]
fn random_loss_and_reordering_converge() {
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |modulo: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % modulo
    };
    let mut server = ServerStream::<u8, u32>::new(32);
    let mut client = ClientStream::<u8, u32>::new(32);
    let mut live = std::collections::BTreeMap::<u8, u32>::new();
    let mut delayed: Vec<Packet> = Vec::new();
    for round in 0..2000u32 {
        // Mutate the world.
        let key = next(12) as u8;
        if next(10) == 0 {
            if live.remove(&key).is_some() {
                server.remove(key);
            }
        } else {
            live.insert(key, round);
        }
        let states: Vec<_> = live.iter().map(|(k, v)| (*k, *v)).collect();
        let packet = send(&mut server, &states);
        let lossy = round < 1900;
        match next(10) {
            0 | 1 if lossy => {}
            2 if lossy => delayed.push(packet),
            _ => {
                receive(&mut client, &packet).unwrap();
            }
        }
        if lossy
            && next(5) == 0
            && let Some(old) = delayed.pop()
        {
            // Late packets may be refused (base aged out) but never corrupt.
            let _ = receive(&mut client, &old);
        }
        if !lossy || next(4) != 0 {
            server.acknowledge(client.ack()).unwrap();
        }
    }
    let held: std::collections::BTreeMap<_, _> = client
        .keys()
        .map(|key| (key, *client.current(key).unwrap().1))
        .collect();
    assert_eq!(held, live);
}

#[test]
fn in_flight_states_are_lost_only_when_a_later_packet_is_acknowledged() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = send(&mut server, &[(1, 10)]);
    assert_eq!(server.in_flight(1), Some((first.seq, &10)));
    assert!(!server.lost(first.seq), "nothing has been reported yet");
    let second = send(&mut server, &[(2, 20)]);
    receive(&mut client, &second).unwrap();
    server.acknowledge(client.ack()).unwrap();
    assert!(server.lost(first.seq));
    assert!(!server.lost(second.seq));
    assert_eq!(
        server.in_flight(2),
        None,
        "acknowledged states become the base"
    );
}

#[test]
fn removal_and_addition_in_one_packet_must_fit_capacity() {
    let mut client = ClientStream::<u8, u32>::new(1);
    client.accept(1, vec![(1, 10)], &[]).unwrap();
    assert!(
        client.accept(2, vec![(2, 20)], &[1]).is_ok(),
        "a packet leaving exactly one live entity fits capacity one"
    );
}

#[test]
fn superseded_removals_do_not_free_capacity_or_partially_commit() {
    let mut client = ClientStream::<u8, u32>::new(1);
    client.accept(3, vec![(1, 30)], &[]).unwrap();
    let ack = client.ack();
    assert_eq!(
        client.accept(2, vec![(2, 20)], &[1]),
        Err(StreamError::Capacity)
    );
    assert_eq!(client.ack(), ack);
    assert_eq!(client.current(1), Some((3, &30)));
    assert!(client.current(2).is_none());
}

#[test]
fn superseded_entries_do_not_consume_capacity() {
    let mut client = ClientStream::<u8, u32>::new(1);
    client.accept(3, vec![(2, 20)], &[1]).unwrap();
    let changes = client.accept(2, vec![(1, 10)], &[]).unwrap();
    assert!(changes.updated.is_empty());
    assert!(client.current(1).is_none());
    assert_eq!(client.current(2), Some((3, &20)));
    assert!(client.ack().covers(2));
}

#[test]
fn old_removal_ack_must_not_cancel_a_new_removal() {
    let mut server = ServerStream::<u8, u32>::new(8);
    let mut client = ClientStream::<u8, u32>::new(8);
    let first = server.begin().unwrap();
    server.include(1, 10).unwrap();
    server.finish().unwrap();
    client.accept(first, vec![(1, 10)], &[]).unwrap();
    server.acknowledge(client.ack()).unwrap();

    server.remove(1);
    let gone = server.begin().unwrap();
    server.include_removal(1).unwrap();
    server.finish().unwrap();
    client.accept(gone, vec![], &[1]).unwrap();
    let delayed_ack = client.ack();

    let back = server.begin().unwrap();
    server.include(1, 30).unwrap();
    server.finish().unwrap();
    client.accept(back, vec![(1, 30)], &[]).unwrap();

    server.remove(1);
    assert_eq!(server.pending_removals().collect::<Vec<_>>(), vec![1]);
    server.acknowledge(delayed_ack).unwrap();
    assert_eq!(
        server.pending_removals().collect::<Vec<_>>(),
        vec![1],
        "the ACK only proves the first departure; client still holds the reentry"
    );
}

#[test]
fn reordered_removal_must_not_move_tombstone_backwards() {
    let mut client = ClientStream::<u8, u32>::new(8);
    client.accept(1, vec![(1, 10)], &[]).unwrap();
    // Server sent: remove at 2, reenter at 3, remove again at 4.
    client.accept(4, vec![], &[1]).unwrap();
    client.accept(2, vec![], &[1]).unwrap();
    client.accept(3, vec![(1, 30)], &[]).unwrap();
    assert!(
        client.current(1).is_none(),
        "packet 3 must not resurrect the entity removed by packet 4"
    );
}
