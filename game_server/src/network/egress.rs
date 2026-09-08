//! Worker-owned replication and diagnostic projections.
use crate::{
    debug_context::{DebugContext, unix_ms},
    replication::{ClientNetState, ReplicationHistory},
};
use game_shared::{ServerDebugClientFrame, ServerDebugFrame, WorldDelta};
use renet::{DefaultChannel, RenetServer};
use std::collections::HashMap;

pub(crate) fn build_server_debug_frame(
    server: &RenetServer,
    udp_clients: &std::collections::HashSet<u64>,
    world: &WorldDelta,
    client_net_states: &HashMap<u64, ClientNetState>,
    context: &DebugContext,
    step_duration_ms: f64,
    conditioner: &renet_cross::server_conditioner::ServerConditionerHandle,
) -> ServerDebugFrame {
    let mut client_ids = server.clients_id();
    client_ids.sort_unstable();

    let clients = client_ids
        .into_iter()
        .map(|client_id| {
            let state = client_net_states.get(&client_id);
            let world = world.clone();
            ServerDebugClientFrame {
                network: server.network_info(client_id).ok().map(|n| {
                    game_shared::DebugNetworkHealth {
                        browser_drops: None,
                        transport: if udp_clients.contains(&client_id) {
                            "udp"
                        } else {
                            "webrtc"
                        }
                        .into(),
                        rtt_ms: n.rtt * 1000.0,
                        packet_loss: n.packet_loss,
                        sent_bytes_per_second: n.bytes_sent_per_second as f64,
                        received_bytes_per_second: n.bytes_received_per_second as f64,
                    }
                }),
                client_id,
                last_acked_tick: state.and_then(|state| state.last_acked_tick),
                confirmed_baseline_tick: state.and_then(|state| state.last_acked_tick),
                sent_history_len: state.map_or(0, |state| state.outstanding.len()),
                snapshot_payload_bytes: state
                    .and_then(|state| state.outstanding.back())
                    .map(|packet| packet.payload.len()),
                snapshot_baseline_tick: state
                    .and_then(|state| state.outstanding.back())
                    .and_then(|packet| packet.baseline_tick),
                world,
            }
        })
        .collect();

    ServerDebugFrame {
        conditioner: Some(crate::conditioner::snapshot(conditioner)),
        schema_version: game_shared::DEBUG_SCHEMA_VERSION,
        identity: context.identity.clone(),
        capture_unix_ms: unix_ms(),
        capture_elapsed_ms: context.elapsed_ms(),
        step_duration_ms,
        tick: world.tick,
        clients,
    }
}

pub(crate) fn broadcast_world_deltas(
    server: &mut RenetServer,
    world: &WorldDelta,
    clients: &mut HashMap<u64, ClientNetState>,
    history: &mut ReplicationHistory,
) {
    history.record(world);
    let client_ids = server.clients_id();
    let baselines: Vec<_> = client_ids
        .iter()
        .map(|id| clients.get(id).and_then(|client| client.last_acked_tick))
        .collect();
    history.prepare_packets(&baselines);
    for client_id in client_ids {
        let client = clients.entry(client_id).or_default();
        let packet = history.packet_for(client.last_acked_tick);
        // Renet drops unreliable messages when its channel is full. Only
        // successfully enqueued packets may establish an ACK baseline.
        if server.can_send_message(client_id, DefaultChannel::Unreliable, packet.payload.len()) {
            server.send_message(
                client_id,
                DefaultChannel::Unreliable,
                packet.payload.clone(),
            );
            client.sent(packet);
        }
    }
    history.retire_acknowledged(clients);
}
