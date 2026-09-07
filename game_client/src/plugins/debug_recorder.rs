//! Best-effort diagnostic uploads. Never block gameplay on HTTP or retain unbounded data.
use bevy::prelude::*;
use game_shared::{
    ClientDebugFrame, ClientDebugUploadBatch, DEBUG_SCHEMA_VERSION, DebugCaptureHealth,
    DebugClientPlatform,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const MAX_BATCH_BYTES: usize = 256 * 1024;
const MAX_PENDING_FRAMES: usize = 32;
#[cfg(not(target_arch = "wasm32"))]
const MAX_QUEUED_BATCHES: usize = 8;

#[derive(Default)]
struct UploadHealth {
    dropped: AtomicUsize,
    failed: AtomicUsize,
    uploaded: AtomicUsize,
}

#[derive(Resource)]
pub(super) struct ClientDebugRecorderState {
    pub enabled: bool,
    pub session_id: String,
    instance_id: String,
    endpoint: String,
    generation: u64,
    upload_seq: u64,
    pending: VecDeque<(ClientDebugFrame, usize)>,
    dropped_frames: u64,
    last_flush: f64,
    health: Arc<UploadHealth>,
    #[cfg(not(target_arch = "wasm32"))]
    sender: Option<std::sync::mpsc::SyncSender<String>>,
    #[cfg(target_arch = "wasm32")]
    in_flight: Arc<std::sync::atomic::AtomicBool>,
}

impl ClientDebugRecorderState {
    pub fn new(enabled: bool, _http_base: &str) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let instance_id = format!(
            "native-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        #[cfg(target_arch = "wasm32")]
        let instance_id = format!("web-{}-{}", js_sys::Date::now(), js_sys::Math::random());
        Self {
            enabled,
            session_id: format!("{instance_id}-0"),
            instance_id,
            endpoint: String::new(),
            generation: 0,
            upload_seq: 0,
            pending: VecDeque::new(),
            dropped_frames: 0,
            last_flush: 0.0,
            health: Arc::new(UploadHealth::default()),
            #[cfg(not(target_arch = "wasm32"))]
            sender: None,
            #[cfg(target_arch = "wasm32")]
            in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // Start before bootstrap, so failed connection attempts are also identifiable.
    pub fn begin_session(&mut self, http_base: &str) {
        self.generation += 1;
        self.session_id = format!("{}-{}", self.instance_id, self.generation);
        self.pending.clear();
        self.upload_seq = 0;
        self.dropped_frames = 0;
        self.health = Arc::new(UploadHealth::default());
        self.endpoint = format!("{}/debug/client-frames", http_base.trim_end_matches('/'));
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sender = self
                .enabled
                .then(|| spawn_uploader(self.endpoint.clone(), Arc::clone(&self.health)));
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
        }
    }

    pub fn health(&self) -> DebugCaptureHealth {
        DebugCaptureHealth {
            dropped_frames: self.dropped_frames,
            dropped_batches: self.health.dropped.load(Ordering::Relaxed) as u64,
            upload_failures: self.health.failed.load(Ordering::Relaxed) as u64,
            uploaded_batches: self.health.uploaded.load(Ordering::Relaxed) as u64,
        }
    }

    pub fn record_frame(&mut self, frame: ClientDebugFrame) {
        if !self.enabled || self.endpoint.is_empty() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&frame) else {
            self.dropped_frames += 1;
            return;
        };
        // Reserve room for the batch envelope. A pathological frame is counted, never retried forever.
        if bytes.len() > MAX_BATCH_BYTES - 4096 {
            self.dropped_frames += 1;
            return;
        }
        if self.pending.len() == MAX_PENDING_FRAMES {
            self.pending.pop_front();
            self.dropped_frames += 1;
        }
        self.pending.push_back((frame, bytes.len()));
    }

    fn take_payload(&mut self) -> Option<String> {
        let mut bytes = 4096;
        let mut frames = Vec::new();
        while let Some((_, size)) = self.pending.front() {
            if bytes + size + 1 > MAX_BATCH_BYTES {
                break;
            }
            bytes += size + 1;
            frames.push(self.pending.pop_front().unwrap().0);
        }
        if frames.is_empty() {
            return None;
        }
        self.upload_seq += 1;
        #[cfg(not(target_arch = "wasm32"))]
        let source = DebugClientPlatform::Native;
        #[cfg(target_arch = "wasm32")]
        let source = DebugClientPlatform::Wasm;
        let batch = ClientDebugUploadBatch {
            schema_version: DEBUG_SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            build: option_env!("TD_BUILD_REVISION")
                .unwrap_or("unknown")
                .to_owned(),
            instance_id: self.instance_id.clone(),
            source,
            upload_seq: self.upload_seq,
            frames,
        };
        match serde_json::to_string(&batch) {
            Ok(payload) if payload.len() <= MAX_BATCH_BYTES => Some(payload),
            _ => {
                self.health.dropped.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

pub(super) fn flush_client_debug_recorder(
    time: Res<Time<Real>>,
    mut recorder: ResMut<ClientDebugRecorderState>,
) {
    let now = time.elapsed_secs_f64();
    if !recorder.enabled || now - recorder.last_flush < 0.5 {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    if recorder.in_flight.load(Ordering::Relaxed) {
        return;
    }
    recorder.last_flush = now;
    let Some(payload) = recorder.take_payload() else {
        return;
    };
    #[cfg(not(target_arch = "wasm32"))]
    if recorder
        .sender
        .as_ref()
        .is_none_or(|sender| sender.try_send(payload).is_err())
    {
        recorder.health.dropped.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(target_arch = "wasm32")]
    upload_browser(
        recorder.endpoint.clone(),
        payload,
        Arc::clone(&recorder.health),
        Arc::clone(&recorder.in_flight),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_uploader(
    endpoint: String,
    health: Arc<UploadHealth>,
) -> std::sync::mpsc::SyncSender<String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(MAX_QUEUED_BATCHES);
    std::thread::Builder::new()
        .name("debug-upload".into())
        .spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(3))
                .build();
            for payload in receiver {
                if agent
                    .post(&endpoint)
                    .set("Content-Type", "application/json")
                    .send_string(&payload)
                    .is_ok()
                {
                    health.uploaded.fetch_add(1, Ordering::Relaxed);
                } else {
                    health.failed.fetch_add(1, Ordering::Relaxed);
                    health.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
        .expect("failed to spawn debug uploader");
    sender
}

#[cfg(target_arch = "wasm32")]
fn upload_browser(
    endpoint: String,
    payload: String,
    health: Arc<UploadHealth>,
    in_flight: Arc<std::sync::atomic::AtomicBool>,
) {
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    in_flight.store(true, Ordering::Relaxed);
    spawn_local(async move {
        let result: Result<bool, JsValue> = async {
            let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
            let controller = web_sys::AbortController::new()?;
            let abort = controller.clone();
            let callback = Closure::once(move || abort.abort());
            let timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                3000,
            )?;
            let init = web_sys::RequestInit::new();
            init.set_method("POST");
            init.set_body(&JsValue::from_str(&payload));
            init.set_signal(Some(&controller.signal()));
            let response = JsFuture::from(window.fetch_with_str_and_init(&endpoint, &init)).await;
            window.clear_timeout_with_handle(timer);
            drop(callback);
            Ok(response?.dyn_into::<web_sys::Response>()?.ok())
        }
        .await;
        if matches!(result, Ok(true)) {
            health.uploaded.fetch_add(1, Ordering::Relaxed);
        } else {
            health.failed.fetch_add(1, Ordering::Relaxed);
            health.dropped.fetch_add(1, Ordering::Relaxed);
        }
        in_flight.store(false, Ordering::Relaxed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> ClientDebugFrame {
        serde_json::from_str(r#"{
            "schema_version": 2, "session_id": "test", "marker": false,
            "replication": {"full_snapshots":0,"patches":0,"baseline_misses":0,"decode_errors":0,"transport_errors":0},
            "capture_elapsed_ms":0.0,"frame_duration_ms":16.0,"snapshot_age_ms":null,"input_ack_age_ms":null,"disconnect_reason":null,"network":null,
            "recorder":{"dropped_frames":0,"dropped_batches":0,"upload_failures":0,"uploaded_batches":0},
            "frame_index":1,"client_id":null,"connected":false,"menu_visible":true,"menu_status":"offline",
            "phase":"InProgress","wave":0,"team_life":10,"objectives":[],"applied_world_tick":0,
            "latest_server_tick":null,"latest_server_message":null,"latest_acked_input_seq":null,"latest_sim_meta":null,
            "predicted_tick":null,"input_dir":[0.0,0.0],"pending_action_count":0,"pending_move_count":0,"rtt_ema":0.1,"jitter_ema":0.02,
            "reconciliation_offset":[0.0,0.0],"authoritative_heroes":[],"authoritative_enemies":[],"authoritative_towers":[],
            "predicted_heroes":[],"predicted_enemies":[],"predicted_towers":[],"rendered_actors":[],
            "interpolation":{"snapshot_ticks":[],"render_time":0.0,"interpolation_delay":0.1,"enemy_render_lead":0.0,"older_tick":null,"newer_tick":null,"factor":null}
        }"#).unwrap()
    }

    #[test]
    fn overload_is_bounded_and_drops_are_visible() {
        let mut recorder = ClientDebugRecorderState::new(true, "unused");
        recorder.endpoint = "http://unused".into();
        for index in 0..MAX_PENDING_FRAMES + 3 {
            let mut frame = frame();
            frame.frame_index = index as u64;
            recorder.record_frame(frame);
        }
        assert_eq!(recorder.pending.len(), MAX_PENDING_FRAMES);
        assert_eq!(recorder.health().dropped_frames, 3);
        assert_eq!(recorder.pending.front().unwrap().0.frame_index, 3);
        let payload = recorder.take_payload().unwrap();
        assert!(payload.len() <= MAX_BATCH_BYTES);
        let batch: ClientDebugUploadBatch = serde_json::from_str(&payload).unwrap();
        assert_eq!(batch.schema_version, DEBUG_SCHEMA_VERSION);
        assert_eq!(batch.upload_seq, 1);
        assert_eq!(batch.frames[0].frame_index, 3);
    }

    #[test]
    fn oversized_frame_does_not_block_later_frames() {
        let mut recorder = ClientDebugRecorderState::new(true, "unused");
        recorder.endpoint = "http://unused".into();
        let mut huge = frame();
        huge.menu_status = "x".repeat(MAX_BATCH_BYTES);
        recorder.record_frame(huge);
        recorder.record_frame(frame());
        assert_eq!(recorder.health().dropped_frames, 1);
        assert_eq!(recorder.pending.len(), 1);
        assert!(recorder.take_payload().is_some());
    }

    #[test]
    fn session_follows_selected_server_and_gets_new_identity() {
        let mut recorder = ClientDebugRecorderState::new(false, "http://localhost:8080");
        recorder.begin_session("http://localhost:18080/");
        let first = recorder.session_id.clone();
        assert_eq!(
            recorder.endpoint,
            "http://localhost:18080/debug/client-frames"
        );
        recorder.begin_session("http://localhost:19080");
        assert_ne!(first, recorder.session_id);
        assert_eq!(
            recorder.endpoint,
            "http://localhost:19080/debug/client-frames"
        );
    }
}
