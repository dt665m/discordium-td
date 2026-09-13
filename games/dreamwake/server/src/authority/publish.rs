use super::*;
use engine_net::{
    replication::*,
    scheduling::{Cadence, TransportBudget, UnitId, UnitRequest},
};
use peer::FrozenGroup;

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
    if let live::ServerControl::Finalized { receipts, .. } = value {
        peer.egress.finalized_messages += 1;
        peer.egress.finalized_bytes += bytes.len() as u64;
        peer.egress.finalized_receipts += receipts.as_slice().len() as u64;
    }
    server.send_message(id, wire::CONTROL_CHANNEL, bytes);
    Ok(())
}
/// Reliable transport owns retransmission after each fence is accepted once.
pub(super) fn lifecycle(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    scopes: &mut ServerScopes<Vec<u8>>,
) -> Result<(), String> {
    let exits: Vec<_> = scopes.unsent_exits().take(8).collect();
    for exit in exits {
        control(server, id, peer, &live::ServerControl::Exit(exit))?;
        scopes.mark_exit_sent(exit).map_err(failure)?;
    }
    let destroys: Vec<_> = scopes.unsent_destroys().take(8).collect();
    for destroy in destroys {
        control(server, id, peer, &live::ServerControl::Destroy(destroy))?;
        scopes.mark_destroy_sent(destroy).map_err(failure)?;
    }
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
struct PreparedGroup {
    publication: GroupPublication,
    members: Vec<FullState<Vec<u8>>>,
    baseline_packets: Vec<engine_net::replication::baselines::BaselinePacket>,
    bytes: Vec<u8>,
}

fn prepare_group(
    peer: &mut Peer,
    scopes: &mut ServerScopes<Vec<u8>>,
) -> Result<Option<PreparedGroup>, String> {
    if peer.frozen.is_some() {
        return Ok(None);
    }
    let mut members = Vec::with_capacity(live::baselines::MEMBERS);
    for id in peer.owner_group_entities() {
        let state = scopes.pending(id).ok_or("Missing owner group member")?;
        members.push(state.clone());
    }
    members.sort_by_key(|state| state.scope);
    let end_tick = members[0].end_tick;
    if members.iter().any(|state| state.end_tick != end_tick) {
        return Err("Mixed owner group ticks".into());
    }
    let manifest: Vec<_> = members.iter().map(|state| state.scope).collect();
    // Revision identifies the prediction topology, not publication frequency.
    // FullState snapshot IDs identify newer checkpoints within that topology.
    if manifest != peer.group_manifest {
        peer.group_revision = peer
            .group_revision
            .checked_next()
            .ok_or("Group revision exhausted")?;
        peer.group_manifest.clone_from(&manifest);
    }
    let publication = GroupPublication {
        connection: peer.welcome.stream.epoch,
        group: live::OWNER_GROUP,
        revision: peer.group_revision,
        snapshot: members
            .iter()
            .find(|state| state.scope.entity == peer.welcome.owner_entity)
            .ok_or("Missing owner")?
            .snapshot,
    };
    if peer.last_owner_publication == Some(publication) {
        return Ok(None);
    }
    let Some(baseline_packets) = peer.baselines.prepare(&members, publication)? else {
        return Ok(None);
    };
    let mut encoded_members = members.clone();
    for (state, packet) in encoded_members.iter_mut().zip(&baseline_packets) {
        state.payload = live::baselines::encode_member(packet).map_err(failure)?;
    }
    let chunk = GroupChunk {
        publication,
        end_tick,
        manifest,
        index: 0,
        count: 1,
        members: encoded_members,
    };
    let sequence = peer.next_frame()?;
    let bytes = live::encode_state(
        &live::StateFrame::Group(chunk),
        peer.welcome.stream.epoch,
        sequence,
        peer.limit(),
    )
    .map_err(failure)?;
    Ok(Some(PreparedGroup {
        publication,
        members,
        baseline_packets,
        bytes,
    }))
}
// Limit this delivery class, not unrelated reliable controls. Clock replies and
// scope fences must not consume finalization's four-batch progress window.
// New committed ticks coalesce in the bounded audit while these await receipt ACK.
const FINALIZED_CONTROL_WINDOW: usize = 4;
fn finalization_audit_expired(peer: &Peer, tick: ServerTick) -> bool {
    tick > peer.finalized_sent_through
        && peer
            .inbox
            .receipts()
            .find(|receipt| receipt.tick > peer.finalized_sent_through)
            .is_none_or(|receipt| receipt.tick.0 != peer.finalized_sent_through.0 + 1)
}
fn finalized(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    tick: ServerTick,
) -> Result<(), String> {
    if tick <= peer.finalized_sent_through {
        return Ok(());
    }
    if peer.finalized_batches.len() >= FINALIZED_CONTROL_WINDOW {
        peer.egress.finalized_deferred += 1;
        return Ok(());
    }
    let receipts: Vec<_> = peer
        .inbox
        .receipts()
        .filter(|receipt| receipt.tick > peer.finalized_sent_through)
        .take(32)
        .copied()
        .collect();
    if receipts
        .first()
        .is_none_or(|receipt| receipt.tick.0 != peer.finalized_sent_through.0 + 1)
    {
        return Err("Unsent finalization audit expired".into());
    }
    // Never claim a later watermark than this bounded batch. This also preserves
    // every execution/substitution receipt when publication had to catch up.
    let through = receipts.last().unwrap().tick;
    control(
        server,
        id,
        peer,
        &live::ServerControl::Finalized {
            through,
            receipts: BoundedVec::new(receipts).map_err(failure)?,
            menu_applied: peer.menu_applied,
            arrival: peer.arrival,
        },
    )?;
    peer.finalized_sent_through = through;
    peer.finalized_batches.issue(through).map_err(failure)?;
    peer.arrival.sample = None;
    Ok(())
}

fn peer_state(
    server: &mut RenetServer,
    id: u64,
    peer: &mut Peer,
    replication: &mut DreamReplication,
    outcomes: &mut outcomes::Outcomes,
    now: Duration,
    tick: ServerTick,
) -> Result<(), String> {
    welcome(server, id, peer)?;
    if !peer.active && now >= peer.initial_deadline {
        return Err("Initial scene synchronization expired".into());
    }
    finalized(server, id, peer, tick)?;
    // Controls have first use of each Renet packetization pass.
    action_outcomes(server, id, peer, outcomes)?;
    if !peer.ready {
        return Ok(());
    }
    // Only bootstrap/reset keeps an exact pending proof. Ordinary publications
    // never wait for application receipts before admitting newer state.
    if peer.active && peer.baselines.reset_pending.is_none() {
        peer.frozen = None;
    }
    if peer
        .frozen
        .as_ref()
        .is_some_and(|group| now.saturating_sub(group.started) >= Duration::from_secs(2))
    {
        return Err("Frozen owner group synchronization expired".into());
    }
    let members = replication
        .owner_group_entities(peer.welcome.player.get())
        .map_err(failure)?;
    let bases: Vec<_> = members
        .into_iter()
        .filter(|entity| {
            *entity != peer.welcome.global_entity
                && *entity != peer.welcome.owner_entity
                && *entity != peer.welcome.collision_entity
        })
        .collect();
    if bases != peer.owner_bases {
        // Already queued old topology remains transport-owned. Its identities
        // cannot satisfy the new closure's atomic publication or decode proof.
        peer.frozen = None;
        peer.baselines.close();
        peer.owner_bases = bases;
    }
    let frozen_ids = if peer.frozen.is_some() {
        peer.owner_group_entities().into_iter().collect()
    } else {
        BTreeSet::new()
    };
    replication
        .gather_peer_with_frozen(
            id,
            &frozen_ids,
            peer.inbox.finalized_through(),
            &peer
                .inbox
                .input_continuity()
                .map(|input| input.held.held_only()),
        )
        .map_err(failure)?;
    let eligible = replication
        .eligible(id)
        .ok_or("Missing prepared eligibility")?
        .clone();
    if replication.global_entity() != Some(peer.welcome.global_entity)
        || replication.owner_entity(peer.welcome.player.get()) != Some(peer.welcome.owner_entity)
        || replication.collision_entity() != Some(peer.welcome.collision_entity)
    {
        return Err("Changed owner identity requires session reset".into());
    }
    let world = replication.world_revision();
    let policy = replication.policy_revision();
    let scopes = replication
        .scopes_mut(id)
        .ok_or("Uncommitted scope barrier")?;
    let group = prepare_group(peer, scopes)?;
    if !peer.baselines.reset_queued
        && let Some(resets) = peer.baselines.reset_pending.clone()
    {
        control(
            server,
            id,
            peer,
            &live::ServerControl::BaselineReset {
                resets: BoundedVec::new(resets).map_err(failure)?,
            },
        )?;
        peer.baselines.reset_queued = true;
    }
    lifecycle(server, id, peer, scopes)?;
    let mut packets = BTreeMap::new();
    // This is admission credit, not a promise to emit the whole group this
    // flush. Renet schedules its own slices/retries under transport pacing.
    let budget = TransportBudget {
        available_wire_bytes: peer.scheduler.available_wire_bytes(now),
        maximum_atomic_wire_bytes: 36 * 1024,
    };
    // Keep remote cadence independent of owner queue backpressure.
    let mut requests = Vec::new();
    for entry in eligible.entries() {
        let entity = entry.entity();
        if peer.owner_group_entities().contains(&entity) {
            continue;
        }
        let Some(state) = scopes.pending(entity) else {
            continue;
        };
        let state = state.clone();
        let sequence = peer.next_frame()?;
        // Every remote DTO is individually bounded and must fit the negotiated
        // single-message allowance. Never delegate an oversized payload to Renet.
        let bytes = live::encode_state(
            &live::StateFrame::Full(state),
            peer.welcome.stream.epoch,
            sequence,
            peer.limit(),
        )
        .map_err(failure)?;
        requests.push(UnitRequest::entity(
            entity,
            admission_cost(bytes.len()),
            Cadence {
                period_ticks: 3,
                maximum_age_ticks: 12,
                priority: 100,
                critical: false,
            },
        ));
        packets.insert(entity, bytes);
    }
    if let Some(group) = &group {
        let owner_age = tick.0.saturating_sub(group.members[0].end_tick.0);
        peer.egress.group_age_ticks = owner_age;
        peer.egress.maximum_group_age_ticks = peer.egress.maximum_group_age_ticks.max(owner_age);
        requests.push(
            UnitRequest::prediction_group(
                &eligible,
                live::OWNER_GROUP,
                admission_cost(group.bytes.len()),
                Cadence {
                    period_ticks: 2,
                    maximum_age_ticks: 6,
                    priority: 255,
                    critical: !peer.active
                        || peer.baselines.reset_pending.is_some()
                        || owner_age >= 6,
                },
            )
            .map_err(failure)?,
        );
    }
    let plan = peer
        .scheduler
        .schedule(&eligible, &requests, now, budget)
        .map_err(failure)?;
    let mut group = group;
    peer.scheduler
        .transmit(plan, world, policy, |unit| {
            match unit.id() {
                UnitId::Entity(entity) => {
                    let Some(bytes) = packets.remove(&entity) else { return false; };
                    if !server.can_send_message(id, wire::STATE_CHANNEL, bytes.len()) {
                        return false;
                    }
                    scopes
                    .transmit_pending(entity, |_| {
                            peer.egress.state_messages += 1;
                            peer.egress.state_bytes += bytes.len() as u64;
                            server.send_message(id, wire::STATE_CHANNEL, bytes);
                        true
                    })
                    .unwrap_or(false)
                },
                UnitId::PredictionGroup(_) => {
                    let Some(group) = group.take() else { return false; };
                    if !server.can_send_message(id, wire::OWNER_GROUP_CHANNEL, group.bytes.len()) {
                        peer.egress.group_queue_deferred += 1;
                        return false;
                    }
                    // No source payload or send proof advances before the
                    // complete authorized unit is accepted by Renet.
                    if group.members.iter().any(|state| {
                        scopes
                            .pending(state.scope.entity)
                            .is_none_or(|pending| pending.receipt() != state.receipt())
                            && scopes.decoded_current(state.scope.entity) != Some(state.receipt())
                    }) {
                        return false;
                    }
                    peer.egress.state_messages += 1;
                    peer.egress.state_bytes += group.bytes.len() as u64;
                    server.send_message(id, wire::OWNER_GROUP_CHANNEL, group.bytes);
                    peer.last_owner_publication = Some(group.publication);
                    peer.egress.group_messages += 1;
                    for state in &group.members {
                        if state.scope.entity == peer.welcome.owner_entity {
                            peer.combat.sent(state.receipt(), state.end_tick, tick);
                        }
                    }
                    for state in &group.members {
                        if scopes.mark_sent(state.receipt()).is_err() {
                            return false;
                        }
                    }
                    if peer
                        .baselines
                        .mark_sent(&group.members, &group.baseline_packets)
                        .is_err()
                    {
                        return false;
                    }
                    if !peer.active || peer.baselines.reset_pending.is_some() {
                        peer.frozen = Some(FrozenGroup {
                            receipts: group.members.iter().map(FullState::receipt).collect(),
                            bases: group.baseline_packets.iter().filter_map(|packet| {
                                match packet.encoding {
                                    engine_net::replication::baselines::BaselineEncoding::Delta { base } => Some(base),
                                    _ => None,
                                }
                            }).collect(),
                            decoded: BTreeSet::new(),
                            started: now,
                            active_state: peer.activation_committed,
                        });
                    }
                    true
                }
            }
        })
        .map_err(failure)?;
    Ok(())
}

/// Conservative initial-send cost for admission. This does not reserve ACKs or
/// predict retries; final encrypted bytes are measured/enforced by the adapter.
fn admission_cost(bytes: usize) -> usize {
    bytes + bytes.div_ceil(1200) * 100
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
    // A stalled initial client may miss an entire immutable bootstrap unit or
    // its unsent command audit before it can send Ready/application receipts.
    // Retire that stream and offer a fresh one, within the original admission
    // deadline and shared resync cap. Already participating owners never use this.
    let initial_retries: Vec<_> = authority
        .peers
        .iter()
        .filter_map(|(&id, peer)| {
            (!peer.active
                && !peer.activation_committed
                && authority.now < peer.initial_deadline
                && (finalization_audit_expired(peer, authority.tick)
                    || peer.frozen.as_ref().is_some_and(|group| {
                        authority.now.saturating_sub(group.started) >= Duration::from_secs(2)
                    })))
            .then_some(id)
        })
        .collect();
    for id in initial_retries {
        if let Err(reason) = authority.resync(id) {
            log::warn!("Dreamwake initial synchronization peer {id}: {reason}");
            authority.failed.insert(id);
        }
    }
    // A participating client can suspend beyond an immutable baseline reset's
    // proof deadline. Retire that stream through the existing bounded recovery
    // handshake, preserving its actor and transport queues. This does not relax
    // finalization-audit continuity or restart a replacement bootstrap.
    let reset_retries: Vec<_> = authority
        .peers
        .iter()
        .filter_map(|(&id, peer)| {
            (peer.active
                && peer.activation_committed
                && peer.baselines.reset_pending.is_some()
                && peer.frozen.as_ref().is_some_and(|group| {
                    authority.now.saturating_sub(group.started) >= Duration::from_secs(2)
                }))
            .then_some(id)
        })
        .collect();
    for id in reset_retries {
        if let Err(reason) = authority.resync(id) {
            log::warn!("Dreamwake baseline reset synchronization peer {id}: {reason}");
            authority.failed.insert(id);
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
                if let Err(reason) = peer_state(
                    server,
                    id,
                    peer,
                    &mut authority.replication,
                    &mut authority.outcomes,
                    authority.now,
                    authority.tick,
                ) {
                    log::warn!("Dreamwake replication peer {id}: {reason}");
                    authority.failed.insert(id);
                }
                report_egress(
                    server,
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
    server: &RenetServer,
    id: u64,
    peer: &mut Peer,
    now: Duration,
    transport: Option<&engine_server::SharedNet>,
) {
    if now < peer.next_egress_report {
        return;
    }
    peer.next_egress_report = now + Duration::from_secs(1);
    let control_bytes = wire::CONTROL_MEMORY_BYTES
        .saturating_sub(server.channel_available_memory(id, wire::CONTROL_CHANNEL));
    let group_bytes = wire::OWNER_GROUP_SEND_BYTES
        .saturating_sub(server.channel_available_memory(id, wire::OWNER_GROUP_CHANNEL));
    let remote_bytes = wire::REMOTE_STATE_MEMORY_BYTES
        .saturating_sub(server.channel_available_memory(id, wire::STATE_CHANNEL));
    let stats = transport.and_then(|transport| transport.peer_egress_stats(id).ok());
    let native = stats.and_then(|stats| stats.native_udp_ip);
    let web = stats.and_then(|stats| stats.webrtc_logical);
    let pacing_drops =
        native.map_or(0, |stats| stats.pacing_drops) + web.map_or(0, |stats| stats.pacing_drops);
    let line = format!(
        "Dreamwake egress peer={id} at_ms={} finalized_through={} control_messages_total={} control_bytes_total={} finalized_messages_total={} finalized_bytes_total={} finalized_receipts_total={} finalized_deferred_total={} state_messages_total={} state_bytes_total={} control_queue_payload_bytes={} state_queue_payload_bytes={} group_queue_payload_bytes={} native_udp_ip={native:?} webrtc_logical={web:?}",
        now.as_millis(),
        peer.finalized_sent_through.0,
        peer.egress.control_messages,
        peer.egress.control_bytes,
        peer.egress.finalized_messages,
        peer.egress.finalized_bytes,
        peer.egress.finalized_receipts,
        peer.egress.finalized_deferred,
        peer.egress.state_messages,
        peer.egress.state_bytes,
        control_bytes,
        group_bytes + remote_bytes,
        group_bytes,
    );
    let line = format!(
        "{line} group_messages={} group_queue_deferred={} group_age_ticks={} maximum_group_age_ticks={}",
        peer.egress.group_messages,
        peer.egress.group_queue_deferred,
        peer.egress.group_age_ticks,
        peer.egress.maximum_group_age_ticks,
    );
    if pacing_drops > peer.last_pacing_drops || control_bytes >= wire::CONTROL_MEMORY_BYTES / 2 {
        log::warn!("{line}");
    } else {
        log::info!("{line}");
    }
    peer.last_pacing_drops = pacing_drops;
}
