use renet_cross::{
    BootstrapError, BootstrapService, MixedServerTransport, NETCODE_USER_DATA_BYTES,
    SessionAuthPolicy, SessionGrant, SessionIdAllocator,
};
use std::sync::{Arc, Mutex};

/// Shared only for HTTP SDP/bootstrap integration. Renet itself belongs solely
/// to the network worker, and no simulation system acquires this transport lock.
#[derive(Clone)]
pub struct SharedNet {
    pub health: crate::InstanceHealth,
    pub max_clients: usize,
    /// Packetizer profile authenticated during bootstrap, shared by all peers
    /// in this server instance. Games may use a smaller application frame limit.
    pub packet_config: renet::PacketConfig,
    pub transport: Arc<Mutex<MixedServerTransport>>,
    pub bootstrap: Arc<dyn BootstrapLifecycle>,
}

/// Cumulative counters have different units by backend and must not be summed
/// into a physical wire-byte total. No credentials or game identity data appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerEgressStats {
    pub native_udp_ip: renet_cross::EgressStats,
    pub webrtc_logical: renet_cross::EgressStats,
}
/// A missing entry means that backend has no individually tracked connection.
/// Aggregate statistics still include traffic assigned to its overflow bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerEgressStats {
    pub native_udp_ip: Option<renet_cross::EgressStats>,
    pub webrtc_logical: Option<renet_cross::EgressStats>,
}
/// A conservative, transport-derived budget for complete application messages.
/// Native units include UDP/IP; WebRTC units end at encrypted DataChannel messages.
#[derive(Debug, Clone, Copy)]
pub struct PublicationBudget {
    pub available_wire_bytes: usize,
    pub maximum_atomic_wire_bytes: usize,
    pub message_overhead_bytes: usize,
}
impl PublicationBudget {
    pub fn message_wire_bytes(self, payload_bytes: usize) -> usize {
        payload_bytes.saturating_add(self.message_overhead_bytes)
    }
    /// Select one bounded prefix of an immutable message transfer. If credit
    /// cannot fit even its first message, return that single candidate so the
    /// scheduler records deferral/infeasibility instead of publishing a suffix.
    pub fn transfer_prefix(self, payload_sizes: impl IntoIterator<Item = usize>) -> (usize, usize) {
        let available = self
            .available_wire_bytes
            .min(self.maximum_atomic_wire_bytes);
        let mut wire_bytes = 0usize;
        let mut count = 0usize;
        for payload in payload_sizes {
            let cost = self.message_wire_bytes(payload);
            if count != 0 && wire_bytes.saturating_add(cost) > available {
                break;
            }
            wire_bytes = wire_bytes.saturating_add(cost);
            count += 1;
            if wire_bytes >= available {
                break;
            }
        }
        (count, wire_bytes)
    }
    fn from_allowance(
        allowance: renet_cross::EgressAllowance,
        packet: renet::PacketConfig,
        queued: impl IntoIterator<Item = renet::SendQueueStats>,
        ack_bytes: usize,
    ) -> Self {
        let transport = renet_cross::NETCODE_DATA_OVERHEAD_MAX
            .saturating_add(allowance.ip_udp_overhead as usize);
        let overhead = packet.max_packet_bytes() - packet.max_small_message_bytes() + transport;
        let queued_bytes = queued.into_iter().fold(0usize, |total, queue| {
            // Callers supply channels whose messages obey the negotiated
            // unsliced maximum. Counting each separately safely overestimates packing.
            total
                .saturating_add(queue.payload_bytes)
                .saturating_add(queue.messages.saturating_mul(overhead))
        });
        let ack = if ack_bytes == 0 {
            0
        } else {
            ack_bytes.saturating_add(transport)
        };
        Self {
            available_wire_bytes: usize::try_from(allowance.available_data_bytes)
                .unwrap_or(usize::MAX)
                .saturating_sub(queued_bytes)
                .saturating_sub(ack),
            maximum_atomic_wire_bytes: usize::try_from(allowance.maximum_data_bytes)
                .unwrap_or(usize::MAX)
                .saturating_sub(ack),
            message_overhead_bytes: overhead,
        }
    }
}
impl SharedNet {
    /// Sample only on the network worker before enqueueing a publication. The
    /// channels must contain only bounded, unsliced messages. Reserve current
    /// unsent/timer-due work once; awaiting ACK is retention, not work due every
    /// flush. Packetization still prioritizes control and the final pacer enforces
    /// its hard byte/packet cap independently of this publication preflight.
    pub fn publication_budget(
        &self,
        server: &renet::RenetServer,
        connection_id: u64,
        channels: &[u8],
    ) -> std::io::Result<Option<PublicationBudget>> {
        let transport = self
            .transport
            .lock()
            .map_err(|_| std::io::Error::other("transport budget lock poisoned"))?;
        let allowance = transport
            .udp()
            .peer_egress_allowance(connection_id)
            .or_else(|| transport.webrtc().peer_egress_allowance(connection_id));
        Ok(allowance.map(|allowance| {
            PublicationBudget::from_allowance(
                allowance,
                self.packet_config,
                channels
                    .iter()
                    .filter_map(|&channel| server.channel_send_due(connection_id, channel)),
                server.pending_ack_bytes(connection_id).unwrap_or(0),
            )
        }))
    }
    /// Sample from the network worker or diagnostics path, outside simulation.
    pub fn egress_stats(&self) -> std::io::Result<ServerEgressStats> {
        let transport = self
            .transport
            .lock()
            .map_err(|_| std::io::Error::other("transport statistics lock poisoned"))?;
        Ok(ServerEgressStats {
            native_udp_ip: transport.udp().egress_stats(),
            webrtc_logical: transport.webrtc().egress_stats(),
        })
    }
    /// The identifier is the transport connection ID, not a game player ID.
    pub fn peer_egress_stats(&self, connection_id: u64) -> std::io::Result<PeerEgressStats> {
        let transport = self
            .transport
            .lock()
            .map_err(|_| std::io::Error::other("transport statistics lock poisoned"))?;
        Ok(PeerEgressStats {
            native_udp_ip: transport.udp().peer_egress_stats(connection_id),
            webrtc_logical: transport.webrtc().peer_egress_stats(connection_id),
        })
    }
}

