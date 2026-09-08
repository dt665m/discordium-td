//! Transport runs independently from Bevy's compute pool. The worker alone owns
//! Renet and replication state; the transport mutex is shared only with HTTP SDP.
use crate::{
    debug_bridge::ServerDebugBridgeHandle,
    debug_context::DebugContext,
    debug_recorder::DebugRecorderHandle,
    net::SharedNet,
    replication::{ClientNetState, ReplicationHistory},
};
use bevy::platform::cell::SyncCell;
use bevy::prelude::*;
use game_shared::{ReliableServerMessage, WorldDelta};
use renet::{DefaultChannel, RenetServer, ServerEvent};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) mod egress;
pub(crate) mod inbox;
pub(crate) mod ingress;
mod plugin;
pub(crate) mod schedule;
pub(crate) mod worker;
pub(crate) use inbox::{Outbound, OutputBatch, PendingOutput};
pub(crate) use ingress::Ingress;
use ingress::{
    INPUTS_PER_CLIENT, IngressReceiver, IngressSender, RELIABLE_INPUT_LIMIT,
    UNRELIABLE_INPUT_LIMIT, ingress_channel,
};
pub(crate) use plugin::ServerNetworkPlugin;

const QUEUE_TICKS: usize = 8;
#[derive(Resource, Default)]
pub(crate) struct NetworkMetrics {
    pub peers: Vec<(u64, &'static str, Option<renet::NetworkInfo>)>,
    pub tick: u32,
    pub ingress_messages: u64,
    pub ingress_deferred_passes: u64,
    pub ingress_peak_pending: usize,
}
#[derive(Resource)]
pub(crate) struct NetworkWorker {
    input: SyncCell<IngressReceiver>,
    // Advances only on a fixed tick, never merely because PreUpdate drained.
    ingress_epoch: Arc<AtomicU64>,
    output: SyncSender<OutputBatch>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<(), String>>>,
}
impl NetworkWorker {
    fn check_health(&mut self) {
        if self.handle.as_ref().is_some_and(|h| h.is_finished()) {
            let result = self.handle.take().unwrap().join();
            panic!("network worker exited: {result:?}");
        }
    }
    pub fn send(&self, batch: OutputBatch) {
        if let Err(error) = self.output.try_send(batch) {
            self.stop.store(true, Ordering::Release);
            panic!(
                "network output pipeline exhausted its {QUEUE_TICKS}-tick retention bound: {error}"
            );
        }
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(Ok(())) => {}
                other => log::error!("network worker shutdown: {other:?}"),
            }
        }
    }
}
impl Drop for NetworkWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn batch(tick: u32) -> OutputBatch {
        OutputBatch {
            step_duration_ms: tick as f64,
            ..default()
        }
    }

    #[test]
    fn pipeline_preserves_fifo_and_fails_explicitly_at_retention_bound() {
        let (worker, rx) = test_worker(Vec::new());
        for tick in 0..QUEUE_TICKS {
            worker.send(batch(tick as u32));
        }
        let failure =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.send(batch(99))));
        assert!(failure.is_err());
        assert!(worker.stop.load(Ordering::Acquire));
        assert_eq!(
            rx.try_iter()
                .map(|batch| batch.step_duration_ms as usize)
                .collect::<Vec<_>>(),
            (0..QUEUE_TICKS).collect::<Vec<_>>()
        );
    }

    #[test]
    fn worker_failure_is_observed_at_simulation_boundary() {
        let (mut worker, _rx) = test_worker(Vec::new());
        let handle = thread::spawn(|| Err("injected transport failure".into()));
        while !handle.is_finished() {
            thread::yield_now();
        }
        worker.handle = Some(handle);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.check_health()))
                .is_err()
        );
    }

    #[test]
    fn reliable_channel_overload_disconnects_instead_of_dropping_event() {
        let mut config = renet::ConnectionConfig::default();
        for channel in &mut config.server_channels_config {
            channel.max_memory_usage_bytes = 1;
        }
        let mut server = RenetServer::new(config);
        server.add_connection(1);
        let simulation = game_sim::Simulation::new();
        worker::send_reliable(
            &mut server,
            1,
            &ReliableServerMessage::JoinSnapshot(simulation.join_snapshot(1)),
        );
        assert!(!server.is_connected(1));
    }

    #[test]
    fn plugin_shutdown_joins_transport_worker() {
        use renet_cross::{
            BootstrapConfig, BootstrapService, MixedTransportBuilder, MonotonicClientIdAllocator,
            UnsecureDevAuthPolicy,
        };
        let bind = "127.0.0.1:0".parse().unwrap();
        let transport = MixedTransportBuilder::new(game_shared::PROTOCOL_ID)
            .udp_bind(bind)
            .webrtc_bind(bind)
            .public_udp_addr(bind)
            .public_webrtc_addr(bind)
            .max_clients(2)
            .authentication(renet_cross::ServerAuthentication::Unsecure)
            .build()
            .unwrap();
        let shared = SharedNet {
            max_clients: 2,
            transport: Arc::new(Mutex::new(transport)),
            bootstrap: Arc::new(BootstrapService::new(
                BootstrapConfig {
                    session_ttl: Duration::from_secs(120),
                    public_udp_addr: bind,
                    public_webrtc_addr: bind,
                    public_http_base: "http://localhost".into(),
                },
                MonotonicClientIdAllocator::new(1),
                UnsecureDevAuthPolicy,
            )),
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(ServerNetworkPlugin {
                shared,
                context: DebugContext::new(),
                bridge: None,
                recorder: None,
                conditioner: renet_cross::server_conditioner::ServerConditionerHandle::new(
                    Default::default(),
                )
                .unwrap(),
            });
        app.update();
        app.world_mut().write_message(AppExit::Success);
        app.update();
        let worker = app.world().resource::<NetworkWorker>();
        assert!(worker.stop.load(Ordering::Acquire));
        assert!(worker.handle.is_none());
    }
}

#[cfg(test)]
pub(crate) fn test_worker(events: Vec<Ingress>) -> (NetworkWorker, Receiver<OutputBatch>) {
    let (output, receiver) = mpsc::sync_channel(QUEUE_TICKS);
    let (mut sender, input) = ingress_channel(events.len().max(1));
    for event in events {
        assert!(sender.try_send(event).is_ok());
    }
    (
        NetworkWorker {
            input: SyncCell::new(input),
            ingress_epoch: Arc::new(AtomicU64::new(0)),
            output,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        },
        receiver,
    )
}
