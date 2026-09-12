//! Local administrator opt-in; no HTTP or peer route exposes these records.
use crate::{ActionTrace, AuthorityActionTrace};
use bevy::prelude::*;

#[derive(Resource)]
struct TraceExport {
    file: Option<engine_net::trace::TraceFile>,
    cursor: u64,
    next: f64,
}

pub(crate) fn install(
    app: &mut App,
    mut file: engine_net::trace::TraceFile,
) -> std::io::Result<()> {
    let mut header = serde_json::to_vec(&serde_json::json!({
        "format": 1, "kind": "authority_trace_header",
        "protocol": dreamwake_protocol::PROTOCOL_ID,
        "ruleset": dreamwake_sim::CheckpointIdentity::current(0).ruleset,
        "schema": dreamwake_sim::replication::schema_identity(),
        "engine_net": engine_net::SOURCE_IDENTITY,
    }))
    .map_err(std::io::Error::other)?;
    header.push(b'\n');
    file.write_record(&header)?;
    app.world().resource::<ActionTrace>().set_enabled(true);
    app.insert_resource(TraceExport {
        file: Some(file),
        cursor: 0,
        next: 0.0,
    })
    .add_systems(Update, export.after(engine_server::ServerSystems::Poll));
    Ok(())
}

#[derive(serde::Serialize)]
struct Record<'a> {
    format: u8,
    kind: &'static str,
    elapsed_seconds: f64,
    eviction_gap: bool,
    dropped: u64,
    record: &'a AuthorityActionTrace,
}

fn export(time: Res<Time<Real>>, trace: Res<ActionTrace>, mut output: ResMut<TraceExport>) {
    let now = time.elapsed_secs_f64();
    if output.file.is_none() || now < output.next {
        return;
    }
    output.next = now + 0.25;
    let batch = trace.snapshot_since(output.cursor, 128);
    if batch.records.is_empty() && batch.gap {
        let gap = serde_json::json!({
            "format": 1, "kind": "authority_trace_gap", "elapsed_seconds": now,
            "eviction_gap": true, "dropped": batch.dropped, "through": batch.through,
        });
        if !write(&gap, &trace, &mut output) {
            return;
        }
    }
    for record in &batch.records {
        if !write(
            &Record {
                format: 1,
                kind: "authority_action",
                elapsed_seconds: now,
                eviction_gap: batch.gap,
                dropped: batch.dropped,
                record,
            },
            &trace,
            &mut output,
        ) {
            return;
        }
    }
    output.cursor = batch.through;
}

fn write(record: &impl serde::Serialize, trace: &ActionTrace, output: &mut TraceExport) -> bool {
    let encoded = serde_json::to_vec(record).map(|mut bytes| {
        bytes.push(b'\n');
        bytes
    });
    let result = encoded
        .map_err(std::io::Error::other)
        .and_then(|bytes| output.file.as_mut().unwrap().write_record(&bytes));
    if let Err(error) = result {
        log::warn!("Local action trace stopped: {error}");
        output.file = None;
        trace.set_enabled(false);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreamwake_protocol::{DreamAction, live::TickInput};
    use engine_net::{
        codec::BoundedVec,
        commands::{ActionEdge, Command, OwnerStream},
        types::*,
    };

    fn command(sequence: u64) -> Command<TickInput, DreamAction> {
        Command {
            owner: OwnerStream {
                connection: ConnectionId(1),
                epoch: ConnectionEpoch(2),
                stream: CommandStream(3),
                owner: EntityId {
                    index: 7,
                    generation: 1,
                },
                ownership: OwnershipEpoch(1),
            },
            sequence: CommandSeq(sequence),
            target: TargetTick(sequence + 5),
            input: TickInput::default(),
            actions: BoundedVec::new(vec![ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        }
    }

    #[test]
    fn export_preserves_header_cursors_gaps_and_stops_at_the_file_budget() {
        let path = std::env::temp_dir().join(format!(
            "authority-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut app = App::new();
        app.init_resource::<ActionTrace>()
            .init_resource::<Time<Real>>();
        install(
            &mut app,
            engine_net::trace::TraceFile::create(&path, 4096).unwrap(),
        )
        .unwrap();
        let trace = app.world().resource::<ActionTrace>().clone();
        trace.command(17, 7, &command(1), Some(ServerTick(1)));
        app.update();
        let read = || {
            std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>()
        };
        let rows = read();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "authority_trace_header");
        assert_eq!(rows[1]["record"]["server_instance"], 17);
        assert_eq!(rows[1]["record"]["target_c"], 6);
        app.update();
        assert_eq!(
            read().len(),
            2,
            "a frame cannot duplicate an exported revision"
        );

        trace.command(17, 7, &command(2), Some(ServerTick(2)));
        trace.set_enabled(false);
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(250));
        app.update();
        assert_eq!(read()[2]["kind"], "authority_trace_gap");
        trace.set_enabled(true);
        for sequence in 3..=128 {
            trace.command(17, 7, &command(sequence), Some(ServerTick(sequence)));
        }
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(250));
        app.update();
        assert!(!trace.enabled());
        assert_eq!(trace.retained_bytes(), 0);
        assert!(app.world().resource::<TraceExport>().file.is_none());
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() <= 4096);
        assert_eq!(
            before.last(),
            Some(&b'\n'),
            "budget exhaustion never truncates a JSON record"
        );
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs(1));
        app.update();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
}
