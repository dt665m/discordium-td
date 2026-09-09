//! A reusable transport driver. Games implement authority callbacks; this owns
//! authenticated admission, polling, bounded fixed ticks and packet generation.
use crate::SharedNet;
use bevy::prelude::*;
use engine_net::clock::TickClock;
use renet::{ConnectionConfig, RenetServer, ServerEvent};
use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
    time::Duration,
};

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

/// Optional elapsed-time override for embedded hosts and deterministic tests.
/// Without an override polling uses real elapsed time, unaffected by game pause.
#[derive(Resource, Default)]
pub struct ServerElapsed(pub Option<Duration>);

/// A shared error handle survives `App::run`, which consumes the app's world.
#[derive(Resource, Clone, Default)]
pub struct ServerFailure(Arc<Mutex<Option<String>>>);
impl ServerFailure {
    pub fn error(&self) -> Option<String> {
        self.0.lock().expect("server failure lock poisoned").clone()
    }
    fn record(&self, error: String) {
        log::error!("server runtime: {error}");
        self.0
            .lock()
            .expect("server failure lock poisoned")
            .get_or_insert(error);
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerSystems {
    Poll,
    Shutdown,
}

/// Game-neutral Bevy runtime. The game installs one non-send `ServerDriver<A>`;
/// this plugin owns real-time polling, fixed-tick dispatch, publication, and exit.
/// Non-send storage allows authorities to own a simulation world without cloning it.
pub struct ServerPlugin<A>(PhantomData<fn() -> A>);
impl<A> Default for ServerPlugin<A> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<A: Authority + 'static> Plugin for ServerPlugin<A> {
    fn build(&self, app: &mut App) {
        app.init_resource::<ServerElapsed>()
            .init_resource::<ServerFailure>()
            .add_systems(Update, poll_server::<A>.in_set(ServerSystems::Poll))
            .add_systems(
                Last,
                shutdown_server::<A>
                    .in_set(ServerSystems::Shutdown)
                    .after(bevy::window::ExitSystems),
            );
    }
}

fn poll_server<A: Authority + 'static>(
    mut driver: NonSendMut<ServerDriver<A>>,
    elapsed: Res<ServerElapsed>,
    time: Res<Time<Real>>,
    failure: Res<ServerFailure>,
    mut exits: MessageWriter<AppExit>,
) {
    if failure.error().is_some() {
        return;
    }
    if let Err(error) = driver.poll(elapsed.0.unwrap_or_else(|| time.elapsed())) {
        failure.record(error);
        exits.write(AppExit::error());
    }
}

fn shutdown_server<A: Authority + 'static>(
    mut driver: NonSendMut<ServerDriver<A>>,
    mut exits: MessageReader<AppExit>,
    failure: Res<ServerFailure>,
) {
    if exits.read().next().is_some() {
        if let Err(error) = driver.shutdown() {
            failure.record(error);
        }
    }
}

/// Run a composed server app and retain detailed errors after its world is dropped.
pub fn run_app(mut app: App) -> Result<(), String> {
    let failure = app.world().resource::<ServerFailure>().clone();
    let exit = app.run();
    if let Some(error) = failure.error() {
        Err(error)
    } else if exit.is_error() {
        Err(format!("server exited: {exit:?}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use renet_cross::{
        BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
        UnsecureDevAuthPolicy,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Counter {
        steps: usize,
        receives: usize,
        published: Vec<usize>,
    }
    impl Authority for Counter {
        fn admit(&mut self, _: u64, _: &mut RenetServer) -> bool {
            true
        }
        fn disconnect(&mut self, _: u64) {}
        fn receive(&mut self, _: &mut RenetServer, _: Duration) {
            self.receives += 1;
        }
        fn step(&mut self, _: Duration) {
            self.steps += 1;
        }
        fn publish(&mut self, _: &mut RenetServer) {
            self.published.push(self.steps);
        }
    }

    fn shared_net() -> SharedNet {
        let address = "127.0.0.1:0".parse().unwrap();
        SharedNet {
            max_clients: 8,
            transport: Arc::new(Mutex::new(
                MixedTransportBuilder::new(99)
                    .udp_bind(address)
                    .webrtc_bind(address)
                    .public_udp_addr(address)
                    .public_webrtc_addr(address)
                    .build()
                    .unwrap(),
            )),
            bootstrap: Arc::new(BootstrapService::new(
                BootstrapConfig {
                    session_ttl: Duration::from_secs(120),
                    public_udp_addr: address,
                    public_webrtc_addr: address,
                    public_http_base: "http://127.0.0.1".into(),
                },
                MonotonicClientIdAllocator::new(1),
                UnsecureDevAuthPolicy,
            )),
        }
    }

    fn counter_app(shared: SharedNet) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().insert_non_send(ServerDriver::new(
            shared,
            engine_net::connection_config(),
            30,
            5,
            Counter::default(),
        ));
        app.add_plugins(ServerPlugin::<Counter>::default());
        app
    }

    #[test]
    fn plugin_dispatches_arbitrary_authority_with_monotonic_manual_time() {
        let mut app = counter_app(shared_net());
        for ms in 0..1000 {
            app.world_mut().resource_mut::<ServerElapsed>().0 = Some(Duration::from_millis(ms));
            app.update();
        }
        let driver = app.world().get_non_send::<ServerDriver<Counter>>().unwrap();
        assert_eq!(driver.authority.receives, 1000);
        assert_eq!(driver.authority.steps, 30);
        assert_eq!(driver.authority.published, vec![5, 10, 15, 20, 25, 30]);
        for elapsed in [
            Duration::from_secs(600),
            Duration::ZERO,
            Duration::from_secs(600),
        ] {
            app.world_mut().resource_mut::<ServerElapsed>().0 = Some(elapsed);
            app.update();
        }
        let driver = app.world().get_non_send::<ServerDriver<Counter>>().unwrap();
        assert_eq!(driver.authority.steps, 36);
        assert_eq!(driver.authority.published.last(), Some(&36));
        assert!(app.world().resource::<ServerFailure>().error().is_none());
    }

    #[test]
    fn plugin_propagates_transport_lock_failure_through_app_runner() {
        let shared = shared_net();
        let transport = shared.transport.clone();
        let _ = std::thread::spawn(move || {
            let _guard = transport.lock().unwrap();
            panic!("poison transport for error-path test");
        })
        .join();
        let app = counter_app(shared);
        let failure = app.world().resource::<ServerFailure>().clone();
        let error = run_app(app).unwrap_err();
        assert!(error.contains("poison"));
        assert_eq!(failure.error(), Some(error));
    }

    #[test]
    fn production_driver_uses_configured_clock_for_arbitrary_authority() {
        let mut driver = ServerDriver::new(
            shared_net(),
            engine_net::connection_config(),
            30,
            5,
            Counter::default(),
        );
        for ms in 0..1000 {
            driver.poll(Duration::from_millis(ms)).unwrap();
        }
        assert_eq!(driver.authority.receives, 1000);
        assert_eq!(driver.authority.steps, 30);
        assert_eq!(driver.authority.published, vec![5, 10, 15, 20, 25, 30]);
        driver.poll(Duration::from_secs(600)).unwrap();
        assert_eq!(driver.authority.steps, 36);
        assert_eq!(driver.authority.published.last(), Some(&36));
        driver.poll(Duration::from_secs(600)).unwrap();
        assert_eq!(driver.authority.steps, 36);
    }
}
