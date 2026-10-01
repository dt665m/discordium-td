use super::*;
use dreamwake_sim::replication::{Replica, ReplicaState};

pub(super) fn control(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    value: &live::ServerControl,
) -> Result<(), String> {
    let sequence = peer.next_frame()?;
    let bytes = live::encode_control(value, peer.welcome.stream.epoch, sequence, peer.limit())
        .map_err(failure)?;
    if !server.can_send_message(id, wire::CONTROL_CHANNEL, bytes.len()) {
        return Err("Control queue capacity".into());
    }
    peer.egress.control_messages += 1;
    peer.egress.control_bytes += bytes.len() as u64;
    server.send_message(id, wire::CONTROL_CHANNEL, bytes);
    Ok(())
}
pub(super) fn welcome(server: &mut RenetServer, id: u64, peer: &mut Peer) -> Result<(), String> {
    if !peer.welcome_pending {
        return Ok(());
    }
    let bytes = live::encode_welcome(&peer.welcome).map_err(failure)?;
    if !server.can_send_message(id, wire::CONTROL_CHANNEL, bytes.len()) {
        return Err("Welcome queue capacity".into());
    }
    peer.egress.control_messages += 1;
    peer.egress.control_bytes += bytes.len() as u64;
    server.send_message(id, wire::CONTROL_CHANNEL, bytes);
    peer.welcome_pending = false;
    Ok(())
}
pub(super) fn disconnect_failed(server: &mut RenetServer, authority: &mut DreamAuthority) {
    for id in std::mem::take(&mut authority.failed) {
        server.disconnect(id);
        authority.disconnect(id);
    }
}

/// Remote actors within this distance of the player update every publication
/// and farther ones every other, as Unreal's replication graph spends less on
/// what matters less. The client's remote-pose buffer covers either interval.
const NEAR_ACTOR_METRES: f64 = 16.0;
/// Per-packet transport framing beyond the state frame: Renet's packet header
/// and netcode's prefix, sequence and MAC.
const TRANSPORT_OVERHEAD_BYTES: u64 = 40;

/// One entity's candidate state for this publication, in send order.
struct Candidate {
    order: (u8, u64),
    entity: EntityId,
    state: ReplicaState,
    owner: bool,
}

