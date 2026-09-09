//! Authority callbacks and the transport driver shared by all server hosts.
use super::{ServerPlugin, run_app};
use crate::SharedNet;
use bevy::prelude::*;
use engine_net::clock::TickClock;
use renet::{ConnectionConfig, RenetServer, ServerEvent};
use std::time::Duration;

pub trait Authority {
    fn admit(&mut self, client_id: u64, server: &mut RenetServer) -> bool;
    fn disconnect(&mut self, client_id: u64);
    fn receive(&mut self, server: &mut RenetServer, elapsed: Duration);
    fn step(&mut self, elapsed: Duration);
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
        Self {
            authority,
            server: RenetServer::new(config),
            shared,
            clock: TickClock::new(tick_hz, ticks_per_snapshot),
            previous: Duration::ZERO,
        }
    }
    /// Service one iteration using elapsed monotonic time. Embedders may call
    /// this from their own loop; simulation timing stays independent of polling.
    pub fn poll(&mut self, elapsed: Duration) -> Result<(), String> {
        let elapsed = elapsed.max(self.previous);
        let delta = elapsed.saturating_sub(self.previous);
        self.previous = elapsed;
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
                    if !(self.shared.bootstrap.on_client_connected(client_id).is_ok()
                        && self.authority.admit(client_id, &mut self.server))
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
        let tick = self.clock.advance(elapsed, || self.authority.step(elapsed));
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
    /// Release admission and game ownership, then send transport disconnects.
    pub fn shutdown(&mut self) -> Result<(), String> {
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
