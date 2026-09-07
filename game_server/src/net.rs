use std::sync::{Arc, Mutex};

use bevy::prelude::Resource;
use renet::RenetServer;
use renet_cross::{
    BootstrapService, MixedServerTransport, MonotonicClientIdAllocator, UnsecureDevAuthPolicy,
};

#[derive(Clone)]
pub struct SharedNet {
    pub server: Arc<Mutex<RenetServer>>,
    pub transport: Arc<Mutex<MixedServerTransport>>,
    pub bootstrap: Arc<BootstrapService<MonotonicClientIdAllocator, UnsecureDevAuthPolicy>>,
}

impl SharedNet {
    pub fn with_server_and_transport<R>(
        &self,
        f: impl FnOnce(&mut RenetServer, &mut MixedServerTransport) -> R,
    ) -> Option<R> {
        let mut server = match self.server.lock() {
            Ok(s) => s,
            Err(err) => {
                log::error!("server mutex poisoned: {err}");
                return None;
            }
        };
        let mut transport = match self.transport.lock() {
            Ok(t) => t,
            Err(err) => {
                log::error!("transport mutex poisoned: {err}");
                return None;
            }
        };
        Some(f(&mut server, &mut transport))
    }
}

#[derive(Resource, Clone)]
pub struct NetRuntime {
    pub shared: SharedNet,
}
