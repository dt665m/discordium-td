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
impl SharedNet {
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
    /// Bytes a connection's transport has accepted but not yet sent. Only the
    /// WebRTC data channel queues (SCTP is congestion controlled); a native UDP
    /// socket either sends or drops, so it reports zero. Senders should hold
    /// back superseded state while this is high rather than add queueing delay.
    pub fn peer_backlog_bytes(&self, connection_id: u64) -> std::io::Result<usize> {
        let mut transport = self
            .transport
            .lock()
            .map_err(|_| std::io::Error::other("transport statistics lock poisoned"))?;
        Ok(transport
            .webrtc_mut()
            .peer_backlog_bytes(connection_id)
            .unwrap_or(0))
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

/// The simulation driver owns admission/lifecycle; HTTP retains the credential
/// verifier. Games inspect verified grants without duplicating token parsing.
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
