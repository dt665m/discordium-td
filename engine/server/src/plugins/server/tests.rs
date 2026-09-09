use super::*;
use crate::SharedNet;
use renet::RenetServer;
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
