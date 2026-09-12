# Persistent real UDP game soak

`scripts/qualify-game-soak.py` supervises one captured Dreamwake server and two
captured native rendered clients under 50 ms delay, 15 ms jitter and 3% independent
loss in each direction. It never builds, recaptures source, or restarts a process.
It prepares the real-UDP bot/resource evidence for T104; its scope is separate from
[N073's generic 50,000-actor resource soak](network-soak.md), the
[bot and human network-profile matrix](network-profile-qualification.md), and the
[clock drift qualification](network-clock-qualification.md).

The runner's implementation and fake-process/file tests are preparation, not a
successful game soak. A day-long result remains unverified until an operator runs
it and returns its complete artifacts. Do not launch an endurance run while
implementation or integration is still changing.

## Captured inputs

Use the final candidate's `source-manifest.json`, read-only `sources/` tree,
`binaries/game_server`, `binaries/dreamwake` and `native-provenance.json`.
Both native executables must be release builds. The supervisor rejects missing,
development or mixed build-profile provenance before launch. The maintained
`scripts/build-native-capture.py` producer records Cargo's emitted executable paths
and optimization evidence, verifies the frozen sources, and copies the pair into
a new native capture directory. A server-only release build is insufficient.
The producer pins and records empty encoded Rust flags so inherited flags cannot
silently override the selected profile's optimization.
The manifest must include the supervisor, clock/source-integrity helpers, native
segmented trace support, lifetime decode classifications, transport identity and
actual replay-work telemetry, and the opt-in autoplay restart behavior. Earlier
captures without those interfaces are unsuitable.

The runner verifies the operator-supplied manifest SHA-256, every captured file,
matching native build provenance and both executable hashes. It repeats those
checks at completion. It rejects added, missing, changed or unsafe source paths
and never relies on Git discovery from a nested capture. Output must be separate
from the read-only source tree and new or empty. All output is private to the
operator by default. Keep identity/proof files private when sharing diagnostic
artifacts.

Set these values from the candidate handoff, not from a moving live checkout:

```sh
export GAME_SOAK_CAPTURE=/absolute/path/to/final-candidate
export GAME_SOAK_MANIFEST_SHA=the_64_character_sha256_from_the_candidate_handoff
cd "$GAME_SOAK_CAPTURE/sources/discordium-td"
```

When assembling a new candidate, build its native pair once:

```sh
python3 scripts/build-native-capture.py \
  --workspace "$GAME_SOAK_CAPTURE/sources/discordium-td" \
  --source-manifest "$GAME_SOAK_CAPTURE/source-manifest.json" \
  --expected-source-sha256 "$GAME_SOAK_MANIFEST_SHA" \
  --output "$GAME_SOAK_CAPTURE/native-build"
```

This produces `native-build/binaries/` and `native-build/native-provenance.json`.
Use that `binaries/` directory with the supervisor, or assemble the pair and its
provenance together at the handoff's documented paths. Never replace artifacts
inside an already handed-off candidate. The commands below use the assembled
`$GAME_SOAK_CAPTURE/binaries` location.

Prepare the exact plan without launching a process:

```sh
python3 scripts/qualify-game-soak.py \
  --source-manifest "$GAME_SOAK_CAPTURE/source-manifest.json" \
  --expected-source-sha256 "$GAME_SOAK_MANIFEST_SHA" \
  --binary-dir "$GAME_SOAK_CAPTURE/binaries" \
  --output "$GAME_SOAK_CAPTURE/manual/game-soak-plan" \
  --prepare-only
```

Review `run-plan.json`, including commands, binary/source identities, hardware,
fixed resource budgets, trace capacity and ports. Preparation performs no build,
network setup or workload. The final handoff should provide the concrete capture
path and hash, replacing the explanatory values above.

## Operator-run preflight and day-long run

A preflight is an explicitly unqualified operator check of startup, native trace
rotation/reading, monitoring and cleanup. It is not automatically run by agents:

```sh
python3 scripts/qualify-game-soak.py \
  --source-manifest "$GAME_SOAK_CAPTURE/source-manifest.json" \
  --expected-source-sha256 "$GAME_SOAK_MANIFEST_SHA" \
  --binary-dir "$GAME_SOAK_CAPTURE/binaries" \
  --output "$GAME_SOAK_CAPTURE/manual/game-soak-preflight" \
  --preflight --seconds 60 --warmup-seconds 10 --report-seconds 10
```

For the final operator-run qualification:

```sh
python3 scripts/qualify-game-soak.py \
  --source-manifest "$GAME_SOAK_CAPTURE/source-manifest.json" \
  --expected-source-sha256 "$GAME_SOAK_MANIFEST_SHA" \
  --binary-dir "$GAME_SOAK_CAPTURE/binaries" \
  --output "$GAME_SOAK_CAPTURE/manual/game-soak-24h"
```

The default measurement lasts at least 86,400 monotonic seconds **after both
clients are active and their current native UDP egress streams are observable**.
Startup is bounded to 120 seconds, with 30 seconds for initial server readiness.
The clients' fallback `--quit-after` includes startup allowance, while the
supervisor independently stops its owned process groups at the measurement
boundary. An early process exit is a failure. No automatic retry, replacement
process or resumption of an interrupted run can turn it into a pass.

The default ports are loopback TCP 18880 and UDP 18881/18882. The latter WebRTC
endpoint is started by the common server but this workload uses native UDP only.
Select three unused ports with `--http-port`, `--udp-port` and `--webrtc-port` if
necessary; the runner does not modify another listener or machine network policy.
A POSIX desktop with `ps`, a functioning display/GPU, an awake session and sufficient
free disk is required. Keep the render/hardware configuration stable and record
it with the handoff. Root/server privileges are not required.

The server uses the normal 80 m base replication distance. Bots use ordinary
spatial-playtest input and normal authority admission, alternating every 12
gameplay seconds between far waypoints `(-80, 0)` / `(80, 0)` and return waypoints
`(-1, 2)` / `(1, 2)`. The 160 m far separation crosses the 90.8 m hero leave
distance, including hysteresis and actor bounds; the return is inside entry
distance. `run-plan.json` records this workload. Endurance qualification still
requires at least one observed scope reentry per client; waypoint intent alone
does not satisfy that gate. The dedicated `scripts/playtest-spatial.py` disclosure
fixture retains its explicit 12 m radius and stricter spatial checks.
Before the final capture is handed off, a short integration run must also cover
two optimized rendered clients at the normal 80 m radius under the same moderate
impairment. Keep the intended window dimensions and inspect sampled frame times
alongside connection health. A pass at 12 m or a single browser client on LAN
does not cover this combined workload.

`--autoplay-restart` repeats
Victory/Defeat using the existing restart action; it does not recreate the process
or hide retained memory. Healthy Combat may persist indefinitely: the supervisor
requires advancing server/gameplay ticks and observed movement, scope reentry and
accepted actions, not periodic victory or room changes. A menu/terminal phase
stuck for 60 seconds, motion stopped for 120 seconds, or no new sampled accepted
action for 300 seconds fails the run. The phase timer begins when a new
phase/room/match is first observed, including an inactive restart/bootstrap
sample. Returning to active in that same phase does not reset its timer;
inactive samples never extend the separate 10-second inactivity budget.
The sampled last-eight action window can
miss events; this activity check is not exhaustive action-delivery accounting.

The game seed is 8192 unless changed explicitly. The compiled conditioner seed is
1; transport identities derive per-peer seeds. The report records this distinction.
The game seed does not make operating-system packet scheduling byte-identical.
The fixed moderate conditioner is not the full network-profile matrix.

## Finite recording and fail conditions

Each client writes one row every two seconds by default. Up to 128 append-only
8 MiB chunks are retained: `left.ndjson`, `left.ndjson.0001`, and so on; similarly
for right. No chunk is overwritten, deleted or wrapped. The hard total is 1 GiB
per client. Exhaustion or I/O/encoding failure emits `DREAMWAKE_TRACE_ERROR` and
terminates the client; the supervisor fails rather than silently dropping data.
Sampling follows rendered frames, so missing samples and elapsed-time gaps are
also checked. Rotating an external file cannot bypass the client's finite budget.

The reader consumes bounded blocks, checks contiguous lifetime sequence numbers,
segment identities, ordinary-file/inode continuity, complete JSON rows and a
1 MiB row ceiling. It verifies all consumed file hashes after shutdown. The
measurement cutoff is the byte prefix visible before stopping the clients;
backlogged rows in that prefix are still audited during final drain. Fully later
rows are retained and counted separately, so intentional shutdown errors cannot
be mistaken for a measured failure or used to hide pre-cutoff failures.

The following defaults are declared supervisor budgets, not assigned
reference-hardware performance claims:

| Measurement | Default bound |
|---|---:|
| Per-process RSS | 2 GiB |
| Post-300-second-warmup RSS growth | 128 MiB |
| Post-warmup RSS linear growth trend | 4 MiB/hour |
| Client prediction history | 64 MiB |
| Client scope payload | 4 MiB |
| Client staging / baselines / interpolation | 16 MiB each |
| Pending commands / sampled scope identities per stream | 256 / 8,192 |
| Sampled actual replay work | 32 ticks |
| Server control/state queue payload | 16 MiB each |
| Estimated-minus-checkpoint and predicted-minus-estimated phase | ±128 ticks |
| Estimated-minus-checkpoint phase trend | ±1 tick/hour |
| Post-activation inactivity / trace gap / halted server or Combat tick | 10 seconds |
| Supervisor scheduling gap | 5 seconds |
| Independent-clock step and cumulative elapsed divergence | 1 second |
| Per-process stdout/stderr | 256 MiB |
| Resource and aggregate interval logs | 64 MiB each |
| Unused disk reserve during run | 256 MiB |

RSS and trend limits have explicit CLI options and must be fixed before execution.
The runner requires enough initial free space for both maximum trace budgets,
three process-log budgets, two report budgets and the reserve, approximately
3.1 GiB under defaults. It samples RSS and free disk every five seconds. A missing
RSS measurement, changed process start/executable identity, exceeded budget,
write error, hard decode failure or unavailable current transport telemetry fails
qualification. Source/binary/provenance drift also fails.

The lifetime decode counters must satisfy `decode_errors = stale_epoch_rejections
+ clock_resync_rejections + obsolete_rejections + decode_failures`; hard failures
must stay zero. Retired replication scopes, publications and baselines have their
own `obsolete_rejections` counter.
Expected `WrongEpoch` and typed clock resynchronization rejections remain visible
as separate monotonic counters, including during inactive startup and recovery.
Clock resynchronization is not malformed clock data: invalid clock messages still
count as hard failures. No resynchronization quota replaces the existing activity
and clock-progress budgets, and no rejection class can mask a hard failure.
Transport peer ID
is read from `transport_connection`, never inferred from the independently
advancing `connection_epoch`. Current transport logs have a bounded reporting
grace during recovery; retired-peer observations remain counted without growing
an unbounded peer map. Scope identity history respects match and stream namespaces.

The supervisor compares monotonic elapsed time to Linux's sleep-inclusive
`CLOCK_BOOTTIME`, or `time.time` where unavailable. Both per-step and cumulative
origin divergence are checked; repeated small sleeps cannot evade a single-step
threshold. A wall-clock adjustment on a fallback host invalidates the run rather
than guessing that continuity held. This continuity check does not inject drift
into the game's clock estimator.

## Artifacts and verdict scope

`run-plan.json` records the exact unexecuted configuration; `report.json` records
status, PIDs, cleanup, source/binary integrity, complete trace hashes, aggregate
counters and failure reasons. Preserve both even on failure. `resources.jsonl`
contains raw per-process RSS/start identity observations every five seconds.
`intervals.jsonl` contains client resource/lifecycle/activity/clock measurements,
server egress counters, trace/log byte counts and rolling quantiles each minute.
Raw client chunks and the server/client stdout/stderr logs remain available for
independent review. Quantiles are exact over the most recent 256 samples, with
lifetime count/min/max/mean; they are not mislabeled lifetime p99 values.

A successful preflight reports `preflight_passed=true` and always leaves
`real_udp_24h_passed=false`. A scoped day-long pass requires the entire duration,
immutable inputs, clean planned shutdown, complete trace prefixes, all budgets,
at least 90% of the 60 Hz server-tick progression, observed movement/scope reentry
and accepted actions, and sufficient bounded RSS/clock trend samples. An
interrupted, suspended, timed-out, failed or shortened run remains incomplete.

This is sampled real-UDP evidence for two persistent native bots under one
moderate profile. It does not exhaustively prove absence of every duplicate
committed action: retain the authority/transport contract tests alongside it.
The bounded last-eight action windows and 1,024-key sampled terminal-consistency
cache are diagnostic observations, not a complete action ledger. Likewise,
estimated clock minus the last received checkpoint includes delivery age; it is
not a one-way-delay estimate or the explicit positive/negative drift fixture.
A scoped pass does not qualify WebRTC, human sessions, all profiles, N073's generic
100-connection / 50,000-actor workload, or unassigned hardware/visual-quality gates.
