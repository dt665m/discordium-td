//! A reusable transport driver. Games implement authority callbacks; this owns
//! authenticated admission, polling, bounded fixed ticks and packet generation.
use crate::SharedNet;
use engine_net::clock::TickClock;
use renet::{ConnectionConfig, RenetServer, ServerEvent};
use std::time::{Duration, Instant};

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
    pub fn run(mut self) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.poll(started.elapsed())?;
            std::thread::sleep(Duration::from_millis(1));
        }
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

    #[test]
    fn production_driver_uses_configured_clock_for_arbitrary_authority() {
        let address = "127.0.0.1:0".parse().unwrap();
        let shared = SharedNet {
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
        };
        let mut driver = ServerDriver::new(
            shared,
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
