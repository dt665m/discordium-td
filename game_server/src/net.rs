use renet_cross::{
    BootstrapService, MixedServerTransport, MonotonicClientIdAllocator, UnsecureDevAuthPolicy,
};
use std::sync::{Arc, Mutex};

/// Shared only for HTTP SDP/bootstrap integration. Renet itself belongs solely
/// to the network worker, and no simulation system acquires this transport lock.
#[derive(Clone)]
pub struct SharedNet {
    pub max_clients: usize,
    pub transport: Arc<Mutex<MixedServerTransport>>,
    pub bootstrap: Arc<BootstrapService<MonotonicClientIdAllocator, UnsecureDevAuthPolicy>>,
}
