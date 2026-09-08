# Server ingress experiment — 2026-09-08

This experiment replaces the network worker's mutex mailbox with a bounded
`rtrb 0.4` SPSC ring, decodes directly from Renet's `Bytes`, and transfers owned
commands to `PreUpdate`. Gameplay still executes at 30 Hz in `FixedUpdate`.
It removes the byte copy and repeated movement-bundle decode. The worker retains
its blocking one-millisecond output wait; neither endpoint spins on the ring.

## Focused decode and handoff benchmark

```bash
cargo run --release -p game_server --example ingress_profile -- \
  --batches 1000 --warmup 100 --repetitions 5 \
  --output target/reports/server-ingress/2026-09-08/micro.json
```

The example compares four paths using identical encoded inputs. Two threads
rendezvous outside timed regions. Measurements include decoding, allocation and
ownership transfer, but exclude sockets, ECS staging, simulation, and mutex
contention. These are component costs, not server frame times or throughput.

Below are medians of each repetition's median, in microseconds per batch of
128 messages (32 peers, four messages each). “Large” bundles contain 16 moves
and four actions; small bundles contain one move and no actions.

| Path | Small producer | Small consumer | Large producer | Large consumer |
| --- | ---: | ---: | ---: | ---: |
| Mutex, copied bytes, decode twice | 6.667 | 6.583 | 388.500 | 389.958 |
| Mutex, owned Bytes, decode twice | 4.459 | 4.959 | 391.958 | 388.167 |
| Mutex, decoded commands | 9.500 | 1.708 | 380.750 | 3.750 |
| SPSC, decoded commands | 4.042 | 1.584 | 379.875 | 3.791 |

For the large batch, summed producer/consumer medians fall from about 778 to
384 microseconds (51%). Almost all of that saving comes from decoding once:
the decoded mutex and SPSC paths are essentially equal here. Removing just the
byte copy helps small inputs, but does not overcome large-input decoding cost.
The mailbox variants use batch vectors, whereas the ring transfers individual
entries, so their allocation patterns also differ.

## Live UDP comparison

The immediate baseline is the exact workspace captured before this experiment,
including its existing dedicated transport worker. A historical implementation
from the preceding experiment is a separate reference; its broader architecture
differs and cannot isolate the ring's effect.

Release binaries run on the same Apple M3 Max host (64 GiB RAM, macOS/aarch64). The fixture seeds a fixed
number of durable enemies and heroes before input staging. Actual UDP clients
send phase-distributed 30 Hz movement bundles with three recent moves, occasional
actions, and replication ACKs. Each measurement follows fixture setup and five
seconds of warm-up, then measures 15 seconds. Actor counts, decoding, replication
baselines and delivery are checked for every client. No builds or tests run
during the timed samples.

CPU is server process user + system time divided by wall time, where 100% means
one core. macOS process counters are converted using the host Mach timebase.
Movement ACK latency is measured by the synthetic clients; it includes network,
input phase, simulation queueing and snapshot delivery. It is not a rendered
client responsiveness measurement.

The final candidate skips Bevy resource removal/reinsertion on empty polls.
Two alternating repetitions per workload compare that version with the immediate
baseline. Values below are medians across repetitions.

| Players / enemies | Baseline CPU | Final CPU | Baseline ACK p50 / p95 | Final ACK p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| 8 / 256 | 12.84% | 12.69% | 23.57 / 39.37 ms | 24.83 / 40.57 ms |
| 32 / 1,024 | 31.86% | 31.81% | 28.09 / 41.41 ms | 27.11 / 40.90 ms |

There is no material whole-server CPU or latency improvement in these samples.
CPU differences are only 0.15 and 0.04 percentage points. Final median RSS was
20.30 / 26.44 MiB, versus 20.15 / 25.81 MiB for the baseline.

The initial candidate entered the resource scope even on empty polls. Three
alternating repetitions showed CPU of 13.20% versus 12.56% at 8 players, and
31.77% versus 31.76% at 32 players. The final paired runs no longer show that
small CPU increase; the short runs do not isolate causality conclusively.

The historical implementation, rerun three times in the initial matrix, used
12.17% / 30.05% CPU at 8 / 32 players, with ACK p50/p95 of 38.39/61.65 and
40.05/63.41 ms. It used less CPU but had higher ACK latency. This is a broader
architecture comparison, not an isolated benefit of SPSC.

All 30 valid live runs sustained approximately 30 snapshots/second, with no
client decode errors or missing replication baselines. Single-run checks of the
initial candidate versus baseline also covered 1 player / 84 enemies and
8 players / 256 enemies with 150 ms added each way. The latter sustained 30 Hz,
with ACK p50/p95 of 330.5/347.0 versus 328.9/343.4 ms. Those checks preceded the
final empty-poll adjustment and are not additional final-version repetitions.

## Admission limits

The inherited per-peer quotas remain 16 reliable and 32 unreliable messages per
fixed tick, including invalid traffic and ACKs. Ring capacity is
`max_clients * (16 + 32 + 2) + 1`: command quotas plus lifecycle and metrics room.
The 256-entry staging cap and soft 500-microsecond deadline are initial
safeguards, not empirically optimal values. Every pass captures the published
range once so arrivals cannot extend it. Deadline checks occur between entries;
connection setup can exceed the soft deadline by itself.

Across the valid SPSC runs, peak published backlog was 40 entries, so none
reached the 256-entry cap. The initial candidate recorded eight deferred passes
in the six main runs (some during connection/setup), and none in the two extra
checks. The four final-candidate runs recorded one deadline deferral in total,
during an 8-player measurement; their maximum backlog was 32. These are soft
deadline deferrals, not dropped messages. The counters do not measure the exact
extra staging latency. The results do not establish overload behavior or justify
raising/lowering the caps.

## Validation and artifacts

`just check`, the workspace tests (150 tests), the wasm client check and the six
netcode analysis tests passed. One conditioner outage test failed on the first
workspace run; its isolated rerun and the full workspace rerun passed. Its
assertion now includes the disconnect reason to make future failures diagnosable.
The server library tests were rerun after the final empty-poll adjustment.

Local raw results, binary hashes, harness sources and source snapshots are under
`target/reports/server-ingress/2026-09-08/`. The saved immediate baseline's 105
files were reconstructed without benchmark instrumentation and checked against
the hashes captured before editing. Benchmark instrumentation is confined to
copied source trees; it is not part of the production server.

Five early runs restored the fixture after staging input, which invalidated
latency comparisons by clearing queued input during setup. Those runs were
excluded and retained separately. Reported live results use the corrected
fixture before staging.

These short localhost runs do not establish production capacity, browser/WebRTC
performance, sustained overload behavior, or a statistically reliable small CPU
difference. The focused benchmark deliberately stresses redundant decoding;
normal live input has much less of that work.