/// Build and send this publication's state packets for one connection.
///
/// An entity is sent only when its state differs from the newest state the
/// client acknowledged, unless that exact state is already on its way and not
/// yet reported lost. A lost packet therefore costs nothing until the client
/// reports it, and then the next packet carries every changed field with its
/// current value. Packets are filled owner first, then the match summary, then
/// actors nearest first, and stop where the connection's egress allowance
/// ends: what does not fit waits for the next publication rather than queueing
/// behind the transport (Unreal's `IsNetReady`).
#[allow(clippy::too_many_arguments)]
fn peer_state(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    replication: &mut DreamReplication,
    outcomes: &mut outcomes::Outcomes,
    tick: ServerTick,
    allowance: Option<renet_cross::EgressAllowance>,
) -> Result<(), String> {
    welcome(server, id, peer)?;
    // Controls have first use of each Renet packetization pass.
    action_outcomes(server, id, peer, outcomes)?;
    if !peer.ready {
        return Ok(());
    }
    let gather = replication
        .gather_peer(
            id,
            peer.inbox.finalized_through(),
            &peer
                .inbox
                .input_continuity()
                .map(|input| input.held.held_only()),
        )
        .map_err(failure)?;
    for entity in gather.removed {
        peer.stream.remove(entity);
    }
    peer.publications += 1;
    let mut candidates = Vec::with_capacity(gather.entities.len());
    for entry in gather.entities {
        let owner = matches!(entry.replica, Replica::Owner(_));
        let global = matches!(entry.replica, Replica::Global(_));
        // A joining client receives its own hero and the match summary
        // first; the rest of the world follows once it participates.
        if !peer.active && !owner && !global {
            continue;
        }
        let near = entry
            .distance
            .is_none_or(|metres| metres <= NEAR_ACTOR_METRES);
        if !owner && !global && !near && (peer.publications + entry.entity.index) % 2 != 0 {
            continue;
        }
        let state = ReplicaState::capture(&entry.replica).map_err(failure)?;
        if peer
            .stream
            .base(entry.entity)
            .is_some_and(|(_, base)| *base == state)
        {
            continue;
        }
        if peer
            .stream
            .in_flight(entry.entity)
            .is_some_and(|(seq, sent)| *sent == state && !peer.stream.lost(seq))
        {
            continue;
        }
        let order = if owner {
            (0, 0)
        } else if global {
            (1, 0)
        } else {
            (2, (entry.distance.unwrap_or(0.0) * 100.0) as u64)
        };
        candidates.push(Candidate {
            order,
            entity: entry.entity,
            state,
            owner,
        });
    }
    candidates.sort_by_key(|candidate| (candidate.order, candidate.entity));
    let mut budget = allowance.map(|allowance| {
        (
            allowance.available_data_bytes,
            TRANSPORT_OVERHEAD_BYTES + allowance.ip_udp_overhead,
        )
    });
    let status = live::OwnerStatus {
        finalized: peer.inbox.finalized_through(),
        menu_applied: peer.menu_applied,
        arrival: peer.arrival,
    };
    let target = live::APPLICATION_FRAME_BYTES;
    let limit = engine_net::codec::FrameLimit::new(live::MAX_STATE_FRAME_BYTES).map_err(failure)?;
    let mut pending = candidates.into_iter().peekable();
    let mut first = true;
    loop {
        let removals: Vec<_> = if first {
            peer.stream
                .pending_removals()
                .take(live::MAX_STATE_REMOVALS)
                .collect()
        } else {
            Vec::new()
        };
        if pending.peek().is_none() && removals.is_empty() && !first {
            break;
        }
        let seq = peer.stream.begin().map_err(failure)?;
        let mut packet = live::StatePacket {
            seq,
            tick,
            status,
            entries: Vec::new(),
            removed: Vec::new(),
        };
        for entity in removals {
            peer.stream.include_removal(entity).map_err(failure)?;
            packet.removed.push(entity);
        }
        let mut size = packet.encoded_len().map_err(failure)?;
        let mut carries_owner = false;
        while let Some(candidate) = pending.peek() {
            let base = peer
                .stream
                .base(candidate.entity)
                .filter(|(_, base)| candidate.state.deltas_from(base));
            let mut body = Vec::new();
            candidate
                .state
                .encode(base.map(|(_, base)| base), &mut body)
                .map_err(failure)?;
            // Entity identity, base distance and length prefix.
            let entry_size = body.len() + 16;
            if !packet.entries.is_empty()
                && (size + entry_size > target || packet.entries.len() == live::MAX_STATE_ENTRIES)
            {
                break;
            }
            let candidate = pending.next().expect("peeked");
            carries_owner |= candidate.owner;
            packet.entries.push(live::StateEntry {
                entity: candidate.entity,
                base: base.map(|(seq, _)| seq),
                body,
            });
            peer.stream
                .include(candidate.entity, candidate.state)
                .map_err(failure)?;
            size += entry_size;
        }
        let bytes =
            live::encode_state(&packet, peer.welcome.stream.epoch, limit).map_err(failure)?;
        let cost = budget.map(|(_, overhead)| bytes.len() as u64 + overhead);
        if budget
            .zip(cost)
            .is_some_and(|((left, _), cost)| cost > left)
            || !server.can_send_message(id, wire::STATE_CHANNEL, bytes.len())
        {
            // Not ready for more this tick: drop the packet's bookkeeping and
            // let these entities go out in a later publication.
            peer.stream.cancel();
            peer.egress.allowance_limited += 1;
            break;
        }
        if let (Some((left, _)), Some(cost)) = (budget.as_mut(), cost) {
            *left -= cost;
        }
        peer.stream.finish().map_err(failure)?;
        if carries_owner {
            peer.combat.sent(seq, tick, tick);
        }
        peer.egress.state_packets += 1;
        peer.egress.state_bytes += bytes.len() as u64;
        peer.egress.state_entries += packet.entries.len() as u64;
        server.send_message(id, wire::STATE_CHANNEL, bytes);
        // A feedback sample is reported once; the counters repeat.
        peer.arrival.sample = None;
        first = false;
    }
    if peer.ready && !peer.active && !peer.activate_pending && peer.holds_owner() {
        peer.activate_pending = true;
    }
    Ok(())
}

