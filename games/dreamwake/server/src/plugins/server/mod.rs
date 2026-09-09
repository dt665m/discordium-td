use crate::authority::DreamAuthority;
use dreamwake_protocol as wire;
use engine_server::SharedNet;

/// Installs Dreamwake admission, commands and snapshots on the reusable server
/// runtime. Its sole authority owns the same shared simulation used by prediction.
pub struct DreamwakeServerPlugin {
    pub shared: SharedNet,
    pub seed: u64,
    pub lucid: bool,
}

impl bevy::prelude::Plugin for DreamwakeServerPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        let authority = DreamAuthority::new(self.seed, self.lucid, self.shared.max_clients);
        app.world_mut()
            .insert_non_send(engine_server::ServerDriver::new(
                self.shared.clone(),
                wire::connection_config(),
                dreamwake_sim::TICK_HZ,
                wire::TICKS_PER_SNAPSHOT,
                authority,
            ));
        app.add_plugins(engine_server::ServerPlugin::<DreamAuthority>::default());
    }
}
