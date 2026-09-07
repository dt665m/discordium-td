//! Bounded, best-effort JSON capture. The simulation never waits for disk writes.
use crate::debug_context::{DebugContext, unix_ms};
use game_shared::{
    ClientDebugUploadBatch, DEBUG_SCHEMA_VERSION, DebugRecorderInfo, ServerDebugFrame,
};
use serde::Serialize;
use std::{
    fs::{File, create_dir_all},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
};

const QUEUE_RECORDS: usize = 128;
const MAX_RUN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
struct WriterHealth {
    dropped: AtomicU64,
    failures: AtomicU64,
    written: AtomicU64,
    limit_reached: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct DebugRecorderHandle {
    sender: SyncSender<Record>,
    info: Arc<DebugRecorderInfo>,
    health: Arc<WriterHealth>,
    context: DebugContext,
}

impl DebugRecorderHandle {
    pub(crate) fn new(log_dir: PathBuf, context: DebugContext) -> Result<Self, std::io::Error> {
        let max_run_bytes = std::env::var("TD_DEBUG_MAX_BYTES")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?
            .unwrap_or(MAX_RUN_BYTES);
        if max_run_bytes == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "TD_DEBUG_MAX_BYTES must be positive",
            ));
        }
        let date = time::OffsetDateTime::now_utc().date().to_string();
        let run_id = context.identity.process_session.clone();
        let run_dir = log_dir.join(date).join(&run_id);
        create_dir_all(&run_dir)?;
        let server_path = run_dir.join("server.ndjson");
        let client_path = run_dir.join("client.ndjson");
        let events_path = run_dir.join("events.ndjson");
        let server_file = File::create(&server_path)?;
        let client_file = File::create(&client_path)?;
        let events_file = File::create(&events_path)?;
        let metadata = serde_json::json!({
            "schema_version": DEBUG_SCHEMA_VERSION, "identity": context.identity,
            "protocol_id": game_shared::PROTOCOL_ID, "tick_hz": game_shared::SERVER_TICK_HZ,
            "started_unix_ms": unix_ms(), "max_run_bytes": max_run_bytes,
            "queue_records": QUEUE_RECORDS,
        });
        std::fs::write(
            run_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&metadata)?,
        )?;
        let info = Arc::new(DebugRecorderInfo {
            limit_reached: false,
            max_run_bytes,
            schema_version: DEBUG_SCHEMA_VERSION,
            identity: context.identity.clone(),
            dropped_records: 0,
            write_failures: 0,
            written_records: 0,
            enabled: true,
            run_id: Some(run_id),
            run_dir: Some(run_dir.display().to_string()),
            server_frames_path: Some(server_path.display().to_string()),
            client_frames_path: Some(client_path.display().to_string()),
        });
        let health = Arc::new(WriterHealth::default());
        let writer_health = Arc::clone(&health);
        let writer_info = Arc::clone(&info);
        let (sender, receiver) = mpsc::sync_channel::<Record>(QUEUE_RECORDS);
        std::thread::Builder::new()
            .name("debug-recorder".into())
            .spawn(move || {
                let mut server = BufWriter::new(server_file);
                let mut client = BufWriter::new(client_file);
                let mut events = BufWriter::new(events_file);
                let mut written_bytes = 0;
                let mut last_health = std::time::Instant::now();
                loop {
                    let record = match receiver.recv_timeout(std::time::Duration::from_secs(1)) {
                        Ok(record) => Some(record),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            persist_health(&run_dir, &writer_info, &writer_health);
                            break;
                        }
                    };
                    if let Some(record) = record {
                        let writer = match record.data {
                            RecordData::Server { .. } => &mut server,
                            RecordData::Client { .. } => &mut client,
                            RecordData::Event { .. } => &mut events,
                        };
                        match write_record(writer, &record, &mut written_bytes, max_run_bytes) {
                            Ok(true) => {
                                writer_health.written.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(false) => {
                                writer_health.limit_reached.store(true, Ordering::Relaxed);
                                writer_health.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => {
                                writer_health.failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    if last_health.elapsed() >= std::time::Duration::from_secs(1) {
                        persist_health(&run_dir, &writer_info, &writer_health);
                        last_health = std::time::Instant::now();
                    }
                }
            })?;
        let recorder = Self {
            sender,
            info,
            health,
            context,
        };
        recorder.event("server_started", None, None, "recorder enabled".into());
        Ok(recorder)
    }

    fn enqueue(&self, data: RecordData) -> bool {
        if self.health.limit_reached.load(Ordering::Relaxed) {
            self.health.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let record = Record {
            schema_version: DEBUG_SCHEMA_VERSION,
            run_id: self.context.identity.process_session.clone(),
            recorded_at_unix_ms: unix_ms(),
            elapsed_ms: self.context.elapsed_ms(),
            data,
        };
        if self.sender.try_send(record).is_err() {
            self.health.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }
    pub(crate) fn record_server_frame(&self, frame: ServerDebugFrame) {
        self.enqueue(RecordData::Server { frame });
    }
    pub(crate) fn record_client_batch(&self, batch: ClientDebugUploadBatch) -> bool {
        self.enqueue(RecordData::Client { batch })
    }
    pub(crate) fn event(
        &self,
        name: &str,
        client_id: Option<u64>,
        tick: Option<u32>,
        reason: String,
    ) {
        self.enqueue(RecordData::Event {
            name: name.into(),
            client_id,
            tick,
            reason,
        });
    }
    pub(crate) fn info(&self) -> DebugRecorderInfo {
        health_snapshot(&self.info, &self.health)
    }
}

fn health_snapshot(base: &DebugRecorderInfo, health: &WriterHealth) -> DebugRecorderInfo {
    let mut info = base.clone();
    info.dropped_records = health.dropped.load(Ordering::Relaxed);
    info.write_failures = health.failures.load(Ordering::Relaxed);
    info.written_records = health.written.load(Ordering::Relaxed);
    info.limit_reached = health.limit_reached.load(Ordering::Relaxed);
    info
}

fn persist_health(run_dir: &std::path::Path, base: &DebugRecorderInfo, health: &WriterHealth) {
    let result = (|| -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&health_snapshot(base, health))?;
        let temporary = run_dir.join("health.json.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, run_dir.join("health.json"))
    })();
    if result.is_err() {
        health.failures.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Serialize)]
struct Record {
    schema_version: u32,
    run_id: String,
    recorded_at_unix_ms: u64,
    elapsed_ms: f64,
    #[serde(flatten)]
    data: RecordData,
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum RecordData {
    #[serde(rename = "server_frame")]
    Server { frame: ServerDebugFrame },
    #[serde(rename = "client_frames")]
    Client { batch: ClientDebugUploadBatch },
    #[serde(rename = "event")]
    Event {
        name: String,
        client_id: Option<u64>,
        tick: Option<u32>,
        reason: String,
    },
}

fn write_record(
    writer: &mut impl Write,
    record: &Record,
    written: &mut u64,
    limit: u64,
) -> Result<bool, std::io::Error> {
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    if *written + bytes.len() as u64 > limit {
        return Ok(false);
    }
    // Charge attempted bytes too: a partial write must not bypass the run limit.
    *written += bytes.len() as u64;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_envelope_keeps_frame_key_and_capture_time() {
        let record = Record {
            schema_version: DEBUG_SCHEMA_VERSION,
            run_id: "run".into(),
            recorded_at_unix_ms: 20,
            elapsed_ms: 10.0,
            data: RecordData::Server {
                frame: ServerDebugFrame {
                    conditioner: None,
                    schema_version: DEBUG_SCHEMA_VERSION,
                    identity: Default::default(),
                    capture_unix_ms: 15,
                    capture_elapsed_ms: 5.0,
                    step_duration_ms: 1.0,
                    tick: 2,
                    clients: Vec::new(),
                },
            },
        };
        let json = serde_json::to_value(record).unwrap();
        assert_eq!(json["kind"], "server_frame");
        assert_eq!(json["frame"]["capture_unix_ms"], 15);
        assert_eq!(json["recorded_at_unix_ms"], 20);
    }

    #[test]
    fn saturated_queue_rejects_without_blocking() {
        let context = DebugContext::new();
        let (sender, _receiver) = mpsc::sync_channel(1);
        let recorder = DebugRecorderHandle {
            sender,
            context: context.clone(),
            health: Arc::new(WriterHealth::default()),
            info: Arc::new(DebugRecorderInfo {
                limit_reached: false,
                max_run_bytes: 4096,
                schema_version: DEBUG_SCHEMA_VERSION,
                identity: context.identity,
                dropped_records: 0,
                write_failures: 0,
                written_records: 0,
                enabled: true,
                run_id: None,
                run_dir: None,
                server_frames_path: None,
                client_frames_path: None,
            }),
        };
        let event = || RecordData::Event {
            name: "test".into(),
            client_id: None,
            tick: None,
            reason: String::new(),
        };
        assert!(recorder.enqueue(event()));
        assert!(!recorder.enqueue(event()));
        assert_eq!(recorder.info().dropped_records, 1);
        recorder.health.limit_reached.store(true, Ordering::Relaxed);
        assert!(!recorder.enqueue(event()));
        assert!(recorder.info().limit_reached);
    }

    #[test]
    fn run_budget_stops_writes_and_events_keep_correlation() {
        let record = Record {
            schema_version: DEBUG_SCHEMA_VERSION,
            run_id: "run".into(),
            recorded_at_unix_ms: 1,
            elapsed_ms: 2.0,
            data: RecordData::Event {
                name: "disconnect".into(),
                client_id: Some(7),
                tick: Some(12),
                reason: "timeout".into(),
            },
        };
        let mut output = Vec::new();
        let mut written = 0;
        assert!(!write_record(&mut output, &record, &mut written, 1).unwrap());
        assert!(output.is_empty());
        assert!(write_record(&mut output, &record, &mut written, 4096).unwrap());
        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["client_id"], 7);
        assert_eq!(json["kind"], "event");
        assert_eq!(json["run_id"], "run");
    }
}
