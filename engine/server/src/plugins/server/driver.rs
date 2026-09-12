//! Authority callbacks and the transport driver shared by all server hosts.
use super::{ServerPlugin, run_app};
use crate::SharedNet;
use bevy::prelude::*;
use engine_net::clock::{TickClock, TickContext, TickHealth};
use engine_net::types::TickRate;
use renet::{ConnectionConfig, RenetServer, ServerEvent};
use std::time::{Duration, Instant};

pub trait Authority {
    /// Hosting debt can gate game-specific creation of new matches as well as
    /// the driver's generic connection admission. Existing ticks keep running.
    fn update_health(&mut self, _health: TickHealth) {}
    fn admit(&mut self, client_id: u64, server: &mut RenetServer) -> bool;
    /// Transport has already verified the exact issued token and authenticated
    /// user data. Games may bind account/party policy before creating an actor.
    fn admit_authenticated(
        &mut self,
        client_id: u64,
        _grant: Option<&renet_cross::SessionGrant>,
        server: &mut RenetServer,
    ) -> bool {
        self.admit(client_id, server)
    }
    fn disconnect(&mut self, client_id: u64);
    fn receive(&mut self, server: &mut RenetServer, elapsed: Duration);
    fn step(&mut self, elapsed: Duration);
    /// Games can use canonical tick/deadline metadata without owning another clock.
    fn step_tick(&mut self, context: TickContext) {
        self.step(context.observed_now);
    }
    fn publish(&mut self, server: &mut RenetServer);
}

pub struct ServerDriver<A> {
    pub authority: A,
    pub server: RenetServer,
    shared: SharedNet,
    clock: TickClock,
    previous: Duration,
}

impl<A: Authority> ServerDriver<A> {
    pub fn new(
        shared: SharedNet,
        config: ConnectionConfig,
        tick_hz: u32,
        ticks_per_snapshot: u32,
        authority: A,
    ) -> Self {
        let clock = TickClock::new(tick_hz, ticks_per_snapshot);
        shared.health.bind(
            TickRate::new(tick_hz).expect("validated server tick rate"),
            clock.catchup_limit(),
        );
        Self {
            authority,
            server: RenetServer::new_with_packet_config(config, shared.packet_config),
            shared,
            clock,
            previous: Duration::ZERO,
        }
    }
    /// Service one iteration using elapsed monotonic time. Embedders may call
    /// this from their own loop; simulation timing stays independent of polling.
    pub fn poll(&mut self, elapsed: Duration) -> Result<(), String> {
        let started_at = Instant::now();
        let elapsed = elapsed.max(self.previous);
        let delta = elapsed.saturating_sub(self.previous);
        self.previous = elapsed;
        let health = self.clock.health(elapsed);
        self.shared.health.observe(elapsed, started_at, health);
        self.authority.update_health(health);
        let admission_healthy = !health.overloaded;
        self.server.update(delta);
        {
            let mut transport = self.shared.transport.lock().map_err(|e| e.to_string())?;
            if let Err(error) = transport.update(delta, &mut self.server) {
                log::warn!("transport update: {error}");
            }
        }
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    let user_data = {
                        let transport = self.shared.transport.lock().map_err(|e| e.to_string())?;
                        transport
                            .udp()
                            .user_data(client_id)
                            .or_else(|| transport.webrtc().user_data(client_id))
                    };
                    if !(admission_healthy
                        && self
                            .shared
                            .bootstrap
                            .activate(client_id, user_data.as_ref())
                            .is_ok()
                        && self.shared.bootstrap.grant(client_id).is_ok_and(|grant| {
                            self.authority.admit_authenticated(
                                client_id,
                                grant.as_ref(),
                                &mut self.server,
                            )
                        }))
                    {
                        self.shared.bootstrap.on_client_disconnected(client_id);
                        self.server.disconnect(client_id);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, .. } => {
                    self.shared.bootstrap.on_client_disconnected(client_id);
                    self.authority.disconnect(client_id);
                }
            }
        }
        self.authority.receive(&mut self.server, elapsed);
        let tick = self
            .clock
            .advance_ticked(elapsed, |context| self.authority.step_tick(context));
        self.shared.health.observe(elapsed, started_at, tick.health);
        self.authority.update_health(tick.health);
        if tick.snapshot_due {
            self.authority.publish(&mut self.server);
        }
        if tick.send_due {
            self.shared
                .transport
                .lock()
                .map_err(|e| e.to_string())?
                .send_packets(&mut self.server);
        }
        Ok(())
    }
    /// Health is available to games and hosting/admission services without I/O.
    pub fn health(&self) -> TickHealth {
        self.clock.health(self.previous)
    }
    /// Release admission and game ownership, then send transport disconnects.
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.shared.health.stop();
        for id in self.server.clients_id() {
            self.shared.bootstrap.on_client_disconnected(id);
            self.authority.disconnect(id);
        }
        self.shared
            .transport
            .lock()
            .map_err(|e| e.to_string())?
            .disconnect_all(&mut self.server);
        Ok(())
    }
    pub fn run(self) -> Result<(), String>
    where
        A: 'static,
    {
        let mut app = App::new();
        app.add_plugins(
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
                Duration::from_millis(1),
            )),
        );
        app.world_mut().insert_non_send(self);
        app.add_plugins(ServerPlugin::<A>::default());
        run_app(app)
    }
}

impl<A> Drop for ServerDriver<A> {
    fn drop(&mut self) {
        self.shared.health.stop();
    }
}
