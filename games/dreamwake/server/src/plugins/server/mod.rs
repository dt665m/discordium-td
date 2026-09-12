use crate::authority::DreamAuthority;
use dreamwake_protocol as wire;
use engine_server::SharedNet;
pub(crate) mod trace;

/// Installs Dreamwake admission, commands and snapshots on the reusable server
/// runtime. Its authority shares owner transition functions with prediction.
pub struct DreamwakeServerPlugin {
    pub shared: SharedNet,
    pub seed: u64,
    pub lucid: bool,
    pub replication_distance: f32,
    pub compression: engine_net::CompressionPolicy,
}

impl bevy::prelude::Plugin for DreamwakeServerPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        let mut authority = DreamAuthority::with_replication_policy(
            self.seed,
            self.lucid,
            self.shared.max_clients,
            self.replication_distance,
            self.compression,
        );
        authority.observe_transport(self.shared.clone());
        app.init_resource::<crate::ActionTrace>();
        authority.action_trace = app.world().resource::<crate::ActionTrace>().clone();
        app.world_mut()
            .insert_non_send(engine_server::ServerDriver::new(
                self.shared.clone(),
                crate::authority::connection_config(),
                dreamwake_sim::TICK_HZ,
                wire::TICKS_PER_SNAPSHOT,
                authority,
            ));
        app.add_plugins(engine_server::ServerPlugin::<DreamAuthority>::default());
    }
}
