use super::*;

pub(crate) struct ServerNetworkPlugin {
    pub shared: SharedNet,
    pub context: DebugContext,
    pub bridge: Option<ServerDebugBridgeHandle>,
    pub recorder: Option<DebugRecorderHandle>,
    pub conditioner: renet_cross::server_conditioner::ServerConditionerHandle,
}
impl Plugin for ServerNetworkPlugin {
    fn build(&self, app: &mut App) {
        let (worker_input, input) =
            ingress_channel(self.shared.max_clients * INPUTS_PER_CLIENT + 1);
        let ingress_epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = Arc::clone(&ingress_epoch);
        let (output_tx, output_rx) = mpsc::sync_channel(QUEUE_TICKS);
        let stop = Arc::new(AtomicBool::new(false));
        let cancellation = stop.clone();
        let shared = self.shared.clone();
        let context = self.context.clone();
        let bridge = self.bridge.clone();
        let recorder = self.recorder.clone();
        let conditioner = self.conditioner.clone();
        let handle = thread::Builder::new()
            .name("server-network".into())
            .spawn(move || {
                worker::run_worker(
                    shared,
                    cancellation,
                    worker_input,
                    worker_epoch,
                    output_rx,
                    context,
                    bridge,
                    recorder,
                    conditioner,
                )
            })
            .expect("start server network worker");
        app.insert_resource(NetworkWorker {
            input: SyncCell::new(input),
            ingress_epoch,
            output: output_tx,
            stop,
            handle: Some(handle),
        })
        .init_resource::<NetworkMetrics>()
        .init_resource::<PendingOutput>()
        .add_systems(PostUpdate, inbox::publish_output)
        .add_systems(Last, shutdown_on_exit.after(bevy::window::ExitSystems));
    }
}
fn shutdown_on_exit(mut exit: MessageReader<AppExit>, mut worker: ResMut<NetworkWorker>) {
    if exit.read().next().is_some() {
        worker.shutdown();
    }
}