/// The simulation driver only owns admission/lifecycle, while the HTTP adapter
/// retains the generic credential verifier. Games can inspect the verified grant
/// without duplicating token parsing or trusting a client-supplied player ID.
pub trait BootstrapLifecycle: Send + Sync {
    fn activate(
        &self,
        client_id: u64,
        user_data: Option<&[u8; NETCODE_USER_DATA_BYTES]>,
    ) -> Result<(), BootstrapError>;
    fn grant(&self, client_id: u64) -> Result<Option<SessionGrant>, BootstrapError>;
    fn on_client_disconnected(&self, client_id: u64);
}
impl<A, P> BootstrapLifecycle for BootstrapService<A, P>
where
    A: SessionIdAllocator + Send + Sync,
    P: SessionAuthPolicy + Send + Sync,
{
    fn activate(
        &self,
        client_id: u64,
        user_data: Option<&[u8; NETCODE_USER_DATA_BYTES]>,
    ) -> Result<(), BootstrapError> {
        match user_data {
            Some(data) => self.on_client_connected_with_user_data(client_id, data),
            None => self.on_client_connected(client_id),
        }
    }
    fn grant(&self, client_id: u64) -> Result<Option<SessionGrant>, BootstrapError> {
        self.session_grant(client_id)
    }
    fn on_client_disconnected(&self, client_id: u64) {
        BootstrapService::on_client_disconnected(self, client_id);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    fn allowance(ip: u64) -> renet_cross::EgressAllowance {
        renet_cross::EgressAllowance {
            basis: if ip == 0 {
                renet_cross::EgressBasis::EncryptedLogical
            } else {
                renet_cross::EgressBasis::NativeUdpIp
            },
            config: None,
            available_data_bytes: 3600,
            maximum_data_bytes: 3600,
            ip_udp_overhead: ip,
        }
    }
    #[test]
    fn full_owner_unit_fits_only_after_control_drains_and_credit_recovers() {
        let packet = renet_cross::PacketProfile::ipv6_1200()
            .packet_config()
            .unwrap();
        let empty = PublicationBudget::from_allowance(allowance(28), packet, [], 13);
        let owner_cost: usize = [1100, 1100, 1031]
            .into_iter()
            .map(|bytes| empty.message_wire_bytes(bytes))
            .sum();
        assert_eq!(owner_cost, 3456);
        assert!(owner_cost <= empty.available_wire_bytes);
        let waiting = PublicationBudget::from_allowance(
            allowance(28),
            packet,
            [renet::SendQueueStats {
                messages: 1,
                payload_bytes: 200,
            }],
            13,
        );
        assert!(owner_cost > waiting.available_wire_bytes);
        let mut spent = allowance(28);
        spent.available_data_bytes = 3500;
        assert!(
            owner_cost
                > PublicationBudget::from_allowance(spent, packet, [], 13).available_wire_bytes
        );
        // Available credit can fall below an atomic unit without changing its
        // maximum feasibility or publishing any partial member.
        assert_eq!(
            empty.maximum_atomic_wire_bytes,
            waiting.maximum_atomic_wire_bytes
        );
    }
    #[test]
    fn sixteen_kib_transfer_makes_bounded_progress_and_cannot_skip_blocked_prefix() {
        let budget = PublicationBudget {
            available_wire_bytes: 1400,
            maximum_atomic_wire_bytes: 1536,
            message_overhead_bytes: 95,
        };
        let messages = [1024usize; 16];
        let mut cursor = 0;
        while cursor < messages.len() {
            let (count, cost) = budget.transfer_prefix(messages[cursor..].iter().copied());
            assert_eq!(count, 1);
            assert!(cost <= budget.available_wire_bytes);
            cursor += count;
        }
        assert_eq!(cursor, 16);
        let blocked = PublicationBudget {
            available_wire_bytes: 100,
            ..budget
        };
        assert_eq!(blocked.transfer_prefix([1024, 1]), (1, 1119));
    }
    #[test]
    fn retained_controls_do_not_age_atomic_groups_at_150ms_rtt_with_loss() {
        use std::collections::{BTreeMap, VecDeque};
        use std::time::Duration;
        for loss_percent in [0u64, 3] {
            let channels = vec![
                renet::ChannelConfig {
                    channel_id: 0,
                    max_memory_usage_bytes: 64 * 1024,
                    send_type: renet::SendType::ReliableOrdered {
                        resend_time: Duration::from_millis(300),
                    },
                },
                renet::ChannelConfig {
                    channel_id: 1,
                    max_memory_usage_bytes: 64 * 1024,
                    send_type: renet::SendType::Unreliable,
                },
            ];
            let config = renet::ConnectionConfig {
                available_bytes_per_tick: 3600,
                server_channels_config: channels.clone(),
                client_channels_config: channels,
            };
            let packet = renet_cross::PacketProfile::ipv6_1200()
                .packet_config()
                .unwrap();
            let mut server = renet::RenetServer::new_with_packet_config(config.clone(), packet);
            server.add_connection(1);
            let mut client = renet::RenetClient::new_with_packet_config(config, packet);
            client.set_connected();
            let mut downstream = VecDeque::new();
            let mut upstream = VecDeque::new();
            let mut packet_number = 0u64;
            let mut candidate = None::<(u64, usize)>;
            let mut chunks = BTreeMap::<u64, u8>::new();
            let mut completed = 0u64;
            let mut largest_lag = 0u64;
            let mut old_budget_blocked = 0;
            let mut largest_retained = 0;
            let mut previous = Duration::ZERO;
            for tick in 1u64..=1800 {
                let now = Duration::from_nanos(tick * 1_000_000_000 / 60);
                server.update(now - previous);
                client.update(now - previous);
                previous = now;
                while downstream.front().is_some_and(|(at, _)| *at <= now) {
                    let (_, bytes): (_, Vec<u8>) = downstream.pop_front().unwrap();
                    client.process_packet(&bytes);
                }
                while upstream.front().is_some_and(|(at, _)| *at <= now) {
                    let (_, bytes): (_, Vec<u8>) = upstream.pop_front().unwrap();
                    server.process_packet_from(&bytes, 1).unwrap();
                }
                while client.receive_message(0).is_some() {}
                while let Some(bytes) = client.receive_message(1) {
                    let publication = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                    if publication <= completed {
                        continue;
                    }
                    let entry = chunks.entry(publication).or_default();
                    *entry |= 1 << bytes[8];
                    if *entry == 7 {
                        assert!(publication > completed);
                        completed = publication;
                        chunks.retain(|key, _| *key > completed);
                    }
                    while chunks.len() > 8 {
                        chunks.pop_first();
                    }
                }
                for _ in 0..3 {
                    server.send_message(1, 0, vec![7; 32]);
                }
                largest_retained =
                    largest_retained.max(server.channel_send_queue(1, 0).unwrap().messages);
                if tick % 3 == 0 {
                    let due: Vec<_> = [0, 1]
                        .into_iter()
                        .map(|channel| server.channel_send_due(1, channel).unwrap())
                        .collect();
                    let old = PublicationBudget::from_allowance(
                        allowance(48),
                        packet,
                        [
                            server.channel_send_queue(1, 0).unwrap(),
                            server.channel_send_queue(1, 1).unwrap(),
                        ],
                        server.pending_ack_bytes(1).unwrap(),
                    );
                    if old.available_wire_bytes < 1095 {
                        old_budget_blocked += 1;
                    }
                    let budget = PublicationBudget::from_allowance(
                        allowance(48),
                        packet,
                        due,
                        server.pending_ack_bytes(1).unwrap(),
                    );
                    let (publication, cursor) = candidate.get_or_insert((tick, 0));
                    let (count, cost) = budget.transfer_prefix((*cursor..3).map(|_| 1000));
                    if cost <= budget.available_wire_bytes {
                        for index in *cursor..*cursor + count {
                            let mut bytes = vec![0; 1000];
                            bytes[..8].copy_from_slice(&publication.to_le_bytes());
                            bytes[8] = index as u8;
                            server.send_message(1, 1, bytes);
                        }
                        *cursor += count;
                        if *cursor == 3 {
                            candidate = None;
                        }
                    }
                }
                for bytes in server.get_packets_to_send(1).unwrap() {
                    packet_number += 1;
                    if packet_number
                        .wrapping_mul(6364136223846793005)
                        .rotate_left(17)
                        % 100
                        >= loss_percent
                    {
                        downstream.push_back((now + Duration::from_millis(75), bytes));
                    }
                }
                for bytes in client.get_packets_to_send() {
                    packet_number += 1;
                    if packet_number
                        .wrapping_mul(6364136223846793005)
                        .rotate_left(17)
                        % 100
                        >= loss_percent
                    {
                        upstream.push_back((now + Duration::from_millis(75), bytes));
                    }
                }
                if tick > 60 {
                    largest_lag = largest_lag.max(tick - completed);
                }
                assert!(server.is_connected(1));
            }
            assert!(largest_retained >= 24);
            assert!(
                old_budget_blocked > 100,
                "exercise retained-queue starvation"
            );
            assert!(
                completed > 1770,
                "groups must progress under {loss_percent}% loss"
            );
            assert!(
                largest_lag <= 30,
                "group lag {largest_lag} ticks at {loss_percent}% loss"
            );
        }
    }

    #[test]
    fn backend_units_and_pending_ack_are_accounted_once() {
        let packet = renet_cross::PacketProfile::ipv6_1200()
            .packet_config()
            .unwrap();
        for (ip, overhead) in [(0, 47), (28, 75), (48, 95)] {
            let budget = PublicationBudget::from_allowance(
                allowance(ip),
                packet,
                [renet::SendQueueStats {
                    messages: 2,
                    payload_bytes: 100,
                }],
                20,
            );
            assert_eq!(budget.message_wire_bytes(1000), 1000 + overhead);
            assert_eq!(
                budget.available_wire_bytes,
                3600 - 100 - 2 * overhead - 20 - 25 - ip as usize
            );
        }
    }
}
