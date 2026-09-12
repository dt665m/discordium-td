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
fn frozen(
    peer: &mut Peer,
    scopes: &mut ServerScopes<Vec<u8>>,
    now: Duration,
) -> Result<(), String> {
    if peer.frozen.is_some() {
        return Ok(());
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
    let Some(baseline_packets) = peer.baselines.prepare(&members, publication)? else {
        return Ok(());
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
    let bytes = encode_group(&chunk, live::group_limits(), |payload| Ok(payload.clone()))
        .map_err(failure)?;
    let fragments = fragment_group(
        publication,
        &bytes,
        live::byte_limits(peer.limit()).map_err(failure)?,
    )
    .map_err(failure)?;
    peer.frozen = Some(FrozenGroup {
        members,
        fragments,
        next_fragment: 0,
        sent_once: false,
        decoded: BTreeSet::new(),
        started: now,
        active_state: peer.activation_committed,
        baseline_packets,
    });
    Ok(())
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
    transport: Option<&engine_server::SharedNet>,
) -> Result<(), String> {
    welcome(server, id, peer)?;
    if !peer.active && now >= peer.initial_deadline {
        return Err("Initial scene synchronization expired".into());
    }
    finalized(server, id, peer, tick)?;
    // Queue all new controls before sampling due-work reservation for state.
    // Finalization remains first; later outcome controls must not surprise the
    // physical packetizer after state has already spent the remaining credit.
    action_outcomes(server, id, peer, outcomes)?;
    if !peer.ready {
        return Ok(());
    }
    // Bootstrap retains its exact advertised dependency closure until decoded.
    // Normally active groups coalesce. A negotiated reset retains one complete
    // full repair group until its exact new-generation proof arrives, avoiding
    // an endless reset-generation loop when feedback or state is lost.
    if peer.active
        && peer.baselines.reset_pending.is_none()
        && peer.frozen.as_ref().is_none_or(|group| group.sent_once)
    {
        peer.frozen = None;
    }
    if peer.frozen.as_ref().is_some_and(|group| {
        (!peer.active || !group.sent_once)
            && now.saturating_sub(group.started) >= Duration::from_secs(2)
    }) {
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
        // A partial transfer from the previous topology cannot be completed
        // against a new dependency closure. The next manifest gets a new
        // revision and full baseline contexts as one atomic publication.
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
    frozen(peer, scopes, now)?;
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
    if peer.frozen.is_none() {
        return Ok(());
    }
    lifecycle(server, id, peer, scopes)?;
    let mut packets: BTreeMap<UnitId, Vec<Vec<u8>>> = BTreeMap::new();
    let group = peer.frozen.as_ref().unwrap();
    let fragments = group.fragments[group.next_fragment..].to_vec();
    let group_end_tick = group.members[0].end_tick;
    let mut group_packets = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let sequence = peer.next_frame()?;
        group_packets.push(
            live::encode_state(
                &live::StateFrame::Fragment(fragment),
                peer.welcome.stream.epoch,
                sequence,
                peer.limit(),
            )
            .map_err(failure)?,
        );
    }
    let mut budget = match transport {
        Some(transport) => transport
            .publication_budget(server, id, &[wire::CONTROL_CHANNEL, wire::STATE_CHANNEL])
            .map_err(failure)?
            .ok_or("Missing transport publication allowance")?,
        // Transport-free deterministic authority tests have no physical pacer.
        None => engine_server::PublicationBudget {
            available_wire_bytes: 16_384,
            maximum_atomic_wire_bytes: 16_384,
            message_overhead_bytes: 0,
        },
    };
    budget.available_wire_bytes = budget
        .available_wire_bytes
        .min(peer.scheduler.available_wire_bytes(now));
    // Keep every eligible remote unit in the scheduler even during a partial
    // owner transfer. Omitting it discards its accumulated deadline history.
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
            budget.message_wire_bytes(bytes.len()),
            Cadence {
                period_ticks: 3,
                maximum_age_ticks: 12,
                priority: 100,
                critical: false,
            },
        ));
        packets.insert(UnitId::Entity(entity), vec![bytes]);
    }
    let owner_age = tick.0.saturating_sub(group_end_tick.0);
    let owner_critical = !peer.active || peer.baselines.reset_pending.is_some() || owner_age >= 6;
    // During steady state, share prefix credit with currently due remote work.
    // Preserve at least half for owner progress; unused remote share is borrowed.
    // This reservation must not include actors still inside their cadence period.
    // If both cannot fit, overdue remote units can win
    // the normal age ordering until the owner's existing deadline is reached.
    let remote_reserve = if owner_critical {
        0
    } else {
        requests
            .iter()
            .filter(|request| {
                let UnitId::Entity(_) = request.id() else {
                    return false;
                };
                peer.scheduler
                    .last_transmitted(request.id())
                    .is_none_or(|sent| tick.0.saturating_sub(sent.0) >= 3)
            })
            .map(UnitRequest::wire_bytes)
            .sum::<usize>()
            .min(budget.available_wire_bytes / 2)
    };
    let prefix_budget = engine_server::PublicationBudget {
        available_wire_bytes: budget.available_wire_bytes.saturating_sub(remote_reserve),
        ..budget
    };
    // Transfer prefixes are staging only; all member proofs remain atomic until
    // the entire owner unit has arrived and decoded.
    let (fragment_count, group_cost) =
        prefix_budget.transfer_prefix(group_packets.iter().map(Vec::len));
    peer.egress.publication_credit = budget.available_wire_bytes;
    peer.egress.group_age_ticks = owner_age;
    peer.egress.maximum_group_age_ticks = peer.egress.maximum_group_age_ticks.max(owner_age);
    peer.egress.control_due_messages = server
        .channel_send_due(id, wire::CONTROL_CHANNEL)
        .map_or(0, |q| q.messages);
    peer.egress.control_due_payload_bytes = server
        .channel_send_due(id, wire::CONTROL_CHANNEL)
        .map_or(0, |q| q.payload_bytes);
    peer.egress.publication_prefix = if group_cost <= budget.available_wire_bytes {
        fragment_count
    } else {
        0
    };
    group_packets.truncate(fragment_count);
    requests.push(
        UnitRequest::prediction_group(
            &eligible,
            live::OWNER_GROUP,
            group_cost,
            Cadence {
                period_ticks: 2,
                maximum_age_ticks: 6,
                priority: 255,
                critical: owner_critical,
            },
        )
        .map_err(failure)?,
    );
    packets.insert(UnitId::PredictionGroup(live::OWNER_GROUP), group_packets);
    let plan = peer
        .scheduler
        .schedule(
            &eligible,
            &requests,
            now,
            TransportBudget {
                available_wire_bytes: budget.available_wire_bytes,
                maximum_atomic_wire_bytes: budget.maximum_atomic_wire_bytes,
            },
        )
        .map_err(failure)?;
    let group = peer.frozen.as_mut().unwrap();
    peer.scheduler
        .transmit(plan, world, policy, |unit| {
            let Some(frames) = packets.remove(&unit.id()) else {
                return false;
            };
            let bytes: usize = frames.iter().map(Vec::len).sum();
            if !server.can_send_message(id, wire::STATE_CHANNEL, bytes) {
                return false;
            }
            match unit.id() {
                UnitId::Entity(entity) => scopes
                    .transmit_pending(entity, |_| {
                        for bytes in frames {
                            peer.egress.state_messages += 1;
                            peer.egress.state_bytes += bytes.len() as u64;
                            server.send_message(id, wire::STATE_CHANNEL, bytes);
                        }
                        true
                    })
                    .unwrap_or(false),
                UnitId::PredictionGroup(_) => {
                    // Revalidate every member before each staged prefix. The
                    // complete graph closure is required even for one fragment.
                    if group.members.iter().any(|state| {
                        scopes
                            .pending(state.scope.entity)
                            .is_none_or(|pending| pending.receipt() != state.receipt())
                            && scopes.decoded_current(state.scope.entity) != Some(state.receipt())
                    }) {
                        return false;
                    }
                    for bytes in frames {
                        peer.egress.state_messages += 1;
                        peer.egress.state_bytes += bytes.len() as u64;
                        server.send_message(id, wire::STATE_CHANNEL, bytes);
                    }
                    peer.egress.group_fragments = peer
                        .egress
                        .group_fragments
                        .saturating_add(fragment_count as u64);
                    group.next_fragment += fragment_count;
                    if group.next_fragment < group.fragments.len() {
                        return true;
                    }
                    group.next_fragment = 0;
                    group.sent_once = true;
                    peer.egress.group_passes = peer.egress.group_passes.saturating_add(1);
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
                    true
                }
            }
        })
        .map_err(failure)?;
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
                    authority.transport_diagnostics.as_ref(),
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
    let control = server
        .channel_send_queue(id, wire::CONTROL_CHANNEL)
        .unwrap_or_default();
    let state = server
        .channel_send_queue(id, wire::STATE_CHANNEL)
        .unwrap_or_default();
    let stats = transport.and_then(|transport| transport.peer_egress_stats(id).ok());
    let native = stats.and_then(|stats| stats.native_udp_ip);
    let web = stats.and_then(|stats| stats.webrtc_logical);
    let pacing_drops =
        native.map_or(0, |stats| stats.pacing_drops) + web.map_or(0, |stats| stats.pacing_drops);
    let line = format!(
        "Dreamwake egress peer={id} at_ms={} finalized_through={} control_messages_total={} control_bytes_total={} finalized_messages_total={} finalized_bytes_total={} finalized_receipts_total={} finalized_deferred_total={} state_messages_total={} state_bytes_total={} control_queue_messages={} control_queue_payload_bytes={} state_queue_messages={} state_queue_payload_bytes={} native_udp_ip={native:?} webrtc_logical={web:?}",
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
        control.messages,
        control.payload_bytes,
        state.messages,
        state.payload_bytes
    );
    let line = format!(
        "{line} group_passes={} group_fragments={} group_age_ticks={} maximum_group_age_ticks={} publication_credit={} publication_prefix={} control_due_messages={} control_due_payload_bytes={}",
        peer.egress.group_passes,
        peer.egress.group_fragments,
        peer.egress.group_age_ticks,
        peer.egress.maximum_group_age_ticks,
        peer.egress.publication_credit,
        peer.egress.publication_prefix,
        peer.egress.control_due_messages,
        peer.egress.control_due_payload_bytes
    );
    if pacing_drops > peer.last_pacing_drops || control.messages >= FINALIZED_CONTROL_WINDOW {
        log::warn!("{line}");
    } else {
        log::info!("{line}");
    }
    peer.last_pacing_drops = pacing_drops;
}