fn outcome_batch(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    records: Vec<live::ActionOutcome>,
) -> Result<(), String> {
    let batch = peer
        .next_outcome_batch
        .checked_add(1)
        .ok_or("Outcome delivery sequence exhausted")?;
    control(
        server,
        id,
        peer,
        &live::ServerControl::ActionOutcomes {
            batch,
            records: BoundedVec::new(records).map_err(failure)?,
        },
    )?;
    peer.outcome_batches.issue(batch).map_err(failure)?;
    peer.next_outcome_batch = batch;
    Ok(())
}
fn action_outcomes(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    outcomes: &mut super::outcomes::Outcomes,
) -> Result<(), String> {
    let owner = peer.welcome.player.get();
    while !peer.outcome_batches.is_full() {
        if !peer.outcome_lookups.is_empty() {
            let keys: Vec<_> = peer.outcome_lookups.iter().take(4).copied().collect();
            let mut found = Vec::new();
            let mut unavailable = Vec::new();
            for key in &keys {
                if let Some(value) = outcomes.lookup(owner, *key) {
                    found.push(value);
                } else {
                    unavailable.push(*key);
                }
            }
            if !unavailable.is_empty() {
                control(
                    server,
                    id,
                    peer,
                    &live::ServerControl::OutcomeUnavailable {
                        keys: BoundedVec::new(unavailable).map_err(failure)?,
                    },
                )?;
            }
            if !found.is_empty() {
                outcome_batch(server, id, peer, found.clone())?;
                outcomes.reported(owner, found.iter().map(|value| value.key))?;
            }
            for key in keys {
                peer.outcome_lookups.remove(&key);
            }
            continue;
        }
        let mut candidates: Vec<_> = outcomes
            .normal
            .unreported(owner, peer.welcome.stream.epoch)
            .take(4)
            .map(super::outcomes::wire)
            .chain(
                outcomes
                    .overload
                    .unreported(owner, peer.welcome.stream.epoch)
                    .take(4)
                    .map(super::outcomes::wire),
            )
            .collect();
        candidates.sort_by_key(|value| value.sequence);
        candidates.truncate(4);
        if candidates.is_empty() {
            break;
        }
        outcome_batch(server, id, peer, candidates.clone())?;
        outcomes.reported(owner, candidates.iter().map(|value| value.key))?;
    }
    Ok(())
}
pub(super) fn states(server: &mut RenetServer, authority: &mut DreamAuthority) {
    while let Some((id, text)) = authority.notices.pop_front() {
        if let Some(peer) = authority.peers.get_mut(&id) {
            let value = live::ServerControl::Notice {
                utf8: BoundedVec::new(text.as_bytes().to_vec()).expect("bounded static notice"),
            };
            if control(server, id, peer, &value).is_err() {
                authority.failed.insert(id);
            }
        }
    }
    disconnect_failed(server, authority);
    let Some(revision) = authority.revision.checked_add(1) else {
        authority.failed.extend(authority.peers.keys());
        disconnect_failed(server, authority);
        return;
    };
    authority.revision = revision;
    if !authority.peers.is_empty() {
        let stamp = dreamwake_sim::replication::ReplicationStamp {
            match_epoch: authority.epoch,
            server_tick: authority.tick.0,
            gameplay_tick: authority.sim.gameplay_tick(),
            scene_revision: u64::from(SCENE.0),
            revision,
        };
        let captured = authority
            .sim
            .capture_replication(stamp)
            .map_err(failure)
            .and_then(|capture| {
                authority
                    .replication
                    .prepare(
                        capture,
                        &authority
                            .peers
                            .values()
                            .map(|peer| peer.welcome.player.get())
                            .collect::<Vec<_>>(),
                    )
                    .map_err(failure)
            });
        if let Err(reason) = captured {
            log::error!("Dreamwake replication capture: {reason}");
            authority.failed.extend(authority.peers.keys());
        } else {
            for (&id, peer) in &mut authority.peers {
                let allowance = authority
                    .transport_diagnostics
                    .as_ref()
                    .and_then(|transport| transport.peer_egress_allowance(id).ok().flatten());
                if let Err(reason) = peer_state(
                    server,
                    id,
                    peer,
                    &mut authority.replication,
                    &mut authority.outcomes,
                    authority.tick,
                    allowance,
                ) {
                    log::warn!("Dreamwake replication peer {id}: {reason}");
                    authority.failed.insert(id);
                }
                report_egress(
                    id,
                    peer,
                    authority.now,
                    authority.transport_diagnostics.as_ref(),
                );
            }
        }
    }
    disconnect_failed(server, authority);
}

fn report_egress(
    id: u64,
    peer: &mut Peer,
    now: Duration,
    transport: Option<&engine_server::SharedNet>,
) {
    if now < peer.next_egress_report {
        return;
    }
    peer.next_egress_report = now + Duration::from_secs(1);
    let stats = transport.and_then(|transport| transport.peer_egress_stats(id).ok());
    let native = stats.and_then(|stats| stats.native_udp_ip);
    let web = stats.and_then(|stats| stats.webrtc_logical);
    let pacing_drops =
        native.map_or(0, |stats| stats.pacing_drops) + web.map_or(0, |stats| stats.pacing_drops);
    // A refused data channel write is discarded after Renet has accounted the
    // packet as sent, so it never appears as a pacing drop.
    let backend_drops =
        native.map_or(0, |stats| stats.backend_drops) + web.map_or(0, |stats| stats.backend_drops);
    let line = format!(
        "Dreamwake egress peer={id} at_ms={} finalized_through={} control_messages_total={} control_bytes_total={} state_packets_total={} state_bytes_total={} state_entries_total={} allowance_limited_total={} stream_unacknowledged={} native_udp_ip={native:?} webrtc_logical={web:?}",
        now.as_millis(),
        peer.inbox.finalized_through().0,
        peer.egress.control_messages,
        peer.egress.control_bytes,
        peer.egress.state_packets,
        peer.egress.state_bytes,
        peer.egress.state_entries,
        peer.egress.allowance_limited,
        peer.stream.unacknowledged(),
    );
    if pacing_drops > peer.last_pacing_drops || backend_drops > peer.last_backend_drops {
        log::warn!("{line}");
    } else {
        log::info!("{line}");
    }
    peer.last_pacing_drops = pacing_drops;
    peer.last_backend_drops = backend_drops;
}
