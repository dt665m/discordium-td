# Netcode capacity eval

> Historical record from before the engine consolidation. Source paths, commands,
> and validation results below describe that version; use the current root README
> and architecture/deployment guides for this workspace.

Use this harness to compare implementations on the same workload, not to infer a
production player limit from localhost frame rate. The default matrix has 1, 8
and 32 players, each with 84, 256 and 1,024 enemies. It runs each case three times
in release mode. No browser or live server is started.

```bash
./scripts/run-netcode-eval.sh --output target/reports/netcode-eval/before.json
# Make an implementation change, then run the identical command/configuration.
./scripts/run-netcode-eval.sh --output target/reports/netcode-eval/after.json
python3 scripts/compare-netcode-eval.py \
  target/reports/netcode-eval/before.json \
  target/reports/netcode-eval/after.json \
  --output target/reports/netcode-eval/comparison.json
```

Run on the same machine, with comparable background activity. The comparison uses
medians across repetitions and rejects differing workload signatures, target,
tick count, missing cases, incomplete repetitions, failed recovery and invalid
metrics. Optional `--max-bandwidth-regression-percent` and
`--max-cpu-regression-percent` turn comparisons into explicit regression gates;
CPU gates need tolerance for scheduler noise. Use
`--max-delivery-regression-percent` and `--max-snapshot-age-regression-percent`
to gate service quality too: lower traffic is not a win if measured delivery
collapses. Schema 2 reports include the complete matrix configuration, aggregate
and minimum-per-client delivery ratios, and measured snapshot-age distributions.
Zero measured delivery is always rejected, even if final recovery succeeds.

Use `--players 1,8,32,64 --entities 84,256,1024` to widen the matrix. `--ticks`
controls the simulation duration (default 180); the first 60 ticks warm caches and
establish baselines and are excluded from measurements. `--repetitions` controls
repeat count. The simulator keeps actors durable and asserts that requested actor
counts remain present, so a dying/empty workload cannot silently appear faster.

## What is measured

The workload runs the actual shared simulation with deterministic movement and
attacks. Enemy positions start in a crowded region, and health is raised to keep
the load sustained. It is a deliberately dense synthetic combat workload; it is
not a claim that normal maps or game balance support these counts. Each world is
signed using a stable hash of its JSON representation for comparison matching.

The replication eval uses the production shared history, per-client acknowledged
versions, packet cache and binary codec. It sends those packets through real
Renet unreliable channels, including fragmentation and transport ACKs. Two cases
run for each workload:

- `Clean`: no injected delay or drops; all clients share comparable baselines.
- `MixedLoss`: different clients have 2–6 ticks of one-way added delay plus 0–2
  ticks of jitter, deterministic 5% datagram loss independently on each direction,
  and a one-second bidirectional outage for the first client. Actual baseline ages
  are recorded rather than assuming delay settings equal measured RTT.

Every accepted snapshot must exactly match the authoritative world for its tick.
Clients acknowledge only reconstructed versions, retain bounded baselines and
ignore older delivered updates. After the measured interval, 128 unconditioned
recovery ticks verify every client catches up; those ticks do not dilute bandwidth
or timing metrics. The workload and default loss seed are fixed. `--seed` selects another reproducible
loss trace; compare multiple matching seeds when delivery/tail-age differences are
small. Repetitions with one seed measure timing variance, not independent loss
trials. Changing the wire format can
change packet boundaries and therefore which messages a dropped datagram affects;
that is part of the real comparison.

Reports include payload bytes per client/update, total Renet bytes and packets
sent by the server, delivered/offered updates, full snapshots, baseline ages,
accounted history memory and final recovery status. Timing distributions separate
simulation plus snapshot creation, replication, Renet handling, and client decode.
Correctness comparisons, JSON input/output, signature generation and sleep time
are outside those timing intervals.

Renet byte totals exclude UDP/IP, netcode encryption and WebRTC/DTLS overhead.
History memory is the production logical accounting budget, not allocator/RSS
measurement, and excludes peer queues and client baselines. The harness runs all
clients in one process; its total runtime is not server frame time. Browser
rendering, OS socket handling, link congestion and reliable gameplay event traffic
still need separate multiplayer validation. CPU timing and throughput are evidence
for the tested workload, not a guarantee of a production capacity limit.

## Captured combat replay

To compare against an actual recorded match:

```bash
cargo run --release -p game_server --example replication_profile -- \
  target/reports/bandwidth-capture/YYYY-MM-DD/RUN/server.ndjson
```

This replays the first client world's snapshots with several fixed ACK ages and
multiple clients, checks exact reconstruction, and emits JSON lines. It measures
replication/decoding separately from file parsing. It measures waves 7–12 and
retains preceding frames to establish baselines. Inspect capture metadata and
health before using a capture. This replay complements the Renet impairment eval;
it does not inject loss itself.

## Tests

```bash
cargo test -p game_shared -p game_server --lib --example netcode_eval
python3 scripts/compare-netcode-eval.test.py
```

Tests cover packed modes, truncation, malformed lengths/integers/masks, namespace
and tick boundaries, randomized decoder inputs, exact resize/removal/recreation
recovery, bounded bitset history, shared cache accounting, independent versus
batched baselines, deterministic workload generation, actual loss injection and
post-outage recovery. Full workspace and wasm checks remain required for wire
contract changes.

The server admission limit is configurable through `--max-clients` or
`TD_MAX_CLIENTS` (default 8, transport range 1–1,024). The packet conditioner peer
budget scales with that setting. Increasing this limit does not change entity
limits or establish that the configured workload fits the server's CPU/network
budget.
