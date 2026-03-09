use std::{
    fs::{File, create_dir_all},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use game_shared::{ClientDebugUploadBatch, DebugRecorderInfo, ServerDebugFrame};
use serde::Serialize;
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

const DATE_DIR_FORMAT: &[FormatItem<'static>] = format_description!("[year]-[month]-[day]");
const RUN_ID_FORMAT: &[FormatItem<'static>] =
    format_description!("[year][month][day]T[hour][minute][second]-[subsecond digits:3]Z");

#[derive(Clone)]
pub(crate) struct DebugRecorderHandle {
    sender: Sender<RecorderCommand>,
    info: Arc<RecorderInfoState>,
}

impl DebugRecorderHandle {
    pub(crate) fn new(log_dir: PathBuf) -> Result<Self, std::io::Error> {
        create_dir_all(&log_dir)?;

        let now = OffsetDateTime::now_utc();
        let date_dir = now
            .format(DATE_DIR_FORMAT)
            .unwrap_or_else(|_| "unknown-date".to_owned());
        let run_id = format!(
            "run-{}-{}",
            now.format(RUN_ID_FORMAT)
                .unwrap_or_else(|_| unix_time_ms().to_string()),
            std::process::id()
        );
        let run_dir = log_dir.join(date_dir).join(&run_id);
        create_dir_all(&run_dir)?;

        let server_frames_path = run_dir.join("server.ndjson");
        let client_frames_path = run_dir.join("client.ndjson");

        let server_file = File::create(&server_frames_path)?;
        let client_file = File::create(&client_frames_path)?;
        let (sender, receiver) = mpsc::channel();

        let info = Arc::new(RecorderInfoState {
            run_id,
            run_dir,
            server_frames_path,
            client_frames_path,
        });
        let writer_info = Arc::clone(&info);

        std::thread::Builder::new()
            .name("discordium-debug-recorder".to_owned())
            .spawn(move || {
                let mut server_writer = BufWriter::new(server_file);
                let mut client_writer = BufWriter::new(client_file);

                for command in receiver {
                    let result = match command {
                        RecorderCommand::ServerFrame(frame) => write_json_line(
                            &mut server_writer,
                            &ServerFrameEnvelope {
                                kind: "server_frame",
                                recorded_at_unix_ms: unix_time_ms(),
                                run_id: &writer_info.run_id,
                                frame,
                            },
                        ),
                        RecorderCommand::ClientBatch(batch) => write_json_line(
                            &mut client_writer,
                            &ClientBatchEnvelope {
                                kind: "client_frames",
                                recorded_at_unix_ms: unix_time_ms(),
                                run_id: &writer_info.run_id,
                                batch,
                            },
                        ),
                    };

                    if let Err(err) = result {
                        log::error!("debug recorder write failed: {err}");
                    }
                }
            })
            .expect("failed to spawn debug recorder thread");

        Ok(Self { sender, info })
    }

    pub(crate) fn record_server_frame(&self, frame: ServerDebugFrame) {
        if let Err(err) = self.sender.send(RecorderCommand::ServerFrame(frame)) {
            log::warn!("failed to queue server debug frame for recording: {err}");
        }
    }

    pub(crate) fn record_client_batch(&self, batch: ClientDebugUploadBatch) {
        if let Err(err) = self.sender.send(RecorderCommand::ClientBatch(batch)) {
            log::warn!("failed to queue client debug batch for recording: {err}");
        }
    }

    pub(crate) fn info(&self) -> DebugRecorderInfo {
        DebugRecorderInfo {
            enabled: true,
            run_id: Some(self.info.run_id.clone()),
            run_dir: Some(path_display(&self.info.run_dir)),
            server_frames_path: Some(path_display(&self.info.server_frames_path)),
            client_frames_path: Some(path_display(&self.info.client_frames_path)),
        }
    }
}

enum RecorderCommand {
    ServerFrame(ServerDebugFrame),
    ClientBatch(ClientDebugUploadBatch),
}

struct RecorderInfoState {
    run_id: String,
    run_dir: PathBuf,
    server_frames_path: PathBuf,
    client_frames_path: PathBuf,
}

#[derive(Serialize)]
struct ServerFrameEnvelope<'a> {
    kind: &'static str,
    recorded_at_unix_ms: u128,
    run_id: &'a str,
    frame: ServerDebugFrame,
}

#[derive(Serialize)]
struct ClientBatchEnvelope<'a> {
    kind: &'static str,
    recorded_at_unix_ms: u128,
    run_id: &'a str,
    batch: ClientDebugUploadBatch,
}

fn write_json_line<T: Serialize>(
    writer: &mut BufWriter<File>,
    value: &T,
) -> Result<(), std::io::Error> {
    serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn path_display(path: &Path) -> String {
    path.display().to_string()
}
