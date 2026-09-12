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
    health: engine_net::clock::TickHealth,
    steps: usize,
    receives: usize,
    published: Vec<usize>,
    admitted: Vec<(u64, Vec<u8>)>,
}
impl Authority for Counter {
    fn update_health(&mut self, health: engine_net::clock::TickHealth) {
        self.health = health;
    }
    fn admit(&mut self, _: u64, _: &mut RenetServer) -> bool {
        true
    }
    fn disconnect(&mut self, _: u64) {}
    fn admit_authenticated(
        &mut self,
        client_id: u64,
        grant: Option<&renet_cross::SessionGrant>,
        server: &mut RenetServer,
    ) -> bool {
        self.admitted.push((
            client_id,
            grant.map_or_else(Vec::new, |g| g.application.clone()),
        ));
        server.send_message(client_id, engine_net::CONTROL_CHANNEL, b"ready".to_vec());
        true
    }
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
fn secure_driver_binds_two_udp_sessions_to_verified_game_grants() {
    use renet_cross::{
        BootstrapAuthError, NativeConnectOptions, SecureSessionAuthPolicy, ServerAuthentication,
        SessionAdmission, SessionCreateRequest, SessionGrant,
    };
    struct Tickets;
    impl SessionAdmission for Tickets {
        fn admit(
            &self,
            request: &SessionCreateRequest,
            now: Duration,
        ) -> Result<SessionGrant, BootstrapAuthError> {
            if request.protocol_id != Some(99)
                || request.service != "test"
                || request.match_id != "match"
                || !matches!(request.credential.as_str(), "one" | "two")
            {
                return Err(BootstrapAuthError::InvalidRequest);
            }
            Ok(SessionGrant {
                protocol_id: 99,
                service: "test".into(),
                match_id: "match".into(),
                expires_at: now.as_secs() + 60,
                replay_key: [request.credential.as_bytes()[0]; 32],
                application: request.credential.as_bytes().to_vec(),
            })
        }
    }
    let key = renet_cross::generate_random_bytes();
    let profile = renet_cross::PacketProfile::ipv6_1200();
    let transport = MixedTransportBuilder::new(99)
        .udp_bind("127.0.0.1:0".parse().unwrap())
        .webrtc_bind("127.0.0.1:0".parse().unwrap())
        .authentication(ServerAuthentication::Secure { private_key: key })
        .build()
        .unwrap();
    let address = transport.udp().addresses()[0];
    let bootstrap = Arc::new(BootstrapService::new(
        BootstrapConfig {
            session_ttl: Duration::from_secs(120),
            public_udp_addr: address,
            public_webrtc_addr: address,
            public_http_base: "http://127.0.0.1".into(),
        },
        MonotonicClientIdAllocator::new(21),
        SecureSessionAuthPolicy::new(Tickets, key)
            .with_packet_profile(profile)
            .unwrap(),
    ));
    let mut clients = Vec::new();
    for credential in ["one", "two"] {
        let request = SessionCreateRequest {
            protocol_id: Some(99),
            service: "test".into(),
            match_id: "match".into(),
            credential: credential.into(),
            require_secure: true,
            packet_profile: profile,
        };
        let session = bootstrap.create_session_with_request(&request).unwrap();
        assert!(
            bootstrap.create_session_with_request(&request).is_err(),
            "one-use ticket replay admitted"
        );
        let (client, transport, id) = renet_cross::connect_from_session(
            session,
            99,
            NativeConnectOptions {
                session_request: request,
                require_secure: true,
                connection_config: engine_net::connection_config(),
                packet_config: profile.packet_config().unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
        clients.push((client, transport, id, false));
    }
    let shared = SharedNet {
        health: Default::default(),
        max_clients: 8,
        packet_config: profile.packet_config().unwrap(),
        transport: Arc::new(Mutex::new(transport)),
        bootstrap,
    };
    let mut driver = ServerDriver::new(
        shared,
        engine_net::connection_config(),
        60,
        2,
        Counter::default(),
    );
    let dt = Duration::from_millis(1);
    for step in 1..=1500 {
        for (client, transport, _, ready) in &mut clients {
            client.update(dt);
            transport.update(dt, client).unwrap();
            transport.send_packets(client).unwrap();
            if let Some(bytes) = client.receive_message(engine_net::CONTROL_CHANNEL) {
                assert_eq!(&bytes[..], b"ready");
                *ready = true;
            }
        }
        driver.poll(dt * step).unwrap();
        if clients.iter().all(|(_, _, _, ready)| *ready) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        clients
            .iter()
            .all(|(client, _, _, ready)| client.is_connected() && *ready)
    );
    driver.authority.admitted.sort();
    assert_eq!(
        driver.authority.admitted,
        [(21, b"one".to_vec()), (23, b"two".to_vec())]
    );
    driver.shutdown().unwrap();
}

fn shared_net() -> SharedNet {
    let address = "127.0.0.1:0".parse().unwrap();
    SharedNet {
        health: Default::default(),
        max_clients: 8,
        packet_config: renet::PacketConfig::default(),
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
    for ms in 1..=1000 {
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
    assert_eq!(driver.authority.steps, 42);
    assert_eq!(driver.authority.published.last(), Some(&30));
    assert!(driver.health().overloaded);
    assert_eq!(driver.health().debt_ticks, 18_000 - 42);
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
fn driver_reports_debt_to_host_and_game_and_closes_readiness_on_drop() {
    let shared = shared_net();
    let health = shared.health.clone();
    let mut driver = ServerDriver::new(
        shared,
        engine_net::connection_config(),
        30,
        5,
        Counter::default(),
    );
    assert_eq!(health.report().status, crate::InstanceStatus::Starting);
    driver.poll(Duration::ZERO).unwrap();
    assert!(health.report().ready());
    driver.poll(Duration::from_secs(1)).unwrap();
    assert_eq!(driver.authority.health.debt_ticks, 26);
    assert!(driver.authority.health.overloaded);
    assert_eq!(health.report().status, crate::InstanceStatus::Overloaded);
    assert_eq!(health.report().committed_tick, 4);
    while driver.health().debt_ticks > 0 {
        driver.poll(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(driver.authority.steps, 30);
    assert!(!driver.authority.health.overloaded);
    assert!(health.report().ready());
    drop(driver);
    assert_eq!(health.report().status, crate::InstanceStatus::Stopped);
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
    for ms in 1..=1000 {
        driver.poll(Duration::from_millis(ms)).unwrap();
    }
    assert_eq!(driver.authority.receives, 1000);
    assert_eq!(driver.authority.steps, 30);
    assert_eq!(driver.authority.published, vec![5, 10, 15, 20, 25, 30]);
    driver.poll(Duration::from_secs(600)).unwrap();
    assert_eq!(driver.authority.steps, 34);
    assert_eq!(driver.authority.published.last(), Some(&30));
    assert!(driver.health().overloaded);
    driver.poll(Duration::from_secs(600)).unwrap();
    assert_eq!(driver.authority.steps, 38);
    assert_eq!(driver.authority.published.last(), Some(&30));
    driver
        .poll(Duration::from_secs(600) + Duration::from_nanos(33_333_334))
        .unwrap();
    assert_eq!(driver.authority.published.last(), Some(&42));
}
